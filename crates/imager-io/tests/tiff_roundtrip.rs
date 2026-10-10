//! Writing: what `save` produces opens again as the same image, for every
//! pixel type and shape, and files re-saved from ImageJ's own carry the
//! description ImageJ wrote.

mod tiff_support;

use std::collections::BTreeMap;

use imager_core::{
    Axis, AxisKind, Buffer, ChannelMeta, ColorModel, Complex32, DType, Image, ImageMeta, Lut,
};
use proptest::prelude::*;
use tiff_support::{fixture, scratch};

/// A small deterministic generator, so one seed describes a whole image.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }

    fn pick<T: Clone>(&mut self, options: &[T]) -> T {
        options[self.below(options.len())].clone()
    }
}

const REAL_TYPES: [DType; 10] = [
    DType::U8,
    DType::I8,
    DType::U16,
    DType::I16,
    DType::U32,
    DType::I32,
    DType::U64,
    DType::I64,
    DType::F32,
    DType::F64,
];

/// A plane of arbitrary bit patterns (for floats: finite values, since NaN
/// payloads would compare unequal).
fn random_plane(dtype: DType, len: usize, rng: &mut Rng) -> Buffer {
    macro_rules! ints {
        ($t:ty) => {
            Buffer::from_vec((0..len).map(|_| rng.next() as $t).collect::<Vec<$t>>())
        };
    }
    match dtype {
        DType::U8 => ints!(u8),
        DType::I8 => ints!(i8),
        DType::U16 => ints!(u16),
        DType::I16 => ints!(i16),
        DType::U32 => ints!(u32),
        DType::I32 => ints!(i32),
        DType::U64 => ints!(u64),
        DType::I64 => ints!(i64),
        DType::F32 => Buffer::from_vec(
            (0..len)
                .map(|_| (rng.next() as i32) as f32 / 1024.0)
                .collect::<Vec<f32>>(),
        ),
        DType::F64 => Buffer::from_vec(
            (0..len)
                .map(|_| (rng.next() as i64) as f64 / 1024.0)
                .collect::<Vec<f64>>(),
        ),
        _ => unreachable!(),
    }
}

fn random_lut(rng: &mut Rng) -> Lut {
    match rng.below(4) {
        0 => Lut::grays(),
        1 => Lut::grays().inverted(),
        2 => Lut::from_fn("Magenta", |i| [i, 0, i]),
        _ => {
            let seed = rng.next();
            Lut::from_fn("Custom", move |i| {
                let v = seed.rotate_left(u32::from(i) % 64) ^ (u64::from(i) * 0x9e37_79b9);
                [v as u8, (v >> 8) as u8, (v >> 16) as u8]
            })
        }
    }
}

/// An image whose metadata is all of a kind TIFF preserves: axes in
/// channel, Z, time order; Y and Z units equal to the X unit or explicitly
/// different; channel tables and ranges either on every channel or on none.
fn random_image(
    dtype: DType,
    (width, height): (usize, usize),
    (channels, slices, frames): (usize, usize, usize),
    rgb: bool,
    seed: u64,
) -> Image {
    let rng = &mut Rng(seed | 1);
    let unit: Option<String> = rng
        .pick(&[None, Some("\u{b5}m"), Some("mm"), Some("inch")])
        .map(String::from);
    let scale = |rng: &mut Rng| rng.pick(&[1.0, 0.25, 0.3, 1.5, 0.065, 2.0, 1.0 / 3.0]);
    let origin = |rng: &mut Rng, scale: f64| -rng.pick(&[0.0, 0.0, 3.0, -2.5]) * scale;
    let other_unit = |rng: &mut Rng, unit: &Option<String>| {
        if rng.coin() {
            unit.clone()
        } else {
            Some("nm".to_string())
        }
    };

    let (sx, sy, sz) = (scale(rng), scale(rng), scale(rng));
    let mut axes = vec![
        (
            Axis {
                kind: AxisKind::X,
                scale: sx,
                origin: origin(rng, sx) + 0.0,
                unit: unit.clone(),
            },
            width,
        ),
        (
            Axis {
                kind: AxisKind::Y,
                scale: sy,
                origin: origin(rng, sy) + 0.0,
                unit: other_unit(rng, &unit),
            },
            height,
        ),
    ];
    if channels > 1 {
        axes.push((Axis::new(AxisKind::Channel), channels));
    }
    if slices > 1 {
        axes.push((
            Axis {
                kind: AxisKind::Z,
                scale: sz,
                origin: origin(rng, sz) + 0.0,
                unit: other_unit(rng, &unit),
            },
            slices,
        ));
    }
    if frames > 1 {
        let time = if rng.coin() {
            Axis::new(AxisKind::Time)
        } else {
            let unit = rng.pick(&["sec", "min", "ms"]);
            Axis::calibrated(AxisKind::Time, rng.pick(&[1.0, 2.0, 2.5, 0.04]), unit)
        };
        axes.push((time, frames));
    }
    let mut meta = ImageMeta::new(dtype, axes).unwrap();
    let images = if rgb {
        slices * frames
    } else {
        channels * slices * frames
    };

    if rgb {
        meta.color = ColorModel::Rgb;
    } else {
        if channels > 1 && rng.coin() {
            meta.color = ColorModel::Composite;
        }
        // Composite images always have tables (ImageJ supplies colours).
        let with_luts = meta.color == ColorModel::Composite || rng.coin();
        let with_ranges = rng.coin();
        if with_luts || with_ranges {
            meta.channels = (0..channels)
                .map(|_| ChannelMeta {
                    name: None,
                    lut: with_luts.then(|| random_lut(rng)),
                    display_range: with_ranges.then(|| {
                        let low = rng.below(1000) as f64 - 500.0;
                        // Ranges of integer images are whole numbers.
                        (low, low + 1.0 + rng.below(5000) as f64)
                    }),
                })
                .collect();
        }
    }

    let mut properties = BTreeMap::new();
    if rng.coin() {
        properties.insert(
            "Info".to_string(),
            "acquired \u{3bb}=488 nm\nline two = x".to_string(),
        );
    }
    if rng.coin() {
        let slice = rng.below(images);
        properties.insert(
            format!("imagej.label.{slice}"),
            format!("label of {slice} \u{e9}"),
        );
    }
    if rng.coin() {
        properties.insert("Objective".to_string(), "63x/1.4 Oil".to_string());
        properties.insert("empty value".to_string(), String::new());
    }
    if rng.coin() {
        properties.insert("imagej.roi".to_string(), "496f757400e3".to_string());
        properties.insert("imagej.overlay.0".to_string(), "496f757401".to_string());
        properties.insert("imagej.overlay.1".to_string(), "496f757402".to_string());
    }
    if rng.coin() && dtype != DType::I16 {
        for (key, value) in [("cf", "0"), ("c0", "0.5"), ("c1", "0.01"), ("vunit", "OD")] {
            properties.insert(format!("imagej.{key}"), value.to_string());
        }
    }
    if rng.coin() {
        properties.insert("imagej.inverty".to_string(), "true".to_string());
        properties.insert(
            "imagej.somethingnew".to_string(),
            "kept \u{b5} as is".to_string(),
        );
    }
    if images > 1 && rng.coin() {
        properties.insert("imagej.fps".to_string(), "12.5".to_string());
        properties.insert("imagej.loop".to_string(), "true".to_string());
    }
    meta.properties = properties;

    let planes = (0..meta.plane_count())
        .map(|_| random_plane(dtype, width * height, rng))
        .collect();
    Image::from_planes(meta, planes).unwrap()
}

/// Saves, re-opens and compares everything.
fn assert_round_trip(image: &Image, name: &str) {
    let path = scratch(name);
    imager_io::save(image, &path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let back = imager_io::open(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut expected = image.meta().clone();
    expected.name = name.to_string();
    assert_eq!(back.meta(), &expected, "{name}");
    for p in 0..expected.plane_count() {
        assert_eq!(
            back.read_plane(p).unwrap(),
            image.read_plane(p).unwrap(),
            "{name} plane {p}"
        );
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn every_type_and_shape_round_trips() {
    let shapes = [
        (1, 1, 1),
        (2, 1, 1),
        (3, 1, 1),
        (1, 4, 1),
        (1, 1, 5),
        (2, 3, 1),
        (3, 1, 2),
        (1, 2, 3),
        (2, 3, 4),
        (9, 2, 1),
    ];
    let mut seed = 0x1234_5678_9abc_def1u64;
    for dtype in REAL_TYPES {
        for dims in shapes {
            for rgb in [false, true] {
                if rgb && dims.0 != 3 {
                    continue;
                }
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let image = random_image(dtype, (7, 5), dims, rgb, seed);
                let name = format!("rt_{dtype}_{}x{}x{}_{rgb}.tif", dims.0, dims.1, dims.2);
                assert_round_trip(&image, &name);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    #[test]
    fn write_then_read_is_identity(
        dtype in 0usize..REAL_TYPES.len(),
        size in (1usize..10, 1usize..8),
        dims in (1usize..4, 1usize..4, 1usize..4),
        rgb in any::<bool>(),
        seed in any::<u64>(),
    ) {
        let rgb = rgb && dims.0 == 3;
        let image = random_image(REAL_TYPES[dtype], size, dims, rgb, seed);
        assert_round_trip(&image, &format!("prop_{seed:016x}.tif"));
    }
}

#[test]
fn axes_in_another_order_are_written_in_imagej_order() {
    // Z before channel: plane numbers differ from ImageJ's, positions do not.
    let axes = vec![
        (Axis::new(AxisKind::X), 4),
        (Axis::new(AxisKind::Y), 3),
        (Axis::new(AxisKind::Z), 2),
        (Axis::new(AxisKind::Channel), 3),
    ];
    let meta = ImageMeta::new(DType::U16, axes).unwrap();
    let planes: Vec<Buffer> = (0..6)
        .map(|p| Buffer::from_vec(vec![p as u16 * 100; 12]))
        .collect();
    let image = Image::from_planes(meta, planes).unwrap();
    let path = scratch("zc_order.tif");
    imager_io::save(&image, &path).unwrap();
    let back = imager_io::open(&path).unwrap();
    assert_eq!(back.meta().shape(), &[4, 3, 3, 2]);
    assert_eq!(back.meta().axes()[2].kind, AxisKind::Channel);
    for z in 0..2 {
        for c in 0..3 {
            let ours = image.meta().plane_index(&[z, c]).unwrap();
            let theirs = back.meta().plane_index(&[c, z]).unwrap();
            assert_eq!(
                back.read_plane(theirs).unwrap(),
                image.read_plane(ours).unwrap()
            );
        }
    }
}

#[test]
fn what_tiff_cannot_hold_is_refused() {
    let complex = Image::from_vec(1, 1, vec![Complex32::new(1.0, 2.0)]).unwrap();
    let path = scratch("refused.tif");
    let message = imager_io::save(&complex, &path).unwrap_err().to_string();
    assert!(
        message.contains("complex") && message.contains("c32"),
        "{message}"
    );

    let axes = vec![
        (Axis::new(AxisKind::X), 2),
        (Axis::new(AxisKind::Y), 2),
        (Axis::new(AxisKind::Other("lambda".to_string())), 2),
    ];
    let meta = ImageMeta::new(DType::U8, axes).unwrap();
    let spectral = Image::from_planes(meta, vec![Buffer::zeros(DType::U8, 4); 2]).unwrap();
    let message = imager_io::save(&spectral, &path).unwrap_err().to_string();
    assert!(message.contains("lambda"), "{message}");
    // A refused save leaves nothing behind.
    assert!(!path.exists());
    let leftovers = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("refused")
        })
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn a_lazily_read_image_can_be_saved_over_its_own_file() {
    let image = random_image(DType::U16, (9, 6), (2, 3, 1), false, 99);
    let path = scratch("in_place.tif");
    imager_io::save(&image, &path).unwrap();
    let opened = imager_io::open(&path).unwrap();
    // Either the save succeeds, or it fails and leaves the file intact;
    // which one depends on whether the platform lets an open file be
    // replaced.
    let _ = imager_io::save(&opened, &path);
    drop(opened);
    let back = imager_io::open(&path).unwrap();
    for p in 0..6 {
        assert_eq!(back.read_plane(p).unwrap(), image.read_plane(p).unwrap());
    }
}

/// The ImageJ description stored in a TIFF, found by its marker.
fn description(bytes: &[u8]) -> String {
    let start = bytes
        .windows(7)
        .position(|w| w == b"ImageJ=")
        .expect("an ImageJ description");
    let end = start + bytes[start..].iter().position(|&b| b == 0).unwrap();
    String::from_utf8(bytes[start..end].to_vec()).unwrap()
}

#[test]
fn resaving_imagej_files_reproduces_their_description_and_content() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixture("")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if !name.ends_with(".tif") {
            continue;
        }
        let original = imager_io::open(fixture(&name)).unwrap();
        let path = scratch(&name);
        imager_io::save(&original, &path).unwrap();
        let back = imager_io::open(&path).unwrap();
        assert_eq!(back.meta(), original.meta(), "{name}");
        for p in 0..original.meta().plane_count() {
            assert_eq!(
                back.read_plane(p).unwrap(),
                original.read_plane(p).unwrap(),
                "{name}"
            );
        }
        // Line for line what `FileSaver.getDescriptionString` produced.
        let theirs = description(&std::fs::read(fixture(&name)).unwrap());
        let ours = description(&std::fs::read(&path).unwrap());
        assert_eq!(ours, theirs, "{name}");
        // Our files are little-endian.
        assert_eq!(&std::fs::read(&path).unwrap()[..4], b"II\x2a\x00");
        checked += 1;
    }
    assert_eq!(checked, 31);
}

#[test]
fn imagej_types_match_imagej_byte_for_byte_in_little_endian() {
    // ImageJ wrote these three with "Intel byte order"; re-saving them must
    // give the very same file, directory entries, metadata and pixels.
    for name in [
        "gray16_le.tif",
        "gray32f_le.tif",
        "rgb_le.tif",
        "stack_z3_16bit_le.tif",
    ] {
        let original = imager_io::open(fixture(name)).unwrap();
        let path = scratch(&format!("bytes_{name}"));
        imager_io::save(&original, &path).unwrap();
        let ours = std::fs::read(&path).unwrap();
        let theirs = std::fs::read(fixture(name)).unwrap();
        assert_eq!(ours.len(), theirs.len(), "{name}");
        let differing = ours.iter().zip(&theirs).filter(|(a, b)| a != b).count();
        assert_eq!(differing, 0, "{name}: {differing} bytes differ");
    }
}
