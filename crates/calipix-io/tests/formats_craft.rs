//! Builds the hand-made GIF and BMP files in
//! `fixtures/imagej/gen/21_crafted_inputs`, which `21_crafted.ijm` has
//! ImageJ open so that `formats_imagej.rs` can compare what Calipix reads
//! from them with what ImageJ reads.
//!
//! They exercise what ImageJ's own writers never produce: GIF disposal
//! methods, transparency, interlacing, local colour tables and partial
//! frames; BMP bit depths, palettes, row order and padding.
//!
//! The files are committed. To rewrite them, run
//! `cargo test -p calipix-io --test formats_craft -- --ignored` and then
//! `cargo xtask gen`.

use std::path::PathBuf;

fn input_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/imagej/gen/21_crafted_inputs")
}

/// GIF image data that uses only literal codes: a clear code often enough
/// that the code width never grows beyond nine bits.
fn lzw_literal(indices: &[u8]) -> Vec<u8> {
    let mut codes: Vec<u16> = Vec::new();
    for chunk in indices.chunks(200) {
        codes.push(256);
        codes.extend(chunk.iter().map(|&i| u16::from(i)));
    }
    codes.push(257);
    let (mut bytes, mut acc, mut count) = (Vec::new(), 0u32, 0);
    for code in codes {
        acc |= u32::from(code) << count;
        count += 9;
        while count >= 8 {
            bytes.push(acc as u8);
            acc >>= 8;
            count -= 8;
        }
    }
    if count > 0 {
        bytes.push(acc as u8);
    }
    let mut out = vec![8];
    for block in bytes.chunks(255) {
        out.push(block.len() as u8);
        out.extend(block);
    }
    out.push(0);
    out
}

struct Gif(Vec<u8>);

impl Gif {
    fn new(width: u16, height: u16, global: &[[u8; 3]], background: u8) -> Gif {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend(width.to_le_bytes());
        bytes.extend(height.to_le_bytes());
        bytes.extend([0xf0 | size_bits(global), background, 0]);
        bytes.extend(global.as_flattened());
        Gif(bytes)
    }

    /// Graphic control extension: disposal method and transparent index.
    fn control(&mut self, disposal: u8, transparent: Option<u8>) {
        let packed = disposal << 2 | u8::from(transparent.is_some());
        self.0
            .extend([0x21, 0xf9, 4, packed, 10, 0, transparent.unwrap_or(0), 0]);
    }

    /// One image; `index(x, y)` gives the palette index in display order.
    fn image(
        &mut self,
        [x, y, w, h]: [u16; 4],
        local: Option<&[[u8; 3]]>,
        interlace: bool,
        index: impl Fn(usize, usize) -> u8,
    ) {
        self.0.push(0x2c);
        for v in [x, y, w, h] {
            self.0.extend(v.to_le_bytes());
        }
        let interlace_bit = if interlace { 0x40 } else { 0 };
        match local {
            Some(table) => {
                self.0.push(0x80 | interlace_bit | size_bits(table));
                self.0.extend(table.as_flattened());
            }
            None => self.0.push(interlace_bit),
        }
        let (w, h) = (usize::from(w), usize::from(h));
        let rows: Vec<usize> = if interlace {
            [(0, 8), (4, 8), (2, 4), (1, 2)]
                .into_iter()
                .flat_map(|(first, step)| (first..h).step_by(step))
                .collect()
        } else {
            (0..h).collect()
        };
        let indices: Vec<u8> = rows
            .into_iter()
            .flat_map(|row| (0..w).map(move |col| (col, row)))
            .map(|(col, row)| index(col, row))
            .collect();
        self.0.extend(lzw_literal(&indices));
    }

    fn finish(mut self) -> Vec<u8> {
        self.0.push(0x3b);
        self.0
    }
}

/// The colour table size field for a table of 2, 4, ... 256 entries.
fn size_bits(table: &[[u8; 3]]) -> u8 {
    assert!(table.len().is_power_of_two() && (2..=256).contains(&table.len()));
    table.len().trailing_zeros() as u8 - 1
}

const COLORS: [[u8; 3]; 8] = [
    [10, 20, 30],
    [250, 40, 40],
    [40, 220, 60],
    [50, 60, 240],
    [240, 230, 20],
    [20, 200, 210],
    [200, 30, 190],
    [128, 128, 128],
];

/// Seven frames covering each disposal method, inherited graphic control
/// state, transparency, interlacing, a local table and a frame that hangs
/// over the edge of the screen.
fn gif_dispose() -> Vec<u8> {
    let local: Vec<[u8; 3]> = (0..4u8)
        .map(|i| [255 - 60 * i, 17 * i, 90 + 40 * i])
        .collect();
    let pattern = |f: usize| move |x: usize, y: usize| ((x + 2 * y + f) % 8) as u8;
    let mut gif = Gif::new(12, 10, &COLORS, 2);
    gif.image([0, 0, 12, 10], None, false, pattern(0));
    gif.control(1, None);
    gif.image([2, 1, 5, 4], None, false, pattern(1));
    gif.control(2, Some(3));
    gif.image([4, 3, 6, 5], None, true, pattern(2));
    gif.control(3, None);
    gif.image([0, 0, 3, 3], Some(&local), false, |x, y| {
        ((x + y) % 4) as u8
    });
    // No graphic control: inherits the previous frame's.
    gif.image([8, 6, 6, 6], None, false, pattern(4));
    gif.control(0, Some(1));
    gif.image([1, 1, 4, 7], None, true, pattern(5));
    gif.image([6, 0, 5, 2], None, false, pattern(6));
    gif.finish()
}

/// A grey first frame followed by coloured ones: ImageJ tests only the
/// first frame before converting the whole stack to 8-bit.
fn gif_gray_first() -> Vec<u8> {
    let grays: Vec<[u8; 3]> = (0..8u8).map(|i| [i * 36; 3]).collect();
    let mut gif = Gif::new(9, 7, &grays, 0);
    gif.image([0, 0, 9, 7], None, false, |x, y| ((x * y + x) % 8) as u8);
    gif.control(1, None);
    gif.image([0, 0, 9, 7], Some(&COLORS), false, |x, y| {
        ((x + y) % 8) as u8
    });
    gif.control(1, None);
    gif.image([1, 1, 7, 5], Some(&COLORS), true, |x, y| {
        ((3 * x + y) % 8) as u8
    });
    gif.finish()
}

/// One image with a local table and a transparent index, smaller than the
/// logical screen.
///
/// It is deliberately not interlaced: Java's decoder, when it delivers an
/// interlaced image that stops short of the bottom of the screen, repeats
/// the image's last row on the row below it (seen with Java 21). Calipix
/// does not reproduce that.
fn gif_single() -> Vec<u8> {
    let mut gif = Gif::new(11, 9, &COLORS, 5);
    let local: Vec<[u8; 3]> = (0..16u8)
        .map(|i| [i * 16, 255 - i * 13, i * 7 + 3])
        .collect();
    gif.control(0, Some(6));
    gif.image([2, 1, 8, 7], Some(&local), false, |x, y| {
        ((x + 3 * y) % 16) as u8
    });
    gif.finish()
}

/// One image with each combination of the features that decide how Java's
/// GIF decoder, which ImageJ uses for single images, delivers the pixels:
/// covering the whole screen or part of it, a transparent index, a local
/// table, interlacing.
fn gif_single_variant(part: bool, transparent: bool, local: bool, interlace: bool) -> Vec<u8> {
    let mut gif = Gif::new(10, 6, &COLORS, 0);
    let table: Vec<[u8; 3]> = COLORS.iter().rev().copied().collect();
    if transparent {
        gif.control(0, Some(3));
    }
    let rect = if part { [3, 1, 6, 4] } else { [0, 0, 10, 6] };
    gif.image(rect, local.then_some(&table[..]), interlace, |x, y| {
        ((x + 2 * y) % 8) as u8
    });
    gif.finish()
}

/// An uncompressed BMP. `height` is negative for top-down rows; `value`
/// gives the palette index, or the packed `0xAARRGGBB` colour for 24 and 32
/// bits, at a position counted from the top left.
fn bmp(
    width: i32,
    height: i32,
    bits: u16,
    palette: &[[u8; 3]],
    value: impl Fn(usize, usize) -> u32,
) -> Vec<u8> {
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let row_bytes = (w * usize::from(bits)).div_ceil(32) * 4;
    let offset = 14 + 40 + palette.len() * 4;
    let mut out = b"BM".to_vec();
    out.extend(((offset + row_bytes * h) as u32).to_le_bytes());
    out.extend([0; 4]);
    out.extend((offset as u32).to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend(width.to_le_bytes());
    out.extend(height.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(bits.to_le_bytes());
    out.extend([0; 16]);
    out.extend((palette.len() as u32).to_le_bytes());
    out.extend([0; 4]);
    for [r, g, b] in palette {
        out.extend([*b, *g, *r, 0]);
    }
    for i in 0..h {
        let y = if height < 0 { i } else { h - 1 - i };
        let mut row = vec![0u8; row_bytes];
        for x in 0..w {
            let v = value(x, y);
            match bits {
                1 | 4 | 8 => {
                    let bit = x * usize::from(bits);
                    row[bit / 8] |= (v as u8) << (8 - usize::from(bits) - bit % 8);
                }
                24 => row[x * 3..x * 3 + 3].copy_from_slice(&v.to_le_bytes()[..3]),
                _ => row[x * 4..x * 4 + 4].copy_from_slice(&v.to_le_bytes()),
            }
        }
        out.extend(row);
    }
    out
}

fn crafted() -> Vec<(&'static str, Vec<u8>)> {
    let two = [[255, 255, 0], [0, 0, 128]];
    let sixteen: Vec<[u8; 3]> = (0..16u8)
        .map(|i| [i * 17, 255 - i * 17, i * 9 + 1])
        .collect();
    let many: Vec<[u8; 3]> = (0..200u8).map(|i| [i, 255 - i, i / 2 + 60]).collect();
    let color = |x: usize, y: usize| {
        (0x5a << 24 | (x * 19 % 256) << 16 | (y * 23 % 256) << 8 | (x * y % 256)) as u32
    };
    vec![
        ("dispose.gif", gif_dispose()),
        ("gray_first.gif", gif_gray_first()),
        ("single.gif", gif_single()),
        (
            "single_full.gif",
            gif_single_variant(false, false, false, false),
        ),
        (
            "single_trans.gif",
            gif_single_variant(false, true, false, false),
        ),
        (
            "single_local.gif",
            gif_single_variant(false, false, true, true),
        ),
        (
            "single_part.gif",
            gif_single_variant(true, false, false, false),
        ),
        (
            "bits1.bmp",
            bmp(13, 7, 1, &two, |x, y| ((x * x + y) % 3 == 0) as u32),
        ),
        (
            "bits4.bmp",
            bmp(13, 7, 4, &sixteen, |x, y| ((x + 5 * y) % 16) as u32),
        ),
        (
            "bits8_topdown.bmp",
            bmp(13, -7, 8, &many, |x, y| ((x * 11 + y * 29) % 200) as u32),
        ),
        ("bits24_topdown.bmp", bmp(13, -7, 24, &[], color)),
        ("bits32.bmp", bmp(13, 7, 32, &[], color)),
    ]
}

#[test]
#[ignore = "rewrites committed fixture inputs"]
fn write_crafted_inputs() {
    std::fs::create_dir_all(input_dir()).unwrap();
    for (name, bytes) in crafted() {
        std::fs::write(input_dir().join(name), bytes).unwrap();
    }
}

/// The committed inputs are exactly what this file builds, so the builders
/// above document them.
#[test]
fn crafted_inputs_are_current() {
    for (name, bytes) in crafted() {
        let committed = std::fs::read(input_dir().join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}; see the module documentation"));
        assert_eq!(committed, bytes, "{name}");
    }
}
