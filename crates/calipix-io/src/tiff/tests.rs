//! Tests that need the module's internals: counting what is read from
//! storage, and forcing the writer's large-file layouts at small sizes.
//! Behaviour visible through `calipix_io::open` and `save` is tested in
//! `tests/tiff*.rs`.

use std::sync::Arc;

use calipix_core::{Buffer, ColorModel, DType, Image, ImageMeta, Lut, Rect};

use super::ifd::{MemStorage, Storage, TiffFile, tag};
use super::{reader, writer};

fn stack(dtype: DType, width: usize, height: usize, planes: usize) -> Image {
    let meta = ImageMeta::xyczt(dtype, width, height, 1, planes, 1).unwrap();
    let data = (0..planes)
        .map(|p| match dtype {
            DType::U8 => Buffer::from_vec(
                (0..width * height)
                    .map(|i| (i * 7 + p * 31) as u8)
                    .collect::<Vec<_>>(),
            ),
            DType::U16 => Buffer::from_vec(
                (0..width * height)
                    .map(|i| (i * 7 + p * 1000) as u16)
                    .collect::<Vec<_>>(),
            ),
            _ => Buffer::from_vec(
                (0..width * height)
                    .map(|i| i as f64 * 0.5 - p as f64)
                    .collect::<Vec<_>>(),
            ),
        })
        .collect();
    Image::from_planes(meta, data).unwrap()
}

fn encode(image: &Image, options: writer::Options) -> Vec<u8> {
    let mut bytes = Vec::new();
    writer::write(image, &mut bytes, options).unwrap();
    bytes
}

fn open(bytes: Vec<u8>) -> (Image, Arc<MemStorage>) {
    let storage = MemStorage::new(bytes);
    let image = reader::open(storage.clone(), "test.tif").unwrap();
    (image, storage)
}

fn assert_same_planes(a: &Image, b: &Image) {
    assert_eq!(a.meta().shape(), b.meta().shape());
    assert_eq!(a.meta().dtype(), b.meta().dtype());
    for p in 0..a.meta().plane_count() {
        assert_eq!(
            a.read_plane(p).unwrap(),
            b.read_plane(p).unwrap(),
            "plane {p}"
        );
    }
}

/// Byte range of the pixel data and the number of directories in the chain.
fn layout(bytes: &[u8], pixel_bytes: u64) -> (std::ops::Range<u64>, usize) {
    let file = TiffFile::open(MemStorage::new(bytes.to_vec())).unwrap();
    let chain = file.ifd_chain().unwrap();
    let start = file.uint(&chain[0].1, tag::STRIP_OFFSETS).unwrap().unwrap();
    (start..start + pixel_bytes, chain.len())
}

fn reads_in(storage: &MemStorage, range: &std::ops::Range<u64>) -> Vec<(u64, usize)> {
    storage
        .reads
        .lock()
        .unwrap()
        .iter()
        .copied()
        .filter(|&(offset, len)| offset < range.end && offset + len as u64 > range.start)
        .collect()
}

#[test]
fn opening_an_imagej_stack_reads_no_pixel_data() {
    let image = stack(DType::U16, 40, 30, 12);
    let bytes = encode(&image, writer::Options::default());
    let (pixels, directories) = layout(&bytes, 40 * 30 * 2 * 12);
    assert_eq!(directories, 12);
    let (opened, storage) = open(bytes);
    assert_eq!(opened.meta().shape(), &[40, 30, 12]);
    assert_eq!(reads_in(&storage, &pixels), []);
    // Opening is a handful of small reads, however many planes there are,
    // and none of them is of a directory after the first.
    let reads = storage.reads.lock().unwrap().clone();
    assert!(reads.len() <= 8, "{reads:?}");
    assert!(
        reads.iter().all(|&(offset, _)| offset < pixels.start),
        "{reads:?}"
    );
    let total: usize = reads.iter().map(|r| r.1).sum();
    assert!(total < 600, "opening read {total} bytes");

    // Reading a plane reads exactly that plane, in one piece.
    storage.reads.lock().unwrap().clear();
    opened.read_plane(7).unwrap();
    let plane = 40 * 30 * 2;
    assert_eq!(
        *storage.reads.lock().unwrap(),
        [(pixels.start + 7 * plane as u64, plane)]
    );
    // And a region reads only its rows.
    storage.reads.lock().unwrap().clear();
    let region = opened.read_region(3, Rect::new(0, 10, 40, 2)).unwrap();
    assert_eq!(
        *storage.reads.lock().unwrap(),
        [(pixels.start + 3 * plane as u64 + 10 * 80, 160)]
    );
    assert_eq!(
        region.try_slice::<u16>().unwrap(),
        &image.read_plane(3).unwrap().try_slice::<u16>().unwrap()[400..480]
    );
    assert_same_planes(&opened, &image);
}

#[test]
fn opening_a_multi_directory_file_reads_no_pixel_data() {
    // 48-bit RGB is written without `images=`, so each directory is read
    // and its image decoded through the `tiff` crate.
    let mut meta = ImageMeta::xyczt(DType::U16, 20, 10, 3, 4, 1).unwrap();
    meta.color = ColorModel::Rgb;
    let planes = (0..12)
        .map(|p| {
            Buffer::from_vec(
                (0..200)
                    .map(|i| (i * 3 + p * 500) as u16)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let image = Image::from_planes(meta, planes).unwrap();
    let bytes = encode(&image, writer::Options::default());
    let (pixels, directories) = layout(&bytes, 20 * 10 * 6 * 4);
    assert_eq!(directories, 4);
    let (opened, storage) = open(bytes);
    assert_eq!(opened.meta().shape(), &[20, 10, 3, 4]);
    assert_eq!(reads_in(&storage, &pixels), []);

    // One channel of one image touches only that image's pixels.
    storage.reads.lock().unwrap().clear();
    opened.read_plane(3 * 2 + 1).unwrap();
    let image_bytes = 20 * 10 * 6;
    let third = pixels.start + 2 * image_bytes..pixels.start + 3 * image_bytes;
    let touched = reads_in(&storage, &pixels);
    assert!(!touched.is_empty());
    // (The buffered reader may run up to its 8 KiB buffer past the strip.)
    for (offset, len) in touched {
        assert!(offset >= third.start && offset + len as u64 <= third.end + 8192);
    }
    assert_same_planes(&opened, &image);
}

#[test]
fn a_narrow_column_of_a_wide_image_is_read_row_by_row() {
    let image = stack(DType::U8, 5000, 40, 2);
    let bytes = encode(&image, writer::Options::default());
    let (opened, storage) = open(bytes);
    storage.reads.lock().unwrap().clear();
    let region = opened.read_region(1, Rect::new(4000, 5, 3, 30)).unwrap();
    let reads = storage.reads.lock().unwrap().clone();
    assert_eq!(reads.len(), 30);
    assert!(reads.iter().all(|r| r.1 == 3));
    let whole = image.read_plane(1).unwrap();
    let whole = whole.try_slice::<u8>().unwrap();
    let expected: Vec<u8> = (5..35)
        .flat_map(|y| whole[y * 5000 + 4000..y * 5000 + 4003].to_vec())
        .collect();
    assert_eq!(region.try_slice::<u8>().unwrap(), expected);
    // A block that is nearly contiguous is one read.
    storage.reads.lock().unwrap().clear();
    opened.read_region(0, Rect::new(10, 0, 4980, 3)).unwrap();
    assert_eq!(storage.reads.lock().unwrap().len(), 1);
}

#[test]
fn single_directory_stack_as_imagej_writes_beyond_4_gib() {
    // The layout `TiffEncoder` switches to when offsets no longer fit:
    // one directory, `images=N`, planes found by offset alone.
    let image = stack(DType::U16, 16, 12, 9);
    let options = writer::Options {
        single_directory: true,
        ..Default::default()
    };
    let bytes = encode(&image, options);
    let (pixels, directories) = layout(&bytes, 16 * 12 * 2 * 9);
    assert_eq!(directories, 1);
    // The file ends with the last plane: no further directories follow.
    assert_eq!(pixels.end, bytes.len() as u64);
    assert_eq!(&bytes[..4], b"II\x2a\x00");
    let (opened, _) = open(bytes);
    assert_eq!(opened.meta().shape(), &[16, 12, 9]);
    assert_same_planes(&opened, &image);
}

#[test]
fn big_tiff_output_round_trips() {
    for (dtype, planes) in [(DType::F64, 5), (DType::U8, 1), (DType::U16, 3)] {
        let image = stack(dtype, 16, 12, planes);
        let options = writer::Options {
            big_tiff: true,
            ..Default::default()
        };
        let bytes = encode(&image, options);
        assert_eq!(&bytes[..8], b"II\x2b\x00\x08\x00\x00\x00");
        let (_, directories) = layout(&bytes, 0);
        assert_eq!(directories, planes);
        let (opened, _) = open(bytes.clone());
        assert_same_planes(&opened, &image);
        // The `tiff` crate reads it as an ordinary multi-page BigTIFF.
        let mut decoder = ::tiff::decoder::Decoder::new(std::io::Cursor::new(&bytes)).unwrap();
        for p in 0..planes {
            assert_eq!(decoder.dimensions().unwrap(), (16, 12));
            decoder.read_image().unwrap();
            assert_eq!(decoder.more_images(), p + 1 < planes);
            if decoder.more_images() {
                decoder.next_image().unwrap();
            }
        }
    }
}

#[test]
fn standard_readers_see_every_plane_of_our_stacks() {
    // Below 4 GiB every image has its own directory, so software that
    // knows nothing of `images=N` still reads the whole stack.
    let image = stack(DType::U16, 16, 12, 4);
    let bytes = encode(&image, writer::Options::default());
    let mut decoder = ::tiff::decoder::Decoder::new(std::io::Cursor::new(&bytes)).unwrap();
    for p in 0..4 {
        let ::tiff::decoder::DecodingResult::U16(samples) = decoder.read_image().unwrap() else {
            panic!("expected 16-bit samples");
        };
        assert_eq!(
            samples,
            image.read_plane(p).unwrap().try_slice::<u16>().unwrap()
        );
        if p < 3 {
            decoder.next_image().unwrap();
        }
    }
    assert!(!decoder.more_images());
}

#[test]
fn truncated_storage_is_an_error_at_open() {
    let image = stack(DType::U16, 16, 12, 3);
    let bytes = encode(&image, writer::Options::default());
    let (pixels, _) = layout(&bytes, 16 * 12 * 2 * 3);
    // Cut inside the last plane: ImageJ refuses such a file too
    // (`FileOpener.validateFileInfo`).
    let cut = MemStorage::new(bytes[..pixels.end as usize - 10].to_vec());
    let error = reader::open(cut.clone(), "cut.tif")
        .unwrap_err()
        .to_string();
    assert!(error.contains("truncated"), "{error}");
    assert_eq!(cut.len(), pixels.end - 10);
}

/// Opens possibly damaged bytes and reads a few planes and a region.
/// Any outcome but a panic is acceptable.
fn exercise(bytes: Vec<u8>) {
    if let Ok(image) = reader::open(MemStorage::new(bytes), "fuzz.tif") {
        let meta = image.meta();
        // Corruption can produce an image that is valid but enormous.
        if meta.plane_len() > 1 << 22 {
            return;
        }
        let planes = meta.plane_count();
        for p in [0, planes / 2, planes - 1] {
            let _ = image.read_plane(p);
        }
        let _ = image.read_region(0, Rect::new(0, 0, 1, 1));
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name);
    std::fs::read(path).unwrap()
}

#[test]
fn every_truncation_of_an_imagej_file_is_an_error() {
    let opens = |bytes: &[u8]| reader::open(MemStorage::new(bytes.to_vec()), "cut.tif").is_ok();
    // Single images end with their pixel data, so any truncation loses
    // something that is needed.
    for name in [
        "gray8.tif",
        "gray8_fire.tif",
        "gray16_le.tif",
        "gray16_signed.tif",
        "gray32f.tif",
        "rgb.tif",
        "gray8_calibrated.tif",
        "gray8_roi_overlay.tif",
    ] {
        let bytes = fixture(name);
        assert!(opens(&bytes), "{name}");
        for cut in 0..bytes.len() {
            assert!(!opens(&bytes[..cut]), "{name} cut to {cut} opened");
        }
    }
    // Stacks below 4 GiB end with directories that ImageJ, and we, never
    // read once `images=N` is known; only losing pixels matters.
    for (name, pixels_end) in [
        ("hyperstack_c3z4t5.tif", 3178 + 60 * 1536),
        ("stack_z6_8bit.tif", 189 + 6 * 768),
        ("stack_z3_16bit_le.tif", 288 + 3 * 1536),
    ] {
        let bytes = fixture(name);
        let step = if bytes.len() > 20_000 { 13 } else { 1 };
        let lengths = (0..4000.min(bytes.len()))
            .chain((4000..bytes.len()).step_by(step))
            .chain(pixels_end - 3..pixels_end + 3);
        for cut in lengths {
            assert_eq!(
                opens(&bytes[..cut]),
                cut >= pixels_end,
                "{name} cut to {cut}"
            );
        }
    }
}

#[test]
fn corrupted_files_never_panic() {
    // Seeds: real ImageJ files (offset layout, palette, private metadata)
    // and files that go through the `tiff` crate.
    let mut rgb48 = ImageMeta::xyczt(DType::U16, 9, 7, 3, 3, 1).unwrap();
    rgb48.color = ColorModel::Rgb;
    let rgb48 = Image::from_planes(rgb48, vec![Buffer::zeros(DType::U16, 63); 9]).unwrap();
    let big = writer::Options {
        big_tiff: true,
        ..Default::default()
    };
    let seeds = [
        fixture("hyperstack_c3z4t5.tif"),
        fixture("gray8_fire.tif"),
        fixture("gray8_roi_overlay.tif"),
        fixture("rgb_le.tif"),
        encode(&rgb48, writer::Options::default()),
        encode(&stack(DType::F64, 8, 6, 3), big),
    ];
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for seed in &seeds {
        exercise(seed.clone());
        // The directories and tag values sit at the start and at the end;
        // most damage is aimed there, the rest anywhere.
        let len = seed.len();
        for round in 0..1200 {
            let mut bytes = seed.clone();
            for _ in 0..1 + next() % 3 {
                let at = match round % 4 {
                    0 | 1 => next() as usize % len.min(400),
                    2 => len - 1 - next() as usize % len.min(400),
                    _ => next() as usize % len,
                };
                bytes[at] = match next() % 4 {
                    0 => 0,
                    1 => 0xff,
                    2 => bytes[at] ^ (1 << (next() % 8)),
                    _ => next() as u8,
                };
            }
            exercise(bytes);
        }
        // And every truncation of the first kilobyte.
        for cut in 0..len.min(1024) {
            exercise(seed[..cut].to_vec());
        }
    }
}

/// What ImageJ should report for `image` once it is saved as TIFF, in the
/// format printed by `tests/tiff_support/imagej_open_check.ijm`. Derived
/// from the image itself, not from our reader.
fn imagej_report(image: &Image) -> String {
    use calipix_core::AxisKind;
    use std::fmt::Write as _;

    let meta = image.meta();
    let (w, h) = (meta.width(), meta.height());
    let rgb = meta.color == ColorModel::Rgb;
    let rgb48 = rgb && meta.dtype() == DType::U16;
    let size = |kind: AxisKind| meta.size(&kind);
    let (samples, z, t) = (
        size(AxisKind::Channel),
        size(AxisKind::Z),
        size(AxisKind::Time),
    );
    // ImageJ has no 16-bit RGB type: such files open as three channels.
    let c = if rgb && !rgb48 { 1 } else { samples };
    let bits = match meta.dtype() {
        DType::U8 if rgb => 24,
        DType::U8 => 8,
        DType::U16 | DType::I16 => 16,
        _ => 32,
    };
    let axis = |kind: AxisKind| meta.axis_index(&kind).map(|i| &meta.axes()[i]);
    let unit = match meta.axes()[0].unit.as_deref() {
        None => "pixels",
        Some("inch") => "inches",
        Some(unit) => unit,
    };
    let codes: String = unit.encode_utf16().map(|u| format!("{u} ")).collect();
    let interval = axis(AxisKind::Time)
        .filter(|a| a.unit.is_some())
        .map_or(0.0, |a| a.scale);
    let mut out = String::new();
    writeln!(
        out,
        "dims {w}x{h} c={c} z={z} t={t} bits={bits} n={}",
        c * z * t
    )
    .unwrap();
    writeln!(
        out,
        "voxel {:.6} {:.6} {:.6} unit={codes}interval={interval:.4}",
        meta.axes()[0].scale,
        meta.axes()[1].scale,
        axis(AxisKind::Z).map_or(1.0, |a| a.scale),
    )
    .unwrap();
    let dimensions = [c, z, t].iter().filter(|&&n| n > 1).count();
    let hyperstack = dimensions > 1 && !rgb48;
    let composite = c > 1;
    writeln!(
        out,
        "flags hyperstack={} composite={}",
        u8::from(hyperstack),
        u8::from(composite)
    )
    .unwrap();

    let decimals = if bits == 32 { 2 } else { 0 };
    let signed = meta.dtype() == DType::I16;
    let raw =
        |plane: &Buffer, i: usize| plane.get_f64(i).unwrap() + if signed { 32768.0 } else { 0.0 };
    let points = [0, w * h - 1, 3 * w + 5];
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for n in 0..c * z * t {
        let (ci, zi, ti) = (n % c, n / c % z, n / (c * z));
        let index = |channel: usize| {
            let mut position = Vec::new();
            for a in &meta.axes()[2..] {
                position.push(match a.kind {
                    AxisKind::Channel => channel,
                    AxisKind::Z => zi,
                    _ => ti,
                });
            }
            meta.plane_index(&position).unwrap()
        };
        if rgb && !rgb48 {
            let planes: Vec<Buffer> = (0..3)
                .map(|s| image.read_plane(index(s)).unwrap())
                .collect();
            let pixel = |i: usize| {
                let v: Vec<String> = planes
                    .iter()
                    .map(|p| format!("{}", p.get_f64(i).unwrap()))
                    .collect();
                v.join(",")
            };
            writeln!(
                out,
                "plane {n}: {} {} {}",
                pixel(points[0]),
                pixel(points[1]),
                pixel(points[2])
            )
            .unwrap();
        } else {
            let plane = image.read_plane(index(ci)).unwrap();
            let values: Vec<f64> = (0..w * h).map(|i| raw(&plane, i)).collect();
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            (low, high) = (low.min(min), high.max(max));
            writeln!(
                out,
                "plane {n}: {:.d$} {:.d$} {:.d$} min={min:.d$} max={max:.d$} mean={mean:.3} value={:.d$}",
                values[points[0]],
                values[points[1]],
                values[points[2]],
                plane.get_f64(points[2]).unwrap(),
                d = decimals,
            )
            .unwrap();
        }
        // Labels are stored per ImageJ stack slice; the channels of 48-bit
        // RGB are labelled by `FileOpener.open`.
        if rgb48 {
            writeln!(out, "label {n}={}", ["Red", "Green", "Blue"][n]).unwrap();
        } else if c * z * t > 1
            && let Some(label) = meta.properties.get(&format!("imagej.label.{n}"))
        {
            writeln!(out, "label {n}={label}").unwrap();
        }
    }
    let entries = |lut: &Lut| {
        let e = |i: usize| {
            format!(
                "{},{},{}",
                lut.table[i][0], lut.table[i][1], lut.table[i][2]
            )
        };
        format!("{} {} {}", e(0), e(128), e(255))
    };
    if composite {
        for ci in 0..c {
            let channel = meta.channels.get(ci);
            let lut = channel
                .and_then(|ch| ch.lut.clone())
                .unwrap_or_else(|| super::imagej::default_channel_lut(ci));
            // 48-bit RGB has no stored ranges; ImageJ uses the data range.
            let (lo, hi) = channel
                .and_then(|ch| ch.display_range)
                .unwrap_or((low, high));
            writeln!(
                out,
                "channel {}: lut {} range={lo:.4}..{hi:.4}",
                ci + 1,
                entries(&lut)
            )
            .unwrap();
        }
        let mode = match meta.color {
            ColorModel::Grayscale if meta.channels.iter().any(|ch| ch.lut.is_some()) => "color",
            ColorModel::Grayscale => "grayscale",
            _ => "composite",
        };
        writeln!(out, "mode={mode}").unwrap();
    } else if !rgb {
        let channel = meta.channels.first();
        let lut = channel
            .and_then(|ch| ch.lut.clone())
            .unwrap_or_else(Lut::grays);
        let (lo, hi) = channel
            .and_then(|ch| ch.display_range)
            .expect("check images set a range");
        // `ImageProcessor.isInvertedLut`: brightness never increases.
        let sum = |i: usize| lut.table[i].iter().map(|&v| u32::from(v)).sum::<u32>();
        let inverting = (1..255).all(|i| sum(i - 1) >= sum(i));
        writeln!(
            out,
            "lut {} range={lo:.4}..{hi:.4} inverting={}",
            entries(&lut),
            u8::from(inverting)
        )
        .unwrap();
    }
    let property = |key: &str| meta.properties.get(key).cloned().unwrap_or_default();
    writeln!(out, "info={}", property("Info").replace('\n', "|")).unwrap();
    writeln!(out, "property={}", property("Objective")).unwrap();
    let overlay = meta
        .properties
        .keys()
        .filter(|k| k.starts_with("imagej.overlay."))
        .count();
    // The selection in the fixture this is copied from is an oval (type 1).
    let selection = if meta.properties.contains_key("imagej.roi") {
        1
    } else {
        -1
    };
    writeln!(out, "selection={selection} overlay={overlay}").unwrap();
    out.push_str("CHECK_DONE\n");
    out
}

/// Writes a set of files covering what the writer can produce, each with
/// the report ImageJ should print for it, into `$CALIPIX_TIFF_CHECK_DIR`.
/// See `tests/tiff_support/imagej_open_check.ijm` for how to run the check.
#[test]
#[ignore = "writes files for a manual check with ImageJ"]
fn write_files_for_imagej_check() {
    use calipix_core::{Axis, AxisKind, ChannelMeta};

    let dir = std::path::PathBuf::from(
        std::env::var("CALIPIX_TIFF_CHECK_DIR").expect("set CALIPIX_TIFF_CHECK_DIR"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (32usize, 24usize);
    // Value of sample `i` of channel c, slice z, frame t (0-based).
    let rule = |c: usize, z: usize, t: usize, i: usize| {
        ((c + 1) * 10000 + (z + 1) * 1000 + (t + 1) * 100 + i % w + 32 * (i / w)) as f64
    };
    type Rule<'a> = &'a dyn Fn(usize, usize, usize, usize) -> f64;
    let build = |dtype: DType, (c, z, t): (usize, usize, usize), value: Rule| {
        let meta = ImageMeta::xyczt(dtype, w, h, c, z, t).unwrap();
        let planes: Vec<Buffer> = (0..c * z * t)
            .map(|p| {
                let v = |i: usize| value(p % c, p / c % z, p / (c * z), i);
                let n = 0..w * h;
                match dtype {
                    DType::U8 => Buffer::from_vec(n.map(|i| v(i) as u8).collect::<Vec<_>>()),
                    DType::U16 => Buffer::from_vec(n.map(|i| v(i) as u16).collect::<Vec<_>>()),
                    DType::I16 => Buffer::from_vec(n.map(|i| v(i) as i16).collect::<Vec<_>>()),
                    DType::U32 => Buffer::from_vec(n.map(|i| v(i) as u32).collect::<Vec<_>>()),
                    DType::I32 => Buffer::from_vec(n.map(|i| v(i) as i32).collect::<Vec<_>>()),
                    _ => Buffer::from_vec(n.map(|i| v(i) as f32).collect::<Vec<_>>()),
                }
            })
            .collect();
        (meta, planes)
    };
    let ranged = |lut: Option<Lut>, range: (f64, f64)| ChannelMeta {
        name: None,
        lut,
        display_range: Some(range),
    };
    let calibrate = |meta: &mut ImageMeta, kind: AxisKind, scale: f64, unit: &str| {
        let index = meta.axis_index(&kind).unwrap();
        *meta.axis_mut(index).unwrap() = Axis::calibrated(kind, scale, unit);
    };
    let mut files: Vec<(&str, Image, writer::Options)> = Vec::new();
    let mut add =
        |name: &'static str, meta: ImageMeta, planes: Vec<Buffer>, options: writer::Options| {
            files.push((name, Image::from_planes(meta, planes).unwrap(), options));
        };
    let default = writer::Options::default();

    let byte = |c: usize, z: usize, t: usize, i: usize| {
        ((rule(c, z, t, i) as usize * 7 + z * 40) % 256) as f64
    };
    let (mut meta, planes) = build(DType::U8, (1, 1, 1), &byte);
    meta.channels = vec![ranged(Some(Lut::grays().inverted()), (0.0, 255.0))];
    calibrate(&mut meta, AxisKind::X, 0.5, "mm");
    calibrate(&mut meta, AxisKind::Y, 0.25, "mm");
    meta.properties
        .insert("Info".into(), "written by Calipix\nsecond line".into());
    meta.properties
        .insert("Objective".into(), "63x/1.4 Oil".into());
    add("u8_inverting_lut_mm.tif", meta, planes, default);

    let (mut meta, planes) = build(DType::U8, (1, 6, 1), &byte);
    let custom = Lut::from_fn("Custom", |i| [i / 2, 255 - i, i.wrapping_mul(3)]);
    meta.channels = vec![ranged(Some(custom), (10.0, 200.0))];
    meta.properties
        .insert("imagej.label.1".into(), "second slice".into());
    add("u8_stack_custom_lut.tif", meta, planes, default);

    let (mut meta, planes) = build(DType::U16, (3, 4, 5), &rule);
    meta.color = ColorModel::Composite;
    meta.channels = vec![
        ranged(
            Some(Lut::from_fn("Cyan", |i| [0, i, i])),
            (10000.0, 15000.0),
        ),
        ranged(
            Some(Lut::from_fn("Magenta", |i| [i, 0, i])),
            (20000.0, 26000.0),
        ),
        ranged(
            Some(Lut::from_fn("Yellow", |i| [i, i, 0])),
            (30000.0, 37000.0),
        ),
    ];
    calibrate(&mut meta, AxisKind::X, 0.25, "\u{b5}m");
    calibrate(&mut meta, AxisKind::Y, 0.25, "\u{b5}m");
    calibrate(&mut meta, AxisKind::Z, 1.5, "\u{b5}m");
    calibrate(&mut meta, AxisKind::Time, 2.5, "sec");
    meta.properties
        .insert("Info".into(), "fixture=hyperstack\nkey=value".into());
    meta.properties
        .insert("imagej.label.43".into(), "c2-z3-t4 label".into());
    add("u16_hyperstack_composite.tif", meta, planes, default);

    let (mut meta, planes) = build(DType::U16, (2, 1, 3), &rule);
    meta.channels = vec![
        ranged(None, (10000.0, 11000.0)),
        ranged(None, (20000.0, 21000.0)),
    ];
    add("u16_channels_grayscale_mode.tif", meta, planes, default);

    let (mut meta, planes) = build(DType::U16, (2, 2, 1), &rule);
    meta.channels = vec![
        ranged(
            Some(Lut::from_fn("Magenta", |i| [i, 0, i])),
            (10000.0, 12000.0),
        ),
        ranged(
            Some(Lut::from_fn("Green", |i| [0, i, 0])),
            (20000.0, 22000.0),
        ),
    ];
    add("u16_channels_color_mode.tif", meta, planes, default);

    let float = |c: usize, z: usize, t: usize, i: usize| rule(c, z, t, i) * 0.25 - 2800.0;
    let (mut meta, planes) = build(DType::F32, (1, 3, 1), &float);
    meta.channels = vec![ranged(None, (-10.5, 320.25))];
    calibrate(&mut meta, AxisKind::Z, 0.5, "\u{b5}m");
    add("f32_stack.tif", meta, planes, default);

    let signed = |c: usize, z: usize, t: usize, i: usize| rule(c, z, t, i) - 11500.0;
    let (mut meta, planes) = build(DType::I16, (1, 2, 1), &signed);
    meta.channels = vec![ranged(None, (-100.0, 3000.0))];
    add("i16_stack.tif", meta, planes, default);

    let color = |c: usize, z: usize, t: usize, i: usize| {
        ((i % w) * (c + 3) + (i / w) * 5 + z * 20 + t * 50) as f64 % 256.0
    };
    let (mut meta, planes) = build(DType::U8, (3, 1, 1), &color);
    meta.color = ColorModel::Rgb;
    add("rgb8.tif", meta, planes, default);
    let (mut meta, planes) = build(DType::U8, (3, 3, 2), &color);
    meta.color = ColorModel::Rgb;
    add("rgb8_slices_and_frames.tif", meta, planes, default);
    let (mut meta, planes) = build(DType::U16, (3, 1, 1), &rule);
    meta.color = ColorModel::Rgb;
    add("rgb48.tif", meta, planes, default);

    let (mut meta, planes) = build(DType::U16, (1, 5, 1), &rule);
    meta.channels = vec![ranged(None, (11000.0, 16000.0))];
    let single = writer::Options {
        single_directory: true,
        ..Default::default()
    };
    add("u16_single_directory_stack.tif", meta, planes, single);

    // The selection and overlay of a file ImageJ wrote.
    let source = reader::open(MemStorage::new(fixture("gray8_roi_overlay.tif")), "x").unwrap();
    let (mut meta, planes) = build(DType::U8, (1, 1, 1), &byte);
    meta.channels = vec![ranged(None, (0.0, 255.0))];
    for (key, value) in &source.meta().properties {
        meta.properties.insert(key.clone(), value.clone());
    }
    add("u8_selection_and_overlay.tif", meta, planes, default);

    // Types ImageJ does not have; it converts 32-bit integers to float.
    let (mut meta, planes) = build(DType::U32, (1, 2, 1), &rule);
    meta.channels = vec![ranged(None, (0.0, 20000.0))];
    add("u32_stack.tif", meta, planes, default);
    let (mut meta, planes) = build(DType::I32, (1, 1, 1), &signed);
    meta.channels = vec![ranged(None, (-500.0, 500.0))];
    add("i32.tif", meta, planes, default);

    for (name, image, options) in &files {
        std::fs::write(dir.join(name), encode(image, *options)).unwrap();
        std::fs::write(
            dir.join(format!("{name}.expected.txt")),
            imagej_report(image),
        )
        .unwrap();
        // Whatever ImageJ makes of the file, we must read it back.
        let (opened, _) = open(std::fs::read(dir.join(name)).unwrap());
        assert_same_planes(&opened, image);
    }
    println!("wrote {} files to {}", files.len(), dir.display());
}
