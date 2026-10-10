//! Lazy pixel sources: [`RawSource`] reads ImageJ-layout planes straight
//! from their file offsets, [`ChunkedSource`] decodes strips and tiles of
//! any other TIFF through the `tiff` crate.

use std::fmt;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use ::tiff::TiffError as CrateError;
use ::tiff::decoder::{Decoder, Limits};
use imager_core::{Buffer, DType, ImageMeta, PixelSource, Rect};

use super::ifd::{
    ByteOrder, Storage, TiffError, TiffFile, TiffResult, field, format_err, tag, unsupported,
};
use super::reader::IfdInfo;

/// What both sources need to know to turn file bytes into planes.
#[derive(Debug)]
pub(crate) struct Geometry {
    pub width: usize,
    pub height: usize,
    /// Pixel type handed out (differs from the stored type only for
    /// ImageJ's signed 16-bit).
    pub dtype: DType,
    pub bytes_per_sample: usize,
    /// Samples per pixel as stored.
    pub samples: usize,
    /// Samples exposed as channels; planes are numbered with these fastest.
    pub used: usize,
    /// Samples stored as separate planes rather than interleaved.
    pub planar: bool,
    /// Stored unsigned with an offset of 32768 (ImageJ's signed 16-bit).
    pub signed16: bool,
    /// Image in the file for each image in our plane order, if they differ.
    pub image_map: Option<Vec<u32>>,
    /// Number of images (directories, or blocks of an ImageJ stack) used.
    pub images: usize,
}

impl Geometry {
    /// The image in the file and the sample within it that hold `plane`.
    fn locate(&self, plane: usize) -> TiffResult<(usize, usize)> {
        let (image, sample) = (plane / self.used, plane % self.used);
        let image = match &self.image_map {
            Some(map) => map.get(image).map(|&i| i as usize),
            None => Some(image),
        };
        match image {
            Some(image) if image < self.images => Ok((image, sample)),
            _ => format_err(format!("plane {plane} is not in the file")),
        }
    }
}

fn to_core(name: &str, error: TiffError) -> imager_core::Error {
    match error {
        TiffError::Io(source) => imager_core::Error::Io {
            context: name.to_string(),
            source,
        },
        other => imager_core::Error::Other(format!("{name}: {other}")),
    }
}

/// A zeroed byte buffer, or an error instead of an abort if the size taken
/// from the file cannot be allocated.
fn alloc(len: usize) -> TiffResult<Vec<u8>> {
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(len).is_err() {
        return format_err(format!("cannot allocate {len} bytes for pixel data"));
    }
    bytes.resize(len, 0);
    Ok(bytes)
}

/// Converts sample bytes in `order` to a typed buffer.
fn to_buffer(dtype: DType, bytes: Vec<u8>, order: ByteOrder, signed16: bool) -> TiffResult<Buffer> {
    // One block copy, then a byte swap in place if the file's order is not
    // the machine's.
    fn typed<T: imager_core::Pixel + bytemuck::Pod>(
        bytes: &[u8],
        order: ByteOrder,
        swap: fn(T) -> T,
    ) -> Vec<T> {
        let mut samples: Vec<T> = bytemuck::pod_collect_to_vec(bytes);
        if order != ByteOrder::NATIVE {
            for sample in &mut samples {
                *sample = swap(*sample);
            }
        }
        samples
    }
    Ok(match dtype {
        DType::U8 => Buffer::from_vec(bytes),
        DType::I8 => Buffer::from_vec(bytemuck::pod_collect_to_vec::<u8, i8>(&bytes)),
        DType::U16 => Buffer::from_vec(typed(&bytes, order, u16::swap_bytes)),
        DType::I16 => {
            let mut samples = typed(&bytes, order, i16::swap_bytes);
            if signed16 {
                // Stored as value + 32768: flipping the top bit undoes it.
                for sample in &mut samples {
                    *sample ^= i16::MIN;
                }
            }
            Buffer::from_vec(samples)
        }
        DType::U32 => Buffer::from_vec(typed(&bytes, order, u32::swap_bytes)),
        DType::I32 => Buffer::from_vec(typed(&bytes, order, i32::swap_bytes)),
        DType::U64 => Buffer::from_vec(typed(&bytes, order, u64::swap_bytes)),
        DType::I64 => Buffer::from_vec(typed(&bytes, order, i64::swap_bytes)),
        DType::F32 => Buffer::from_vec(typed::<f32>(&bytes, order, |v| {
            f32::from_bits(v.to_bits().swap_bytes())
        })),
        DType::F64 => Buffer::from_vec(typed::<f64>(&bytes, order, |v| {
            f64::from_bits(v.to_bits().swap_bytes())
        })),
        DType::C32 | DType::C64 => return unsupported("complex samples are not supported"),
    })
}

/// Planes of a file laid out as ImageJ writes it: every image uncompressed
/// and back to back from one offset, so image `k` starts at
/// `offset + k * image_bytes` whatever the directories say
/// (`ij.io.ImageReader` / `FileInfo.getOffset`). All arithmetic is 64-bit,
/// which is what lets single-directory stacks beyond 4 GiB open.
pub(crate) struct RawSource {
    meta: ImageMeta,
    storage: Arc<dyn Storage>,
    geometry: Geometry,
    order: ByteOrder,
    offset: u64,
    image_bytes: u64,
}

impl fmt::Debug for RawSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawSource")
            .field("name", &self.meta.name)
            .field("offset", &self.offset)
            .finish_non_exhaustive()
    }
}

impl RawSource {
    pub(crate) fn new(
        meta: ImageMeta,
        storage: Arc<dyn Storage>,
        geometry: Geometry,
        order: ByteOrder,
        offset: u64,
        image_bytes: u64,
    ) -> RawSource {
        RawSource {
            meta,
            storage,
            geometry,
            order,
            offset,
            image_bytes,
        }
    }

    fn region(&self, plane: usize, r: Rect) -> TiffResult<Buffer> {
        let g = &self.geometry;
        let (image, sample) = g.locate(plane)?;
        let bps = g.bytes_per_sample;
        // Bytes from one pixel to the next along a row.
        let step = if g.planar { bps } else { bps * g.samples };
        let mut base = self.offset + image as u64 * self.image_bytes;
        if g.planar {
            base += sample as u64 * (g.width as u64 * g.height as u64 * bps as u64);
        }
        let start = base + (r.y as u64 * g.width as u64 + r.x as u64) * step as u64;
        let row = r.w * step;
        let mut raw = alloc(r.h * row)?;
        // One read when the rows are adjacent in the file or nearly so,
        // otherwise one read per row so that a narrow column of a wide
        // image does not pull in everything between.
        let span = ((r.h - 1) * g.width + r.w) * step;
        if r.w == g.width {
            self.storage.read_at(start, &mut raw)?;
        } else if span <= (1 << 16) || span <= 2 * raw.len() {
            let block = {
                let mut block = alloc(span)?;
                self.storage.read_at(start, &mut block)?;
                block
            };
            for (y, out) in raw.chunks_exact_mut(row).enumerate() {
                let from = y * g.width * step;
                out.copy_from_slice(&block[from..from + row]);
            }
        } else {
            for (y, out) in raw.chunks_exact_mut(row).enumerate() {
                self.storage
                    .read_at(start + (y * g.width * step) as u64, out)?;
            }
        }
        let bytes = if step == bps {
            raw
        } else {
            // Keep one sample of each interleaved pixel.
            let mut one = alloc(r.w * r.h * bps)?;
            for (pixel, out) in raw.chunks_exact(step).zip(one.chunks_exact_mut(bps)) {
                out.copy_from_slice(&pixel[sample * bps..(sample + 1) * bps]);
            }
            one
        };
        to_buffer(g.dtype, bytes, self.order, g.signed16)
    }
}

impl PixelSource for RawSource {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, plane: usize) -> imager_core::Result<Buffer> {
        let whole = Rect::new(0, 0, self.geometry.width, self.geometry.height);
        self.read_region(plane, whole)
    }

    fn read_region(&self, plane: usize, region: Rect) -> imager_core::Result<Buffer> {
        self.region(plane, region)
            .map_err(|e| to_core(&self.meta.name, e))
    }
}

/// The real file with a different first directory.
///
/// The `tiff` crate decodes whichever image the header points at, has no way
/// to jump to a directory by offset, cannot decode palette images, inverts
/// WhiteIsZero samples (ImageJ does not), and drops planes of planar files.
/// So for each image to decode we describe exactly the strips or tiles
/// wanted in a directory of our own, containing only the tags that matter
/// for decompression, and present the crate with a view of the file in
/// which that directory is appended and the header points at it. The crate
/// then serves purely as a decompressor of chunks.
struct VirtualFile {
    storage: Arc<dyn Storage>,
    /// The file's header with the first-directory pointer replaced.
    head: Vec<u8>,
    /// Where the synthetic directory starts: the file length, word aligned.
    tail_start: u64,
    tail: Vec<u8>,
    position: u64,
}

impl Read for VirtualFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let real = self.storage.len();
        let end = self.tail_start + self.tail.len() as u64;
        let pos = self.position;
        if pos >= end || buf.is_empty() {
            return Ok(0);
        }
        let n = if pos < self.head.len() as u64 {
            let from = pos as usize;
            let n = buf.len().min(self.head.len() - from);
            buf[..n].copy_from_slice(&self.head[from..from + n]);
            n
        } else if pos < real {
            let n = (buf.len() as u64).min(real - pos) as usize;
            self.storage.read_at(pos, &mut buf[..n])?;
            n
        } else if pos < self.tail_start {
            buf[0] = 0;
            1
        } else {
            let from = (pos - self.tail_start) as usize;
            let n = buf.len().min(self.tail.len() - from);
            buf[..n].copy_from_slice(&self.tail[from..from + n]);
            n
        };
        self.position += n as u64;
        Ok(n)
    }
}

impl Seek for VirtualFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let end = self.tail_start + self.tail.len() as u64;
        let target = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => end.checked_add_signed(d),
            SeekFrom::Current(d) => self.position.checked_add_signed(d),
        };
        match target {
            Some(p) => {
                self.position = p;
                Ok(p)
            }
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start of the file",
            )),
        }
    }
}

type CrateDecoder = Decoder<BufReader<VirtualFile>>;

/// How one image (or one sample plane of a planar image) is cut into chunks.
struct Chunks {
    /// Nominal chunk size; edge chunks hold less.
    width: usize,
    height: usize,
    across: usize,
    /// Samples per pixel inside a chunk.
    samples: usize,
    /// Tiles always decode at full size; the last strip holds fewer rows.
    tiled: bool,
}

struct Pooled {
    image: usize,
    /// Sample plane for planar files, otherwise 0.
    plane: usize,
    decoder: CrateDecoder,
    chunks: Chunks,
}

/// Decoders kept ready for reuse. Region reads tend to come in runs on one
/// plane, and building a decoder means re-reading that image's directory.
const POOL_SIZE: usize = 4;

/// Planes of a TIFF that is not laid out the ImageJ way: one directory per
/// image, in strips or tiles, possibly compressed.
pub(crate) struct ChunkedSource {
    meta: ImageMeta,
    file: TiffFile,
    geometry: Geometry,
    first: IfdInfo,
    /// Offset of the directory of each image.
    directories: Vec<u64>,
    pool: Mutex<Vec<Pooled>>,
}

impl fmt::Debug for ChunkedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChunkedSource")
            .field("name", &self.meta.name)
            .field("directories", &self.directories.len())
            .finish_non_exhaustive()
    }
}

fn crate_error(error: CrateError) -> TiffError {
    match error {
        CrateError::IoError(e) => TiffError::Io(e),
        CrateError::UnsupportedError(e) => TiffError::Unsupported(e.to_string()),
        other => TiffError::Format(other.to_string()),
    }
}

/// Runs a call into the `tiff` crate. The crate asserts some invariants of
/// the data it decodes instead of returning errors; a damaged file must
/// never take the application down, so such a panic becomes an error (and
/// the decoder involved is discarded by the caller).
fn guarded<T>(call: impl FnOnce() -> Result<T, CrateError>) -> TiffResult<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(result) => result.map_err(crate_error),
        Err(_) => format_err("the pixel data could not be decoded (the file is probably damaged)"),
    }
}

impl ChunkedSource {
    pub(crate) fn new(
        meta: ImageMeta,
        file: TiffFile,
        geometry: Geometry,
        first: IfdInfo,
        directories: Vec<u64>,
    ) -> ChunkedSource {
        ChunkedSource {
            meta,
            file,
            geometry,
            first,
            directories,
            pool: Mutex::new(Vec::new()),
        }
    }

    /// Builds a decoder positioned on one image (or sample plane of it).
    fn decoder(&self, image: usize, plane: usize) -> TiffResult<Pooled> {
        let file = &self.file;
        let g = &self.geometry;
        let offset = self.directories[image];
        let ifd = file.read_ifd(offset)?;
        let info = IfdInfo::parse(file, &ifd)?;
        if info.dtype()? != self.first.dtype()?
            || info.spp != self.first.spp
            || info.width != self.first.width
            || info.height != self.first.height
            || info.planar != self.first.planar
        {
            return format_err(format!(
                "the directory at byte {offset} changed since the file was opened"
            ));
        }
        let (width, height) = (u64::from(info.width), u64::from(info.height));
        let (chunk_w, chunk_h, offsets_tag, counts_tag) = match info.tile {
            Some((w, h)) => (
                u64::from(w),
                u64::from(h),
                tag::TILE_OFFSETS,
                tag::TILE_BYTE_COUNTS,
            ),
            None => (
                width,
                u64::from(info.rows_per_strip),
                tag::STRIP_OFFSETS,
                tag::STRIP_BYTE_COUNTS,
            ),
        };
        let across = width.div_ceil(chunk_w);
        let per_plane = across
            .checked_mul(height.div_ceil(chunk_h))
            .filter(|&n| n <= u64::from(u32::MAX))
            .ok_or_else(|| TiffError::Format("impossibly many strips or tiles".to_string()))?
            as usize;
        let chunk_samples = if info.planar { 1 } else { g.samples };
        let chunk_bytes = chunk_w
            .checked_mul(chunk_h)
            .and_then(|n| n.checked_mul((chunk_samples * g.bytes_per_sample) as u64))
            .ok_or_else(|| TiffError::Format("impossibly large strips or tiles".to_string()))?;

        let Some(entry) = ifd.find(offsets_tag) else {
            return format_err("the image has no strip or tile offsets");
        };
        let mut offsets = file.uints(entry)?;
        let mut counts = match ifd.find(counts_tag) {
            Some(entry) => file.uints(entry)?,
            None => Vec::new(),
        };
        let first_chunk = if info.planar { plane * per_plane } else { 0 };
        if offsets.len() < first_chunk + per_plane {
            return format_err(format!(
                "the image needs {} strips or tiles but lists {}",
                first_chunk + per_plane,
                offsets.len()
            ));
        }
        offsets = offsets[first_chunk..first_chunk + per_plane].to_vec();
        if counts.len() >= first_chunk + per_plane {
            counts = counts[first_chunk..first_chunk + per_plane].to_vec();
        } else if info.compression == 1 && per_plane == 1 {
            // Old writers leave StripByteCounts out; ImageJ copes, so do we.
            counts = vec![chunk_bytes.min(file.storage.len().saturating_sub(offsets[0]))];
        } else {
            return format_err("the image has no usable strip or tile byte counts");
        }
        let real = file.storage.len();
        for (&offset, &count) in offsets.iter().zip(&counts) {
            if offset.checked_add(count).is_none_or(|end| end > real) {
                return format_err(format!(
                    "the pixel data is truncated: {count} bytes at byte {offset} do not fit in a file of {real} bytes"
                ));
            }
            // No supported codec expands data anything like 1500 times
            // (Deflate tops out near 1030, LZW near 1370), so a chunk
            // claiming to is a corrupt size, not something to allocate for.
            if info.compression != 1
                && count.saturating_mul(1500).saturating_add(1 << 16) < chunk_bytes
            {
                return format_err(format!(
                    "a {chunk_w}x{chunk_h} strip or tile cannot be stored in {count} bytes"
                ));
            }
        }
        if info.compression == 1 {
            // Uncompressed data must all be there; this also bounds what a
            // corrupt size can make us allocate by the size of the file.
            let needed = match info.tile {
                Some(_) => chunk_bytes.saturating_mul(per_plane as u64),
                None => width * height * (chunk_samples * g.bytes_per_sample) as u64,
            };
            let present = counts.iter().fold(0u64, |sum, &c| sum.saturating_add(c));
            if present < needed {
                return format_err(format!(
                    "the pixel data is truncated: {needed} bytes are needed but the strips or tiles hold {present}"
                ));
            }
        }

        // The directory handed to the crate. Photometric is always
        // BlackIsZero, so samples come back exactly as stored. A tiled image
        // is declared as large as its whole tiles: the crate then decodes
        // every tile in full and we crop, which avoids its row-by-row path
        // for edge tiles (with tiff 0.11.3 that path failed with "no lzw end
        // code found" on an LZW-compressed corner tile of a 70x50 test image).
        let (coded_width, coded_height) = match info.tile {
            Some(_) => (across * chunk_w, height.div_ceil(chunk_h) * chunk_h),
            None => (width, height),
        };
        if coded_width > u64::from(u32::MAX) || coded_height > u64::from(u32::MAX) {
            return format_err("the tiles extend beyond the largest possible image");
        }
        let order = file.order;
        let wide = file.big;
        let long = |v: &[u64]| -> (u16, Vec<u8>) {
            if wide {
                (
                    field::LONG8,
                    v.iter().flat_map(|&x| order.put_u64(x)).collect(),
                )
            } else {
                (
                    field::LONG,
                    v.iter().flat_map(|&x| order.put_u32(x as u32)).collect(),
                )
            }
        };
        let short = |v: &[u16]| -> (u16, Vec<u8>) {
            (
                field::SHORT,
                v.iter().flat_map(|&x| order.put_u16(x)).collect(),
            )
        };
        let mut entries: Vec<(u16, u64, (u16, Vec<u8>))> = vec![
            (tag::IMAGE_WIDTH, 1, long(&[coded_width])),
            (tag::IMAGE_LENGTH, 1, long(&[coded_height])),
            (
                tag::BITS_PER_SAMPLE,
                chunk_samples as u64,
                short(&vec![info.bits; chunk_samples]),
            ),
            (tag::COMPRESSION, 1, short(&[info.compression as u16])),
            (tag::PHOTOMETRIC, 1, short(&[1])),
            (tag::SAMPLES_PER_PIXEL, 1, short(&[chunk_samples as u16])),
            (tag::PLANAR_CONFIGURATION, 1, short(&[1])),
            (tag::PREDICTOR, 1, short(&[info.predictor])),
            (
                tag::SAMPLE_FORMAT,
                chunk_samples as u64,
                short(&vec![info.format; chunk_samples]),
            ),
            (offsets_tag, per_plane as u64, long(&offsets)),
            (counts_tag, per_plane as u64, long(&counts)),
        ];
        match info.tile {
            Some((w, h)) => {
                entries.push((tag::TILE_WIDTH, 1, long(&[u64::from(w)])));
                entries.push((tag::TILE_LENGTH, 1, long(&[u64::from(h)])));
            }
            None => entries.push((tag::ROWS_PER_STRIP, 1, long(&[chunk_h]))),
        }
        entries.sort_by_key(|e| e.0);

        let tail_start = real + real % 2;
        let inline = if wide { 8 } else { 4 };
        let directory_size = if wide {
            8 + entries.len() * 20 + 8
        } else {
            2 + entries.len() * 12 + 4
        };
        let mut tail = Vec::with_capacity(directory_size);
        let mut values: Vec<u8> = Vec::new();
        if wide {
            tail.extend(order.put_u64(entries.len() as u64));
        } else {
            tail.extend(order.put_u16(entries.len() as u16));
        }
        for (tag, count, (typ, data)) in &entries {
            tail.extend(order.put_u16(*tag));
            tail.extend(order.put_u16(*typ));
            let mut value = vec![0u8; inline];
            if data.len() <= inline {
                value[..data.len()].copy_from_slice(data);
            } else {
                let at = tail_start + (directory_size + values.len()) as u64;
                values.extend(data);
                if wide {
                    value.copy_from_slice(&order.put_u64(at));
                } else {
                    value.copy_from_slice(&order.put_u32(at as u32));
                }
            }
            if wide {
                tail.extend(order.put_u64(*count));
            } else {
                tail.extend(order.put_u32(*count as u32));
            }
            tail.extend(value);
        }
        tail.extend(std::iter::repeat_n(0u8, inline)); // no next directory
        tail.extend(values);
        if !wide && tail_start + tail.len() as u64 > u64::from(u32::MAX) {
            return unsupported(
                "classic TIFF files of 4 GiB or more are only supported in ImageJ's uncompressed layout",
            );
        }
        let mut head = vec![0u8; if wide { 16 } else { 8 }];
        file.storage.read_at(0, &mut head)?;
        if wide {
            head[8..16].copy_from_slice(&order.put_u64(tail_start));
        } else {
            head[4..8].copy_from_slice(&order.put_u32(tail_start as u32));
        }

        let reader = BufReader::new(VirtualFile {
            storage: file.storage.clone(),
            head,
            tail_start,
            tail,
            position: 0,
        });
        // We allocate the output ourselves, bounded by the checks above.
        let mut limits = Limits::default();
        limits.decoding_buffer_size = usize::MAX;
        limits.ifd_value_size = usize::MAX;
        let decoder = guarded(|| Decoder::new(reader))?.with_limits(limits);
        Ok(Pooled {
            image,
            plane,
            decoder,
            chunks: Chunks {
                width: chunk_w as usize,
                height: chunk_h as usize,
                across: across as usize,
                samples: chunk_samples,
                tiled: info.tile.is_some(),
            },
        })
    }

    fn checkout(&self, image: usize, plane: usize) -> TiffResult<Pooled> {
        {
            let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = pool
                .iter()
                .position(|p| p.image == image && p.plane == plane)
            {
                return Ok(pool.remove(i));
            }
        }
        self.decoder(image, plane)
    }

    fn checkin(&self, pooled: Pooled) {
        let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
        if pool.len() >= POOL_SIZE {
            pool.remove(0);
        }
        pool.push(pooled);
    }

    fn region(&self, plane: usize, r: Rect) -> TiffResult<Buffer> {
        let g = &self.geometry;
        let (image, sample) = g.locate(plane)?;
        let mut pooled = self.checkout(image, if g.planar { sample } else { 0 })?;
        let bytes = self.decode(&mut pooled, sample, r)?;
        // A decoder that failed is dropped rather than reused.
        self.checkin(pooled);
        to_buffer(g.dtype, bytes, ByteOrder::NATIVE, g.signed16)
    }

    /// Decodes the chunks that intersect `r` and copies out one sample.
    fn decode(&self, pooled: &mut Pooled, sample: usize, r: Rect) -> TiffResult<Vec<u8>> {
        let g = &self.geometry;
        let c = &pooled.chunks;
        let bps = g.bytes_per_sample;
        let pixel = c.samples * bps;
        let within = if c.samples == 1 { 0 } else { sample * bps };
        let mut out = alloc(r.w * r.h * bps)?;
        let mut chunk = Vec::new();
        for cy in r.y / c.height..=(r.y + r.h - 1) / c.height {
            for cx in r.x / c.width..=(r.x + r.w - 1) / c.width {
                let (x0, y0) = (cx * c.width, cy * c.height);
                let (data_w, data_h) = if c.tiled {
                    (c.width, c.height)
                } else {
                    (c.width, c.height.min(g.height - y0))
                };
                let len = data_w * data_h * pixel;
                if chunk.len() < len {
                    chunk = alloc(len)?;
                }
                let index = (cy * c.across + cx) as u32;
                guarded(|| pooled.decoder.read_chunk_bytes(index, &mut chunk[..len]))?;
                let (xa, xb) = (x0.max(r.x), (x0 + data_w).min(r.x + r.w));
                for y in y0.max(r.y)..(y0 + data_h).min(r.y + r.h) {
                    let from = ((y - y0) * data_w + (xa - x0)) * pixel;
                    let to = ((y - r.y) * r.w + (xa - r.x)) * bps;
                    let n = xb - xa;
                    if pixel == bps {
                        out[to..to + n * bps].copy_from_slice(&chunk[from..from + n * bps]);
                    } else {
                        let source = chunk[from..from + n * pixel].chunks_exact(pixel);
                        for (px, dst) in source.zip(out[to..to + n * bps].chunks_exact_mut(bps)) {
                            dst.copy_from_slice(&px[within..within + bps]);
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

impl PixelSource for ChunkedSource {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, plane: usize) -> imager_core::Result<Buffer> {
        let whole = Rect::new(0, 0, self.geometry.width, self.geometry.height);
        self.read_region(plane, whole)
    }

    fn read_region(&self, plane: usize, region: Rect) -> imager_core::Result<Buffer> {
        self.region(plane, region)
            .map_err(|e| to_core(&self.meta.name, e))
    }

    fn tile_size(&self) -> Option<(usize, usize)> {
        self.first.tile.map(|(w, h)| (w as usize, h as usize))
    }
}
