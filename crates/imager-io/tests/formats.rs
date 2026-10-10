//! JPEG, GIF and BMP through the public `open` and `save`: round trips,
//! refusals, detection by content, and behaviour on damaged files.
//! NRRD has its own file, `formats_nrrd.rs`; comparisons with files
//! written by ImageJ are in `formats_imagej.rs`.

use std::path::PathBuf;

use imager_core::{AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut, Pixel};
use imager_io::{Error, open, save};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("imager-io-formats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A greyscale image of `planes` Z slices.
fn gray<T: Pixel>(
    width: usize,
    height: usize,
    planes: usize,
    value: impl Fn(usize, usize, usize) -> T,
) -> Image {
    let meta = ImageMeta::xyczt(T::DTYPE, width, height, 1, planes, 1).unwrap();
    let buffers = (0..planes)
        .map(|p| {
            let data: Vec<T> = (0..width * height)
                .map(|i| value(i % width, i / width, p))
                .collect();
            Buffer::from_vec(data)
        })
        .collect();
    Image::from_planes(meta, buffers).unwrap()
}

/// An RGB image of `frames` Z slices; `value(x, y, channel, frame)`.
fn rgb(
    width: usize,
    height: usize,
    frames: usize,
    value: impl Fn(usize, usize, usize, usize) -> u8,
) -> Image {
    let mut meta = ImageMeta::xyczt(DType::U8, width, height, 3, frames, 1).unwrap();
    meta.color = ColorModel::Rgb;
    let buffers = (0..frames * 3)
        .map(|p| {
            let data: Vec<u8> = (0..width * height)
                .map(|i| value(i % width, i / width, p % 3, p / 3))
                .collect();
            Buffer::from_vec(data)
        })
        .collect();
    Image::from_planes(meta, buffers).unwrap()
}

fn with_lut(image: &Image, lut: Lut) -> Image {
    let mut meta = image.meta().clone();
    meta.channels = vec![ChannelMeta {
        lut: Some(lut),
        ..ChannelMeta::default()
    }];
    image.with_meta(meta).unwrap()
}

fn fire_like() -> Lut {
    Lut::from_fn("test", |i| [i, 255 - i, i.wrapping_mul(7)])
}

fn lut_of(image: &Image) -> Option<Lut> {
    image.meta().channels.first().and_then(|c| c.lut.clone())
}

fn assert_same_pixels(a: &Image, b: &Image, what: &str) {
    assert_eq!(a.meta().dtype(), b.meta().dtype(), "{what}");
    assert_eq!(a.meta().shape(), b.meta().shape(), "{what}");
    assert_eq!(a.meta().color, b.meta().color, "{what}");
    let kinds =
        |i: &Image| -> Vec<AxisKind> { i.meta().axes().iter().map(|a| a.kind.clone()).collect() };
    assert_eq!(kinds(a), kinds(b), "{what}");
    for plane in 0..a.meta().plane_count() {
        assert_eq!(
            a.read_plane(plane).unwrap(),
            b.read_plane(plane).unwrap(),
            "{what}, plane {plane}"
        );
    }
}

fn round_trip(image: &Image, name: &str) -> Image {
    let path = temp(name);
    save(image, &path).unwrap();
    let back = open(&path).unwrap();
    assert_eq!(back.meta().name, name);
    back
}

/// Largest absolute difference between two 8-bit images of the same shape.
fn max_difference(a: &Image, b: &Image) -> u8 {
    assert_eq!(a.meta().shape(), b.meta().shape());
    let mut max = 0;
    for plane in 0..a.meta().plane_count() {
        let (pa, pb) = (a.read_plane(plane).unwrap(), b.read_plane(plane).unwrap());
        let (sa, sb) = (pa.try_slice::<u8>().unwrap(), pb.try_slice::<u8>().unwrap());
        for (x, y) in sa.iter().zip(sb) {
            max = max.max(x.abs_diff(*y));
        }
    }
    max
}

// BMP

#[test]
fn bmp_round_trips_gray_with_every_row_padding() {
    for width in [1, 2, 3, 4, 5, 13] {
        let image = gray(width, 3, 1, |x, y, _| (x * 50 + y * 7) as u8);
        let back = round_trip(&image, &format!("gray_w{width}.bmp"));
        assert_same_pixels(&image, &back, &format!("width {width}"));
        assert!(lut_of(&back).is_none(), "a grey ramp is not a lookup table");
    }
}

#[test]
fn bmp_round_trips_rgb_with_every_row_padding() {
    for width in [1, 2, 3, 4, 5, 13] {
        let image = rgb(width, 3, 1, |x, y, c, _| (x * 40 + y * 9 + c * 80) as u8);
        let back = round_trip(&image, &format!("rgb_w{width}.bmp"));
        assert_same_pixels(&image, &back, &format!("width {width}"));
    }
}

#[test]
fn bmp_keeps_indices_and_lookup_table() {
    let image = with_lut(&gray(7, 5, 1, |x, y, _| (x * 36 + y) as u8), fire_like());
    let back = round_trip(&image, "lut.bmp");
    assert_same_pixels(&image, &back, "indices");
    assert_eq!(lut_of(&back).unwrap().table, fire_like().table);
}

#[test]
fn bmp_refuses_what_it_cannot_hold() {
    let deep = gray(2, 2, 1, |x, _, _| x as u16);
    let err = save(&deep, temp("deep.bmp")).unwrap_err();
    assert!(matches!(err, Error::Unsupported { format: "BMP", .. }));
    assert!(err.to_string().contains("u16"), "{err}");
    assert!(err.to_string().contains("convert"), "{err}");

    let stack = gray(2, 2, 3, |x, _, _| x as u8);
    let err = save(&stack, temp("stack.bmp")).unwrap_err();
    assert!(err.to_string().contains("single plane"), "{err}");

    let meta = ImageMeta::xyczt(DType::U8, 2, 2, 2, 1, 1).unwrap();
    let channels = Image::from_planes(meta, vec![Buffer::zeros(DType::U8, 4); 2]).unwrap();
    let err = save(&channels, temp("channels.bmp")).unwrap_err();
    assert!(err.to_string().contains("2 separate channels"), "{err}");
}

// GIF

#[test]
fn gif_round_trips_gray() {
    let image = gray(31, 17, 1, |x, y, _| (x * 9 + y * 13) as u8);
    let back = round_trip(&image, "gray.gif");
    assert_same_pixels(&image, &back, "gray");
    assert!(lut_of(&back).is_none());

    // Large enough for the LZW code table to fill and reset.
    let noisy = gray(200, 150, 1, |x, y, _| {
        (x.wrapping_mul(2_654_435_761).wrapping_add(y * 40_503) >> 7) as u8
    });
    assert_same_pixels(&noisy, &round_trip(&noisy, "noisy.gif"), "noisy");

    let tiny = gray(1, 1, 1, |_, _, _| 200u8);
    assert_same_pixels(&tiny, &round_trip(&tiny, "tiny.gif"), "tiny");
}

#[test]
fn gif_keeps_indices_and_lookup_table() {
    let image = with_lut(&gray(9, 4, 1, |x, y, _| (x * 28 + y) as u8), fire_like());
    let back = round_trip(&image, "lut.gif");
    assert_same_pixels(&image, &back, "indices");
    assert_eq!(lut_of(&back).unwrap().table, fire_like().table);
}

#[test]
fn gif_round_trips_gray_stack_as_animation() {
    let image = gray(12, 9, 4, |x, y, p| (x * 20 + y * 3 + p * 50) as u8);
    let back = round_trip(&image, "stack.gif");
    assert_same_pixels(&image, &back, "stack");
    assert_eq!(back.meta().size(&AxisKind::Z), 4);
}

#[test]
fn gif_stack_with_color_table_reads_back_as_rgb() {
    // As in ImageJ, the frames of an animation are composited to RGB, so
    // the lookup table is applied rather than kept.
    let indices = gray(6, 5, 2, |x, y, p| (x * 40 + y + p * 3) as u8);
    let back = round_trip(&with_lut(&indices, fire_like()), "lut_stack.gif");
    let table = fire_like().table;
    let expected = rgb(6, 5, 2, |x, y, c, p| table[x * 40 + y + p * 3][c]);
    assert_same_pixels(&expected, &back, "applied table");
}

#[test]
fn gif_round_trips_rgb_with_few_colors() {
    // One image comes back as its indices and palette, as in ImageJ;
    // through the palette it is the original.
    let single = rgb(10, 8, 1, |x, y, c, _| ((x / 2 + y / 2 + c) % 6 * 50) as u8);
    let back = round_trip(&single, "rgb.gif");
    assert_eq!(back.meta().shape(), &[10, 8]);
    let table = lut_of(&back).unwrap().table;
    let indices = back.read_plane(0).unwrap();
    for c in 0..3 {
        let channel = single.read_plane(c).unwrap();
        let through_table: Vec<u8> = indices
            .try_slice::<u8>()
            .unwrap()
            .iter()
            .map(|&i| table[usize::from(i)][c])
            .collect();
        assert_eq!(
            through_table,
            channel.try_slice::<u8>().unwrap(),
            "channel {c}"
        );
    }

    // Several come back composited to RGB.
    let frames = rgb(10, 8, 3, |x, y, c, f| ((x / 2 + y + c + f) % 5 * 60) as u8);
    assert_same_pixels(&frames, &round_trip(&frames, "rgb_stack.gif"), "rgb stack");
}

#[test]
fn gif_single_rgb_in_gray_levels_reads_back_as_indices() {
    // One image is stored as indices plus a palette, and that is what
    // comes back; only the palette says the image was grey.
    let single = rgb(4, 4, 1, |x, y, _, _| (x * 60 + y) as u8);
    let back = round_trip(&single, "gray_rgb.gif");
    assert_eq!(back.meta().color, ColorModel::Grayscale);
    assert_eq!(back.meta().plane_count(), 1);
    let table = lut_of(&back).unwrap().table;
    let indices = back.read_plane(0).unwrap();
    let red = single.read_plane(0).unwrap();
    for (i, v) in indices
        .try_slice::<u8>()
        .unwrap()
        .iter()
        .zip(red.try_slice::<u8>().unwrap())
    {
        assert_eq!(table[*i as usize], [*v; 3]);
    }
}

#[test]
fn gif_refuses_what_it_cannot_hold() {
    let deep = gray(2, 2, 1, |x, _, _| x as f32);
    let err = save(&deep, temp("deep.gif")).unwrap_err();
    assert!(matches!(err, Error::Unsupported { format: "GIF", .. }));
    assert!(err.to_string().contains("f32"), "{err}");

    let colorful = rgb(32, 32, 1, |x, y, c, _| [x * 8, y * 8, x + y][c] as u8);
    let err = save(&colorful, temp("colorful.gif")).unwrap_err();
    assert!(err.to_string().contains("256 colours"), "{err}");
}

// JPEG

/// Smooth content, which JPEG reproduces closely.
fn smooth_gray() -> Image {
    gray(40, 24, 1, |x, y, _| (x * 4 + y * 3) as u8)
}

fn smooth_rgb() -> Image {
    rgb(40, 24, 1, |x, y, c, _| match c {
        0 => (x * 6) as u8,
        1 => (y * 10) as u8,
        _ => (255 - x * 3 - y * 2) as u8,
    })
}

#[test]
fn jpeg_round_trips_gray_within_tolerance() {
    let image = smooth_gray();
    let back = round_trip(&image, "gray.jpg");
    assert_eq!(back.meta().dtype(), DType::U8);
    assert_eq!(back.meta().shape(), &[40, 24]);
    assert_eq!(back.meta().color, ColorModel::Grayscale);
    let max = max_difference(&image, &back);
    println!("JPEG grey round trip: maximum difference {max}");
    assert!(max <= 4, "maximum difference {max}");
}

#[test]
fn jpeg_round_trips_rgb_within_tolerance() {
    let image = smooth_rgb();
    let back = round_trip(&image, "rgb.jpeg");
    assert_eq!(back.meta().dtype(), DType::U8);
    assert_eq!(back.meta().shape(), &[40, 24, 3]);
    assert_eq!(back.meta().color, ColorModel::Rgb);
    let max = max_difference(&image, &back);
    println!("JPEG RGB round trip: maximum difference {max}");
    assert!(max <= 6, "maximum difference {max}");
}

#[test]
fn jpeg_rgb_holding_only_gray_opens_as_8_bit() {
    // Opener.convertGrayJpegTo8Bits.
    let image = rgb(16, 16, 1, |x, y, _, _| (x * 8 + y * 7) as u8);
    let back = round_trip(&image, "gray_rgb.jpg");
    assert_eq!(back.meta().shape(), &[16, 16]);
    assert_eq!(back.meta().color, ColorModel::Grayscale);
}

#[test]
fn jpeg_refuses_what_it_cannot_hold() {
    let deep = gray(2, 2, 1, |x, _, _| x as u16);
    let err = save(&deep, temp("deep.jpg")).unwrap_err();
    assert!(matches!(err, Error::Unsupported { format: "JPEG", .. }));
    assert!(err.to_string().contains("u16"), "{err}");
    assert!(err.to_string().contains("convert"), "{err}");

    let stack = rgb(2, 2, 2, |x, _, _, _| x as u8);
    let err = save(&stack, temp("stack.jpg")).unwrap_err();
    assert!(err.to_string().contains("single plane"), "{err}");
}

// Detection

/// One small valid file per format, with the image it should open as.
fn samples() -> Vec<(&'static str, Image)> {
    vec![
        (
            "bmp",
            rgb(8, 6, 1, |x, y, c, _| (x * 30 + y * 11 + c * 70) as u8),
        ),
        ("gif", gray(8, 6, 1, |x, y, _| (x * 30 + y * 11) as u8)),
        ("jpg", smooth_gray()),
        (
            "nrrd",
            gray(8, 6, 2, |x, y, p| (x * 300 + y * 11 + p) as u16),
        ),
    ]
}

#[test]
fn content_wins_over_extension() {
    for (ext, image) in samples() {
        let path = temp(&format!("detect.{ext}"));
        save(&image, &path).unwrap();
        let expected = open(&path).unwrap();
        // No extension, an unknown one, and every other format's.
        for wrong in ["", "dat", "png", "tif", "bmp", "gif", "jpg", "nrrd"] {
            if wrong == ext {
                continue;
            }
            let renamed = temp(&format!("detect_{ext}_as.{wrong}"));
            std::fs::copy(&path, &renamed).unwrap();
            let opened =
                open(&renamed).unwrap_or_else(|e| panic!("{ext} content named .{wrong}: {e}"));
            assert_same_pixels(&expected, &opened, &format!("{ext} as .{wrong}"));
        }
    }
}

#[test]
fn unknown_content_is_rejected() {
    for (name, content) in [
        ("empty.xyz", &b""[..]),
        ("text.xyz", b"BMW and GIF are not NRRD"),
        ("almost_gif.xyz", b"GIF7"),
        ("almost_jpeg.xyz", b"\xff\xd8\x00\x00"),
        ("almost_nrrd.xyz", b"NRRD is a format"),
        ("zeros.xyz", &[0u8; 64]),
    ] {
        let path = temp(name);
        std::fs::write(&path, content).unwrap();
        assert!(
            matches!(open(&path), Err(Error::UnknownFormat(_))),
            "{name}"
        );
    }
    // With a known extension the named format reports what is wrong.
    for ext in ["bmp", "gif", "jpg", "nrrd"] {
        let path = temp(&format!("garbage.{ext}"));
        std::fs::write(&path, b"this is not an image at all").unwrap();
        let err = open(&path).unwrap_err();
        assert!(matches!(err, Error::Decode { .. }), "{ext}: {err}");
    }
}

// Robustness

/// Opens a file and reads every plane; panics are the failure, errors are
/// expected.
fn try_open(path: &std::path::Path) -> Result<(), String> {
    let image = open(path).map_err(|e| e.to_string())?;
    for plane in 0..image.meta().plane_count().min(64) {
        image.read_plane(plane).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[test]
fn truncated_files_give_errors() {
    for (ext, image) in samples() {
        let path = temp(&format!("whole.{ext}"));
        save(&image, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let cut = temp(&format!("cut.{ext}"));
        let mut failures = 0;
        for len in 0..bytes.len() {
            std::fs::write(&cut, &bytes[..len]).unwrap();
            match try_open(&cut) {
                Err(message) => {
                    assert!(!message.is_empty());
                    failures += 1;
                }
                // Only a decoder that tolerates a missing tail may accept
                // a shortened file: JPEG without its last bytes, or a GIF
                // without its trailer byte.
                Ok(()) => assert!(
                    ext == "jpg" || (ext == "gif" && len == bytes.len() - 1),
                    "{ext} truncated to {len} of {} bytes opened without error",
                    bytes.len()
                ),
            }
        }
        println!("{ext}: {failures} of {} truncations rejected", bytes.len());
        assert!(failures >= bytes.len() / 2, "{ext}");
    }
}

#[test]
fn corrupt_files_do_not_panic() {
    for (ext, image) in samples() {
        let path = temp(&format!("intact.{ext}"));
        save(&image, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let damaged = temp(&format!("damaged.{ext}"));
        // One byte inverted at a time: every byte of the first 200, where
        // the headers are, and every third after that.
        for at in (0..bytes.len()).filter(|at| *at < 200 || at % 3 == 0) {
            let mut copy = bytes.clone();
            copy[at] = !copy[at];
            std::fs::write(&damaged, &copy).unwrap();
            let _ = try_open(&damaged);
        }
        // Scattered damage.
        let mut state = 0x2545_f491u32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as usize
        };
        for _ in 0..150 {
            let mut copy = bytes.clone();
            for _ in 0..1 + next() % 8 {
                let at = next() % copy.len();
                copy[at] = next() as u8;
            }
            std::fs::write(&damaged, &copy).unwrap();
            let _ = try_open(&damaged);
        }
    }
}

#[test]
fn absurd_sizes_are_errors_not_allocations() {
    // A GIF claiming a 65535 x 65535 screen with a tiny image block.
    let mut gif = b"GIF89a\xff\xff\xff\xff\x80\x00\x00".to_vec();
    gif.extend([0, 0, 0, 255, 255, 255]);
    gif.extend([0x2c, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0]);
    gif.extend([2, 2, 0x4c, 0x01, 0, 0x3b]);
    let path = temp("huge.gif");
    std::fs::write(&path, gif).unwrap();
    let err = open(&path).unwrap_err();
    assert!(err.to_string().contains("cannot fit"), "{err}");

    // A BMP whose header promises far more rows than the file holds.
    let mut bmp = b"BM".to_vec();
    bmp.extend([0u8; 8]);
    bmp.extend(1078u32.to_le_bytes());
    bmp.extend(40u32.to_le_bytes());
    bmp.extend(60_000i32.to_le_bytes());
    bmp.extend(60_000i32.to_le_bytes());
    bmp.extend([1, 0, 8, 0]);
    bmp.extend([0u8; 24]);
    bmp.extend([0u8; 1024]);
    let path = temp("huge.bmp");
    std::fs::write(&path, bmp).unwrap();
    let err = open(&path).unwrap_err();
    assert!(err.to_string().contains("truncated"), "{err}");
}
