#![no_std]

//! Fixed-size [`BufferPool`] for [`no_std`] firmware using [`embassy_sync`].
//!
//! The pool holds `N` values of type `T` (**1..=32** slots). A bitmask tracks which
//! slots are free; [`BufferGuard`] and [`MappedBufferGuard`] return a slot to the pool on
//! [`Drop`]. When the pool is empty, [`BufferPool::take`] registers a waker and completes
//! when another task releases a buffer.
//!
//! # Requirements
//!
//! - Acquire APIs take `&'static self`: the pool is intended to live in a `static` item.
//! - Choose a [`RawMutex`] (`M`) compatible with your executor (for example the
//!   critical-section mutex in `embassy_sync`).
//! - `T` must be [`Copy`] because [`BufferPool::new`] is a `const fn`.
//!
//! # Example
//!
//! ```rust,ignore
//! use embedded_buffer_pool::BufferPool;
//! use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
//!
//! /// Shared pool: two 64-byte packet buffers.
//! static POOL: BufferPool<CriticalSectionRawMutex, [u8; 64], 2> =
//!     BufferPool::new([[0u8; 64]; 2]);
//!
//! fn try_fill() -> Option<()> {
//!     let mut guard = POOL.try_take()?;
//!     guard[0] = 0xAA;
//!     Some(())
//! }
//!
//! async fn wait_for_buffer() {
//!     let mut guard = POOL.take().await;
//!     guard[0] = 0xBB;
//! }
//! ```
//!
//! [`embassy_sync`]: https://docs.rs/embassy-sync
//! [`no_std`]: https://doc.rust-lang.org/nomicon/no-std.html
//!
//! For arrays built by repeating an expression (constructors, `const` values, literals),
//! see [`array_new`].

mod macros;

use core::cell::UnsafeCell;
use core::future::poll_fn;
use core::ops::{Deref, DerefMut};
use core::task::Poll;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    waitqueue::WakerRegistration,
};

#[repr(transparent)]
#[derive(Debug)]
struct BufferPtr<T: ?Sized>(*mut T);

unsafe impl<T: ?Sized> Send for BufferPtr<T> {}
unsafe impl<T: ?Sized> Sync for BufferPtr<T> {}

struct State {
    /// Bitmask of free slots: bit `i` set means slot `i` is available.
    available: u32,
    waker: WakerRegistration,
}

/// Pool of `N` buffers of type `T`, synchronized with mutex `M`.
///
/// Free slots are tracked with a `u32` bitmask, so **`N` must be in 1..=32**.
pub struct BufferPool<M: RawMutex, T, const N: usize> {
    buffer: UnsafeCell<[T; N]>,
    state: Mutex<M, State>,
}

unsafe impl<M: RawMutex + Send, T: Send, const N: usize> Send for BufferPool<M, T, N> {}
unsafe impl<M: RawMutex + Sync, T: Send, const N: usize> Sync for BufferPool<M, T, N> {}

impl<M: RawMutex, T: Copy, const N: usize> BufferPool<M, T, N> {
    /// Creates a pool with the given backing storage; all slots start available.
    ///
    /// # Valid `N`
    ///
    /// `N` must be in **1..=32**. Otherwise [`BufferPool::new`] panics when run, or fails
    /// const evaluation if used in a `const` item.
    pub const fn new(buffer: [T; N]) -> Self {
        assert!(N > 0 && N <= 32);
        Self {
            buffer: UnsafeCell::new(buffer),
            state: Mutex::new(State {
                available: u32::MAX >> (32 - N),
                waker: WakerRegistration::new(),
            }),
        }
    }

    /// Tries to take one buffer without blocking. Returns [`None`] if the pool is empty.
    pub fn try_take(&'static self) -> Option<BufferGuard<M, T>> {
        unsafe {
            self.state.lock_mut(|state| {
                if state.available == 0 {
                    return None;
                }
                let index = state.available.trailing_zeros() as usize;
                state.available &= !(1 << index);
                let buffer = &mut (*self.buffer.get())[index];
                Some(BufferGuard {
                    store: &self.state,
                    ptr: BufferPtr(buffer),
                    index,
                })
            })
        }
    }

    /// Waits until a buffer is available, then returns a guard.
    ///
    /// If the pool is empty, the current task’s waker is registered; when another task
    /// drops a [`BufferGuard`] or [`MappedBufferGuard`], waiters are woken.
    pub fn take(&'static self) -> impl Future<Output = BufferGuard<M, T>> {
        poll_fn(|cx| unsafe {
            self.state.lock_mut(|state| {
                if state.available == 0 {
                    state.waker.register(cx.waker());
                    return Poll::Pending;
                }
                let index = state.available.trailing_zeros() as usize;
                state.available &= !(1 << index);
                let buffer = &mut (*self.buffer.get())[index];
                Poll::Ready(BufferGuard {
                    store: &self.state,
                    ptr: BufferPtr(buffer),
                    index,
                })
            })
        })
    }
}

/// Exclusive handle to one pooled `T`. Releasing the slot on [`Drop`].
///
/// Derefs to `T` via [`Deref`] / [`DerefMut`]. Use [`BufferGuard::map`] to borrow a
/// subfield or slice while keeping the same pool slot.
pub struct BufferGuard<M: RawMutex + 'static, T> {
    store: &'static Mutex<M, State>,
    ptr: BufferPtr<T>,
    index: usize,
}

impl<M: RawMutex + 'static, T> Drop for BufferGuard<M, T> {
    fn drop(&mut self) {
        unsafe {
            self.store.lock_mut(|state| {
                state.available |= 1 << self.index;
                state.waker.wake();
            });
        }
    }
}

impl<M: RawMutex + 'static, T> Deref for BufferGuard<M, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.ptr.0 }
    }
}

impl<M: RawMutex + 'static, T> DerefMut for BufferGuard<M, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.ptr.0 }
    }
}

impl<M: RawMutex + 'static, T> BufferGuard<M, T> {
    /// Narrows the guard to a `&mut U` derived from the buffer (for example a slice into
    /// a larger `[u8]`). The original guard is consumed without running its [`Drop`]; the
    /// returned [`MappedBufferGuard`] still returns the pool slot when dropped.
    ///
    /// This is an **associated function**, not a method: the guard is passed as `orig`,
    /// not `self`. You call it as `BufferGuard::map(guard, f)`. If the first parameter
    /// were `self`, `guard.map(...)` would always resolve to this routine and **shadow**
    /// a `map` method on `T`; with `orig: Self`, `guard.map(...)` can still go through
    /// [`Deref`] / [`DerefMut`] to `T::map` when you want the inner value’s API.
    pub fn map<U: ?Sized>(
        orig: Self,
        fun: impl FnOnce(&mut T) -> &mut U,
    ) -> MappedBufferGuard<M, U> {
        let store = orig.store;
        let index = orig.index;
        let value = fun(unsafe { &mut *orig.ptr.0 });
        // Ownership of the slot is moved to the mapped guard; do not run `BufferGuard::drop`.
        core::mem::forget(orig);
        MappedBufferGuard {
            store,
            value,
            index,
        }
    }
}

/// Like [`BufferGuard`], but derefs to a borrowed `U` (often a slice or inner field).
///
/// Dropping this guard marks the same pool index free as the original [`BufferGuard`].
pub struct MappedBufferGuard<M: RawMutex + 'static, U: ?Sized> {
    store: &'static Mutex<M, State>,
    index: usize,
    value: *mut U,
}

impl<M: RawMutex + 'static, U: ?Sized> Drop for MappedBufferGuard<M, U> {
    fn drop(&mut self) {
        unsafe {
            self.store.lock_mut(|state| {
                state.available |= 1 << self.index;
                state.waker.wake();
            });
        }
    }
}

impl<M: RawMutex + 'static, U: ?Sized> Deref for MappedBufferGuard<M, U> {
    type Target = U;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<M: RawMutex + 'static, U: ?Sized> DerefMut for MappedBufferGuard<M, U> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.value }
    }
}

impl<M: RawMutex + 'static, U: ?Sized> MappedBufferGuard<M, U> {
    /// Chains [`BufferGuard::map`]: re-borrows `&mut U` into `&mut V` without releasing the
    /// pool slot until the final mapped guard is dropped.
    ///
    /// Same as [`BufferGuard::map`]: `orig: Self` instead of `self`, so call
    /// `MappedBufferGuard::map(guard, f)` and keep `guard.map(...)` available for `U::map`
    /// via [`Deref`] / [`DerefMut`].
    pub fn map<V: ?Sized>(
        orig: Self,
        fun: impl FnOnce(&mut U) -> &mut V,
    ) -> MappedBufferGuard<M, V> {
        let store = orig.store;
        let index = orig.index;
        let value = fun(unsafe { &mut *orig.value });
        core::mem::forget(orig);
        MappedBufferGuard {
            store,
            value,
            index,
        }
    }
}
