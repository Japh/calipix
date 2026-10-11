//! PNG compared with ImageJ 1.x (`fixtures/imagej/gen/22_png.ijm`): the
//! hand-built files of `png_craft.rs` must open to what ImageJ makes of
//! them, and images must be saved to the files ImageJ saves.

#[path = "png_craft/mod.rs"]
mod craft;

use std::path::PathBuf;

use craft::{Craft, crafted, input_dir};
use imager_core::{AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut};
use imager_io::{Error, open, save};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("imager-io-png-imagej-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn text(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Sample values of an ImageJ "Text Image": tab-separated rows.
fn text_image(name: &str) -> Vec<u16> {
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

/// An image as ImageJ described it: a line of `png_info.csv` (what it made
/// of an input) or `pngw_info.csv` (what it saved as PNG), which the files
/// of sample values and lookup tables are found from.
struct Seen {
    /// `png` or `pngw`.
    prefix: &'static str,
    name: String,
    bit_depth: u32,
    width: usize,
    height: usize,
    channels: usize,
    slices: usize,
    composite: bool,
    inverting_lut: bool,
}

impl Seen {
    /// The values of slice `s`, counted from 1: of the red, green and blue
    /// parts for an RGB result.
    fn slice(&self, s: usize) -> Vec<u16> {
        let values = text_image(&format!("{}_{}_{s}.txt", self.prefix, self.name));
        assert_eq!(values.len(), self.width * self.height, "{}", self.name);
        values
    }

    /// The lookup table of slice `s`, counted from 1. The macro writes no
    /// file for the 256-level grey ramp.
    fn lut(&self, s: usize) -> Vec<[u8; 3]> {
        let name = format!("{}_{}_lut{s}.csv", self.prefix, self.name);
        if fixture(&name).exists() {
            lut_csv(&name)
        } else {
            Lut::grays().table.to_vec()
        }
    }
}

fn described(prefix: &'static str) -> Vec<Seen> {
    text(&format!("{prefix}_info.csv"))
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            let number = |i: usize| fields[i].parse::<usize>().unwrap();
            assert_eq!(number(6), 1, "{line}");
            Seen {
                prefix,
                name: fields[0].to_string(),
                bit_depth: number(1) as u32,
                width: number(2),
                height: number(3),
                channels: number(4),
                slices: number(5),
                composite: number(7) == 1,
                inverting_lut: number(8) == 1,
            }
        })
        .collect()
}

/// What ImageJ made of each input. None is a stack.
fn seen() -> Vec<Seen> {
    let all = described("png");
    assert!(all.iter().all(|s| s.slices == 1));
    all
}

fn seen_for(name: &str) -> Seen {
    seen().into_iter().find(|s| s.name == name).unwrap()
}

fn craft_for(name: &str) -> Craft {
    crafted().into_iter().find(|c| c.name == name).unwrap()
}

fn ours(name: &str) -> Image {
    open(input_dir().join(format!("{name}.png"))).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The samples of a plane, whatever its type.
fn plane(image: &Image, index: usize) -> Vec<u16> {
    let buffer = image.read_plane(index).unwrap();
    match image.meta().dtype() {
        DType::U8 => buffer
            .try_slice::<u8>()
            .unwrap()
            .iter()
            .map(|&v| u16::from(v))
            .collect(),
        _ => buffer.try_slice::<u16>().unwrap().to_vec(),
    }
}

fn lut_of(image: &Image) -> Option<Lut> {
    image.meta().channels.first().and_then(|c| c.lut.clone())
}

/// Checks that an image of ours has the type and shape of what ImageJ made
/// of the same file, apart from the three channels of a 16-bit RGB file
/// being marked as RGB where ImageJ has a composite.
fn assert_same_kind(image: &Image, seen: &Seen) {
    let name = &seen.name;
    let meta = image.meta();
    assert_eq!(
        (meta.width(), meta.height()),
        (seen.width, seen.height),
        "{name}"
    );
    assert_eq!(meta.size(&AxisKind::Z), 1, "{name}");
    assert_eq!(meta.size(&AxisKind::Time), 1, "{name}");
    assert_eq!(meta.name, format!("{name}.png"));
    let (dtype, channels, color) = match (seen.bit_depth, seen.channels) {
        (8, 1) => (DType::U8, 1, ColorModel::Grayscale),
        (16, 1) => (DType::U16, 1, ColorModel::Grayscale),
        (16, 3) => (DType::U16, 3, ColorModel::Rgb),
        (24, 1) => (DType::U8, 3, ColorModel::Rgb),
        other => panic!("{name}: unexpected ImageJ type {other:?}"),
    };
    assert_eq!(meta.dtype(), dtype, "{name}");
    assert_eq!(meta.size(&AxisKind::Channel), channels, "{name}");
    assert_eq!(meta.plane_count(), channels, "{name}");
    assert_eq!(meta.color, color, "{name}");
}

/// Inputs that imager reads differently from ImageJ on purpose; see
/// `gray_with_transparency_is_not_brightened`.
const GRAY_WITH_TRANSPARENCY: [&str; 8] = [
    "graya8",
    "graya16",
    "gray8_trns",
    "gray16_trns",
    "gray4_trns_zero",
    "gray4_trns",
    "blend_graya8",
    "blend_graya16",
];

#[test]
fn imagej_saw_every_input() {
    let built: Vec<&str> = crafted().iter().map(|c| c.name).collect();
    let seen: Vec<String> = seen().into_iter().map(|s| s.name).collect();
    assert_eq!(seen, built);
}

/// Every input but the grey ones with transparency opens to the type,
/// shape, sample values and lookup table that ImageJ gives it.
#[test]
fn reads_what_imagej_reads() {
    let mut checked = 0;
    for seen in seen() {
        let name = seen.name.as_str();
        if GRAY_WITH_TRANSPARENCY.contains(&name) {
            continue;
        }
        let image = ours(name);
        assert_same_kind(&image, &seen);
        for p in 0..image.meta().plane_count() {
            assert_eq!(plane(&image, p), seen.slice(p + 1), "{name}, plane {p}");
        }
        match (seen.bit_depth, seen.channels) {
            (8, _) => {
                // ImageJ's table has as many entries as the file's depth
                // allows; ours always has 256, black where ImageJ has none.
                let theirs = seen.lut(1);
                let table = *lut_of(&image).unwrap_or_else(Lut::grays).table;
                assert_eq!(&table[..theirs.len()], &theirs[..], "{name}");
                assert!(table[theirs.len()..].iter().all(|c| *c == [0; 3]), "{name}");
                // The default grey ramp is the absence of a lookup table.
                let is_ramp = table == *Lut::grays().table;
                assert_eq!(lut_of(&image).is_none(), is_ramp, "{name}");
                let is_inverted = table == *Lut::grays().inverted().table;
                assert_eq!(seen.inverting_lut, is_inverted, "{name}");
            }
            (16, 3) => {
                // ImageJ: a composite with red, green and blue tables.
                assert!(seen.composite, "{name}");
                for c in 0..3 {
                    let expected: Vec<[u8; 3]> = (0..=255u8)
                        .map(|i| {
                            let mut entry = [0; 3];
                            entry[c] = i;
                            entry
                        })
                        .collect();
                    assert_eq!(seen.lut(c + 1), expected, "{name}, channel {c}");
                }
                assert!(lut_of(&image).is_none(), "{name}");
            }
            _ => assert!(lut_of(&image).is_none(), "{name}"),
        }
        assert!(!seen.composite || seen.channels == 3, "{name}");
        checked += 1;
    }
    assert_eq!(checked, 36);
}

/// The inputs give ImageJ each of its image types, so the comparison above
/// is not of one kind only.
#[test]
fn inputs_cover_every_result_type() {
    let all = seen();
    let count = |depth: u32, channels: usize| {
        all.iter()
            .filter(|s| s.bit_depth == depth && s.channels == channels)
            .count()
    };
    assert_eq!(count(8, 1), 19);
    assert_eq!(count(16, 1), 2);
    assert_eq!(count(16, 3), 1);
    assert_eq!(count(24, 1), 22);
    assert_eq!(all.len(), 44);
}

/// Java's conversion of a linear grey level to sRGB, both as fractions of
/// full scale (`ColorModel.getLinearRGB8TosRGB8LUT` and its 16-bit
/// counterpart), giving an 8-bit level.
fn java_linear_to_srgb(linear: f64) -> u16 {
    let srgb = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round() as u16
}

/// `(alpha * level + (255 - alpha) * 255) / 255`, to the nearest.
fn over_white(level: u16, alpha: u16) -> u16 {
    (alpha * level + (255 - alpha) * 255 + 127) / 255
}

/// `sample * 255 / max`, to the nearest.
fn to_8_bits(sample: u16, max: u32) -> u16 {
    ((u32::from(sample) * 255 + max / 2) / max) as u16
}

/// Grey files with transparency are the one place where imager's samples
/// differ from ImageJ's. Both blend over white into an RGB image, but
/// ImageJ (that is, Java) first brightens the grey levels as if they were
/// linear light; and for fewer than 8 bits it misses any transparent colour
/// but 0. This pins down both sides: ImageJ's values are exactly that, and
/// ours are exactly the same blend of the stored levels.
#[test]
fn gray_with_transparency_is_not_brightened() {
    let mut brightened = 0;
    for name in GRAY_WITH_TRANSPARENCY {
        let (craft, seen) = (craft_for(name), seen_for(name));
        let image = ours(name);
        assert_same_kind(&image, &seen);
        assert_eq!(seen.bit_depth, 24, "{name}");

        let max = (1u32 << craft.depth) - 1;
        let has_alpha = craft.color == craft::GRAY_ALPHA;
        let key = (!has_alpha).then(|| u16::from_be_bytes([craft.trns[0], craft.trns[1]]));
        let (mut java, mut plain) = (Vec::new(), Vec::new());
        for y in 0..craft.height {
            for x in 0..craft.width {
                let gray = craft.at(x, y, 0);
                let alpha = if has_alpha {
                    to_8_bits(craft.at(x, y, 1), max)
                } else {
                    255
                };
                let java_level = java_linear_to_srgb(f64::from(gray) / f64::from(max));
                // Java compares a transparent colour of fewer than 8 bits
                // with something other than the stored sample: only 0 ever
                // matches.
                let java_keyed = key == Some(gray) && (craft.depth >= 8 || gray == 0);
                java.push(if java_keyed {
                    255
                } else {
                    over_white(java_level, alpha)
                });
                plain.push(if key == Some(gray) {
                    255
                } else {
                    over_white(to_8_bits(gray, max), alpha)
                });
            }
        }
        for c in 0..3 {
            assert_eq!(seen.slice(c + 1), java, "{name}: ImageJ, channel {c}");
            assert_eq!(plane(&image, c), plain, "{name}: imager, channel {c}");
        }
        brightened += java.iter().zip(&plain).filter(|(j, p)| j != p).count();
    }
    // Of the 1554 pixels, the ones that ImageJ shows lighter than stored.
    assert_eq!(brightened, 1486);
}

/// The transparent colour of `gray4_trns` is level 8, which Java does not
/// find: ImageJ leaves those pixels grey where imager makes them white.
#[test]
fn transparent_color_below_8_bits_is_honoured() {
    let (craft, seen) = (craft_for("gray4_trns"), seen_for("gray4_trns"));
    let (image, theirs) = (ours("gray4_trns"), seen.slice(1));
    let ours = plane(&image, 0);
    let mut keyed = 0;
    for y in 0..craft.height {
        for x in 0..craft.width {
            if craft.at(x, y, 0) == 8 {
                let i = y * craft.width + x;
                assert_eq!((theirs[i], ours[i]), (193, 255));
                keyed += 1;
            }
        }
    }
    assert_eq!(keyed, 4);
}

/// The larger inputs put the rounding of the blend to the test: the number
/// of distinct pairs of level and alpha they hold, all of which come out as
/// in ImageJ (`reads_what_imagej_reads`).
#[test]
fn blend_inputs_cover_many_combinations() {
    for (name, expected) in [("blend_rgba8", 5208), ("blend_rgba16", 1385)] {
        let craft = craft_for(name);
        let max = (1u32 << craft.depth) - 1;
        let mut pairs = std::collections::BTreeSet::new();
        for y in 0..craft.height {
            for x in 0..craft.width {
                let alpha = to_8_bits(craft.at(x, y, 3), max);
                for c in 0..3 {
                    pairs.insert((to_8_bits(craft.at(x, y, c), max), alpha));
                }
            }
        }
        assert_eq!(pairs.len(), expected, "{name}");
    }
}

// Writing

/// A PNG file's header, palette and image data as stored.
#[derive(Debug, PartialEq)]
struct Stored {
    width: u32,
    height: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    palette: Option<Vec<u8>>,
    transparency: Option<Vec<u8>>,
    data: Vec<u8>,
}

fn stored(path: PathBuf) -> Stored {
    let file = std::io::BufReader::new(std::fs::File::open(&path).unwrap());
    let mut reader = png::Decoder::new(file).read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut data).unwrap();
    data.truncate(frame.buffer_size());
    let info = reader.info();
    Stored {
        width: info.width,
        height: info.height,
        color: info.color_type,
        depth: info.bit_depth,
        palette: info.palette.as_ref().map(|p| p.to_vec()),
        transparency: info.trns.as_ref().map(|t| t.to_vec()),
        data,
    }
}

/// The image ImageJ saved as `pngw_<name>.png`, rebuilt from ImageJ's
/// description of it.
fn source(name: &str) -> Image {
    let seen = described("pngw")
        .into_iter()
        .find(|s| s.name == name)
        .unwrap();
    let (width, height) = (seen.width, seen.height);
    if seen.bit_depth == 24 {
        let mut meta = ImageMeta::xyczt(DType::U8, width, height, 3, 1, 1).unwrap();
        meta.color = ColorModel::Rgb;
        let planes = (1..=3)
            .map(|s| Buffer::from_vec(seen.slice(s).iter().map(|&v| v as u8).collect::<Vec<_>>()))
            .collect();
        return Image::from_planes(meta, planes).unwrap();
    }
    let dtype = if seen.bit_depth == 8 {
        DType::U8
    } else {
        DType::U16
    };
    let mut meta = ImageMeta::xyczt(dtype, width, height, seen.channels, seen.slices, 1).unwrap();
    if seen.composite {
        meta.color = ColorModel::Composite;
    }
    // The display ranges the macro sets.
    let display_range = match name {
        "range8" => Some((50.0, 150.0)),
        "range16" | "fire16" => Some((1000.0, 20000.0)),
        _ => None,
    };
    meta.channels = (1..=seen.channels)
        .map(|c| {
            let table = seen.lut(c);
            let ramp = table == Lut::grays().table.to_vec();
            ChannelMeta {
                lut: (!ramp).then(|| Lut::from_fn("ImageJ", |i| table[usize::from(i)])),
                display_range,
                ..ChannelMeta::default()
            }
        })
        .collect();
    let planes = (1..=seen.channels * seen.slices)
        .map(|s| match dtype {
            DType::U8 => {
                Buffer::from_vec(seen.slice(s).iter().map(|&v| v as u8).collect::<Vec<_>>())
            }
            _ => Buffer::from_vec(seen.slice(s)),
        })
        .collect();
    Image::from_planes(meta, planes).unwrap()
}

/// The image ImageJ saved as `pngw_<name>.png`, and that image saved as PNG
/// by us.
///
/// Tests run side by side, so each names its own files with `test`.
fn saved_by_us(name: &str, test: &str) -> (Image, imager_io::Result<PathBuf>) {
    let image = source(name);
    let path = temp(&format!("{test}-{name}.png"));
    let result = save(&image, &path).map(|()| path);
    (image, result)
}

fn saved_by_imagej(name: &str) -> Stored {
    stored(fixture(&format!("pngw_{name}.png")))
}

/// 8-bit, 8-bit with a colour or inverting lookup table, 16-bit and RGB
/// images make the same file as in ImageJ: the same layout, palette and
/// sample values.
#[test]
fn writes_what_imagej_writes() {
    use png::{BitDepth::*, ColorType::*};
    for (name, color, depth) in [
        ("gray8", Grayscale, Eight),
        ("fire8", Indexed, Eight),
        ("inv8", Indexed, Eight),
        ("gray16", Grayscale, Sixteen),
        ("range16", Grayscale, Sixteen),
        ("rgb", Rgb, Eight),
    ] {
        let (image, path) = saved_by_us(name, "writes");
        let (ours, theirs) = (stored(path.unwrap()), saved_by_imagej(name));
        assert_eq!((theirs.color, theirs.depth), (color, depth), "{name}");
        assert_eq!(ours, theirs, "{name}");
        assert_eq!(ours.transparency, None, "{name}");

        // The palette is the image's lookup table, all 256 entries of it.
        let lut = lut_of(&image).map(|lut| lut.table.as_flattened().to_vec());
        assert_eq!(ours.palette, lut, "{name}");
        assert_eq!(lut.is_some(), color == Indexed, "{name}");
    }
    let inverted = saved_by_imagej("inv8").palette.unwrap();
    assert_eq!(inverted[..6], [255, 255, 255, 254, 254, 254]);
}

/// Our files and ImageJ's open to the image that was saved.
#[test]
fn written_files_open_to_the_saved_image() {
    for name in ["gray8", "fire8", "inv8", "gray16", "range16", "rgb"] {
        let (image, path) = saved_by_us(name, "opens");
        for (whose, path) in [
            ("ours", path.unwrap()),
            ("ImageJ's", fixture(&format!("pngw_{name}.png"))),
        ] {
            let back = open(path).unwrap();
            assert_eq!(back.meta().dtype(), image.meta().dtype(), "{name}, {whose}");
            assert_eq!(back.meta().shape(), image.meta().shape(), "{name}, {whose}");
            assert_eq!(back.meta().color, image.meta().color, "{name}, {whose}");
            for p in 0..image.meta().plane_count() {
                assert_eq!(plane(&back, p), plane(&image, p), "{name}, {whose}");
            }
            assert_eq!(
                lut_of(&back).map(|lut| lut.table),
                lut_of(&image).map(|lut| lut.table),
                "{name}, {whose}"
            );
        }
    }
}

/// Where ImageJ saves what is on screen instead of the samples, imager
/// saves the samples or refuses.
#[test]
fn saves_samples_where_imagej_saves_the_rendering() {
    use png::{BitDepth::*, ColorType::*};

    // An 8-bit image with a display range: ImageJ stretches the samples.
    let (image, path) = saved_by_us("range8", "samples");
    let (ours, theirs) = (stored(path.unwrap()), saved_by_imagej("range8"));
    assert_eq!((ours.color, ours.depth), (Grayscale, Eight));
    assert_eq!((theirs.color, theirs.depth), (Grayscale, Eight));
    let samples: Vec<u8> = plane(&image, 0).iter().map(|&v| v as u8).collect();
    assert_eq!(ours.data, samples);
    assert_eq!(ours.data, saved_by_imagej("gray8").data);
    assert_ne!(theirs.data, samples);
    assert!(theirs.data.iter().zip(&samples).all(|(&t, &s)| match s {
        0..=50 => t == 0,
        150..=255 => t == 255,
        _ => t > 0 && t < 255,
    }));

    // A 16-bit image with a lookup table: ImageJ saves an 8-bit palette
    // file; we keep the 16-bit samples and drop the table.
    let (image, path) = saved_by_us("fire16", "samples");
    assert!(lut_of(&image).is_some());
    let (ours, theirs) = (stored(path.unwrap()), saved_by_imagej("fire16"));
    assert_eq!((theirs.color, theirs.depth), (Indexed, Eight));
    assert_eq!(ours, saved_by_imagej("gray16"));

    // A stack: ImageJ saves the slice on screen, the second of three.
    let (image, result) = saved_by_us("stack8", "samples");
    let theirs = saved_by_imagej("stack8");
    assert_eq!((theirs.color, theirs.depth), (Grayscale, Eight));
    let second: Vec<u8> = plane(&image, 1).iter().map(|&v| v as u8).collect();
    assert_eq!(theirs.data, second);
    let message = result.unwrap_err().to_string();
    assert!(message.contains("this image has 3"), "{message}");
    assert!(message.contains("TIFF"), "{message}");

    // A 16-bit composite and a float image: ImageJ saves 8-bit renderings.
    let (_, result) = saved_by_us("comp16", "samples");
    assert_eq!(saved_by_imagej("comp16").color, Rgb);
    assert_eq!(saved_by_imagej("comp16").depth, Eight);
    let message = result.unwrap_err().to_string();
    assert!(message.contains("3 separate channels"), "{message}");
    assert!(message.contains("RGB"), "{message}");

    let float = Image::from_vec(2, 1, vec![0.5f32, -0.25]).unwrap();
    let result = save(&float, temp("samples-float.png"));
    assert_eq!(saved_by_imagej("float").color, Grayscale);
    assert_eq!(saved_by_imagej("float").depth, Eight);
    let message = result.unwrap_err().to_string();
    assert!(message.contains("f32"), "{message}");
    assert!(message.contains("convert"), "{message}");
}

/// A 16-bit RGB file is read as RGB and so written back as one, sample for
/// sample; ImageJ would save its 8-bit rendering.
#[test]
fn rgb16_round_trips() {
    let image = ours("rgb16");
    let path = temp("rgb16-again.png");
    save(&image, &path).unwrap();
    let (ours, original) = (stored(path), stored(input_dir().join("rgb16.png")));
    assert_eq!(ours, original);
}

// Robustness

/// Every truncation of three small files (one interlaced, one with 16-bit
/// samples, one with a palette and transparency) is an error that names the
/// file. So is every damaged byte in a chunk the image cannot do without,
/// since each has a checksum; a damaged transparency chunk is dropped, as
/// libpng drops it, and must only not panic.
#[test]
fn damaged_files_are_errors() {
    let path = temp("damaged.png");
    for name in ["gray16_i", "pal4_trns", "gray1_i"] {
        let bytes = craft_for(name).encode();
        // The last twelve bytes are the end marker, which the decoder does
        // not need: damage there must only not panic.
        let needed = bytes.len() - 12;
        let all_critical = craft_for(name).trns.is_empty();
        for length in 0..needed {
            std::fs::write(&path, &bytes[..length]).unwrap();
            let Err(error) = open(&path) else {
                panic!("{name} cut to {length} bytes opened")
            };
            assert!(
                matches!(error, Error::Decode { .. }),
                "{name} cut to {length} bytes: {error:?}"
            );
            assert!(error.to_string().contains("damaged.png"), "{error}");
        }
        for position in 0..bytes.len() {
            for flip in [0x01, 0x80] {
                let mut damaged = bytes.clone();
                damaged[position] ^= flip;
                std::fs::write(&path, &damaged).unwrap();
                let result = open(&path);
                assert!(
                    result.is_err() || position >= needed || !all_critical,
                    "{name} with byte {position} damaged opened"
                );
            }
        }
    }
}

/// A header that claims an image too large for memory, or one with no
/// pixels, is an error.
#[test]
fn impossible_sizes_are_errors() {
    let path = temp("impossible.png");
    for (width, height) in [(0x7fff_ffffu32, 0x7fff_ffffu32), (0, 9), (11, 0)] {
        // The size is changed in the header of a small file, and the
        // header's checksum put right.
        let mut bytes = craft_for("rgba16").encode();
        bytes[16..20].copy_from_slice(&width.to_be_bytes());
        bytes[20..24].copy_from_slice(&height.to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.sum().to_be_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let error = open(&path).unwrap_err();
        assert!(
            matches!(error, Error::Decode { .. }),
            "{width} by {height}: {error:?}"
        );
    }
}
