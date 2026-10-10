//! Every TIFF in `fixtures/imagej/out` was written by ImageJ 1.54p from the
//! macros in `fixtures/imagej/gen`. These tests open each one and compare
//! pixels with the formula in its macro (or the PNG / text image ImageJ
//! saved from the same pixels) and metadata with what the macro set.

mod tiff_support;

use imager_core::{AxisKind, ColorModel, DType, Image, Lut, Rect};
use tiff_support::{fixture, pattern, plane};

fn open(name: &str) -> Image {
    let image = imager_io::open(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(image.meta().name, name);
    assert!(!image.is_in_memory(), "{name} must be read lazily");
    image
}

fn gray8_px(x: usize, y: usize) -> u8 {
    ((x * x + 3 * y * y + x * y) % 256) as u8
}

fn gray16(x: usize, y: usize) -> u16 {
    ((x * 257 + y * y * 31 + 1000 * ((x / 8 + y / 8) % 2)) % 65536) as u16
}

fn gray32f(x: usize, y: usize) -> f32 {
    ((x as f64 - 31.5) * 0.25 + (y * y) as f64 / 64.0 - 3.5 * ((x + y) % 5) as f64) as f32
}

fn rgb(channel: usize) -> impl Fn(usize, usize) -> u8 {
    move |x, y| match channel {
        0 => ((x * 4) & 255) as u8,
        1 => ((y * 5) & 255) as u8,
        _ => (((x + y) * 3) & 255) as u8,
    }
}

/// A single 64x48 plane with no calibration, lookup table or extra axes.
fn assert_plain_plane(image: &Image, dtype: DType) {
    let meta = image.meta();
    assert_eq!(meta.dtype(), dtype);
    assert_eq!(meta.shape(), &[64, 48]);
    assert_eq!(meta.color, ColorModel::Grayscale);
    for axis in meta.axes() {
        assert_eq!((axis.scale, axis.origin, &axis.unit), (1.0, 0.0, &None));
    }
}

/// The planes of `tif` equal those of the PNG ImageJ saved from the same
/// image.
fn assert_same_as_png(tif: &Image, png: &str) {
    let png = imager_io::open(fixture(png)).unwrap();
    assert_eq!(tif.meta().dtype(), png.meta().dtype());
    assert_eq!(tif.meta().shape(), png.meta().shape());
    assert_eq!(tif.meta().color, png.meta().color);
    for p in 0..png.meta().plane_count() {
        assert_eq!(tif.read_plane(p).unwrap(), png.read_plane(p).unwrap());
    }
}

/// Values of an ImageJ "Text Image": rows of tab-separated numbers.
fn text_image(name: &str) -> Vec<f64> {
    std::fs::read_to_string(fixture(name))
        .unwrap()
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect()
}

#[test]
fn gray8() {
    let image = open("gray8.tif");
    assert_plain_plane(&image, DType::U8);
    assert!(image.meta().channels.is_empty());
    assert!(image.meta().properties.is_empty());
    let pixels = plane::<u8>(&image, 0);
    assert_eq!(pixels, pattern(64, 48, gray8_px));
    assert_same_as_png(&image, "gray8.png");
    let text = text_image("gray8_text.txt");
    assert_eq!(text.len(), pixels.len());
    assert!(text.iter().zip(&pixels).all(|(t, p)| *t == f64::from(*p)));
}

#[test]
fn gray16_in_both_byte_orders() {
    for name in ["gray16.tif", "gray16_le.tif"] {
        let image = open(name);
        assert_plain_plane(&image, DType::U16);
        assert_eq!(plane::<u16>(&image, 0), pattern(64, 48, gray16));
        assert_same_as_png(&image, "gray16.png");
        // `min=0.0 max=65534.0` in the description.
        assert_eq!(image.meta().channels.len(), 1);
        assert_eq!(image.meta().channels[0].display_range, Some((0.0, 65534.0)));
        assert_eq!(image.meta().channels[0].lut, None);
    }
    // The two files really differ in byte order.
    let be = std::fs::read(fixture("gray16.tif")).unwrap();
    let le = std::fs::read(fixture("gray16_le.tif")).unwrap();
    assert_eq!((&be[..2], &le[..2]), (&b"MM"[..], &b"II"[..]));
}

#[test]
fn gray32f_in_both_byte_orders() {
    let text = text_image("gray32f_text.txt");
    for name in ["gray32f.tif", "gray32f_le.tif"] {
        let image = open(name);
        assert_plain_plane(&image, DType::F32);
        let pixels = plane::<f32>(&image, 0);
        assert_eq!(pixels, pattern(64, 48, gray32f));
        assert_eq!(
            image.meta().channels[0].display_range,
            Some((-21.625, 42.390625))
        );
        // The text image is rounded to three decimals.
        assert_eq!(text.len(), pixels.len());
        for (t, p) in text.iter().zip(&pixels) {
            assert!((t - f64::from(*p)).abs() <= 5.1e-4, "{t} vs {p}");
        }
    }
}

#[test]
fn rgb_in_both_byte_orders() {
    for name in ["rgb.tif", "rgb_le.tif"] {
        let image = open(name);
        let meta = image.meta();
        assert_eq!(meta.dtype(), DType::U8);
        assert_eq!(meta.shape(), &[64, 48, 3]);
        assert_eq!(meta.axes()[2].kind, AxisKind::Channel);
        assert_eq!(meta.color, ColorModel::Rgb);
        assert!(meta.channels.is_empty());
        for c in 0..3 {
            assert_eq!(plane::<u8>(&image, c), pattern(64, 48, rgb(c)));
        }
        assert_same_as_png(&image, "rgb.png");
        // A region of one channel comes out of the interleaved samples.
        let region = image.read_region(1, Rect::new(5, 7, 3, 2)).unwrap();
        let expected: Vec<u8> = [(5, 7), (6, 7), (7, 7), (5, 8), (6, 8), (7, 8)]
            .iter()
            .map(|&(x, y)| rgb(1)(x, y))
            .collect();
        assert_eq!(region.try_slice::<u8>().unwrap(), expected);
    }
}

#[test]
fn inverted_images_match_their_pngs() {
    assert_same_as_png(&open("gray8_invert.tif"), "gray8_invert.png");
    assert_same_as_png(&open("gray16_invert.tif"), "gray16_invert.png");
    assert_same_as_png(&open("rgb_invert.tif"), "rgb_invert.png");
    let inverted: Vec<u8> = pattern(64, 48, |x, y| 255 - gray8_px(x, y));
    assert_eq!(plane::<u8>(&open("gray8_invert.tif"), 0), inverted);
}

#[test]
fn derived_images_open_with_the_right_type() {
    for (name, dtype, shape) in [
        ("gray8_gauss2.tif", DType::U8, &[64usize, 48][..]),
        ("gray16_gauss2.tif", DType::U16, &[64, 48]),
        ("gray32f_gauss2.tif", DType::F32, &[64, 48]),
        ("gray32f_invert.tif", DType::F32, &[64, 48]),
        ("rgb_gauss2.tif", DType::U8, &[64, 48, 3]),
        ("gray8_otsu_mask.tif", DType::U8, &[64, 48]),
        ("blobs.tif", DType::U8, &[64, 48]),
        ("blobs_count_masks.tif", DType::U16, &[64, 48]),
    ] {
        let image = open(name);
        assert_eq!(image.meta().dtype(), dtype, "{name}");
        assert_eq!(image.meta().shape(), shape, "{name}");
        for p in 0..image.meta().plane_count() {
            image.read_plane(p).unwrap();
        }
    }
    // Masks and blobs are binary images.
    for name in ["gray8_otsu_mask.tif", "blobs.tif"] {
        assert!(
            plane::<u8>(&open(name), 0)
                .iter()
                .all(|&v| v == 0 || v == 255)
        );
    }
    // A float inverted about its display range: min + max - v.
    let inverted = plane::<f32>(&open("gray32f_invert.tif"), 0);
    let source = pattern(64, 48, gray32f);
    for (i, s) in inverted.iter().zip(&source) {
        assert!((i - (-21.625 + 42.390625 - s)).abs() < 1e-4);
    }
}

#[test]
fn inverting_lut_keeps_samples_and_inverts_the_table() {
    // ImageJ stores an inverting LUT as a palette, not as WhiteIsZero.
    let image = open("gray8_invlut.tif");
    assert_eq!(image.meta().dtype(), DType::U8);
    assert_eq!(image.meta().shape(), &[64, 48]);
    assert_eq!(image.meta().color, ColorModel::Grayscale);
    assert_eq!(plane::<u8>(&image, 0), pattern(64, 48, gray8_px));
    let channels = &image.meta().channels;
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].lut, Some(Lut::grays().inverted()));
    assert_eq!(channels[0].display_range, None);
}

#[test]
fn fire_lut_is_a_palette() {
    let image = open("gray8_fire.tif");
    assert_eq!(image.meta().dtype(), DType::U8);
    assert_eq!(plane::<u8>(&image, 0), pattern(64, 48, gray8_px));
    let lut = image.meta().channels[0].lut.as_ref().unwrap();
    assert_eq!(lut.name, "fire");
    // The table of `ij.plugin.LutLoader.fire`, interpolated from 32 to 256
    // entries as `LutLoader.interpolate` does.
    const R: [u8; 32] = [
        0, 0, 1, 25, 49, 73, 98, 122, 146, 162, 173, 184, 195, 207, 217, 229, 240, 252, 255, 255,
        255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    ];
    const G: [u8; 32] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 14, 35, 57, 79, 101, 117, 133, 147, 161, 175, 190,
        205, 219, 234, 248, 255, 255, 255, 255,
    ];
    const B: [u8; 32] = [
        0, 61, 96, 130, 165, 192, 220, 227, 210, 181, 151, 122, 93, 64, 35, 5, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 35, 98, 160, 223, 255,
    ];
    for (i, entry) in lut.table.iter().enumerate() {
        let position = i as f64 * 32.0 / 256.0;
        let (i1, fraction) = (position as usize, position.fract());
        let i2 = (i1 + 1).min(31);
        let mix = |t: &[u8; 32]| {
            ((1.0 - fraction) * f64::from(t[i1]) + fraction * f64::from(t[i2])) as u8
        };
        assert_eq!(*entry, [mix(&R), mix(&G), mix(&B)], "entry {i}");
    }
    assert_eq!(
        image
            .meta()
            .properties
            .get("CurrentLUT")
            .map(String::as_str),
        Some("fire")
    );
}

#[test]
fn signed_16_bit_has_true_values() {
    let image = open("gray16_signed.tif");
    assert_plain_plane(&image, DType::I16);
    // The macro reinterpreted the gray16 pattern as signed.
    let expected: Vec<i16> = pattern(64, 48, |x, y| gray16(x, y) as i16);
    assert_eq!(plane::<i16>(&image, 0), expected);
    assert!(expected.iter().any(|&v| v < 0));
    // The raw big-endian dump ImageJ imported the image from.
    let raw = std::fs::read(fixture("gray16_signed_src.raw")).unwrap();
    let raw: Vec<i16> = raw
        .chunks(2)
        .map(|c| i16::from_be_bytes([c[0], c[1]]))
        .collect();
    assert_eq!(raw, expected);
    // `min=4.0 max=65530.0` are in stored (offset) values.
    assert_eq!(
        image.meta().channels[0].display_range,
        Some((4.0 - 32768.0, 65530.0 - 32768.0))
    );
    // The calibration function is the signedness, not a property.
    assert!(
        image.meta().properties.is_empty(),
        "{:?}",
        image.meta().properties
    );
}

#[test]
fn spatial_and_density_calibration() {
    let image = open("gray8_calibrated.tif");
    let meta = image.meta();
    assert_eq!(meta.dtype(), DType::U8);
    assert_eq!(meta.shape(), &[64, 48]);
    assert_eq!(plane::<u8>(&image, 0), pattern(64, 48, gray8_px));
    let (x, y) = (&meta.axes()[0], &meta.axes()[1]);
    assert_eq!((x.scale, x.unit.as_deref()), (0.5, Some("mm")));
    assert_eq!((y.scale, y.unit.as_deref()), (0.25, Some("mm")));
    // The origin is pixel (3, 4): pixel 0 lies at -3 * 0.5 and -4 * 0.25.
    assert_eq!((x.origin, y.origin), (-1.5, -1.0));
    let property = |key: &str| meta.properties.get(key).map(String::as_str);
    assert_eq!(property("imagej.cf"), Some("0"));
    assert_eq!(property("imagej.c0"), Some("0.4999999999999998"));
    assert_eq!(property("imagej.c1"), Some("0.01"));
    assert_eq!(property("imagej.vunit"), Some("OD"));
    assert_eq!(meta.properties.len(), 4);
}

#[test]
fn selection_and_overlay_are_kept() {
    let image = open("gray8_roi_overlay.tif");
    assert_plain_plane(&image, DType::U8);
    assert_eq!(plane::<u8>(&image, 0), pattern(64, 48, gray8_px));
    let properties = &image.meta().properties;
    // ".roi" data starts with "Iout".
    for key in ["imagej.roi", "imagej.overlay.0", "imagej.overlay.1"] {
        let hex = properties.get(key).unwrap_or_else(|| panic!("{key}"));
        assert!(hex.starts_with("496f7574"), "{key}: {hex}");
        assert_eq!(hex.len(), 2 * 128);
    }
    assert_eq!(properties.len(), 3);
}

#[test]
fn plain_stack_is_z() {
    let image = open("stack_z6_8bit.tif");
    let meta = image.meta();
    assert_eq!(meta.dtype(), DType::U8);
    assert_eq!(meta.shape(), &[32, 24, 6]);
    assert_eq!(meta.axes()[2].kind, AxisKind::Z);
    assert_eq!(meta.axes()[2].scale, 1.0);
    assert_eq!(meta.color, ColorModel::Grayscale);
    assert!(meta.channels.is_empty());
    assert!(meta.properties.is_empty());
    for s in 0..6 {
        let expected: Vec<u8> = pattern(32, 24, |x, y| (((s + 1) * 40 + x + y) % 256) as u8);
        assert_eq!(plane::<u8>(&image, s), expected, "slice {s}");
    }
}

#[test]
fn little_endian_calibrated_stack() {
    let image = open("stack_z3_16bit_le.tif");
    let meta = image.meta();
    assert_eq!(meta.dtype(), DType::U16);
    assert_eq!(meta.shape(), &[32, 24, 3]);
    let micron = Some("\u{b5}m");
    for (axis, kind, scale) in [
        (0, AxisKind::X, 0.1),
        (1, AxisKind::Y, 0.1),
        (2, AxisKind::Z, 0.5),
    ] {
        let a = &meta.axes()[axis];
        assert_eq!(
            (&a.kind, a.scale, a.unit.as_deref()),
            (&kind, scale, micron)
        );
    }
    assert_eq!(meta.channels[0].display_range, Some((1000.0, 1767.0)));
    for s in 0..3 {
        let expected: Vec<u16> = pattern(32, 24, |x, y| ((s + 1) * 1000 + x + 32 * y) as u16);
        assert_eq!(plane::<u16>(&image, s), expected, "slice {s}");
    }
}

#[test]
fn hyperstack() {
    let image = open("hyperstack_c3z4t5.tif");
    let meta = image.meta();
    assert_eq!(meta.dtype(), DType::U16);
    assert_eq!(meta.shape(), &[32, 24, 3, 4, 5]);
    assert_eq!(meta.plane_count(), 60);
    let micron = Some("\u{b5}m");
    let axes: Vec<_> = meta
        .axes()
        .iter()
        .map(|a| (a.kind.clone(), a.scale, a.origin, a.unit.as_deref()))
        .collect();
    assert_eq!(
        axes,
        [
            (AxisKind::X, 0.25, 0.0, micron),
            (AxisKind::Y, 0.25, 0.0, micron),
            (AxisKind::Channel, 1.0, 0.0, None),
            (AxisKind::Z, 1.5, 0.0, micron),
            (AxisKind::Time, 2.5, 0.0, Some("sec")),
        ]
    );

    // Plane order is XYCZT with the channel varying fastest.
    for t in 0..5 {
        for z in 0..4 {
            for c in 0..3 {
                let index = meta.plane_index(&[c, z, t]).unwrap();
                assert_eq!(index, c + 3 * z + 12 * t);
                let base = (c + 1) * 10000 + (z + 1) * 1000 + (t + 1) * 100;
                let expected: Vec<u16> = pattern(32, 24, |x, y| (base + x + 32 * y) as u16);
                assert_eq!(plane::<u16>(&image, index), expected, "c{c} z{z} t{t}");
            }
        }
    }
    let region = image
        .read_region(
            meta.plane_index(&[1, 2, 3]).unwrap(),
            Rect::new(30, 22, 2, 2),
        )
        .unwrap();
    let base = 20000 + 3000 + 400;
    let expected = [30 + 32 * 22, 31 + 32 * 22, 30 + 32 * 23, 31 + 32 * 23].map(|v| base + v);
    assert_eq!(region.try_slice::<u16>().unwrap(), expected);

    // Cyan, magenta and yellow channels blended, each with its own range.
    assert_eq!(meta.color, ColorModel::Composite);
    let expected = [
        ("Cyan", [0, 255, 255], (10000.0, 15000.0)),
        ("Magenta", [255, 0, 255], (20000.0, 26000.0)),
        ("Yellow", [255, 255, 0], (30000.0, 37000.0)),
    ];
    assert_eq!(meta.channels.len(), 3);
    for (channel, (name, top, range)) in meta.channels.iter().zip(expected) {
        let lut = channel.lut.as_ref().unwrap();
        assert_eq!(lut.name, name);
        assert_eq!(lut.table[255], top);
        assert_eq!(lut.table[0], [0, 0, 0]);
        assert_eq!(lut.table[128], top.map(|v| if v == 255 { 128 } else { 0 }));
        assert_eq!(channel.display_range, Some(range));
    }

    let property = |key: &str| meta.properties.get(key).map(String::as_str);
    assert_eq!(
        property("Info"),
        Some("fixture=hyperstack_c3z4t5\nkey=value")
    );
    // The macro labelled channel 2, slice 3, frame 4: stack index 44 (1-based).
    assert_eq!(property("imagej.label.43"), Some("c2-z3-t4 label"));
    assert_eq!(meta.plane_index(&[1, 2, 3]).unwrap(), 43);
    assert_eq!(property("CurrentLUT"), Some("yellow"));
    assert_eq!(meta.properties.len(), 3, "{:?}", meta.properties);
}

#[test]
fn every_corpus_tiff_is_covered() {
    // Fails when a fixture is added without a test above.
    let mut names: Vec<String> = std::fs::read_dir(fixture(""))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tif"))
        .collect();
    names.sort();
    let covered = [
        "blobs.tif",
        "blobs_count_masks.tif",
        "gray16.tif",
        "gray16_gauss2.tif",
        "gray16_invert.tif",
        "gray16_le.tif",
        "gray16_signed.tif",
        "gray32f.tif",
        "gray32f_gauss2.tif",
        "gray32f_invert.tif",
        "gray32f_le.tif",
        "gray8.tif",
        "gray8_calibrated.tif",
        "gray8_fire.tif",
        "gray8_gauss2.tif",
        "gray8_invert.tif",
        "gray8_invlut.tif",
        "gray8_otsu_mask.tif",
        "gray8_roi_overlay.tif",
        "hyperstack_c3z4t5.tif",
        "rgb.tif",
        "rgb_gauss2.tif",
        "rgb_invert.tif",
        "rgb_le.tif",
        "stack_z3_16bit_le.tif",
        "stack_z6_8bit.tif",
    ];
    assert_eq!(names, covered);
}
