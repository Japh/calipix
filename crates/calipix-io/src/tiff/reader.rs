//! Opening a TIFF: deciding how its pixels are laid out and building the
//! image description. No pixel data is read here.

use std::collections::BTreeMap;
use std::sync::Arc;

use calipix_core::{Axis, AxisKind, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut};

use super::ifd::{Ifd, Storage, TiffError, TiffFile, TiffResult, format_err, tag, unsupported};
use super::imagej::{
    Description, IjMeta, decode_text, default_channel_lut, hex_encode, lut_from_planar, named_lut,
};
use super::ome::{self, OmeImage};
use super::source::{ChunkedSource, Geometry, RawSource};

/// The tags of one directory that decide how its pixels are decoded.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IfdInfo {
    pub width: u32,
    pub height: u32,
    pub bits: u16,
    /// Samples per pixel as stored.
    pub spp: u16,
    /// SampleFormat: 1 unsigned, 2 signed, 3 floating point.
    pub format: u16,
    pub compression: u32,
    pub photometric: u16,
    /// PlanarConfiguration 2 with more than one sample.
    pub planar: bool,
    pub predictor: u16,
    pub tile: Option<(u32, u32)>,
    pub rows_per_strip: u32,
}

impl IfdInfo {
    pub(crate) fn parse(file: &TiffFile, ifd: &Ifd) -> TiffResult<IfdInfo> {
        let get = |t: u16| file.uint(ifd, t);
        let dimension = |t: u16, what: &str| -> TiffResult<u32> {
            match get(t)? {
                Some(v) if v > 0 && v <= u64::from(u32::MAX) => Ok(v as u32),
                Some(v) => format_err(format!("{what} is {v}")),
                None => format_err(format!("{what} is missing")),
            }
        };
        let width = dimension(tag::IMAGE_WIDTH, "ImageWidth")?;
        let height = dimension(tag::IMAGE_LENGTH, "ImageLength")?;
        let uniform = |t: u16, default: u64, what: &str| -> TiffResult<u64> {
            let Some(entry) = ifd.find(t) else {
                return Ok(default);
            };
            let values = file.uints(entry)?;
            match values.first() {
                None => Ok(default),
                Some(&first) if values.iter().all(|&v| v == first) => Ok(first),
                Some(_) => unsupported(format!(
                    "samples with different {what} ({values:?}) are not supported"
                )),
            }
        };
        let bits = uniform(tag::BITS_PER_SAMPLE, 1, "bit depths")?;
        let spp = get(tag::SAMPLES_PER_PIXEL)?.unwrap_or(1);
        if spp == 0 || spp > 1024 {
            return format_err(format!("SamplesPerPixel is {spp}"));
        }
        let format = match uniform(tag::SAMPLE_FORMAT, 1, "sample formats")? {
            // "Undefined" data is treated as unsigned, as most readers do.
            4 => 1,
            other => other,
        };
        let compression = match get(tag::COMPRESSION)?.unwrap_or(1) {
            0 => 1,
            other => other,
        };
        let tile = match (get(tag::TILE_WIDTH)?, get(tag::TILE_LENGTH)?) {
            (Some(w), Some(h)) => {
                if w == 0 || h == 0 || w > u64::from(u32::MAX) || h > u64::from(u32::MAX) {
                    return format_err(format!("tile size is {w}x{h}"));
                }
                Some((w as u32, h as u32))
            }
            _ => None,
        };
        let rows_per_strip = match get(tag::ROWS_PER_STRIP)? {
            Some(rows) if rows > 0 && rows < u64::from(height) => rows as u32,
            _ => height,
        };
        Ok(IfdInfo {
            width,
            height,
            bits: bits.min(u64::from(u16::MAX)) as u16,
            spp: spp as u16,
            format: format.min(u64::from(u16::MAX)) as u16,
            compression: compression.min(u64::from(u32::MAX)) as u32,
            photometric: get(tag::PHOTOMETRIC)?.unwrap_or(1).min(65_535) as u16,
            planar: spp > 1 && get(tag::PLANAR_CONFIGURATION)? == Some(2),
            predictor: get(tag::PREDICTOR)?.unwrap_or(1).min(65_535) as u16,
            tile,
            rows_per_strip,
        })
    }

    /// The pixel type of the samples, or an error naming whatever about
    /// this directory we cannot decode.
    pub(crate) fn dtype(&self) -> TiffResult<DType> {
        let dtype = match (self.bits, self.format) {
            (8, 1) => DType::U8,
            (8, 2) => DType::I8,
            (16, 1) => DType::U16,
            (16, 2) => DType::I16,
            (32, 1) => DType::U32,
            (32, 2) => DType::I32,
            (32, 3) => DType::F32,
            (64, 1) => DType::U64,
            (64, 2) => DType::I64,
            (64, 3) => DType::F64,
            (bits, 3) => {
                return unsupported(format!(
                    "{bits}-bit floating-point samples are not supported"
                ));
            }
            (_, 5 | 6) => return unsupported("complex samples are not supported"),
            (bits @ (8 | 16 | 32 | 64), format) => {
                return format_err(format!("unknown SampleFormat {format} for {bits}-bit data"));
            }
            (bits, _) => return unsupported(format!("{bits}-bit samples are not supported")),
        };
        match self.compression {
            1 | 5 | 8 | 32946 | 32773 => {}
            other => {
                let name = match other {
                    2..=4 => "CCITT fax",
                    6 | 7 => "JPEG",
                    34712 => "JPEG 2000",
                    50000 => "Zstandard",
                    50001 => "WebP",
                    34925 => "LZMA",
                    _ => "unknown",
                };
                return unsupported(format!(
                    "{name} compression (TIFF compression {other}) is not supported"
                ));
            }
        }
        match self.photometric {
            0 | 1 => {}
            2 if self.spp >= 3 => {}
            2 => return format_err("RGB data with fewer than 3 samples per pixel"),
            3 if self.spp == 1 && self.bits == 8 => {}
            3 => {
                return unsupported(format!(
                    "palette images with {}-bit indices or {} samples per pixel are not supported",
                    self.bits, self.spp
                ));
            }
            other => {
                let name = match other {
                    4 => "transparency mask",
                    5 => "CMYK",
                    6 => "YCbCr",
                    8..=10 => "CIE L*a*b*",
                    32803 => "colour filter array",
                    _ => "unknown",
                };
                return unsupported(format!(
                    "{name} colour data (PhotometricInterpretation {other}) is not supported"
                ));
            }
        }
        Ok(dtype)
    }

    /// Number of channels the image exposes: RGB keeps its first three
    /// samples (alpha and other extra samples are discarded, as for PNG),
    /// anything else keeps every sample as a channel.
    pub(crate) fn used_samples(&self) -> usize {
        if self.photometric == 2 {
            3
        } else {
            usize::from(self.spp)
        }
    }

    /// True if `other` can be another plane of a stack that starts with
    /// `self` (`Opener.allSameSizeAndType`).
    fn same_kind(&self, other: &IfdInfo) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.bits == other.bits
            && self.spp == other.spp
            && self.format == other.format
            && self.planar == other.planar
            && (self.photometric == 2) == (other.photometric == 2)
    }
}

/// Spatial and temporal calibration as ImageJ's `Calibration` holds it.
#[derive(Debug)]
struct Calibration {
    pixel_width: f64,
    pixel_height: f64,
    pixel_depth: f64,
    unit: Option<String>,
    y_unit: Option<String>,
    z_unit: Option<String>,
    frame_interval: f64,
    time_unit: Option<String>,
    /// Origins in pixels, as ImageJ stores them.
    origin: [f64; 3],
}

/// `Calibration.sanitizeUnit`, with ImageJ's "pixel" (uncalibrated) and
/// blank units mapped to `None`.
fn sanitize_unit(unit: &str) -> Option<String> {
    match unit {
        "um" => Some("\u{b5}m".to_string()),
        "A" => Some("\u{c5}".to_string()),
        u if u.trim().is_empty() || u == "pixel" || u == "pixels" => None,
        u => Some(u.to_string()),
    }
}

/// Reproduces `TiffDecoder.OpenIFD` (resolution tags) followed by
/// `FileOpener.decodeDescriptionString` and `FileOpener.setCalibration`.
fn calibration(file: &TiffFile, ifd: &Ifd, ij: Option<&Description>) -> TiffResult<Calibration> {
    let mut width = 1.0;
    let mut height = 1.0;
    let mut depth = 1.0;
    let mut unit: Option<String> = None;
    // ImageJ computes 1/(numerator/denominator); dividing the other way
    // round is the same value to within one unit in the last place and lets
    // sizes such as 0.3 survive a save exactly.
    let size = |t: u16| -> TiffResult<Option<f64>> {
        Ok(match file.rational(ifd, t)? {
            Some((n, d)) if n != 0 && d != 0 => Some(f64::from(d) / f64::from(n)),
            _ => None,
        })
    };
    if let Some(w) = size(tag::X_RESOLUTION)? {
        width = w;
    }
    if let Some(h) = size(tag::Y_RESOLUTION)? {
        height = h;
    }
    match file.uint(ifd, tag::RESOLUTION_UNIT)? {
        Some(1) => unit = Some(" ".to_string()),
        Some(2) => {
            // 72 dpi is what software writes when it knows nothing.
            if file.rational(ifd, tag::X_RESOLUTION)? == Some((72, 1)) || width == 1.0 / 72.0 {
                width = 1.0;
                height = 1.0;
            } else {
                unit = Some("inch".to_string());
            }
        }
        Some(3) => unit = Some("cm".to_string()),
        _ => {}
    }
    if let Some(ij) = ij {
        let described = ij.get("unit").unwrap_or("");
        if unit.as_deref() == Some("cm") && described == "um" {
            width *= 10000.0;
            height *= 10000.0;
        }
        unit = Some(described.to_string());
        if let Some(spacing) = ij.number("spacing") {
            depth = spacing.abs();
        }
    }
    let mut cal = Calibration {
        pixel_width: 1.0,
        pixel_height: 1.0,
        pixel_depth: 1.0,
        unit: None,
        y_unit: None,
        z_unit: None,
        frame_interval: 0.0,
        time_unit: None,
        origin: [0.0; 3],
    };
    if let Some(mut unit) = unit.filter(|_| width > 0.0) {
        // `Prefs.convertToMicrons`, on by default.
        let threshold = if ij.is_some() { 0.0001 } else { 0.001 };
        if width <= threshold && unit == "cm" {
            width *= 10000.0;
            height *= 10000.0;
            if depth != 1.0 {
                depth *= 10000.0;
            }
            unit = "um".to_string();
        }
        cal.pixel_width = width;
        cal.pixel_height = height;
        cal.pixel_depth = depth;
        cal.unit = sanitize_unit(&unit);
    }
    if let Some(ij) = ij {
        cal.origin = [
            ij.double("xorigin"),
            ij.double("yorigin"),
            ij.double("zorigin"),
        ];
        cal.frame_interval = ij.double("finterval");
        cal.time_unit = Some(ij.get("tunit").unwrap_or("sec").to_string());
        cal.y_unit = ij.get("yunit").and_then(sanitize_unit);
        cal.z_unit = ij.get("zunit").and_then(sanitize_unit);
    }
    Ok(cal)
}

fn axis(kind: AxisKind, scale: f64, origin_px: f64, unit: Option<String>) -> Axis {
    let scale = if scale.is_finite() && scale != 0.0 {
        scale
    } else {
        1.0
    };
    Axis {
        kind,
        scale,
        // ImageJ's origin is the pixel index of physical zero.
        origin: if origin_px == 0.0 || !origin_px.is_finite() {
            0.0
        } else {
            -origin_px * scale
        },
        unit,
    }
}

/// ImageJ reads only the high byte of each 16-bit palette entry
/// (`TiffDecoder.getColorMap`).
fn color_map(file: &TiffFile, ifd: &Ifd, name: &str) -> TiffResult<Option<Lut>> {
    let Some(entry) = ifd.find(tag::COLOR_MAP) else {
        return Ok(None);
    };
    if entry.count != 768 || entry.typ != super::ifd::field::SHORT {
        return Ok(None);
    }
    let raw = file.raw(entry)?;
    let high = |i: usize| (file.order.u16(&raw[i * 2..]) >> 8) as u8;
    let table: [[u8; 3]; 256] = std::array::from_fn(|i| [high(i), high(256 + i), high(512 + i)]);
    Ok(Some(named_lut(table, name)))
}

/// Description keys that are turned into image structure and calibration
/// (and regenerated from them when writing) rather than kept as properties.
pub(crate) const STRUCTURAL_KEYS: [&str; 19] = [
    "ImageJ",
    "images",
    "channels",
    "slices",
    "frames",
    "hyperstack",
    "mode",
    "unit",
    "yunit",
    "zunit",
    "spacing",
    "finterval",
    "tunit",
    "min",
    "max",
    "xorigin",
    "yorigin",
    "zorigin",
    "name",
];

fn checked_plane_bytes(info: &IfdInfo, bps: usize) -> TiffResult<u64> {
    let too_large = || {
        TiffError::Format(format!(
            "a {}x{} image with {} samples per pixel is too large to address",
            info.width, info.height, info.spp
        ))
    };
    let samples = u64::from(info.width)
        .checked_mul(u64::from(info.height))
        .ok_or_else(too_large)?;
    let bytes = samples
        .checked_mul(bps as u64 * u64::from(info.spp))
        .ok_or_else(too_large)?;
    // One plane must be addressable in memory.
    if bytes > isize::MAX as u64 / 2 {
        return Err(too_large());
    }
    Ok(samples * bps as u64)
}

/// Where the first image's uncompressed pixels start, if they form one
/// contiguous block that can be read by offset alone.
fn contiguous_offset(
    file: &TiffFile,
    ifd: &Ifd,
    info: &IfdInfo,
    bps: usize,
) -> TiffResult<Option<u64>> {
    if info.compression != 1 || info.tile.is_some() {
        return Ok(None);
    }
    let Some(entry) = ifd.find(tag::STRIP_OFFSETS) else {
        return Ok(None);
    };
    let offsets = file.uints(entry)?;
    let Some(&first) = offsets.first() else {
        return Ok(None);
    };
    // Rows are whole pixels unless the samples are stored as separate planes.
    let row =
        u64::from(info.width) * bps as u64 * if info.planar { 1 } else { u64::from(info.spp) };
    let (rows, height) = (u64::from(info.rows_per_strip), u64::from(info.height));
    let strips_per_plane = height.div_ceil(rows);
    let mut expected = first;
    for (i, &offset) in offsets.iter().enumerate() {
        if offset != expected {
            return Ok(None);
        }
        let strip = i as u64 % strips_per_plane;
        let strip_rows = rows.min(height - strip * rows);
        expected = offset.saturating_add(strip_rows * row);
    }
    Ok(Some(first))
}

/// How planes in our channel-fastest order map to images in the file when
/// an OME DimensionOrder says the file uses a different order.
fn ome_image_map(order: [char; 3], c: usize, z: usize, t: usize) -> Option<Vec<u32>> {
    if order == ['C', 'Z', 'T'] {
        return None;
    }
    let size = |axis: char| match axis {
        'C' => c,
        'Z' => z,
        _ => t,
    };
    let mut map = Vec::with_capacity(c * z * t);
    for ti in 0..t {
        for zi in 0..z {
            for ci in 0..c {
                let position = |axis: char| match axis {
                    'C' => ci,
                    'Z' => zi,
                    _ => ti,
                };
                let index = position(order[0])
                    + size(order[0]) * (position(order[1]) + size(order[1]) * position(order[2]));
                map.push(index as u32);
            }
        }
    }
    Some(map)
}

/// Opens a TIFF from `storage`. `file_name` becomes the image name.
pub(crate) fn open(storage: Arc<dyn Storage>, file_name: &str) -> TiffResult<Image> {
    let file = TiffFile::open(storage)?;
    let first = file.read_ifd(file.first_ifd)?;
    let info = IfdInfo::parse(&file, &first)?;
    let stored = info.dtype()?;
    let bps = stored.size();
    let plane_bytes = checked_plane_bytes(&info, bps)?;
    let image_bytes = plane_bytes * u64::from(info.spp);

    let description = file
        .bytes(&first, tag::IMAGE_DESCRIPTION)?
        .map(|b| decode_text(&b))
        .filter(|t| !t.is_empty());
    let ij_text = description.as_deref().filter(|t| t.starts_with("ImageJ="));
    let ij = ij_text.map(Description::parse);
    let ij_meta = match (ij.is_some(), first.find(tag::IJ_META_COUNTS)) {
        (true, Some(counts)) => {
            let counts = file.uints(counts)?;
            let data = file.bytes(&first, tag::IJ_META_DATA)?.unwrap_or_default();
            IjMeta::parse(file.order, &counts, &data)
        }
        _ => IjMeta::default(),
    };
    let ome_image: Option<OmeImage> = match description.as_deref() {
        Some(text) if ij.is_none() && ome::is_ome_xml(text) => ome::parse(text, file_name)?,
        _ => None,
    };

    // Where the pixels are. ImageJ's own files hold every plane in one
    // block after the first directory (`TiffEncoder`), so they are read by
    // offset; `images=N` makes ImageJ ignore all further directories
    // (`TiffDecoder.getTiffInfo`), and so do we.
    let block = match &ij {
        Some(_) => contiguous_offset(&file, &first, &info, bps)?,
        None => None,
    };
    let declared = ij.as_ref().map_or(0.0, |d| d.double("images"));
    let declared = if declared > 1.0 && declared < 1e12 {
        declared as usize
    } else {
        1
    };
    enum Pixels {
        Block { offset: u64, images: usize },
        Directories(Vec<u64>),
    }
    let pixels = match block {
        Some(offset) if declared > 1 => Pixels::Block {
            offset,
            images: declared,
        },
        _ => {
            // Every directory of the same size and type is a plane; if
            // they differ (thumbnails, pyramids) only the first image is
            // opened (`Opener.openTiffStack`).
            let chain = file.ifd_chain()?;
            let mut same = true;
            for (_, ifd) in chain.iter().skip(1) {
                let other = IfdInfo::parse(&file, ifd)?;
                same &= info.same_kind(&other) && other.dtype().is_ok();
            }
            match block {
                Some(offset) if chain.len() == 1 || !same => Pixels::Block { offset, images: 1 },
                _ if same => Pixels::Directories(chain.iter().map(|(o, _)| *o).collect()),
                _ => Pixels::Directories(vec![file.first_ifd]),
            }
        }
    };
    let mut images = match &pixels {
        Pixels::Block { offset, images } => {
            let needed = (*images as u64)
                .checked_mul(image_bytes)
                .and_then(|b| b.checked_add(*offset));
            if needed.is_none_or(|n| n > file.storage.len()) {
                return format_err(format!(
                    "the pixel data is truncated: {images} image(s) of {image_bytes} bytes at byte {offset} do not fit in a file of {} bytes",
                    file.storage.len()
                ));
            }
            *images
        }
        Pixels::Directories(offsets) => offsets.len(),
    };

    // Dimensions: `FileOpener.setCalibration` accepts channels, slices and
    // frames only if they multiply to the stack size; a plain stack is Z.
    let rgb = info.photometric == 2;
    let used = info.used_samples();
    let nonzero = |v: f64| {
        if (1.0..1e12).contains(&v) {
            v as usize
        } else {
            1
        }
    };
    let (dc, dz, dt) = ij.as_ref().map_or((1, 1, 1), |d| {
        (
            nonzero(d.double("channels")),
            nonzero(d.double("slices")),
            nonzero(d.double("frames")),
        )
    });
    let (mut channels, mut slices, mut frames) = if used > 1 {
        if dc == 1 && dz.checked_mul(dt) == Some(images) {
            (used, dz, dt)
        } else {
            (used, images, 1)
        }
    } else if dc.checked_mul(dz).and_then(|n| n.checked_mul(dt)) == Some(images) {
        (dc, dz, dt)
    } else {
        (1, images, 1)
    };
    let mut image_map = None;
    let mut ome_dims = false;
    if let Some(ome) = &ome_image {
        // With several samples per pixel the samples are the channels.
        let plane_channels = if used > 1 { 1 } else { ome.size_c };
        let wanted = plane_channels
            .checked_mul(ome.size_z)
            .and_then(|n| n.checked_mul(ome.size_t));
        let fits = wanted.is_some_and(|n| n <= images) && (used == 1 || ome.size_c == used);
        if let (true, Some(wanted)) = (fits, wanted) {
            // Further directories belong to other images of the dataset.
            images = wanted;
            (channels, slices, frames) = (plane_channels.max(used), ome.size_z, ome.size_t);
            image_map = ome_image_map(ome.order, plane_channels, ome.size_z, ome.size_t);
            ome_dims = true;
        }
    }

    // ImageJ's "16-bit signed": unsigned storage plus the calibration
    // function value = raw - 32768 (`Calibration.isSigned16Bit`).
    let signed16 = stored == DType::U16
        && ij.as_ref().is_some_and(|d| {
            d.get("vunit").is_some()
                && (0.0..=13.0).contains(&d.number("cf").unwrap_or(0.0))
                && d.number("c0") == Some(-32768.0)
                && d.number("c1") == Some(1.0)
        });
    let dtype = if signed16 { DType::I16 } else { stored };
    let range_shift = if signed16 { -32768.0 } else { 0.0 };

    // Axes and calibration.
    let cal = calibration(&file, &first, ij.as_ref())?;
    let mut x = axis(
        AxisKind::X,
        cal.pixel_width,
        cal.origin[0],
        cal.unit.clone(),
    );
    let mut y = axis(
        AxisKind::Y,
        cal.pixel_height,
        cal.origin[1],
        cal.y_unit.clone().or(cal.unit.clone()),
    );
    let mut z = axis(
        AxisKind::Z,
        cal.pixel_depth,
        cal.origin[2],
        cal.z_unit.clone().or(cal.unit.clone()),
    );
    let mut time = if cal.frame_interval != 0.0 && cal.frame_interval.is_finite() {
        axis(
            AxisKind::Time,
            cal.frame_interval,
            0.0,
            cal.time_unit.clone(),
        )
    } else {
        Axis::new(AxisKind::Time)
    };
    if let Some(ome) = &ome_image {
        for (target, value) in [
            (&mut x, &ome.physical_x),
            (&mut y, &ome.physical_y),
            (&mut z, &ome.physical_z),
            (&mut time, &ome.time_increment),
        ] {
            if let Some((scale, unit)) = value {
                target.scale = *scale;
                target.unit = Some(unit.clone());
            }
        }
    }
    let mut axes = vec![(x, info.width as usize), (y, info.height as usize)];
    for (axis, size) in [
        (Axis::new(AxisKind::Channel), channels),
        (z, slices),
        (time, frames),
    ] {
        if size != 1 {
            axes.push((axis, size));
        }
    }
    let invalid = |e: calipix_core::Error| TiffError::Format(e.to_string());
    let mut meta = ImageMeta::new(dtype, axes).map_err(invalid)?;
    meta.name = ij
        .as_ref()
        .and_then(|d| d.get("name"))
        .unwrap_or(file_name)
        .to_string();

    // Channels: lookup tables, display ranges and how they combine.
    let mut luts: Vec<Option<Lut>> = vec![None; channels];
    let mut ranges: Vec<Option<(f64, f64)>> = vec![None; channels];
    let mut names: Vec<Option<String>> = vec![None; channels];
    let mut stored_mode = None;
    if rgb {
        meta.color = ColorModel::Rgb;
    } else {
        let described = ij.as_ref().and_then(|d| {
            let (min, max) = (d.double("min"), d.double("max"));
            (!(min == 0.0 && max == 0.0)).then_some((min + range_shift, max + range_shift))
        });
        let composite = channels > 1
            && (ij_text.is_some_and(|t| t.contains("mode="))
                || ij.as_ref().is_some_and(|d| d.flag("hyperstack")));
        if composite {
            // `Opener.makeComposite` and `CompositeImage.setupLuts`.
            let text = ij_text.unwrap_or_default();
            let mode = if text.contains("mode=composite") {
                "composite"
            } else if text.contains("mode=gray") {
                "grayscale"
            } else {
                "color"
            };
            if mode == "composite" {
                meta.color = ColorModel::Composite;
            }
            for (i, lut) in luts.iter_mut().enumerate() {
                *lut = match ij_meta.luts.get(i) {
                    // A table of the wrong size becomes white
                    // (`CompositeImage.createLutFromBytes`).
                    Some(bytes) => Some(
                        lut_from_planar(bytes, "Custom").unwrap_or_else(|| default_channel_lut(3)),
                    ),
                    None if ij_meta.luts.is_empty() && mode == "grayscale" => None,
                    None => Some(default_channel_lut(i)),
                };
            }
            if mode == "grayscale" && !ij_meta.luts.is_empty() {
                stored_mode = Some(mode);
            }
            for (i, range) in ranges.iter_mut().enumerate() {
                *range = if ij_meta.ranges.len() == channels * 2 {
                    Some((
                        ij_meta.ranges[i * 2] + range_shift,
                        ij_meta.ranges[i * 2 + 1] + range_shift,
                    ))
                } else {
                    described
                };
            }
        } else {
            // One lookup table for the whole image: a stored palette, or
            // inverted greys for WhiteIsZero (the samples themselves are
            // never inverted; `FileOpener.createColorModel`). ImageJ ignores
            // palettes on 16-bit files it did not write.
            let current = ij_meta
                .props
                .iter()
                .find(|(k, _)| k == "CurrentLUT")
                .map_or("Custom", |(_, v)| v.as_str());
            let ignore_palette =
                info.photometric != 3 && stored == DType::U16 && description.is_none();
            let lut = match color_map(&file, &first, current)? {
                Some(lut) if !ignore_palette => Some(lut),
                _ if info.photometric == 0 => Some(Lut::grays().inverted()),
                _ => None,
            };
            luts.fill(lut);
            ranges.fill(described);
        }
    }
    if let Some(ome) = &ome_image
        && ome_dims
        && ome.channel_names.len() == channels
    {
        names.clone_from(&ome.channel_names);
    }
    if luts.iter().any(Option::is_some)
        || ranges.iter().any(Option::is_some)
        || names.iter().any(Option::is_some)
    {
        meta.channels = luts
            .into_iter()
            .zip(ranges)
            .zip(names)
            .map(|((lut, display_range), name)| ChannelMeta {
                name,
                lut,
                display_range,
            })
            .collect();
    }

    meta.properties = properties(&file, &first, ij.as_ref(), &ij_meta, signed16, stored_mode)?;
    if ij.is_none()
        && let Some(text) = description
    {
        meta.properties.insert("tiff.description".to_string(), text);
    }
    meta.validate().map_err(invalid)?;

    let geometry = Geometry {
        width: info.width as usize,
        height: info.height as usize,
        dtype,
        bytes_per_sample: bps,
        samples: usize::from(info.spp),
        used,
        planar: info.planar,
        signed16,
        image_map,
        images,
    };
    Ok(match pixels {
        Pixels::Block { offset, .. } => Image::cached(RawSource::new(
            meta,
            file.storage.clone(),
            geometry,
            file.order,
            offset,
            image_bytes,
        )),
        Pixels::Directories(offsets) => {
            Image::cached(ChunkedSource::new(meta, file, geometry, info, offsets))
        }
    })
}

/// Everything that does not fit the structured part of [`ImageMeta`], under
/// the keys documented on the module.
fn properties(
    file: &TiffFile,
    ifd: &Ifd,
    ij: Option<&Description>,
    ij_meta: &IjMeta,
    signed16: bool,
    stored_mode: Option<&str>,
) -> TiffResult<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for (key, t) in [
        ("tiff.software", tag::SOFTWARE),
        ("tiff.datetime", tag::DATE_TIME),
        ("tiff.artist", tag::ARTIST),
        ("tiff.host_computer", tag::HOST_COMPUTER),
    ] {
        if ifd
            .find(t)
            .is_some_and(|e| e.typ == super::ifd::field::ASCII)
            && let Some(bytes) = file.bytes(ifd, t)?
        {
            let text = decode_text(&bytes);
            if !text.is_empty() {
                map.insert(key.to_string(), text);
            }
        }
    }
    if let Some(ij) = ij {
        for (key, value) in &ij.0 {
            let structural = STRUCTURAL_KEYS.contains(&key.as_str())
                || (signed16 && matches!(key.as_str(), "cf" | "c0" | "c1" | "vunit"))
                || (key == "loop" && value != "true");
            if !structural && !key.is_empty() {
                map.insert(format!("imagej.{key}"), value.clone());
            }
        }
        if let Some(mode) = stored_mode {
            map.insert("imagej.mode".to_string(), mode.to_string());
        }
    }
    for (key, value) in &ij_meta.props {
        if !key.is_empty() && key != "Info" {
            map.insert(key.clone(), value.clone());
        }
    }
    if let Some(info) = &ij_meta.info {
        map.insert("Info".to_string(), info.clone());
    }
    for (i, label) in ij_meta.labels.iter().enumerate() {
        if let Some(label) = label {
            map.insert(format!("imagej.label.{i}"), label.clone());
        }
    }
    if let Some(roi) = &ij_meta.roi {
        map.insert("imagej.roi".to_string(), hex_encode(roi));
    }
    for (i, element) in ij_meta.overlay.iter().enumerate() {
        map.insert(format!("imagej.overlay.{i}"), hex_encode(element));
    }
    Ok(map)
}
