//! ImageJ's `.roi` file format: `ij.io.RoiDecoder` and `ij.io.RoiEncoder`.
//!
//! A file is a 64-byte header, the coordinates, a second 64-byte header
//! and then the name and other variable-length parts, all big-endian.
//! The same bytes are stored in the entries of a ROI Manager `.zip` and,
//! for the selection and overlay of an image, in its TIFF file.

use crate::model::{Coords, Frame};
use crate::path::Path;
use crate::{Error, Point, Roi};

/// The format version written: that of ImageJ 1.54p, the release results
/// are checked against.
const VERSION: i32 = 228;
const HEADER_SIZE: usize = 64;
const HEADER2_SIZE: usize = 64;

// Header offsets (RoiDecoder).
const VERSION_OFFSET: usize = 4;
const TYPE: usize = 6;
const TOP: usize = 8;
const LEFT: usize = 10;
const BOTTOM: usize = 12;
const RIGHT: usize = 14;
const N_COORDINATES: usize = 16;
const X1: usize = 18;
const Y1: usize = 22;
const X2: usize = 26;
const Y2: usize = 30;
const SIZE: usize = 18;
const STROKE_WIDTH: usize = 34;
const SHAPE_ROI_SIZE: usize = 36;
const STROKE_COLOR: usize = 40;
const FILL_COLOR: usize = 44;
const SUBTYPE: usize = 48;
const OPTIONS: usize = 50;
const ARROW_STYLE: usize = 52;
const FLOAT_PARAM: usize = 52;
const POINT_TYPE: usize = 52;
const ARROW_HEAD_SIZE: usize = 53;
const ROUNDED_RECT_ARC_SIZE: usize = 54;
const POSITION: usize = 56;
const HEADER2_OFFSET: usize = 60;
const COORDINATES: usize = 64;
// Second header offsets.
const C_POSITION: usize = 4;
const Z_POSITION: usize = 8;
const T_POSITION: usize = 12;
const NAME_OFFSET: usize = 16;
const NAME_LENGTH: usize = 20;
const OVERLAY_LABEL_COLOR: usize = 24;
const OVERLAY_FONT_SIZE: usize = 28;
const GROUP: usize = 30;
const IMAGE_SIZE: usize = 32;
const FLOAT_STROKE_WIDTH: usize = 36;
const ROI_PROPS_OFFSET: usize = 40;
const ROI_PROPS_LENGTH: usize = 44;
const COUNTERS_OFFSET: usize = 48;
const GROUP_EXTENDED: usize = 52;
// Subtypes.
const SUB_TEXT: i32 = 1;
const SUB_ARROW: i32 = 2;
const SUB_ELLIPSE: i32 = 3;
const SUB_IMAGE: i32 = 4;
const SUB_ROTATED_RECT: i32 = 5;
// Options.
const SPLINE_FIT: i32 = 1;
const DOUBLE_HEADED: i32 = 2;
const OUTLINE: i32 = 4;
const OVERLAY_LABELS: i32 = 8;
const OVERLAY_NAMES: i32 = 16;
const OVERLAY_BACKGROUNDS: i32 = 32;
const OVERLAY_BOLD: i32 = 64;
const SUB_PIXEL_RESOLUTION: i32 = 128;
const DRAW_OFFSET: i32 = 256;
const SHOW_LABELS: i32 = 1024;
const SCALE_LABELS: i32 = 2048;
const PROMPT_BEFORE_DELETING: i32 = 4096;
const SCALE_STROKE_WIDTH: i32 = 8192;
// Types.
const T_POLYGON: u8 = 0;
const T_RECT: u8 = 1;
const T_OVAL: u8 = 2;
const T_LINE: u8 = 3;
const T_FREELINE: u8 = 4;
const T_POLYLINE: u8 = 5;
const T_FREEHAND: u8 = 7;
const T_TRACED: u8 = 8;
const T_ANGLE: u8 = 9;
const T_POINT: u8 = 10;

/// A colour as ImageJ stores it: `0xAARRGGBB`, alpha 255 being opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub u32);

impl Color {
    pub fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color(0xff00_0000 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
    }

    /// Red, green, blue and alpha.
    pub fn rgba(self) -> [u8; 4] {
        [
            (self.0 >> 16) as u8,
            (self.0 >> 8) as u8,
            self.0 as u8,
            (self.0 >> 24) as u8,
        ]
    }
}

/// How an overlay labels its selections; ImageJ stores this with each
/// selection of the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OverlayOptions {
    pub labels: bool,
    pub names: bool,
    pub backgrounds: bool,
    pub bold: bool,
    pub scale_labels: bool,
    pub label_color: Option<Color>,
    pub font_size: i16,
}

/// The head of an arrow. ImageJ's arrows are straight lines with one;
/// Calipix keeps the line and carries the head's description along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrowHead {
    /// 0 filled, 1 notched, 2 open, 3 headless, 4 bar.
    pub style: u8,
    pub size: u8,
    pub double_headed: bool,
    pub outline: bool,
}

/// What ImageJ stores with a selection besides its geometry.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RoiProps {
    /// The selection's name; the ROI Manager shows it in its list.
    pub name: Option<String>,
    /// Width of the outline in pixels; 0 for the default. For line
    /// selections wider than one pixel the width is part of the
    /// selection itself ([`Roi::line_width`]) and this is ignored.
    pub stroke_width: f32,
    pub stroke_color: Option<Color>,
    pub fill_color: Option<Color>,
    /// The stack position the selection belongs to, counted from 1; 0 if
    /// it belongs to none.
    pub position: i32,
    /// Hyperstack position, each counted from 1; 0 for any.
    pub channel: i32,
    pub slice: i32,
    pub frame: i32,
    /// ImageJ's selection group, 0 for none.
    pub group: u16,
    /// Whether the file stores coordinates with fractions. Selections
    /// with fractional coordinates are always stored that way; this
    /// records it for those with whole-number coordinates, so that a file
    /// is written back the way it was read.
    pub sub_pixel: bool,
    pub draw_offset: bool,
    /// The selection is to be shown as a spline through its vertices.
    /// Calipix keeps the flag but measures the straight-edged outline.
    pub spline_fit: bool,
    pub scale_stroke_width: bool,
    /// Marker of a point selection: 0 hybrid, 1 cross, 2 dot, 3 circle.
    pub point_type: u8,
    /// Marker size of a point selection, an index from 0 (tiny) to 6.
    pub point_size: i16,
    pub show_labels: bool,
    pub prompt_before_deleting: bool,
    /// The counter each point of a multi-point selection belongs to.
    pub counters: Option<Vec<i32>>,
    pub arrow: Option<ArrowHead>,
    pub overlay: OverlayOptions,
    /// ImageJ's free-form `key: value` properties, one per line.
    pub properties: Option<String>,
}

impl Default for RoiProps {
    fn default() -> RoiProps {
        RoiProps {
            name: None,
            stroke_width: 0.0,
            stroke_color: None,
            fill_color: None,
            position: 0,
            channel: 0,
            slice: 0,
            frame: 0,
            group: 0,
            sub_pixel: false,
            draw_offset: false,
            spline_fit: false,
            scale_stroke_width: false,
            point_type: 0,
            point_size: 1,
            show_labels: false,
            prompt_before_deleting: false,
            counters: None,
            arrow: None,
            overlay: OverlayOptions::default(),
            properties: None,
        }
    }
}

/// A selection together with what ImageJ stores about it: one `.roi`
/// file, or one entry of a ROI Manager set or an overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct RoiEntry {
    pub roi: Roi,
    pub props: RoiProps,
}

impl RoiEntry {
    pub fn new(roi: Roi) -> RoiEntry {
        RoiEntry {
            roi,
            props: RoiProps::default(),
        }
    }

    pub fn named(roi: Roi, name: impl Into<String>) -> RoiEntry {
        let mut entry = RoiEntry::new(roi);
        entry.props.name = Some(name.into());
        entry
    }
}

impl From<Roi> for RoiEntry {
    fn from(roi: Roi) -> RoiEntry {
        RoiEntry::new(roi)
    }
}

struct Reader<'a> {
    data: &'a [u8],
}

impl Reader<'_> {
    fn bytes(&self, at: usize, n: usize) -> Result<&[u8], Error> {
        at.checked_add(n)
            .and_then(|end| self.data.get(at..end))
            .ok_or_else(|| {
                Error::Format(format!(
                    "the data ends at byte {} where {n} bytes at {at} were expected",
                    self.data.len()
                ))
            })
    }

    fn byte(&self, at: usize) -> Result<u8, Error> {
        Ok(self.bytes(at, 1)?[0])
    }

    fn ushort(&self, at: usize) -> Result<i32, Error> {
        let b = self.bytes(at, 2)?;
        Ok(i32::from(u16::from_be_bytes([b[0], b[1]])))
    }

    /// `RoiDecoder.getShort`: signed, except that values below -5000 are
    /// taken to be unsigned ones above 32767.
    fn short(&self, at: usize) -> Result<i32, Error> {
        let unsigned = self.ushort(at)?;
        let signed = i32::from(unsigned as u16 as i16);
        Ok(if signed < -5000 { unsigned } else { signed })
    }

    fn int(&self, at: usize) -> Result<i32, Error> {
        let b = self.bytes(at, 4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn float(&self, at: usize) -> Result<f32, Error> {
        Ok(f32::from_bits(self.int(at)? as u32))
    }

    /// A string of `length` UTF-16 units at `offset`, or `None` if it is
    /// absent or does not fit, as ImageJ treats it.
    fn string(&self, offset: i32, length: i32) -> Option<String> {
        if offset <= 0 || length <= 0 {
            return None;
        }
        let (offset, length) = (offset as usize, length as usize);
        let bytes = self.data.get(offset..offset.checked_add(length.checked_mul(2)?)?)?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

fn finite(v: f32, what: &str) -> Result<f64, Error> {
    if v.is_finite() {
        Ok(f64::from(v))
    } else {
        Err(Error::Format(format!("{what} is not a number")))
    }
}

/// Reads a selection from the bytes of an ImageJ `.roi` file.
///
/// Every kind ImageJ writes is read except text and image selections,
/// which give [`Error::Unsupported`]. An arrow is read as the straight
/// line it lies on, with its head described in [`RoiProps::arrow`].
/// Malformed data gives [`Error::Format`].
pub fn decode(data: &[u8]) -> Result<RoiEntry, Error> {
    let r = Reader { data };
    if data.len() < 4 || &data[..4] != b"Iout" {
        return Err(Error::Format(
            "the data does not start with \"Iout\"".to_string(),
        ));
    }
    if data.len() < HEADER_SIZE {
        return Err(Error::Format(format!(
            "{} bytes is shorter than the 64-byte header",
            data.len()
        )));
    }
    let size = data.len();
    let version = r.short(VERSION_OFFSET)?;
    let kind = r.byte(TYPE)?;
    let subtype = r.short(SUBTYPE)?;
    let top = r.short(TOP)?;
    let left = r.short(LEFT)?;
    let bottom = r.short(BOTTOM)?;
    let right = r.short(RIGHT)?;
    let (width, height) = (right - left, bottom - top);
    let mut n = r.ushort(N_COORDINATES)?;
    let options = r.short(OPTIONS)?;
    let hdr2 = r.int(HEADER2_OFFSET)?;
    let hdr2 = if hdr2 > 0 { Some(hdr2 as usize) } else { None };

    let mut props = RoiProps {
        position: r.int(POSITION)?,
        ..RoiProps::default()
    };
    let sub_pixel = options & SUB_PIXEL_RESOLUTION != 0 && version >= 222;
    props.sub_pixel = sub_pixel;
    props.draw_offset = sub_pixel && options & DRAW_OFFSET != 0;
    props.scale_stroke_width = if version >= 228 {
        options & SCALE_STROKE_WIDTH != 0
    } else {
        true
    };
    props.overlay = OverlayOptions {
        labels: options & OVERLAY_LABELS != 0,
        names: options & OVERLAY_NAMES != 0,
        backgrounds: options & OVERLAY_BACKGROUNDS != 0,
        bold: options & OVERLAY_BOLD != 0,
        scale_labels: options & SCALE_LABELS != 0,
        label_color: None,
        font_size: 0,
    };
    let mut image_size = 0;
    if let Some(h) = hdr2.filter(|h| h + IMAGE_SIZE + 4 <= size) {
        props.channel = r.int(h + C_POSITION)?;
        props.slice = r.int(h + Z_POSITION)?;
        props.frame = r.int(h + T_POSITION)?;
        let label_color = r.int(h + OVERLAY_LABEL_COLOR)?;
        if version >= 220 && label_color != 0 {
            props.overlay.label_color = Some(Color(label_color as u32));
        }
        props.overlay.font_size = r.short(h + OVERLAY_FONT_SIZE)? as i16;
        image_size = r.int(h + IMAGE_SIZE)?;
        let group = i32::from(r.byte(h + GROUP)?);
        props.group = if version >= 229 && group == 0 && h + GROUP_EXTENDED + 2 <= size {
            r.ushort(h + GROUP_EXTENDED)? as u16
        } else {
            group as u16
        };
        props.name = r.string(r.int(h + NAME_OFFSET)?, r.int(h + NAME_LENGTH)?);
        if version >= 224 && h + ROI_PROPS_LENGTH + 4 <= size {
            props.properties =
                r.string(r.int(h + ROI_PROPS_OFFSET)?, r.int(h + ROI_PROPS_LENGTH)?);
        }
    }
    // RoiDecoder.getStrokeWidthAndColor
    if version >= 218 {
        let mut stroke_width = r.short(STROKE_WIDTH)? as f32;
        if let Some(h) = hdr2.filter(|h| h + FLOAT_STROKE_WIDTH + 4 <= size) {
            let precise = r.float(h + FLOAT_STROKE_WIDTH)?;
            if precise > 0.0 {
                stroke_width = precise;
            }
        }
        if stroke_width > 0.0 && stroke_width.is_finite() {
            props.stroke_width = stroke_width;
        }
        let fill = r.int(FILL_COLOR)?;
        if fill != 0 {
            props.fill_color = Some(Color(fill as u32));
        }
        let stroke = r.int(STROKE_COLOR)?;
        if stroke != 0 {
            props.stroke_color = Some(Color(stroke as u32));
        }
        props.spline_fit = options & SPLINE_FIT != 0;
    }
    let line_width = if props.stroke_width > 1.0 {
        f64::from(props.stroke_width)
    } else {
        1.0
    };

    // A composite stores its path where other kinds store coordinates.
    let shape_size = r.int(SHAPE_ROI_SIZE)?;
    if shape_size > 0 {
        if kind != T_RECT {
            return Err(Error::Format(
                "a composite selection must have type 1".to_string(),
            ));
        }
        let count = shape_size as usize;
        r.bytes(COORDINATES, count.checked_mul(4).unwrap_or(usize::MAX))?;
        let array = (0..count)
            .map(|i| r.float(COORDINATES + 4 * i))
            .collect::<Result<Vec<f32>, Error>>()?;
        if array.iter().any(|v| !v.is_finite()) {
            return Err(Error::Format(
                "the composite path has coordinates that are not numbers".to_string(),
            ));
        }
        let path = Path::from_array(&array)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Error::Format("the composite path is not valid".to_string()))?;
        if !matches!(path.segments()[0], crate::Segment::MoveTo(..)) {
            return Err(Error::Format(
                "the composite path does not start with a move".to_string(),
            ));
        }
        props.spline_fit = false;
        return Ok(RoiEntry {
            roi: Roi::Composite { path },
            props,
        });
    }

    let sub_pixel_rect = version >= 223 && sub_pixel && (kind == T_RECT || kind == T_OVAL);
    let rect = |r: &Reader<'_>| -> Result<(f64, f64, f64, f64), Error> {
        if sub_pixel_rect {
            Ok((
                finite(r.float(X1)?, "x")?,
                finite(r.float(Y1)?, "y")?,
                finite(r.float(X2)?, "width")?,
                finite(r.float(Y2)?, "height")?,
            ))
        } else {
            // The Roi constructor makes every selection at least a pixel
            // wide and high.
            Ok((
                f64::from(left),
                f64::from(top),
                f64::from(width.max(1)),
                f64::from(height.max(1)),
            ))
        }
    };
    let roi = match kind {
        T_RECT => {
            if version >= 218 && subtype == SUB_TEXT {
                return Err(Error::Unsupported("text".to_string()));
            }
            if version >= 221 && subtype == SUB_IMAGE && image_size > 0 {
                return Err(Error::Unsupported("image".to_string()));
            }
            let (x, y, w, h) = rect(&r)?;
            if !(w > 0.0 && h > 0.0) {
                return Err(Error::Format("the rectangle has no size".to_string()));
            }
            let arc = r.short(ROUNDED_RECT_ARC_SIZE)?;
            if arc > 0 {
                Roi::rounded_rectangle(x, y, w, h, arc as u32)
            } else {
                Roi::rectangle(x, y, w, h)
            }
        }
        T_OVAL => {
            let (x, y, w, h) = rect(&r)?;
            if !(w > 0.0 && h > 0.0) {
                return Err(Error::Format("the oval has no size".to_string()));
            }
            Roi::oval(x, y, w, h)
        }
        T_LINE => {
            if subtype == SUB_ARROW {
                props.arrow = Some(ArrowHead {
                    style: r.byte(ARROW_STYLE)?,
                    size: r.byte(ARROW_HEAD_SIZE)?,
                    double_headed: options & DOUBLE_HEADED != 0,
                    outline: options & OUTLINE != 0,
                });
            }
            Roi::Line {
                x1: finite(r.float(X1)?, "x1")?,
                y1: finite(r.float(Y1)?, "y1")?,
                x2: finite(r.float(X2)?, "x2")?,
                y2: finite(r.float(Y2)?, "y2")?,
                width: line_width,
            }
        }
        T_POLYGON | T_FREEHAND | T_TRACED | T_POLYLINE | T_FREELINE | T_ANGLE | T_POINT => {
            if n == 0 {
                n = r.int(SIZE)?;
            }
            if n <= 0 {
                return Err(Error::Format("the selection has no points".to_string()));
            }
            let n = n as usize;
            let needed = n
                .checked_mul(if sub_pixel { 12 } else { 4 })
                .and_then(|c| c.checked_add(COORDINATES))
                .unwrap_or(usize::MAX);
            if needed > size {
                return Err(Error::Format(format!(
                    "{n} points need {needed} bytes but there are {size}"
                )));
            }
            let points: Vec<Point> = if sub_pixel {
                let (base1, base2) = (COORDINATES + 4 * n, COORDINATES + 8 * n);
                (0..n)
                    .map(|i| {
                        Ok((
                            finite(r.float(base1 + 4 * i)?, "a coordinate")?,
                            finite(r.float(base2 + 4 * i)?, "a coordinate")?,
                        ))
                    })
                    .collect::<Result<_, Error>>()?
            } else {
                let (base1, base2) = (COORDINATES, COORDINATES + 2 * n);
                (0..n)
                    .map(|i| {
                        Ok((
                            f64::from(left + r.short(base1 + 2 * i)?.max(0)),
                            f64::from(top + r.short(base2 + 2 * i)?.max(0)),
                        ))
                    })
                    .collect::<Result<_, Error>>()?
            };
            let at_least = |points: Vec<Point>, min: usize, what: &str| {
                if points.len() >= min {
                    Ok(points)
                } else {
                    Err(Error::Format(format!(
                        "{what} needs at least {min} points but has {}",
                        points.len()
                    )))
                }
            };
            match kind {
                T_POINT => {
                    if version >= 226 {
                        props.point_type = r.byte(POINT_TYPE)?;
                        props.point_size = r.short(STROKE_WIDTH)? as i16;
                    }
                    props.show_labels = options & SHOW_LABELS != 0;
                    props.prompt_before_deleting = options & PROMPT_BEFORE_DELETING != 0;
                    props.stroke_width = 0.0;
                    props.spline_fit = false;
                    if let (true, Some(h)) = (version >= 227, hdr2) {
                        let offset = r.int(h + COUNTERS_OFFSET).unwrap_or(0);
                        if offset > 0 && offset as usize + n * 4 <= size {
                            props.counters = Some(
                                (0..n)
                                    .map(|i| r.int(offset as usize + 4 * i))
                                    .collect::<Result<_, Error>>()?,
                            );
                        }
                    }
                    Roi::Points { points }
                }
                T_FREEHAND if subtype == SUB_ELLIPSE || subtype == SUB_ROTATED_RECT => {
                    let (x1, y1) = (finite(r.float(X1)?, "x1")?, finite(r.float(Y1)?, "y1")?);
                    let (x2, y2) = (finite(r.float(X2)?, "x2")?, finite(r.float(Y2)?, "y2")?);
                    let param = finite(r.float(FLOAT_PARAM)?, "the shape parameter")?;
                    if x1 == x2 && y1 == y2 {
                        return Err(Error::Format("the shape has no length".to_string()));
                    }
                    if subtype == SUB_ROTATED_RECT {
                        Roi::rotated_rectangle(x1, y1, x2, y2, param)
                    } else {
                        Roi::ellipse(x1, y1, x2, y2, param.clamp(0.0, 1.0))
                    }
                }
                T_POLYGON => Roi::Polygon {
                    points: at_least(points, 3, "a polygon")?,
                },
                T_FREEHAND => Roi::Freehand {
                    points: at_least(points, 3, "a freehand selection")?,
                },
                T_TRACED => Roi::Traced {
                    points: at_least(points, 3, "a traced selection")?,
                },
                T_POLYLINE => Roi::Polyline {
                    points: at_least(points, 2, "a line")?,
                    width: line_width,
                },
                T_FREELINE => Roi::Freeline {
                    points: at_least(points, 2, "a line")?,
                    width: line_width,
                },
                _ => {
                    let &[a, b, c] = points.as_slice() else {
                        return Err(Error::Format(format!(
                            "an angle has 3 points, not {}",
                            points.len()
                        )));
                    };
                    Roi::Angle { points: [a, b, c] }
                }
            }
        }
        other => {
            return Err(Error::Format(format!(
                "{other} is not a selection type"
            )));
        }
    };
    if !matches!(
        roi,
        Roi::Polygon { .. } | Roi::Freehand { .. } | Roi::Polyline { .. } | Roi::Freeline { .. }
    ) {
        props.spline_fit = false;
    }
    // The sub-pixel flag only says something for selections that could
    // also have been stored as whole numbers.
    props.sub_pixel = props.sub_pixel
        && match &roi {
            Roi::Rectangle {
                x,
                y,
                width,
                height,
            }
            | Roi::RoundedRectangle {
                x,
                y,
                width,
                height,
                ..
            }
            | Roi::Oval {
                x,
                y,
                width,
                height,
            } => [x, y, width, height].iter().all(|v| v.fract() == 0.0),
            Roi::Polygon { points }
            | Roi::Freehand { points }
            | Roi::Traced { points }
            | Roi::Polyline { points, .. }
            | Roi::Freeline { points, .. } => crate::model::all_whole(points),
            Roi::Angle { points } => crate::model::all_whole(points),
            _ => false,
        };
    // A line's width is held by the selection itself.
    if roi.is_line() && roi.line_width() > 1.0 {
        props.stroke_width = 0.0;
    }
    Ok(RoiEntry { roi, props })
}

/// Reads a selection stored as hexadecimal digits, as the TIFF reader
/// keeps an ImageJ file's selection and overlay in the image properties
/// `imagej.roi` and `imagej.overlay.N`.
pub fn decode_hex(hex: &str) -> Result<RoiEntry, Error> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 || !hex.is_ascii() {
        return Err(Error::Format(
            "expected an even number of hexadecimal digits".to_string(),
        ));
    }
    let bytes = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| Error::Format("expected hexadecimal digits".to_string()))?;
    decode(&bytes)
}

struct Writer {
    data: Vec<u8>,
}

impl Writer {
    fn put_byte(&mut self, at: usize, v: i32) {
        self.data[at] = v as u8;
    }

    fn put_short(&mut self, at: usize, v: i32) {
        self.data[at..at + 2].copy_from_slice(&(v as u16).to_be_bytes());
    }

    fn put_int(&mut self, at: usize, v: i32) {
        self.data[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }

    fn put_float(&mut self, at: usize, v: f32) {
        self.data[at..at + 4].copy_from_slice(&v.to_bits().to_be_bytes());
    }

    fn put_string(&mut self, at: usize, units: &[u16]) {
        for (i, u) in units.iter().enumerate() {
            self.data[at + 2 * i..at + 2 * i + 2].copy_from_slice(&u.to_be_bytes());
        }
    }
}

/// Writes a selection as the bytes of an ImageJ `.roi` file, the way
/// ImageJ 1.54p's `RoiEncoder` writes it.
///
/// Fails for a polyline, freeline or angle with more than 65535 points,
/// which the format cannot hold. A polygon that large is written as a
/// composite, as ImageJ does.
pub fn encode(entry: &RoiEntry) -> Result<Vec<u8>, Error> {
    let (roi, props) = (&entry.roi, &entry.props);
    let frame = roi.frame();
    let too_large = frame.w > 60000 || frame.h > 60000 || frame.x > 60000 || frame.y > 60000;
    let name: Vec<u16> = props
        .name
        .as_deref()
        .map(|s| s.encode_utf16().collect())
        .unwrap_or_default();
    let properties: Vec<u16> = props
        .properties
        .as_deref()
        .map(|s| s.encode_utf16().collect())
        .unwrap_or_default();
    let mut options = 0;
    if props.scale_stroke_width {
        options |= SCALE_STROKE_WIDTH;
    }
    let overlay_options = |options: i32| {
        let o = &props.overlay;
        options
            | if o.labels { OVERLAY_LABELS } else { 0 }
            | if o.names { OVERLAY_NAMES } else { 0 }
            | if o.backgrounds { OVERLAY_BACKGROUNDS } else { 0 }
            | if o.bold { OVERLAY_BOLD } else { 0 }
            | if o.scale_labels { SCALE_LABELS } else { 0 }
    };
    // The stroke width ImageJ holds: a line's width is its stroke width.
    let stroke_width = if roi.is_line() && roi.line_width() > 1.0 {
        roi.line_width() as f32
    } else if roi.is_points() {
        0.0
    } else {
        props.stroke_width
    };

    let header = |w: &mut Writer, kind: u8, frame: Frame| {
        w.data[..4].copy_from_slice(b"Iout");
        w.put_short(VERSION_OFFSET, VERSION);
        w.data[TYPE] = kind;
        w.put_short(TOP, frame.y);
        w.put_short(LEFT, frame.x);
        w.put_short(BOTTOM, frame.y.wrapping_add(frame.h));
        w.put_short(RIGHT, frame.x.wrapping_add(frame.w));
        w.put_int(POSITION, props.position);
    };
    let stroke_and_colors = |w: &mut Writer| {
        if stroke_width > 0.0 {
            w.put_short(STROKE_WIDTH, stroke_width as i32);
        }
        if let Some(c) = props.stroke_color {
            w.put_int(STROKE_COLOR, c.0 as i32);
        }
        if let Some(c) = props.fill_color {
            w.put_int(FILL_COLOR, c.0 as i32);
        }
    };
    let header2 = |w: &mut Writer, h: usize, counters: &[i32]| {
        w.put_int(HEADER2_OFFSET, h as i32);
        w.put_int(h + C_POSITION, props.channel);
        w.put_int(h + Z_POSITION, props.slice);
        w.put_int(h + T_POSITION, props.frame);
        if let Some(c) = props.overlay.label_color {
            w.put_int(h + OVERLAY_LABEL_COLOR, c.0 as i32);
        }
        w.put_short(h + OVERLAY_FONT_SIZE, i32::from(props.overlay.font_size));
        if !name.is_empty() {
            w.put_int(h + NAME_OFFSET, (h + HEADER2_SIZE) as i32);
            w.put_int(h + NAME_LENGTH, name.len() as i32);
            w.put_string(h + HEADER2_SIZE, &name);
        }
        w.put_float(h + FLOAT_STROKE_WIDTH, stroke_width);
        if !properties.is_empty() {
            let offset = h + HEADER2_SIZE + 2 * name.len();
            w.put_int(h + ROI_PROPS_OFFSET, offset as i32);
            w.put_int(h + ROI_PROPS_LENGTH, properties.len() as i32);
            w.put_string(offset, &properties);
        }
        if !counters.is_empty() {
            let offset = h + HEADER2_SIZE + 2 * name.len() + 2 * properties.len();
            w.put_int(h + COUNTERS_OFFSET, offset as i32);
            for (i, &c) in counters.iter().enumerate() {
                w.put_int(offset + 4 * i, c);
            }
        }
        // Version 228 holds the group in one byte.
        w.put_byte(h + GROUP, i32::from(props.group.min(255)));
    };
    let tail = 2 * name.len() + 2 * properties.len();

    let poly = roi.poly();
    let n = poly.as_ref().map_or(0, |p| p.len());
    let as_shape = n > 65535 && matches!(roi, Roi::Polygon { .. } | Roi::Freehand { .. } | Roi::Traced { .. });
    if let (Roi::Composite { .. }, _) | (_, true) = (roi, as_shape) {
        // RoiEncoder.saveShapeRoi
        let path = match roi {
            Roi::Composite { path } => path.clone(),
            _ => roi
                .shape_path()
                .ok_or_else(|| Error::Encode("the selection has no outline".to_string()))?,
        };
        let frame = path.frame();
        // ShapeRoi.getShapeAsArray: stored relative to the frame and
        // offset back, both in single precision.
        let array = path
            .local(frame)
            .mapped(|x, y| (x + frame.x as f32, y + frame.y as f32))
            .to_array();
        let h = HEADER_SIZE + 4 * array.len();
        let mut w = Writer {
            data: vec![0; h + HEADER2_SIZE + tail],
        };
        header(&mut w, T_RECT, frame);
        w.put_int(SHAPE_ROI_SIZE, array.len() as i32);
        stroke_and_colors(&mut w);
        w.put_short(OPTIONS, overlay_options(options));
        for (i, &v) in array.iter().enumerate() {
            w.put_float(COORDINATES + 4 * i, v);
        }
        header2(&mut w, h, &[]);
        return Ok(w.data);
    }

    let kind = match roi {
        Roi::Polygon { .. } | Roi::RotatedRectangle { .. } => T_POLYGON,
        Roi::Freehand { .. } | Roi::Ellipse { .. } => T_FREEHAND,
        Roi::Traced { .. } => T_TRACED,
        Roi::Oval { .. } => T_OVAL,
        Roi::Line { .. } => T_LINE,
        Roi::Polyline { .. } => T_POLYLINE,
        Roi::Freeline { .. } => T_FREELINE,
        Roi::Angle { .. } => T_ANGLE,
        Roi::Points { .. } => T_POINT,
        Roi::Rectangle { .. } | Roi::RoundedRectangle { .. } | Roi::Composite { .. } => T_RECT,
    };
    // RotatedRectRoi is a POLYGON in ImageJ but is written as freehand
    // with a subtype, which is how the decoder recognises it.
    let kind = if matches!(roi, Roi::RotatedRectangle { .. }) {
        T_FREEHAND
    } else {
        kind
    };
    if n > 65535 && kind != T_POINT {
        return Err(Error::Encode(format!(
            "a {} with {n} points is more than the 65535 the format holds",
            roi.kind()
        )));
    }
    // Coordinates: integers relative to the frame, then, for sub-pixel
    // selections, floats in image coordinates.
    let (ints, floats) = match &poly {
        None => (None, None),
        Some(p) => {
            let float = matches!(p.coords, Coords::Float(..))
                || props.sub_pixel
                || too_large
                || matches!(
                    roi,
                    Roi::Points { .. } | Roi::Ellipse { .. } | Roi::RotatedRectangle { .. }
                );
            (Some(p.relative_int()), float.then(|| p.float_polygon()))
        }
    };
    let float_size = if floats.is_some() { 8 * n } else { 0 };
    let counters: &[i32] = match (&props.counters, roi) {
        (Some(c), Roi::Points { .. }) if c.len() >= n => &c[..n],
        _ => &[],
    };
    let h = HEADER_SIZE + 4 * n + float_size;
    let mut w = Writer {
        data: vec![0; h + HEADER2_SIZE + tail + 4 * counters.len()],
    };
    header(&mut w, kind, frame);
    if floats.is_some() {
        options |= SUB_PIXEL_RESOLUTION;
        if props.draw_offset {
            options |= DRAW_OFFSET;
        }
    }
    if let Roi::Rectangle {
        x,
        y,
        width,
        height,
    }
    | Roi::RoundedRectangle {
        x,
        y,
        width,
        height,
        ..
    }
    | Roi::Oval {
        x,
        y,
        width,
        height,
    } = roi
    {
        let whole = [x, y, width, height].iter().all(|v| v.fract() == 0.0);
        if props.sub_pixel || !whole || too_large {
            // The corners of Roi.getFloatPolygon(), in single precision.
            let (x0, x1) = (*x as f32, (x + width) as f32);
            let (y0, y1) = (*y as f32, (y + height) as f32);
            w.put_float(X1, x0);
            w.put_float(Y1, y0);
            w.put_float(X2, x1 - x0);
            w.put_float(Y2, y1 - y0);
            options |= SUB_PIXEL_RESOLUTION;
        }
    }
    if kind == T_POINT && n > 65535 {
        w.put_int(SIZE, n as i32);
    } else {
        w.put_short(N_COORDINATES, n as i32);
    }
    match roi {
        Roi::RoundedRectangle {
            corner_diameter, ..
        } => w.put_short(ROUNDED_RECT_ARC_SIZE, *corner_diameter as i32),
        Roi::Line { x1, y1, x2, y2, .. } => {
            w.put_float(X1, *x1 as f32);
            w.put_float(Y1, *y1 as f32);
            w.put_float(X2, *x2 as f32);
            w.put_float(Y2, *y2 as f32);
            if let Some(arrow) = props.arrow {
                w.put_short(SUBTYPE, SUB_ARROW);
                if arrow.double_headed {
                    options |= DOUBLE_HEADED;
                }
                if arrow.outline {
                    options |= OUTLINE;
                }
                w.put_byte(ARROW_STYLE, i32::from(arrow.style));
                w.put_byte(ARROW_HEAD_SIZE, i32::from(arrow.size));
            } else if props.draw_offset {
                options |= SUB_PIXEL_RESOLUTION + DRAW_OFFSET;
            }
        }
        Roi::Points { .. } => {
            w.put_byte(POINT_TYPE, i32::from(props.point_type));
            w.put_short(STROKE_WIDTH, i32::from(props.point_size));
            if props.show_labels {
                options |= SHOW_LABELS;
            }
            if props.prompt_before_deleting {
                options |= PROMPT_BEFORE_DELETING;
            }
        }
        Roi::Ellipse { aspect_ratio, .. } => {
            let p = roi.axis_params().expect("an ellipse has axis end points");
            w.put_short(SUBTYPE, SUB_ELLIPSE);
            for (at, v) in [X1, Y1, X2, Y2].into_iter().zip(p) {
                w.put_float(at, v as f32);
            }
            w.put_float(FLOAT_PARAM, aspect_ratio.clamp(0.0, 1.0) as f32);
        }
        Roi::RotatedRectangle { width, .. } => {
            let p = roi
                .axis_params()
                .expect("a rotated rectangle has axis end points");
            w.put_short(SUBTYPE, SUB_ROTATED_RECT);
            for (at, v) in [X1, Y1, X2, Y2].into_iter().zip(p) {
                w.put_float(at, v as f32);
            }
            w.put_float(FLOAT_PARAM, *width as f32);
        }
        _ => {}
    }
    stroke_and_colors(&mut w);
    if props.spline_fit
        && matches!(
            roi,
            Roi::Polygon { .. } | Roi::Freehand { .. } | Roi::Polyline { .. } | Roi::Freeline { .. }
        )
    {
        options |= SPLINE_FIT;
    }
    header2(&mut w, h, counters);
    if let Some((xs, ys)) = &ints {
        for i in 0..n {
            w.put_short(COORDINATES + 2 * i, xs[i]);
            w.put_short(COORDINATES + 2 * n + 2 * i, ys[i]);
        }
    }
    if let Some((xs, ys)) = &floats {
        for i in 0..n {
            w.put_float(COORDINATES + 4 * n + 4 * i, xs[i]);
            w.put_float(COORDINATES + 8 * n + 4 * i, ys[i]);
        }
    }
    w.put_short(OPTIONS, overlay_options(options));
    Ok(w.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_data_is_an_error() {
        assert!(matches!(decode(b""), Err(Error::Format(_))));
        assert!(matches!(decode(b"Iout"), Err(Error::Format(_))));
        assert!(matches!(decode(&[0u8; 200]), Err(Error::Format(_))));
        let mut data = encode(&RoiEntry::new(Roi::polygon(vec![
            (1.0, 1.0),
            (9.0, 2.0),
            (4.0, 8.0),
        ])))
        .unwrap();
        data[TYPE] = 6;
        assert!(matches!(decode(&data), Err(Error::Format(_))));
        assert!(decode_hex("zz").is_err());
        assert!(decode_hex("496").is_err());
    }

    #[test]
    fn names_and_colours_survive() {
        let mut entry = RoiEntry::named(Roi::oval(3.0, 4.0, 10.0, 6.0), "nucleus \u{3b1}");
        entry.props.stroke_color = Some(Color::rgb(255, 0, 0));
        entry.props.fill_color = Some(Color(0x4000_ff00));
        entry.props.stroke_width = 2.5;
        entry.props.position = 3;
        entry.props.group = 7;
        entry.props.properties = Some("kind: test\n".to_string());
        let bytes = encode(&entry).unwrap();
        assert_eq!(decode(&bytes), Ok(entry.clone()));
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(decode_hex(&hex), Ok(entry));
    }
}
