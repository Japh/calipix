//! TIFFs that were not written by ImageJ: tiles, BigTIFF, compression,
//! predictors, both byte orders, multi-page files, unusual photometric
//! interpretations and OME-TIFF. Files come from the `tiff` crate's encoder
//! or from the byte-level builder in `tiff_support`.

mod tiff_support;

use std::io::Cursor;

use calipix_core::{AxisKind, ColorModel, DType, Image, Lut, Pixel, Rect};
use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype};
use tiff::tags::Predictor;
use tiff_support::{
    Enc, Page, ascii, build, deflate, difference_u16, float_predict, lzw, open_bytes, packbits,
    pattern, plane, set, strips, tiles,
};

const W: usize = 70;
const H: usize = 50;

fn gray16(x: usize, y: usize) -> u16 {
    (x * 601 + y * 257 + 7 * x * y) as u16
}

fn gray32f(x: usize, y: usize) -> f32 {
    (x as f32 - 20.5) * 0.37 + (y * y) as f32 / 9.0
}

const LE: Enc = Enc { big_endian: false };
const BE: Enc = Enc { big_endian: true };

/// Every region read must equal the same rectangle of the whole plane.
fn assert_regions_match<T: Pixel>(image: &Image, plane_index: usize) {
    let (w, h) = (image.meta().width(), image.meta().height());
    let whole = plane::<T>(image, plane_index);
    for r in [
        Rect::new(0, 0, w, h),
        Rect::new(0, 0, 1, 1),
        Rect::new(w - 1, h - 1, 1, 1),
        Rect::new(3, 5, 40, 30),
        Rect::new(31, 15, 2, 2),
        Rect::new(0, h - 3, w, 3),
        Rect::new(w - 5, 0, 5, h),
    ] {
        let region = image.read_region(plane_index, r).unwrap();
        let expected: Vec<T> = (r.y..r.y + r.h)
            .flat_map(|y| whole[y * w + r.x..y * w + r.x + r.w].to_vec())
            .collect();
        assert_eq!(region.try_slice::<T>().unwrap(), expected, "{r:?}");
    }
}

fn assert_gray16(image: &Image) {
    assert_eq!(image.meta().dtype(), DType::U16);
    assert_eq!(image.meta().shape(), &[W, H]);
    assert_eq!(plane::<u16>(image, 0), pattern(W, H, gray16));
    assert_regions_match::<u16>(image, 0);
    assert!(image.meta().channels.is_empty());
}

#[test]
fn crate_encoder_files() {
    let data = pattern(W, H, gray16);
    let deflate = || Compression::Deflate(DeflateLevel::default());
    for (name, big, compression, predictor) in [
        ("plain", false, Compression::Uncompressed, Predictor::None),
        ("bigtiff", true, Compression::Uncompressed, Predictor::None),
        ("lzw", false, Compression::Lzw, Predictor::None),
        ("deflate", false, deflate(), Predictor::None),
        ("packbits", false, Compression::Packbits, Predictor::None),
        ("lzw_h", false, Compression::Lzw, Predictor::Horizontal),
        ("deflate_h", false, deflate(), Predictor::Horizontal),
        ("big_lzw_h", true, Compression::Lzw, Predictor::Horizontal),
    ] {
        let mut bytes = Cursor::new(Vec::new());
        if big {
            let mut encoder = TiffEncoder::new_big(&mut bytes)
                .unwrap()
                .with_compression(compression)
                .with_predictor(predictor);
            encoder
                .write_image::<colortype::Gray16>(W as u32, H as u32, &data)
                .unwrap();
        } else {
            let mut encoder = TiffEncoder::new(&mut bytes)
                .unwrap()
                .with_compression(compression)
                .with_predictor(predictor);
            encoder
                .write_image::<colortype::Gray16>(W as u32, H as u32, &data)
                .unwrap();
        }
        let image = open_bytes(&format!("enc_{name}.tif"), bytes.get_ref())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_gray16(&image);
        assert!(!image.is_in_memory());
    }
}

#[test]
fn crate_encoder_sample_types() {
    fn check<C>(name: &str, data: Vec<C::Inner>, samples: usize)
    where
        C: colortype::ColorType,
        C::Inner: Pixel,
        [C::Inner]: tiff::encoder::TiffValue,
    {
        let mut bytes = Cursor::new(Vec::new());
        TiffEncoder::new(&mut bytes)
            .unwrap()
            .with_compression(Compression::Lzw)
            .write_image::<C>(W as u32, H as u32, &data)
            .unwrap();
        let image = open_bytes(&format!("enc_{name}.tif"), bytes.get_ref())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(image.meta().dtype(), <C::Inner as Pixel>::DTYPE, "{name}");
        assert_eq!(image.meta().plane_count(), samples, "{name}");
        for s in 0..samples {
            let expected: Vec<C::Inner> = data.iter().skip(s).step_by(samples).copied().collect();
            assert_eq!(plane::<C::Inner>(&image, s), expected, "{name} sample {s}");
            assert_regions_match::<C::Inner>(&image, s);
        }
    }
    let n = W * H;
    check::<colortype::Gray8>("u8", (0..n).map(|i| (i * 7) as u8).collect(), 1);
    check::<colortype::GrayI8>("i8", (0..n).map(|i| (i * 7) as i8).collect(), 1);
    check::<colortype::GrayI16>("i16", (0..n).map(|i| (i * 37) as i16).collect(), 1);
    check::<colortype::Gray32>(
        "u32",
        (0..n).map(|i| (i as u32).wrapping_mul(1_234_567)).collect(),
        1,
    );
    check::<colortype::GrayI32>(
        "i32",
        (0..n)
            .map(|i| (i as i32 - 900).wrapping_mul(1_234_567))
            .collect(),
        1,
    );
    check::<colortype::Gray64>("u64", (0..n).map(|i| (i as u64) << 40).collect(), 1);
    check::<colortype::GrayI64>("i64", (0..n).map(|i| (i as i64 - 900) << 40).collect(), 1);
    check::<colortype::Gray32Float>("f32", (0..n).map(|i| i as f32 * 0.37 - 5.0).collect(), 1);
    check::<colortype::Gray64Float>("f64", (0..n).map(|i| i as f64 * 0.37 - 5.0).collect(), 1);
    check::<colortype::RGB8>("rgb8", (0..n * 3).map(|i| (i * 5) as u8).collect(), 3);
    check::<colortype::RGB16>("rgb16", (0..n * 3).map(|i| (i * 51) as u16).collect(), 3);
    // Alpha is discarded.
    let rgba: Vec<u8> = (0..n * 4).map(|i| (i * 3) as u8).collect();
    let mut bytes = Cursor::new(Vec::new());
    TiffEncoder::new(&mut bytes)
        .unwrap()
        .write_image::<colortype::RGBA8>(W as u32, H as u32, &rgba)
        .unwrap();
    let image = open_bytes("enc_rgba8.tif", bytes.get_ref()).unwrap();
    assert_eq!(image.meta().shape(), &[W, H, 3]);
    assert_eq!(image.meta().color, ColorModel::Rgb);
    for c in 0..3 {
        let expected: Vec<u8> = rgba.iter().skip(c).step_by(4).copied().collect();
        assert_eq!(plane::<u8>(&image, c), expected);
    }
}

#[test]
fn rgb_is_three_channels() {
    let data: Vec<u16> = (0..W * H * 3).map(|i| (i * 51) as u16).collect();
    let mut bytes = Cursor::new(Vec::new());
    TiffEncoder::new(&mut bytes)
        .unwrap()
        .with_compression(Compression::Lzw)
        .with_predictor(Predictor::Horizontal)
        .write_image::<colortype::RGB16>(W as u32, H as u32, &data)
        .unwrap();
    let image = open_bytes("rgb16_lzw_h.tif", bytes.get_ref()).unwrap();
    let meta = image.meta();
    assert_eq!(meta.shape(), &[W, H, 3]);
    assert_eq!(meta.axes()[2].kind, AxisKind::Channel);
    assert_eq!(meta.color, ColorModel::Rgb);
    for c in 0..3 {
        let expected: Vec<u16> = data.iter().skip(c).step_by(3).copied().collect();
        assert_eq!(plane::<u16>(&image, c), expected);
    }
}

#[test]
fn tiles_in_both_byte_orders_and_containers() {
    let data = pattern(W, H, gray16);
    let (tw, th) = (32, 16);
    for (name, enc, big, compression) in [
        ("tiled_le", LE, false, 1),
        ("tiled_be", BE, false, 1),
        ("tiled_le_lzw_h", LE, false, 5),
        ("tiled_be_deflate_h", BE, false, 8),
        ("tiled_be_big_lzw_h", BE, true, 5),
        ("tiled_le_big", LE, true, 1),
    ] {
        let predict = compression != 1;
        let chunks: Vec<Vec<u8>> = tiles(&enc.u16_samples(&data), W, H, 2, tw, th)
            .into_iter()
            .map(|tile| {
                if !predict {
                    return tile;
                }
                // Differencing applies to sample values, so decode, predict
                // and encode again in the file's byte order.
                let values: Vec<u16> = tile
                    .chunks(2)
                    .map(|c| {
                        if enc.big_endian {
                            u16::from_be_bytes([c[0], c[1]])
                        } else {
                            u16::from_le_bytes([c[0], c[1]])
                        }
                    })
                    .collect();
                let bytes = enc.u16_samples(&difference_u16(tw, 1, &values));
                if compression == 5 {
                    lzw(&bytes)
                } else {
                    deflate(&bytes)
                }
            })
            .collect();
        let mut entries = enc.gray(W as u32, H as u32, 16, 1);
        set(&mut entries, enc.shorts(259, &[compression]));
        if predict {
            entries.push(enc.shorts(317, &[2]));
        }
        entries.push(enc.longs(322, &[tw as u32]));
        entries.push(enc.longs(323, &[th as u32]));
        let bytes = build(
            enc,
            big,
            &[Page {
                entries,
                chunks,
                tiled: true,
            }],
        );
        let image =
            open_bytes(&format!("{name}.tif"), &bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_gray16(&image);
        assert_eq!(image.tile_size(), Some((tw, th)), "{name}");
    }
}

#[test]
fn strips_with_every_codec_and_byte_order() {
    let data = pattern(W, H, gray16);
    let rows = 7;
    for enc in [LE, BE] {
        for (compression, predictor) in [
            (1, 1),
            (5, 1),
            (5, 2),
            (8, 1),
            (8, 2),
            (32946, 2),
            (32773, 1),
        ] {
            let samples = if predictor == 2 {
                difference_u16(W, 1, &data)
            } else {
                data.clone()
            };
            let chunks: Vec<Vec<u8>> = strips(&enc.u16_samples(&samples), W * 2, rows)
                .into_iter()
                .map(|strip| match compression {
                    1 => strip,
                    5 => lzw(&strip),
                    32773 => packbits(&strip, W * 2),
                    _ => deflate(&strip),
                })
                .collect();
            let mut entries = enc.gray(W as u32, H as u32, 16, 1);
            set(&mut entries, enc.shorts(259, &[compression]));
            entries.push(enc.longs(278, &[rows as u32]));
            entries.push(enc.shorts(317, &[predictor]));
            let bytes = build(
                enc,
                false,
                &[Page {
                    entries,
                    chunks,
                    tiled: false,
                }],
            );
            let name = format!(
                "strips_{}_{compression}_{predictor}.tif",
                if enc.big_endian { "be" } else { "le" }
            );
            let image = open_bytes(&name, &bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_gray16(&image);
            assert_eq!(image.tile_size(), None);
        }
    }
}

#[test]
fn floating_point_predictor() {
    let data = pattern(W, H, gray32f);
    for enc in [LE, BE] {
        for compression in [5u16, 8] {
            let predicted = float_predict(W, &data);
            let packed = if compression == 5 {
                lzw(&predicted)
            } else {
                deflate(&predicted)
            };
            let mut entries = enc.gray(W as u32, H as u32, 32, 3);
            set(&mut entries, enc.shorts(259, &[compression]));
            entries.push(enc.shorts(317, &[3]));
            let bytes = build(enc, false, &[Page::strip(entries, packed)]);
            let image = open_bytes("float_predictor.tif", &bytes).unwrap();
            assert_eq!(image.meta().dtype(), DType::F32);
            assert_eq!(plane::<f32>(&image, 0), data);
            assert_regions_match::<f32>(&image, 0);
        }
        // And without compression.
        let entries = enc.gray(W as u32, H as u32, 32, 3);
        let bytes = build(enc, false, &[Page::strip(entries, enc.f32_samples(&data))]);
        assert_eq!(
            plane::<f32>(&open_bytes("float_raw.tif", &bytes).unwrap(), 0),
            data
        );
    }
}

fn page16(enc: Enc, width: usize, height: usize, offset: usize) -> Page {
    let data = pattern(width, height, |x, y| {
        gray16(x, y).wrapping_add(offset as u16 * 1000)
    });
    Page::strip(
        enc.gray(width as u32, height as u32, 16, 1),
        lzw(&enc.u16_samples(&data)),
    )
    .with(enc.shorts(259, &[5]))
}

trait With {
    fn with(self, entry: tiff_support::Entry) -> Self;
}

impl With for Page {
    fn with(mut self, entry: tiff_support::Entry) -> Page {
        set(&mut self.entries, entry);
        self
    }
}

#[test]
fn pages_form_a_z_stack() {
    for (enc, big) in [(LE, false), (BE, false), (LE, true)] {
        let pages: Vec<Page> = (0..5).map(|p| page16(enc, W, H, p)).collect();
        let image = open_bytes("pages.tif", &build(enc, big, &pages)).unwrap();
        let meta = image.meta();
        assert_eq!(meta.shape(), &[W, H, 5]);
        assert_eq!(meta.axes()[2].kind, AxisKind::Z);
        assert_eq!(meta.color, ColorModel::Grayscale);
        // Read out of order to exercise jumping between directories.
        for p in [4, 0, 2, 1, 3, 4] {
            let expected = pattern(W, H, |x, y| gray16(x, y).wrapping_add(p as u16 * 1000));
            assert_eq!(plane::<u16>(&image, p), expected, "page {p}");
        }
        assert_regions_match::<u16>(&image, 3);
    }
}

#[test]
fn pages_of_different_sizes_open_as_the_first_image() {
    // A full-size image followed by a thumbnail, as pyramids are stored.
    let pages = [page16(LE, W, H, 0), page16(LE, W / 2, H / 2, 1)];
    let image = open_bytes("pyramid.tif", &build(LE, false, &pages)).unwrap();
    assert_eq!(image.meta().shape(), &[W, H]);
    assert_eq!(plane::<u16>(&image, 0), pattern(W, H, gray16));
}

#[test]
fn many_threads_can_read_at_once() {
    let pages: Vec<Page> = (0..8).map(|p| page16(LE, W, H, p)).collect();
    let image = open_bytes("threads.tif", &build(LE, false, &pages)).unwrap();
    std::thread::scope(|scope| {
        for t in 0..8 {
            let image = &image;
            scope.spawn(move || {
                for round in 0..6 {
                    let p = (t * 3 + round) % 8;
                    let expected = pattern(W, H, |x, y| gray16(x, y).wrapping_add(p as u16 * 1000));
                    assert_eq!(plane::<u16>(image, p), expected);
                    let region = image.read_region(p, Rect::new(10, 20, 4, 1)).unwrap();
                    assert_eq!(
                        region.try_slice::<u16>().unwrap(),
                        &expected[20 * W + 10..20 * W + 14]
                    );
                }
            });
        }
    });
}

#[test]
fn white_is_zero_keeps_samples_and_inverts_the_table() {
    // ImageJ shows such files through an inverting LUT and leaves the data
    // alone (`FileOpener.createColorModel`); the `tiff` crate would invert.
    let data8: Vec<u8> = pattern(W, H, |x, y| (x * 3 + y) as u8);
    let mut entries = LE.gray(W as u32, H as u32, 8, 1);
    set(&mut entries, LE.shorts(262, &[0]));
    let image = open_bytes(
        "wiz8.tif",
        &build(LE, false, &[Page::strip(entries, data8.clone())]),
    )
    .unwrap();
    assert_eq!(plane::<u8>(&image, 0), data8);
    assert_eq!(image.meta().channels[0].lut, Some(Lut::grays().inverted()));

    let data16 = pattern(W, H, gray16);
    for compression in [1u16, 5] {
        let mut entries = BE.gray(W as u32, H as u32, 16, 1);
        set(&mut entries, BE.shorts(262, &[0]));
        set(&mut entries, BE.shorts(259, &[compression]));
        let raw = BE.u16_samples(&data16);
        let chunk = if compression == 5 { lzw(&raw) } else { raw };
        let image = open_bytes(
            "wiz16.tif",
            &build(BE, false, &[Page::strip(entries, chunk)]),
        )
        .unwrap();
        assert_eq!(plane::<u16>(&image, 0), data16);
        assert_eq!(image.meta().channels[0].lut, Some(Lut::grays().inverted()));
    }
}

#[test]
fn planar_samples_become_channels() {
    let planes: Vec<Vec<u16>> = (0..3)
        .map(|c| pattern(W, H, |x, y| gray16(x, y).wrapping_add(c * 111)))
        .collect();
    for (enc, compression, rows) in [(LE, 1u16, H), (BE, 5, 9), (BE, 1, 9)] {
        // Planar: all strips of red, then of green, then of blue.
        let chunks: Vec<Vec<u8>> = planes
            .iter()
            .flat_map(|p| strips(&enc.u16_samples(p), W * 2, rows))
            .map(|s| if compression == 5 { lzw(&s) } else { s })
            .collect();
        let mut entries = vec![
            enc.longs(256, &[W as u32]),
            enc.longs(257, &[H as u32]),
            enc.shorts(258, &[16, 16, 16]),
            enc.shorts(259, &[compression]),
            enc.shorts(262, &[2]),
            enc.shorts(277, &[3]),
            enc.longs(278, &[rows as u32]),
            enc.shorts(284, &[2]),
        ];
        entries.push(enc.shorts(339, &[1, 1, 1]));
        let bytes = build(
            enc,
            false,
            &[Page {
                entries,
                chunks,
                tiled: false,
            }],
        );
        let image = open_bytes("planar.tif", &bytes).unwrap();
        assert_eq!(image.meta().shape(), &[W, H, 3]);
        assert_eq!(image.meta().color, ColorModel::Rgb);
        for (c, expected) in planes.iter().enumerate() {
            assert_eq!(&plane::<u16>(&image, c), expected, "channel {c}");
            assert_regions_match::<u16>(&image, c);
        }
    }
}

#[test]
fn extra_samples_become_channels() {
    // Two interleaved 16-bit samples that are not colour.
    let data: Vec<u16> = (0..W * H * 2).map(|i| (i * 13) as u16).collect();
    let entries = vec![
        LE.longs(256, &[W as u32]),
        LE.longs(257, &[H as u32]),
        LE.shorts(258, &[16, 16]),
        LE.shorts(259, &[1]),
        LE.shorts(262, &[1]),
        LE.shorts(277, &[2]),
    ];
    let bytes = build(LE, false, &[Page::strip(entries, LE.u16_samples(&data))]);
    let image = open_bytes("two_samples.tif", &bytes).unwrap();
    assert_eq!(image.meta().shape(), &[W, H, 2]);
    assert_eq!(image.meta().axes()[2].kind, AxisKind::Channel);
    assert_eq!(image.meta().color, ColorModel::Grayscale);
    for c in 0..2 {
        let expected: Vec<u16> = data.iter().skip(c).step_by(2).copied().collect();
        assert_eq!(plane::<u16>(&image, c), expected);
    }
}

#[test]
fn compressed_palette() {
    let data: Vec<u8> = pattern(W, H, |x, y| (x * 5 + y * 3) as u8);
    // Red ramps up, green down, blue constant; 16-bit entries as TIFF wants.
    let mut map = Vec::new();
    map.extend((0..256u16).map(|i| i * 257));
    map.extend((0..256u16).map(|i| (255 - i) * 257));
    map.extend((0..256u16).map(|_| 0x8080));
    for enc in [LE, BE] {
        let mut entries = enc.gray(W as u32, H as u32, 8, 1);
        set(&mut entries, enc.shorts(262, &[3]));
        set(&mut entries, enc.shorts(259, &[5]));
        entries.push(enc.shorts(320, &map));
        let bytes = build(enc, false, &[Page::strip(entries, lzw(&data))]);
        let image = open_bytes("palette_lzw.tif", &bytes).unwrap();
        assert_eq!(image.meta().dtype(), DType::U8);
        assert_eq!(plane::<u8>(&image, 0), data);
        let lut = image.meta().channels[0].lut.as_ref().unwrap();
        for i in 0..256 {
            assert_eq!(lut.table[i], [i as u8, 255 - i as u8, 0x80]);
        }
    }
}

#[test]
fn missing_strip_byte_counts() {
    // Built by hand: header, pixels, then a directory without tag 279.
    let data = pattern(W, H, gray16);
    let mut bytes = b"II\x2a\x00".to_vec();
    let pixels = LE.u16_samples(&data);
    bytes.extend(LE.u32(8 + pixels.len() as u32));
    bytes.extend(&pixels);
    let entries = [
        LE.longs(256, &[W as u32]),
        LE.longs(257, &[H as u32]),
        LE.shorts(258, &[16]),
        LE.shorts(262, &[1]),
        LE.longs(273, &[8]),
    ];
    bytes.extend(LE.u16(entries.len() as u16));
    for e in &entries {
        bytes.extend(LE.u16(e.tag));
        bytes.extend(LE.u16(e.typ));
        bytes.extend(LE.u32(e.count as u32));
        let mut field = [0u8; 4];
        field[..e.data.len()].copy_from_slice(&e.data);
        bytes.extend(field);
    }
    bytes.extend(LE.u32(0));
    let image = open_bytes("no_counts.tif", &bytes).unwrap();
    assert_gray16(&image);
}

#[test]
fn unsupported_features_are_named() {
    let small = |entries: Vec<tiff_support::Entry>, bytes: usize| {
        open_bytes(
            "unsupported.tif",
            &build(LE, false, &[Page::strip(entries, vec![0u8; bytes])]),
        )
        .unwrap_err()
        .to_string()
    };
    let with = |tag: u16, value: u16, bits: u16| {
        let mut entries = LE.gray(8, 8, bits, 1);
        set(&mut entries, LE.shorts(tag, &[value]));
        entries
    };
    for (entries, expected) in [
        (LE.gray(8, 8, 12, 1), "12-bit samples"),
        (LE.gray(8, 8, 1, 1), "1-bit samples"),
        (LE.gray(8, 8, 16, 3), "16-bit floating-point"),
        (LE.gray(8, 8, 32, 6), "complex"),
        (with(259, 7, 8), "JPEG compression"),
        (with(259, 4, 8), "CCITT fax compression"),
        (with(259, 50000, 8), "Zstandard compression"),
        (with(259, 34712, 8), "JPEG 2000 compression"),
        (with(262, 6, 8), "YCbCr"),
        (with(262, 5, 8), "CMYK"),
        (with(262, 3, 16), "palette images with 16-bit"),
    ] {
        let message = small(entries, 8 * 8 * 8);
        assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        assert!(message.contains("unsupported.tif"), "{message}");
    }
}

#[test]
fn resolution_tags_calibrate_like_imagej() {
    let open_with = |extra: Vec<tiff_support::Entry>| {
        let mut entries = LE.gray(8, 8, 8, 1);
        entries.extend(extra);
        open_bytes(
            "resolution.tif",
            &build(LE, false, &[Page::strip(entries, vec![0u8; 64])]),
        )
        .unwrap()
    };
    let calibration = |image: &Image| {
        let a = image.meta().axes();
        (a[0].scale, a[1].scale, a[0].unit.clone(), a[1].unit.clone())
    };
    let inch = Some("inch".to_string());
    // 300 x 150 pixels per inch.
    let image = open_with(vec![
        LE.rational(282, 300, 1),
        LE.rational(283, 150, 1),
        LE.shorts(296, &[2]),
    ]);
    assert_eq!(
        calibration(&image),
        (1.0 / 300.0, 1.0 / 150.0, inch.clone(), inch)
    );
    // 72 dpi means "unknown" (`TiffDecoder.OpenIFD`).
    let image = open_with(vec![
        LE.rational(282, 72, 1),
        LE.rational(283, 72, 1),
        LE.shorts(296, &[2]),
    ]);
    assert_eq!(calibration(&image), (1.0, 1.0, None, None));
    // Pixels per centimetre; small pixels are shown in micrometres
    // (`Prefs.convertToMicrons`, on by default).
    let image = open_with(vec![
        LE.rational(282, 5, 2),
        LE.rational(283, 5, 2),
        LE.shorts(296, &[3]),
    ]);
    let cm = Some("cm".to_string());
    assert_eq!(calibration(&image), (0.4, 0.4, cm.clone(), cm));
    let image = open_with(vec![
        LE.rational(282, 40000, 1),
        LE.rational(283, 40000, 1),
        LE.shorts(296, &[3]),
    ]);
    let micron = Some("\u{b5}m".to_string());
    assert_eq!(calibration(&image), (0.25, 0.25, micron.clone(), micron));
    // Without a ResolutionUnit tag ImageJ applies no calibration.
    let image = open_with(vec![LE.rational(282, 300, 1), LE.rational(283, 300, 1)]);
    assert_eq!(calibration(&image), (1.0, 1.0, None, None));
    // Unit "none": the scale applies but has no unit.
    let image = open_with(vec![
        LE.rational(282, 4, 1),
        LE.rational(283, 2, 1),
        LE.shorts(296, &[1]),
    ]);
    assert_eq!(calibration(&image), (0.25, 0.5, None, None));
    // Zero resolutions are ignored.
    let image = open_with(vec![
        LE.rational(282, 0, 1),
        LE.rational(283, 1, 0),
        LE.shorts(296, &[3]),
    ]);
    assert_eq!(calibration(&image).0, 1.0);
}

#[test]
fn text_tags_become_properties() {
    let mut entries = LE.gray(8, 8, 8, 1);
    entries.push(ascii(270, b"made by a microscope"));
    entries.push(ascii(305, b"AcquireIt 2.0"));
    entries.push(ascii(306, b"2026:10:11 12:00:00"));
    let image = open_bytes(
        "text_tags.tif",
        &build(LE, false, &[Page::strip(entries, vec![0u8; 64])]),
    )
    .unwrap();
    let property = |key: &str| image.meta().properties.get(key).map(String::as_str);
    assert_eq!(property("tiff.description"), Some("made by a microscope"));
    assert_eq!(property("tiff.software"), Some("AcquireIt 2.0"));
    assert_eq!(property("tiff.datetime"), Some("2026:10:11 12:00:00"));
    assert_eq!(image.meta().name, "text_tags.tif");
}

#[test]
fn imagej_description_that_is_not_utf8() {
    // Older ImageJ on Windows wrote the micro sign as the single byte B5.
    let data = pattern(W, H, gray16);
    let mut entries = LE.gray(W as u32, H as u32, 16, 1);
    entries.push(ascii(
        270,
        b"ImageJ=1.52a\nunit=\xb5m\nmin=5.0\nmax=900.0\n",
    ));
    entries.push(LE.rational(282, 4, 1));
    entries.push(LE.rational(283, 4, 1));
    entries.push(LE.shorts(296, &[1]));
    let bytes = build(LE, false, &[Page::strip(entries, LE.u16_samples(&data))]);
    let image = open_bytes("latin1.tif", &bytes).unwrap();
    assert_gray16_pixels(&image);
    let x = &image.meta().axes()[0];
    assert_eq!((x.scale, x.unit.as_deref()), (0.25, Some("\u{b5}m")));
    assert_eq!(image.meta().channels[0].display_range, Some((5.0, 900.0)));
}

fn assert_gray16_pixels(image: &Image) {
    assert_eq!(plane::<u16>(image, 0), pattern(W, H, gray16));
    assert_regions_match::<u16>(image, 0);
}

fn ome_xml(order: &str, extra: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image ID="Image:0" Name="test">
    <Pixels ID="Pixels:0" DimensionOrder="{order}" Type="uint16" SizeX="{W}" SizeY="{H}" SizeC="2" SizeZ="3" SizeT="4"
        PhysicalSizeX="0.25" PhysicalSizeY="0.5" PhysicalSizeZ="1.5" PhysicalSizeZUnit="nm" TimeIncrement="2.5">
      <Channel ID="Channel:0:0" Name="DAPI" SamplesPerPixel="1"/>
      <Channel ID="Channel:0:1" Name="GFP &amp; friends" SamplesPerPixel="1"/>
      <TiffData IFD="0" PlaneCount="24">{extra}</TiffData>
    </Pixels>
  </Image>
</OME>"#
    )
}

fn ome_file(order: &str, extra: &str) -> Vec<u8> {
    // The value of each plane encodes its position in the file.
    let mut pages: Vec<Page> = (0..24).map(|p| page16(LE, W, H, p)).collect();
    pages[0]
        .entries
        .push(ascii(270, ome_xml(order, extra).as_bytes()));
    build(LE, false, &pages)
}

#[test]
fn ome_tiff_dimensions_order_and_calibration() {
    for order in ["XYCZT", "XYZCT", "XYTZC", "XYZTC"] {
        let image = open_bytes("minimal.ome.tif", &ome_file(order, "")).unwrap();
        let meta = image.meta();
        assert_eq!(meta.shape(), &[W, H, 2, 3, 4], "{order}");
        let axes: Vec<_> = meta
            .axes()
            .iter()
            .map(|a| (a.kind.clone(), a.scale, a.unit.as_deref()))
            .collect();
        assert_eq!(
            axes,
            [
                (AxisKind::X, 0.25, Some("\u{b5}m")),
                (AxisKind::Y, 0.5, Some("\u{b5}m")),
                (AxisKind::Channel, 1.0, None),
                (AxisKind::Z, 1.5, Some("nm")),
                (AxisKind::Time, 2.5, Some("s")),
            ]
        );
        let names: Vec<_> = meta.channels.iter().map(|c| c.name.as_deref()).collect();
        assert_eq!(names, [Some("DAPI"), Some("GFP & friends")]);
        assert!(meta.properties["tiff.description"].contains("<OME"));

        // Where each of our planes sits in the file under this order.
        let sizes = |axis: char| match axis {
            'C' => 2,
            'Z' => 3,
            _ => 4,
        };
        let letters: Vec<char> = order.chars().skip(2).collect();
        for t in 0..4 {
            for z in 0..3 {
                for c in 0..2 {
                    let at = |axis: char| match axis {
                        'C' => c,
                        'Z' => z,
                        _ => t,
                    };
                    let page = at(letters[0])
                        + sizes(letters[0]) * (at(letters[1]) + sizes(letters[1]) * at(letters[2]));
                    let index = meta.plane_index(&[c, z, t]).unwrap();
                    let first = plane::<u16>(&image, index)[0];
                    assert_eq!(first, (page * 1000) as u16, "{order} c{c} z{z} t{t}");
                }
            }
        }
    }
}

#[test]
fn ome_tiff_spanning_files_is_refused() {
    let own = r#"<UUID FileName="multi.ome.tif">urn:uuid:0</UUID>"#;
    assert!(open_bytes("multi.ome.tif", &ome_file("XYCZT", own)).is_ok());
    let other = r#"<UUID FileName="part2.ome.tif">urn:uuid:1</UUID>"#;
    let message = open_bytes("multi.ome.tif", &ome_file("XYCZT", other))
        .unwrap_err()
        .to_string();
    assert!(message.contains("multi-file OME-TIFF"), "{message}");
    assert!(message.contains("part2.ome.tif"), "{message}");
}

#[test]
fn ome_xml_that_does_not_fit_the_file_is_ignored() {
    // 24 planes claimed, 5 present: a plain stack, still calibrated.
    let mut pages: Vec<Page> = (0..5).map(|p| page16(LE, W, H, p)).collect();
    pages[0]
        .entries
        .push(ascii(270, ome_xml("XYCZT", "").as_bytes()));
    let image = open_bytes("short.ome.tif", &build(LE, false, &pages)).unwrap();
    assert_eq!(image.meta().shape(), &[W, H, 5]);
    assert_eq!(image.meta().axes()[2].kind, AxisKind::Z);
    assert_eq!(image.meta().axes()[0].scale, 0.25);
}
