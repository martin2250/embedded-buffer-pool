# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Breaking:** `BufferPool<'items, M, T>` now borrows a runtime-sized slice of
  1..=32 items. It has no buffer-count const parameter, and acquisitions borrow
  the pool rather than requiring `&'static self`.
- **Breaking:** `BufferGuard<'pool, M, V>` describes mutable or shared reference
  access. `map` retains mutable projections; `map_ref` supports shared projection
  and chaining. `MappedBufferGuard` is removed. Wrapper structs can own projected
  guards to retain their pool slots.
- Removed the array-construction macro; use inline const repeat expressions such
  as `[const { Thing::new() }; 16]` instead.
- Waker cloning, destruction, and notification run outside the state mutex, so
  wake callbacks can safely re-enter the pool.
- Corrected the declared minimum Rust version to 1.87, as required by the existing
  `embassy-sync`/`heapless` dependency chain.

### Added

- `BufferGuard::map_mut_ref` supports projections that take mutable input and
  return a shared reference, retaining the original pool slot.
- `StaticBufferPool<M, T, N>` owns an array, has a const constructor, and
  dereferences to `BufferPool`. Local owners can move between borrows.
- `capacity()` and `available()` queries, plus lifetime, mapping, static storage,
  thread-safety, and re-entrant waker validation.

## [0.2.0] - 2026-03-29

### Changed

- Relaxed `BufferPool` so `T` is no longer required to be `Copy`. Construct the backing
  `[T; N]` with repeat-array syntax or inline const expressions when `[expr; N]`
  does not apply.

[Unreleased]: https://github.com/martin2250/embedded-buffer-pool/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/martin2250/embedded-buffer-pool/compare/v0.1.0...v0.2.0
