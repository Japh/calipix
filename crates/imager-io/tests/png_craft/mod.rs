//! The hand-made PNG files of `fixtures/imagej/gen/22_png_inputs`: what
//! each holds and how it is encoded. Shared by `png_craft.rs`, which writes
//! them, and `png_imagej.rs`, which knows from here what every sample is.

// Each test file uses its own part of this module.
#![allow(dead_code)]

use std::path::PathBuf;

pub fn input_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/imagej/gen/22_png_inputs")
}

pub const GRAY: u8 = 0;
pub const RGB: u8 = 2;
pub const INDEXED: u8 = 3;
pub const GRAY_ALPHA: u8 = 4;
pub const RGBA: u8 = 6;

/// One crafted PNG file.
#[derive(Clone)]
pub struct Craft {
    /// File name without `.png`.
    pub name: &'static str,
    pub width: usize,
    pub height: usize,
    /// PNG colour type.
    pub color: u8,
    /// Bits per sample.
    pub depth: u8,
    /// Adam7 interlacing.
    pub interlace: bool,
    /// `PLTE` entries; empty for no chunk.
    pub palette: Vec<[u8; 3]>,
    /// `tRNS` chunk data; empty for no chunk.
    pub trns: Vec<u8>,
    /// Complete chunks to place between `IHDR` and `PLTE`.
    pub extra: Vec<u8>,
    /// Samples are `pattern(x, y, channel) % modulo`.
    pub pattern: fn(usize, usize, usize) -> usize,
    pub modulo: usize,
}

/// Values that wander over the whole range of a sample, differently in each
/// channel.
pub fn spread(x: usize, y: usize, c: usize) -> usize {
    const X: [usize; 4] = [5003, 17, 811, 6007];
    const Y: [usize; 4] = [7919, 9001, 4093, 2999];
    const XY: [usize; 4] = [0, 3, 811, 1];
    x * X[c] + y * Y[c] + x * y * XY[c] + 123 * (c + 1)
}

/// A pattern for small ranges such as palette indices; channels are equal.
pub fn weave(x: usize, y: usize, _c: usize) -> usize {
    x * 3 + y * 5 + x * y
}

/// A palette of `n` unrelated colours.
pub fn colors(n: usize) -> Vec<[u8; 3]> {
    (0..n)
        .map(|i| {
            [
                (i * 37 + 10) as u8,
                (255 - i * 53 % 256) as u8,
                (i * 91 + 5) as u8,
            ]
        })
        .collect()
}

/// A grey palette of `n` evenly spaced levels from black to white.
pub fn grays(n: usize) -> Vec<[u8; 3]> {
    (0..n).map(|i| [(i * 255 / (n - 1)) as u8; 3]).collect()
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(data);
    let mut crc = flate2::Crc::new();
    crc.update(&out[4..]);
    out.extend(crc.sum().to_be_bytes());
    out
}

/// A zlib stream holding `data` in stored blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = data.chunks(0xffff).peekable();
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        out.extend((block.len() as u16).to_le_bytes());
        out.extend((!(block.len() as u16)).to_le_bytes());
        out.extend(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    out.extend((b << 16 | a).to_be_bytes());
    out
}

impl Craft {
    fn new(name: &'static str, color: u8, depth: u8) -> Craft {
        Craft {
            name,
            width: 11,
            height: 9,
            color,
            depth,
            interlace: false,
            palette: Vec::new(),
            trns: Vec::new(),
            extra: Vec::new(),
            pattern: if depth < 8 || color == INDEXED {
                weave
            } else {
                spread
            },
            modulo: 1 << depth,
        }
    }

    fn size(mut self, width: usize, height: usize) -> Craft {
        (self.width, self.height) = (width, height);
        self
    }

    fn interlaced(mut self) -> Craft {
        self.interlace = true;
        self
    }

    /// A palette; indices stay within it.
    fn palette(mut self, palette: Vec<[u8; 3]>) -> Craft {
        self.modulo = palette.len();
        self.palette = palette;
        self
    }

    fn trns(mut self, trns: Vec<u8>) -> Craft {
        self.trns = trns;
        self
    }

    /// Makes the colour at `(x, y)` the transparent one.
    fn key_at(mut self, x: usize, y: usize) -> Craft {
        let colour_channels = if self.color == RGB { 3 } else { 1 };
        self.trns = (0..colour_channels)
            .flat_map(|c| self.at(x, y, c).to_be_bytes())
            .collect();
        self
    }

    pub fn file_name(&self) -> String {
        format!("{}.png", self.name)
    }

    /// Samples per pixel.
    pub fn channels(&self) -> usize {
        match self.color {
            RGB => 3,
            GRAY_ALPHA => 2,
            RGBA => 4,
            _ => 1,
        }
    }

    /// The stored sample of channel `c` at `(x, y)`.
    pub fn at(&self, x: usize, y: usize, c: usize) -> u16 {
        ((self.pattern)(x, y, c) % self.modulo) as u16
    }

    /// One filter byte (none) and the packed samples of the pixels of row
    /// `y` in columns `xs`.
    fn row(&self, xs: impl Iterator<Item = usize>, y: usize, out: &mut Vec<u8>) {
        out.push(0);
        let depth = usize::from(self.depth);
        let (mut acc, mut bits) = (0u32, 0);
        for x in xs {
            for c in 0..self.channels() {
                acc = acc << depth | u32::from(self.at(x, y, c));
                bits += depth;
                while bits >= 8 {
                    out.push((acc >> (bits - 8)) as u8);
                    bits -= 8;
                }
            }
        }
        if bits > 0 {
            out.push((acc << (8 - bits)) as u8);
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend((self.width as u32).to_be_bytes());
        header.extend((self.height as u32).to_be_bytes());
        header.extend([self.depth, self.color, 0, 0, u8::from(self.interlace)]);

        let mut data = Vec::new();
        if self.interlace {
            // The seven Adam7 passes: first column, first row, column step
            // and row step of each.
            const PASSES: [[usize; 4]; 7] = [
                [0, 0, 8, 8],
                [4, 0, 8, 8],
                [0, 4, 4, 8],
                [2, 0, 4, 4],
                [0, 2, 2, 4],
                [1, 0, 2, 2],
                [0, 1, 1, 2],
            ];
            for [x0, y0, dx, dy] in PASSES {
                if x0 >= self.width {
                    continue;
                }
                for y in (y0..self.height).step_by(dy) {
                    self.row((x0..self.width).step_by(dx), y, &mut data);
                }
            }
        } else {
            for y in 0..self.height {
                self.row(0..self.width, y, &mut data);
            }
        }

        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &header));
        out.extend(&self.extra);
        if !self.palette.is_empty() {
            out.extend(chunk(b"PLTE", self.palette.as_flattened()));
        }
        if !self.trns.is_empty() {
            out.extend(chunk(b"tRNS", &self.trns));
        }
        out.extend(chunk(b"IDAT", &zlib_stored(&data)));
        out.extend(chunk(b"IEND", &[]));
        out
    }
}

pub fn crafted() -> Vec<Craft> {
    let alphas = |n: usize| (0..n).map(|i| (i * 37) as u8).collect::<Vec<u8>>();
    let mut gamma_and_background = chunk(b"gAMA", &100_000u32.to_be_bytes());
    gamma_and_background.extend(chunk(b"bKGD", &[0, 255, 0, 0, 0, 0]));
    vec![
        // Every colour type at every depth.
        Craft::new("gray1", GRAY, 1),
        Craft::new("gray2", GRAY, 2),
        Craft::new("gray4", GRAY, 4),
        Craft::new("gray8", GRAY, 8),
        Craft::new("gray16", GRAY, 16),
        Craft::new("graya8", GRAY_ALPHA, 8),
        Craft::new("graya16", GRAY_ALPHA, 16),
        Craft::new("rgb8", RGB, 8),
        Craft::new("rgb16", RGB, 16),
        Craft::new("rgba8", RGBA, 8),
        Craft::new("rgba16", RGBA, 16),
        Craft::new("pal1", INDEXED, 1).palette(colors(2)),
        Craft::new("pal2", INDEXED, 2).palette(colors(4)),
        Craft::new("pal4", INDEXED, 4).palette(colors(16)),
        Craft::new("pal8", INDEXED, 8).palette(colors(256)),
        // Palettes: grey ramps (the 256-level one is what an image has by
        // default), an inverted ramp, and fewer entries than the depth
        // allows.
        Craft::new("pal8_gray", INDEXED, 8).palette(grays(256)),
        Craft::new("pal4_gray", INDEXED, 4).palette(grays(16)),
        Craft::new("pal1_gray", INDEXED, 1).palette(grays(2)),
        Craft::new("pal8_invgray", INDEXED, 8).palette(grays(256).into_iter().rev().collect()),
        Craft::new("pal8_short", INDEXED, 8).palette(colors(200)),
        Craft::new("pal4_short", INDEXED, 4).palette(colors(5)),
        Craft::new("pal8_grayshort", INDEXED, 8).palette(grays(256)[..100].to_vec()),
        // An RGB file whose pixels are all grey.
        {
            let mut craft = Craft::new("rgb8_gray", RGB, 8);
            craft.pattern = weave;
            craft
        },
        // Transparency: palette alpha, including a table that makes nothing
        // transparent, and a transparent colour.
        Craft::new("pal8_trns", INDEXED, 8)
            .palette(colors(256))
            .trns(alphas(100)),
        Craft::new("pal4_trns", INDEXED, 4)
            .palette(colors(16))
            .trns(vec![0, 255, 128, 64, 200]),
        Craft::new("pal1_trns", INDEXED, 1)
            .palette(colors(2))
            .trns(vec![0]),
        Craft::new("pal8_trns_opaque", INDEXED, 8)
            .palette(colors(256))
            .trns(vec![255; 10]),
        Craft::new("gray8_trns", GRAY, 8).key_at(3, 4),
        Craft::new("gray16_trns", GRAY, 16).key_at(3, 4),
        Craft::new("gray4_trns_zero", GRAY, 4).trns(vec![0, 0]),
        Craft::new("gray4_trns", GRAY, 4).trns(vec![0, 8]),
        Craft::new("rgb8_trns", RGB, 8).key_at(3, 4),
        Craft::new("rgb16_trns", RGB, 16).key_at(3, 4),
        // Chunks that could change the colours but must not: ImageJ ignores
        // the gamma and the suggested background.
        {
            let mut craft = Craft::new("rgba8_gama_bkgd", RGBA, 8);
            craft.extra = gamma_and_background;
            craft
        },
        // Interlacing.
        Craft::new("gray1_i", GRAY, 1).interlaced(),
        Craft::new("gray8_i", GRAY, 8).interlaced(),
        Craft::new("gray16_i", GRAY, 16).interlaced(),
        Craft::new("rgb8_i", RGB, 8).interlaced(),
        Craft::new("rgba8_i", RGBA, 8).interlaced(),
        Craft::new("pal4_i", INDEXED, 4)
            .palette(colors(16))
            .interlaced(),
        // Larger files with many combinations of sample and alpha, to pin
        // down the rounding of the blend over white.
        Craft::new("blend_rgba8", RGBA, 8).size(48, 40),
        Craft::new("blend_rgba16", RGBA, 16).size(24, 20),
        Craft::new("blend_graya8", GRAY_ALPHA, 8).size(24, 20),
        Craft::new("blend_graya16", GRAY_ALPHA, 16).size(24, 20),
    ]
}
