#![no_std]

//! Async pools of **1..=32** buffers for `no_std` firmware.
//!
//! [`BufferPool`] borrows a slice and can be constructed at runtime.
//! [`StaticBufferPool`] owns an array and has a const constructor. Both return
//! [`BufferGuard`] values that keep a physical slot reserved until dropped.
//! Neither acquisition nor mapping allocates.
//!
//! # Runtime storage
//!
//! ```
//! use embedded_buffer_pool::{BufferGuard, BufferPool};
//! use embassy_sync::blocking_mutex::raw::NoopRawMutex;
//!
//! let mut items = [[0u8; 64]; 2];
//! let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
//! let mut slice = BufferGuard::map(pool.try_take().unwrap(), |b| &mut b[..16]);
//! slice[0] = 0xAA;
//! drop(slice);
//! assert_eq!(pool.try_take().unwrap()[0], 0xAA);
//! ```
//!
//! # Owning static storage
//!
//! ```
//! use embedded_buffer_pool::{BufferPool, StaticBufferPool};
//! use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
//!
//! static POOL: StaticBufferPool<CriticalSectionRawMutex, [u8; 64], 2> =
//!     StaticBufferPool::new([[0; 64]; 2]);
//!
//! async fn fill(pool: &BufferPool<'_, CriticalSectionRawMutex, [u8; 64]>) {
//!     let mut buffer = pool.take().await;
//!     buffer[0] = 0xBB;
//! }
//!
//! async fn example() {
//!     fill(&POOL).await; // Deref coercion supplies the borrowed pool view.
//! }
//! ```
//!
//! Use an inline const expression such as `[const { Thing::new() }; 16]` to
//! construct arrays of non-`Copy` values in a const context.
//!
//! # Waiting
//!
//! Each pool stores one waker. A different waiting task replaces and wakes the
//! previous registration, so several waiting tasks can cause contention;
//! acquisition order is not guaranteed. [`BufferPool::try_take`] can claim a free
//! slot even while tasks are waiting. Canceling an acquisition may leave a
//! harmless stale waker. Forgetting a guard retains its slot. Choose a [`RawMutex`]
//! suitable for the executor and interrupt contexts sharing the pool.

use core::cell::{RefCell, UnsafeCell};
use core::future::{Future, poll_fn};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::task::{Poll, Waker};

use embassy_sync::blocking_mutex::{Mutex, raw::RawMutex};

struct ItemPtr(*mut ());

// SAFETY: the erased pointer is only read or updated under the state mutex. It
// never grants item access by itself; BufferPool's Send/Sync bounds and storage
// lifetime account for T, and only a claimed slot is turned into a reference.
unsafe impl Send for ItemPtr {}

struct State {
    items: ItemPtr,
    /// Bit i is set exactly when physical slot i is free.
    available: u32,
    waker: Option<Waker>,
}

impl State {
    /// Caller has checked that at least one slot is available.
    fn claim(&mut self) -> usize {
        let index = self.available.trailing_zeros() as usize;
        self.available &= !(1 << index);
        index
    }
}

/// A pool borrowing **1..=32** values, synchronized with mutex `M`.
///
/// `'items` is the lifetime of the backing slice. Acquired guards and pending
/// futures borrow the pool itself, so they cannot outlive or move it.
///
/// ```compile_fail
/// use embedded_buffer_pool::BufferPool;
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// let guard;
/// {
///     let mut items = [0u8];
///     let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
///     guard = pool.try_take().unwrap();
/// }
/// assert_eq!(*guard, 0); // The pool and its storage have been dropped.
/// ```
///
/// ```compile_fail
/// use embedded_buffer_pool::BufferPool;
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// let mut items = [0u8];
/// let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
/// items[0] = 1; // The pool exclusively borrows the backing slice.
/// let _guard = pool.try_take();
/// ```
///
/// ```compile_fail
/// use embedded_buffer_pool::BufferPool;
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// let pending;
/// {
///     let mut items = [0u8];
///     let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
///     pending = pool.take();
/// }
/// drop(pending); // A future also borrows its pool.
/// ```
pub struct BufferPool<'items, M: RawMutex, T> {
    state: Mutex<M, RefCell<State>>,
    capacity: u8,
    items: PhantomData<&'items mut [T]>,
}

// SAFETY: transferring a pool transfers its exclusive backing-storage borrow.
// Access to shared bookkeeping is synchronized by M.
unsafe impl<M: RawMutex + Send, T: Send> Send for BufferPool<'_, M, T> {}
// SAFETY: the bitmap grants exclusive access to disjoint physical slots; sharing
// the pool does not share references to T. Moving a slot between threads needs
// T: Send, while the mutex must support shared access.
unsafe impl<M: RawMutex + Sync, T: Send> Sync for BufferPool<'_, M, T> {}

impl<'items, M: RawMutex, T> BufferPool<'items, M, T> {
    /// Borrows a slice with all slots initially available.
    ///
    /// Panics unless the slice length is in **1..=32**. In const evaluation an
    /// invalid length is a compilation error. T need not be Copy or `'static`.
    pub const fn new(items: &'items mut [T]) -> Self {
        let len = items.len();
        assert!(len > 0 && len <= 32, "buffer count must be in 1..=32");
        Self::from_raw(items.as_mut_ptr(), len)
    }

    // The pointer must cover len exclusively borrowed items before this pool is
    // exposed. StaticBufferPool keeps its unbound pool private until Deref binds it.
    const fn from_raw(items: *mut T, len: usize) -> Self {
        Self {
            state: Mutex::new(RefCell::new(State {
                items: ItemPtr(items.cast()),
                available: u32::MAX >> (32 - len),
                waker: None,
            })),
            capacity: len as u8,
            items: PhantomData,
        }
    }

    /// Total number of slots, including currently reserved slots.
    pub const fn capacity(&self) -> usize {
        self.capacity as usize
    }

    /// Number of slots currently free. This is a snapshot, not a reservation.
    pub fn available(&self) -> usize {
        self.state
            .lock(|state| state.borrow().available.count_ones() as usize)
    }

    /// Claims a free slot immediately, including when async tasks are waiting.
    // Interior mutability is checked by the slot bitmap; this is a guard, not an
    // unrestricted mutable reference derived from shared input.
    #[allow(clippy::mut_from_ref)]
    pub fn try_take(&self) -> Option<BufferGuard<'_, M, &mut T>> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.available == 0 {
                return None;
            }
            let index = state.claim();
            // SAFETY: the bit was cleared under this mutex. The pointer covers
            // the backing storage and only this slot is borrowed, not the slice.
            Some(unsafe { self.guard(state.items.0.cast::<T>().add(index), index) })
        })
    }

    /// Waits infallibly for a slot using one shared waker registration.
    ///
    /// A new task replaces and wakes the previous waiter. Wake notifications
    /// happen after unlocking; polling and slot publication cannot lose a wake.
    /// Dropping a pending future may leave its waker registered until replacement
    /// or the next release. Acquisition order is not guaranteed.
    pub fn take(&self) -> impl Future<Output = BufferGuard<'_, M, &mut T>> + '_ {
        poll_fn(move |cx| {
            // Waker cloning, dropping, and waking can execute user code. Keep
            // those operations outside the mutex and its RefCell borrow.
            let mut new_waker = Some(cx.waker().clone());
            let (result, replaced) = self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.available != 0 {
                    let index = state.claim();
                    // SAFETY: same exclusive-slot invariant as try_take.
                    let guard = unsafe { self.guard(state.items.0.cast::<T>().add(index), index) };
                    return (Poll::Ready(guard), None);
                }
                if state
                    .waker
                    .as_ref()
                    .is_some_and(|old| old.will_wake(cx.waker()))
                {
                    return (Poll::Pending, None);
                }
                let replaced = core::mem::replace(&mut state.waker, new_waker.take());
                (Poll::Pending, replaced)
            });
            if let Some(waker) = replaced {
                waker.wake();
            }
            result
        })
    }

    // SAFETY: items points to the exclusively claimed slot and remains valid
    // for the pool borrow. Its bit stays clear until the token is dropped.
    #[allow(clippy::mut_from_ref)]
    unsafe fn guard(&self, items: *mut T, index: usize) -> BufferGuard<'_, M, &mut T> {
        BufferGuard {
            value: items,
            token: ReleaseToken {
                state: &self.state,
                index,
            },
            reference: PhantomData,
        }
    }
}

/// An owning pool with a const constructor, suitable for a `static` item.
///
/// Dereferences to [`BufferPool`], so functions taking `&BufferPool` accept
/// `&StaticBufferPool`. N determines the owned array's layout and must be in
/// **1..=32**. T need not be Copy, but must be `'static` for this owning type.
/// The wrapper can also be used locally and moved between borrows.
///
/// ```compile_fail
/// use embedded_buffer_pool::StaticBufferPool;
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// let pool = StaticBufferPool::<NoopRawMutex, _, 1>::new([0u8]);
/// let guard = pool.try_take().unwrap();
/// let moved = pool; // A usable guard keeps the owner borrowed.
/// assert_eq!(*guard, 0);
/// ```
pub struct StaticBufferPool<M: RawMutex, T: 'static, const N: usize> {
    // Drop the pool's bookkeeping (including any stale waker) before the items.
    pool: BufferPool<'static, M, T>,
    items: UnsafeCell<[T; N]>,
}

// SAFETY: the owned items are accessed only through the mutex-protected slot
// bitmap. Pointer binding is also protected by that mutex, and borrows of this
// owner keep its storage immobile while guards/futures can use it.
unsafe impl<M: RawMutex + Sync, T: Send, const N: usize> Sync for StaticBufferPool<M, T, N> {}

impl<M: RawMutex, T, const N: usize> StaticBufferPool<M, T, N> {
    /// Owns an array with all slots initially available.
    ///
    /// Panics unless N is in **1..=32**, or fails const evaluation in a static.
    pub const fn new(items: [T; N]) -> Self {
        assert!(N > 0 && N <= 32, "buffer count must be in 1..=32");
        Self {
            pool: BufferPool::from_raw(core::ptr::null_mut(), N),
            items: UnsafeCell::new(items),
        }
    }
}

impl<M: RawMutex, T, const N: usize> Deref for StaticBufferPool<M, T, N> {
    type Target = BufferPool<'static, M, T>;

    fn deref(&self) -> &Self::Target {
        // Never create &mut [T; N]: other physical slots may have live guards.
        // Refresh on every exposure, because a local owner can move between
        // borrows. No safe API exposes the unbound pool or moves it out.
        self.pool.state.lock(|state| {
            state.borrow_mut().items = ItemPtr(self.items.get().cast());
        });
        &self.pool
    }
}

struct ReleaseToken<'pool, M: RawMutex> {
    state: &'pool Mutex<M, RefCell<State>>,
    index: usize,
}

impl<M: RawMutex> Drop for ReleaseToken<'_, M> {
    fn drop(&mut self) {
        let waker = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.available |= 1 << self.index;
            state.waker.take()
        });
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Owns the reservation of one physical pool slot.
///
/// A `BufferGuard<&mut T>` dereferences to T and supports mutation; a
/// `BufferGuard<&T>` dereferences to T with shared access. The reference payload
/// stays private: references obtained through Deref borrow the guard, so they
/// cannot remain usable after its slot is released.
///
/// Use [`BufferGuard::map`] for mutable projection, [`BufferGuard::map_mut_ref`]
/// to mutate before projecting to shared access, and [`BufferGuard::map_ref`]
/// for shared projection. A struct can own a guard to retain its slot across
/// function returns or await points:
///
/// ```
/// use embedded_buffer_pool::{BufferGuard, BufferPool};
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// struct Packet<'pool> {
///     buffer: BufferGuard<'pool, NoopRawMutex, &'pool mut [u8]>,
/// }
/// fn packet<'pool>(pool: &'pool BufferPool<'_, NoopRawMutex, [u8; 64]>)
///     -> Packet<'pool>
/// {
///     Packet { buffer: BufferGuard::map(pool.try_take().unwrap(), |b| &mut b[..]) }
/// }
/// let mut items = [[0; 64]];
/// let pool = BufferPool::new(&mut items);
/// let mut packet = packet(&pool);
/// packet.buffer[0] = 42;
/// assert_eq!(pool.available(), 0);
/// drop(packet);
/// assert_eq!(pool.available(), 1);
/// ```
///
/// ```compile_fail
/// use embedded_buffer_pool::{BufferGuard, BufferPool};
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// let mut items = [[0u8; 4]];
/// let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
/// let mut shared = BufferGuard::map_ref(pool.try_take().unwrap(), |b| &b[..]);
/// shared[0] = 1; // A shared guard has no DerefMut.
/// ```
///
/// ```compile_fail
/// use embedded_buffer_pool::{BufferGuard, BufferPool};
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// let mut items = [0u8];
/// let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
/// let shared = BufferGuard::map_ref(pool.try_take().unwrap(), |b| b);
/// let escaped = &*shared;
/// drop(shared);
/// assert_eq!(*escaped, 0); // References borrow the guard, not just the pool.
/// ```
///
/// Guard thread safety follows both the reference type and the mutex:
///
/// ```compile_fail
/// use core::cell::Cell;
/// use embedded_buffer_pool::BufferGuard;
/// use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
/// fn require_sync<T: Sync>() {}
/// require_sync::<BufferGuard<'static, CriticalSectionRawMutex, &'static mut Cell<u8>>>();
/// ```
///
/// ```compile_fail
/// use core::cell::Cell;
/// use embedded_buffer_pool::BufferGuard;
/// use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
/// fn require_send<T: Send>() {}
/// require_send::<BufferGuard<'static, CriticalSectionRawMutex, &'static Cell<u8>>>();
/// ```
///
/// ```compile_fail
/// use embedded_buffer_pool::BufferGuard;
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
/// fn require_send<T: Send>() {}
/// require_send::<BufferGuard<'static, NoopRawMutex, &'static mut u8>>();
/// ```
///
/// ```compile_fail
/// use std::rc::Rc;
/// use embedded_buffer_pool::BufferGuard;
/// use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
/// fn require_send<T: Send>() {}
/// require_send::<BufferGuard<'static, CriticalSectionRawMutex, &'static mut Rc<u8>>>();
/// ```
pub struct BufferGuard<'pool, M: RawMutex, V: Deref> {
    // V is a reference type describing access, rather than a stored reference.
    // An actual &mut T here would be protected for the entire mem::drop call,
    // including token release, preventing a woken task from reclaiming the slot.
    // The pointer's referent remains valid for 'pool; Deref only makes short loans.
    value: *mut V::Target,
    token: ReleaseToken<'pool, M>,
    reference: PhantomData<V>,
}

// SAFETY: the pointer represents the reference V, whose Send/Sync bounds specify
// whether its referent may be accessed on another thread. Transferring/sharing
// the release token requires a mutex that can be shared. Deref loans remain tied
// to the guard; ownership of the physical reservation is never duplicated.
unsafe impl<M: RawMutex + Sync, V: Deref + Send> Send for BufferGuard<'_, M, V> {}
unsafe impl<M: RawMutex + Sync, V: Deref + Sync> Sync for BufferGuard<'_, M, V> {}

impl<M: RawMutex, V: Deref> Deref for BufferGuard<'_, M, V> {
    type Target = V::Target;

    fn deref(&self) -> &Self::Target {
        // SAFETY: the token retains this slot and the reference marker retains
        // its backing-storage lifetime. The returned loan borrows this guard.
        unsafe { &*self.value }
    }
}

impl<M: RawMutex, V: DerefMut> DerefMut for BufferGuard<'_, M, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: only mutable-reference payloads implement DerefMut. This
        // exclusive borrow of the guard prevents overlapping guard-derived loans.
        unsafe { &mut *self.value }
    }
}

impl<'pool, M: RawMutex, T: ?Sized> BufferGuard<'pool, M, &'pool mut T> {
    /// Projects to a mutable field or slice, retaining the original slot.
    ///
    /// Call `BufferGuard::map(guard, f)`. This is an associated function so it
    /// does not shadow a map method on the inner T. The closure must work for
    /// any input borrow lifetime; it cannot save the buffer reference elsewhere.
    /// If it panics, the original slot is released.
    ///
    /// ```compile_fail
    /// use embedded_buffer_pool::{BufferGuard, BufferPool};
    /// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    /// let mut items = [0u8];
    /// let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    /// let mut escaped = None;
    /// let mapped = BufferGuard::map(pool.try_take().unwrap(), |buffer| {
    ///     escaped = Some(buffer); // A projection cannot leak its input borrow.
    ///     escaped.as_deref_mut().unwrap()
    /// });
    /// drop(mapped);
    /// let _ = escaped;
    /// ```
    pub fn map<U: ?Sized>(
        orig: Self,
        fun: impl for<'a> FnOnce(&'a mut T) -> &'a mut U,
    ) -> BufferGuard<'pool, M, &'pool mut U> {
        // SAFETY: orig retains exclusive access to its slot. The closure's
        // higher-ranked borrow cannot escape, and ends before token transfer.
        let value = fun(unsafe { &mut *orig.value }) as *mut U;
        let Self { token, .. } = orig;
        BufferGuard {
            value,
            token,
            reference: PhantomData,
        }
    }

    /// Projects a mutable guard to shared access, retaining the original slot.
    ///
    /// Call `BufferGuard::map_mut_ref(guard, f)` when the projection needs mutable
    /// access but returns a shared reference, such as `Thing::do_sth(&mut self)
    /// -> &[u8]`. The resulting guard supports only shared access and can be
    /// projected further with [`BufferGuard::map_ref`].
    ///
    /// The closure must work for any input borrow lifetime; it cannot save the
    /// buffer reference elsewhere. If it panics, the original slot is released.
    ///
    /// ```
    /// use embedded_buffer_pool::{BufferGuard, BufferPool};
    /// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    /// struct Thing([u8; 4]);
    /// impl Thing {
    ///     fn do_sth(&mut self) -> &[u8] {
    ///         self.0[1] = 42;
    ///         &self.0[1..3]
    ///     }
    /// }
    /// let mut items = [Thing([0; 4])];
    /// let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    /// let bytes = BufferGuard::map_mut_ref(pool.try_take().unwrap(), Thing::do_sth);
    /// assert_eq!(&*bytes, &[42, 0]);
    /// assert_eq!(pool.available(), 0);
    /// drop(bytes);
    /// assert_eq!(pool.available(), 1);
    /// ```
    ///
    /// ```compile_fail
    /// use embedded_buffer_pool::{BufferGuard, BufferPool};
    /// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    /// let mut items = [[0u8; 4]];
    /// let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    /// let mut shared = BufferGuard::map_mut_ref(pool.try_take().unwrap(), |b| &b[..]);
    /// shared[0] = 1; // The projected guard has no DerefMut.
    /// ```
    ///
    /// ```compile_fail
    /// use embedded_buffer_pool::{BufferGuard, BufferPool};
    /// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    /// let mut items = [[0u8; 4]];
    /// let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    /// let mut escaped = None;
    /// let shared = BufferGuard::map_mut_ref(pool.try_take().unwrap(), |b| {
    ///     escaped = Some(&b[..]); // The input borrow cannot escape the closure.
    ///     &b[..]
    /// });
    /// drop(shared);
    /// let _ = escaped;
    /// ```
    ///
    /// ```compile_fail
    /// use embedded_buffer_pool::{BufferGuard, BufferPool};
    /// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    /// let mut items = [[0u8; 4]];
    /// let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    /// let shared = BufferGuard::map_mut_ref(pool.try_take().unwrap(), |b| &b[..]);
    /// let escaped = &*shared;
    /// drop(shared);
    /// assert_eq!(escaped[0], 0); // References borrow the projected guard.
    /// ```
    pub fn map_mut_ref<U: ?Sized>(
        orig: Self,
        fun: impl for<'a> FnOnce(&'a mut T) -> &'a U,
    ) -> BufferGuard<'pool, M, &'pool U> {
        // SAFETY: orig retains exclusive access to its slot. The closure's
        // higher-ranked borrow cannot escape, and ends before token transfer.
        let value = fun(unsafe { &mut *orig.value }) as *const U;
        let Self { token, .. } = orig;
        BufferGuard {
            // Preserve the shared projection's provenance and metadata. The
            // shared reference marker prevents mutable access to this pointer.
            value: value.cast_mut(),
            token,
            reference: PhantomData,
        }
    }
}

impl<'pool, M: RawMutex, V: Deref> BufferGuard<'pool, M, V> {
    /// Projects a mutable or shared guard to shared access, retaining its slot.
    ///
    /// Call `BufferGuard::map_ref(guard, f)`. This also chains shared projections.
    /// For a projection requiring mutable input, use [`BufferGuard::map_mut_ref`].
    /// The returned reference is tied to the input borrow and cannot escape into
    /// the closure's captured state. If the closure panics, the slot is released.
    pub fn map_ref<U: ?Sized>(
        orig: Self,
        fun: impl for<'a> FnOnce(&'a V::Target) -> &'a U,
    ) -> BufferGuard<'pool, M, &'pool U> {
        // SAFETY: orig retains its physical slot. The closure can only return a
        // reference valid for this input borrow and cannot leak it elsewhere.
        let value = fun(unsafe { &*orig.value }) as *const U;
        let Self { token, .. } = orig;
        BufferGuard {
            // Retain the projection's provenance/metadata without making a
            // long-lived reference. The shared marker prohibits DerefMut.
            value: value.cast_mut(),
            token,
            reference: PhantomData,
        }
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
