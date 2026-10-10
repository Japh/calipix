use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use image::{DynamicImage, ExtendedColorType, ImageError, ImageFormat};
use imager_core::{Buffer, ColorModel, DType, Image, ImageMeta, Pixel};

use crate::{Error, Format, Result};

pub(crate) struct Png;

impl Format for Png {
    fn name(&self) -> &'static str {
        "PNG"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["png"]
    }

    fn matches(&self, header: &[u8]) -> bool {
        header.starts_with(b"\x89PNG\r\n\x1a\n")
    }

    /// Greyscale files become a single plane and colour files three
    /// channels marked as RGB, at 8 or 16 bits as stored. Palettes are
    /// expanded and transparency is discarded.
    fn read(&self, path: &Path) -> Result<Image> {
        let file = File::open(path).map_err(|e| Error::io(path, e))?;
        let decoded = image::ImageReader::with_format(BufReader::new(file), ImageFormat::Png)
            .decode()
            .map_err(|e| image_error(path, e))?;
        let (width, height) = (decoded.width() as usize, decoded.height() as usize);

        let (dtype, planes) = match decoded {
            DynamicImage::ImageLuma8(_) | DynamicImage::ImageLumaA8(_) => (
                DType::U8,
                vec![Buffer::from_vec(decoded.into_luma8().into_raw())],
            ),
            DynamicImage::ImageLuma16(_) | DynamicImage::ImageLumaA16(_) => (
                DType::U16,
                vec![Buffer::from_vec(decoded.into_luma16().into_raw())],
            ),
            DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_) => {
                (DType::U8, split_channels(&decoded.into_rgb8().into_raw()))
            }
            DynamicImage::ImageRgb16(_) | DynamicImage::ImageRgba16(_) => {
                (DType::U16, split_channels(&decoded.into_rgb16().into_raw()))
            }
            other => {
                return Err(Error::Decode {
                    path: path.to_path_buf(),
                    message: format!("unsupported PNG pixel layout {:?}", other.color()),
                });
            }
        };

        let mut meta = ImageMeta::xyczt(dtype, width, height, planes.len(), 1, 1)?;
        if planes.len() == 3 {
            meta.color = ColorModel::Rgb;
        }
        meta.name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Image::from_planes(meta, planes)?)
    }

    fn write(&self, image: &Image, path: &Path) -> Result<()> {
        let meta = image.meta();
        let unsupported = |reason: String| Error::Unsupported {
            format: "PNG",
            reason,
        };
        let is_rgb = meta.color == ColorModel::Rgb && meta.plane_count() == 3;
        if meta.plane_count() != 1 && !is_rgb {
            return Err(unsupported(format!(
                "PNG holds a single plane, but this image has {}",
                meta.plane_count()
            )));
        }
        match meta.dtype() {
            DType::U8 => write_typed::<u8>(
                image,
                path,
                is_rgb,
                |s| s.to_vec(),
                ExtendedColorType::L8,
                ExtendedColorType::Rgb8,
            ),
            DType::U16 => write_typed::<u16>(
                image,
                path,
                is_rgb,
                // The encoder takes 16-bit samples in native byte order.
                |s| s.iter().flat_map(|v| v.to_ne_bytes()).collect(),
                ExtendedColorType::L16,
                ExtendedColorType::Rgb16,
            ),
            other => Err(unsupported(format!(
                "PNG holds 8-bit or 16-bit unsigned samples, but this image is {other}"
            ))),
        }
    }
}

/// Splits interleaved RGB samples into one buffer per channel.
fn split_channels<T: Pixel>(interleaved: &[T]) -> Vec<Buffer> {
    (0..3)
        .map(|c| {
            Buffer::from_vec(
                interleaved
                    .iter()
                    .skip(c)
                    .step_by(3)
                    .copied()
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn write_typed<T: Pixel>(
    image: &Image,
    path: &Path,
    is_rgb: bool,
    to_bytes: fn(&[T]) -> Vec<u8>,
    gray: ExtendedColorType,
    rgb: ExtendedColorType,
) -> Result<()> {
    let meta = image.meta();
    let (samples, color) = if is_rgb {
        let planes = (0..3)
            .map(|c| image.read_plane(c))
            .collect::<imager_core::Result<Vec<_>>>()?;
        let channels = planes
            .iter()
            .map(|p| p.try_slice::<T>())
            .collect::<imager_core::Result<Vec<_>>>()?;
        let mut interleaved = Vec::with_capacity(meta.plane_len() * 3);
        for ((&r, &g), &b) in channels[0].iter().zip(channels[1]).zip(channels[2]) {
            interleaved.extend([r, g, b]);
        }
        (interleaved, rgb)
    } else {
        (image.read_plane(0)?.try_slice::<T>()?.to_vec(), gray)
    };
    image::save_buffer_with_format(
        path,
        &to_bytes(&samples),
        meta.width() as u32,
        meta.height() as u32,
        color,
        ImageFormat::Png,
    )
    .map_err(|e| image_error(path, e))
}

fn image_error(path: &Path, error: ImageError) -> Error {
    match error {
        ImageError::IoError(source) => Error::io(path, source),
        other => Error::Decode {
            path: path.to_path_buf(),
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open, save};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("imager-io-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn assert_round_trip(image: &Image, name: &str) {
        let path = temp(name);
        save(image, &path).unwrap();
        let back = open(&path).unwrap();
        assert_eq!(back.meta().dtype(), image.meta().dtype());
        assert_eq!(back.meta().shape(), image.meta().shape());
        assert_eq!(back.meta().color, image.meta().color);
        assert_eq!(back.meta().name, name);
        for plane in 0..image.meta().plane_count() {
            assert_eq!(
                back.read_plane(plane).unwrap(),
                image.read_plane(plane).unwrap()
            );
        }
    }

    #[test]
    fn gray_round_trips() {
        let gray8 = Image::from_vec(3, 2, vec![0u8, 1, 2, 253, 254, 255]).unwrap();
        assert_round_trip(&gray8, "gray8.png");
        let gray16 = Image::from_vec(2, 2, vec![0u16, 255, 256, 65535]).unwrap();
        assert_round_trip(&gray16, "gray16.png");
    }

    #[test]
    fn rgb_round_trips() {
        let mut meta = ImageMeta::xyczt(DType::U8, 2, 1, 3, 1, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = vec![
            Buffer::from_vec(vec![10u8, 20]),
            Buffer::from_vec(vec![30u8, 40]),
            Buffer::from_vec(vec![50u8, 60]),
        ];
        assert_round_trip(&Image::from_planes(meta, planes).unwrap(), "rgb8.png");

        let mut meta = ImageMeta::xyczt(DType::U16, 1, 2, 3, 1, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = vec![
            Buffer::from_vec(vec![1000u16, 2000]),
            Buffer::from_vec(vec![3000u16, 4000]),
            Buffer::from_vec(vec![5000u16, 65535]),
        ];
        assert_round_trip(&Image::from_planes(meta, planes).unwrap(), "rgb16.png");
    }

    #[test]
    fn refuses_what_png_cannot_hold() {
        let float = Image::from_vec(1, 1, vec![0.5f32]).unwrap();
        let err = save(&float, temp("float.png")).unwrap_err();
        assert!(err.to_string().contains("f32"), "{err}");

        let meta = ImageMeta::xyczt(DType::U8, 1, 1, 1, 2, 1).unwrap();
        let stack = Image::from_planes(meta, vec![Buffer::zeros(DType::U8, 1); 2]).unwrap();
        let err = save(&stack, temp("stack.png")).unwrap_err();
        assert!(err.to_string().contains("single plane"), "{err}");
    }

    #[test]
    fn open_reports_problems() {
        assert!(matches!(open(temp("missing.png")), Err(Error::Io { .. })));
        let junk = temp("junk.xyz");
        std::fs::write(&junk, b"not an image").unwrap();
        assert!(matches!(open(&junk), Err(Error::UnknownFormat(_))));
        // Content wins over extension: a PNG named .dat still opens.
        let gray = Image::from_vec(1, 1, vec![9u8]).unwrap();
        let png = temp("real.png");
        save(&gray, &png).unwrap();
        let renamed = temp("real.dat");
        std::fs::copy(&png, &renamed).unwrap();
        assert_eq!(
            open(&renamed).unwrap().read_plane(0).unwrap().get_f64(0),
            Some(9.0)
        );
        // A truncated PNG is a decode error, not a panic.
        let bytes = std::fs::read(&png).unwrap();
        let broken = temp("broken.png");
        std::fs::write(&broken, &bytes[..bytes.len() / 2]).unwrap();
        assert!(open(&broken).is_err());
    }
}
