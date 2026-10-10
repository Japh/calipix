//! Tests that need the module's internals: counting what is read from
//! storage, and forcing the writer's large-file layouts at small sizes.
//! Behaviour visible through `imager_io::open` and `save` is tested in
//! `tests/tiff*.rs`.

use std::sync::Arc;

use imager_core::{Buffer, ColorModel, DType, Image, ImageMeta, Rect};

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
        for round in 0..2500 {
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
