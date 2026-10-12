//! Reading a plane at fractional coordinates, as ImageJ's processors do.

use calipix_core::RealPixel;

/// How samples between pixel centres are estimated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// The nearest sample is used unchanged.
    None,
    Bilinear,
    /// Catmull-Rom cubic interpolation over 4 x 4 samples.
    Bicubic,
}

impl Interpolation {
    pub const ALL: [Interpolation; 3] = [
        Interpolation::None,
        Interpolation::Bilinear,
        Interpolation::Bicubic,
    ];

    /// The method's name in ImageJ's dialogs.
    pub fn name(self) -> &'static str {
        match self {
            Interpolation::None => "None",
            Interpolation::Bilinear => "Bilinear",
            Interpolation::Bicubic => "Bicubic",
        }
    }

    pub fn from_name(name: &str) -> Option<Interpolation> {
        Interpolation::ALL.into_iter().find(|i| i.name() == name)
    }
}

/// A float type that averaged samples are accumulated in.
pub(crate) trait Accum: Sample {
    fn add_weighted(&mut self, value: Self, weight: f32);
}

impl Accum for f32 {
    fn add_weighted(&mut self, value: f32, weight: f32) {
        *self += value * weight;
    }
}

impl Accum for f64 {
    fn add_weighted(&mut self, value: f64, weight: f32) {
        *self += value * weight as f64;
    }
}

/// A sample type that can be interpolated.
pub(crate) trait Sample: RealPixel {
    /// The float type averaging is done in: single precision for the types
    /// it represents exactly, as ImageJ does, and double otherwise.
    type Acc: Accum;

    /// Stores an interpolated value. Integers round half up and are limited
    /// to the type's range, which for ImageJ's types is its `(int)(v+0.5)`
    /// followed by clamping; floats take the nearest value.
    fn from_interpolated(v: f64) -> Self;

    fn to_acc(self) -> Self::Acc;

    /// Stores an averaged value: `v + 0.5` in the accumulator's precision,
    /// limited to the type's range and truncated, for integers
    /// (`ByteProcessor.setPixels(int, FloatProcessor)`).
    fn from_acc(v: Self::Acc) -> Self;

    /// Stores a single-precision value without rescaling it, as
    /// `TypeConverter` does with scaling off: 8-bit adds a half in single
    /// precision, wider integers in double precision.
    fn from_f32(v: f32) -> Self;
}

macro_rules! int_sample {
    ($($ty:ty => $acc:ty, $half:ty),*) => {$(
        impl Sample for $ty {
            type Acc = $acc;

            fn from_interpolated(v: f64) -> Self {
                // `as` limits to the type's range and turns NaN into 0.
                (v + 0.5).floor() as $ty
            }

            fn to_acc(self) -> $acc {
                self as $acc
            }

            fn from_acc(v: $acc) -> Self {
                (v + 0.5).floor() as $ty
            }

            fn from_f32(v: f32) -> Self {
                (v as $half + 0.5).floor() as $ty
            }
        }
    )*};
}

int_sample!(
    u8 => f32, f32, i8 => f32, f32, u16 => f32, f64, i16 => f32, f64,
    u32 => f64, f64, i32 => f64, f64, u64 => f64, f64, i64 => f64, f64
);

impl Sample for f32 {
    type Acc = f32;

    fn from_interpolated(v: f64) -> f32 {
        v as f32
    }

    fn to_acc(self) -> f32 {
        self
    }

    fn from_acc(v: f32) -> f32 {
        v
    }

    fn from_f32(v: f32) -> f32 {
        v
    }
}

impl Sample for f64 {
    type Acc = f64;

    fn from_interpolated(v: f64) -> f64 {
        v
    }

    fn to_acc(self) -> f64 {
        self
    }

    fn from_acc(v: f64) -> f64 {
        v
    }

    fn from_f32(v: f32) -> f64 {
        v as f64
    }
}

/// `ImageProcessor.cubic`: the Catmull-Rom kernel (a = 0.5).
pub(crate) fn cubic(x: f64) -> f64 {
    const A: f64 = 0.5;
    let x = x.abs();
    if x < 1.0 {
        x * x * (x * (-A + 2.0) + (A - 3.0)) + 1.0
    } else if x < 2.0 {
        -A * x * x * x + 5.0 * A * x * x - 8.0 * A * x + 4.0 * A
    } else {
        0.0
    }
}

/// A row-major plane that can be read between its samples.
pub(crate) struct Grid<'a, T> {
    pub px: &'a [T],
    pub w: usize,
    pub h: usize,
}

impl<T: Sample> Grid<'_, T> {
    fn at(&self, x: usize, y: usize) -> f64 {
        self.px[y * self.w + x].to_f64()
    }

    /// Bilinear interpolation at `(x, y)`, which the caller keeps within
    /// `0 <= x < w - 1` and `0 <= y < h - 1`
    /// (`getInterpolatedPixel(double, double, pixels)`).
    ///
    /// This is `FloatProcessor`'s version, which lets the nearer neighbour
    /// stand in for a NaN one; for integer samples it is the same formula
    /// as `ByteProcessor`'s and `ShortProcessor`'s.
    pub fn bilinear(&self, x: f64, y: f64) -> f64 {
        // Truncation, as (int)x. The limits only matter for planes less
        // than two samples wide or high, which ImageJ cannot interpolate.
        let xbase = (x as isize).clamp(0, self.w as isize - 1) as usize;
        let ybase = (y as isize).clamp(0, self.h as isize - 1) as usize;
        let x_fraction = x - xbase as f64;
        let y_fraction = y - ybase as f64;
        let x1 = (xbase + 1).min(self.w - 1);
        let y1 = (ybase + 1).min(self.h - 1);
        let lower_left = self.at(xbase, ybase);
        let lower_right = self.at(x1, ybase);
        let upper_right = self.at(x1, y1);
        let upper_left = self.at(xbase, y1);
        let mix = |a: f64, b: f64, fraction: f64| {
            if a.is_nan() && fraction >= 0.5 {
                b
            } else if b.is_nan() && fraction < 0.5 {
                a
            } else {
                a + fraction * (b - a)
            }
        };
        let upper_average = mix(upper_left, upper_right, x_fraction);
        let lower_average = mix(lower_left, lower_right, x_fraction);
        mix(lower_average, upper_average, y_fraction)
    }

    /// Bilinear interpolation with the coordinates pulled inside the plane
    /// first (`getInterpolatedPixel(double, double)`): below 0 becomes 0,
    /// and `w - 1` or more becomes `w - 1.001`.
    pub fn bilinear_clamped(&self, mut x: f64, mut y: f64) -> f64 {
        if x < 0.0 {
            x = 0.0;
        }
        if x >= self.w as f64 - 1.0 {
            x = self.w as f64 - 1.001;
        }
        if y < 0.0 {
            y = 0.0;
        }
        if y >= self.h as f64 - 1.0 {
            y = self.h as f64 - 1.001;
        }
        self.bilinear(x, y)
    }

    /// `ImageProcessor.getBicubicInterpolatedPixel`: cubic interpolation
    /// over the 4 x 4 samples around `(x0, y0)`. Within one sample of the
    /// border it falls back to clamped bilinear interpolation, and more
    /// than one sample outside the plane it gives `background`.
    pub fn bicubic(&self, x0: f64, y0: f64, background: f64) -> f64 {
        let u0 = x0.floor() as i64;
        let v0 = y0.floor() as i64;
        let (w, h) = (self.w as i64, self.h as i64);
        if u0 <= 0 || u0 >= w - 2 || v0 <= 0 || v0 >= h - 2 {
            return if x0 >= -1.0 && x0 < w as f64 && y0 >= -1.0 && y0 < h as f64 {
                self.bilinear_clamped(x0, y0)
            } else {
                background
            };
        }
        let mut q = 0.0;
        for j in 0..=3 {
            let v = v0 - 1 + j;
            let mut p = 0.0;
            for i in 0..=3 {
                let u = u0 - 1 + i;
                p += self.at(u as usize, v as usize) * cubic(x0 - u as f64);
            }
            q += p * cubic(y0 - v as f64);
        }
        q
    }
}
