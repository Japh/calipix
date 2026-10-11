//! JPEG, GIF and BMP compared with ImageJ 1.x: files ImageJ wrote
//! (`fixtures/imagej/gen/20_formats.ijm`) must open to the pixels ImageJ
//! has, and hand-built files (`21_crafted.ijm`, `formats_craft.rs`) must
//! open to what ImageJ makes of them.
//!
//! ImageJ 1.54p has no NRRD reader or writer, so NRRD has no such tests.

use std::path::PathBuf;

use calipix_core::{AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut};
use calipix_io::{open, save};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn crafted(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/gen/21_crafted_inputs")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-io-imagej-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn text(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Pixel values of an ImageJ "Text Image": tab-separated rows.
fn text_image(name: &str) -> Vec<u8> {
    text(name)
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect()
}

/// A lookup table saved as one "r,g,b" line per entry.
fn lut_csv(name: &str) -> Vec<[u8; 3]> {
    text(name)
        .lines()
        .map(|line| {
            let mut parts = line.split(',').map(|v| v.parse().unwrap());
            [(); 3].map(|_| parts.next().unwrap())
        })
        .collect()
}

fn plane(image: &Image, index: usize) -> Vec<u8> {
    image
        .read_plane(index)
        .unwrap()
        .try_slice::<u8>()
        .unwrap()
        .to_vec()
}

fn lut_of(image: &Image) -> Option<Lut> {
    image.meta().channels.first().and_then(|c| c.lut.clone())
}

/// The table an 8-bit image is shown through: its own or the grey ramp.
fn table_of(image: &Image) -> [[u8; 3]; 256] {
    *lut_of(image).unwrap_or_else(Lut::grays).table
}

/// ImageJ's bit depth and slice count for what we opened.
fn depth_and_slices(image: &Image) -> (u32, usize) {
    let meta = image.meta();
    assert_eq!(meta.dtype(), DType::U8);
    let depth = if meta.color == ColorModel::Rgb { 24 } else { 8 };
    let channels = if depth == 24 { 3 } else { 1 };
    assert_eq!(meta.size(&AxisKind::Channel), channels);
    assert_eq!(meta.size(&AxisKind::Time), 1);
    (depth, meta.size(&AxisKind::Z))
}

/// Compares an RGB slice of ours with a PNG ImageJ saved of the same slice.
fn assert_rgb_slice(image: &Image, slice: usize, png: &str) {
    let expected = open(fixture(png)).unwrap();
    assert_eq!(expected.meta().color, ColorModel::Rgb, "{png}");
    assert_eq!(
        &image.meta().shape()[..2],
        &expected.meta().shape()[..2],
        "{png}"
    );
    for c in 0..3 {
        assert_eq!(
            plane(image, slice * 3 + c),
            plane(&expected, c),
            "{png}, channel {c}"
        );
    }
}

/// How many samples differ by 0, 1, 2, ... levels, up to the largest
/// difference present.
fn difference_counts(a: &Image, b: &Image) -> Vec<usize> {
    assert_eq!(a.meta().shape(), b.meta().shape());
    assert_eq!(a.meta().color, b.meta().color);
    let mut counts = Vec::new();
    for p in 0..a.meta().plane_count() {
        for (x, y) in plane(a, p).iter().zip(plane(b, p)) {
            let difference = usize::from(x.abs_diff(y));
            if counts.len() <= difference {
                counts.resize(difference + 1, 0);
            }
            counts[difference] += 1;
        }
    }
    counts
}

fn max_difference(a: &Image, b: &Image) -> u8 {
    assert_eq!(a.meta().shape(), b.meta().shape());
    assert_eq!(a.meta().color, b.meta().color);
    let mut max = 0;
    for p in 0..a.meta().plane_count() {
        for (x, y) in plane(a, p).iter().zip(plane(b, p)) {
            max = max.max(x.abs_diff(y));
        }
    }
    max
}

// Files written by ImageJ

#[test]
fn reopened_types_match_imagej() {
    for line in text("fmt_reopened.csv").lines() {
        let fields: Vec<&str> = line.split(',').collect();
        let image = open(fixture(fields[0])).unwrap();
        let (depth, slices) = depth_and_slices(&image);
        assert_eq!(depth.to_string(), fields[1], "{line}");
        assert_eq!(slices.to_string(), fields[2], "{line}");
        assert_eq!(image.meta().name, fields[0]);
    }
}

#[test]
fn jpeg_written_by_imagej_decodes_like_imagej() {
    // ImageJ decodes with Java's JPEG reader and we with zune-jpeg; the
    // two round the inverse DCT and chroma upsampling differently.
    // Measured on these fixtures: grey differs by at most 1 level; RGB
    // (saved by ImageJ with 4:2:0 chroma) by at most 2, except for two
    // samples of 9216 that differ by 3.
    let gray = open(fixture("fmt_gray8.jpg")).unwrap();
    assert_eq!(gray.meta().shape(), &[64, 48]);
    assert_eq!(gray.meta().color, ColorModel::Grayscale);
    let differences = difference_counts(&gray, &open(fixture("fmt_gray8_jpg.png")).unwrap());
    println!("grey JPEG: samples by difference from ImageJ's decoding {differences:?}");
    assert_eq!(differences.len(), 2, "maximum difference 1");

    let rgb = open(fixture("fmt_rgb.jpg")).unwrap();
    assert_eq!(rgb.meta().shape(), &[64, 48, 3]);
    assert_eq!(rgb.meta().color, ColorModel::Rgb);
    let differences = difference_counts(&rgb, &open(fixture("fmt_rgb_jpg.png")).unwrap());
    println!("RGB JPEG: samples by difference from ImageJ's decoding {differences:?}");
    assert_eq!(differences.len(), 4, "maximum difference 3");
    assert_eq!(differences[3], 2, "samples differing by 3");
}

#[test]
fn gif_and_bmp_written_by_imagej_hold_the_original_pixels() {
    let gray = open(fixture("gray8.png")).unwrap();
    for name in ["fmt_gray8.gif", "fmt_gray8.bmp"] {
        let image = open(fixture(name)).unwrap();
        assert_eq!(image.meta().shape(), &[64, 48], "{name}");
        assert_eq!(plane(&image, 0), plane(&gray, 0), "{name}");
        assert!(lut_of(&image).is_none(), "{name}: grey ramp");
    }

    // With a colour table the samples stay indices and the table is kept.
    let fire = lut_csv("fmt_fire_lut.csv");
    assert_eq!(fire.len(), 256);
    for name in ["fmt_fire.gif", "fmt_fire.bmp"] {
        let image = open(fixture(name)).unwrap();
        assert_eq!(image.meta().color, ColorModel::Grayscale, "{name}");
        assert_eq!(plane(&image, 0), plane(&gray, 0), "{name}");
        assert_eq!(table_of(&image)[..], fire[..], "{name}");
    }

    let rgb = open(fixture("rgb.png")).unwrap();
    let image = open(fixture("fmt_rgb.bmp")).unwrap();
    assert_eq!(image.meta().shape(), &[64, 48, 3]);
    assert_eq!(image.meta().color, ColorModel::Rgb);
    for c in 0..3 {
        assert_eq!(plane(&image, c), plane(&rgb, c), "channel {c}");
    }
}

fn anim_value(x: usize, y: usize, slice: usize) -> u8 {
    ((x * 7 + y * 3 + slice * 40) % 256) as u8
}

#[test]
fn animated_gif_written_by_imagej_opens_as_a_stack() {
    let image = open(fixture("fmt_anim.gif")).unwrap();
    assert_eq!(image.meta().shape(), &[32, 24, 5]);
    assert_eq!(image.meta().axes()[2].kind, AxisKind::Z);
    assert_eq!(image.meta().color, ColorModel::Grayscale);
    for slice in 0..5 {
        let expected: Vec<u8> = (0..32 * 24)
            .map(|i| anim_value(i % 32, i / 32, slice + 1))
            .collect();
        assert_eq!(plane(&image, slice), expected, "slice {slice}");
    }

    // With a colour table ImageJ gets RGB frames back.
    let image = open(fixture("fmt_anim_fire.gif")).unwrap();
    assert_eq!(image.meta().shape(), &[32, 24, 3, 5]);
    assert_eq!(image.meta().axes()[3].kind, AxisKind::Z);
    for slice in 0..5 {
        assert_rgb_slice(&image, slice, &format!("fmt_anim_fire_{}.png", slice + 1));
    }
}

// Files written by us, compared with ImageJ's

#[test]
fn bmp_writer_output_is_identical_to_imagej() {
    for (source, expected) in [
        ("gray8.png", "fmt_gray8.bmp"),
        ("rgb.png", "fmt_rgb.bmp"),
        ("fmt_fire.bmp", "fmt_fire.bmp"),
    ] {
        let path = temp(expected);
        save(&open(fixture(source)).unwrap(), &path).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(fixture(expected)).unwrap(),
            "{expected}"
        );
    }
}

#[test]
fn animated_gif_writer_lays_the_file_out_like_imagej() {
    let meta = ImageMeta::xyczt(DType::U8, 32, 24, 1, 5, 1).unwrap();
    let planes = (1..=5)
        .map(|s| {
            Buffer::from_vec(
                (0..32 * 24)
                    .map(|i| anim_value(i % 32, i / 32, s))
                    .collect(),
            )
        })
        .collect();
    let stack = Image::from_planes(meta, planes).unwrap();
    for (name, lut) in [
        ("fmt_anim.gif", None),
        ("fmt_anim_fire.gif", Some("fmt_fire_lut.csv")),
    ] {
        let mut image = stack.clone();
        if let Some(csv) = lut {
            let table = lut_csv(csv);
            let mut meta = image.meta().clone();
            meta.channels = vec![ChannelMeta {
                lut: Some(Lut::from_fn("Fire", |i| table[i as usize])),
                ..ChannelMeta::default()
            }];
            image = image.with_meta(meta).unwrap();
        }
        let path = temp(name);
        save(&image, &path).unwrap();
        let (ours, theirs) = (
            std::fs::read(&path).unwrap(),
            std::fs::read(fixture(name)).unwrap(),
        );
        // Everything up to the first frame's compressed data: signature,
        // screen descriptor, loop extension, graphic control, image
        // descriptor and local colour table.
        let prefix = 13 + 19 + 8 + 10 + 768;
        assert_eq!(ours[..prefix], theirs[..prefix], "{name}");
        // The compressed data may differ, but not what it decodes to.
        let (a, b) = (open(&path).unwrap(), open(fixture(name)).unwrap());
        assert_eq!(a.meta().shape(), b.meta().shape(), "{name}");
        for p in 0..a.meta().plane_count() {
            assert_eq!(plane(&a, p), plane(&b, p), "{name}, plane {p}");
        }
    }
}

#[test]
fn jpeg_writer_output_is_close_to_the_original_and_readable() {
    // Our encoder is not Java's, so files differ; both must stay close to
    // the source at quality 85. ImageJ's own loss is measured for scale.
    for (source, theirs, ours) in [
        ("gray8.png", "fmt_gray8.jpg", "ours_gray8.jpg"),
        ("rgb.png", "fmt_rgb.jpg", "ours_rgb.jpg"),
    ] {
        let original = open(fixture(source)).unwrap();
        let path = temp(ours);
        save(&original, &path).unwrap();
        let our_loss = max_difference(&original, &open(&path).unwrap());
        let their_loss = max_difference(&original, &open(fixture(theirs)).unwrap());
        println!("{source}: maximum JPEG loss ours {our_loss}, ImageJ's {their_loss}");
        assert!(
            our_loss <= their_loss.max(8),
            "{source}: {our_loss} vs {their_loss}"
        );
    }
}

// Hand-built files, compared with what ImageJ reads from them

#[test]
fn crafted_files_open_as_imagej_opens_them() {
    let info = text("crafted_info.csv");
    assert_eq!(info.lines().count(), 12);
    for line in info.lines() {
        let fields: Vec<&str> = line.split(',').collect();
        let name = fields[0];
        let base = format!("crafted_{}", name.rsplit_once('.').unwrap().0);
        let image = open(crafted(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (depth, slices) = depth_and_slices(&image);
        assert_eq!(depth.to_string(), fields[1], "{name}: bit depth");
        assert_eq!(slices.to_string(), fields[2], "{name}: slices");

        for slice in 0..slices {
            if depth == 8 {
                let expected = text_image(&format!("{base}_{}.txt", slice + 1));
                assert_eq!(plane(&image, slice), expected, "{name}, slice {slice}");
            } else {
                assert_rgb_slice(&image, slice, &format!("{base}_{}.png", slice + 1));
            }
        }
        if depth == 8 {
            // ImageJ reports as many entries as the file's palette has;
            // ours is padded to 256 with black.
            let expected = lut_csv(&format!("{base}_lut.csv"));
            let table = table_of(&image);
            assert_eq!(
                table[..expected.len()],
                expected[..],
                "{name}: lookup table"
            );
            assert!(
                table[expected.len()..].iter().all(|c| *c == [0; 3]),
                "{name}"
            );
        }
    }
}
