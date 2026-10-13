# embedded-buffer-pool

Fixed-size async buffer pool for `no_std` firmware using [`embassy-sync`](https://docs.rs/embassy-sync).

`BufferPool<'items, M, T>` borrows 1 through 32 items. Its capacity is determined
at construction, so different slice lengths share the same pool type. Acquisitions
borrow the pool and return `BufferGuard<'pool, M, &'pool mut T>`.

```rust
use embedded_buffer_pool::{BufferGuard, BufferPool};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

let mut items = [[0u8; 64]; 2];
let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
let mut buffer = pool.try_take().unwrap();
buffer[0] = 42;
let slice = BufferGuard::map(buffer, |b| &mut b[..16]);
let shared = BufferGuard::map_ref(slice, |b| b);
assert_eq!(shared[0], 42);
drop(shared); // The original physical slot becomes available again.
```

Use `StaticBufferPool<M, T, N>` for a const-created pool that owns its items. It
dereferences to `BufferPool`, including when passed to functions:

```rust
use embedded_buffer_pool::{BufferPool, StaticBufferPool};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

static POOL: StaticBufferPool<CriticalSectionRawMutex, [u8; 64], 2> =
    StaticBufferPool::new([[0; 64]; 2]);

async fn fill(pool: &BufferPool<'_, CriticalSectionRawMutex, [u8; 64]>) {
    pool.take().await[0] = 42;
}

async fn example() {
    fill(&POOL).await;
}
```

`T` need not be `Copy`. Use `[const { Thing::new() }; 16]` to initialize an array
of non-`Copy` items with a const constructor.

Borrowed pools support items containing local references;
the owning static type requires `T: 'static`. Both constructors reject zero or
more than 32 items. No heap allocation is required.

## Structs containing buffers

To return or transport a struct borrowing pooled data, let it own a projected
guard. The guard dereferences to the slice, so indexing and slicing work normally:

```rust
use embedded_buffer_pool::BufferGuard;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

struct Packet<'pool> {
    buffer: BufferGuard<'pool, NoopRawMutex, &'pool mut [u8]>,
}
```

All projection functions consume their input guard and retain exactly its
original slot. Shared projections can be chained with `map_ref`. A projection
panic releases the slot. The public API does not expose the contained reference
independently of the guard's lifetime.

Use `map_mut_ref` when a method needs mutable access but returns a shared
reference, such as `Thing::do_sth(&mut self) -> &[u8]`:

```rust
use embedded_buffer_pool::{BufferGuard, BufferPool};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

struct Thing([u8; 4]);
impl Thing {
    fn do_sth(&mut self) -> &[u8] {
        self.0[1] = 42;
        &self.0[1..3]
    }
}

let mut items = [Thing([0; 4])];
let pool = BufferPool::<NoopRawMutex, _>::new(&mut items);
let bytes = BufferGuard::map_mut_ref(pool.try_take().unwrap(), Thing::do_sth);
assert_eq!(&*bytes, &[42, 0]);
drop(bytes); // Releases the original Thing's slot.
```

The returned `BufferGuard<'pool, M, &'pool [u8]>` provides shared access. Use
`map` for mutable input and output, or `map_ref` for shared input and output.

## Waiting

`take().await` waits infallibly. Each pool keeps a single waker registration:
a different waiting task replaces and wakes the previous one, and `try_take()`
can claim any free slot even while tasks are waiting. Acquisition order is not
guaranteed; multiple waiting tasks can cause contention.

Canceling a pending acquisition may leave a harmless stale waker, which is cleared
on release or replaced by another waiter. Forgetting a guard retains its slot.
`capacity()` reports the total slots; `available()` reports the current free count
from the `u32` bitmap. Waker callbacks run after the state mutex is unlocked.

## Migration from 0.2

- Replace owning `BufferPool<M, T, N>` declarations with `StaticBufferPool<M, T, N>`.
- For borrowed runtime storage, use `BufferPool<'items, M, T>::new(&mut items)`.
- Change guard signatures from `BufferGuard<M, T>` to
  `BufferGuard<'pool, M, &'pool mut T>`.
- Replace `MappedBufferGuard::map` with `BufferGuard::map`. Use `map_ref` for
  shared access; `MappedBufferGuard` is removed.

Rust 1.87 or newer is required by the existing `embassy-sync` dependency chain.
See the [crate documentation on docs.rs](https://docs.rs/embedded-buffer-pool) for
API details.

## License

Dual-licensed under the MIT and Apache 2.0 licenses; see `Cargo.toml` for the SPDX expression.
