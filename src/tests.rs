use super::*;
use core::{cell::Cell, pin::Pin, task::Context};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use std::{
    boxed::Box,
    string::String,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Wake,
    vec::Vec,
};

#[derive(Default)]
struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl Counter {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

fn waker() -> (Arc<Counter>, Waker) {
    let count = Arc::new(Counter::default());
    (count.clone(), Waker::from(count))
}

fn poll<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
    future.as_mut().poll(&mut Context::from_waker(waker))
}

fn ready<T>(poll: Poll<T>) -> T {
    match poll {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("expected ready"),
    }
}

#[test]
fn borrowed_items_can_contain_local_references() {
    let text = String::from("local data");
    let mut items = [text.as_str()];
    {
        let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
        assert_eq!(*pool.try_take().unwrap(), "local data");
    }
    assert_eq!(items[0], "local data");
}

#[test]
fn backing_storage_is_reusable_after_pool_drops() {
    let mut items = [0u8];
    {
        let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
        *pool.try_take().unwrap() = 42;
    }
    assert_eq!(items, [42]);
    items[0] = 7;
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    assert_eq!(*pool.try_take().unwrap(), 7);
}

#[test]
fn capacity_and_availability_are_independent() {
    let mut items = [0u8; 2];
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    assert_eq!(pool.capacity(), 2);
    assert_eq!(pool.available(), 2);
    let first = pool.try_take().unwrap();
    let second = pool.try_take().unwrap();
    assert_eq!(pool.capacity(), 2);
    assert_eq!(pool.available(), 0);
    assert!(pool.try_take().is_none());
    drop(first);
    assert_eq!(pool.available(), 1);
    drop(second);
    assert_eq!(pool.available(), 2);
}

#[test]
#[should_panic(expected = "buffer count must be in 1..=32")]
fn borrowed_empty_slice_is_rejected() {
    let _ = BufferPool::<NoopRawMutex, u8>::new(&mut []);
}

#[test]
#[should_panic(expected = "buffer count must be in 1..=32")]
fn borrowed_33_items_are_rejected() {
    let _ = BufferPool::<NoopRawMutex, u8>::new(&mut [0; 33]);
}

#[test]
#[should_panic(expected = "buffer count must be in 1..=32")]
fn owning_empty_array_is_rejected() {
    let _ = StaticBufferPool::<NoopRawMutex, u8, 0>::new([]);
}

#[test]
#[should_panic(expected = "buffer count must be in 1..=32")]
fn owning_33_items_are_rejected() {
    let _ = StaticBufferPool::<NoopRawMutex, u8, 33>::new([0; 33]);
}

fn exercise_32_slots(pool: &BufferPool<'_, NoopRawMutex, [u8; 4]>) {
    let mut guards = Vec::new();
    let mut seen = 0u32;
    for _ in 0..32 {
        let mut guard = pool.try_take().unwrap();
        let index = guard.token.index;
        assert_eq!(seen & (1 << index), 0);
        seen |= 1 << index;
        guard[0] = index as u8;
        guards.push(guard);
    }
    assert_eq!(seen, u32::MAX);
    assert_eq!(pool.available(), 0);
    for guard in guards {
        assert_eq!(guard[0], guard.token.index as u8);
    }
    assert_eq!(pool.available(), 32);
    let restored: Vec<_> = (0..32).map(|_| pool.try_take().unwrap()).collect();
    assert_eq!(restored.len(), 32);
    assert!(pool.try_take().is_none());
}

#[test]
fn all_32_borrowed_slots_are_unique_and_restored() {
    let mut items = [[0; 4]; 32];
    exercise_32_slots(&BufferPool::new(&mut items));
}

#[test]
fn all_32_owned_slots_are_unique_and_restored() {
    let pool = StaticBufferPool::new([[0; 4]; 32]);
    exercise_32_slots(&pool);
}

#[test]
fn mutable_projections_transfer_exactly_one_slot() {
    let mut items = [[0u8; 4]];
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    let mut mapped = BufferGuard::map(pool.try_take().unwrap(), |b| &mut b[..]);
    mapped[0] = 42;
    assert!(pool.try_take().is_none());
    let mut mapped = BufferGuard::map(mapped, |b| &mut b[0]);
    *mapped += 1;
    assert!(pool.try_take().is_none());
    drop(mapped);
    assert_eq!(pool.available(), 1);
    assert_eq!(pool.try_take().unwrap()[0], 43);
}

#[test]
fn shared_projections_chain_and_retain_the_slot() {
    let mut items = [[7u8; 4]];
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    let shared = BufferGuard::map_ref(pool.try_take().unwrap(), |b| &b[..]);
    let shared = BufferGuard::map_ref(shared, |b| &b[1]);
    assert_eq!(*shared, 7);
    assert!(pool.try_take().is_none());
    drop(shared);
    assert_eq!(pool.available(), 1);
}

#[test]
fn mapping_panic_releases_and_wakes_once() {
    let mut items = [[0u8; 4]];
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    let guard = pool.try_take().unwrap();
    let (count, waker) = waker();
    let mut pending = Box::pin(pool.take());
    assert!(poll(&mut pending, &waker).is_pending());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        BufferGuard::map(guard, |_| -> &mut u8 { panic!("mapping failed") });
    }));
    assert!(result.is_err());
    assert_eq!(pool.available(), 1);
    assert_eq!(count.count(), 1);
    drop(ready(poll(&mut pending, &waker)));
    assert_eq!(count.count(), 1);

    let guard = pool.try_take().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        BufferGuard::map_ref(guard, |_| -> &u8 { panic!("shared mapping failed") });
    }));
    assert!(result.is_err());
    assert_eq!(pool.available(), 1);

    let shared = BufferGuard::map_ref(pool.try_take().unwrap(), |b| &b[..]);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        BufferGuard::map_ref(shared, |_| -> &u8 { panic!("chained mapping failed") });
    }));
    assert!(result.is_err());
    assert_eq!(pool.available(), 1);
}

#[test]
fn single_waker_replacement_and_greedy_try_take() {
    let mut items = [0u8];
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    let held = pool.try_take().unwrap();
    let (first, first_waker) = waker();
    let (second, second_waker) = waker();
    let mut a = Box::pin(pool.take());
    let mut b = Box::pin(pool.take());
    assert!(poll(&mut a, &first_waker).is_pending());
    assert!(poll(&mut a, &first_waker).is_pending());
    // will_wake is best effort: Miri may give cloned Arc wakers distinct vtables.
    let repeated_notifications = first.count();
    assert!(poll(&mut b, &second_waker).is_pending());
    assert_eq!(first.count(), repeated_notifications + 1);
    drop(held);
    assert_eq!(second.count(), 1);
    let stolen = pool.try_take().unwrap();
    assert!(poll(&mut a, &first_waker).is_pending());
    drop(stolen);
    assert_eq!(first.count(), repeated_notifications + 2);
    let acquired = ready(poll(&mut b, &second_waker));
    assert!(poll(&mut a, &first_waker).is_pending());
    drop(acquired);
    drop(ready(poll(&mut a, &first_waker)));
    assert_eq!(pool.available(), 1);
}

#[test]
fn canceling_a_pending_future_leaves_a_harmless_waker() {
    let mut items = [0u8];
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    let held = pool.try_take().unwrap();
    let (count, waker) = waker();
    let mut pending = Box::pin(pool.take());
    assert!(poll(&mut pending, &waker).is_pending());
    drop(pending);
    drop(held);
    assert_eq!(count.count(), 1);
    assert_eq!(pool.available(), 1);
    let mut next = Box::pin(pool.take());
    drop(ready(poll(&mut next, &waker)));
    assert_eq!(count.count(), 1);
}

#[test]
fn a_mapped_release_wakes_a_waiter() {
    let pool = StaticBufferPool::<NoopRawMutex, _, 1>::new([[0u8; 4]]);
    let held = BufferGuard::map(pool.try_take().unwrap(), |b| &mut b[..]);
    let (count, waker) = waker();
    let mut pending = Box::pin(pool.take());
    assert!(poll(&mut pending, &waker).is_pending());
    drop(held);
    assert_eq!(count.count(), 1);
    drop(ready(poll(&mut pending, &waker)));
}

#[test]
fn const_created_owner_rebinds_after_moves_and_keeps_live_slots_disjoint() {
    let initial = const { StaticBufferPool::<NoopRawMutex, [u8; 4], 2>::new([[0; 4]; 2]) };
    let owner = initial;
    let mut first = owner.try_take().unwrap();
    first[0] = 11;
    let mut second = owner.try_take().unwrap();
    second[0] = 22;
    // Exercise repeated binding while references to both slots remain live.
    assert_eq!(owner.available(), 0);
    assert_eq!(first[0], 11);
    assert_eq!(second[0], 22);
    drop((first, second));

    let boxed = Box::new(owner);
    assert_eq!(boxed.try_take().unwrap()[0], 11);
    let moved = *boxed;
    assert_eq!(moved.try_take().unwrap()[0], 11);
    assert_eq!(moved.capacity(), 2);
}

#[test]
fn both_construction_styles_coerce_to_one_pool_parameter() {
    fn fill(pool: &BufferPool<'_, NoopRawMutex, u8>) {
        *pool.try_take().unwrap() = pool.capacity() as u8;
    }
    let one = StaticBufferPool::new([0u8]);
    let two = StaticBufferPool::new([0u8; 2]);
    let mut items = [0u8; 3];
    let runtime = BufferPool::new(&mut items);
    fill(&one);
    fill(&two);
    fill(&runtime);
    assert_eq!(*one.try_take().unwrap(), 1);
    assert_eq!(*two.try_take().unwrap(), 2);
    assert_eq!(*runtime.try_take().unwrap(), 3);
}

#[test]
fn owned_non_copy_items_drop_once_and_borrowed_items_are_not_owned() {
    struct Item(Arc<AtomicUsize>);
    impl Drop for Item {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let mut items = [Item(count.clone())];
    {
        let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
        drop(pool.try_take().unwrap());
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
    drop(items);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    {
        let pool =
            StaticBufferPool::<NoopRawMutex, _, 2>::new([Item(count.clone()), Item(count.clone())]);
        drop(pool.try_take().unwrap());
    }
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn zero_sized_items_have_independent_reservations() {
    let mut items = [(); 32];
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    let guards: Vec<_> = (0..32).map(|_| pool.try_take().unwrap()).collect();
    assert_eq!(pool.available(), 0);
    drop(guards);
    assert_eq!(pool.available(), 32);
}

#[test]
fn forgetting_a_guard_retains_only_its_slot() {
    let mut items = [0u8; 2];
    let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
    core::mem::forget(pool.try_take().unwrap());
    let other = pool.try_take().unwrap();
    assert_eq!(pool.available(), 0);
    drop(other);
    assert_eq!(pool.available(), 1);
}

#[test]
fn a_struct_owns_a_slice_guard_across_await() {
    struct Packet<'pool> {
        buffer: BufferGuard<'pool, NoopRawMutex, &'pool mut [u8]>,
    }
    async fn make_packet<'pool>(
        pool: &'pool BufferPool<'_, NoopRawMutex, [u8; 4]>,
    ) -> Packet<'pool> {
        Packet {
            buffer: BufferGuard::map(pool.take().await, |b| &mut b[..]),
        }
    }
    let mut items = [[0u8; 4]];
    let pool = BufferPool::new(&mut items);
    let phase = Cell::new(0);
    let mut task = Box::pin(async {
        let mut packet = make_packet(&pool).await;
        packet.buffer[0] = 42;
        poll_fn(|_| {
            if phase.replace(1) == 0 {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        assert_eq!(packet.buffer[0], 42);
        packet
    });
    let (_, waker) = waker();
    assert!(poll(&mut task, &waker).is_pending());
    assert!(pool.try_take().is_none());
    let packet = ready(poll(&mut task, &waker));
    assert_eq!(packet.buffer[0], 42);
    drop(packet);
    assert_eq!(pool.available(), 1);
}

#[test]
fn send_sync_bounds_follow_items_references_and_mutex() {
    fn send<T: Send>() {}
    fn sync<T: Sync>() {}
    send::<BufferPool<'_, NoopRawMutex, Cell<u8>>>();
    sync::<BufferPool<'_, CriticalSectionRawMutex, Cell<u8>>>();
    send::<StaticBufferPool<NoopRawMutex, Cell<u8>, 1>>();
    sync::<StaticBufferPool<CriticalSectionRawMutex, Cell<u8>, 1>>();
    send::<BufferGuard<'_, CriticalSectionRawMutex, &mut Cell<u8>>>();
    send::<BufferGuard<'_, CriticalSectionRawMutex, &mut u8>>();
    sync::<BufferGuard<'_, CriticalSectionRawMutex, &mut u8>>();
    send::<BufferGuard<'_, CriticalSectionRawMutex, &u8>>();
    sync::<BufferGuard<'_, CriticalSectionRawMutex, &u8>>();
}

struct TestRawMutex(std::sync::Mutex<()>);

// SAFETY: the standard mutex excludes every other thread until f completes.
unsafe impl RawMutex for TestRawMutex {
    const INIT: Self = Self(std::sync::Mutex::new(()));

    fn lock<R>(&self, f: impl FnOnce() -> R) -> R {
        let _lock = self.0.lock().unwrap();
        f()
    }
}

#[test]
fn a_guard_can_move_to_another_thread_and_release_there() {
    let mut items = [Cell::new(0u8)];
    let pool = BufferPool::<TestRawMutex, _>::new(&mut items);
    let guard = pool.try_take().unwrap();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            guard.set(42);
        });
    });
    assert_eq!(pool.available(), 1);
    assert_eq!(pool.try_take().unwrap().get(), 42);
}

#[test]
fn an_actual_static_owner_can_be_acquired() {
    static POOL: StaticBufferPool<TestRawMutex, Cell<u8>, 1> =
        StaticBufferPool::new([const { Cell::new(0) }; 1]);
    POOL.try_take().unwrap().set(42);
    assert_eq!(POOL.try_take().unwrap().get(), 42);
    assert_eq!(POOL.available(), 1);
}

#[test]
fn concurrent_owned_slots_remain_disjoint_during_rebinding() {
    let pool = StaticBufferPool::<TestRawMutex, Cell<u8>, 2>::new([Cell::new(0), Cell::new(0)]);
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        for value in [10, 20] {
            let pool = &pool;
            let barrier = &barrier;
            scope.spawn(move || {
                let mut guard = pool.try_take().unwrap();
                let item = &mut *guard;
                barrier.wait(); // Both slot loans remain live while binding runs.
                item.set(value);
                assert_eq!(item.get(), value);
            });
        }
    });
    let first = pool.try_take().unwrap();
    let second = pool.try_take().unwrap();
    assert_eq!(first.get() + second.get(), 30);
}

#[test]
fn wake_callbacks_can_reenter_on_replacement_and_release() {
    struct Reentrant {
        pool: Arc<StaticBufferPool<TestRawMutex, u8, 1>>,
        calls: AtomicUsize,
    }
    impl Wake for Reentrant {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(mut guard) = self.pool.try_take() {
                *guard = 42;
            }
        }
    }
    let pool = Arc::new(StaticBufferPool::<TestRawMutex, _, 1>::new([0u8]));
    let callback = Arc::new(Reentrant {
        pool: pool.clone(),
        calls: AtomicUsize::new(0),
    });
    let reentrant_waker = Waker::from(callback.clone());
    let (_, other_waker) = waker();
    let held = pool.try_take().unwrap();
    let mut pending = Box::pin(pool.take());
    assert!(poll(&mut pending, &reentrant_waker).is_pending());
    assert!(poll(&mut pending, &other_waker).is_pending());
    assert_eq!(callback.calls.load(Ordering::SeqCst), 1);
    assert!(poll(&mut pending, &reentrant_waker).is_pending());
    drop(held);
    assert_eq!(callback.calls.load(Ordering::SeqCst), 2);
    assert_eq!(pool.available(), 1);
    assert_eq!(*ready(poll(&mut pending, &other_waker)), 42);
}
