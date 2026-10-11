use std::fs::File;
use std::io::{BufWriter, Cursor, Write};
use std::path::Path;

use calipix_core::{Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut};
use image::ImageFormat;

use crate::{Error, Format, Result};

pub(crate) struct Bmp;

const FILE_HEADER: usize = 14;
const INFO_HEADER: usize = 40;

impl Format for Bmp {
    fn name(&self) -> &'static str {
        "BMP"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["bmp", "dib"]
    }

    fn matches(&self, header: &[u8]) -> bool {
        // "BM", and at offset 14 the size of one of the known info headers.
        // ImageJ accepts any file starting with "BM" (`Opener.getFileType`),
        // but only after ruling out text; the header size does that here.
        header.starts_with(b"BM")
            && header.len() >= 16
            && matches!(header[14], 12 | 16 | 40 | 52 | 56 | 64 | 108 | 124)
            && header[15] == 0
    }

    /// Uncompressed files with a palette (1, 2, 4 or 8 bits per pixel) open
    /// as one 8-bit plane of palette indices with the palette as the
    /// channel's lookup table, as ImageJ's `BMP_Reader` followed by
    /// `ImagePlus.setImage` does. Everything else opens as RGB; the alpha
    /// byte of 32-bit files is ignored (`BMPDecoder.unpack32`).
    fn read(&self, path: &Path) -> Result<Image> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        if let Some(indexed) = read_indexed(&bytes).map_err(|m| decode_error(path, m))? {
            return indexed_image(
                path,
                indexed.width,
                indexed.height,
                indexed.indices,
                &indexed.palette,
            );
        }
        let decoded = image::ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Bmp)
            .decode()
            // The file is already in memory, so an "I/O error" here is the
            // decoder running off the end of the data.
            .map_err(|e| match e {
                image::ImageError::IoError(_) => decode_error(path, "the BMP file is truncated"),
                other => image_error(path, other),
            })?;
        let (width, height) = (decoded.width() as usize, decoded.height() as usize);
        let rgb = decoded.into_rgb8().into_raw();
        rgb_image(path, width, height, std::slice::from_ref(&rgb))
    }

    /// Writes what ImageJ's `BMP_Writer` writes: RGB images as 24 bits per
    /// pixel, single 8-bit planes as 8 bits per pixel with the channel's
    /// lookup table (or a grey ramp) as a 256-entry palette. Rows are stored
    /// bottom-up and padded to four bytes.
    fn write(&self, image: &Image, path: &Path) -> Result<()> {
        let meta = image.meta();
        let frames = frame_planes(meta, "BMP")?;
        if frames.len() != 1 {
            return Err(Error::Unsupported {
                format: "BMP",
                reason: format!(
                    "BMP holds a single plane, but this image has {}",
                    meta.plane_count()
                ),
            });
        }
        let (width, height) = (meta.width(), meta.height());
        if i32::try_from(width).is_err() || i32::try_from(height).is_err() {
            return Err(Error::Unsupported {
                format: "BMP",
                reason: format!("{width} x {height} is larger than BMP allows"),
            });
        }

        let (samples, bytes_per_pixel, palette) = match &frames[0] {
            Frame::Gray(plane) => {
                let data = image.read_plane(*plane)?.try_slice::<u8>()?.to_vec();
                (data, 1, Some(lut_or_gray(meta)))
            }
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
                let mut bgr = Vec::with_capacity(width * height * 3);
                for i in 0..width * height {
                    bgr.extend([b[i], g[i], r[i]]);
                }
                (bgr, 3, None)
            }
        };

        let row_bytes = width * bytes_per_pixel;
        let pad = (4 - row_bytes % 4) % 4;
        let palette_bytes = if palette.is_some() { 256 * 4 } else { 0 };
        let offset = FILE_HEADER + INFO_HEADER + palette_bytes;
        let file_size = offset as u64 + (row_bytes + pad) as u64 * height as u64;
        let Ok(file_size) = u32::try_from(file_size) else {
            return Err(Error::Unsupported {
                format: "BMP",
                reason: "the image is larger than the 4 GiB a BMP file can hold".to_string(),
            });
        };

        let file = File::create(path).map_err(|e| Error::io(path, e))?;
        let mut out = BufWriter::new(file);
        let mut emit = || -> std::io::Result<()> {
            out.write_all(b"BM")?;
            out.write_all(&file_size.to_le_bytes())?;
            out.write_all(&[0; 4])?;
            out.write_all(&(offset as u32).to_le_bytes())?;

            out.write_all(&(INFO_HEADER as u32).to_le_bytes())?;
            out.write_all(&(width as i32).to_le_bytes())?;
            out.write_all(&(height as i32).to_le_bytes())?;
            out.write_all(&1u16.to_le_bytes())?;
            out.write_all(&(bytes_per_pixel as u16 * 8).to_le_bytes())?;
            // No compression; image size and resolution left at zero, as
            // BMP_Writer leaves them.
            out.write_all(&[0; 16])?;
            let colors: u32 = if palette.is_some() { 256 } else { 0 };
            out.write_all(&colors.to_le_bytes())?;
            out.write_all(&[0; 4])?;

            if let Some(lut) = &palette {
                for [r, g, b] in lut.table.iter() {
                    out.write_all(&[*b, *g, *r, 0])?;
                }
            }
            for row in samples.chunks_exact(row_bytes).rev() {
                out.write_all(row)?;
                out.write_all(&[0u8; 3][..pad])?;
            }
            out.flush()
        };
        emit().map_err(|e| Error::io(path, e))
    }
}

struct Indexed {
    width: usize,
    height: usize,
    indices: Vec<u8>,
    palette: Vec<[u8; 3]>,
}

/// Decodes an uncompressed palette BMP to its indices, following ImageJ's
/// `BMPDecoder`. Returns `None` for every other kind of BMP (and for files
/// too short to tell), which the general decoder then handles or rejects.
fn read_indexed(bytes: &[u8]) -> std::result::Result<Option<Indexed>, String> {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    if bytes.len() < FILE_HEADER + INFO_HEADER || !bytes.starts_with(b"BM") {
        return Ok(None);
    }
    let header_size = u32_at(14) as usize;
    let (width, height) = (u32_at(18) as i32, u32_at(22) as i32);
    let (bits, compression) = (u16_at(28) as usize, u32_at(30));
    if header_size < INFO_HEADER
        || compression != 0
        || !matches!(bits, 1 | 2 | 4 | 8)
        || width <= 0
        || height == 0
        || height == i32::MIN
    {
        return Ok(None);
    }
    // A negative height marks rows stored top to bottom.
    let top_down = height < 0;
    let (width, height) = (width as usize, height.unsigned_abs() as usize);

    let colors = match u32_at(46) {
        0 => 1 << bits,
        n => n.min(256) as usize,
    };
    let palette_start = FILE_HEADER
        .checked_add(header_size)
        .ok_or("invalid BMP header size")?;
    let palette_end = palette_start + colors * 4;
    let Some(table) = bytes.get(palette_start..palette_end) else {
        return Err("BMP colour table is truncated".to_string());
    };
    let palette = table
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| [c[2], c[1], c[0]])
        .collect();

    // Rows are padded to a multiple of four bytes.
    let row_bytes = (width * bits).div_ceil(32) * 4;
    let start = (u32_at(10) as usize).max(palette_end);
    let data = row_bytes
        .checked_mul(height)
        .and_then(|len| start.checked_add(len))
        .and_then(|end| bytes.get(start..end))
        .ok_or_else(|| {
            format!("BMP pixel data is truncated ({width} x {height} at {bits} bits per pixel)")
        })?;

    let mut indices = vec![0u8; width * height];
    let mask = ((1u16 << bits) - 1) as u8;
    for (i, raw) in data.chunks_exact(row_bytes).enumerate() {
        let y = if top_down { i } else { height - 1 - i };
        let row = &mut indices[y * width..(y + 1) * width];
        for (x, index) in row.iter_mut().enumerate() {
            let bit = x * bits;
            // The leftmost pixel is in the most significant bits.
            *index = (raw[bit / 8] >> (8 - bits - bit % 8)) & mask;
        }
    }
    Ok(Some(Indexed {
        width,
        height,
        indices,
        palette,
    }))
}

// Helpers shared by the BMP, GIF and JPEG modules.

/// The file name of `path`, used as the image name.
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn decode_error(path: &Path, message: impl Into<String>) -> Error {
    Error::Decode {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

pub(crate) fn image_error(path: &Path, error: image::ImageError) -> Error {
    match error {
        image::ImageError::IoError(source) => Error::io(path, source),
        other => decode_error(path, other.to_string()),
    }
}

/// A lookup table for a file palette, or `None` if the palette is the
/// 256-level grey ramp that images have by default. Entries the palette
/// does not define are black.
pub(crate) fn palette_lut(palette: &[[u8; 3]]) -> Option<Lut> {
    let is_ramp =
        palette.len() == 256 && palette.iter().enumerate().all(|(i, c)| *c == [i as u8; 3]);
    if is_ramp {
        return None;
    }
    Some(Lut::from_fn("Palette", |i| {
        palette.get(i as usize).copied().unwrap_or([0; 3])
    }))
}

/// One 8-bit plane of palette indices shown through the palette.
pub(crate) fn indexed_image(
    path: &Path,
    width: usize,
    height: usize,
    indices: Vec<u8>,
    palette: &[[u8; 3]],
) -> Result<Image> {
    let mut meta = ImageMeta::plane(DType::U8, width, height)?.with_name(file_name(path));
    if let Some(lut) = palette_lut(palette) {
        meta.channels = vec![ChannelMeta {
            lut: Some(lut),
            ..ChannelMeta::default()
        }];
    }
    Ok(Image::from_planes(meta, vec![Buffer::from_vec(indices)])?)
}

/// An RGB image from interleaved 8-bit RGB frames; more than one frame
/// becomes a Z stack.
pub(crate) fn rgb_image(
    path: &Path,
    width: usize,
    height: usize,
    frames: &[Vec<u8>],
) -> Result<Image> {
    let mut meta =
        ImageMeta::xyczt(DType::U8, width, height, 3, frames.len(), 1)?.with_name(file_name(path));
    meta.color = ColorModel::Rgb;
    let mut planes = Vec::with_capacity(frames.len() * 3);
    for frame in frames {
        for c in 0..3 {
            let channel: Vec<u8> = frame.iter().skip(c).step_by(3).copied().collect();
            planes.push(Buffer::from_vec(channel));
        }
    }
    Ok(Image::from_planes(meta, planes)?)
}

/// An 8-bit greyscale image; more than one frame becomes a Z stack.
pub(crate) fn gray_image(
    path: &Path,
    width: usize,
    height: usize,
    frames: Vec<Vec<u8>>,
) -> Result<Image> {
    let meta =
        ImageMeta::xyczt(DType::U8, width, height, 1, frames.len(), 1)?.with_name(file_name(path));
    let planes = frames.into_iter().map(Buffer::from_vec).collect();
    Ok(Image::from_planes(meta, planes)?)
}

/// True if every pixel of an interleaved RGB frame has equal red, green and
/// blue, as `ColorProcessor.isGrayscale` tests.
pub(crate) fn is_gray(rgb: &[u8]) -> bool {
    rgb.as_chunks::<3>()
        .0
        .iter()
        .all(|p| p[0] == p[1] && p[0] == p[2])
}

/// What an 8-bit file format stores for one frame: plane numbers of a
/// greyscale plane or of the red, green and blue planes.
pub(crate) enum Frame {
    Gray(usize),
    Rgb([usize; 3]),
}

/// Groups the planes of an 8-bit image into the frames an 8-bit format can
/// hold, in stack order, or explains why the image cannot be saved.
pub(crate) fn frame_planes(meta: &ImageMeta, format: &'static str) -> Result<Vec<Frame>> {
    let unsupported = |reason: String| Error::Unsupported { format, reason };
    if meta.dtype() != DType::U8 {
        return Err(unsupported(format!(
            "{format} holds 8-bit samples, but this image is {}; convert it to 8-bit or RGB first",
            meta.dtype()
        )));
    }
    let channel_axis = meta.axis_index(&calipix_core::AxisKind::Channel);
    match (meta.color, channel_axis) {
        (ColorModel::Rgb, Some(axis)) => {
            let mut frames = Vec::new();
            for plane in 0..meta.plane_count() {
                let mut position = meta.plane_position(plane)?;
                if position[axis - 2] != 0 {
                    continue;
                }
                let mut planes = [plane; 3];
                for (c, slot) in planes.iter_mut().enumerate() {
                    position[axis - 2] = c;
                    *slot = meta.plane_index(&position)?;
                }
                frames.push(Frame::Rgb(planes));
            }
            Ok(frames)
        }
        (_, Some(_)) if meta.size(&calipix_core::AxisKind::Channel) > 1 => {
            Err(unsupported(format!(
                "{format} holds greyscale or RGB, but this image has {} separate channels; convert it to RGB first",
                meta.size(&calipix_core::AxisKind::Channel)
            )))
        }
        _ => Ok((0..meta.plane_count()).map(Frame::Gray).collect()),
    }
}

/// The lookup table of a single-channel image, or the grey ramp.
pub(crate) fn lut_or_gray(meta: &ImageMeta) -> Lut {
    meta.channels
        .first()
        .and_then(|c| c.lut.clone())
        .unwrap_or_else(Lut::grays)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_needs_a_known_header_size() {
        assert!(Bmp.matches(b"BM\x46\0\0\0\0\0\0\0\x36\0\0\0\x28\0"));
        assert!(!Bmp.matches(b"BMW sales report"));
        assert!(!Bmp.matches(b"BM"));
    }

    #[test]
    fn identity_ramp_needs_no_lut() {
        let ramp: Vec<[u8; 3]> = (0..=255).map(|i| [i; 3]).collect();
        assert!(palette_lut(&ramp).is_none());
        let lut = palette_lut(&[[0, 0, 0], [255, 0, 0]]).unwrap();
        assert_eq!(lut.table[1], [255, 0, 0]);
        assert_eq!(lut.table[2], [0, 0, 0]);
    }
}
