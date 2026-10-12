//! N-dimensional image data model.
//!
//! An [`Image`] is an immutable, cheaply cloned snapshot backed by a
//! [`PixelSource`]. Pixel data is exchanged one XY plane (or a region of one)
//! at a time as a dynamically typed [`Buffer`]; algorithms dispatch on the
//! pixel type once per buffer with [`dispatch!`] and then run monomorphic
//! loops over a typed slice.
//!
//! Axis 0 is always X and axis 1 is always Y. Any further axes (channel, Z,
//! time, ...) are named and ordered freely, with axis 2 varying fastest when
//! planes are numbered.

mod buffer;
mod cache;
mod dtype;
mod error;
mod mem;
mod meta;
mod source;
mod table;
mod view;

pub use buffer::{Buffer, crop_buffer};
pub use cache::{Cached, PlaneCache};
pub use dtype::{DType, Pixel, RealPixel};
pub use error::{Error, Result};
pub use mem::MemImage;
pub use meta::{Axis, AxisKind, ChannelMeta, ColorModel, ImageMeta, Lut};
pub use num_complex::{Complex32, Complex64};
pub use source::{Image, PixelSource, Rect};
pub use table::{Cell, Table};
