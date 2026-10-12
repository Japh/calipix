//! Every filter against results written by ImageJ 1.54p.
//!
//! `fixtures/imagej/gen/50_filters.ijm` applies each command to a slice of a
//! stack of copies of a base image and lists the slices in
//! `filt_index.txt`. This test repeats every listed command and requires
//! the same samples: identical for integer types, and bit for bit for
//! float as well (NaN matching NaN).

use std::collections::BTreeMap;
use std::path::PathBuf;

use calipix_core::{Buffer, Rect};
use calipix_ops::filter::{
    Direction, Frame, Kernel, Outliers, Rank, convolve, despeckle, find_edges, gaussian_accuracy,
    gaussian_blur, rank_filter, remove_nans, remove_outliers, shadows, sharpen, smooth, top_hat,
    unsharp_mask,
};

const W: usize = 40;
const H: usize = 30;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// How a raw file stores a base image.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Gray8,
    Gray16,
    Float,
    Rgb,
}

fn kind_of(file: &str) -> Kind {
    match file {
        "filt_16.raw" => Kind::Gray16,
        "filt_32.raw" | "filt_nan.raw" => Kind::Float,
        "filt_rgb.raw" => Kind::Rgb,
        _ => Kind::Gray8,
    }
}

/// The slices of a raw stack; each slice is one plane, or three for RGB.
fn read_slices(file: &str, kind: Kind) -> Vec<Vec<Buffer>> {
    let bytes = std::fs::read(fixture(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
    let n = W * H;
    match kind {
        Kind::Gray8 => bytes
            .chunks(n)
            .map(|s| vec![Buffer::from_vec(s.to_vec())])
            .collect(),
        Kind::Gray16 => bytes
            .chunks(2 * n)
            .map(|s| {
                let v: Vec<u16> = s
                    .chunks(2)
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect();
                vec![Buffer::from_vec(v)]
            })
            .collect(),
        Kind::Float => bytes
            .chunks(4 * n)
            .map(|s| {
                let v: Vec<f32> = s
                    .chunks(4)
                    .map(|b| f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                vec![Buffer::from_vec(v)]
            })
            .collect(),
        Kind::Rgb => bytes
            .chunks(3 * n)
            .map(|s| {
                (0..3)
                    .map(|c| {
                        Buffer::from_vec(s.iter().skip(c).step_by(3).copied().collect::<Vec<u8>>())
                    })
                    .collect()
            })
            .collect(),
    }
}

/// ImageJ macro options: `key=value`, `key=[bracketed value]` and bare
/// keywords.
fn options(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let end = rest.find([' ', '=']).unwrap_or(rest.len());
        let key = rest[..end].to_string();
        rest = &rest[end..];
        let value = if let Some(after) = rest.strip_prefix("=[") {
            let close = after.find(']').expect("closing bracket");
            rest = &after[close + 1..];
            after[..close].to_string()
        } else if let Some(after) = rest.strip_prefix('=') {
            let close = after.find(' ').unwrap_or(after.len());
            rest = &after[close..];
            after[..close].to_string()
        } else {
            String::new()
        };
        map.insert(key, value);
        rest = rest.trim_start();
    }
    map
}

/// Runs one ImageJ command on one plane.
fn run(
    command: &str,
    opts: &BTreeMap<String, String>,
    plane: &Buffer,
    frame: Frame,
    rgb: bool,
    scale: (f64, f64),
) -> Buffer {
    let number = |key: &str| -> f64 {
        opts.get(key)
            .unwrap_or_else(|| panic!("{command}: no `{key}`"))
            .parse()
            .unwrap()
    };
    let flag = |key: &str| opts.contains_key(key);
    let rank = |r: Rank| rank_filter(plane, frame, number("radius"), r);
    let result = match command {
        "Gaussian Blur..." => {
            let sigma = number("sigma");
            let (sx, sy) = if flag("scaled") {
                (sigma / scale.0, sigma / scale.1)
            } else {
                (sigma, sigma)
            };
            gaussian_blur(plane, frame, sx, sy, gaussian_accuracy(plane.dtype()))
        }
        "Mean..." => rank(Rank::Mean),
        "Median..." => rank(Rank::Median),
        "Minimum..." => rank(Rank::Minimum),
        "Maximum..." => rank(Rank::Maximum),
        "Variance..." => rank(Rank::Variance),
        "Despeckle" => despeckle(plane, frame),
        "Remove Outliers..." => {
            let which = match opts["which"].as_str() {
                "Bright" => Outliers::Bright,
                "Dark" => Outliers::Dark,
                other => panic!("which={other}"),
            };
            remove_outliers(
                plane,
                frame,
                number("radius"),
                number("threshold"),
                which,
                false,
            )
        }
        "Remove NaNs..." => remove_nans(plane, frame, number("radius")),
        "Top Hat..." => top_hat(
            plane,
            frame,
            number("radius"),
            flag("light"),
            flag("don't"),
            false,
        ),
        "Convolve..." => {
            let kernel = Kernel::parse(&opts["text1"].replace('|', "\n")).unwrap();
            convolve(plane, frame, &kernel, flag("normalize"))
        }
        "Unsharp Mask..." => unsharp_mask(plane, frame, number("radius"), number("mask")),
        "Smooth" => smooth(plane, frame),
        "Sharpen" => sharpen(plane, frame, rgb),
        "Find Edges" => find_edges(plane, frame),
        direction => shadows(plane, frame, direction.parse::<Direction>().unwrap(), rgb),
    };
    result.unwrap_or_else(|e| panic!("{command}: {e}"))
}

/// Differences between two planes over some samples.
#[derive(Default, Clone, Copy)]
struct Diff {
    samples: usize,
    different: usize,
    max_abs: f64,
    max_rel: f64,
}

impl Diff {
    fn add(&mut self, ours: &Buffer, theirs: &Buffer, at: impl Iterator<Item = usize>) {
        for i in at {
            self.samples += 1;
            let same = match (ours, theirs) {
                (Buffer::F32(a), Buffer::F32(b)) => {
                    a[i].to_bits() == b[i].to_bits() || (a[i].is_nan() && b[i].is_nan())
                }
                _ => ours.get_f64(i) == theirs.get_f64(i),
            };
            if !same {
                self.different += 1;
                let (a, b) = (ours.get_f64(i).unwrap(), theirs.get_f64(i).unwrap());
                let abs = (a - b).abs();
                let abs = if abs.is_nan() { f64::INFINITY } else { abs };
                self.max_abs = self.max_abs.max(abs);
                self.max_rel = self.max_rel.max(abs / b.abs().max(f64::MIN_POSITIVE));
            }
        }
    }

    fn merge(&mut self, other: Diff) {
        self.samples += other.samples;
        self.different += other.different;
        self.max_abs = self.max_abs.max(other.max_abs);
        self.max_rel = self.max_rel.max(other.max_rel);
    }
}

fn inside(r: Rect) -> impl Iterator<Item = usize> {
    (r.y..r.y + r.h).flat_map(move |y| (r.x..r.x + r.w).map(move |x| y * W + x))
}

fn outside(r: Rect) -> impl Iterator<Item = usize> {
    (0..W * H).filter(move |i| {
        let (x, y) = (i % W, i / W);
        !(x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
    })
}

#[test]
fn every_listed_command_matches_imagej() {
    let index = std::fs::read_to_string(fixture("filt_index.txt")).unwrap();
    let mut stacks: BTreeMap<String, Vec<Vec<Buffer>>> = BTreeMap::new();
    // Per command and pixel type.
    let mut report: BTreeMap<(String, &str), (usize, Diff)> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut cases = 0;
    let mut without_effect = Vec::new();
    let mut imagej_changed_outside = Vec::new();
    for line in index.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [file, slice, command, opts, roi] = fields[..] else {
            panic!("bad index line: {line}");
        };
        if command == "base" {
            continue;
        }
        let kind = kind_of(file);
        let slices = stacks
            .entry(file.to_string())
            .or_insert_with(|| read_slices(file, kind));
        let expected = &slices[slice.parse::<usize>().unwrap() - 1];
        let base = &slices[0];
        let rect = if roi == "-" {
            Rect::new(0, 0, W, H)
        } else {
            let v: Vec<usize> = roi.split(' ').map(|n| n.parse().unwrap()).collect();
            Rect::new(v[0], v[1], v[2], v[3])
        };
        let frame = Frame::new(W, H, rect);
        let scale = if file == "filt_8cal.raw" {
            (0.5, 0.25)
        } else {
            (1.0, 1.0)
        };
        let opts = options(opts);
        cases += 1;
        // A reference equal to its base would prove nothing.
        let mut effect = Diff::default();
        let mut beside = Diff::default();
        for (plane, reference) in base.iter().zip(expected) {
            effect.add(reference, plane, inside(rect));
            beside.add(reference, plane, outside(rect));
        }
        if effect.different == 0 {
            without_effect.push(line.to_string());
        }
        if beside.different > 0 {
            imagej_changed_outside.push(line.to_string());
        }
        for (channel, plane) in base.iter().enumerate() {
            let ours = run(command, &opts, plane, frame, kind == Kind::Rgb, scale);
            let mut diff = Diff::default();
            diff.add(&ours, &expected[channel], inside(rect));
            let mut untouched = Diff::default();
            untouched.add(&ours, plane, outside(rect));
            if diff.different > 0 || untouched.different > 0 {
                failures.push(format!(
                    "{line}: channel {channel}: {} of {} samples differ (max abs {:e}, rel {:e}); \
                     {} samples outside the selection changed",
                    diff.different, diff.samples, diff.max_abs, diff.max_rel, untouched.different
                ));
            }
            let entry = report
                .entry((command.to_string(), plane.dtype().name()))
                .or_default();
            entry.0 += 1;
            entry.1.merge(diff);
        }
    }
    println!("{cases} cases");
    for ((command, dtype), (planes, diff)) in &report {
        println!(
            "{command:20} {dtype:4} {planes:3} planes {:6} samples: {} differ, max abs {:e}, max rel {:e}",
            diff.samples, diff.different, diff.max_abs, diff.max_rel
        );
    }
    println!("ImageJ changed nothing in: {without_effect:#?}");
    println!("ImageJ changed samples outside the selection in: {imagej_changed_outside:#?}");
    assert!(cases > 150, "only {cases} cases in the index");
    assert!(without_effect.is_empty(), "{without_effect:#?}");
    // ImageJ's Gaussian blur writes beside a narrow selection that lies
    // within a kernel radius of the left edge; nothing else strays.
    assert_eq!(
        imagej_changed_outside,
        ["filt_16.raw	7	Gaussian Blur...	sigma=2	1 2 3 24"]
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The `stack` keyword filters every slice the same way, each on its own.
#[test]
fn whole_stacks_match_imagej() {
    let read = |file: &str| read_slices(file, Kind::Gray16);
    let base = read("filt_stack16_base.raw");
    assert_eq!(base.len(), 3);
    let blurred = read("filt_stack16_gauss.raw");
    let median = read("filt_stack16_median.raw");
    let rect = Rect::new(6, 5, 20, 14);
    for (i, slice) in base.iter().enumerate() {
        let ours = gaussian_blur(&slice[0], Frame::whole(W, H), 1.5, 1.5, 0.0002).unwrap();
        assert_eq!(ours, blurred[i][0], "Gaussian, slice {}", i + 1);
        let ours = rank_filter(&slice[0], Frame::new(W, H, rect), 2.0, Rank::Median).unwrap();
        assert_eq!(ours, median[i][0], "Median, slice {}", i + 1);
    }
}

/// The older full-size references of `02_ops.ijm`: Gaussian blur, sigma 2,
/// of each base image.
#[test]
fn gaussian_sigma_2_matches_the_tiff_references() {
    for base in ["gray8", "gray16", "gray32f", "rgb"] {
        let source = calipix_io::open(fixture(&format!("{base}.tif"))).unwrap();
        let expected = calipix_io::open(fixture(&format!("{base}_gauss2.tif"))).unwrap();
        let meta = source.meta();
        let frame = Frame::whole(meta.width(), meta.height());
        for plane in 0..meta.plane_count() {
            let input = source.read_plane(plane).unwrap();
            let accuracy = gaussian_accuracy(input.dtype());
            let ours = gaussian_blur(&input, frame, 2.0, 2.0, accuracy).unwrap();
            assert_eq!(
                ours,
                expected.read_plane(plane).unwrap(),
                "{base} plane {plane}"
            );
        }
    }
}

/// Above sigma 8.5 ImageJ blurs a downscaled copy of each line, on a grid
/// that starts a kernel radius before the selection. These selections of
/// the full 64 x 48 images lie far enough from the left and top edges for
/// that to matter: the result must match ImageJ, and for float must not be
/// the same as blurring the whole plane.
#[test]
fn a_large_gaussian_in_a_selection_matches_imagej() {
    let (w, h) = (64, 48);
    type Run = fn(&Buffer, Frame) -> Buffer;
    let cases: [(&str, &str, Rect, Run); 3] = [
        (
            "gray8.tif",
            "filt_big8_gauss9.raw",
            Rect::new(41, 37, 20, 10),
            |p, f| gaussian_blur(p, f, 9.0, 9.0, 0.002).unwrap(),
        ),
        (
            "gray32f.tif",
            "filt_big32_gauss9.raw",
            Rect::new(41, 41, 20, 6),
            |p, f| gaussian_blur(p, f, 9.0, 9.0, 0.0002).unwrap(),
        ),
        (
            "gray16.tif",
            "filt_big16_unsharp9.raw",
            Rect::new(43, 40, 20, 8),
            |p, f| unsharp_mask(p, f, 9.0, 0.6).unwrap(),
        ),
    ];
    for (source, reference, rect, run) in cases {
        let input = calipix_io::open(fixture(source))
            .unwrap()
            .read_plane(0)
            .unwrap();
        let bytes = std::fs::read(fixture(reference)).unwrap();
        let expected = match &input {
            Buffer::U8(_) => Buffer::from_vec(bytes),
            Buffer::U16(_) => Buffer::from_vec(
                bytes
                    .chunks(2)
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect::<Vec<_>>(),
            ),
            _ => Buffer::from_vec(
                bytes
                    .chunks(4)
                    .map(|b| f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                    .collect::<Vec<_>>(),
            ),
        };
        let ours = run(&input, Frame::new(w, h, rect));
        assert_eq!(ours, expected, "{reference}");
        let whole = run(&input, Frame::whole(w, h));
        let in_rect = |i: &usize| {
            let (x, y) = (i % w, i / w);
            x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
        };
        // Rounding to integers hides the difference; float shows it.
        if input.dtype().is_float() {
            assert!(
                (0..w * h)
                    .filter(in_rect)
                    .any(|i| whole.get_f64(i) != ours.get_f64(i)),
                "{reference}: the selection made no difference"
            );
        }
    }
}
