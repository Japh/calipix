//! TIFF container parsing: the header, image file directories (IFDs) and tag
//! values of classic TIFF and BigTIFF files in either byte order.
//!
//! This is deliberately our own code rather than the `tiff` crate's: opening
//! a file must cope with tags that crate rejects (descriptions that are not
//! UTF-8, palettes), must not touch pixel data, and must be able to jump to
//! any IFD without walking the chain again.

use std::collections::HashSet;
use std::fmt;
use std::fs::File;
use std::io;
use std::sync::Arc;

/// Why a TIFF could not be read. Converted to the crate's public error, with
/// the file path attached, at the module boundary.
#[derive(Debug)]
pub(crate) enum TiffError {
    Io(io::Error),
    /// The file is damaged or is not a TIFF.
    Format(String),
    /// The file is valid but uses a feature we do not read.
    Unsupported(String),
}

impl fmt::Display for TiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TiffError::Io(e) => write!(f, "{e}"),
            TiffError::Format(m) => write!(f, "invalid TIFF: {m}"),
            TiffError::Unsupported(m) => write!(f, "unsupported TIFF: {m}"),
        }
    }
}

impl From<io::Error> for TiffError {
    fn from(e: io::Error) -> TiffError {
        TiffError::Io(e)
    }
}

pub(crate) type TiffResult<T> = Result<T, TiffError>;

pub(crate) fn format_err<T>(message: impl Into<String>) -> TiffResult<T> {
    Err(TiffError::Format(message.into()))
}

pub(crate) fn unsupported<T>(message: impl Into<String>) -> TiffResult<T> {
    Err(TiffError::Unsupported(message.into()))
}

/// Random-access bytes of a TIFF file. Reads take `&self` so that any number
/// of threads can read planes at once.
pub(crate) trait Storage: Send + Sync + fmt::Debug {
    fn len(&self) -> u64;

    /// Fills `buf` from `offset`, failing if the file ends first.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

#[derive(Debug)]
pub(crate) struct FileStorage {
    file: File,
    len: u64,
}

impl FileStorage {
    pub(crate) fn new(file: File) -> io::Result<FileStorage> {
        let len = file.metadata()?.len();
        Ok(FileStorage { file, len })
    }
}

impl Storage for FileStorage {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let n = positional_read(&self.file, offset, buf)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unexpected end of file",
                ));
            }
            offset += n as u64;
            buf = &mut buf[n..];
        }
        Ok(())
    }
}

#[cfg(unix)]
fn positional_read(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

// `seek_read` moves the file cursor, but every call names its own offset, so
// concurrent calls on one handle do not interfere.
#[cfg(windows)]
fn positional_read(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

#[cfg(not(any(unix, windows)))]
fn positional_read(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = file;
    file.seek(SeekFrom::Start(offset))?;
    file.read(buf)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    pub(crate) const NATIVE: ByteOrder = if cfg!(target_endian = "big") {
        ByteOrder::Big
    } else {
        ByteOrder::Little
    };

    pub(crate) fn u16(self, b: &[u8]) -> u16 {
        let a = [b[0], b[1]];
        match self {
            ByteOrder::Little => u16::from_le_bytes(a),
            ByteOrder::Big => u16::from_be_bytes(a),
        }
    }

    pub(crate) fn u32(self, b: &[u8]) -> u32 {
        let a = [b[0], b[1], b[2], b[3]];
        match self {
            ByteOrder::Little => u32::from_le_bytes(a),
            ByteOrder::Big => u32::from_be_bytes(a),
        }
    }

    pub(crate) fn u64(self, b: &[u8]) -> u64 {
        let mut a = [0u8; 8];
        a.copy_from_slice(&b[..8]);
        match self {
            ByteOrder::Little => u64::from_le_bytes(a),
            ByteOrder::Big => u64::from_be_bytes(a),
        }
    }

    pub(crate) fn put_u16(self, v: u16) -> [u8; 2] {
        match self {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        }
    }

    pub(crate) fn put_u32(self, v: u32) -> [u8; 4] {
        match self {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        }
    }

    pub(crate) fn put_u64(self, v: u64) -> [u8; 8] {
        match self {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        }
    }
}

/// Tag numbers used by the reader and writer.
pub(crate) mod tag {
    pub const IMAGE_WIDTH: u16 = 256;
    pub const IMAGE_LENGTH: u16 = 257;
    pub const BITS_PER_SAMPLE: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const IMAGE_DESCRIPTION: u16 = 270;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const SAMPLES_PER_PIXEL: u16 = 277;
    pub const ROWS_PER_STRIP: u16 = 278;
    pub const STRIP_BYTE_COUNTS: u16 = 279;
    pub const X_RESOLUTION: u16 = 282;
    pub const Y_RESOLUTION: u16 = 283;
    pub const PLANAR_CONFIGURATION: u16 = 284;
    pub const RESOLUTION_UNIT: u16 = 296;
    pub const SOFTWARE: u16 = 305;
    pub const DATE_TIME: u16 = 306;
    pub const ARTIST: u16 = 315;
    pub const HOST_COMPUTER: u16 = 316;
    pub const PREDICTOR: u16 = 317;
    pub const COLOR_MAP: u16 = 320;
    pub const TILE_WIDTH: u16 = 322;
    pub const TILE_LENGTH: u16 = 323;
    pub const TILE_OFFSETS: u16 = 324;
    pub const TILE_BYTE_COUNTS: u16 = 325;
    pub const SAMPLE_FORMAT: u16 = 339;
    pub const NEW_SUBFILE_TYPE: u16 = 254;
    /// ImageJ's private pair: byte counts and data of its metadata blocks.
    pub const IJ_META_COUNTS: u16 = 50838;
    pub const IJ_META_DATA: u16 = 50839;
}

/// Field types.
pub(crate) mod field {
    pub const BYTE: u16 = 1;
    pub const ASCII: u16 = 2;
    pub const SHORT: u16 = 3;
    pub const LONG: u16 = 4;
    pub const RATIONAL: u16 = 5;
    pub const UNDEFINED: u16 = 7;
    pub const IFD: u16 = 13;
    pub const LONG8: u16 = 16;
    pub const IFD8: u16 = 18;
}

/// Bytes taken by one value of a field type, or `None` for unknown types.
fn type_size(typ: u16) -> Option<u64> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return None,
    })
}

/// One directory entry. `field` is the raw value-or-offset field in file
/// byte order (4 bytes used in classic TIFF, 8 in BigTIFF).
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u64,
    field: [u8; 8],
}

#[derive(Clone, Debug)]
pub(crate) struct Ifd {
    pub entries: Vec<Entry>,
    /// Offset of the next IFD, 0 at the end of the chain.
    pub next: u64,
}

impl Ifd {
    pub(crate) fn find(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

/// An open TIFF container.
#[derive(Clone, Debug)]
pub(crate) struct TiffFile {
    pub storage: Arc<dyn Storage>,
    pub order: ByteOrder,
    pub big: bool,
    pub first_ifd: u64,
}

/// True if `header` starts like a classic TIFF or BigTIFF file.
pub(crate) fn is_tiff_header(header: &[u8]) -> bool {
    matches!(
        header,
        [b'I', b'I', 42 | 43, 0, ..] | [b'M', b'M', 0, 42 | 43, ..]
    )
}

impl TiffFile {
    pub(crate) fn open(storage: Arc<dyn Storage>) -> TiffResult<TiffFile> {
        let len = storage.len();
        if len < 8 {
            return format_err("file is too short to be a TIFF");
        }
        let mut head = [0u8; 16];
        let n = len.min(16) as usize;
        storage.read_at(0, &mut head[..n])?;
        let order = match &head[..2] {
            b"II" => ByteOrder::Little,
            b"MM" => ByteOrder::Big,
            _ => return format_err("missing TIFF signature"),
        };
        let (big, first_ifd) = match order.u16(&head[2..]) {
            42 => (false, u64::from(order.u32(&head[4..]))),
            43 => {
                if n < 16 || order.u16(&head[4..]) != 8 || order.u16(&head[6..]) != 0 {
                    return format_err("malformed BigTIFF header");
                }
                (true, order.u64(&head[8..]))
            }
            other => return format_err(format!("unknown TIFF version {other}")),
        };
        Ok(TiffFile {
            storage,
            order,
            big,
            first_ifd,
        })
    }

    /// Reads the directory at `offset` without reading any out-of-line value.
    pub(crate) fn read_ifd(&self, offset: u64) -> TiffResult<Ifd> {
        let len = self.storage.len();
        let (count_size, entry_size, field_size) = if self.big { (8, 20, 8) } else { (2, 12, 4) };
        let truncated = || {
            TiffError::Format(format!(
                "the directory at byte {offset} lies outside the file (truncated?)"
            ))
        };
        if offset == 0 || offset.checked_add(count_size).is_none_or(|end| end > len) {
            return Err(truncated());
        }
        // Exact reads only: a speculative larger read could run into pixel
        // data, which opening a file must never touch.
        let mut block = vec![0u8; count_size as usize];
        self.storage.read_at(offset, &mut block)?;
        let count = if self.big {
            self.order.u64(&block)
        } else {
            u64::from(self.order.u16(&block))
        };
        if count == 0 || count > 65_535 {
            return format_err(format!(
                "the directory at byte {offset} claims {count} entries"
            ));
        }
        let size = count_size + count * entry_size + field_size;
        if offset.checked_add(size).is_none_or(|end| end > len) {
            return Err(truncated());
        }
        block.resize(size as usize, 0);
        self.storage
            .read_at(offset + count_size, &mut block[count_size as usize..])?;
        let mut entries = Vec::with_capacity(count as usize);
        let mut pos = count_size as usize;
        for _ in 0..count {
            let b = &block[pos..pos + entry_size as usize];
            let mut field = [0u8; 8];
            let (count, raw) = if self.big {
                (self.order.u64(&b[4..]), &b[12..20])
            } else {
                (u64::from(self.order.u32(&b[4..])), &b[8..12])
            };
            field[..raw.len()].copy_from_slice(raw);
            entries.push(Entry {
                tag: self.order.u16(b),
                typ: self.order.u16(&b[2..]),
                count,
                field,
            });
            pos += entry_size as usize;
        }
        let next = if self.big {
            self.order.u64(&block[pos..])
        } else {
            u64::from(self.order.u32(&block[pos..]))
        };
        Ok(Ifd { entries, next })
    }

    /// Offsets of every directory in the main chain, in file order.
    pub(crate) fn ifd_chain(&self) -> TiffResult<Vec<(u64, Ifd)>> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = self.first_ifd;
        while offset != 0 {
            if !seen.insert(offset) {
                return format_err("the directory chain loops back on itself");
            }
            let ifd = self.read_ifd(offset)?;
            let next = ifd.next;
            chain.push((offset, ifd));
            offset = next;
        }
        Ok(chain)
    }

    /// The value bytes of an entry, in file byte order.
    pub(crate) fn raw(&self, entry: &Entry) -> TiffResult<Vec<u8>> {
        let Some(size) = type_size(entry.typ) else {
            return format_err(format!(
                "tag {} has unknown field type {}",
                entry.tag, entry.typ
            ));
        };
        let inline = if self.big { 8 } else { 4 };
        let bytes = size
            .checked_mul(entry.count)
            .ok_or_else(|| TiffError::Format(format!("tag {} is impossibly large", entry.tag)))?;
        if bytes <= inline {
            return Ok(entry.field[..bytes as usize].to_vec());
        }
        let offset = if self.big {
            self.order.u64(&entry.field)
        } else {
            u64::from(self.order.u32(&entry.field))
        };
        // Bounding the value by the file size also bounds the allocation.
        if offset
            .checked_add(bytes)
            .is_none_or(|end| end > self.storage.len())
        {
            return format_err(format!(
                "the value of tag {} lies outside the file (truncated?)",
                entry.tag
            ));
        }
        let mut data = vec![0u8; bytes as usize];
        self.storage.read_at(offset, &mut data)?;
        Ok(data)
    }

    /// All values of an unsigned integer tag (BYTE, SHORT, LONG or LONG8).
    pub(crate) fn uints(&self, entry: &Entry) -> TiffResult<Vec<u64>> {
        let raw = self.raw(entry)?;
        let o = self.order;
        Ok(match entry.typ {
            field::BYTE | field::UNDEFINED => raw.iter().map(|&b| u64::from(b)).collect(),
            field::SHORT => raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u64::from(o.u16(c)))
                .collect(),
            field::LONG | field::IFD => raw
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u64::from(o.u32(c)))
                .collect(),
            field::LONG8 | field::IFD8 => raw.as_chunks::<8>().0.iter().map(|c| o.u64(c)).collect(),
            other => {
                return format_err(format!(
                    "tag {} has field type {other}, expected an unsigned integer",
                    entry.tag
                ));
            }
        })
    }

    /// The first value of an unsigned integer tag, if the tag is present.
    pub(crate) fn uint(&self, ifd: &Ifd, tag: u16) -> TiffResult<Option<u64>> {
        match ifd.find(tag) {
            Some(e) if e.count >= 1 => Ok(self.uints(e)?.first().copied()),
            _ => Ok(None),
        }
    }

    /// The value of a RATIONAL tag as numerator and denominator.
    pub(crate) fn rational(&self, ifd: &Ifd, tag: u16) -> TiffResult<Option<(u32, u32)>> {
        let Some(entry) = ifd.find(tag) else {
            return Ok(None);
        };
        if entry.typ != field::RATIONAL || entry.count < 1 {
            return Ok(None);
        }
        let raw = self.raw(entry)?;
        Ok(Some((self.order.u32(&raw), self.order.u32(&raw[4..]))))
    }

    /// The bytes of a text or byte tag, if present.
    pub(crate) fn bytes(&self, ifd: &Ifd, tag: u16) -> TiffResult<Option<Vec<u8>>> {
        match ifd.find(tag) {
            Some(e) if matches!(e.typ, field::BYTE | field::ASCII | field::UNDEFINED) => {
                Ok(Some(self.raw(e)?))
            }
            _ => Ok(None),
        }
    }
}

/// In-memory storage that records what was read, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MemStorage {
    pub data: Vec<u8>,
    /// Every `(offset, length)` read so far.
    pub reads: std::sync::Mutex<Vec<(u64, usize)>>,
}

#[cfg(test)]
impl MemStorage {
    pub(crate) fn new(data: Vec<u8>) -> Arc<MemStorage> {
        Arc::new(MemStorage {
            data,
            reads: Default::default(),
        })
    }
}

#[cfg(test)]
impl Storage for MemStorage {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.reads.lock().unwrap().push((offset, buf.len()));
        let range = usize::try_from(offset)
            .ok()
            .and_then(|start| Some(start..start.checked_add(buf.len())?));
        match range.and_then(|r| self.data.get(r)) {
            Some(src) => {
                buf.copy_from_slice(src);
                Ok(())
            }
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected end of file",
            )),
        }
    }
}
