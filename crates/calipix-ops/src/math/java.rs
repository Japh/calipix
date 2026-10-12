//! Java's numeric conversions, which ImageJ's results depend on.

/// `(int)x` for a `double`: truncates toward zero, saturates at the limits
/// of a 32-bit integer and turns NaN into 0.
pub(crate) fn int(x: f64) -> i32 {
    x as i32
}

/// `Math.round(x)` for a `double`: the nearest `long`, ties toward positive
/// infinity, saturating; NaN gives 0.
pub(crate) fn round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// `(int)Math.round(x)`: the low 32 bits of [`round`].
pub(crate) fn round_int(x: f64) -> i32 {
    round(x) as i32
}
