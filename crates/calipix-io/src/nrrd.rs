//! NRRD ("nearly raw raster data"), the format of Teem, 3D Slicer and ITK.
//!
//! ImageJ 1.x has no NRRD support of its own, so this follows the format
//! specification (<https://teem.sourceforge.net/nrrd/format.html>), with
//! SCIFIO's `NRRDFormat` as a second opinion on what files look like.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calipix_core::{
    Axis, AxisKind, Buffer, ColorModel, DType, Image, ImageMeta, PixelSource, Rect, crop_buffer,
};
use flate2::read::MultiGzDecoder;

use crate::{Error, Format, Result};

pub(crate) struct Nrrd;

/// Headers are a few hundred bytes; anything longer than this is not one.
const MAX_HEADER_BYTES: usize = 1 << 20;

impl Format for Nrrd {
    fn name(&self) -> &'static str {
        "NRRD"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["nrrd", "nhdr"]
    }

    fn matches(&self, header: &[u8]) -> bool {
        header.starts_with(b"NRRD000")
    }

    /// Opens a `.nrrd` file or a `.nhdr` header with its data file.
    ///
    /// Raw data is read one plane at a time as it is asked for; gzip, text
    /// and hex data are decoded in full on first access.
    ///
    /// Axes take their meaning from `labels` (x, y, z, c, t), then `kinds`
    /// (`domain`/`space` are spatial, `time` is time, anything else is a
    /// channel), then `space directions` (`none` marks a channel). Axes
    /// that none of these describe are X, Y, Z and time in file order. X
    /// and Y must be the first two axes, or the second and third after a
    /// channel axis (interleaved samples).
    ///
    /// Pixel size comes from `spacings` or the length of each
    /// `space directions` vector, and units from `units` or `space units`.
    /// `key:=value` lines become image properties.
    fn read(&self, path: &Path) -> Result<Image> {
        let decode = |message: String| Error::Decode {
            path: path.to_path_buf(),
            message,
        };
        let file = File::open(path).map_err(|e| Error::io(path, e))?;
        let mut reader = BufReader::new(file);
        let (lines, header_len) = read_header_lines(&mut reader)
            .map_err(|e| Error::io(path, e))?
            .map_err(decode)?;
        let header = Header::parse(&lines).map_err(decode)?;
        let (meta, layout) = header.image_meta(path).map_err(decode)?;

        let (data_path, start) = match &header.data_file {
            Some(name) => {
                if name == "LIST" || name.contains('%') {
                    return Err(decode(
                        "data split over several files (`data file: LIST` or a numbered pattern) is not supported"
                            .to_string(),
                    ));
                }
                let dir = path.parent().unwrap_or(Path::new(""));
                (dir.join(name), 0)
            }
            None => (path.to_path_buf(), header_len),
        };
        let io = |e| Error::io(&data_path, e);
        let mut data = BufReader::new(File::open(&data_path).map_err(io)?);
        let file_len = data.get_ref().metadata().map_err(io)?.len();
        data.seek(SeekFrom::Start(start)).map_err(io)?;
        let mut start = start;
        for _ in 0..header.line_skip {
            let skipped = data.skip_until(b'\n').map_err(io)?;
            if skipped == 0 {
                return Err(decode(format!(
                    "`line skip: {}` runs past the end of {}",
                    header.line_skip,
                    data_path.display()
                )));
            }
            start += skipped as u64;
        }

        let sample_bytes = header.dtype.size() as u64;
        // Checked in `image_meta`: the sample count fits in usize.
        let total_bytes = header.sizes.iter().product::<usize>() as u64 * sample_bytes;
        let big_endian = header.big_endian.unwrap_or(false);
        if header.encoding != Encoding::Raw {
            if header.byte_skip < 0 {
                return Err(decode(format!(
                    "`byte skip: {}` is only meaningful with raw encoding",
                    header.byte_skip
                )));
            }
            return Ok(Image::new(DecodedSource {
                meta,
                path: data_path,
                start,
                byte_skip: header.byte_skip as u64,
                encoding: header.encoding,
                big_endian,
                layout,
                planes: OnceLock::new(),
            }));
        }

        let start = match header.byte_skip {
            // Data at the very end of the file, whatever precedes it.
            -1 => file_len.checked_sub(total_bytes),
            skip if skip >= 0 => start.checked_add(skip as u64),
            _ => None,
        };
        let enough = start
            .and_then(|s| s.checked_add(total_bytes))
            .is_some_and(|end| end <= file_len);
        let Some(start) = start.filter(|_| enough) else {
            return Err(decode(format!(
                "the header describes {total_bytes} bytes of samples, which {} ({file_len} bytes) does not hold; the file is truncated or the header is wrong",
                data_path.display(),
            )));
        };
        Ok(Image::cached(RawSource {
            meta,
            file: data.into_inner(),
            path: data_path,
            start,
            big_endian,
            layout,
        }))
    }

    /// Writes a single `.nrrd` file: raw encoding, little-endian, axes in
    /// the image's own order with `kinds`, `labels`, `spacings`, `units`
    /// and, when an axis has a non-zero origin, node-centred `axis mins`.
    /// Image properties are written as `key:=value` lines.
    fn write(&self, image: &Image, path: &Path) -> Result<()> {
        let meta = image.meta();
        let Some(type_name) = type_name(meta.dtype()) else {
            return Err(Error::Unsupported {
                format: "NRRD",
                reason: format!(
                    "NRRD has no complex sample type, but this image is {}",
                    meta.dtype()
                ),
            });
        };
        let header = write_header(meta, type_name);

        // The image may be read lazily from the very file being replaced,
        // so write beside it and move the result into place.
        let mut temp = path.as_os_str().to_owned();
        temp.push(".part");
        let temp = PathBuf::from(temp);
        let write = || -> Result<()> {
            let io = |e| Error::io(&temp, e);
            let mut out = BufWriter::new(File::create(&temp).map_err(io)?);
            out.write_all(header.as_bytes()).map_err(io)?;
            for plane in 0..meta.plane_count() {
                let buffer = image.read_plane(plane)?;
                out.write_all(&le_bytes(&buffer)).map_err(io)?;
            }
            out.flush().map_err(io)?;
            drop(out);
            std::fs::rename(&temp, path).map_err(|e| Error::io(path, e))
        };
        write().inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
    }
}

/// Reads the header up to the blank line that ends it (or the end of the
/// file, for a detached header). Returns the lines and the number of bytes
/// consumed.
fn read_header_lines(
    reader: &mut impl BufRead,
) -> std::io::Result<std::result::Result<(Vec<String>, u64), String>> {
    let mut lines = Vec::new();
    let mut consumed = 0usize;
    loop {
        let mut raw = Vec::new();
        let n = reader
            .by_ref()
            .take((MAX_HEADER_BYTES - consumed) as u64 + 1)
            .read_until(b'\n', &mut raw)?;
        consumed += n;
        if consumed > MAX_HEADER_BYTES {
            return Ok(Err(format!(
                "the NRRD header is longer than {MAX_HEADER_BYTES} bytes"
            )));
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end_matches(['\n', '\r']);
        if n == 0 || line.is_empty() {
            return Ok(Ok((lines, consumed as u64)));
        }
        lines.push(line.to_string());
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Encoding {
    Raw,
    Gzip,
    Text,
    Hex,
}

/// How samples are arranged in the file relative to XY planes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Layout {
    /// X and Y are the two fastest axes: each plane is contiguous.
    Planar,
    /// A channel axis of this size varies fastest, then X and Y.
    Interleaved(usize),
}

#[derive(Debug)]
struct Header {
    dtype: DType,
    sizes: Vec<usize>,
    encoding: Encoding,
    big_endian: Option<bool>,
    spacings: Option<Vec<f64>>,
    /// One entry per axis; `None` for `none`.
    directions: Option<Vec<Option<Vec<f64>>>>,
    origin: Option<Vec<f64>>,
    units: Option<Vec<String>>,
    space_units: Option<Vec<String>>,
    kinds: Option<Vec<String>>,
    labels: Option<Vec<String>>,
    axis_mins: Option<Vec<f64>>,
    centers: Option<Vec<String>>,
    line_skip: usize,
    byte_skip: i64,
    data_file: Option<String>,
    pairs: Vec<(String, String)>,
}

impl Header {
    fn parse(lines: &[String]) -> std::result::Result<Header, String> {
        let magic = lines.first().map(String::as_str).unwrap_or_default();
        match magic.strip_prefix("NRRD000") {
            Some("1" | "2" | "3" | "4" | "5") => {}
            Some(version) => return Err(format!("unsupported NRRD version {version:?}")),
            None => return Err("not an NRRD file".to_string()),
        }

        let mut fields: Vec<(String, &str)> = Vec::new();
        let mut pairs = Vec::new();
        for line in &lines[1..] {
            if line.starts_with('#') {
                continue;
            }
            let Some(colon) = line.find(':') else {
                return Err(format!("unreadable NRRD header line {line:?}"));
            };
            let (name, rest) = (&line[..colon], &line[colon + 1..]);
            if let Some(value) = rest.strip_prefix('=') {
                pairs.push((name.to_string(), unescape(value)));
            } else {
                fields.push((name.trim().to_ascii_lowercase(), rest.trim()));
            }
        }
        let field = |names: &[&str]| {
            fields
                .iter()
                .find(|(name, _)| names.contains(&name.as_str()))
                .map(|(_, value)| *value)
        };
        let required =
            |name: &str| field(&[name]).ok_or_else(|| format!("the NRRD header has no `{name}`"));

        let type_text = required("type")?;
        let dtype = parse_type(type_text)
            .ok_or_else(|| format!("unsupported NRRD sample type {type_text:?}"))?;
        let dimension: usize = required("dimension")?
            .parse()
            .ok()
            .filter(|d| (1..=16).contains(d))
            .ok_or_else(|| format!("invalid NRRD dimension {:?}", required("dimension")))?;
        let sizes_text = required("sizes")?;
        let sizes: Vec<usize> = sizes_text
            .split_whitespace()
            .map(|t| t.parse().ok().filter(|&n| n > 0))
            .collect::<Option<_>>()
            .ok_or_else(|| format!("invalid NRRD sizes {sizes_text:?}"))?;
        if sizes.len() != dimension {
            return Err(format!(
                "the NRRD header gives {} sizes for {dimension} dimensions",
                sizes.len()
            ));
        }
        let encoding = match required("encoding")?.to_ascii_lowercase().as_str() {
            "raw" => Encoding::Raw,
            "gzip" | "gz" => Encoding::Gzip,
            "txt" | "text" | "ascii" => Encoding::Text,
            "hex" => Encoding::Hex,
            other @ ("bzip2" | "bz2") => {
                return Err(format!(
                    "NRRD encoding {other:?} is not supported; resave the file with raw or gzip encoding"
                ));
            }
            other => return Err(format!("unknown NRRD encoding {other:?}")),
        };
        let big_endian = match field(&["endian"]).map(str::to_ascii_lowercase).as_deref() {
            Some("little") => Some(false),
            Some("big") => Some(true),
            Some(other) => return Err(format!("unknown NRRD endian {other:?}")),
            None => None,
        };
        if big_endian.is_none() && dtype.size() > 1 && encoding != Encoding::Text {
            return Err(format!(
                "the NRRD header has no `endian`, which {type_text} samples need"
            ));
        }

        // Per-axis lists must have one entry per axis.
        let per_axis = |name: &str, len: Option<usize>| {
            len.is_none_or(|n| n == dimension)
                .then_some(())
                .ok_or_else(|| {
                    format!(
                        "the NRRD header gives {} `{name}` for {dimension} dimensions",
                        len.unwrap_or(0)
                    )
                })
        };
        let numbers = |names: &[&str]| -> std::result::Result<Option<Vec<f64>>, String> {
            let Some(text) = field(names) else {
                return Ok(None);
            };
            let values: Vec<f64> = text
                .split_whitespace()
                .map(parse_number)
                .collect::<Option<_>>()
                .ok_or_else(|| format!("invalid NRRD `{}` {text:?}", names[0]))?;
            per_axis(names[0], Some(values.len()))?;
            Ok(Some(values))
        };
        let strings = |names: &[&str]| field(names).map(parse_strings);
        let words = |names: &[&str]| -> std::result::Result<Option<Vec<String>>, String> {
            let list: Option<Vec<String>> =
                field(names).map(|t| t.split_whitespace().map(str::to_string).collect());
            per_axis(names[0], list.as_ref().map(Vec::len))?;
            Ok(list)
        };

        let directions = field(&["space directions"])
            .map(|text| {
                parse_vectors(text).ok_or_else(|| format!("invalid NRRD space directions {text:?}"))
            })
            .transpose()?;
        per_axis("space directions", directions.as_ref().map(Vec::len))?;
        let origin = field(&["space origin"])
            .map(|text| match parse_vectors(text).as_deref() {
                Some([Some(v)]) => Ok(v.clone()),
                _ => Err(format!("invalid NRRD space origin {text:?}")),
            })
            .transpose()?;
        let units = strings(&["units"]);
        per_axis("units", units.as_ref().map(Vec::len))?;
        let labels = strings(&["labels"]);
        per_axis("labels", labels.as_ref().map(Vec::len))?;

        let integer = |names: &[&str]| -> std::result::Result<i64, String> {
            match field(names) {
                None => Ok(0),
                Some(text) => text
                    .parse()
                    .map_err(|_| format!("invalid NRRD `{}` {text:?}", names[0])),
            }
        };
        let line_skip = integer(&["line skip", "lineskip"])?;
        let line_skip = usize::try_from(line_skip)
            .map_err(|_| format!("invalid NRRD `line skip` {line_skip}"))?;

        Ok(Header {
            dtype,
            sizes,
            encoding,
            big_endian,
            spacings: numbers(&["spacings"])?,
            directions,
            origin,
            units,
            space_units: strings(&["space units"]),
            kinds: words(&["kinds"])?,
            labels,
            axis_mins: numbers(&["axis mins", "axismins"])?,
            centers: words(&["centers", "centerings"])?,
            line_skip,
            byte_skip: integer(&["byte skip", "byteskip"])?,
            data_file: field(&["data file", "datafile"]).map(str::to_string),
            pairs,
        })
    }

    /// Works out what each axis is and builds the image description.
    fn image_meta(&self, path: &Path) -> std::result::Result<(ImageMeta, Layout), String> {
        let n = self.sizes.len();
        // Every size and the total byte count must be representable before
        // anything multiplies them.
        self.sizes
            .iter()
            .try_fold(self.dtype.size(), |acc, &s| acc.checked_mul(s))
            .filter(|&bytes| bytes <= isize::MAX as usize)
            .ok_or_else(|| format!("NRRD sizes {:?} are too large", self.sizes))?;

        let kinds = self.axis_kinds();
        let position = |kind: &AxisKind| kinds.iter().position(|k| k == kind);
        let (x, y) = (position(&AxisKind::X), position(&AxisKind::Y));
        // The file's axes in the image's order, and where a missing Y goes.
        let (order, layout): (Vec<usize>, Layout) = match (x, y) {
            (Some(0), None) if n == 1 => (vec![0], Layout::Planar),
            (Some(0), Some(1)) => ((0..n).collect(), Layout::Planar),
            (Some(1), Some(2)) => {
                let mut order = vec![1, 2, 0];
                order.extend(3..n);
                (order, Layout::Interleaved(self.sizes[0]))
            }
            _ => {
                return Err(format!(
                    "unsupported NRRD axis order {kinds:?}: X and Y must be the first two axes, or follow a single channel axis"
                ));
            }
        };

        let mut axes: Vec<(Axis, usize)> = order
            .iter()
            .map(|&i| (self.axis(i, kinds[i].clone()), self.sizes[i]))
            .collect();
        if n == 1 {
            axes.push((Axis::new(AxisKind::Y), 1));
        }
        let mut meta = ImageMeta::new(self.dtype, axes).map_err(|e| e.to_string())?;
        let rgb = (0..n).any(|i| {
            kinds[i] == AxisKind::Channel
                && self.sizes[i] == 3
                && self
                    .kinds
                    .as_ref()
                    .is_some_and(|k| k[i].eq_ignore_ascii_case("RGB-color"))
        });
        if rgb {
            meta.color = ColorModel::Rgb;
        }
        meta.name = crate::bmp::file_name(path);
        for (key, value) in &self.pairs {
            meta.properties.insert(key.clone(), value.clone());
        }
        Ok((meta, layout))
    }

    fn axis_kinds(&self) -> Vec<AxisKind> {
        #[derive(Clone, PartialEq)]
        enum Hint {
            Is(AxisKind),
            Spatial,
            NotSpatial,
            Unknown,
        }
        let n = self.sizes.len();
        let label = |i: usize| self.labels.as_ref().map(|l| l[i].trim()).unwrap_or("");
        let hints: Vec<Hint> = (0..n)
            .map(|i| {
                match label(i).to_ascii_lowercase().as_str() {
                    "x" => return Hint::Is(AxisKind::X),
                    "y" => return Hint::Is(AxisKind::Y),
                    "z" => return Hint::Is(AxisKind::Z),
                    "c" | "ch" | "channel" | "channels" | "rgb" | "color" | "colour" => {
                        return Hint::Is(AxisKind::Channel);
                    }
                    "t" | "time" => return Hint::Is(AxisKind::Time),
                    "" => {}
                    _ => return Hint::Is(AxisKind::Other(label(i).to_string())),
                }
                let kind = self.kinds.as_ref().map(|k| k[i].to_ascii_lowercase());
                match kind.as_deref() {
                    Some("domain" | "space") => return Hint::Spatial,
                    Some("time") => return Hint::Is(AxisKind::Time),
                    Some("???" | "none") | None => {}
                    Some(_) => return Hint::NotSpatial,
                }
                match &self.directions {
                    Some(d) if d[i].is_none() => Hint::NotSpatial,
                    Some(_) => Hint::Spatial,
                    None => Hint::Unknown,
                }
            })
            .collect();

        // Named axes are taken first so that unnamed ones fill the gaps.
        let mut used: Vec<AxisKind> = Vec::new();
        let mut kinds: Vec<Option<AxisKind>> = vec![None; n];
        for (i, hint) in hints.iter().enumerate() {
            if let Hint::Is(kind) = hint
                && (matches!(kind, AxisKind::Other(_)) || !used.contains(kind))
            {
                used.push(kind.clone());
                kinds[i] = Some(kind.clone());
            }
        }
        for (i, hint) in hints.iter().enumerate() {
            if kinds[i].is_some() {
                continue;
            }
            let candidates: &[AxisKind] = match hint {
                Hint::Spatial => &[AxisKind::X, AxisKind::Y, AxisKind::Z],
                Hint::NotSpatial => &[AxisKind::Channel],
                _ => &[AxisKind::X, AxisKind::Y, AxisKind::Z, AxisKind::Time],
            };
            let kind = candidates
                .iter()
                .find(|k| !used.contains(k))
                .cloned()
                .unwrap_or_else(|| AxisKind::Other(format!("axis {}", i + 1)));
            used.push(kind.clone());
            kinds[i] = Some(kind);
        }
        kinds.into_iter().flatten().collect()
    }

    /// Calibration of file axis `i`.
    fn axis(&self, i: usize, kind: AxisKind) -> Axis {
        let mut axis = Axis::new(kind);
        let direction = self.directions.as_ref().and_then(|d| d[i].as_ref());
        let length = direction.map(|v| v.iter().map(|c| c * c).sum::<f64>().sqrt());
        let spacing = self.spacings.as_ref().map(|s| s[i].abs());
        if let Some(scale) = spacing
            .into_iter()
            .chain(length)
            .find(|s| s.is_finite() && *s > 0.0)
        {
            axis.scale = scale;
        }

        // `space units` and `space origin` have one entry per dimension of
        // world space; an axis is tied to the one its direction runs along.
        let world = direction.and_then(|v| {
            let mut nonzero = v.iter().enumerate().filter(|(_, c)| **c != 0.0);
            match (nonzero.next(), nonzero.next()) {
                (Some((j, _)), None) => Some(j),
                _ => None,
            }
        });
        let own_unit = self.units.as_ref().map(|u| u[i].as_str());
        let space_unit = world
            .and_then(|j| self.space_units.as_ref()?.get(j))
            .map(String::as_str);
        axis.unit = own_unit
            .into_iter()
            .chain(space_unit)
            .find(|u| !u.is_empty())
            .map(str::to_string);

        let min = self.axis_mins.as_ref().map(|m| m[i]);
        if let Some(min) = min.filter(|m| m.is_finite()) {
            let cell = self
                .centers
                .as_ref()
                .is_some_and(|c| c[i].eq_ignore_ascii_case("cell"));
            // `axis mins` is the edge of the first sample when samples are
            // cell-centred, and its centre otherwise.
            axis.origin = if cell { min + axis.scale / 2.0 } else { min };
        } else if let Some(o) = world.and_then(|j| self.origin.as_ref()?.get(j))
            && o.is_finite()
        {
            axis.origin = *o;
        }
        axis
    }
}

fn parse_type(text: &str) -> Option<DType> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(match normalized.to_ascii_lowercase().as_str() {
        "signed char" | "int8" | "int8_t" => DType::I8,
        "uchar" | "unsigned char" | "uint8" | "uint8_t" => DType::U8,
        "short" | "short int" | "signed short" | "signed short int" | "int16" | "int16_t" => {
            DType::I16
        }
        "ushort" | "unsigned short" | "unsigned short int" | "uint16" | "uint16_t" => DType::U16,
        "int" | "signed int" | "int32" | "int32_t" => DType::I32,
        "uint" | "unsigned int" | "uint32" | "uint32_t" => DType::U32,
        "longlong"
        | "long long"
        | "long long int"
        | "signed long long"
        | "signed long long int"
        | "int64"
        | "int64_t" => DType::I64,
        "ulonglong" | "unsigned long long" | "unsigned long long int" | "uint64" | "uint64_t" => {
            DType::U64
        }
        "float" => DType::F32,
        "double" => DType::F64,
        _ => return None,
    })
}

fn type_name(dtype: DType) -> Option<&'static str> {
    Some(match dtype {
        DType::I8 => "int8",
        DType::U8 => "uint8",
        DType::I16 => "int16",
        DType::U16 => "uint16",
        DType::I32 => "int32",
        DType::U32 => "uint32",
        DType::I64 => "int64",
        DType::U64 => "uint64",
        DType::F32 => "float",
        DType::F64 => "double",
        DType::C32 | DType::C64 => return None,
    })
}

/// A number as NRRD writes it; `nan` and `inf` in any case.
fn parse_number(text: &str) -> Option<f64> {
    match text.to_ascii_lowercase().as_str() {
        "nan" | "+nan" | "-nan" => Some(f64::NAN),
        "inf" | "+inf" => Some(f64::INFINITY),
        "-inf" => Some(f64::NEG_INFINITY),
        other => other.parse().ok(),
    }
}

/// Parses `(1,0,0) (0,1,0) none` into one optional vector per entry.
fn parse_vectors(text: &str) -> Option<Vec<Option<Vec<f64>>>> {
    let mut vectors = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('(') {
            let close = inner.find(')')?;
            let vector: Vec<f64> = inner[..close]
                .split(',')
                .map(|c| parse_number(c.trim()))
                .collect::<Option<_>>()?;
            vectors.push(Some(vector));
            rest = inner[close + 1..].trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            if !rest[..end].eq_ignore_ascii_case("none") {
                return None;
            }
            vectors.push(None);
            rest = rest[end..].trim_start();
        }
    }
    Some(vectors)
}

/// Parses a list of double-quoted strings, in which `\"` is a quote.
/// Unquoted words are accepted too.
fn parse_strings(text: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut s = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' if chars.peek() == Some(&'"') => {
                        chars.next();
                        s.push('"');
                    }
                    '"' => break,
                    c => s.push(c),
                }
            }
            strings.push(s);
        } else {
            let mut s = String::new();
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                s.push(c);
            }
            strings.push(s);
        }
    }
    strings
}

/// Undoes the escaping of key/value lines: `\\` and `\n`.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some('n')) => {
                chars.next();
                out.push('\n');
            }
            ('\\', Some('\\')) => {
                chars.next();
                out.push('\\');
            }
            _ => out.push(c),
        }
    }
    out
}

fn write_header(meta: &ImageMeta, type_name: &str) -> String {
    fn number(v: f64) -> String {
        if v.is_nan() {
            "nan".to_string()
        } else {
            v.to_string()
        }
    }
    fn quoted(s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\\\""))
    }
    fn join(items: impl Iterator<Item = String>) -> String {
        items.collect::<Vec<_>>().join(" ")
    }

    let axes = meta.axes();
    let is_channel = |a: &Axis| a.kind == AxisKind::Channel;
    let mut h = String::from("NRRD0004\n");
    h += &format!("type: {type_name}\n");
    h += &format!("dimension: {}\n", meta.ndim());
    h += &format!(
        "sizes: {}\n",
        join(meta.shape().iter().map(usize::to_string))
    );
    let kinds = axes.iter().map(|a| match a.kind {
        AxisKind::X | AxisKind::Y | AxisKind::Z => "domain",
        AxisKind::Time => "time",
        AxisKind::Channel if meta.color == ColorModel::Rgb => "RGB-color",
        AxisKind::Channel => "list",
        AxisKind::Other(_) => "???",
    });
    h += &format!("kinds: {}\n", join(kinds.map(str::to_string)));
    let labels = axes.iter().enumerate().map(|(i, a)| match &a.kind {
        AxisKind::X => "x".to_string(),
        AxisKind::Y => "y".to_string(),
        AxisKind::Z => "z".to_string(),
        AxisKind::Channel => "c".to_string(),
        AxisKind::Time => "t".to_string(),
        AxisKind::Other(name) if !name.trim().is_empty() => name.clone(),
        AxisKind::Other(_) => format!("axis {}", i + 1),
    });
    h += &format!("labels: {}\n", join(labels.map(|l| quoted(&l))));
    // A channel axis has no spacing.
    let spacings = axes
        .iter()
        .map(|a| number(if is_channel(a) { f64::NAN } else { a.scale }));
    h += &format!("spacings: {}\n", join(spacings));
    if axes.iter().any(|a| a.unit.is_some()) {
        let units = axes.iter().map(|a| quoted(a.unit.as_deref().unwrap_or("")));
        h += &format!("units: {}\n", join(units));
    }
    if axes.iter().any(|a| a.origin != 0.0) {
        let mins = axes
            .iter()
            .map(|a| number(if is_channel(a) { f64::NAN } else { a.origin }));
        h += &format!("axis mins: {}\n", join(mins));
        let centers = axes
            .iter()
            .map(|a| if is_channel(a) { "???" } else { "node" });
        h += &format!("centers: {}\n", join(centers.map(str::to_string)));
    }
    h += "endian: little\nencoding: raw\n";
    for (key, value) in &meta.properties {
        // A key cannot hold the separator or a line break, and must not
        // read as a comment or a field.
        let representable = !key.is_empty()
            && !key.starts_with('#')
            && !key.contains(':')
            && !key.contains(['\n', '\r']);
        if representable {
            let value = value
                .replace('\\', "\\\\")
                .replace("\r\n", "\n")
                .replace('\n', "\\n");
            h += &format!("{key}:={value}\n");
        }
    }
    h += "\n";
    h
}

/// The samples of a buffer as little-endian bytes.
fn le_bytes(buffer: &Buffer) -> Vec<u8> {
    macro_rules! bytes {
        ($s:expr) => {
            $s.iter().flat_map(|v| v.to_le_bytes()).collect()
        };
    }
    match buffer {
        Buffer::U8(s) => s.to_vec(),
        Buffer::I8(s) => bytes!(s),
        Buffer::U16(s) => bytes!(s),
        Buffer::I16(s) => bytes!(s),
        Buffer::U32(s) => bytes!(s),
        Buffer::I32(s) => bytes!(s),
        Buffer::U64(s) => bytes!(s),
        Buffer::I64(s) => bytes!(s),
        Buffer::F32(s) => bytes!(s),
        Buffer::F64(s) => bytes!(s),
        // Refused by `write` before any plane is read.
        Buffer::C32(_) | Buffer::C64(_) => Vec::new(),
    }
}

/// Decodes every `step`-th sample of `bytes`, starting with sample `first`.
fn samples(dtype: DType, bytes: &[u8], big_endian: bool, first: usize, step: usize) -> Buffer {
    macro_rules! decode {
        ($t:ty) => {{
            const SIZE: usize = std::mem::size_of::<$t>();
            let values: Vec<$t> = bytes
                .chunks_exact(SIZE)
                .skip(first)
                .step_by(step)
                .map(|chunk| {
                    let mut raw = [0u8; SIZE];
                    raw.copy_from_slice(chunk);
                    if big_endian {
                        <$t>::from_be_bytes(raw)
                    } else {
                        <$t>::from_le_bytes(raw)
                    }
                })
                .collect();
            Buffer::from_vec(values)
        }};
    }
    match dtype {
        DType::U8 => decode!(u8),
        DType::I8 => decode!(i8),
        DType::U16 => decode!(u16),
        DType::I16 => decode!(i16),
        DType::U32 => decode!(u32),
        DType::I32 => decode!(i32),
        DType::U64 => decode!(u64),
        DType::I64 => decode!(i64),
        DType::F32 => decode!(f32),
        // NRRD has no complex types, so a header never asks for one.
        DType::F64 | DType::C32 | DType::C64 => decode!(f64),
    }
}

/// A buffer of `len` zero bytes, or an error instead of an abort when a
/// header claims more than can be allocated.
fn zeroed(len: usize) -> calipix_core::Result<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| {
        calipix_core::Error::Other(format!("not enough memory for {len} bytes of NRRD samples"))
    })?;
    v.resize(len, 0);
    Ok(v)
}

fn read_at(file: &File, path: &Path, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let _ = path;
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        let _ = path;
        let (mut done, mut offset) = (0, offset);
        while done < buf.len() {
            match std::os::windows::fs::FileExt::seek_read(file, &mut buf[done..], offset) {
                Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    done += n;
                    offset += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        // No positional reads: use a private handle so that concurrent
        // readers do not share a file position.
        let _ = file;
        let mut own = File::open(path)?;
        own.seek(SeekFrom::Start(offset))?;
        own.read_exact(buf)
    }
}

/// Raw-encoded samples, read from the file a plane at a time.
#[derive(Debug)]
struct RawSource {
    meta: ImageMeta,
    file: File,
    path: PathBuf,
    start: u64,
    big_endian: bool,
    layout: Layout,
}

impl RawSource {
    fn read(&self, offset: u64, len: usize) -> calipix_core::Result<Vec<u8>> {
        let mut bytes = zeroed(len)?;
        read_at(&self.file, &self.path, self.start + offset, &mut bytes).map_err(|source| {
            calipix_core::Error::Io {
                context: self.path.display().to_string(),
                source,
            }
        })?;
        Ok(bytes)
    }
}

impl PixelSource for RawSource {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, plane: usize) -> calipix_core::Result<Buffer> {
        let dtype = self.meta.dtype();
        let plane_bytes = self.meta.plane_len() * dtype.size();
        match self.layout {
            Layout::Planar => {
                let bytes = self.read(plane as u64 * plane_bytes as u64, plane_bytes)?;
                Ok(samples(dtype, &bytes, self.big_endian, 0, 1))
            }
            Layout::Interleaved(n) => {
                // The channel axis comes right after X and Y, so it is the
                // fastest part of the plane number.
                let (group, channel) = (plane / n, plane % n);
                let group_bytes = plane_bytes * n;
                let bytes = self.read(group as u64 * group_bytes as u64, group_bytes)?;
                Ok(samples(dtype, &bytes, self.big_endian, channel, n))
            }
        }
    }

    fn read_region(&self, plane: usize, region: Rect) -> calipix_core::Result<Buffer> {
        let Layout::Planar = self.layout else {
            return crop_buffer(&self.read_plane(plane)?, self.meta.width(), region);
        };
        // Only the rows the region touches are read.
        let dtype = self.meta.dtype();
        let row_bytes = self.meta.width() * dtype.size();
        let plane_bytes = self.meta.plane_len() * dtype.size();
        let offset = plane as u64 * plane_bytes as u64 + (region.y * row_bytes) as u64;
        let bytes = self.read(offset, region.h * row_bytes)?;
        let rows = samples(dtype, &bytes, self.big_endian, 0, 1);
        let within = Rect::new(region.x, 0, region.w, region.h);
        crop_buffer(&rows, self.meta.width(), within)
    }
}

/// Samples in an encoding that cannot be read at an arbitrary position.
/// The whole file is decoded the first time a plane is asked for.
#[derive(Debug)]
struct DecodedSource {
    meta: ImageMeta,
    path: PathBuf,
    start: u64,
    /// Bytes to skip in the decoded stream (gzip) or the file (text, hex).
    byte_skip: u64,
    encoding: Encoding,
    big_endian: bool,
    layout: Layout,
    planes: OnceLock<std::result::Result<Vec<Buffer>, String>>,
}

impl DecodedSource {
    fn load(&self) -> std::result::Result<Vec<Buffer>, String> {
        let io = |e: std::io::Error| format!("{}: {e}", self.path.display());
        let mut file = BufReader::new(File::open(&self.path).map_err(io)?);
        let dtype = self.meta.dtype();
        let mut big_endian = self.big_endian;
        let mut stream: Box<dyn Read> = match self.encoding {
            Encoding::Gzip => {
                file.seek(SeekFrom::Start(self.start)).map_err(io)?;
                let mut gz = MultiGzDecoder::new(file);
                std::io::copy(&mut gz.by_ref().take(self.byte_skip), &mut std::io::sink())
                    .map_err(io)?;
                Box::new(gz)
            }
            Encoding::Text | Encoding::Hex | Encoding::Raw => {
                file.seek(SeekFrom::Start(self.start + self.byte_skip))
                    .map_err(io)?;
                let mut text = String::new();
                file.read_to_string(&mut text).map_err(io)?;
                let bytes = if self.encoding == Encoding::Hex {
                    parse_hex(&text)
                } else {
                    big_endian = false;
                    parse_text(dtype, &text)
                };
                Box::new(Cursor::new(
                    bytes.map_err(|m| format!("{}: {m}", self.path.display()))?,
                ))
            }
        };

        let plane_bytes = self.meta.plane_len() * dtype.size();
        let (group, step) = match self.layout {
            Layout::Planar => (plane_bytes, 1),
            Layout::Interleaved(n) => (plane_bytes * n, n),
        };
        let mut bytes = zeroed(group).map_err(|e| e.to_string())?;
        let mut planes = Vec::new();
        while planes.len() < self.meta.plane_count() {
            stream.read_exact(&mut bytes).map_err(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    format!(
                        "{}: the NRRD data ends before the {} planes its header describes",
                        self.path.display(),
                        self.meta.plane_count()
                    )
                } else {
                    io(e)
                }
            })?;
            for channel in 0..step {
                planes.push(samples(dtype, &bytes, big_endian, channel, step));
            }
        }
        Ok(planes)
    }
}

impl PixelSource for DecodedSource {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, plane: usize) -> calipix_core::Result<Buffer> {
        match self.planes.get_or_init(|| self.load()) {
            Ok(planes) => Ok(planes[plane].clone()),
            Err(message) => Err(calipix_core::Error::Other(message.clone())),
        }
    }
}

/// Hex-encoded bytes; white space between digits is ignored.
fn parse_hex(text: &str) -> std::result::Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(text.len() / 2);
    let mut high = None;
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        let digit = c
            .to_digit(16)
            .ok_or_else(|| format!("invalid character {c:?} in hex-encoded NRRD data"))?
            as u8;
        match high.take() {
            Some(h) => bytes.push(h << 4 | digit),
            None => high = Some(digit),
        }
    }
    Ok(bytes)
}

/// Text-encoded numbers, as the little-endian bytes of their type.
fn parse_text(dtype: DType, text: &str) -> std::result::Result<Vec<u8>, String> {
    macro_rules! parse {
        ($t:ty) => {{
            let mut bytes = Vec::new();
            for token in text.split_whitespace() {
                let value: $t = token.parse().map_err(|_| {
                    format!("invalid {dtype} value {token:?} in text-encoded NRRD data")
                })?;
                bytes.extend(value.to_le_bytes());
            }
            Ok(bytes)
        }};
    }
    match dtype {
        DType::U8 => parse!(u8),
        DType::I8 => parse!(i8),
        DType::U16 => parse!(u16),
        DType::I16 => parse!(i16),
        DType::U32 => parse!(u32),
        DType::I32 => parse!(i32),
        DType::U64 => parse!(u64),
        DType::I64 => parse!(i64),
        DType::F32 => parse!(f32),
        DType::F64 | DType::C32 | DType::C64 => parse!(f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_type_names() {
        assert_eq!(parse_type("unsigned  char"), Some(DType::U8));
        assert_eq!(parse_type("signed char"), Some(DType::I8));
        assert_eq!(parse_type("short"), Some(DType::I16));
        assert_eq!(parse_type("uint16_t"), Some(DType::U16));
        assert_eq!(parse_type("int"), Some(DType::I32));
        assert_eq!(parse_type("unsigned int"), Some(DType::U32));
        assert_eq!(parse_type("long long"), Some(DType::I64));
        assert_eq!(parse_type("ulonglong"), Some(DType::U64));
        assert_eq!(parse_type("float"), Some(DType::F32));
        assert_eq!(parse_type("double"), Some(DType::F64));
        assert_eq!(parse_type("block"), None);
        for dtype in DType::ALL {
            if let Some(name) = type_name(dtype) {
                assert_eq!(parse_type(name), Some(dtype));
            }
        }
    }

    #[test]
    fn parses_vectors_and_strings() {
        assert_eq!(
            parse_vectors("(1,0,0) (0, 2.5 ,0) none"),
            Some(vec![
                Some(vec![1.0, 0.0, 0.0]),
                Some(vec![0.0, 2.5, 0.0]),
                None
            ])
        );
        assert_eq!(parse_vectors("(1,0"), None);
        assert_eq!(parse_vectors("some"), None);
        assert_eq!(
            parse_strings(r#""mm" "a \"b\"" "" bare"#),
            vec!["mm", "a \"b\"", "", "bare"]
        );
        assert_eq!(unescape(r"a\nb\\n"), "a\nb\\n");
        assert!(parse_number("NaN").unwrap().is_nan());
    }

    #[test]
    fn parses_hex_and_text_data() {
        assert_eq!(parse_hex("00ff\n1A 2b").unwrap(), vec![0, 255, 26, 43]);
        assert!(parse_hex("0g").is_err());
        assert_eq!(
            parse_text(DType::I16, "-2 3").unwrap(),
            vec![254, 255, 3, 0]
        );
        assert!(parse_text(DType::U8, "256").is_err());
    }
}
