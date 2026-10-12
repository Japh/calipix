//! Helpers shared by the tests that compare against ImageJ's results in
//! `fixtures/imagej/out` (written by `fixtures/imagej/gen/6?_*.ijm`).
#![allow(dead_code)]

use std::path::PathBuf;

use calipix_core::{Buffer, Image, Rect, crop_buffer};

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// One of the base images written by `01_planes.ijm` and `03_hyperstack.ijm`.
pub fn base(name: &str) -> Image {
    let path = fixture(&format!("{name}.tif"));
    calipix_io::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The rows of a CSV fixture after the header, split at commas.
pub fn csv(name: &str) -> Vec<Vec<String>> {
    let path = fixture(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| line.split(',').map(str::to_string).collect())
        .collect()
}

/// What a raw file written by ImageJ holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raw {
    U8,
    U16,
    F32,
    /// Three bytes per pixel: red, green, blue.
    Rgb,
}

impl Raw {
    pub fn for_base(base: &str) -> Raw {
        match base {
            "gray8" | "stack_z6_8bit" => Raw::U8,
            "gray16" | "hyperstack_c3z4t5" => Raw::U16,
            "gray32f" => Raw::F32,
            "rgb" => Raw::Rgb,
            other => panic!("unknown base image {other}"),
        }
    }

    pub fn from_bit_depth(bits: &str) -> Raw {
        match bits {
            "8" => Raw::U8,
            "16" => Raw::U16,
            "32" => Raw::F32,
            "24" => Raw::Rgb,
            other => panic!("unknown bit depth {other}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Raw::U8 => "8-bit",
            Raw::U16 => "16-bit",
            Raw::F32 => "32-bit",
            Raw::Rgb => "RGB",
        }
    }

    fn bytes_per_pixel(self) -> usize {
        match self {
            Raw::U8 => 1,
            Raw::U16 => 2,
            Raw::F32 => 4,
            Raw::Rgb => 3,
        }
    }
}

/// The planes of an ImageJ "Raw Data" file (big-endian). Each plane is one
/// buffer, or three (red, green, blue) for RGB.
pub fn raw_planes(name: &str, kind: Raw, width: usize, height: usize) -> Vec<Vec<Buffer>> {
    let path = fixture(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let plane_bytes = width * height * kind.bytes_per_pixel();
    assert_eq!(
        bytes.len() % plane_bytes,
        0,
        "{name} is not a whole number of {width}x{height} planes"
    );
    bytes
        .chunks(plane_bytes)
        .map(|plane| match kind {
            Raw::U8 => vec![Buffer::from_vec(plane.to_vec())],
            Raw::U16 => vec![Buffer::from_vec(
                plane
                    .chunks(2)
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect::<Vec<_>>(),
            )],
            Raw::F32 => vec![Buffer::from_vec(
                plane
                    .chunks(4)
                    .map(|b| f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                    .collect::<Vec<_>>(),
            )],
            Raw::Rgb => (0..3)
                .map(|c| Buffer::from_vec(plane.chunks(3).map(|b| b[c]).collect::<Vec<u8>>()))
                .collect(),
        })
        .collect()
}

/// The channel planes of plane group `index` of an image: one plane, or the
/// three channels of an RGB image.
pub fn planes_of(image: &Image, rgb: bool) -> Vec<Buffer> {
    let count = if rgb { 3 } else { 1 };
    (0..count).map(|p| image.read_plane(p).unwrap()).collect()
}

pub fn crop(plane: &Buffer, width: usize, rect: Rect) -> Buffer {
    crop_buffer(plane, width, rect).unwrap()
}

/// A copy of `plane` with some samples replaced.
pub fn with_samples<T: calipix_core::Pixel>(plane: &Buffer, values: &[(usize, T)]) -> Buffer {
    let mut samples = plane.as_slice::<T>().expect("pixel type").to_vec();
    for &(index, value) in values {
        samples[index] = value;
    }
    Buffer::from_vec(samples)
}

/// How far a float result is from ImageJ's.
#[derive(Clone, Copy, Debug, Default)]
pub struct Difference {
    /// Largest relative difference, `|a - b| / max(|a|, |b|)`.
    pub max_relative: f64,
    /// Samples that are not bit-for-bit equal (NaN equals NaN).
    pub unequal: usize,
    pub samples: usize,
}

impl Difference {
    pub fn merge(&mut self, other: Difference) {
        self.max_relative = self.max_relative.max(other.max_relative);
        self.unequal += other.unequal;
        self.samples += other.samples;
    }
}

/// Compares a result with ImageJ's. Integer planes must be equal. Float
/// planes must agree in which samples are NaN and infinite, and are
/// otherwise measured.
pub fn compare(ours: &Buffer, theirs: &Buffer, what: &str) -> Difference {
    assert_eq!(ours.dtype(), theirs.dtype(), "{what}: pixel type");
    assert_eq!(ours.len(), theirs.len(), "{what}: size");
    let (Some(a), Some(b)) = (ours.as_slice::<f32>(), theirs.as_slice::<f32>()) else {
        if ours != theirs {
            let first = (0..ours.len())
                .find(|&i| ours.get_f64(i) != theirs.get_f64(i))
                .unwrap();
            let count = (0..ours.len())
                .filter(|&i| ours.get_f64(i) != theirs.get_f64(i))
                .count();
            panic!(
                "{what}: {count} of {} samples differ, first at {first}: ours {:?}, ImageJ {:?}",
                ours.len(),
                ours.get_f64(first),
                theirs.get_f64(first)
            );
        }
        return Difference {
            samples: ours.len(),
            ..Difference::default()
        };
    };
    let mut difference = Difference {
        samples: a.len(),
        ..Difference::default()
    };
    for (i, (&x, &y)) in a.iter().zip(b).enumerate() {
        if x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()) {
            continue;
        }
        difference.unequal += 1;
        assert!(
            x.is_finite() && y.is_finite(),
            "{what}: sample {i} is {x} but ImageJ has {y}"
        );
        let scale = x.abs().max(y.abs()) as f64;
        let relative = (x as f64 - y as f64).abs() / scale;
        difference.max_relative = difference.max_relative.max(relative);
    }
    difference
}

/// [`compare`], requiring float results to be within 1e-5 relative error.
pub fn assert_matches(ours: &Buffer, theirs: &Buffer, what: &str) -> Difference {
    let difference = compare(ours, theirs, what);
    assert!(
        difference.max_relative <= 1e-5,
        "{what}: relative error {} exceeds 1e-5",
        difference.max_relative
    );
    difference
}

/// Prints one line of a per-operation report (shown with `--nocapture`).
pub fn report(operation: &str, difference: Difference) {
    if difference.unequal == 0 {
        println!(
            "{operation}: identical to ImageJ ({} samples)",
            difference.samples
        );
    } else {
        println!(
            "{operation}: {} of {} samples differ, largest relative difference {:.3e}",
            difference.unequal, difference.samples, difference.max_relative
        );
    }
}

/// The second operands `61_calculator.ijm` generates, one plane each (three
/// for `brgb`), from the formulas documented there.
pub fn generated(name: &str) -> Vec<Buffer> {
    let (w, h) = if name == "small8" { (20, 10) } else { (32, 24) };
    let xy = || (0..h).flat_map(move |y| (0..w).map(move |x| (x, y)));
    let holes = |x: usize, y: usize| x.is_multiple_of(5) && y.is_multiple_of(3);
    match name {
        "b8" => vec![Buffer::from_vec(
            xy().map(|(x, y)| {
                if holes(x, y) {
                    0
                } else {
                    ((x * 7 + y * 13) % 256) as u8
                }
            })
            .collect::<Vec<_>>(),
        )],
        "b16" => {
            let mut v: Vec<u16> = xy()
                .map(|(x, y)| {
                    if holes(x, y) {
                        0
                    } else {
                        ((x * 2021 + y * 517) % 65536) as u16
                    }
                })
                .collect();
            v[1] = 65535;
            vec![Buffer::from_vec(v)]
        }
        "bf" => {
            let mut v: Vec<f32> = xy()
                .map(|(x, y)| {
                    ((x as f64 - 10.0) * 0.5 - (y % 7) as f64 + 0.25 * (x % 3) as f64) as f32
                })
                .collect();
            v[w + 2] = f32::NAN;
            vec![Buffer::from_vec(v)]
        }
        "brgb" => (0..3)
            .map(|c| {
                Buffer::from_vec(
                    xy().map(|(x, y)| {
                        if x.is_multiple_of(6) && y.is_multiple_of(4) {
                            0
                        } else {
                            [(x * 9) & 255, (y * 11) & 255, (x * y) & 255][c] as u8
                        }
                    })
                    .collect::<Vec<_>>(),
                )
            })
            .collect(),
        "small8" => vec![Buffer::from_vec(
            xy().map(|(x, y)| ((x * 11 + y * 3) % 256) as u8)
                .collect::<Vec<_>>(),
        )],
        other => panic!("unknown generated image {other}"),
    }
}

/// The values `60_math.ijm` and `61_calculator.ijm` write into the first
/// samples of the cropped 16-bit and float base planes.
pub fn with_math_specials(name: &str, plane: &Buffer) -> Buffer {
    match name {
        "gray32f" => with_samples(
            plane,
            &[
                (0, f32::NAN),
                (1, 0.0),
                (2, f32::INFINITY),
                (3, f32::NEG_INFINITY),
                (4, 1e-30),
                (5, 3.4e38),
                (6, -2.5),
                (7, 0.5),
            ],
        ),
        "gray16" => with_samples(plane, &[(0, 0u16), (1, 65535), (2, 1), (3, 32768)]),
        _ => plane.clone(),
    }
}

/// The values `62_convert.ijm` writes into the cropped 16-bit and float
/// base planes.
pub fn with_convert_specials(name: &str, plane: &Buffer) -> Buffer {
    match name {
        "gray32f" => with_samples(
            plane,
            &[
                (0, f32::NAN),
                (1, 0.0),
                (2, 300.5),
                (3, -70000.0),
                (4, 1e-30),
                (5, 70000.6),
                (6, -2.5),
                (7, 0.5),
                (8, 254.5),
                (9, 255.49),
                (10, 65534.5),
                (11, 1.5),
            ],
        ),
        "gray16" => with_samples(plane, &[(0, 0u16), (1, 65535), (2, 255), (3, 256)]),
        _ => plane.clone(),
    }
}

/// A base image cropped to `rect`, in memory, with `edit` applied to each
/// of its planes. Metadata, including the display range, is kept; the
/// calibration origin stays at zero, as when ImageJ crops an image that
/// has none.
pub fn cropped_base(name: &str, rect: Rect, edit: impl Fn(&Buffer) -> Buffer) -> Image {
    let view = base(name).crop(rect).unwrap();
    let mut meta = view.meta().clone();
    for axis in 0..2 {
        meta.axis_mut(axis).unwrap().origin = 0.0;
    }
    let mut image = view.with_meta(meta).unwrap().materialize().unwrap();
    for plane in 0..image.meta().plane_count() {
        let edited = edit(&image.read_plane(plane).unwrap());
        image = image.with_plane(plane, edited).unwrap();
    }
    image
}

/// A number as ImageJ's macro language prints it.
pub fn ij_number(text: &str) -> f64 {
    text.parse()
        .unwrap_or_else(|_| panic!("`{text}` is not a number"))
}

/// True if `ours` is the number ImageJ printed as `theirs` with four
/// decimals.
pub fn close(ours: f64, theirs: f64) -> bool {
    ours == theirs
        || (ours.is_nan() && theirs.is_nan())
        || (ours - theirs).abs() <= 1e-4 * (1.0 + theirs.abs())
}
