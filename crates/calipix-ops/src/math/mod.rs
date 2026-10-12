//! Arithmetic on samples, and between images.

mod calc;
pub(crate) mod java;
mod unary;

pub use calc::{CalcOp, calculate, calculate_rgb};
pub use unary::{MathOp, apply, rgb_set_components};
