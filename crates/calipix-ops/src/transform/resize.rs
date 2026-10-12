//! Scaling: to a new size, within the plane, and along Z.

use calipix_core::{Buffer, Error, Rect, Result};

use super::sample::{Accum, Grid, Interpolation, Sample, cubic};
use super::{on_samples, plane_height};
use crate::math::java;

/// Scales the part of a plane inside `rect` to a new plane of
/// `dst_width` x `dst_height` samples, as `ImageProcessor.resize(int, int,
/// boolean)` does for ImageJ's Image > Scale and Image > Adjust > Size.
///
/// * Without `average`, or when neither side shrinks, each result sample is
///   read from the source at the corresponding position (the `resize`
///   method of each processor), with the chosen interpolation.
/// * With `average`, a side that shrinks is first reduced by averaging the
///   source samples each result sample covers, weighted by the
///   interpolation's kernel (box, triangle or cubic; `FloatProcessor
///   .downsize` and `DownsizeTable`), which avoids aliasing. ImageJ does
///   this in single precision; so does Calipix for 8 and 16-bit integer and
///   32-bit float samples, and in double precision for wider types.
///
/// A result the same size as `rect` is a plain copy. Interpolating a plane
/// that is one sample wide or high interpolates linearly along its one row
/// or column, ignoring `rect` (`resizeLinearly`).
pub fn resize(
    buf: &Buffer,
    width: usize,
    rect: Rect,
    dst_width: usize,
    dst_height: usize,
    interpolation: Interpolation,
    average: bool,
) -> Result<Buffer> {
    let height = plane_height(buf.len(), width, Some(rect))?;
    if dst_width == 0 || dst_height == 0 {
        return Err(Error::Shape(
            "the new size must be at least one sample wide and high".to_string(),
        ));
    }
    on_samples!(buf, |s| resize_samples(
        s,
        width,
        height,
        rect,
        dst_width,
        dst_height,
        interpolation,
        average
    ))
}

#[allow(clippy::too_many_arguments)]
fn resize_samples<T: Sample>(
    src: &[T],
    width: usize,
    height: usize,
    r: Rect,
    dst_width: usize,
    dst_height: usize,
    interpolation: Interpolation,
    average: bool,
) -> Vec<T> {
    if (dst_width >= r.w && dst_height >= r.h) || !average {
        return resize_plain(src, width, height, r, dst_width, dst_height, interpolation);
    }
    // ImageProcessor.resize(int, int, boolean): through a float copy.
    let floats: Vec<T::Acc> = src.iter().map(|&v| v.to_acc()).collect();
    downsize(
        &floats,
        width,
        height,
        r,
        dst_width,
        dst_height,
        interpolation,
    )
    .into_iter()
    .map(T::from_acc)
    .collect()
}

fn crop<T: Copy>(src: &[T], width: usize, r: Rect) -> Vec<T> {
    let mut out = Vec::with_capacity(r.w * r.h);
    for y in r.y..r.y + r.h {
        out.extend_from_slice(&src[y * width + r.x..y * width + r.x + r.w]);
    }
    out
}

/// The `resize(int, int)` method of `ByteProcessor`, `ShortProcessor` and
/// `FloatProcessor`.
fn resize_plain<T: Sample>(
    src: &[T],
    width: usize,
    height: usize,
    r: Rect,
    dst_width: usize,
    dst_height: usize,
    interpolation: Interpolation,
) -> Vec<T> {
    if r.w == dst_width && r.h == dst_height {
        return crop(src, width, r);
    }
    if (width == 1 || height == 1) && interpolation != Interpolation::None {
        return resize_linearly(src, width, height, dst_width, dst_height);
    }
    let grid = Grid {
        px: src,
        w: width,
        h: height,
    };
    let src_center_x = r.x as f64 + r.w as f64 / 2.0;
    let src_center_y = r.y as f64 + r.h as f64 / 2.0;
    let mut dst_center_x = dst_width as f64 / 2.0;
    let mut dst_center_y = dst_height as f64 / 2.0;
    let x_scale = dst_width as f64 / r.w as f64;
    let y_scale = dst_height as f64 / r.h as f64;
    if interpolation != Interpolation::None {
        if dst_width != width {
            dst_center_x += x_scale / 4.0;
        }
        if dst_height != height {
            dst_center_y += y_scale / 4.0;
        }
    }
    let (xlimit, xlimit2) = (width as f64 - 1.0, width as f64 - 1.001);
    let (ylimit, ylimit2) = (height as f64 - 1.0, height as f64 - 1.001);
    let mut out = Vec::with_capacity(dst_width * dst_height);
    for y in 0..dst_height {
        let mut ys = (y as f64 - dst_center_y) / y_scale + src_center_y;
        if interpolation == Interpolation::Bilinear {
            if ys < 0.0 {
                ys = 0.0;
            }
            if ys >= ylimit {
                ys = ylimit2;
            }
        }
        let row = (java::int(ys).max(0) as usize).min(height - 1);
        for x in 0..dst_width {
            let mut xs = (x as f64 - dst_center_x) / x_scale + src_center_x;
            out.push(match interpolation {
                Interpolation::Bicubic => T::from_interpolated(grid.bicubic(xs, ys, 0.0)),
                Interpolation::Bilinear => {
                    if xs < 0.0 {
                        xs = 0.0;
                    }
                    if xs >= xlimit {
                        xs = xlimit2;
                    }
                    T::from_interpolated(grid.bilinear(xs, ys))
                }
                Interpolation::None => {
                    let column = (java::int(xs).max(0) as usize).min(width - 1);
                    src[row * width + column]
                }
            });
        }
    }
    out
}

/// `ImageProcessor.resizeLinearly`: linear interpolation along the single
/// row or column of a plane, repeated across the other direction.
fn resize_linearly<T: Sample>(
    src: &[T],
    width: usize,
    height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<T> {
    // A column is handled as a row and turned back afterwards. Either way
    // only the first row or column is read.
    let column = width == 1;
    let (n1, n2, repeat) = if column {
        (height, dst_height, dst_width)
    } else {
        (width, dst_width, dst_height)
    };
    // rotateLeft turns the column into a row read from the top down.
    let data1: Vec<f32> = (0..n1)
        .map(|i| src[if column { i * width } else { i }].to_f64() as f32)
        .collect();
    let scale = (n1 as f64 - 1.0) / (n2 as f64 - 1.0);
    let data2: Vec<T> = (0..n2)
        .map(|x| {
            let position = x as f64 * scale;
            // NaN, from 0 / 0 when both lengths are 1, truncates to 0.
            let x1 = (java::int(position).max(0) as usize).min(n1 - 1);
            let x2 = (x1 + 1).min(n1 - 1);
            let fraction = position - x1 as f64;
            let value =
                ((1.0 - fraction) * data1[x1] as f64 + fraction * data1[x2] as f64) as f32;
            // convertToByte(false) and convertToShort(false) for integers.
            T::from_f32(value)
        })
        .collect();
    let mut out = Vec::with_capacity(dst_width * dst_height);
    if column {
        for value in data2 {
            out.extend(std::iter::repeat_n(value, repeat));
        }
    } else {
        for _ in 0..repeat {
            out.extend_from_slice(&data2);
        }
    }
    out
}

/// `FloatProcessor.downsize`: averaging reduction in X, then in Y, then any
/// enlargement the other direction still needs.
fn downsize<A: Accum>(
    src: &[A],
    width: usize,
    height: usize,
    r: Rect,
    dst_width: usize,
    dst_height: usize,
    interpolation: Interpolation,
) -> Vec<A> {
    // The plane being worked on, and the part of it still to be scaled.
    let mut plane: std::borrow::Cow<'_, [A]> = std::borrow::Cow::Borrowed(src);
    let (mut w, mut h, mut roi) = (width, height, r);
    if dst_width < r.w {
        let table = DownsizeTable::new(w, roi.x, roi.w, dst_width, interpolation);
        let mut out = vec![A::default(); dst_width * roi.h];
        for line in 0..roi.h {
            let mut pointer = 0;
            for point in table.src_start..=table.src_end {
                let v = plane[(roi.y + line) * w + point];
                for _ in 0..table.kernel_size {
                    out[line * dst_width + table.indices[pointer]]
                        .add_weighted(v, table.weights[pointer]);
                    pointer += 1;
                }
            }
        }
        (w, h) = (dst_width, roi.h);
        roi = Rect::new(0, 0, w, h);
        plane = std::borrow::Cow::Owned(out);
    }
    if dst_height < r.h {
        let table = DownsizeTable::new(h, roi.y, roi.h, dst_height, interpolation);
        let mut out = vec![A::default(); roi.w * dst_height];
        for line in 0..roi.w {
            let mut pointer = 0;
            for point in table.src_start..=table.src_end {
                let v = plane[point * w + roi.x + line];
                for _ in 0..table.kernel_size {
                    out[table.indices[pointer] * roi.w + line]
                        .add_weighted(v, table.weights[pointer]);
                    pointer += 1;
                }
            }
        }
        (w, h) = (roi.w, dst_height);
        roi = Rect::new(0, 0, w, h);
        plane = std::borrow::Cow::Owned(out);
    }
    if w != dst_width || h != dst_height {
        return resize_plain(&plane, w, h, roi, dst_width, dst_height, interpolation);
    }
    plane.into_owned()
}

/// `ij.process.DownsizeTable`: for each source sample along one direction,
/// the result samples it contributes to and with what weight.
struct DownsizeTable {
    kernel_size: usize,
    src_start: usize,
    src_end: usize,
    indices: Vec<usize>,
    weights: Vec<f32>,
}

impl DownsizeTable {
    fn new(
        src_size: usize,
        src_origin: usize,
        src_length: usize,
        dst_size: usize,
        interpolation: Interpolation,
    ) -> DownsizeTable {
        const UNUSED: usize = usize::MAX;
        let scale = src_length as f64 / dst_size as f64;
        let kernel_size: usize = match interpolation {
            Interpolation::None => 1,
            Interpolation::Bilinear => 2,
            Interpolation::Bicubic => 4,
        };
        let half = 0.5 * kernel_size as f64;
        let src_index = |dst_index: f64| src_origin as f64 - 0.5 + (dst_index + 0.5) * scale;
        let dst_index = |src: i64| (src as f64 - src_origin as f64 + 0.5) / scale - 0.5;
        let kernel = |x: f64| -> f32 {
            match interpolation {
                Interpolation::None => 1.0,
                Interpolation::Bilinear => 1.0 - x.abs() as f32,
                Interpolation::Bicubic => cubic(x) as f32,
            }
        };

        let src_start = ((1e-8 + src_index(-half)).ceil() as i64).max(0);
        let src_end = ((1e-8 + src_index(dst_size as f64 - 1.0 + half)).floor() as i64)
            .min(src_size as i64 - 1);
        let array_size = (src_end - src_start + 1).max(0) as usize * kernel_size;
        let mut indices = vec![UNUSED; array_size];
        let mut weights = vec![0f32; array_size];
        for dst in 0..dst_size {
            let mut sum = 0f64;
            let lowest = (1e-8 + src_index(dst as f64 - half)).ceil() as i64;
            let highest = (-1e-8 + src_index(dst as f64 + half)).floor() as i64;
            for src in lowest..=highest {
                // A sample beyond the plane is replaced by the edge sample.
                let s = src.clamp(0, src_size as i64 - 1);
                let mut p = (s - src_start) as usize * kernel_size;
                while indices[p] != UNUSED && indices[p] != dst {
                    p += 1;
                }
                indices[p] = dst;
                let weight = kernel(dst as f64 - dst_index(src));
                sum += weight as f64;
                weights[p] += weight;
            }
            // The weights reaching one result sample add up to 1.
            let start = ((lowest - src_start) * kernel_size as i64).max(0) as usize;
            let stop = (((highest - src_start) * kernel_size as i64 + kernel_size as i64 - 1)
                .min(array_size as i64 - 1))
            .max(-1);
            for i in start as i64..=stop {
                let i = i as usize;
                if indices[i] == dst {
                    weights[i] = (weights[i] as f64 / sum) as f32;
                }
            }
        }
        for index in &mut indices {
            if *index == UNUSED {
                *index = 0;
            }
        }
        DownsizeTable {
            kernel_size,
            src_start: src_start as usize,
            src_end: src_end as usize,
            indices,
            weights,
        }
    }
}

/// Scales the contents of `rect` by the given factors about the centre of
/// `rect`, within the plane, which keeps its size (the `scale` method of
/// ImageJ's processors, used by Image > Scale without a new window).
///
/// Enlarging in both directions lets the result grow beyond `rect`, up to
/// the edges of the plane. When shrinking, the part of `rect` the reduced
/// contents no longer cover is set to `background`; with bicubic
/// interpolation ImageJ instead fills it with whatever the interpolation
/// finds there, and Calipix does the same.
pub fn scale_in_place(
    buf: &Buffer,
    width: usize,
    rect: Rect,
    x_scale: f64,
    y_scale: f64,
    interpolation: Interpolation,
    background: f64,
) -> Result<Buffer> {
    let height = plane_height(buf.len(), width, Some(rect))?;
    if !(x_scale > 0.0 && y_scale > 0.0) {
        return Err(Error::Other(format!(
            "scale factors must be positive, not {x_scale} and {y_scale}"
        )));
    }
    on_samples!(buf, |s| scale_samples(
        s,
        width,
        height,
        rect,
        x_scale,
        y_scale,
        interpolation,
        background
    ))
}

#[allow(clippy::too_many_arguments)]
fn scale_samples<T: Sample>(
    src: &[T],
    width: usize,
    height: usize,
    r: Rect,
    x_scale: f64,
    y_scale: f64,
    interpolation: Interpolation,
    background: f64,
) -> Vec<T> {
    let mut out = src.to_vec();
    let grid = Grid {
        px: src,
        w: width,
        h: height,
    };
    let (roi_x, roi_y) = (r.x as i64, r.y as i64);
    let x_center = r.x as f64 + r.w as f64 / 2.0;
    let y_center = r.y as f64 + r.h as f64 / 2.0;
    let (xmin, xmax, ymin, ymax) = if x_scale > 1.0 && y_scale > 1.0 {
        // The result may spread beyond the rectangle.
        let xmin = (java::int(x_center - (x_center - r.x as f64) * x_scale) as i64).max(0);
        let xmax = (xmin + java::int(r.w as f64 * x_scale) as i64 - 1).min(width as i64 - 1);
        let ymin = (java::int(y_center - (y_center - r.y as f64) * y_scale) as i64).max(0);
        let ymax = (ymin + java::int(r.h as f64 * y_scale) as i64 - 1).min(height as i64 - 1);
        (xmin, xmax, ymin, ymax)
    } else {
        (
            roi_x,
            roi_x + r.w as i64 - 1,
            roi_y,
            roi_y + r.h as i64 - 1,
        )
    };
    let check_coordinates = x_scale < 1.0 || y_scale < 1.0;
    let (xlimit, xlimit2) = (width as f64 - 1.0, width as f64 - 1.001);
    let (ylimit, ylimit2) = (height as f64 - 1.0, height as f64 - 1.001);
    let fill = T::from_f64(background);
    for y in ymin..=ymax {
        let mut ys = (y as f64 - y_center) / y_scale + y_center;
        let ysi = java::int(ys) as i64;
        if interpolation != Interpolation::Bicubic {
            if ys < 0.0 {
                ys = 0.0;
            }
            if ys >= ylimit {
                ys = ylimit2;
            }
        }
        let row = (java::int(ys).max(0) as usize).min(height - 1);
        for x in xmin..=xmax {
            let mut xs = (x as f64 - x_center) / x_scale + x_center;
            let xsi = java::int(xs) as i64;
            let value = if interpolation == Interpolation::Bicubic {
                T::from_interpolated(grid.bicubic(xs, ys, 0.0))
            } else if check_coordinates && (xsi < xmin || xsi > xmax || ysi < ymin || ysi > ymax)
            {
                fill
            } else if interpolation == Interpolation::Bilinear {
                if xs < 0.0 {
                    xs = 0.0;
                }
                if xs >= xlimit {
                    xs = xlimit2;
                }
                T::from_interpolated(grid.bilinear(xs, ys))
            } else {
                src[row * width + (xsi.max(0) as usize).min(width - 1)]
            };
            out[y as usize * width + x as usize] = value;
        }
    }
    out
}

/// Scales a stack of planes along Z to `new_depth` planes
/// (`Resizer.resizeZ`): every XZ section of the stack, one per row of the
/// planes, is resized with [`resize`] from the stack's depth to
/// `new_depth`, leaving its width alone.
pub fn resize_z(
    planes: &[Buffer],
    width: usize,
    new_depth: usize,
    interpolation: Interpolation,
    average: bool,
) -> Result<Vec<Buffer>> {
    let Some(first) = planes.first() else {
        return Err(Error::Shape("there are no planes to scale".to_string()));
    };
    let height = plane_height(first.len(), width, None)?;
    if new_depth == 0 {
        return Err(Error::Shape(
            "the new depth must be at least one plane".to_string(),
        ));
    }
    if planes
        .iter()
        .any(|p| p.dtype() != first.dtype() || p.len() != first.len())
    {
        return Err(Error::Shape(
            "the planes of a stack must share one pixel type and size".to_string(),
        ));
    }
    fn go<T: Sample>(
        first: &[T],
        planes: &[Buffer],
        width: usize,
        height: usize,
        new_depth: usize,
        interpolation: Interpolation,
        average: bool,
    ) -> Result<Vec<Buffer>> {
        let depth = planes.len();
        let typed = planes
            .iter()
            .map(|p| p.try_slice::<T>())
            .collect::<Result<Vec<_>>>()?;
        let mut out = vec![vec![first[0]; width * height]; new_depth];
        let mut section = vec![first[0]; width * depth];
        let whole = Rect::new(0, 0, width, depth);
        for y in 0..height {
            for (z, plane) in typed.iter().enumerate() {
                section[z * width..(z + 1) * width]
                    .copy_from_slice(&plane[y * width..(y + 1) * width]);
            }
            let scaled = resize_samples(
                &section,
                width,
                depth,
                whole,
                width,
                new_depth,
                interpolation,
                average,
            );
            for (z, plane) in out.iter_mut().enumerate() {
                plane[y * width..(y + 1) * width]
                    .copy_from_slice(&scaled[z * width..(z + 1) * width]);
            }
        }
        Ok(out.into_iter().map(Buffer::from_vec).collect())
    }
    calipix_core::dispatch_real!(
        first,
        |s| go(s, planes, width, height, new_depth, interpolation, average),
        complex => Err(super::complex_error())
    )
}
