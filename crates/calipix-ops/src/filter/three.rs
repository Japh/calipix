//! The 3 x 3 filters of ImageJ's Process menu: Smooth, Sharpen, Find
//! Edges and Shadows.
//!
//! Unlike the other filters, ImageJ computes these in the image's own
//! pixel type, with integer arithmetic for integer images. The functions
//! named are those of `ByteProcessor`, `ShortProcessor`, `FloatProcessor`
//! and `ColorProcessor`.

use calipix_core::{Buffer, Error, Pixel, RealPixel, Rect, Result};

use super::tile::{Strip, default_strip_rows, filter_strips};
use super::{Frame, complex_error, to_f32};

/// The direction the light comes from in [`shadows`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    North,
    Northeast,
    East,
    Southeast,
    South,
    Southwest,
    West,
    Northwest,
}

impl Direction {
    pub const ALL: [Direction; 8] = [
        Direction::North,
        Direction::Northeast,
        Direction::East,
        Direction::Southeast,
        Direction::South,
        Direction::Southwest,
        Direction::West,
        Direction::Northwest,
    ];

    /// The kernel of `ij.plugin.filter.Shadows` for this direction.
    pub fn kernel(self) -> [i32; 9] {
        match self {
            Direction::North => [1, 2, 1, 0, 1, 0, -1, -2, -1],
            Direction::South => [-1, -2, -1, 0, 1, 0, 1, 2, 1],
            Direction::East => [-1, 0, 1, -2, 1, 2, -1, 0, 1],
            Direction::West => [1, 0, -1, 2, 1, -2, 1, 0, -1],
            Direction::Northwest => [2, 1, 0, 1, 1, -1, 0, -1, -2],
            Direction::Southeast => [-2, -1, 0, -1, 1, 1, 0, 1, 2],
            Direction::Northeast => [0, 1, 2, -1, 1, 1, -2, -1, 0],
            Direction::Southwest => [0, -1, -2, 1, 1, -1, 2, 1, 0],
        }
    }
}

/// The kernel of `ImageProcessor.sharpen`.
const SHARPEN: [i32; 9] = [-1, -1, -1, -1, 12, -1, -1, -1, -1];

/// What to compute from each 3 x 3 neighbourhood.
#[derive(Clone, Copy)]
enum Op {
    /// `BLUR_MORE`.
    Smooth,
    /// `FIND_EDGES`.
    Edges,
    /// `convolve3x3`.
    Convolve([i32; 9]),
    /// `ColorProcessor.convolve3x3`: no rounding.
    ConvolveRgb([i32; 9]),
}

/// Process > Smooth: replaces each sample of `frame.rect` with the mean
/// of its 3 x 3 neighbourhood (`ImageProcessor.smooth`, `filter(BLUR_MORE)`).
///
/// Integer types: `(sum + 4) / 9` in integer arithmetic. Float types: the
/// sum times `0.11111111` in single precision. A plane one sample wide is
/// left unchanged, as in ImageJ.
pub fn smooth(plane: &Buffer, frame: Frame) -> Result<Buffer> {
    if frame.width == 1 {
        frame.check(plane.len())?;
        return Ok(plane.clone());
    }
    apply(plane, frame, Op::Smooth)
}

/// Process > Sharpen: convolves `frame.rect` with
/// `-1 -1 -1 / -1 12 -1 / -1 -1 -1` divided by 4 (`ImageProcessor.sharpen`).
///
/// Integer types: `(sum + 2) / 4`, the division rounding towards zero,
/// clamped to the type's range. Float types: the sum times 0.25 in single
/// precision. A plane one sample wide is left unchanged.
///
/// `rgb` says that the plane is a channel of an RGB image. For 8-bit RGB,
/// ImageJ uses `ColorProcessor.convolve3x3`, which divides without
/// rounding (`sum / 4`) and leaves the outermost rows and columns of the
/// image unchanged; this is reproduced.
pub fn sharpen(plane: &Buffer, frame: Frame, rgb: bool) -> Result<Buffer> {
    if frame.width == 1 {
        frame.check(plane.len())?;
        return Ok(plane.clone());
    }
    convolve3x3(plane, frame, SHARPEN, rgb)
}

/// Process > Find Edges: the magnitude of the Sobel gradient,
/// `sqrt(gx^2 + gy^2)` (`filter(FIND_EDGES)`).
///
/// Integer types: computed in double precision, truncated, and clamped to
/// the top of the type's range. Float types: single precision.
pub fn find_edges(plane: &Buffer, frame: Frame) -> Result<Buffer> {
    apply(plane, frame, Op::Edges)
}

/// Process > Shadows: an embossing convolution lit from `direction`
/// (`ij.plugin.filter.Shadows`). The kernels sum to 1, so nothing is
/// divided; results are clamped to an integer type's range. `rgb` is as
/// for [`sharpen`].
pub fn shadows(plane: &Buffer, frame: Frame, direction: Direction, rgb: bool) -> Result<Buffer> {
    convolve3x3(plane, frame, direction.kernel(), rgb)
}

/// `ImageProcessor.convolve3x3` with an integer kernel given row by row.
/// The result is divided by the sum of the kernel unless that is zero.
fn convolve3x3(plane: &Buffer, frame: Frame, kernel: [i32; 9], rgb: bool) -> Result<Buffer> {
    if rgb && matches!(plane, Buffer::U8(_)) {
        frame.check(plane.len())?;
        // Only samples off the border of the image are computed.
        let r = frame.rect;
        let (x0, y0) = (r.x.max(1), r.y.max(1));
        let x1 = (r.x + r.w).min(frame.width.saturating_sub(1));
        let y1 = (r.y + r.h).min(frame.height.saturating_sub(1));
        if x0 >= x1 || y0 >= y1 {
            return Ok(plane.clone());
        }
        let inner = Frame::new(
            frame.width,
            frame.height,
            Rect::new(x0, y0, x1 - x0, y1 - y0),
        );
        return apply(plane, inner, Op::ConvolveRgb(kernel));
    }
    apply(plane, frame, Op::Convolve(kernel))
}

fn apply(plane: &Buffer, frame: Frame, op: Op) -> Result<Buffer> {
    Ok(match plane {
        Buffer::U8(s) => Buffer::from_vec(map3x3(s, frame, |n| integer(n, op))?),
        Buffer::I8(s) => Buffer::from_vec(map3x3(s, frame, |n| integer(n, op))?),
        Buffer::U16(s) => Buffer::from_vec(map3x3(s, frame, |n| integer(n, op))?),
        Buffer::I16(s) => Buffer::from_vec(map3x3(s, frame, |n| integer(n, op))?),
        Buffer::U32(s) => Buffer::from_vec(map3x3(s, frame, |n| integer(n, op))?),
        Buffer::I32(s) => Buffer::from_vec(map3x3(s, frame, |n| integer(n, op))?),
        Buffer::U64(s) => Buffer::from_vec(map3x3(s, frame, |n| integer(n, op))?),
        Buffer::I64(s) => Buffer::from_vec(map3x3(s, frame, |n| integer(n, op))?),
        Buffer::F32(s) => Buffer::from_vec(map3x3(s, frame, |n| float(n, op))?),
        Buffer::F64(s) => {
            // Filtered in single precision, like ImageJ's float images.
            let single: Vec<f32> = s.iter().map(|&v| to_f32(v)).collect();
            let filtered = map3x3(&single, frame, |n| float(n, op))?;
            let mut out = s.to_vec();
            let r = frame.rect;
            for y in r.y..r.y + r.h {
                for p in y * frame.width + r.x..y * frame.width + r.x + r.w {
                    out[p] = filtered[p] as f64;
                }
            }
            Buffer::from_vec(out)
        }
        Buffer::C32(_) | Buffer::C64(_) => return Err(complex_error("filter")),
    })
}

/// Replaces each sample of `frame.rect` by `f` of its 3 x 3 neighbourhood,
/// given row by row. Neighbours outside the plane are the nearest sample.
fn map3x3<T: Pixel>(src: &[T], frame: Frame, f: impl Fn([T; 9]) -> T + Sync) -> Result<Vec<T>> {
    let rows = default_strip_rows(1, frame.rect.h);
    filter_strips(src, frame, 1, rows, |strip: Strip<'_, T>| {
        let Frame {
            width,
            height,
            rect: r,
        } = strip.frame;
        let px = strip.rows;
        let mut out = Vec::with_capacity(r.w * r.h);
        for y in r.y..r.y + r.h {
            let up = y.saturating_sub(1) * width;
            let mid = y * width;
            let down = (y + 1).min(height - 1) * width;
            for x in r.x..r.x + r.w {
                let left = x.saturating_sub(1);
                let right = (x + 1).min(width - 1);
                out.push(f([
                    px[up + left],
                    px[up + x],
                    px[up + right],
                    px[mid + left],
                    px[mid + x],
                    px[mid + right],
                    px[down + left],
                    px[down + x],
                    px[down + right],
                ]));
            }
        }
        Ok(out)
    })
}

/// An integer sample type with its range.
trait IntSample: RealPixel + Into<i128> + TryFrom<i128> {
    const MIN: Self;
    const MAX: Self;

    /// `v` clamped to the type's range.
    fn clamped(v: i128) -> Self {
        let (min, max): (i128, i128) = (Self::MIN.into(), Self::MAX.into());
        Self::try_from(v.clamp(min, max)).unwrap_or(Self::MAX)
    }
}

macro_rules! int_sample {
    ($($ty:ty),*) => {$(
        impl IntSample for $ty {
            const MIN: Self = <$ty>::MIN;
            const MAX: Self = <$ty>::MAX;
        }
    )*};
}
int_sample!(u8, i8, u16, i16, u32, i32, u64, i64);

/// The rules of `ByteProcessor.filter`/`convolve3x3` and
/// `ShortProcessor.filter3x3`, which agree, applied to any integer type.
///
/// Java's division rounds towards zero where this rounds down. For
/// ImageJ's unsigned types the two cannot be told apart, because a
/// negative result is clamped to zero either way. For signed types, which
/// ImageJ lacks, rounding down keeps the rounding the same on both sides
/// of zero, so that a constant image stays as it is.
fn integer<T: IntSample>(n: [T; 9], op: Op) -> T {
    let v: [i128; 9] = n.map(Into::into);
    match op {
        Op::Smooth => T::clamped((v.iter().sum::<i128>() + 4).div_euclid(9)),
        Op::Edges => {
            let sum1 = (v[0] + 2 * v[1] + v[2] - v[6] - 2 * v[7] - v[8]) as f64;
            let sum2 = (v[0] + 2 * v[3] + v[6] - v[2] - 2 * v[5] - v[8]) as f64;
            // The cast truncates and saturates.
            T::clamped((sum1 * sum1 + sum2 * sum2).sqrt() as i128)
        }
        Op::Convolve(kernel) | Op::ConvolveRgb(kernel) => {
            let mut scale: i128 = kernel.iter().map(|&k| k as i128).sum();
            if scale == 0 {
                scale = 1;
            }
            let sum: i128 = kernel.iter().zip(&v).map(|(&k, &v)| k as i128 * v).sum();
            T::clamped(match op {
                Op::ConvolveRgb(_) => sum.div_euclid(scale),
                _ => (sum + scale / 2).div_euclid(scale),
            })
        }
    }
}

/// The rules of `FloatProcessor.filter3x3`, all in single precision.
fn float(v: [f32; 9], op: Op) -> f32 {
    match op {
        Op::Smooth => {
            (v[0] + v[1] + v[2] + v[3] + v[4] + v[5] + v[6] + v[7] + v[8]) * 0.111_111_11_f32
        }
        Op::Edges => {
            let sum1 = v[0] + 2.0 * v[1] + v[2] - v[6] - 2.0 * v[7] - v[8];
            let sum2 = v[0] + 2.0 * v[3] + v[6] - v[2] - 2.0 * v[5] - v[8];
            ((sum1 * sum1 + sum2 * sum2) as f64).sqrt() as f32
        }
        Op::Convolve(kernel) | Op::ConvolveRgb(kernel) => {
            let mut scale = 0f32;
            for k in kernel {
                scale += k as f32;
            }
            if scale == 0.0 {
                scale = 1.0;
            }
            let scale = 1f32 / scale;
            let k = kernel.map(|k| k as f32);
            let sum = k[0] * v[0]
                + k[1] * v[1]
                + k[2] * v[2]
                + k[3] * v[3]
                + k[4] * v[4]
                + k[5] * v[5]
                + k[6] * v[6]
                + k[7] * v[7]
                + k[8] * v[8];
            sum * scale
        }
    }
}

/// Parses a direction name such as `"north"` or `"Southwest"`.
impl std::str::FromStr for Direction {
    type Err = Error;

    fn from_str(s: &str) -> Result<Direction> {
        Direction::ALL
            .into_iter()
            .find(|d| format!("{d:?}").eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| Error::Other(format!("`{s}` is not a compass direction")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_rounding_follows_java() {
        // Smooth: (sum + 4) / 9.
        let plane = Buffer::from_vec(vec![0u8, 0, 0, 0, 5, 0, 0, 0, 0]);
        let out = smooth(&plane, Frame::whole(3, 3)).unwrap();
        assert_eq!(out.as_slice::<u8>().unwrap()[4], 1);
        // Sharpen: (12 * 5 + 2) / 4 = 15; neighbours (-5 + 2) / 4 = 0.
        let out = sharpen(&plane, Frame::whole(3, 3), false).unwrap();
        assert_eq!(out.as_slice::<u8>().unwrap(), &[0, 0, 0, 0, 15, 0, 0, 0, 0]);
        // Find Edges saturates.
        let step = Buffer::from_vec(vec![0u8, 255, 255, 0, 255, 255, 0, 255, 255]);
        let out = find_edges(&step, Frame::whole(3, 3)).unwrap();
        assert_eq!(
            out.as_slice::<u8>().unwrap(),
            &[255, 255, 0, 255, 255, 0, 255, 255, 0]
        );
    }

    #[test]
    fn rgb_sharpen_skips_the_border_and_does_not_round() {
        let plane = Buffer::from_vec(vec![0u8, 0, 0, 0, 0, 5, 1, 0, 0, 0, 0, 0]);
        let out = sharpen(&plane, Frame::whole(4, 3), true).unwrap();
        // (60 - 1) / 4 = 14 where the grey rule gives (59 + 2) / 4 = 15.
        assert_eq!(
            out.as_slice::<u8>().unwrap(),
            &[0, 0, 0, 0, 0, 14, 1, 0, 0, 0, 0, 0]
        );
        let grey = sharpen(&plane, Frame::whole(4, 3), false).unwrap();
        assert_eq!(grey.as_slice::<u8>().unwrap()[5], 15);
        // Two samples wide: nothing is off the border.
        let narrow = Buffer::from_vec(vec![9u8, 0, 0, 9]);
        assert_eq!(sharpen(&narrow, Frame::whole(2, 2), true).unwrap(), narrow);
    }

    #[test]
    fn wide_integers_do_not_overflow() {
        let plane = Buffer::from_vec(vec![u64::MAX; 9]);
        let frame = Frame::whole(3, 3);
        assert_eq!(smooth(&plane, frame).unwrap(), plane);
        assert_eq!(sharpen(&plane, frame, false).unwrap(), plane);
        let low = Buffer::from_vec(vec![i64::MIN; 9]);
        assert_eq!(
            smooth(&low, frame).unwrap().as_slice::<i64>().unwrap()[4],
            i64::MIN
        );
        assert_eq!(
            find_edges(&low, frame).unwrap().as_slice::<i64>().unwrap()[4],
            0
        );
    }

    #[test]
    fn directions_parse() {
        assert_eq!("north".parse::<Direction>().unwrap(), Direction::North);
        assert_eq!(
            " SouthWest ".parse::<Direction>().unwrap(),
            Direction::Southwest
        );
        assert!("up".parse::<Direction>().is_err());
    }
}
