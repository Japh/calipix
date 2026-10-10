//! The renderer against what ImageJ itself produced.
//!
//! The reference files live in `fixtures/imagej/out` and are written by
//! real ImageJ from the macros `fixtures/imagej/gen/3?_*.ijm` (regenerate
//! with `cargo xtask gen`). The test images are rebuilt here from the
//! formulas documented in those macros. Integer results must match exactly
//! and floating point results to 1e-5 relative error.

use std::collections::BTreeMap;
use std::path::Path;

use imager_core::{Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut, Pixel};
use imager_render::{
    Blend, ChannelMode, DisplayOptions, Histogram, HistogramOptions, LevelMap, builtin_lut,
    builtin_luts, enhance_contrast, inverted_lut, plane_histogram, render_plane_with, reset_range,
    stack_histogram, value_counts,
};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn numbers<T: std::str::FromStr>(fields: &[&str]) -> Vec<T>
where
    T::Err: std::fmt::Debug,
{
    fields.iter().map(|f| f.parse().unwrap()).collect()
}

fn close(ours: f64, imagej: f64) -> bool {
    ours == imagej || (ours - imagej).abs() <= 1e-5 * ours.abs().max(imagej.abs())
}

fn packed(picture: &imager_render::Rgba8) -> Vec<u32> {
    picture
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)
        .collect()
}

/// A lookup table by the name used in the fixtures; `inverted` applies
/// Invert LUT.
fn lut(name: &str, inverted: bool) -> Lut {
    let lut = builtin_lut(name).unwrap_or_else(|| panic!("no built-in LUT {name:?}"));
    if inverted { inverted_lut(&lut) } else { lut }
}

fn with_channels(image: Image, channels: Vec<ChannelMeta>) -> Image {
    let mut meta = image.meta().clone();
    meta.channels = channels;
    image.with_meta(meta).unwrap()
}

fn channel(lut: Option<Lut>, display_range: Option<(f64, f64)>) -> ChannelMeta {
    ChannelMeta {
        name: None,
        lut,
        display_range,
    }
}

#[test]
fn builtin_luts_match_lut_loader() {
    let text = fixture("luts.csv");
    let mut seen = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').collect();
        let name = fields[0];
        let expected: Vec<u8> = numbers(&fields[1..]);
        assert_eq!(expected.len(), 768, "{name}");
        let ours = match name.strip_suffix(" inverted") {
            Some(base) => lut(base, true),
            None => {
                seen.push(name.to_string());
                lut(name, false)
            }
        };
        assert_eq!(ours.table.as_flattened(), &expected[..], "{name}");
    }
    // Every built-in table was compared, under ImageJ's own name.
    let ours: Vec<String> = builtin_luts().iter().map(|l| l.name.to_string()).collect();
    assert_eq!(seen, ours);
    assert_eq!(text.lines().count(), 14);
}

/// The one-row images of `31_display.ijm`.
fn display_image(kind: &str) -> Image {
    match kind {
        "u8" => Image::from_vec(256, 1, (0..=255u8).collect()).unwrap(),
        "u16" => {
            let samples: Vec<u16> = (0..512u32)
                .map(|x| (500 + (x * 37) % 4000) as u16)
                .collect();
            Image::from_vec(512, 1, samples).unwrap()
        }
        "f32" | "f32s" => {
            let mut samples: Vec<f32> = (0..256)
                .map(|x| ((x as f64 - 100.25) * 0.375) as f32)
                .collect();
            if kind == "f32s" {
                samples[3] = f32::NAN;
                samples[200] = f32::INFINITY;
                samples[10] = f32::NEG_INFINITY;
            }
            Image::from_vec(256, 1, samples).unwrap()
        }
        "f32t" => {
            let samples: Vec<f32> = (0..256).map(|x| (0.1 + x as f64 * 2e-9) as f32).collect();
            Image::from_vec(256, 1, samples).unwrap()
        }
        "rgb" | "rgb4" => {
            let mut meta = ImageMeta::xyczt(DType::U8, 256, 1, 3, 1, 1).unwrap();
            meta.color = ColorModel::Rgb;
            let planes = vec![
                Buffer::from_vec((0..=255u8).collect()),
                Buffer::from_vec((0..=255u8).rev().collect()),
                Buffer::from_vec((0..256u32).map(|x| ((x * 7) % 256) as u8).collect()),
            ];
            Image::from_planes(meta, planes).unwrap()
        }
        other => panic!("unknown image kind {other:?}"),
    }
}

#[test]
fn displayed_levels_match_imagej() {
    let text = fixture("display_levels.csv");
    let mut cases = 0;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').collect();
        let (kind, lut_name, inverted) = (fields[0], fields[1], fields[2] == "1");
        let label = fields[..5].join(",");
        let range =
            (fields[3] != "auto").then(|| (fields[3].parse().unwrap(), fields[4].parse().unwrap()));
        let expected: Vec<u32> = numbers(&fields[5..]);

        let image = display_image(kind);
        let table = lut(lut_name, inverted);
        let channels = match kind {
            "rgb" => vec![channel(None, range); 3],
            // setMinAndMax(min, max, 4) changes the red channel only.
            "rgb4" => vec![
                channel(None, range),
                channel(None, None),
                channel(None, None),
            ],
            _ => vec![channel(Some(table.clone()), range)],
        };
        let image = with_channels(image, channels);
        let position = vec![0; image.meta().plane_shape().len()];
        let picture = render_plane_with(&image, &position, &DisplayOptions::default()).unwrap();
        assert_eq!(packed(&picture), expected, "{label}");

        // The same through the documented pieces: range, level, table.
        if !kind.starts_with("rgb") {
            let range = imager_render::effective_display_range(&image, 0).unwrap();
            let map = LevelMap::new(image.meta().dtype(), false, range).unwrap();
            let plane = image.read_plane(0).unwrap();
            let by_hand: Vec<u32> = (0..plane.len())
                .map(|i| table.table[map.level(plane.get_f64(i).unwrap()) as usize])
                .map(|[r, g, b]| (r as u32) << 16 | (g as u32) << 8 | b as u32)
                .collect();
            assert_eq!(by_hand, expected, "{label} via LevelMap");
        }
        cases += 1;
    }
    assert_eq!(cases, 55);
}

/// The 16x8, three-channel images of `32_composite.ijm`.
fn composite_image(kind: &str) -> Image {
    let dtype = if kind == "8-bit" {
        DType::U8
    } else {
        DType::U16
    };
    let mut meta = ImageMeta::xyczt(dtype, 16, 8, 3, 1, 1).unwrap();
    meta.color = ColorModel::Composite;
    let planes = (1..=3u32)
        .map(|c| {
            let at = (0..8u32).flat_map(|y| (0..16u32).map(move |x| (x, y)));
            if kind == "8-bit" {
                Buffer::from_vec(
                    at.map(|(x, y)| ((x * 16 + y * c * 5 + c * 40) % 256) as u8)
                        .collect(),
                )
            } else {
                Buffer::from_vec(
                    at.map(|(x, y)| ((x * 4099 + y * 523 * c + c * 7001) % 65536) as u16)
                        .collect(),
                )
            }
        })
        .collect();
    Image::from_planes(meta, planes).unwrap()
}

#[test]
fn composites_match_imagej() {
    let text = fixture("composite.csv");
    let mut cases = 0;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').collect();
        let (kind, case) = (fields[0], fields[1]);
        let expected: Vec<u32> = numbers(&fields[2..]);

        let custom_luts = case != "default" && case != "ranges";
        let luts = if custom_luts {
            ["Fire", "Cyan", "Yellow"].map(|name| Some(lut(name, false)))
        } else {
            // ImageJ's defaults: red, green, blue.
            [None, None, None]
        };
        let ranges = match (kind, case) {
            ("16-bit", _) => [(0.0, 65535.0), (5000.0, 40000.0), (20000.0, 20255.0)].map(Some),
            (_, "ranges") => [(20.0, 200.0), (0.0, 100.0), (100.4, 180.6)].map(Some),
            _ => [None; 3],
        };
        let channels = luts
            .into_iter()
            .zip(ranges)
            .map(|(lut, range)| channel(lut, range))
            .collect();
        let image = with_channels(composite_image(kind), channels);

        let mut options = DisplayOptions::default();
        let mut position = [0];
        match case {
            "color2" => (options.mode, position) = (ChannelMode::Color, [1]),
            "gray3" => (options.mode, position) = (ChannelMode::Grayscale, [2]),
            "max" => options.blend = Blend::Max,
            "min" => options.blend = Blend::Min,
            _ => {}
        }
        if let Some(active) = case.strip_prefix("active") {
            options.visible = active.chars().map(|c| c == '1').collect();
        }
        let ours = packed(&render_plane_with(&image, &position, &options).unwrap());
        // When the first channel is hidden, ImageJ clears its buffer from the
        // second pixel on (`for (int i=1; ...)` in CompositeImage.updateImage)
        // and the first pixel keeps whatever was drawn before. imager shows
        // the correct colour there.
        let skip = usize::from(!options.is_visible(0));
        assert_eq!(ours[skip..], expected[skip..], "{kind} {case}");
        cases += 1;
    }
    assert_eq!(cases, 22);
}

/// The 64x48 images of `33_histogram.ijm` and `34_contrast.ijm`.
fn pattern_image(kind: &str, slices: usize) -> Image {
    fn planes<T: Pixel>(slices: usize, f: impl Fn(f64, f64) -> T) -> Vec<Vec<T>> {
        (0..slices)
            .map(|z| {
                (0..48)
                    .flat_map(|y| (0..64).map(move |x| (x, y)))
                    .map(|(x, y)| f((x + 7 * z) as f64, (y + 3 * z) as f64))
                    .collect()
            })
            .collect()
    }
    fn stack<T: Pixel>(planes: Vec<Vec<T>>) -> Image {
        let meta = ImageMeta::xyczt(T::DTYPE, 64, 48, 1, planes.len(), 1).unwrap();
        Image::from_planes(meta, planes.into_iter().map(Buffer::from_vec).collect()).unwrap()
    }
    match kind {
        "u8" => stack(planes(slices, |x, y| {
            ((x * x + 3.0 * y * y + x * y) % 256.0) as u8
        })),
        "u16" => stack(planes(slices, |x, y| {
            let check = ((x / 8.0).floor() + (y / 8.0).floor()) % 2.0;
            ((x * 257.0 + y * y * 31.0 + 1000.0 * check) % 65536.0) as u16
        })),
        "u16skew" => stack(planes(slices, |x, y| {
            (100.0 + (((x * y) % 97.0).powi(2) / 3.0).floor()) as u16
        })),
        "f32" | "f32s" => {
            let mut planes = planes(slices, |x, y| {
                ((x - 31.5) * 0.25 + (y * y) / 64.0 - 3.5 * ((x + y) % 5.0)) as f32
            });
            if kind == "f32s" {
                for plane in &mut planes {
                    plane[3] = f32::NAN;
                    plane[7 * 64 + 5] = f32::NAN;
                    plane[64 + 20] = f32::INFINITY;
                    plane[2 * 64 + 10] = f32::NEG_INFINITY;
                }
            }
            stack(planes)
        }
        "rgb" => {
            let component = |f: fn(u32, u32) -> u32| {
                planes(slices, move |x, y| (f(x as u32, y as u32) & 255) as u8)
            };
            let (r, g, b) = (
                component(|x, _| x * 4),
                component(|_, y| y * 5),
                component(|x, y| (x + y) * 3),
            );
            let mut meta = ImageMeta::xyczt(DType::U8, 64, 48, 3, slices, 1).unwrap();
            meta.color = ColorModel::Rgb;
            let planes = (0..slices)
                .flat_map(|z| [&r[z], &g[z], &b[z]])
                .map(|plane| Buffer::from_vec(plane.clone()))
                .collect();
            Image::from_planes(meta, planes).unwrap()
        }
        other => panic!("unknown image kind {other:?}"),
    }
}

fn first_plane(image: &Image) -> Vec<usize> {
    vec![0; image.meta().plane_shape().len()]
}

/// Compares bin positions with the `values` array of the macro function
/// `getHistogram`, which starts at `histMin` and advances by 1 for plain
/// 8-bit histograms and by `(histMax - histMin) / nBins` otherwise.
fn check_bin_values(ours: &Histogram, values: &[f64], unit_steps: bool, label: &str) {
    assert_eq!(values.len(), ours.counts.len(), "{label}");
    let step = if unit_steps {
        1.0
    } else {
        (ours.hist_max - ours.hist_min) / ours.counts.len() as f64
    };
    for (i, &value) in values.iter().enumerate() {
        let position = ours.hist_min + i as f64 * step;
        assert!(
            close(position, value),
            "{label} bin {i}: {position} vs {value}"
        );
    }
}

#[test]
fn histograms_and_statistics_match_imagej() {
    let text = fixture("histograms.csv");
    let mut records: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').collect();
        let key = (fields[0].to_string(), fields[1].to_string());
        assert!(records.insert(key, numbers(&fields[2..])).is_none());
    }
    let mut checked = 0;
    for ((kind, key), expected) in &records {
        let label = format!("{kind} {key}");
        let plane = pattern_image(kind, 1);
        let position = first_plane(&plane);
        let counts_of = |h: &Histogram| h.counts.iter().map(|&c| c as f64).collect::<Vec<_>>();
        match key.as_str() {
            "stats" | "stackstats" => {
                let ours = if key == "stats" {
                    plane_histogram(&plane, &position, &Default::default()).unwrap()
                } else {
                    stack_histogram(&pattern_image(kind, 3), &Default::default()).unwrap()
                };
                assert_eq!(ours.count as f64, expected[0], "{label} count");
                let mut pairs = vec![
                    ("mean", ours.mean, expected[1]),
                    ("min", ours.min, expected[2]),
                    ("max", ours.max, expected[3]),
                    ("standard deviation", ours.std_dev, expected[4]),
                ];
                if key == "stats" {
                    pairs.push(("mode", ours.mode, expected[5]));
                }
                for (name, ours, imagej) in pairs {
                    assert!(close(ours, imagej), "{label} {name}: {ours} vs {imagej}");
                }
            }
            "stackcounts" => {
                let ours = stack_histogram(&pattern_image(kind, 3), &Default::default()).unwrap();
                assert_eq!(&counts_of(&ours), expected, "{label}");
                assert_eq!(ours.counts.iter().sum::<u64>(), ours.count, "{label}");
            }
            "raw" => {
                let ours = value_counts(&plane.read_plane(0).unwrap()).unwrap();
                let flat: Vec<f64> = ours.iter().flat_map(|&(v, n)| [v, n as f64]).collect();
                assert_eq!(&flat, expected, "{label}");
            }
            _ => {
                // counts<N>, values<N>, counts<N>_<a>_<b>, values<N>_<a>_<b>
                let (what, spec) = key.split_at(6);
                let parts: Vec<&str> = spec.split('_').collect();
                let options = HistogramOptions {
                    bins: parts[0].parse().unwrap(),
                    range: (parts.len() == 3)
                        .then(|| (parts[1].parse().unwrap(), parts[2].parse().unwrap())),
                };
                let ours = plane_histogram(&plane, &position, &options).unwrap();
                match what {
                    "counts" => assert_eq!(&counts_of(&ours), expected, "{label}"),
                    "values" => {
                        let unit_steps = (kind == "u8" || kind == "rgb") && options.bins == 256;
                        check_bin_values(&ours, expected, unit_steps, &label);
                    }
                    other => panic!("unknown record {other:?}"),
                }
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 56);
}

#[test]
fn enhance_contrast_matches_imagej() {
    let text = fixture("contrast.csv");
    let mut cases = 0;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').collect();
        let (kind, scope) = (fields[0], fields[1]);
        let numbers: Vec<f64> = numbers(&fields[2..]);
        let (saturated, imagej) = (numbers[0], (numbers[1], numbers[2]));

        let stack = scope == "stack";
        let image = pattern_image(kind, if stack { 3 } else { 1 });
        let position = first_plane(&image);
        // Where ImageJ leaves the display range alone it is still the
        // default one, since the fixture images are new.
        let ours = match enhance_contrast(&image, &position, saturated, stack).unwrap() {
            Some(range) => range,
            None => reset_range(&image, &position).unwrap(),
        };
        assert!(
            close(ours.0, imagej.0) && close(ours.1, imagej.1),
            "{line}: {ours:?}"
        );
        cases += 1;
    }
    assert_eq!(cases, 35);
}

/// The B&C "Auto" button cannot be pressed from a batch macro, so its
/// reference is not in `fixtures/imagej/out`: `tests/data/bc_auto.csv` was
/// written by `tests/data/BcAuto.java`, which calls ImageJ's
/// `ContrastAdjuster.autoAdjust` directly, twelve presses per image.
#[test]
fn auto_contrast_matches_the_bc_auto_button() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/bc_auto.csv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut click = 0;
    let mut presses = 0;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').collect();
        let (kind, press, how) = (fields[0], fields[1], fields[3]);
        if press == "1" {
            click = 0;
        }
        click += 1;
        let threshold: u64 = fields[2].parse().unwrap();
        assert_eq!(imager_render::auto_threshold(click), threshold, "{line}");

        let image = pattern_image(kind, 1);
        let ours = imager_render::auto_contrast(&image, &first_plane(&image), click).unwrap();
        assert_eq!(ours.reset, how == "reset", "{line}");
        // ImageJ reports the range as its processor stores it: rounded to
        // whole numbers for 16-bit images and to single precision for float.
        let stored = match image.meta().dtype() {
            DType::U16 => ((ours.range.0 + 0.5).floor(), (ours.range.1 + 0.5).floor()),
            _ => ours.range,
        };
        let imagej: (f64, f64) = (fields[4].parse().unwrap(), fields[5].parse().unwrap());
        assert!(
            close(stored.0, imagej.0) && close(stored.1, imagej.1),
            "{line}: {ours:?}"
        );
        // A press that falls back to Reset starts the count again.
        if ours.reset {
            click = 0;
        }
        presses += 1;
    }
    assert_eq!(presses, 72);
}
