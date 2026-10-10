//! CPU reference renderer: turns image planes into displayable RGBA.
//!
//! This is the definition of how an image should look. The GPU viewer must
//! agree with it, and it is what export, thumbnails and the software
//! fallback use. The mapping from samples to display levels follows
//! ImageJ 1.x (`ShortProcessor.create8BitImage` for integers and
//! `FloatProcessor.create8BitImage` for floats).

use imager_core::{
    AxisKind, Buffer, ColorModel, DType, Error, Image, Lut, RealPixel, Result, dispatch_real,
};

/// An 8-bit RGBA picture, row-major, not premultiplied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// The display range used when an image does not specify one: the full
/// range for 8-bit data, and the smallest and largest non-NaN samples
/// otherwise.
pub fn default_display_range(plane: &Buffer) -> Result<(f64, f64)> {
    if plane.dtype() == DType::U8 {
        return Ok((0.0, 255.0));
    }
    fn range<T: RealPixel>(samples: &[T]) -> (f64, f64) {
        let mut range: Option<(f64, f64)> = None;
        for v in samples.iter().map(|v| v.to_f64()).filter(|v| !v.is_nan()) {
            range = Some(match range {
                None => (v, v),
                Some((lo, hi)) => (lo.min(v), hi.max(v)),
            });
        }
        range.unwrap_or((0.0, 0.0))
    }
    dispatch_real!(plane, |s| Ok(range(s)), complex => Err(complex_error()))
}

fn complex_error() -> Error {
    Error::Unsupported("complex images cannot be displayed directly".to_string())
}

/// Maps every sample to a display level from 0 to 255 for the given
/// display range.
pub fn to_levels(plane: &Buffer, (min, max): (f64, f64)) -> Result<Vec<u8>> {
    fn integer<T: RealPixel>(samples: &[T], min: f64, max: f64) -> Vec<u8> {
        let (min, max) = (min.trunc(), max.trunc());
        let scale = 256.0 / (max - min + 1.0);
        samples
            .iter()
            .map(|v| {
                let value = (v.to_f64() - min).max(0.0);
                ((value * scale + 0.5) as i64).min(255) as u8
            })
            .collect()
    }
    fn float<T: RealPixel>(samples: &[T], min: f64, max: f64) -> Vec<u8> {
        let scale = 255.0 / (max - min);
        samples
            .iter()
            .map(|v| {
                let value = v.to_f64() - min;
                let value = if value < 0.0 { 0.0 } else { value };
                // As in Java, NaN becomes 0 and the cast saturates.
                ((value * scale + 0.5) as i32).min(255) as u8
            })
            .collect()
    }
    if plane.dtype().is_float() {
        dispatch_real!(plane, |s| Ok(float(s, min, max)), complex => Err(complex_error()))
    } else {
        dispatch_real!(plane, |s| Ok(integer(s, min, max)), complex => Err(complex_error()))
    }
}

/// Renders the XY plane at `position` (one index per non-XY axis).
///
/// RGB images combine their three channels directly; the channel index in
/// `position` is ignored. Other images show the single plane at `position`
/// through its channel's lookup table and display range.
pub fn render_plane(image: &Image, position: &[usize]) -> Result<Rgba8> {
    let meta = image.meta();
    let (width, height) = (meta.width(), meta.height());
    let mut data = vec![255u8; width * height * 4];

    if meta.color == ColorModel::Rgb {
        let channel_axis = meta
            .axis_index(&AxisKind::Channel)
            .ok_or_else(|| Error::Shape("RGB image without a channel axis".to_string()))?;
        let full_range = meta.dtype().integer_range().unwrap_or((0.0, 1.0));
        let mut position = position.to_vec();
        for c in 0..3 {
            *position
                .get_mut(channel_axis - 2)
                .ok_or_else(|| Error::Shape("position is too short".to_string()))? = c;
            let plane = image.read_plane(meta.plane_index(&position)?)?;
            let range = channel_range(image, c).unwrap_or(full_range);
            for (pixel, level) in data
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(to_levels(&plane, range)?)
            {
                pixel[c] = level;
            }
        }
    } else {
        let index = meta.plane_index(position)?;
        let channel = meta.plane_channel(index)?;
        let plane = image.read_plane(index)?;
        let range = match channel_range(image, channel) {
            Some(range) => range,
            None => default_display_range(&plane)?,
        };
        let lut = meta
            .channels
            .get(channel)
            .and_then(|c| c.lut.clone())
            .unwrap_or_else(Lut::grays);
        for (pixel, level) in data
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(to_levels(&plane, range)?)
        {
            pixel[..3].copy_from_slice(&lut.table[level as usize]);
        }
    }
    Ok(Rgba8 {
        width,
        height,
        data,
    })
}

fn channel_range(image: &Image, channel: usize) -> Option<(f64, f64)> {
    image.meta().channels.get(channel)?.display_range
}

#[cfg(test)]
mod tests {
    use super::*;
    use imager_core::{ChannelMeta, ImageMeta};

    #[test]
    fn eight_bit_default_is_identity() {
        let plane = Buffer::from_vec((0..=255u8).collect());
        let range = default_display_range(&plane).unwrap();
        assert_eq!(range, (0.0, 255.0));
        let levels = to_levels(&plane, range).unwrap();
        assert!(levels.iter().enumerate().all(|(i, &l)| l as usize == i));
    }

    #[test]
    fn sixteen_bit_follows_imagej_scaling() {
        // ShortProcessor: scale = 256 / (max - min + 1), level = (int)(v * scale + 0.5).
        let plane = Buffer::from_vec(vec![100u16, 101, 227, 228, 355, 50, 400]);
        let levels = to_levels(&plane, (100.0, 355.0)).unwrap();
        assert_eq!(levels, [0, 1, 127, 128, 255, 0, 255]);
        // A range wider than 256 values: scale = 256 / 1001.
        let levels = to_levels(&Buffer::from_vec(vec![0u16, 500, 1000]), (0.0, 1000.0)).unwrap();
        assert_eq!(levels, [0, 128, 255]);
    }

    #[test]
    fn float_follows_imagej_scaling() {
        // FloatProcessor: scale = 255 / (max - min), level = (int)(v * scale + 0.5).
        let plane = Buffer::from_vec(vec![-1.0f32, 0.0, 0.5, 1.0, 2.0, f32::NAN, f32::INFINITY]);
        let levels = to_levels(&plane, (0.0, 1.0)).unwrap();
        assert_eq!(levels, [0, 0, 128, 255, 255, 0, 255]);
        // A flat image has no range; everything maps to 0, as in ImageJ.
        assert_eq!(
            to_levels(&Buffer::from_vec(vec![3.0f32; 2]), (3.0, 3.0)).unwrap(),
            [0, 0]
        );
    }

    #[test]
    fn default_range_ignores_nan() {
        let plane = Buffer::from_vec(vec![f32::NAN, 2.0, -3.0]);
        assert_eq!(default_display_range(&plane).unwrap(), (-3.0, 2.0));
        let plane = Buffer::from_vec(vec![7u16, 900]);
        assert_eq!(default_display_range(&plane).unwrap(), (7.0, 900.0));
    }

    #[test]
    fn grayscale_uses_lut_and_display_range() {
        let image = Image::from_vec(2, 1, vec![0u16, 1000]).unwrap();
        let out = render_plane(&image, &[]).unwrap();
        assert_eq!(out.data, [0, 0, 0, 255, 255, 255, 255, 255]);

        let mut meta = image.meta().clone();
        meta.channels = vec![ChannelMeta {
            name: None,
            lut: Some(Lut::from_fn("Red", |i| [i, 0, 0])),
            display_range: Some((0.0, 2000.0)),
        }];
        let out = render_plane(&image.with_meta(meta).unwrap(), &[]).unwrap();
        assert_eq!(out.data, [0, 0, 0, 255, 128, 0, 0, 255]);
    }

    #[test]
    fn rgb_combines_channels() {
        let mut meta = ImageMeta::xyczt(DType::U8, 1, 1, 3, 1, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = [10u8, 20, 30].map(|v| Buffer::from_vec(vec![v])).to_vec();
        let image = Image::from_planes(meta, planes).unwrap();
        assert_eq!(render_plane(&image, &[0]).unwrap().data, [10, 20, 30, 255]);
        // The channel coordinate does not matter for RGB.
        assert_eq!(render_plane(&image, &[2]).unwrap().data, [10, 20, 30, 255]);
        assert!(render_plane(&image, &[]).is_err());
    }

    #[test]
    fn picks_the_requested_plane() {
        let meta = ImageMeta::xyczt(DType::U8, 1, 1, 1, 2, 1).unwrap();
        let planes = vec![Buffer::from_vec(vec![5u8]), Buffer::from_vec(vec![200u8])];
        let image = Image::from_planes(meta, planes).unwrap();
        assert_eq!(
            render_plane(&image, &[1]).unwrap().data,
            [200, 200, 200, 255]
        );
        assert!(render_plane(&image, &[2]).is_err());
    }
}
