//! Helpers shared by the TIFF integration tests: locating fixtures, scratch
//! files, and a byte-level TIFF builder for layouts no encoder at hand can
//! produce (big-endian, tiles, BigTIFF, planar samples, odd tags).
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use calipix_core::{Image, Pixel};

/// A file written by real ImageJ (see `fixtures/imagej/gen`).
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// A scratch path unique to this process and `name`.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-io-tiff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Writes `bytes` to a scratch file and opens it.
pub fn open_bytes(name: &str, bytes: &[u8]) -> calipix_io::Result<Image> {
    let path = scratch(name);
    std::fs::write(&path, bytes).unwrap();
    calipix_io::open(&path)
}

/// Plane `plane` as a typed vector.
pub fn plane<T: Pixel>(image: &Image, plane: usize) -> Vec<T> {
    image
        .read_plane(plane)
        .unwrap()
        .try_slice::<T>()
        .unwrap()
        .to_vec()
}

/// A plane computed from `f(x, y)`.
pub fn pattern<T: Pixel>(width: usize, height: usize, f: impl Fn(usize, usize) -> T) -> Vec<T> {
    (0..width * height)
        .map(|i| f(i % width, i / width))
        .collect()
}

/// One directory entry; `data` is already in file byte order.
#[derive(Clone)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u64,
    pub data: Vec<u8>,
}

/// Value encoders for one byte order.
#[derive(Clone, Copy)]
pub struct Enc {
    pub big_endian: bool,
}

impl Enc {
    pub fn u16(self, v: u16) -> [u8; 2] {
        if self.big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    }

    pub fn u32(self, v: u32) -> [u8; 4] {
        if self.big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    }

    pub fn u64(self, v: u64) -> [u8; 8] {
        if self.big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    }

    pub fn shorts(self, tag: u16, v: &[u16]) -> Entry {
        Entry {
            tag,
            typ: 3,
            count: v.len() as u64,
            data: v.iter().flat_map(|&x| self.u16(x)).collect(),
        }
    }

    pub fn longs(self, tag: u16, v: &[u32]) -> Entry {
        Entry {
            tag,
            typ: 4,
            count: v.len() as u64,
            data: v.iter().flat_map(|&x| self.u32(x)).collect(),
        }
    }

    pub fn long8s(self, tag: u16, v: &[u64]) -> Entry {
        Entry {
            tag,
            typ: 16,
            count: v.len() as u64,
            data: v.iter().flat_map(|&x| self.u64(x)).collect(),
        }
    }

    pub fn rational(self, tag: u16, n: u32, d: u32) -> Entry {
        Entry {
            tag,
            typ: 5,
            count: 1,
            data: [self.u32(n), self.u32(d)].concat(),
        }
    }

    pub fn u16_samples(self, v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|&x| self.u16(x)).collect()
    }

    pub fn f32_samples(self, v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|&x| self.u32(x.to_bits())).collect()
    }

    /// The tags every greyscale image needs, minus chunk offsets.
    pub fn gray(self, width: u32, height: u32, bits: u16, format: u16) -> Vec<Entry> {
        vec![
            self.longs(256, &[width]),
            self.longs(257, &[height]),
            self.shorts(258, &[bits]),
            self.shorts(259, &[1]),
            self.shorts(262, &[1]),
            self.shorts(277, &[1]),
            self.shorts(339, &[format]),
        ]
    }
}

/// Text with the terminating NUL TIFF requires.
pub fn ascii(tag: u16, text: &[u8]) -> Entry {
    let mut data = text.to_vec();
    data.push(0);
    Entry {
        tag,
        typ: 2,
        count: data.len() as u64,
        data,
    }
}

/// Replaces the entry with the same tag, or adds it.
pub fn set(entries: &mut Vec<Entry>, entry: Entry) {
    entries.retain(|e| e.tag != entry.tag);
    entries.push(entry);
}

/// One image: its tags (without chunk offsets and byte counts) and chunks.
pub struct Page {
    pub entries: Vec<Entry>,
    pub chunks: Vec<Vec<u8>>,
    pub tiled: bool,
}

impl Page {
    /// An image stored as a single strip.
    pub fn strip(entries: Vec<Entry>, data: Vec<u8>) -> Page {
        Page {
            entries,
            chunks: vec![data],
            tiled: false,
        }
    }
}

/// Builds a TIFF with, for each page, its chunks, then its out-of-line
/// values, then its directory.
pub fn build(enc: Enc, big: bool, pages: &[Page]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(if enc.big_endian { b"MM" } else { b"II" });
    let cap = if big { 8 } else { 4 };
    if big {
        out.extend(enc.u16(43));
        out.extend(enc.u16(8));
        out.extend(enc.u16(0));
    } else {
        out.extend(enc.u16(42));
    }
    let put = |out: &mut Vec<u8>, at: usize, v: u64| {
        if big {
            out[at..at + 8].copy_from_slice(&enc.u64(v));
        } else {
            out[at..at + 4].copy_from_slice(&enc.u32(v as u32));
        }
    };
    let pad = |out: &mut Vec<u8>| {
        if out.len() % 2 == 1 {
            out.push(0);
        }
    };
    let mut link = out.len();
    out.extend(vec![0u8; cap]);
    for page in pages {
        let (mut offsets, mut counts) = (Vec::new(), Vec::new());
        for chunk in &page.chunks {
            pad(&mut out);
            offsets.push(out.len() as u64);
            counts.push(chunk.len() as u64);
            out.extend(chunk);
        }
        let (offsets_tag, counts_tag) = if page.tiled { (324, 325) } else { (273, 279) };
        let mut entries = page.entries.clone();
        if big {
            entries.push(enc.long8s(offsets_tag, &offsets));
            entries.push(enc.long8s(counts_tag, &counts));
        } else {
            let narrow = |v: &[u64]| v.iter().map(|&x| x as u32).collect::<Vec<_>>();
            entries.push(enc.longs(offsets_tag, &narrow(&offsets)));
            entries.push(enc.longs(counts_tag, &narrow(&counts)));
        }
        entries.sort_by_key(|e| e.tag);
        let mut fields = Vec::new();
        for entry in &entries {
            let mut field = vec![0u8; cap];
            if entry.data.len() <= cap {
                field[..entry.data.len()].copy_from_slice(&entry.data);
            } else {
                pad(&mut out);
                let at = out.len() as u64;
                out.extend(&entry.data);
                if big {
                    field.copy_from_slice(&enc.u64(at));
                } else {
                    field.copy_from_slice(&enc.u32(at as u32));
                }
            }
            fields.push(field);
        }
        pad(&mut out);
        let directory = out.len() as u64;
        put(&mut out, link, directory);
        if big {
            out.extend(enc.u64(entries.len() as u64));
        } else {
            out.extend(enc.u16(entries.len() as u16));
        }
        for (entry, field) in entries.iter().zip(&fields) {
            out.extend(enc.u16(entry.tag));
            out.extend(enc.u16(entry.typ));
            if big {
                out.extend(enc.u64(entry.count));
            } else {
                out.extend(enc.u32(entry.count as u32));
            }
            out.extend(field);
        }
        link = out.len();
        out.extend(vec![0u8; cap]);
    }
    out
}

/// TIFF predictor 2 (horizontal differencing) on 16-bit samples.
pub fn difference_u16(width: usize, samples_per_pixel: usize, v: &[u16]) -> Vec<u16> {
    let mut out = v.to_vec();
    for row in out.chunks_mut(width * samples_per_pixel) {
        for i in (samples_per_pixel..row.len()).rev() {
            row[i] = row[i].wrapping_sub(row[i - samples_per_pixel]);
        }
    }
    out
}

/// TIFF predictor 3 (floating point) for single-sample `f32` rows.
pub fn float_predict(width: usize, v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for row in v.chunks(width) {
        let mut bytes = vec![0u8; width * 4];
        for (i, x) in row.iter().enumerate() {
            // Byte planes are most significant first whatever the file order.
            for (k, b) in x.to_bits().to_be_bytes().into_iter().enumerate() {
                bytes[k * width + i] = b;
            }
        }
        for i in (1..bytes.len()).rev() {
            bytes[i] = bytes[i].wrapping_sub(bytes[i - 1]);
        }
        out.extend(bytes);
    }
    out
}

pub fn lzw(data: &[u8]) -> Vec<u8> {
    weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
        .encode(data)
        .unwrap()
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// PackBits with literal runs only, which is valid if not compact.
pub fn packbits(data: &[u8], row_bytes: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in data.chunks(row_bytes) {
        for run in row.chunks(128) {
            out.push((run.len() - 1) as u8);
            out.extend(run);
        }
    }
    out
}

/// Cuts a row-major image into strips of `rows` rows.
pub fn strips(data: &[u8], row_bytes: usize, rows: usize) -> Vec<Vec<u8>> {
    data.chunks(row_bytes * rows).map(<[u8]>::to_vec).collect()
}

/// Cuts a row-major image of `bytes`-byte pixels into padded tiles.
pub fn tiles(
    data: &[u8],
    width: usize,
    height: usize,
    bytes: usize,
    tile_w: usize,
    tile_h: usize,
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for ty in (0..height).step_by(tile_h) {
        for tx in (0..width).step_by(tile_w) {
            let mut tile = vec![0u8; tile_w * tile_h * bytes];
            for y in ty..(ty + tile_h).min(height) {
                let n = tile_w.min(width - tx) * bytes;
                let from = (y * width + tx) * bytes;
                let to = (y - ty) * tile_w * bytes;
                tile[to..to + n].copy_from_slice(&data[from..from + n]);
            }
            out.push(tile);
        }
    }
    out
}

/// The start of a file in ImageJ's layout with a single directory: header,
/// directory, out-of-line values. Pixel data for every plane follows
/// directly, `plane_bytes` each (`ij.io.TiffEncoder`). `entries` must not
/// include the strip tags.
pub fn imagej_head(enc: Enc, entries: &[Entry], plane_bytes: u32) -> Vec<u8> {
    let mut entries = entries.to_vec();
    entries.push(enc.longs(273, &[0]));
    entries.push(enc.longs(279, &[plane_bytes]));
    entries.sort_by_key(|e| e.tag);
    let directory = 2 + entries.len() * 12 + 4;
    let pixels = 8
        + directory
        + entries
            .iter()
            .map(|e| e.data.len())
            .filter(|&n| n > 4)
            .sum::<usize>();
    let mut out = Vec::new();
    out.extend(if enc.big_endian { b"MM" } else { b"II" });
    out.extend(enc.u16(42));
    out.extend(enc.u32(8));
    out.extend(enc.u16(entries.len() as u16));
    let mut values: Vec<u8> = Vec::new();
    for entry in &entries {
        out.extend(enc.u16(entry.tag));
        out.extend(enc.u16(entry.typ));
        out.extend(enc.u32(entry.count as u32));
        if entry.tag == 273 {
            out.extend(enc.u32(pixels as u32));
        } else if entry.data.len() <= 4 {
            let mut field = [0u8; 4];
            field[..entry.data.len()].copy_from_slice(&entry.data);
            out.extend(field);
        } else {
            out.extend(enc.u32((8 + directory + values.len()) as u32));
            values.extend(&entry.data);
        }
    }
    out.extend(enc.u32(0));
    out.extend(values);
    assert_eq!(out.len(), pixels);
    out
}
