//! ImageJ's own conventions inside a TIFF: the `ImageJ=` ImageDescription
//! and the metadata blocks of private tags 50838 and 50839.

use imager_core::Lut;

use super::ifd::ByteOrder;

/// The version written into `ImageJ=`; the layout we write is that of
/// ImageJ 1.54p's `TiffEncoder`.
pub(crate) const IMAGEJ_VERSION: &str = "1.54p";

/// Decodes description bytes: UTF-8 if valid, otherwise ISO-8859-1 (what
/// older ImageJ versions on Windows wrote for characters such as the micro
/// sign). Trailing NULs are dropped.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let bytes = &bytes[..end];
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// The key/value pairs of an ImageJ description, in file order.
///
/// ImageJ reads the description with `java.util.Properties.load`
/// (`FileOpener.decodeDescriptionString`), so this follows that format:
/// `key=value` lines, `#`/`!` comments, backslash escapes including
/// `\uXXXX`, and continuation lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Description(pub Vec<(String, String)>);

impl Description {
    pub(crate) fn parse(text: &str) -> Description {
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut lines = text.split(['\n', '\r']);
        while let Some(first) = lines.next() {
            let mut logical = first.trim_start_matches([' ', '\t', '\x0c']).to_string();
            if logical.is_empty() || logical.starts_with(['#', '!']) {
                continue;
            }
            // A line ending in an odd number of backslashes continues.
            while logical.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1 {
                logical.pop();
                match lines.next() {
                    Some(more) => logical.push_str(more.trim_start_matches([' ', '\t', '\x0c'])),
                    None => break,
                }
            }
            let (key, value) = split_property(&logical);
            match pairs.iter_mut().find(|(k, _)| *k == key) {
                Some(existing) => existing.1 = value,
                None => pairs.push((key, value)),
            }
        }
        Description(pairs)
    }

    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// `FileOpener.getNumber`: the value as a number, if it parses.
    pub(crate) fn number(&self, key: &str) -> Option<f64> {
        self.get(key)?.trim().parse().ok()
    }

    /// `FileOpener.getDouble`: 0 when missing or malformed.
    pub(crate) fn double(&self, key: &str) -> f64 {
        self.number(key).unwrap_or(0.0)
    }

    /// `FileOpener.getBoolean`: true only for the exact text `true`.
    pub(crate) fn flag(&self, key: &str) -> bool {
        self.get(key) == Some("true")
    }
}

fn split_property(line: &str) -> (String, String) {
    let chars: Vec<char> = line.chars().collect();
    let mut key = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i = unescape(&chars, i, &mut key),
            '=' | ':' | ' ' | '\t' | '\x0c' => break,
            c => {
                key.push(c);
                i += 1;
            }
        }
    }
    let blank = |c: char| matches!(c, ' ' | '\t' | '\x0c');
    while i < chars.len() && blank(chars[i]) {
        i += 1;
    }
    if i < chars.len() && matches!(chars[i], '=' | ':') {
        i += 1;
    }
    while i < chars.len() && blank(chars[i]) {
        i += 1;
    }
    let mut value = String::new();
    while i < chars.len() {
        if chars[i] == '\\' {
            i = unescape(&chars, i, &mut value);
        } else {
            value.push(chars[i]);
            i += 1;
        }
    }
    (key, value)
}

/// Decodes the escape starting at `chars[i]` (a backslash) into `out` and
/// returns the index after it.
fn unescape(chars: &[char], i: usize, out: &mut String) -> usize {
    match chars.get(i + 1) {
        None => i + 1,
        Some('u') => {
            let hex: String = chars[i + 2..].iter().take(4).collect();
            match (hex.len(), u32::from_str_radix(&hex, 16)) {
                (4, Ok(code)) => {
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    i + 6
                }
                _ => {
                    out.push('u');
                    i + 2
                }
            }
        }
        Some(&c) => {
            out.push(match c {
                't' => '\t',
                'n' => '\n',
                'r' => '\r',
                'f' => '\x0c',
                other => other,
            });
            i + 2
        }
    }
}

/// `FileSaver.appendEscapedLine`: printable ASCII as is, everything else
/// (and the backslash) as `\uXXXX`; characters outside the BMP are dropped.
pub(crate) fn push_escaped_line(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push('=');
    for c in value.chars() {
        let code = c as u32;
        if (0x20..0x7f).contains(&code) && c != '\\' {
            out.push(c);
        } else if code <= 0xffff {
            out.push_str(&format!("\\u{code:04X}"));
        }
    }
    out.push('\n');
}

/// Formats a number the way Java's `Double.toString` does, which is how
/// ImageJ writes every floating-point value in the description.
pub(crate) fn java_double(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let magnitude = v.abs();
    if v == 0.0 || (1e-3..1e7).contains(&magnitude) {
        return format!("{v:?}");
    }
    let text = format!("{v:e}");
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    if mantissa.contains('.') {
        format!("{mantissa}E{exponent}")
    } else {
        format!("{mantissa}.0E{exponent}")
    }
}

/// ImageJ writes `finterval` and `fps` as integers when they are whole.
pub(crate) fn java_int_or_double(v: f64) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 2_147_483_648.0 {
        format!("{}", v as i64)
    } else {
        java_double(v)
    }
}

const MAGIC: u32 = 0x494a_494a; // "IJIJ"
const INFO: u32 = 0x696e_666f; // "info"
const LABELS: u32 = 0x6c61_626c; // "labl"
const RANGES: u32 = 0x7261_6e67; // "rang"
const LUTS: u32 = 0x6c75_7473; // "luts"
const ROI: u32 = 0x726f_6920; // "roi "
const OVERLAY: u32 = 0x6f76_6572; // "over"
const PROPERTIES: u32 = 0x7072_6f70; // "prop"

/// The contents of ImageJ's metadata tags.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct IjMeta {
    /// The "Info" image property.
    pub info: Option<String>,
    /// One label per stack slice; `None` where a slice has none.
    pub labels: Vec<Option<String>>,
    /// Display minimum and maximum of each channel, interleaved.
    pub ranges: Vec<f64>,
    /// One 768-byte lookup table (reds, greens, blues) per channel.
    pub luts: Vec<Vec<u8>>,
    /// The active selection, in ImageJ's `.roi` encoding.
    pub roi: Option<Vec<u8>>,
    /// Overlay elements, each in `.roi` encoding.
    pub overlay: Vec<Vec<u8>>,
    /// Image properties as key/value pairs.
    pub props: Vec<(String, String)>,
}

fn utf16(order: ByteOrder, bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| order.u16(c)).collect();
    String::from_utf16_lossy(&units)
}

impl IjMeta {
    /// Decodes tags 50838 (`counts`) and 50839 (`data`) as
    /// `TiffDecoder.getMetaData` does. Damaged metadata is ignored from the
    /// point where it stops making sense, as the pixels are still usable.
    ///
    /// Unlike ImageJ, text is decoded as full UTF-16 in little-endian files
    /// too (an operator-precedence slip in `TiffDecoder.getInfoProperty`
    /// keeps only the low byte of each character there).
    pub(crate) fn parse(order: ByteOrder, counts: &[u64], data: &[u8]) -> IjMeta {
        let mut meta = IjMeta::default();
        let Some(&header) = counts.first() else {
            return meta;
        };
        if !(12..=804).contains(&header) || data.len() < header as usize {
            return meta;
        }
        if order.u32(data) != MAGIC {
            return meta;
        }
        let mut pos = header as usize;
        let mut index = 1;
        for t in 0..(header as usize - 4) / 8 {
            let kind = order.u32(&data[4 + t * 8..]);
            let n = order.u32(&data[8 + t * 8..]) as usize;
            let mut blocks: Vec<&[u8]> = Vec::new();
            for _ in 0..n {
                let Some(&len) = counts.get(index) else {
                    return meta;
                };
                let Some(block) = usize::try_from(len)
                    .ok()
                    .and_then(|len| data.get(pos..pos.checked_add(len)?))
                else {
                    return meta;
                };
                blocks.push(block);
                pos += block.len();
                index += 1;
            }
            match kind {
                INFO => meta.info = blocks.first().map(|b| utf16(order, b)),
                LABELS => {
                    meta.labels = blocks
                        .iter()
                        .map(|b| (!b.is_empty()).then(|| utf16(order, b)))
                        .collect();
                }
                RANGES => {
                    if let Some(block) = blocks.first() {
                        meta.ranges = block
                            .chunks_exact(8)
                            .map(|c| f64::from_bits(order.u64(c)))
                            .collect();
                    }
                }
                LUTS => meta.luts = blocks.iter().map(|b| b.to_vec()).collect(),
                ROI => meta.roi = blocks.first().map(|b| b.to_vec()),
                OVERLAY => meta.overlay = blocks.iter().map(|b| b.to_vec()).collect(),
                PROPERTIES => {
                    meta.props = blocks
                        .chunks_exact(2)
                        .map(|pair| (utf16(order, pair[0]), utf16(order, pair[1])))
                        .collect();
                }
                // Plots and unknown block types are skipped.
                _ => {}
            }
        }
        meta
    }

    /// Encodes the metadata as `TiffEncoder.writeMetaData` does, returning
    /// the values of tags 50838 and 50839, or `None` if there is nothing to
    /// store. Labels are padded or cut to `n_images`.
    pub(crate) fn build(&self, order: ByteOrder, n_images: usize) -> Option<(Vec<u32>, Vec<u8>)> {
        let chars =
            |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|u| order.put_u16(u)).collect() };
        // (type, blocks) in ImageJ's fixed order.
        let mut groups: Vec<(u32, Vec<Vec<u8>>)> = Vec::new();
        if let Some(info) = self.info.as_deref().filter(|s| !s.is_empty()) {
            groups.push((INFO, vec![chars(info)]));
        }
        if self.labels.iter().flatten().any(|l| !l.is_empty()) {
            let labels = (0..n_images)
                .map(|i| match self.labels.get(i) {
                    Some(Some(label)) => chars(label),
                    _ => Vec::new(),
                })
                .collect();
            groups.push((LABELS, labels));
        }
        if !self.ranges.is_empty() {
            let bytes = self
                .ranges
                .iter()
                .flat_map(|v| order.put_u64(v.to_bits()))
                .collect();
            groups.push((RANGES, vec![bytes]));
        }
        if !self.luts.is_empty() {
            groups.push((LUTS, self.luts.clone()));
        }
        if let Some(roi) = &self.roi {
            groups.push((ROI, vec![roi.clone()]));
        }
        if !self.overlay.is_empty() {
            groups.push((OVERLAY, self.overlay.clone()));
        }
        if !self.props.is_empty() {
            let blocks = self
                .props
                .iter()
                .flat_map(|(k, v)| [chars(k), chars(v)])
                .collect();
            groups.push((PROPERTIES, blocks));
        }
        let payload: usize = groups.iter().flat_map(|(_, b)| b).map(Vec::len).sum();
        if payload == 0 {
            return None;
        }
        let header = 4 + groups.len() * 8;
        let mut counts = vec![header as u32];
        let mut data = Vec::with_capacity(header + payload);
        data.extend(order.put_u32(MAGIC));
        for (kind, blocks) in &groups {
            data.extend(order.put_u32(*kind));
            data.extend(order.put_u32(blocks.len() as u32));
        }
        for block in groups.iter().flat_map(|(_, b)| b) {
            counts.push(u32::try_from(block.len()).ok()?);
            data.extend(block);
        }
        Some((counts, data))
    }
}

/// A lookup table from ImageJ's 768-byte form: 256 reds, greens, then blues.
pub(crate) fn lut_from_planar(bytes: &[u8], name: impl Into<String>) -> Option<Lut> {
    if bytes.len() != 768 {
        return None;
    }
    let table: [[u8; 3]; 256] = std::array::from_fn(|i| [bytes[i], bytes[256 + i], bytes[512 + i]]);
    Some(named_lut(table, name))
}

pub(crate) fn lut_to_planar(lut: &Lut) -> Vec<u8> {
    let mut bytes = vec![0u8; 768];
    for (i, rgb) in lut.table.iter().enumerate() {
        bytes[i] = rgb[0];
        bytes[256 + i] = rgb[1];
        bytes[512 + i] = rgb[2];
    }
    bytes
}

const PRIMARIES: [(&str, [bool; 3]); 7] = [
    ("Grays", [true, true, true]),
    ("Red", [true, false, false]),
    ("Green", [false, true, false]),
    ("Blue", [false, false, true]),
    ("Cyan", [false, true, true]),
    ("Magenta", [true, false, true]),
    ("Yellow", [true, true, false]),
];

fn ramp(mask: [bool; 3]) -> [[u8; 3]; 256] {
    std::array::from_fn(|i| mask.map(|on| if on { i as u8 } else { 0 }))
}

/// Builds a [`Lut`], naming it after the standard table it equals if any
/// (file formats store only the table), else `fallback`.
pub(crate) fn named_lut(table: [[u8; 3]; 256], fallback: impl Into<String>) -> Lut {
    let known = PRIMARIES
        .iter()
        .find(|(_, mask)| ramp(*mask) == table)
        .map(|(name, _)| name.to_string());
    let inverted = Lut::grays().inverted();
    let name = match known {
        Some(name) => name,
        None if *inverted.table == table => inverted.name.clone(),
        None => fallback.into(),
    };
    Lut {
        name,
        table: std::sync::Arc::new(table),
    }
}

/// The colour ImageJ gives channel `index` of a composite image that has no
/// stored lookup tables: red, green, blue, white, cyan, magenta, yellow,
/// then white (`CompositeImage.colors`).
pub(crate) fn default_channel_lut(index: usize) -> Lut {
    const ORDER: [usize; 7] = [1, 2, 3, 0, 4, 5, 6];
    let (name, mask) = PRIMARIES[ORDER.get(index).copied().unwrap_or(0)];
    Lut {
        name: name.to_string(),
        table: std::sync::Arc::new(ramp(mask)),
    }
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_follows_java_properties() {
        let d = Description::parse(
            "ImageJ=1.54p\nimages=60\nunit=\\u00B5m\n# comment\nspacing = 1.5\nlong=a\\\n  b\nwin=C\\:\\\\x\r\nflag=true\n",
        );
        assert_eq!(d.get("ImageJ"), Some("1.54p"));
        assert_eq!(d.double("images"), 60.0);
        assert_eq!(d.get("unit"), Some("\u{b5}m"));
        assert_eq!(d.number("spacing"), Some(1.5));
        assert_eq!(d.get("long"), Some("ab"));
        assert_eq!(d.get("win"), Some("C:\\x"));
        assert!(d.flag("flag"));
        assert!(!d.flag("images"));
        assert_eq!(d.double("missing"), 0.0);
    }

    #[test]
    fn escaped_lines_round_trip() {
        let mut text = String::new();
        push_escaped_line(&mut text, "unit", "\u{b5}m \\ x");
        assert_eq!(text, "unit=\\u00B5m \\u005C x\n");
        assert_eq!(Description::parse(&text).get("unit"), Some("\u{b5}m \\ x"));
    }

    #[test]
    fn numbers_print_like_java() {
        for (v, expected) in [
            (10000.0, "10000.0"),
            (0.4999999999999998, "0.4999999999999998"),
            (-21.625, "-21.625"),
            (0.0, "0.0"),
            (1e7, "1.0E7"),
            (1.5e-5, "1.5E-5"),
            (0.001, "0.001"),
            (f64::NAN, "NaN"),
            (f64::NEG_INFINITY, "-Infinity"),
        ] {
            assert_eq!(java_double(v), expected);
        }
        assert_eq!(java_int_or_double(2.0), "2");
        assert_eq!(java_int_or_double(2.5), "2.5");
    }

    #[test]
    fn metadata_blocks_round_trip_in_both_byte_orders() {
        let meta = IjMeta {
            info: Some("line one\nπ".to_string()),
            labels: vec![None, Some("second".to_string()), None],
            ranges: vec![0.0, 255.0, -1.5, 9e9],
            luts: vec![vec![7u8; 768], vec![9u8; 768]],
            roi: Some(b"Iout....".to_vec()),
            overlay: vec![b"Iout1".to_vec(), b"Iout22".to_vec()],
            props: vec![("CurrentLUT".to_string(), "fire".to_string())],
        };
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let (counts, data) = meta.build(order, 3).unwrap();
            let counts: Vec<u64> = counts.iter().map(|&c| u64::from(c)).collect();
            assert_eq!(IjMeta::parse(order, &counts, &data), meta);
            // Damaged input must not panic.
            for cut in 0..data.len() {
                IjMeta::parse(order, &counts, &data[..cut]);
            }
            IjMeta::parse(order, &counts[..2], &data);
        }
        assert!(IjMeta::default().build(ByteOrder::Little, 1).is_none());
    }

    #[test]
    fn standard_tables_are_named() {
        assert_eq!(default_channel_lut(0).name, "Red");
        assert_eq!(default_channel_lut(3).name, "Grays");
        assert_eq!(default_channel_lut(9).name, "Grays");
        let cyan = lut_to_planar(&default_channel_lut(4));
        assert_eq!(lut_from_planar(&cyan, "x").unwrap().name, "Cyan");
        let inverted = lut_to_planar(&Lut::grays().inverted());
        assert_eq!(
            lut_from_planar(&inverted, "x").unwrap(),
            Lut::grays().inverted()
        );
        assert_eq!(lut_from_planar(&[1u8; 768], "Fire").unwrap().name, "Fire");
        assert!(lut_from_planar(&[0u8; 5], "x").is_none());
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(
            hex_decode(&hex_encode(&[0, 15, 255])).unwrap(),
            [0, 15, 255]
        );
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }
}
