use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageFormat};
use imager_core::Image;

use crate::bmp::{Frame, decode_error, frame_planes, gray_image, image_error, is_gray, rgb_image};
use crate::{Error, Format, Result};

pub(crate) struct Jpeg;

/// ImageJ's quality setting in the fixtures and in most installations
/// (Edit > Options > Input/Output). [`Format::write`] has no parameters, so
/// this is not adjustable yet.
const QUALITY: u8 = 85;

impl Format for Jpeg {
    fn name(&self) -> &'static str {
        "JPEG"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["jpg", "jpeg", "jpe"]
    }

    fn matches(&self, header: &[u8]) -> bool {
        // The same three bytes `Opener.getFileType` tests.
        header.starts_with(&[0xff, 0xd8, 0xff])
    }

    /// Greyscale files become a single 8-bit plane and colour files RGB. A
    /// colour file in which every pixel is grey also opens as 8-bit, as
    /// `Opener.convertGrayJpegTo8Bits` does. EXIF orientation is not
    /// applied (ImageJ applies it only when an Exif_Reader plugin is
    /// installed).
    fn read(&self, path: &Path) -> Result<Image> {
        let file = File::open(path).map_err(|e| Error::io(path, e))?;
        let decoded = image::ImageReader::with_format(BufReader::new(file), ImageFormat::Jpeg)
            .decode()
            .map_err(|e| image_error(path, e))?;
        let (width, height) = (decoded.width() as usize, decoded.height() as usize);
        match decoded {
            DynamicImage::ImageLuma8(gray) => {
                gray_image(path, width, height, vec![gray.into_raw()])
            }
            DynamicImage::ImageRgb8(rgb) => {
                let rgb = rgb.into_raw();
                if is_gray(&rgb) {
                    let gray = rgb.iter().step_by(3).copied().collect();
                    gray_image(path, width, height, vec![gray])
                } else {
                    rgb_image(path, width, height, std::slice::from_ref(&rgb))
                }
            }
            other => Err(decode_error(
                path,
                format!(
                    "unsupported JPEG pixel layout {:?}; only 8-bit greyscale and colour JPEGs can be opened",
                    other.color()
                ),
            )),
        }
    }

    /// Writes 8-bit greyscale as a one-component JPEG and RGB as YCbCr, at
    /// quality 85, like `JpegWriter.saveAsJpeg`. Unlike ImageJ, the chroma
    /// channels are never subsampled and a lookup table or display range is
    /// not burned into the saved pixels.
    fn write(&self, image: &Image, path: &Path) -> Result<()> {
        let meta = image.meta();
        let unsupported = |reason: String| Error::Unsupported {
            format: "JPEG",
            reason,
        };
        let frames = frame_planes(meta, "JPEG")?;
        if frames.len() != 1 {
            return Err(unsupported(format!(
                "JPEG holds a single plane, but this image has {}",
                meta.plane_count()
            )));
        }
        let (width, height) = (meta.width(), meta.height());
        if width > 65535 || height > 65535 {
            return Err(unsupported(format!(
                "JPEG is limited to 65535 x 65535 pixels, but this image is {width} x {height}"
            )));
        }
        let (samples, color) = match &frames[0] {
            Frame::Gray(plane) => (
                image.read_plane(*plane)?.try_slice::<u8>()?.to_vec(),
                ExtendedColorType::L8,
            ),
            Frame::Rgb(planes) => {
                let [r, g, b] = [
                    image.read_plane(planes[0])?,
                    image.read_plane(planes[1])?,
                    image.read_plane(planes[2])?,
                ];
                let (r, g, b) = (
                    r.try_slice::<u8>()?,
                    g.try_slice::<u8>()?,
                    b.try_slice::<u8>()?,
                );
                let mut rgb = Vec::with_capacity(width * height * 3);
                for i in 0..width * height {
                    rgb.extend([r[i], g[i], b[i]]);
                }
                (rgb, ExtendedColorType::Rgb8)
            }
        };
        let file = File::create(path).map_err(|e| Error::io(path, e))?;
        let mut out = BufWriter::new(file);
        JpegEncoder::new_with_quality(&mut out, QUALITY)
            .encode(&samples, width as u32, height as u32, color)
            .map_err(|e| image_error(path, e))?;
        out.flush().map_err(|e| Error::io(path, e))
    }
}
