//! ROI Manager sets: ZIP archives holding one `.roi` file per selection,
//! as ImageJ's `RoiManager` saves and opens them.
//!
//! Only what such archives need is implemented: stored and deflated
//! entries, no encryption, no ZIP64.

use std::collections::HashSet;
use std::io::{Read, Write};

use crate::codec::{RoiEntry, decode, encode};
use crate::{Error, Roi};

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4b50;
/// No selection file is anywhere near this large; a bigger claim is
/// treated as corruption rather than allocated.
const MAX_ENTRY_SIZE: u64 = 256 << 20;

/// The selections of a ROI Manager set, in the order they were stored.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RoiSet {
    pub entries: Vec<RoiEntry>,
    /// Entries that could not be read, by file name, with the reason:
    /// kinds Calipix does not have (text, image) and damaged files.
    pub skipped: Vec<(String, Error)>,
}

fn bad(reason: impl Into<String>) -> Error {
    Error::Format(format!("not a readable ZIP archive: {}", reason.into()))
}

fn u16_at(data: &[u8], at: usize) -> Result<usize, Error> {
    data.get(at..at.saturating_add(2))
        .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
        .ok_or_else(|| bad("it ends unexpectedly"))
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, Error> {
    data.get(at..at.checked_add(4).ok_or_else(|| bad("an offset overflows"))?)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| bad("it ends unexpectedly"))
}

/// Reads the `.roi` entries of a ZIP archive written by ImageJ's ROI
/// Manager (or by [`write_zip`]). Entries with other extensions are
/// ignored, as ImageJ ignores them. A selection without a stored name
/// takes its entry's file name without the extension.
///
/// Fails if the data is not a ZIP archive or holds no `.roi` entry.
/// Individual entries that cannot be read are listed in
/// [`RoiSet::skipped`].
pub fn read_zip(data: &[u8]) -> Result<RoiSet, Error> {
    // The end-of-central-directory record is at the very end, before an
    // optional comment of up to 65535 bytes.
    if data.len() < 22 {
        return Err(bad("it is too short"));
    }
    let lowest = data.len().saturating_sub(22 + 65535);
    let eocd = (lowest..=data.len() - 22)
        .rev()
        .find(|&at| u32_at(data, at).ok() == Some(END_OF_CENTRAL_DIRECTORY))
        .ok_or_else(|| bad("it has no central directory"))?;
    let count = u16_at(data, eocd + 10)?;
    let mut at = u32_at(data, eocd + 16)? as usize;
    if count == 0xffff || at == 0xffff_ffff {
        return Err(bad("ZIP64 archives are not supported"));
    }
    let mut set = RoiSet::default();
    let mut found = 0;
    for _ in 0..count {
        if u32_at(data, at)? != CENTRAL_HEADER {
            return Err(bad("its central directory is damaged"));
        }
        let flags = u16_at(data, at + 8)?;
        let method = u16_at(data, at + 10)?;
        let crc = u32_at(data, at + 16)?;
        let compressed = u32_at(data, at + 20)? as usize;
        let uncompressed = u64::from(u32_at(data, at + 24)?);
        let name_len = u16_at(data, at + 28)?;
        let extra_len = u16_at(data, at + 30)?;
        let comment_len = u16_at(data, at + 32)?;
        let local = u32_at(data, at + 42)? as usize;
        let name_bytes = data
            .get(at + 46..at + 46 + name_len)
            .ok_or_else(|| bad("its central directory is damaged"))?;
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        at += 46 + name_len + extra_len + comment_len;
        if !name.ends_with(".roi") {
            continue;
        }
        found += 1;
        let contents = (|| -> Result<Vec<u8>, Error> {
            if flags & 1 != 0 {
                return Err(bad(format!("`{name}` is encrypted")));
            }
            if uncompressed > MAX_ENTRY_SIZE {
                return Err(bad(format!("`{name}` claims to be {uncompressed} bytes")));
            }
            if u32_at(data, local)? != LOCAL_HEADER {
                return Err(bad(format!("the entry `{name}` is damaged")));
            }
            let start = local + 30 + u16_at(data, local + 26)? + u16_at(data, local + 28)?;
            let raw = start
                .checked_add(compressed)
                .and_then(|end| data.get(start..end))
                .ok_or_else(|| bad(format!("the entry `{name}` is cut short")))?;
            let bytes = match method {
                0 => raw.to_vec(),
                8 => {
                    let mut out = Vec::new();
                    flate2::read::DeflateDecoder::new(raw)
                        .take(MAX_ENTRY_SIZE + 1)
                        .read_to_end(&mut out)
                        .map_err(|e| bad(format!("`{name}` cannot be decompressed: {e}")))?;
                    out
                }
                other => {
                    return Err(bad(format!(
                        "`{name}` uses compression method {other}"
                    )));
                }
            };
            let mut check = flate2::Crc::new();
            check.update(&bytes);
            if bytes.len() as u64 != uncompressed || check.sum() != crc {
                return Err(bad(format!("`{name}` fails its checksum")));
            }
            Ok(bytes)
        })();
        match contents.and_then(|bytes| decode(&bytes)) {
            Ok(mut entry) => {
                if entry.props.name.is_none() {
                    entry.props.name = Some(name[..name.len() - 4].to_string());
                }
                set.entries.push(entry);
            }
            Err(e) => set.skipped.push((name, e)),
        }
    }
    if found == 0 {
        return Err(Error::Format(
            "the ZIP archive contains no \".roi\" files".to_string(),
        ));
    }
    Ok(set)
}

/// The name ImageJ's ROI Manager gives a selection that has none:
/// the row and column of the centre of its bounds, `yyyy-xxxx`
/// (`RoiManager.getLabel`).
pub(crate) fn default_label(roi: &Roi) -> String {
    let (x, y, w, h) = roi.int_bounds();
    let xc = (x + w / 2).max(0);
    let yc = (y + h / 2).max(0);
    let digits = xc.to_string().len().max(yc.to_string().len()).max(4);
    format!("{yc:0digits$}-{xc:0digits$}")
}

/// Writes selections as a ROI Manager set that ImageJ opens.
///
/// Each selection becomes an entry called after its name, or after the
/// centre of its bounds if it has none; a number is appended where two
/// would have the same name. The archive carries no timestamps (every
/// entry is dated 1980-01-01), so the same selections always give the
/// same bytes.
pub fn write_zip(entries: &[RoiEntry]) -> Result<Vec<u8>, Error> {
    if entries.len() >= 0xffff {
        return Err(Error::Encode(format!(
            "{} selections are more than a ZIP archive holds",
            entries.len()
        )));
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut used = HashSet::new();
    for entry in entries {
        let base = entry
            .props
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| default_label(&entry.roi));
        let base = base.strip_suffix(".roi").unwrap_or(&base).replace('/', "_");
        let mut name = base.clone();
        let mut n = 1;
        while !used.insert(name.clone()) {
            name = format!("{base}-{n}");
            n += 1;
        }
        let file_name = format!("{name}.roi");
        let bytes = encode(entry)?;
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        let compressed = encoder
            .write_all(&bytes)
            .and_then(|()| encoder.finish())
            .map_err(|e| Error::Encode(format!("compression failed: {e}")))?;
        let mut crc = flate2::Crc::new();
        crc.update(&bytes);
        let offset = out.len() as u32;
        // Fields shared by the local and the central header: version
        // needed (2.0), flags (bit 11: UTF-8 names), method (deflate),
        // time, date (1980-01-01), CRC, sizes, name and extra lengths.
        let mut shared = Vec::new();
        shared.extend_from_slice(&20u16.to_le_bytes());
        shared.extend_from_slice(&0x0800u16.to_le_bytes());
        shared.extend_from_slice(&8u16.to_le_bytes());
        shared.extend_from_slice(&0u16.to_le_bytes());
        shared.extend_from_slice(&0x0021u16.to_le_bytes());
        shared.extend_from_slice(&crc.sum().to_le_bytes());
        shared.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        shared.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        shared.extend_from_slice(&(file_name.len() as u16).to_le_bytes());
        shared.extend_from_slice(&0u16.to_le_bytes());

        out.extend_from_slice(&LOCAL_HEADER.to_le_bytes());
        out.extend_from_slice(&shared);
        out.extend_from_slice(file_name.as_bytes());
        out.extend_from_slice(&compressed);

        central.extend_from_slice(&CENTRAL_HEADER.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&shared);
        central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(file_name.as_bytes());
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&END_OF_CENTRAL_DIRECTORY.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with the directory
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_round_trips() {
        let entries = vec![
            RoiEntry::named(Roi::rectangle(4.0, 4.0, 10.0, 10.0), "first"),
            RoiEntry::new(Roi::oval(20.0, 10.0, 15.0, 20.0)),
            RoiEntry::new(Roi::oval(20.0, 10.0, 15.0, 20.0)),
            RoiEntry::named(Roi::line(2.0, 40.0, 60.0, 44.0), "first"),
        ];
        let bytes = write_zip(&entries).unwrap();
        assert_eq!(bytes, write_zip(&entries).unwrap());
        let set = read_zip(&bytes).unwrap();
        assert!(set.skipped.is_empty());
        let names: Vec<_> = set
            .entries
            .iter()
            .map(|e| e.props.name.clone().unwrap())
            .collect();
        assert_eq!(names, ["first", "0020-0027", "0020-0027-1", "first"]);
        let rois: Vec<_> = set.entries.iter().map(|e| e.roi.clone()).collect();
        let expected: Vec<_> = entries.iter().map(|e| e.roi.clone()).collect();
        assert_eq!(rois, expected);
    }

    #[test]
    fn damaged_archives_are_errors() {
        let bytes = write_zip(&[RoiEntry::new(Roi::rectangle(1.0, 2.0, 3.0, 4.0))]).unwrap();
        assert!(read_zip(b"").is_err());
        assert!(read_zip(b"PK\x03\x04 not really a zip archive at all").is_err());
        for cut in 0..bytes.len() {
            // Never a panic, whatever is missing.
            let _ = read_zip(&bytes[..cut]);
        }
        for i in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[i] ^= 0x5a;
            let _ = read_zip(&damaged);
        }
        assert_eq!(read_zip(&write_zip(&[]).unwrap()).map(|s| s.entries.len()).ok(), None);
    }
}
