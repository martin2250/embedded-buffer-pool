# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-03-29

### Changed

- Relaxed `BufferPool` so `T` is no longer required to be `Copy`. Construct the backing
  `[T; N]` with repeat-array syntax, the `array_new!` macro, or any other approach valid
  in your context when `[expr; N]` does not apply.

[Unreleased]: https://github.com/martin2250/embedded-buffer-pool/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/martin2250/embedded-buffer-pool/compare/v0.1.0...v0.2.0
