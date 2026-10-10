//! Writing TIFF the way ImageJ does (`ij.io.TiffEncoder` with the
//! description of `ij.io.FileSaver.getDescriptionString`): header, first
//! directory, its out-of-line values, every plane back to back, then one
//! directory per further image. Planes are read and written one at a time.

use std::io::{self, Write};

use imager_core::{AxisKind, Buffer, ColorModel, DType, Image, ImageMeta, Lut};

use super::ifd::{ByteOrder, field, tag};
use super::imagej::{
    IMAGEJ_VERSION, IjMeta, hex_decode, java_double, java_int_or_double, lut_to_planar,
    push_escaped_line,
};
use super::reader::STRUCTURAL_KEYS;

#[derive(Debug)]
pub(crate) enum WriteError {
    Unsupported(String),
    Io(io::Error),
    Core(imager_core::Error),
}

impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> WriteError {
        WriteError::Io(e)
    }
}

impl From<imager_core::Error> for WriteError {
    fn from(e: imager_core::Error) -> WriteError {
        WriteError::Core(e)
    }
}

/// Switches that force the layouts otherwise chosen only for files of
/// 4 GiB or more, so that tests can exercise them at small sizes.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Options {
    /// Write only the first directory, as ImageJ does for huge stacks.
    pub single_directory: bool,
    pub big_tiff: bool,
}

/// All output is little-endian ("Intel byte order").
const ORDER: ByteOrder = ByteOrder::Little;

/// Property keys with these prefixes describe the file an image came from
/// or are mapped to dedicated TIFF structures; everything else is stored as
/// an ImageJ image property.
const RESERVED_PREFIXES: [&str; 3] = ["imagej.", "tiff.", "ome."];

/// Description keys ImageJ writes itself, in the order it writes them
/// between the calibration and the end of the description.
const KNOWN_PROPERTY_KEYS: [&str; 16] = [
    "cf",
    "c0",
    "c1",
    "c2",
    "c3",
    "c4",
    "vunit",
    "zeroclip",
    "fps",
    "loop",
    "info",
    "inverty",
    "8bitcolor",
    "mode",
    "roi",
    "label",
];

/// How the image maps onto ImageJ's channel, slice and frame dimensions.
struct Plan<'a> {
    meta: &'a ImageMeta,
    rgb: bool,
    /// ImageJ's signed 16-bit: stored unsigned, offset by 32768.
    signed16: bool,
    /// Sizes and (index within the plane position) of the three axes.
    channels: (usize, Option<usize>),
    slices: (usize, Option<usize>),
    frames: (usize, Option<usize>),
}

impl Plan<'_> {
    /// Channels in ImageJ's sense: RGB is a pixel type there, not channels.
    fn ij_channels(&self) -> usize {
        if self.rgb { 1 } else { self.channels.0 }
    }

    fn images(&self) -> usize {
        self.ij_channels() * self.slices.0 * self.frames.0
    }

    fn samples(&self) -> usize {
        if self.rgb { 3 } else { 1 }
    }

    /// Our plane number for a channel, slice and frame.
    fn plane(&self, c: usize, z: usize, t: usize) -> imager_core::Result<usize> {
        let mut position = vec![0; self.meta.ndim() - 2];
        for ((_, axis), value) in [self.channels, self.slices, self.frames]
            .into_iter()
            .zip([c, z, t])
        {
            if let Some(axis) = axis {
                position[axis] = value;
            }
        }
        self.meta.plane_index(&position)
    }
}

fn plan(meta: &ImageMeta) -> Result<Plan<'_>, WriteError> {
    let dtype = meta.dtype();
    if dtype.is_complex() {
        return Err(WriteError::Unsupported(format!(
            "TIFF cannot hold complex samples ({dtype})"
        )));
    }
    let mut dims = [(1, None), (1, None), (1, None)];
    for (i, axis) in meta.axes().iter().enumerate().skip(2) {
        let slot = match &axis.kind {
            AxisKind::Channel => 0,
            AxisKind::Z => 1,
            AxisKind::Time => 2,
            other => {
                return Err(WriteError::Unsupported(format!(
                    "TIFF holds channel, Z and time axes, but this image has a {other:?} axis"
                )));
            }
        };
        dims[slot] = (meta.shape()[i], Some(i - 2));
    }
    if u32::try_from(meta.width()).is_err() || u32::try_from(meta.height()).is_err() {
        return Err(WriteError::Unsupported(
            "the image is too wide or tall for TIFF".to_string(),
        ));
    }
    Ok(Plan {
        meta,
        rgb: meta.color == ColorModel::Rgb,
        signed16: dtype == DType::I16 && meta.color != ColorModel::Rgb,
        channels: dims[0],
        slices: dims[1],
        frames: dims[2],
    })
}

/// XResolution and YResolution for the given pixel sizes, as
/// (pixels, units) fractions.
///
/// `TiffEncoder.writeScale` stores `1e6 / size` over `1e6`, truncated to
/// integers. Where that reads back as exactly the same size we write the
/// same numbers as ImageJ; where it would lose digits (a size of 0.3 comes
/// back as 0.30000003) we store the nearest fraction instead.
fn resolutions(width: f64, height: f64) -> Option<[(u32, u32); 2]> {
    let (exact_x, exact_y) = (resolution(width)?, resolution(height)?);
    let (x_scale, y_scale) = (1.0 / width, 1.0 / height);
    let max = f64::from(i32::MAX);
    let mut scale = 1_000_000.0;
    if x_scale * scale > max || y_scale * scale > max {
        scale = (max / x_scale.max(y_scale)).trunc();
    }
    let imagej = |per_unit: f64, size: f64, exact: (u32, u32)| {
        let (pixels, units) = ((per_unit * scale) as u32, scale as u32);
        if pixels > 0 && f64::from(units) / f64::from(pixels) == size {
            (pixels, units)
        } else {
            exact
        }
    };
    Some([
        imagej(x_scale, width, exact_x),
        imagej(y_scale, height, exact_y),
    ])
}

/// The smallest-termed fraction `pixels / units` equal (or as near as
/// 32-bit terms allow) to `1 / size`.
fn resolution(size: f64) -> Option<(u32, u32)> {
    if !(size.is_finite() && size > 0.0) {
        return None;
    }
    let max = u64::from(u32::MAX);
    // Convergents units/pixels of `size`.
    let (mut u0, mut p0, mut u1, mut p1) = (0u64, 1u64, 1u64, 0u64);
    let mut value = size;
    for _ in 0..64 {
        let whole = value.floor();
        if whole > max as f64 {
            break;
        }
        let a = whole as u64;
        let (Some(u2), Some(p2)) = (
            a.checked_mul(u1).and_then(|v| v.checked_add(u0)),
            a.checked_mul(p1).and_then(|v| v.checked_add(p0)),
        ) else {
            break;
        };
        if u2 > max || p2 > max {
            break;
        }
        (u0, p0, u1, p1) = (u1, p1, u2, p2);
        if u1 as f64 / p1 as f64 == size {
            break;
        }
        let rest = value - whole;
        if rest <= 0.0 {
            break;
        }
        value = 1.0 / rest;
    }
    if u1 == 0 || p1 == 0 {
        // Too small or too large for a ratio of 32-bit terms: store the
        // nearest representable resolution.
        let pixels = (1.0 / size).clamp(1.0 / max as f64, max as f64);
        return Some(if pixels >= 1.0 {
            (pixels as u32, 1)
        } else {
            (1, (1.0 / pixels) as u32)
        });
    }
    Some((p1 as u32, u1 as u32))
}

fn mode(plan: &Plan) -> &'static str {
    let meta = plan.meta;
    if meta.color == ColorModel::Composite {
        return "composite";
    }
    match meta.properties.get("imagej.mode").map(String::as_str) {
        Some("gray") => "gray",
        Some("color") => "color",
        _ if meta.channels.iter().any(|c| c.lut.is_some()) => "color",
        _ => "gray",
    }
}

/// `FileSaver.getDescriptionString`, from our metadata.
fn description(plan: &Plan) -> String {
    let meta = plan.meta;
    let images = plan.images();
    let property = |key: &str| meta.properties.get(&format!("imagej.{key}"));
    let axis = |kind: AxisKind| meta.axis_index(&kind).map(|i| &meta.axes()[i]);
    let (x, y) = (&meta.axes()[0], &meta.axes()[1]);
    let shift = if plan.signed16 { 32768.0 } else { 0.0 };

    let mut d = format!("ImageJ={IMAGEJ_VERSION}\n");
    // ImageJ leaves the count out for 48-bit RGB, which it reads back
    // directory by directory.
    if images > 1 && !(plan.rgb && meta.dtype() != DType::U8) {
        d.push_str(&format!("images={images}\n"));
    }
    if plan.ij_channels() > 1 {
        d.push_str(&format!("channels={}\n", plan.ij_channels()));
    }
    if plan.slices.0 > 1 {
        d.push_str(&format!("slices={}\n", plan.slices.0));
    }
    if plan.frames.0 > 1 {
        d.push_str(&format!("frames={}\n", plan.frames.0));
    }
    if plan.ij_channels() > 1 || plan.frames.0 > 1 {
        d.push_str("hyperstack=true\n");
    }
    if plan.ij_channels() > 1 {
        d.push_str(&format!("mode={}\n", mode(plan)));
    }
    if let Some(unit) = &x.unit {
        push_escaped_line(&mut d, "unit", unit);
    }
    if plan.signed16 {
        d.push_str("cf=0\nc0=-32768.0\nc1=1.0\nvunit=Gray Value\n");
    } else {
        for key in ["cf", "c0", "c1", "c2", "c3", "c4"] {
            if let Some(value) = property(key) {
                push_escaped_line(&mut d, key, value);
            }
        }
        for key in ["vunit", "zeroclip"] {
            if let Some(value) = property(key) {
                push_escaped_line(&mut d, key, value);
            }
        }
    }
    if let Some(time) = axis(AxisKind::Time)
        && (time.scale != 1.0 || time.unit.is_some())
    {
        d.push_str(&format!("finterval={}\n", java_int_or_double(time.scale)));
        if let Some(unit) = time.unit.as_deref().filter(|u| *u != "sec") {
            push_escaped_line(&mut d, "tunit", unit);
        }
    }
    if let Some(unit) = y.unit.as_ref().filter(|u| Some(*u) != x.unit.as_ref()) {
        push_escaped_line(&mut d, "yunit", unit);
    }
    let z = axis(AxisKind::Z);
    if let Some(unit) = z
        .and_then(|z| z.unit.as_ref())
        .filter(|u| Some(*u) != x.unit.as_ref())
    {
        push_escaped_line(&mut d, "zunit", unit);
    }
    if images > 1 {
        if let Some(z) = z.filter(|z| z.scale != 1.0) {
            d.push_str(&format!("spacing={}\n", java_double(z.scale)));
        }
        if let Some(fps) = property("fps") {
            push_escaped_line(&mut d, "fps", fps);
        }
        let looping = property("loop").is_some_and(|v| v == "true");
        d.push_str(&format!("loop={looping}\n"));
    }
    if !plan.rgb
        && let Some((min, max)) = meta.channels.first().and_then(|c| c.display_range)
    {
        d.push_str(&format!("min={}\n", java_double(min + shift)));
        d.push_str(&format!("max={}\n", java_double(max + shift)));
    }
    for (key, axis) in [("xorigin", Some(x)), ("yorigin", Some(y)), ("zorigin", z)] {
        if let Some(axis) = axis.filter(|a| a.origin != 0.0 && a.scale != 0.0) {
            d.push_str(&format!(
                "{key}={}\n",
                java_double(-axis.origin / axis.scale)
            ));
        }
    }
    for key in ["info", "inverty", "8bitcolor"] {
        if let Some(value) = property(key) {
            push_escaped_line(&mut d, key, value);
        }
    }
    // Description entries we do not interpret are carried through.
    for (key, value) in &meta.properties {
        if let Some(key) = key.strip_prefix("imagej.") {
            let stem = key.split('.').next().unwrap_or(key);
            if !KNOWN_PROPERTY_KEYS.contains(&stem)
                && stem != "overlay"
                && !key.contains(['=', ':', ' ', '\n', '\\'])
                && !STRUCTURAL_KEYS.contains(&key)
            {
                push_escaped_line(&mut d, key, value);
            }
        }
    }
    d
}

/// The blocks of tags 50838/50839 (`FileSaver.saveAsTiffStack`).
fn ij_meta(plan: &Plan) -> IjMeta {
    let meta = plan.meta;
    let images = plan.images();
    let shift = if plan.signed16 { 32768.0 } else { 0.0 };
    let mut out = IjMeta {
        info: meta.properties.get("Info").cloned(),
        ..IjMeta::default()
    };
    out.labels = (0..images)
        .map(|i| meta.properties.get(&format!("imagej.label.{i}")).cloned())
        .collect();
    if plan.ij_channels() > 1 {
        if meta.channels.iter().all(|c| c.display_range.is_some()) {
            out.ranges = meta
                .channels
                .iter()
                .filter_map(|c| c.display_range)
                .flat_map(|(min, max)| [min + shift, max + shift])
                .collect();
        }
        if meta.channels.iter().any(|c| c.lut.is_some()) {
            let white = Lut::grays();
            out.luts = meta
                .channels
                .iter()
                .map(|c| lut_to_planar(c.lut.as_ref().unwrap_or(&white)))
                .collect();
        }
    }
    out.roi = meta
        .properties
        .get("imagej.roi")
        .and_then(|h| hex_decode(h));
    let mut overlay: Vec<(usize, Vec<u8>)> = meta
        .properties
        .iter()
        .filter_map(|(key, value)| {
            let index = key.strip_prefix("imagej.overlay.")?.parse().ok()?;
            Some((index, hex_decode(value)?))
        })
        .collect();
    overlay.sort_by_key(|(index, _)| *index);
    out.overlay = overlay.into_iter().map(|(_, bytes)| bytes).collect();
    out.props = meta
        .properties
        .iter()
        .filter(|(key, _)| *key != "Info" && !RESERVED_PREFIXES.iter().any(|p| key.starts_with(p)))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    out
}

/// One directory entry: tag, field type, value count and value bytes.
struct Entry {
    tag: u16,
    typ: u16,
    count: u64,
    data: Vec<u8>,
}

fn shorts(tag: u16, values: &[u16]) -> Entry {
    Entry {
        tag,
        typ: field::SHORT,
        count: values.len() as u64,
        data: values.iter().flat_map(|&v| ORDER.put_u16(v)).collect(),
    }
}

fn longs(tag: u16, values: &[u32]) -> Entry {
    Entry {
        tag,
        typ: field::LONG,
        count: values.len() as u64,
        data: values.iter().flat_map(|&v| ORDER.put_u32(v)).collect(),
    }
}

/// A file offset or byte count, 64-bit in BigTIFF.
fn offset_entry(tag: u16, value: u64, big: bool) -> Entry {
    if big {
        Entry {
            tag,
            typ: field::LONG8,
            count: 1,
            data: ORDER.put_u64(value).to_vec(),
        }
    } else {
        // ImageJ writes 0 for a byte count that does not fit.
        longs(tag, &[u32::try_from(value).unwrap_or(0)])
    }
}

/// Sample bytes of a plane in file order.
fn plane_bytes(buffer: &Buffer, signed16: bool) -> Vec<u8> {
    fn le<T: Copy, const N: usize>(samples: &[T], f: fn(T) -> [u8; N]) -> Vec<u8> {
        samples.iter().flat_map(|&v| f(v)).collect()
    }
    match buffer {
        Buffer::U8(s) => s.to_vec(),
        Buffer::I8(s) => s.iter().map(|&v| v as u8).collect(),
        Buffer::U16(s) => le(s, u16::to_le_bytes),
        Buffer::I16(s) if signed16 => le(s, |v| ((v as u16) ^ 0x8000).to_le_bytes()),
        Buffer::I16(s) => le(s, i16::to_le_bytes),
        Buffer::U32(s) => le(s, u32::to_le_bytes),
        Buffer::I32(s) => le(s, i32::to_le_bytes),
        Buffer::U64(s) => le(s, u64::to_le_bytes),
        Buffer::I64(s) => le(s, i64::to_le_bytes),
        Buffer::F32(s) => le(s, f32::to_le_bytes),
        Buffer::F64(s) => le(s, f64::to_le_bytes),
        // Refused by `plan` before any plane is read.
        Buffer::C32(_) | Buffer::C64(_) => Vec::new(),
    }
}

/// Writes `image` to `out` from its start.
pub(crate) fn write<W: Write>(
    image: &Image,
    out: &mut W,
    options: Options,
) -> Result<(), WriteError> {
    let meta = image.meta();
    let plan = plan(meta)?;
    let dtype = meta.dtype();
    let bps = dtype.size();
    let samples = plan.samples();
    let images = plan.images();
    let (width, height) = (meta.width() as u64, meta.height() as u64);
    let image_bytes = width * height * (bps * samples) as u64;
    let stack_bytes = image_bytes
        .checked_mul(images as u64)
        .ok_or_else(|| WriteError::Unsupported("the image is too large for TIFF".to_string()))?;

    // ImageJ has 8-bit, 16-bit, float and RGB images; those are written
    // exactly as it would. Other types use the same layout with a
    // SampleFormat tag, and BigTIFF rather than ImageJ's single-directory
    // form when they outgrow 4 GiB.
    let imagej_type = match dtype {
        DType::U8 => true,
        DType::U16 | DType::F32 | DType::I16 => !plan.rgb,
        _ => false,
    };
    let lut = (!plan.rgb && plan.ij_channels() == 1)
        .then(|| meta.channels.first().and_then(|c| c.lut.as_ref()))
        .flatten();
    let (x, y) = (&meta.axes()[0], &meta.axes()[1]);
    let scaled = x.scale != 1.0 || y.scale != 1.0 || x.unit.is_some();
    let ratios = scaled
        .then(|| resolutions(x.scale, y.scale))
        .flatten()
        .map(|[rx, ry]| (rx, ry));
    let mut text = description(&plan).into_bytes();
    text.push(0);
    let blocks = ij_meta(&plan).build(ORDER, images);

    let build = |big: bool| -> Vec<Entry> {
        let mut entries = vec![
            longs(tag::NEW_SUBFILE_TYPE, &[0]),
            longs(tag::IMAGE_WIDTH, &[width as u32]),
            longs(tag::IMAGE_LENGTH, &[height as u32]),
            shorts(tag::BITS_PER_SAMPLE, &vec![(bps * 8) as u16; samples]),
            shorts(tag::COMPRESSION, &[1]),
            shorts(
                tag::PHOTOMETRIC,
                &[if plan.rgb {
                    2
                } else if lut.is_some() && dtype == DType::U8 {
                    3
                } else {
                    1
                }],
            ),
            Entry {
                tag: tag::IMAGE_DESCRIPTION,
                typ: field::ASCII,
                count: text.len() as u64,
                data: text.clone(),
            },
            offset_entry(tag::STRIP_OFFSETS, 0, big),
            shorts(tag::SAMPLES_PER_PIXEL, &[samples as u16]),
            longs(tag::ROWS_PER_STRIP, &[height as u32]),
            offset_entry(tag::STRIP_BYTE_COUNTS, image_bytes, big),
        ];
        if let Some(((xn, xd), (yn, yd))) = ratios {
            for (tag, n, d) in [(tag::X_RESOLUTION, xn, xd), (tag::Y_RESOLUTION, yn, yd)] {
                entries.push(Entry {
                    tag,
                    typ: field::RATIONAL,
                    count: 1,
                    data: [ORDER.put_u32(n), ORDER.put_u32(d)].concat(),
                });
            }
            let unit = match x.unit.as_deref() {
                Some("inch") => 2,
                Some("cm") => 3,
                _ => 1,
            };
            entries.push(shorts(tag::RESOLUTION_UNIT, &[unit]));
        }
        if let Some(lut) = lut {
            // ImageJ stores each 8-bit level in the high byte.
            let planar = lut_to_planar(lut);
            let map: Vec<u16> = planar.iter().map(|&v| u16::from(v) << 8).collect();
            entries.push(shorts(tag::COLOR_MAP, &map));
        }
        let format = match dtype {
            DType::F32 | DType::F64 => Some(3),
            DType::I8 | DType::I32 | DType::I64 => Some(2),
            DType::I16 if !plan.signed16 => Some(2),
            DType::U32 | DType::U64 => Some(1),
            // ImageJ's signed 16-bit is unsigned on disk.
            _ => None,
        };
        if let Some(format) = format {
            entries.push(shorts(tag::SAMPLE_FORMAT, &vec![format; samples]));
        }
        if let Some((counts, data)) = &blocks {
            entries.push(longs(tag::IJ_META_COUNTS, counts));
            entries.push(Entry {
                tag: tag::IJ_META_DATA,
                typ: field::BYTE,
                count: data.len() as u64,
                data: data.clone(),
            });
        }
        entries.sort_by_key(|e| e.tag);
        entries
    };

    // Sizes of the pieces, to know every offset before writing anything.
    let layout = |big: bool, entries: &[Entry]| -> (u64, u64, u64, u64) {
        let (header, count, entry, inline) = if big { (16, 8, 20, 8) } else { (8, 2, 12, 4) };
        let first = count + entries.len() as u64 * entry + inline;
        let private = entries
            .iter()
            .filter(|e| matches!(e.tag, tag::IJ_META_COUNTS | tag::IJ_META_DATA))
            .count() as u64;
        let values: u64 = entries
            .iter()
            .filter(|e| e.data.len() as u64 > inline)
            .map(|e| e.data.len() as u64)
            .sum();
        (header, first, first - private * entry, values)
    };
    let classic = build(false);
    let (header, first_size, _, values_size) = layout(false, &classic);
    // `TiffEncoder.write`: past this point offsets no longer fit in 32 bits.
    let too_big = (header + first_size + values_size)
        .saturating_add(stack_bytes)
        .saturating_add(images as u64 * first_size)
        >= 0xffff_ffff;
    let big = options.big_tiff || (too_big && !imagej_type);
    let single = images > 1 && (options.single_directory || (too_big && imagej_type));
    let entries = if big { build(true) } else { classic };
    let (header, first_size, later_size, values_size) = layout(big, &entries);
    let inline = if big { 8 } else { 4 };
    let pixels_at = header + first_size + values_size;
    let later_at = pixels_at + stack_bytes;

    let put_offset = |out: &mut Vec<u8>, v: u64| {
        if big {
            out.extend(ORDER.put_u64(v));
        } else {
            out.extend(ORDER.put_u32(v as u32));
        }
    };
    // Where each out-of-line value lives: after the first directory, in
    // tag order. Later directories point at the same values.
    let mut value_at = Vec::with_capacity(entries.len());
    let mut next_value = header + first_size;
    for entry in &entries {
        if entry.data.len() as u64 > inline as u64 {
            value_at.push(Some(next_value));
            next_value += entry.data.len() as u64;
        } else {
            value_at.push(None);
        }
    }
    let directory = |index: usize, next: u64| -> Vec<u8> {
        let kept: Vec<usize> = (0..entries.len())
            .filter(|&k| {
                index == 0 || !matches!(entries[k].tag, tag::IJ_META_COUNTS | tag::IJ_META_DATA)
            })
            .collect();
        let mut out = Vec::new();
        if big {
            out.extend(ORDER.put_u64(kept.len() as u64));
        } else {
            out.extend(ORDER.put_u16(kept.len() as u16));
        }
        for k in kept {
            let entry = &entries[k];
            out.extend(ORDER.put_u16(entry.tag));
            out.extend(ORDER.put_u16(entry.typ));
            put_offset(&mut out, entry.count);
            if entry.tag == tag::STRIP_OFFSETS {
                put_offset(&mut out, pixels_at + index as u64 * image_bytes);
            } else if let Some(at) = value_at[k] {
                put_offset(&mut out, at);
            } else {
                let mut value = vec![0u8; inline];
                value[..entry.data.len()].copy_from_slice(&entry.data);
                out.extend(value);
            }
        }
        put_offset(&mut out, next);
        out
    };

    let mut head = Vec::new();
    head.extend(b"II");
    if big {
        head.extend(ORDER.put_u16(43));
        head.extend(ORDER.put_u16(8));
        head.extend(ORDER.put_u16(0));
        head.extend(ORDER.put_u64(header));
    } else {
        head.extend(ORDER.put_u16(42));
        head.extend(ORDER.put_u32(header as u32));
    }
    let chained = images > 1 && !single;
    head.extend(directory(0, if chained { later_at } else { 0 }));
    for (entry, at) in entries.iter().zip(&value_at) {
        if at.is_some() {
            head.extend(&entry.data);
        }
    }
    debug_assert_eq!(head.len() as u64, pixels_at);
    out.write_all(&head)?;

    // Pixels in ImageJ's stack order: channel fastest, then slice, then
    // frame; RGB samples interleaved.
    for t in 0..plan.frames.0 {
        for z in 0..plan.slices.0 {
            if plan.rgb {
                let planes = (0..3)
                    .map(|c| Ok(plane_bytes(&image.read_plane(plan.plane(c, z, t)?)?, false)))
                    .collect::<imager_core::Result<Vec<_>>>()?;
                let mut interleaved = Vec::with_capacity(image_bytes as usize);
                for i in 0..planes[0].len() / bps {
                    for plane in &planes {
                        interleaved.extend_from_slice(&plane[i * bps..(i + 1) * bps]);
                    }
                }
                out.write_all(&interleaved)?;
            } else {
                for c in 0..plan.channels.0 {
                    let plane = image.read_plane(plan.plane(c, z, t)?)?;
                    out.write_all(&plane_bytes(&plane, plan.signed16))?;
                }
            }
        }
    }

    if chained {
        for index in 1..images {
            let next = if index == images - 1 {
                0
            } else {
                later_at + index as u64 * later_size
            };
            out.write_all(&directory(index, next))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions_are_exact_where_possible() {
        for size in [0.25, 0.3, 1.5, 0.1, 2.0, 1.0 / 3.0, 0.065, 1234.5, 1e-4] {
            let (pixels, units) = resolution(size).unwrap();
            assert_eq!(f64::from(units) / f64::from(pixels), size, "{size}");
        }
        assert_eq!(resolution(0.25), Some((4, 1)));
        // Not representable: still close, never zero terms.
        for size in [std::f64::consts::PI, 1e-12, 1e12, f64::MIN_POSITIVE] {
            let (pixels, units) = resolution(size).unwrap();
            assert!(pixels > 0 && units > 0, "{size}");
        }
        let (pixels, units) = resolution(std::f64::consts::PI).unwrap();
        assert!((f64::from(units) / f64::from(pixels) - std::f64::consts::PI).abs() < 1e-12);
        // ImageJ's own numbers where they are exact, ours where not.
        assert_eq!(
            resolutions(0.1, 0.25),
            Some([(10_000_000, 1_000_000), (4_000_000, 1_000_000)])
        );
        assert_eq!(
            resolutions(0.3, 0.5),
            Some([(10, 3), (2_000_000, 1_000_000)])
        );
        assert_eq!(
            resolutions(1e-9, 1.0).map(|r| r[0]),
            Some((1_000_000_000, 1))
        );
        assert_eq!(resolutions(0.0, 1.0), None);
        assert_eq!(resolution(0.0), None);
        assert_eq!(resolution(f64::NAN), None);
        assert_eq!(resolution(-1.0), None);
    }
}
