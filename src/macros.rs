//! Declarative macros.

/// Builds `[T; N]` by repeating the same expression `N` times in the source.
///
/// Use this where `[expr; N]` is invalid (non-[`Copy`] `T`, or you need distinct
/// evaluations of `expr`). Each array element is a separate copy of `expr` in the
/// expansion, so `array_new!(Buf::new(), 3)` calls `Buf::new()` three times.
///
/// The first argument may be any expression valid in the surrounding context: a
/// constructor call, a literal, a `const` name, or other [`const`]-evaluable value.
///
/// `N` must be a literal from **0 through 32** (inclusive). [`crate::BufferPool`]
/// accepts **1 through 32** slots (`N == 0` is only for non-pool array construction).
///
/// # Examples
///
/// ```
/// use embedded_buffer_pool::array_new;
///
/// const ROW: [u8; 2] = [1, 2];
/// static GRID: [[u8; 2]; 3] = array_new!(ROW, 3);
///
/// struct Buf([u8; 4]);
/// impl Buf {
///     const fn new() -> Self {
///         Self([0; 4])
///     }
/// }
///
/// static BUFFERS: [Buf; 2] = array_new!(Buf::new(), 2);
/// ```
#[macro_export]
macro_rules! array_new {
    ($e:expr, 0) => { [] };
    ($e:expr, 1) => { [$e] };
    ($e:expr, 2) => { [$e, $e] };
    ($e:expr, 3) => { [$e, $e, $e] };
    ($e:expr, 4) => { [$e, $e, $e, $e] };
    ($e:expr, 5) => { [$e, $e, $e, $e, $e] };
    ($e:expr, 6) => { [$e, $e, $e, $e, $e, $e] };
    ($e:expr, 7) => { [$e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 8) => { [$e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 9) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 10) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 11) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 12) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 13) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 14) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 15) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 16) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 17) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 18) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 19) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 20) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 21) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 22) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 23) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 24) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 25) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 26) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 27) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 28) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 29) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 30) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 31) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
    ($e:expr, 32) => { [$e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e] };
}
