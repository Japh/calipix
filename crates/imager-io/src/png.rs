//! PNG, read and written the way ImageJ 1.x does it.
//!
//! # Reading
//!
//! ImageJ opens PNG through Java's ImageIO (`Opener.openUsingImageIO`) and
//! turns the result into one of its image types (`ImagePlus.setImage`).
//! What comes out, checked against ImageJ 1.54p on Java 21 for every case
//! (`fixtures/imagej/gen/22_png.ijm`):
//!
//! | File | Image |
//! |---|---|
//! | Grey, 8 bits | 8-bit |
//! | Grey, 1, 2 or 4 bits | 8-bit holding the stored values (0 to 1, 3 or 15), with a lookup table of that many grey levels from black to white |
//! | Grey, 16 bits | 16-bit |
//! | Palette, any depth | 8-bit holding the palette indices, with the palette as lookup table; no table for the 256-level grey ramp |
//! | RGB, 8 bits | RGB |
//! | RGB, 16 bits | 16-bit with three channels |
//! | Anything with transparency | 8-bit RGB, blended over white |
//!
//! Transparency is an alpha channel, a palette with at least one entry
//! that is not opaque, or a transparent colour. ImageJ draws such an image
//! onto an opaque white one, so a sample `v` with alpha `a` becomes
//! `(a * v + (255 - a) * 255) / 255` rounded to the nearest integer; 16-bit
//! samples and alpha are first reduced to 8 bits, as `v * 255 / 65535`
//! rounded to the nearest. A pixel of the transparent colour becomes white. Gamma and
//! background chunks are ignored.
//!
//! Where ImageR2 differs from ImageJ on purpose:
//!
//! - **Grey with transparency.** Java takes the grey samples of a
//!   grey-with-alpha image, and of a grey image with a transparent colour,
//!   to be linear light and converts them to sRGB before blending: a stored
//!   8-bit `v` becomes `round(255 * f(v / 255))` with `f(x) = 12.92 x` up to
//!   0.0031308 and `1.055 x^(1/2.4) - 0.055` above, so 1 becomes 13 and 128
//!   becomes 188, even where the image is opaque. No other grey or colour
//!   PNG is treated so. ImageR2 blends the stored values.
//! - **A transparent colour in a grey file of fewer than 8 bits.** Java
//!   honours it only when it is 0. ImageR2 honours any.
//! - **RGB, 16 bits.** ImageJ shows the three channels as a composite with
//!   red, green and blue lookup tables; ImageR2 marks them as RGB, as it does
//!   for 16-bit RGB TIFF. The sample values are the same.
//!
//! # Writing
//!
//! `PNG_Writer.writeImage` writes what `ImagePlus.getBufferedImage` shows,
//! except for 16-bit images with the grey lookup table, whose samples it
//! writes as they are. ImageR2 always writes samples:
//!
//! | Image | File | ImageJ |
//! |---|---|---|
//! | 8-bit | Grey, 8 bits | Same, but with a display range applied to the samples |
//! | 8-bit with a lookup table | Palette, 8 bits, 256 entries | Same |
//! | 16-bit | Grey, 16 bits; a lookup table is not saved | Same; with a lookup table, the 8-bit rendering as a palette file |
//! | RGB, 8 bits | RGB, 8 bits | Same |
//! | RGB, 16 bits | RGB, 16 bits | The 8-bit rendering |
//! | More than one plane | Refused | Only the plane shown; composites as their rendering |
//! | Other sample types | Refused | The 8-bit rendering |

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use imager_core::{AxisKind, Buffer, ColorModel, DType, Image, ImageMeta, Lut};
use png::{BitDepth, ColorType, DecodingError, EncodingError};

use crate::bmp::{decode_error, file_name, gray_image, indexed_image, rgb_image};
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

    /// Opens a file as ImageJ does; see the module documentation.
    fn read(&self, path: &Path) -> Result<Image> {
        let file = File::open(path).map_err(|e| Error::io(path, e))?;
        let file = decode(BufReader::new(file)).map_err(|e| decoding_error(path, e))?;
        file.into_image(path)
    }

    /// Saves the samples of a single 8-bit or 16-bit plane, or of an RGB
    /// image; see the module documentation.
    fn write(&self, image: &Image, path: &Path) -> Result<()> {
        let layout = layout(image.meta())?;
        let meta = image.meta();
        let planes = (0..meta.plane_count())
            .map(|p| image.read_plane(p))
            .collect::<imager_core::Result<Vec<_>>>()?;
        let mut data = Vec::with_capacity(meta.plane_len() * planes.len() * meta.dtype().size());
        match meta.dtype() {
            DType::U8 => {
                let channels = planes
                    .iter()
                    .map(|p| p.try_slice::<u8>())
                    .collect::<imager_core::Result<Vec<_>>>()?;
                for i in 0..meta.plane_len() {
                    data.extend(channels.iter().map(|c| c[i]));
                }
            }
            _ => {
                let channels = planes
                    .iter()
                    .map(|p| p.try_slice::<u16>())
                    .collect::<imager_core::Result<Vec<_>>>()?;
                for i in 0..meta.plane_len() {
                    data.extend(channels.iter().flat_map(|c| c[i].to_be_bytes()));
                }
            }
        }

        let file = File::create(path).map_err(|e| Error::io(path, e))?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), layout.width, layout.height);
        encoder.set_color(layout.color);
        encoder.set_depth(layout.depth);
        if let Some(lut) = &layout.palette {
            encoder.set_palette(lut.table.as_flattened().to_vec());
        }
        encoder
            .write_header()
            .and_then(|mut writer| {
                writer.write_image_data(&data)?;
                writer.finish()
            })
            .map_err(|e| match e {
                EncodingError::IoError(source) => Error::io(path, source),
                other => unsupported(other.to_string()),
            })
    }
}

fn unsupported(reason: String) -> Error {
    Error::Unsupported {
        format: "PNG",
        reason,
    }
}

/// How an image is laid out in a PNG file.
struct Layout {
    width: u32,
    height: u32,
    color: ColorType,
    depth: BitDepth,
    palette: Option<Lut>,
}

/// Chooses the PNG layout for an image, or explains why PNG cannot hold it.
///
/// ImageJ (`PNG_Writer.writeImage`) saves anything, by writing the 8-bit
/// rendering of the plane on screen when the image is not 8-bit, 16-bit
/// grey or RGB. That silently drops planes and precision, so it is refused
/// here.
fn layout(meta: &ImageMeta) -> Result<Layout> {
    let depth = match meta.dtype() {
        DType::U8 => BitDepth::Eight,
        DType::U16 => BitDepth::Sixteen,
        other => {
            return Err(unsupported(format!(
                "PNG holds 8-bit or 16-bit unsigned samples, but this image is {other}; \
                 convert it to 8-bit or 16-bit first, or save it as TIFF"
            )));
        }
    };
    let channels = meta.size(&AxisKind::Channel);
    let is_rgb = meta.color == ColorModel::Rgb && channels == 3;
    let planes_held = if is_rgb { 3 } else { 1 };
    if meta.plane_count() != planes_held {
        return Err(unsupported(if channels > 1 && !is_rgb {
            format!(
                "PNG holds greyscale or RGB, but this image has {channels} separate channels; \
                 convert it to RGB first, or save it as TIFF"
            )
        } else {
            format!(
                "PNG holds a single plane, but this image has {}; \
                 save it as TIFF, or save the planes one at a time",
                meta.plane_count() / planes_held
            )
        }));
    }
    let too_large = |_| unsupported("PNG cannot hold an image this large".to_string());
    let width = u32::try_from(meta.width()).map_err(too_large)?;
    let height = u32::try_from(meta.height()).map_err(too_large)?;

    // An 8-bit image with a lookup table is written as a palette file, as
    // `ByteProcessor.createBufferedImage` and Java's PNG writer do; the grey
    // ramp is no lookup table. PNG has no palette for 16-bit samples.
    let palette = meta
        .channels
        .first()
        .and_then(|c| c.lut.clone())
        .filter(|lut| !is_rgb && depth == BitDepth::Eight && lut.table != Lut::grays().table);
    let color = if is_rgb {
        ColorType::Rgb
    } else if palette.is_some() {
        ColorType::Indexed
    } else {
        ColorType::Grayscale
    };
    Ok(Layout {
        width,
        height,
        color,
        depth,
        palette,
    })
}

/// A PNG file's first image as stored: nothing expanded, scaled or
/// blended.
struct Decoded {
    width: usize,
    height: usize,
    color: ColorType,
    /// Bits per stored sample.
    depth: u8,
    /// Rows of `line_size` bytes, packed as in the file but not interlaced.
    data: Vec<u8>,
    line_size: usize,
    palette: Vec<[u8; 3]>,
    /// The transparency chunk as the decoder keeps it: an alpha for each
    /// of the first palette entries, or the samples of the transparent
    /// colour, in one byte each unless samples have 16 bits.
    trns: Vec<u8>,
}

fn decode(
    source: impl std::io::BufRead + std::io::Seek,
) -> std::result::Result<Decoded, DecodingError> {
    let mut decoder = png::Decoder::new(source);
    // Neither is used, and each is a way for a file to fail to open.
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let mut reader = decoder.read_info()?;

    // The size comes from the header alone, so a corrupt one must not be
    // able to abort the process by asking for more memory than there is.
    let size = reader
        .output_buffer_size()
        .ok_or(DecodingError::LimitsExceeded)?;
    let mut data = Vec::new();
    data.try_reserve_exact(size)
        .map_err(|_| DecodingError::LimitsExceeded)?;
    data.resize(size, 0u8);
    let frame = reader.next_frame(&mut data)?;
    data.truncate(frame.buffer_size());

    let info = reader.info();
    Ok(Decoded {
        width: frame.width as usize,
        height: frame.height as usize,
        color: frame.color_type,
        depth: frame.bit_depth as u8,
        data,
        line_size: frame.line_size,
        palette: info
            .palette
            .as_deref()
            .map(|p| p.as_chunks::<3>().0.to_vec())
            .unwrap_or_default(),
        trns: info.trns.as_deref().unwrap_or_default().to_vec(),
    })
}

/// A sample of `depth` bits as an 8-bit level. Sixteen bits are reduced as
/// Java reduces them when it draws the image, by rounding `v * 255 / 65535`.
fn level(sample: u16, depth: u8) -> u8 {
    match depth {
        16 => ((u32::from(sample) * 255 + 32767) / 65535) as u8,
        8 => sample as u8,
        _ => (u32::from(sample) * 255 / ((1 << depth) - 1)) as u8,
    }
}

/// An 8-bit level with 8-bit alpha drawn over white, as Java's `SrcOver`
/// drawing onto an opaque `TYPE_INT_ARGB` image computes it.
fn over_white(level: u8, alpha: u8) -> u8 {
    let (level, alpha) = (u32::from(level), u32::from(alpha));
    ((alpha * level + (255 - alpha) * 255 + 127) / 255) as u8
}

impl Decoded {
    /// The stored samples in reading order, channels interleaved.
    fn samples(&self) -> impl Iterator<Item = u16> + '_ {
        let depth = usize::from(self.depth);
        let per_row = self.width * self.color.samples();
        // A PNG has at least one pixel, so rows are never empty.
        self.data
            .chunks_exact(self.line_size.max(1))
            .flat_map(move |row| {
                (0..per_row).map(move |i| match depth {
                    16 => u16::from_be_bytes([row[i * 2], row[i * 2 + 1]]),
                    8 => u16::from(row[i]),
                    _ => {
                        // The leftmost pixel is in the most significant bits.
                        let bit = i * depth;
                        u16::from(row[bit / 8] >> (8 - depth - bit % 8)) & ((1 << depth) - 1)
                    }
                })
            })
    }

    /// The stored pixels of an image with `N` samples per pixel.
    fn pixels<const N: usize>(&self) -> impl Iterator<Item = [u16; N]> + '_ {
        let mut samples = self.samples();
        std::iter::from_fn(move || {
            let mut pixel = [samples.next()?; N];
            for sample in &mut pixel[1..] {
                *sample = samples.next()?;
            }
            Some(pixel)
        })
    }

    /// The image ImageJ makes of the file (`Opener.openUsingImageIO`,
    /// `ImagePlus.setImage`); see the module documentation.
    fn into_image(self, path: &Path) -> Result<Image> {
        let (width, height) = (self.width, self.height);
        if let Some(rgb) = self.blended() {
            return rgb_image(path, width, height, &[rgb]);
        }
        let bytes = || self.samples().map(|v| v as u8).collect::<Vec<u8>>();
        match (self.color, self.depth) {
            (ColorType::Grayscale, 8) => gray_image(path, width, height, vec![bytes()]),
            (ColorType::Grayscale, 16) => {
                let meta = ImageMeta::plane(DType::U16, width, height)?.with_name(file_name(path));
                let plane: Vec<u16> = self.samples().collect();
                Ok(Image::from_planes(meta, vec![Buffer::from_vec(plane)])?)
            }
            (ColorType::Grayscale, depth) => {
                // Java gives a grey image of fewer than 8 bits a palette of
                // its grey levels.
                let palette: Vec<[u8; 3]> =
                    (0..1u16 << depth).map(|v| [level(v, depth); 3]).collect();
                indexed_image(path, width, height, bytes(), &palette)
            }
            // Java pads a palette with black to the size the depth allows,
            // and the lookup table is black beyond the palette.
            (ColorType::Indexed, _) => indexed_image(path, width, height, bytes(), &self.palette),
            (ColorType::Rgb, 8) => rgb_image(path, width, height, &[bytes()]),
            (ColorType::Rgb, 16) => {
                let mut meta = ImageMeta::xyczt(DType::U16, width, height, 3, 1, 1)?
                    .with_name(file_name(path));
                meta.color = ColorModel::Rgb;
                let planes = (0..3)
                    .map(|c| {
                        let channel: Vec<u16> = self.samples().skip(c).step_by(3).collect();
                        Buffer::from_vec(channel)
                    })
                    .collect();
                Ok(Image::from_planes(meta, planes)?)
            }
            (color, depth) => Err(decode_error(
                path,
                format!("unsupported PNG layout: {color:?} with {depth} bits per sample"),
            )),
        }
    }

    /// Interleaved 8-bit RGB of the image drawn over white, if the file has
    /// transparency.
    fn blended(&self) -> Option<Vec<u8>> {
        let depth = self.depth;
        // The transparent colour of a grey or RGB file, and the alpha of
        // each palette entry.
        let mut key = [0u16; 3];
        let alphas = &self.trns[..self.trns.len().min(self.palette.len())];
        match self.color {
            ColorType::GrayscaleAlpha | ColorType::Rgba => {}
            // A palette whose entries are all opaque has no transparency,
            // whatever chunks the file has (`IndexColorModel.setRGBs`).
            ColorType::Indexed if alphas.iter().any(|&a| a != 255) => {}
            ColorType::Indexed => return None,
            ColorType::Grayscale | ColorType::Rgb => {
                // The decoder keeps one byte of each sample of the colour
                // unless samples have two.
                let bytes = if depth == 16 { 2 } else { 1 };
                let stored = self.trns.chunks_exact(bytes);
                if stored.len() != self.color.samples() {
                    return None;
                }
                for (sample, stored) in key.iter_mut().zip(stored) {
                    *sample = stored.iter().fold(0, |v, &b| v << 8 | u16::from(b));
                }
            }
        }

        let mut rgb = Vec::with_capacity(self.width * self.height * 3);
        match self.color {
            ColorType::GrayscaleAlpha => {
                for [gray, alpha] in self.pixels() {
                    let blended = over_white(level(gray, depth), level(alpha, depth));
                    rgb.extend([blended; 3]);
                }
            }
            ColorType::Rgba => {
                for [r, g, b, alpha] in self.pixels() {
                    let alpha = level(alpha, depth);
                    rgb.extend([r, g, b].map(|v| over_white(level(v, depth), alpha)));
                }
            }
            ColorType::Indexed => {
                for index in self.samples() {
                    let index = usize::from(index);
                    let color = self.palette.get(index).copied().unwrap_or([0; 3]);
                    let alpha = alphas.get(index).copied().unwrap_or(255);
                    rgb.extend(color.map(|v| over_white(v, alpha)));
                }
            }
            ColorType::Grayscale => {
                for gray in self.samples() {
                    let shown = if gray == key[0] {
                        255
                    } else {
                        level(gray, depth)
                    };
                    rgb.extend([shown; 3]);
                }
            }
            ColorType::Rgb => {
                for pixel in self.pixels::<3>() {
                    if pixel == key {
                        rgb.extend([255; 3]);
                    } else {
                        rgb.extend(pixel.map(|v| level(v, depth)));
                    }
                }
            }
        }
        Some(rgb)
    }
}

fn decoding_error(path: &Path, error: DecodingError) -> Error {
    match error {
        DecodingError::IoError(source) if source.kind() == std::io::ErrorKind::UnexpectedEof => {
            decode_error(path, "the PNG file ends before its image data is complete")
        }
        DecodingError::IoError(source) => Error::io(path, source),
        DecodingError::LimitsExceeded => decode_error(
            path,
            "the PNG image is too large to open in the memory available",
        ),
        other => decode_error(path, format!("not a valid PNG file: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open, save};
    use imager_core::ChannelMeta;

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
    fn lookup_table_round_trips_as_palette() {
        let lut = Lut::from_fn("Test", |i| [i, 255 - i, i / 2]);
        let mut meta = ImageMeta::plane(DType::U8, 3, 1).unwrap();
        meta.channels = vec![ChannelMeta {
            lut: Some(lut.clone()),
            ..ChannelMeta::default()
        }];
        let image = Image::from_planes(meta, vec![Buffer::from_vec(vec![0u8, 7, 255])]).unwrap();
        let path = temp("lut.png");
        save(&image, &path).unwrap();
        let back = open(&path).unwrap();
        assert_eq!(back.read_plane(0).unwrap(), image.read_plane(0).unwrap());
        let back_lut = back.meta().channels[0].lut.as_ref().unwrap();
        assert_eq!(back_lut.table, lut.table);

        // The grey ramp is what an image without a lookup table shows, so
        // it makes a greyscale file.
        let mut meta = ImageMeta::plane(DType::U8, 1, 1).unwrap();
        meta.channels = vec![ChannelMeta {
            lut: Some(Lut::grays()),
            ..ChannelMeta::default()
        }];
        let path = temp("ramp.png");
        save(
            &Image::from_planes(meta, vec![Buffer::from_vec(vec![9u8])]).unwrap(),
            &path,
        )
        .unwrap();
        assert!(open(&path).unwrap().meta().channels.is_empty());
    }

    #[test]
    fn levels_and_blend_round_as_java_does() {
        assert_eq!(level(1, 1), 255);
        assert_eq!(level(2, 2), 170);
        assert_eq!(level(7, 4), 119);
        assert_eq!(level(200, 8), 200);
        // 128.498 and 128.502, either side of a half.
        assert_eq!(level(33024, 16), 128);
        assert_eq!(level(33025, 16), 129);
        assert_eq!(level(65535, 16), 255);

        assert_eq!(over_white(0, 0), 255);
        assert_eq!(over_white(0, 255), 0);
        assert_eq!(over_white(77, 255), 77);
        // 127.498, then 254.498 and 254.502: to the nearest, not down.
        assert_eq!(over_white(0, 128), 127);
        assert_eq!(over_white(127, 1), 254);
        assert_eq!(over_white(128, 1), 255);
        assert_eq!(over_white(0, 1), 254);
    }

    #[test]
    fn refuses_what_png_cannot_hold() {
        let float = Image::from_vec(1, 1, vec![0.5f32]).unwrap();
        let err = save(&float, temp("float.png")).unwrap_err();
        assert!(err.to_string().contains("f32"), "{err}");
        assert!(err.to_string().contains("TIFF"), "{err}");

        let meta = ImageMeta::xyczt(DType::U8, 1, 1, 1, 2, 1).unwrap();
        let stack = Image::from_planes(meta, vec![Buffer::zeros(DType::U8, 1); 2]).unwrap();
        let path = temp("stack.png");
        let err = save(&stack, &path).unwrap_err();
        assert!(err.to_string().contains("single plane"), "{err}");
        assert!(err.to_string().contains("this image has 2"), "{err}");
        // A refusal leaves no file behind.
        assert!(!path.exists());

        let mut meta = ImageMeta::xyczt(DType::U16, 1, 1, 2, 1, 1).unwrap();
        meta.color = ColorModel::Composite;
        let channels = Image::from_planes(meta, vec![Buffer::zeros(DType::U16, 1); 2]).unwrap();
        let err = save(&channels, temp("channels.png")).unwrap_err();
        assert!(err.to_string().contains("2 separate channels"), "{err}");

        let mut meta = ImageMeta::xyczt(DType::U8, 1, 1, 3, 2, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let rgb_stack = Image::from_planes(meta, vec![Buffer::zeros(DType::U8, 1); 6]).unwrap();
        let err = save(&rgb_stack, temp("rgb-stack.png")).unwrap_err();
        assert!(err.to_string().contains("this image has 2"), "{err}");
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
        let err = open(&broken).unwrap_err();
        assert!(matches!(err, Error::Decode { .. }), "{err}");
        assert!(err.to_string().contains("ends before"), "{err}");
        // So is one that is not a PNG beyond its signature.
        let fake = temp("fake.png");
        std::fs::write(&fake, b"\x89PNG\r\n\x1a\nnothing of the kind").unwrap();
        let err = open(&fake).unwrap_err();
        assert!(matches!(err, Error::Decode { .. }), "{err}");
    }
}
