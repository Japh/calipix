//! Geometry commands: cropping, duplicating, scaling, the canvas, flips,
//! rotations and translation.
//!
//! Flip, Rotate and Translate change samples without changing the shape of
//! the image and follow the shared plane scope. The others change the shape
//! and so act on every plane, or make a new image.

use calipix_command::{
    CommandCtx, CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs, ParamType, Params,
    Registry, Result, Workspace, typed,
};
use calipix_core::{Axis, AxisKind, Buffer, ColorModel, DType, Image, ImageMeta, Rect};
use calipix_ops::transform::{self, Interpolation};
use calipix_roi::{Combine, Roi};

use crate::InputOnly;
use crate::convert::{axes_of, rebuilt};
use crate::process::{Extent, PlaneScope};

fn modifies() -> Effects {
    Effects {
        image: ImageEffect::Modifies,
        ..Effects::default()
    }
}

fn creates() -> Effects {
    Effects {
        image: ImageEffect::Creates,
        ..Effects::default()
    }
}

/// The plane on view. Interfaces fill these in from the view.
#[derive(Params, Clone, Copy)]
struct Position {
    /// Channel on view, counted from 1; 0 means every channel.
    #[param(default = 0, min = 0, advanced, no_persist, position = "channel")]
    channel: i64,
    /// Slice on view, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "slice")]
    slice: i64,
    /// Frame on view, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "frame")]
    frame: i64,
}

impl Position {
    /// The planes on view of `input`: one, or the channels shown together.
    fn planes(&self, input: DatasetId, meta: &ImageMeta) -> Result<Vec<usize>> {
        PlaneScope {
            input,
            channel: self.channel,
            slice: self.slice,
            frame: self.frame,
            stack: false,
        }
        .planes(meta)
    }
}

fn interpolation(name: &str) -> Result<Interpolation> {
    Interpolation::from_name(name).ok_or_else(|| Error::InvalidArg {
        param: "interpolation".to_string(),
        reason: format!("`{name}` is not None, Bilinear or Bicubic"),
    })
}

/// Math.round.
fn round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

/// ImageJ's `Calibration.scaled()`: true once any pixel size or the unit
/// has been set.
fn scaled(meta: &ImageMeta) -> bool {
    let axes = meta.axes();
    let z = meta.axis_index(&AxisKind::Z).map(|i| &axes[i]);
    axes[0].scale != 1.0
        || axes[1].scale != 1.0
        || z.is_some_and(|z| z.scale != 1.0)
        || axes[0].unit.is_some()
}

/// The pixel index of physical zero along an axis: ImageJ's `xOrigin`.
fn pixel_origin(axis: &Axis) -> f64 {
    if axis.origin == 0.0 || axis.scale == 0.0 {
        0.0
    } else {
        -axis.origin / axis.scale
    }
}

fn set_pixel_origin(axis: &mut Axis, pixels: f64) {
    axis.origin = if pixels == 0.0 {
        0.0
    } else {
        -pixels * axis.scale
    };
}

/// ImageJ moves the origin with the image only if one has been set.
fn has_origin(meta: &ImageMeta) -> bool {
    meta.axes()[0].origin != 0.0 || meta.axes()[1].origin != 0.0
}

/// The pixel rectangle of the image's area selection, if it has one.
fn selection_rect(ctx: &CommandCtx<'_>, id: DatasetId) -> Result<Option<Rect>> {
    let dataset = ctx.workspace().get(id)?;
    let meta = dataset.image.meta();
    match dataset.roi.as_ref().filter(|roi| roi.is_area()) {
        None => Ok(None),
        Some(roi) => roi
            .pixel_bounds(meta.width(), meta.height())
            .map(Some)
            .ok_or_else(|| Error::failed("the selection is outside the image")),
    }
}

/// What Crop and Duplicate take from an image with an area selection
/// (`Resizer.run`, `Duplicator.cropRoi`): the rectangle of pixels, and the
/// selection the result keeps.
///
/// A rectangle (rounded or not) is used up: the result has no selection.
/// Any other area keeps its shape on the result, moved so that its bounds
/// start at the corner (`Roi.setLocation(0, 0)`, which also drops the
/// fraction of a sub-pixel selection's position). A selection reaching
/// beyond the image is first cut to the image.
fn crop_region(roi: &Roi, width: usize, height: usize) -> Result<(Rect, Option<Roi>)> {
    let outside = || Error::failed("the selection is outside the image");
    if matches!(roi, Roi::Rectangle { .. } | Roi::RoundedRectangle { .. }) {
        let rect = roi.pixel_bounds(width, height).ok_or_else(outside)?;
        return Ok((rect, None));
    }
    let (x, y, w, h) = roi.int_bounds();
    let inside = x >= 0
        && y >= 0
        && i64::from(x) + i64::from(w) <= width as i64
        && i64::from(y) + i64::from(h) <= height as i64;
    let roi = if inside {
        roi.clone()
    } else {
        // ImageJ intersects the outlines; Calipix the pixels covered,
        // which gives an outline along pixel edges around the same pixels.
        Roi::rectangle(0.0, 0.0, width as f64, height as f64)
            .combined(roi, Combine::And)
            .ok_or_else(outside)?
    };
    let rect = roi.pixel_bounds(width, height).ok_or_else(outside)?;
    let (left, top, _, _) = roi.bounds();
    Ok((rect, Some(roi.translated(-left, -top))))
}

/// Metadata like `meta` for planes of another size, with `edit` applied to
/// its X and Y axes.
fn resized_meta(
    meta: &ImageMeta,
    width: usize,
    height: usize,
    edit: impl FnOnce(&mut Axis, &mut Axis),
) -> Result<ImageMeta> {
    let mut axes = axes_of(meta);
    axes[0].1 = width;
    axes[1].1 = height;
    let (x, y) = axes.split_at_mut(1);
    edit(&mut x[0].0, &mut y[0].0);
    let mut out = rebuilt(meta, meta.dtype(), axes)?;
    out.color = meta.color;
    out.channels = meta.channels.clone();
    Ok(out)
}

/// Every plane of `image` passed through `change`, with progress and
/// cancellation.
fn each_plane(
    ctx: &CommandCtx<'_>,
    image: &Image,
    verb: &str,
    change: impl Fn(usize, &Buffer) -> calipix_core::Result<Buffer>,
) -> Result<Vec<Buffer>> {
    let count = image.meta().plane_count();
    (0..count)
        .map(|plane| {
            ctx.check_cancelled()?;
            ctx.progress(plane as f64 / count as f64, verb);
            Ok(change(plane, &image.read_plane(plane)?)?)
        })
        .collect()
}

/// A name no open image has: `name` itself, or `name` with `-1`, `-2`, ...
/// before its extension (ImageJ's `WindowManager.getUniqueName`).
fn unique_name(workspace: &Workspace, name: &str) -> String {
    let taken = |candidate: &str| {
        workspace
            .datasets()
            .any(|d| d.image.meta().name == candidate)
    };
    if !taken(name) {
        return name.to_string();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    (1..)
        .map(|n| format!("{stem}-{n}{extension}"))
        .find(|candidate| !taken(candidate))
        .expect("some number is free")
}

/// A view kept in memory if its source was, so that reads stay immediate.
fn settle(view: Image, source: &Image) -> Result<Image> {
    Ok(if source.is_in_memory() {
        view.materialize()?
    } else {
        view
    })
}

/// `image` cropped to `rect`. The calibration origin moves with the image
/// if one has been set, as in ImageJ (`Resizer.run`, `Duplicator.run`).
fn cropped(image: &Image, rect: Rect) -> Result<Image> {
    let meta = image.meta();
    if rect == Rect::new(0, 0, meta.width(), meta.height()) {
        return Ok(image.clone());
    }
    let view = image.crop(rect)?;
    if has_origin(meta) {
        return Ok(view);
    }
    // The view moved the origin; an image without one keeps none.
    let mut fixed = view.meta().clone();
    for axis in 0..2 {
        if let Some(axis) = fixed.axis_mut(axis) {
            axis.origin = 0.0;
        }
    }
    Ok(view.with_meta(fixed)?)
}

// ---------------------------------------------------------------- crop

fn crop(ctx: &mut CommandCtx<'_>, p: InputOnly) -> Result<Outputs> {
    let dataset = ctx.workspace().get(p.input)?;
    let Some(roi) = dataset.roi.as_ref().filter(|roi| roi.is_area()) else {
        return Err(Error::failed(
            "Crop needs an area selection; select the part of the image to keep first",
        ));
    };
    let image = dataset.image.clone();
    let (rect, kept) = crop_region(roi, image.meta().width(), image.meta().height())?;
    let result = settle(cropped(&image, rect)?, &image)?;
    ctx.workspace_mut().set_image(p.input, result)?;
    ctx.workspace_mut().set_roi(p.input, kept)?;
    Ok(Outputs::new())
}

// ----------------------------------------------------------- duplicate

#[derive(Params)]
struct DuplicateParams {
    /// The image to copy.
    #[param(label = "Image")]
    input: DatasetId,
    /// Name of the copy. Left empty, it is the image's name with a number
    /// added.
    #[param(label = "Title", default = "", no_persist, ijm = "title")]
    title: String,
    /// Copy the whole image even if part of it is selected.
    #[param(label = "Ignore selection", default = false, ijm = "ignore")]
    ignore_selection: bool,
    /// Copy every plane (or the ranges given below) rather than only the
    /// plane on view.
    #[param(
        label = "Duplicate stack",
        default = false,
        ijm = "duplicate",
        position = "stack"
    )]
    stack: bool,
    /// Channels to copy, such as `2` or `1-3`; all of them if empty.
    #[param(label = "Channels", default = "", no_persist, ijm = "channels")]
    channels: String,
    /// Slices to copy, such as `5` or `2-10`; all of them if empty.
    #[param(label = "Slices", default = "", no_persist, ijm = "slices")]
    slices: String,
    /// Frames to copy, such as `1` or `3-8`; all of them if empty.
    #[param(label = "Frames", default = "", no_persist, ijm = "frames")]
    frames: String,
    /// For an image with one stack axis: the planes to copy, such as `2-4`.
    #[param(label = "Range", default = "", no_persist, advanced, ijm = "range")]
    range: String,
    #[param(flatten)]
    position: Position,
}

/// `first-last` or a single number, counted from 1, as ImageJ reads it:
/// out of range or reversed means everything. Returns start (from 0) and
/// length.
fn parse_range(text: &str, size: usize, param: &str) -> Result<(usize, usize)> {
    let text = text.trim();
    if text.is_empty() {
        return Ok((0, size));
    }
    let bad = || Error::InvalidArg {
        param: param.to_string(),
        reason: format!("`{text}` is not a range such as 2-5"),
    };
    let numbers = text
        .split(['-', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<f64>().map_err(|_| bad()))
        .collect::<Result<Vec<_>>>()?;
    let (first, last) = match numbers[..] {
        [one] => (one, one),
        [first, last] => (first, last),
        _ => return Err(bad()),
    };
    let first = (first as i64).max(1);
    let last = (last as i64).min(size as i64);
    if first > last {
        return Ok((0, size));
    }
    Ok((first as usize - 1, (last - first + 1) as usize))
}

/// `Duplicator.straightenRotatedRect`: the copy ImageJ makes of an image
/// with a rotated rectangle selected is turned so that the rectangle lies
/// straight, and cut to its length and width.
///
/// `copy` is the duplicate so far (the selection's bounding rectangle, or
/// the whole image when the selection is ignored) and `source` the size of
/// the image it came from. As in ImageJ, the copy is put on a square
/// canvas as wide as the rectangle's diagonal, rotated about the middle
/// with bilinear interpolation, moved to make up for the part of the
/// bounds that lay outside the image, and cropped about the middle.
fn straightened(
    ctx: &CommandCtx<'_>,
    copy: &Image,
    roi: &Roi,
    source: (usize, usize),
) -> Result<Image> {
    let corners = roi.outline();
    let (left, top, bounds_width, bounds_height) = roi.int_bounds();
    let side = |a: usize, b: usize| {
        // Differences of single-precision vertices, as in ImageJ.
        let dx = f64::from(corners[b].0 as f32 - corners[a].0 as f32);
        let dy = f64::from(corners[b].1 as f32 - corners[a].1 as f32);
        (dx, dy)
    };
    let ((dx1, dy1), (dx2, dy2)) = (side(0, 1), side(1, 2));
    let rr_width = (dx1 * dx1 + dy1 * dy1).sqrt();
    let rr_height = (dx2 * dx2 + dy2 * dy2).sqrt();
    let diagonal = (rr_width * rr_width + rr_height * rr_height).sqrt();
    let phi1 = -dy1.atan2(dx1);
    let angle = phi1 * 180.0 / std::f64::consts::PI;

    // The part of the bounds inside the image, and how far its middle is
    // from the middle of the rectangle.
    let used_left = f64::from(left.max(0));
    let used_right = (f64::from(left) + f64::from(bounds_width)).min(source.0 as f64);
    let used_top = f64::from(top.max(0));
    let used_bottom = (f64::from(top) + f64::from(bounds_height)).min(source.1 as f64);
    let dx3 = f64::from(left + bounds_width / 2) - (used_left + used_right) / 2.0;
    let dy3 = f64::from(top + bounds_height / 2) - (used_top + used_bottom) / 2.0;
    let rad3 = (dx3 * dx3 + dy3 * dy3).sqrt();
    let phi4 = dy3.atan2(dx3) + phi1;
    let (dx4, dy4) = (-rad3 * phi4.cos(), -rad3 * phi4.sin());

    let canvas = diagonal as usize;
    let (out_width, out_height) = (rr_width as usize, rr_height as usize);
    if out_width == 0 || out_height == 0 || canvas < out_width || canvas < out_height {
        return Err(Error::failed(
            "the rotated rectangle is too small to duplicate",
        ));
    }
    let x_offset = ((diagonal - (used_right - used_left)) / 2.0) as i64;
    let y_offset = ((diagonal - (used_bottom - used_top)) / 2.0) as i64;
    let whole = Rect::new(0, 0, canvas, canvas);
    let cut = Rect::new(
        (canvas - out_width) / 2,
        (canvas - out_height) / 2,
        out_width,
        out_height,
    );
    let width = copy.meta().width();
    let planes = each_plane(ctx, copy, "Straightening", |_, samples| {
        let square = transform::canvas(samples, width, canvas, canvas, x_offset, y_offset, 0.0)?;
        let turned =
            transform::rotate(&square, canvas, whole, angle, Interpolation::Bilinear, 0.0)?;
        let moved =
            transform::translate(&turned, canvas, whole, dx4, dy4, Interpolation::Bilinear)?;
        calipix_core::crop_buffer(&moved, canvas, cut)
    })?;
    let meta = resized_meta(copy.meta(), out_width, out_height, |_, _| {})?;
    Ok(Image::from_planes(meta, planes)?)
}

fn duplicate(ctx: &mut CommandCtx<'_>, p: DuplicateParams) -> Result<Outputs> {
    let dataset = ctx.workspace().get(p.input)?.clone();
    let source = dataset.image.clone();
    let meta = source.meta().clone();
    let area = dataset.roi.as_ref().filter(|roi| roi.is_area());
    // ImageJ straightens the copy of a rotated rectangle, selection
    // ignored or not.
    let rotated = area.filter(|roi| matches!(roi, Roi::RotatedRectangle { .. }));
    let (rect, kept) = match area {
        Some(roi) if rotated.is_some() && !p.ignore_selection => {
            let bounds = roi
                .pixel_bounds(meta.width(), meta.height())
                .ok_or_else(|| Error::failed("the selection is outside the image"))?;
            (Some(bounds), None)
        }
        Some(roi) if !p.ignore_selection => {
            let (rect, kept) = crop_region(roi, meta.width(), meta.height())?;
            (Some(rect), kept)
        }
        _ => (None, None),
    };

    let mut view = source.clone();
    if let Some(rect) = rect {
        view = cropped(&view, rect)?;
    }
    // Narrow each stack axis: to the range asked for, or to the plane on
    // view. Slices and frames keep their axis, and with it its calibration.
    let size = |kind: &AxisKind| meta.size(kind);
    let stack_axes = [AxisKind::Z, AxisKind::Time]
        .iter()
        .filter(|kind| size(kind) > 1)
        .count();
    for (kind, text, param, on_view) in [
        (AxisKind::Z, &p.slices, "slices", p.position.slice),
        (AxisKind::Time, &p.frames, "frames", p.position.frame),
    ] {
        let Some(axis) = view.meta().axis_index(&kind) else {
            continue;
        };
        let (start, len) = if p.stack {
            // `range` is ImageJ's keyword for a stack that is not a
            // hyperstack: it applies to the image's only stack axis.
            let text = if text.trim().is_empty() && stack_axes == 1 && size(&kind) > 1 {
                &p.range
            } else {
                text
            };
            parse_range(text, size(&kind), param)?
        } else {
            let index = on_view.clamp(1, size(&kind) as i64) as usize - 1;
            (index, 1)
        };
        if (start, len) != (0, size(&kind)) {
            view = view.range(axis, start, len)?;
        }
    }
    if let Some(axis) = view.meta().axis_index(&AxisKind::Channel) {
        let count = size(&AxisKind::Channel);
        let (start, len) = if meta.color == ColorModel::Rgb {
            (0, count)
        } else if p.stack {
            parse_range(&p.channels, count, "channels")?
        } else if p.position.channel == 0 {
            (0, count)
        } else {
            ((p.position.channel as usize).min(count) - 1, 1)
        };
        if len == 1 && count > 1 {
            view = view.slice(axis, start)?;
        } else if (start, len) != (0, count) {
            view = view.range(axis, start, len)?;
        }
    }

    let name = if p.title.trim().is_empty() {
        unique_name(ctx.workspace(), &meta.name)
    } else {
        p.title.clone()
    };
    let mut copy_meta = view.meta().clone();
    copy_meta.name = name;
    let mut copy = settle(view.with_meta(copy_meta)?, &source)?;
    if let Some(roi) = rotated {
        copy = straightened(ctx, &copy, roi, (meta.width(), meta.height()))?;
    }
    let id = ctx.workspace_mut().insert(copy, None);
    // A copy of the whole image keeps the selection. A copy cropped to a
    // rectangle is the selection; one cropped to another shape keeps the
    // shape.
    if rotated.is_some() {
        // The straightened copy has no selection.
    } else if p.ignore_selection
        && let Some(roi) = dataset.roi
        && roi.pixel_bounds(meta.width(), meta.height()).is_some()
    {
        ctx.workspace_mut().set_roi(id, Some(roi))?;
    } else if kept.is_some() {
        ctx.workspace_mut().set_roi(id, kept)?;
    }
    Ok(Outputs::new().with("image", id))
}

// --------------------------------------------------------------- flips

#[derive(Params)]
struct ScopeOnly {
    #[param(flatten)]
    scope: PlaneScope,
}

/// `Transformer.run` for the two flips.
fn flip(ctx: &mut CommandCtx<'_>, scope: &PlaneScope, horizontal: bool) -> Result<Outputs> {
    let whole = selection_rect(ctx, scope.input)?.is_none_or(|rect| {
        let meta = ctx.image(scope.input).map(|image| image.meta().clone());
        meta.is_ok_and(|meta| rect == Rect::new(0, 0, meta.width(), meta.height()))
    });
    // Transformer does not confine itself to the selection's shape: the
    // whole bounding rectangle is mirrored.
    let outputs = scope.apply_within(ctx, "Flipping", Extent::Bounds, |plane, info| {
        if horizontal {
            transform::flip_horizontal(plane, info.width, info.rect)
        } else {
            transform::flip_vertical(plane, info.width, info.rect)
        }
    })?;
    // Flipping the whole image mirrors a calibration origin with it.
    let image = ctx.image(scope.input)?.clone();
    let mut meta = image.meta().clone();
    if whole && has_origin(&meta) {
        let (index, size) = if horizontal {
            (0, meta.width())
        } else {
            (1, meta.height())
        };
        if let Some(axis) = meta.axis_mut(index) {
            let origin = size as f64 - 1.0 - pixel_origin(axis);
            set_pixel_origin(axis, origin);
        }
        ctx.workspace_mut()
            .set_image(scope.input, image.with_meta(meta)?)?;
    }
    Ok(outputs)
}

/// `StackReverser.flipStack`, for the Z axis of any image (or the time
/// axis of one without slices).
fn flip_z(ctx: &mut CommandCtx<'_>, p: InputOnly) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta().clone();
    let axis = [AxisKind::Z, AxisKind::Time]
        .iter()
        .filter(|kind| meta.size(kind) > 1)
        .find_map(|kind| meta.axis_index(kind))
        .ok_or_else(|| Error::failed("Flip Z needs an image with several slices or frames"))?;
    let size = meta.shape()[axis];
    let planes = (0..meta.plane_count())
        .map(|plane| {
            ctx.check_cancelled()?;
            let mut position = meta.plane_position(plane)?;
            position[axis - 2] = size - 1 - position[axis - 2];
            Ok(image.read_plane(meta.plane_index(&position)?)?)
        })
        .collect::<Result<Vec<_>>>()?;
    ctx.workspace_mut()
        .set_image(p.input, Image::from_planes(meta, planes)?)?;
    Ok(Outputs::new())
}

// ------------------------------------------------------- quarter turns

/// `Transformer.run` for the quarter turns: every plane is turned, and the
/// calibration of X and Y changes places.
fn rotate_90(ctx: &mut CommandCtx<'_>, p: InputOnly, clockwise: bool) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta().clone();
    let (width, height) = (meta.width(), meta.height());
    let planes = each_plane(ctx, &image, "Rotating", |_, plane| {
        if clockwise {
            transform::rotate_right(plane, width)
        } else {
            transform::rotate_left(plane, width)
        }
    })?;
    let moved = has_origin(&meta);
    let (old_x, old_y) = (meta.axes()[0].clone(), meta.axes()[1].clone());
    let (x_origin, y_origin) = (pixel_origin(&old_x), pixel_origin(&old_y));
    let out = resized_meta(&meta, height, width, |x, y| {
        (x.scale, x.unit) = (old_y.scale, old_y.unit.clone());
        (y.scale, y.unit) = (old_x.scale, old_x.unit.clone());
        let (new_x, new_y) = match (moved, clockwise) {
            (false, _) => (0.0, 0.0),
            (true, true) => (height as f64 - 1.0 - y_origin, x_origin),
            (true, false) => (y_origin, width as f64 - 1.0 - x_origin),
        };
        set_pixel_origin(x, new_x);
        set_pixel_origin(y, new_y);
    })?;
    ctx.workspace_mut()
        .set_image(p.input, Image::from_planes(out, planes)?)?;
    ctx.workspace_mut().set_roi(p.input, None)?;
    Ok(Outputs::new())
}

// -------------------------------------------------------------- canvas

#[derive(Params)]
struct CanvasParams {
    /// The image whose canvas changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// New width; the current width if left empty.
    #[param(label = "Width", min = 1, unit = "pixels", no_persist, ijm = "width")]
    width: Option<i64>,
    /// New height; the current height if left empty.
    #[param(label = "Height", min = 1, unit = "pixels", no_persist, ijm = "height")]
    height: Option<i64>,
    /// Where the image sits on the new canvas.
    #[param(
        label = "Position",
        default = "Center",
        choices(
            "Top-Left",
            "Top-Center",
            "Top-Right",
            "Center-Left",
            "Center",
            "Center-Right",
            "Bottom-Left",
            "Bottom-Center",
            "Bottom-Right"
        ),
        ijm = "position"
    )]
    position: String,
    /// Fill the added border with zero. Otherwise it is white: 255 for
    /// 8-bit and RGB images, and the top of the display range for others.
    #[param(label = "Zero fill", default = false, ijm = "zero")]
    zero: bool,
}

/// Where an image of `old` samples sits along a canvas of `new` samples.
fn canvas_offset(old: usize, new: usize, placement: usize) -> i64 {
    let difference = new as i64 - old as i64;
    match placement {
        0 => 0,
        1 => difference / 2,
        _ => difference,
    }
}

/// The value of ImageJ's default background colour, white, for each plane
/// of `image` (`ImageProcessor.setGlobalBackgroundColor`): 255 for 8-bit
/// samples, and the top of the display range for others.
fn white(image: &Image) -> Result<Vec<f64>> {
    let meta = image.meta();
    if meta.dtype() == DType::U8 {
        // White is the first entry of an inverting lookup table.
        return Ok((0..meta.plane_count())
            .map(|plane| {
                let lut = meta
                    .plane_channel(plane)
                    .ok()
                    .and_then(|c| meta.channels.get(c))
                    .and_then(|c| c.lut.as_ref());
                match lut {
                    Some(lut) if lut.table[0] == [255; 3] && lut.table[255] == [0; 3] => 0.0,
                    _ => 255.0,
                }
            })
            .collect());
    }
    // One value per channel, from its first plane if no range is set.
    let mut by_channel: Vec<Option<f64>> = vec![None; meta.size(&AxisKind::Channel)];
    (0..meta.plane_count())
        .map(|plane| {
            let channel = meta.plane_channel(plane)?;
            if by_channel[channel].is_none() {
                let (_, max) = calipix_render::effective_display_range(image, plane)?;
                // ShortProcessor.setColor truncates.
                by_channel[channel] = Some(if meta.dtype().is_integer() {
                    max.trunc()
                } else {
                    max
                });
            }
            Ok(by_channel[channel].unwrap_or(0.0))
        })
        .collect()
}

/// Every plane of `image` on a canvas of another size
/// (`CanvasResizer.expandStack`). A calibration origin, if one is set,
/// moves with the samples; ImageJ leaves it where it was.
fn on_canvas(
    ctx: &CommandCtx<'_>,
    image: &Image,
    size: (usize, usize),
    offset: (i64, i64),
    fill: &[f64],
) -> Result<Image> {
    let meta = image.meta();
    let width = meta.width();
    let planes = each_plane(ctx, image, "Resizing canvas", |plane, samples| {
        transform::canvas(
            samples,
            width,
            size.0,
            size.1,
            offset.0,
            offset.1,
            fill[plane],
        )
    })?;
    let moved = has_origin(meta);
    let out = resized_meta(meta, size.0, size.1, |x, y| {
        if moved {
            x.origin -= offset.0 as f64 * x.scale;
            y.origin -= offset.1 as f64 * y.scale;
        }
    })?;
    Ok(Image::from_planes(out, planes)?)
}

fn canvas_size(ctx: &mut CommandCtx<'_>, p: CanvasParams) -> Result<Outputs> {
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta();
    let new_width = p.width.map_or(meta.width(), |w| w as usize);
    let new_height = p.height.map_or(meta.height(), |h| h as usize);
    const POSITIONS: [&str; 9] = [
        "Top-Left",
        "Top-Center",
        "Top-Right",
        "Center-Left",
        "Center",
        "Center-Right",
        "Bottom-Left",
        "Bottom-Center",
        "Bottom-Right",
    ];
    let index = POSITIONS
        .iter()
        .position(|name| *name == p.position)
        .ok_or_else(|| Error::InvalidArg {
            param: "position".to_string(),
            reason: format!("`{}` is not a position", p.position),
        })?;
    let offset = (
        canvas_offset(meta.width(), new_width, index % 3),
        canvas_offset(meta.height(), new_height, index / 3),
    );
    let fill = if p.zero {
        vec![0.0; meta.plane_count()]
    } else {
        white(&image)?
    };
    let result = on_canvas(ctx, &image, (new_width, new_height), offset, &fill)?;
    ctx.workspace_mut().set_image(p.input, result)?;
    ctx.workspace_mut().set_roi(p.input, None)?;
    Ok(Outputs::new())
}

// -------------------------------------------------------------- rotate

#[derive(Params)]
struct RotateParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Angle to turn by, clockwise.
    #[param(
        label = "Angle",
        default = 15.0,
        step = 0.1,
        unit = "degrees",
        ijm = "angle"
    )]
    angle: f64,
    /// Accepted for ImageJ macros; Calipix draws no grid.
    #[param(label = "Grid lines", default = 1, advanced, no_persist, ijm = "grid")]
    #[allow(dead_code)]
    grid: i64,
    /// How samples between pixel centres are estimated.
    #[param(
        label = "Interpolation",
        default = "Bilinear",
        choices("None", "Bilinear", "Bicubic"),
        ijm = "interpolation"
    )]
    interpolation: String,
    /// Make the image large enough to hold all of the turned picture. Only
    /// possible without a selection, and it changes every plane's size.
    #[param(label = "Enlarge image", default = false, ijm = "enlarge")]
    enlarge: bool,
}

/// The canvas that holds an image turned by `angle` degrees: the bounding
/// box of its turned outline, corners in single precision as in ImageJ
/// (`Rotator.enlargeCanvas`, `RoiRotator.rotate`), and never smaller than
/// the image.
fn enlarged_size(width: usize, height: usize, angle: f64) -> (usize, usize) {
    let (w, h) = (width as f64, height as f64);
    let (xc, yc) = (w / 2.0, h / 2.0);
    let theta = -angle * std::f64::consts::PI / 180.0;
    let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y): (f64, f64)| {
        let (dx, dy) = (x - xc, yc - y);
        let radius = (dx * dx + dy * dy).sqrt();
        let a = dy.atan2(dx);
        (
            (xc + radius * (a + theta).cos()) as f32,
            (yc - radius * (a + theta).sin()) as f32,
        )
    });
    let span = |values: [f32; 4]| {
        let min = values.iter().copied().fold(f32::MAX, f32::min);
        let max = values.iter().copied().fold(f32::MIN_POSITIVE, f32::max);
        round((max - min) as f64).max(0) as usize
    };
    (
        span(corners.map(|c| c.0)).max(width),
        span(corners.map(|c| c.1)).max(height),
    )
}

/// `Rotator.run`.
fn rotate(ctx: &mut CommandCtx<'_>, p: RotateParams) -> Result<Outputs> {
    let method = interpolation(&p.interpolation)?;
    let image = ctx.image(p.scope.input)?.clone();
    let meta = image.meta().clone();
    let (width, height) = (meta.width(), meta.height());
    let whole = Rect::new(0, 0, width, height);
    // ImageJ only offers to enlarge when nothing less than the whole image
    // is selected.
    let can_enlarge = selection_rect(ctx, p.scope.input)?.is_none_or(|rect| rect == whole);
    if !(p.enlarge && can_enlarge) {
        return p.scope.apply(ctx, "Rotating", |plane, info| {
            transform::rotate(plane, info.width, info.rect, p.angle, method, 0.0)
        });
    }

    let size = enlarged_size(width, height, p.angle);
    let offset = (
        canvas_offset(width, size.0, 1),
        canvas_offset(height, size.1, 1),
    );
    let larger = on_canvas(ctx, &image, size, offset, &vec![0.0; meta.plane_count()])?;
    let in_scope = p.scope.planes(&meta)?;
    let turned = Rect::new(0, 0, size.0, size.1);
    let planes = each_plane(ctx, &larger, "Rotating", |plane, samples| {
        if in_scope.contains(&plane) {
            transform::rotate(samples, size.0, turned, p.angle, method, 0.0)
        } else {
            Ok(samples.clone())
        }
    })?;
    let result = Image::from_planes(larger.meta().clone(), planes)?;
    ctx.workspace_mut().set_image(p.scope.input, result)?;
    ctx.workspace_mut().set_roi(p.scope.input, None)?;
    Ok(Outputs::new())
}

// ----------------------------------------------------------- translate

#[derive(Params)]
struct TranslateParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Distance to move to the right; negative moves left.
    #[param(label = "X offset", default = 15.0, unit = "pixels", ijm = "x")]
    x: f64,
    /// Distance to move down; negative moves up.
    #[param(label = "Y offset", default = 15.0, unit = "pixels", ijm = "y")]
    y: f64,
    /// How samples are estimated when the distance is not a whole number
    /// of pixels. Without interpolation the distance is truncated.
    #[param(
        label = "Interpolation",
        default = "None",
        choices("None", "Bilinear", "Bicubic"),
        ijm = "interpolation"
    )]
    interpolation: String,
}

// --------------------------------------------------------------- scale

#[derive(Params)]
struct ScaleParams {
    /// The image to scale.
    #[param(label = "Image")]
    input: DatasetId,
    /// Factor for the width.
    #[param(label = "X scale", default = 0.5, ijm = "x")]
    x_scale: f64,
    /// Factor for the height.
    #[param(label = "Y scale", default = 0.5, ijm = "y")]
    y_scale: f64,
    /// Factor for the number of slices (or frames, without slices).
    #[param(label = "Z scale", default = 1.0, advanced, ijm = "z")]
    z_scale: f64,
    /// New width; given together with a height, it replaces the factors.
    #[param(label = "Width", min = 1, unit = "pixels", no_persist, ijm = "width")]
    width: Option<i64>,
    /// New height; given together with a width, it replaces the factors.
    #[param(label = "Height", min = 1, unit = "pixels", no_persist, ijm = "height")]
    height: Option<i64>,
    /// New number of slices; it replaces the Z factor.
    #[param(label = "Depth", min = 1, no_persist, advanced, ijm = "depth")]
    depth: Option<i64>,
    /// How samples between pixel centres are estimated.
    #[param(
        label = "Interpolation",
        default = "Bilinear",
        choices("None", "Bilinear", "Bicubic"),
        ijm = "interpolation"
    )]
    interpolation: String,
    /// When shrinking, average the samples each new sample covers instead
    /// of picking one, which avoids jagged results.
    #[param(label = "Average when downsizing", default = true, ijm = "average")]
    average: bool,
    /// Scale every plane rather than only the one on view. Images with
    /// several channels are always scaled whole.
    #[param(
        label = "Process entire stack",
        default = true,
        ijm = "process",
        position = "stack"
    )]
    process: bool,
    /// Put the result in a new image of the new size. Otherwise the image
    /// keeps its size and its contents are scaled about their centre.
    #[param(label = "Create new image", default = true, ijm = "create")]
    create: bool,
    /// Name of the new image. Left empty, it is the image's name with a
    /// number added.
    #[param(label = "Title", default = "", no_persist, ijm = "title")]
    title: String,
    #[param(flatten)]
    position: Position,
}

/// The stack axis ImageJ's Scale and Size count as depth: slices, or
/// frames for an image without slices.
fn depth_axis(meta: &ImageMeta) -> Option<usize> {
    let slices = meta.size(&AxisKind::Z);
    let frames = meta.size(&AxisKind::Time);
    let kind = if slices == 1 && frames > 1 {
        AxisKind::Time
    } else {
        AxisKind::Z
    };
    meta.axis_index(&kind)
}

/// Scales `image` along one stack axis to `new_depth` planes
/// (`Resizer.zScale`).
fn z_scaled(
    ctx: &CommandCtx<'_>,
    image: &Image,
    axis: usize,
    new_depth: usize,
    method: Interpolation,
    average: bool,
) -> Result<Image> {
    let meta = image.meta();
    let depth = meta.shape()[axis];
    let mut axes = axes_of(meta);
    axes[axis].1 = new_depth;
    if axes[axis].0.kind == AxisKind::Z {
        // The slices cover the same depth, from the same origin.
        let origin = pixel_origin(&axes[axis].0) * new_depth as f64 / depth as f64;
        if scaled(meta) {
            axes[axis].0.scale *= depth as f64 / new_depth as f64;
        }
        set_pixel_origin(&mut axes[axis].0, origin);
    }
    let mut out = rebuilt(meta, meta.dtype(), axes)?;
    out.color = meta.color;
    out.channels = meta.channels.clone();

    let mut planes: Vec<Option<Buffer>> = vec![None; out.plane_count()];
    let lines = meta.plane_count() / depth;
    let mut done = 0;
    for plane in 0..meta.plane_count() {
        let mut position = meta.plane_position(plane)?;
        if position[axis - 2] != 0 {
            continue;
        }
        ctx.check_cancelled()?;
        ctx.progress(done as f64 / lines as f64, "Scaling in Z");
        done += 1;
        let column = (0..depth)
            .map(|z| {
                position[axis - 2] = z;
                Ok(image.read_plane(meta.plane_index(&position)?)?)
            })
            .collect::<Result<Vec<_>>>()?;
        let scaled = transform::resize_z(&column, meta.width(), new_depth, method, average)?;
        for (z, buffer) in scaled.into_iter().enumerate() {
            position[axis - 2] = z;
            planes[out.plane_index(&position)?] = Some(buffer);
        }
    }
    let planes = planes
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| Error::failed("scaling in Z left a plane empty"))?;
    Ok(Image::from_planes(out, planes)?)
}

/// The calibration of X and Y after scaling the part of the image starting
/// at `(left, top)` by `(x_scale, y_scale)`: pixels grow as the image
/// shrinks, and a calibration origin stays where it is in the picture.
fn scale_axes(
    source: &ImageMeta,
    (left, top): (usize, usize),
    (x_scale, y_scale): (f64, f64),
) -> impl FnOnce(&mut Axis, &mut Axis) {
    let calibrated = scaled(source);
    let moved = has_origin(source);
    move |x, y| {
        for (axis, start, factor) in [(x, left, x_scale), (y, top, y_scale)] {
            let origin = if moved {
                (pixel_origin(axis) - start as f64) * factor
            } else {
                0.0
            };
            if calibrated {
                axis.scale /= factor;
            }
            set_pixel_origin(axis, origin);
        }
    }
}

/// `Scaler.run`.
fn scale(ctx: &mut CommandCtx<'_>, p: ScaleParams) -> Result<Outputs> {
    let method = interpolation(&p.interpolation)?;
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta().clone();
    let (width, height) = (meta.width(), meta.height());
    let whole = Rect::new(0, 0, width, height);
    let rect = selection_rect(ctx, p.input)?.unwrap_or(whole);

    let (new_width, new_height, x_scale, y_scale) = match (p.width, p.height) {
        (Some(w), Some(h)) => (
            w as usize,
            h as usize,
            w as f64 / rect.w as f64,
            h as f64 / rect.h as f64,
        ),
        _ => {
            if !(p.x_scale > 0.0 && p.y_scale > 0.0) {
                return Err(Error::failed("the scale factors must be greater than 0"));
            }
            (
                round(rect.w as f64 * p.x_scale).max(0) as usize,
                round(rect.h as f64 * p.y_scale).max(0) as usize,
                p.x_scale,
                p.y_scale,
            )
        }
    };
    if new_width == 0 || new_height == 0 {
        return Err(Error::failed(
            "the scaled image would be less than one pixel wide or high",
        ));
    }

    let count = meta.plane_count();
    let pictures = if meta.color == ColorModel::Rgb {
        count / 3
    } else {
        count
    };
    let depth = depth_axis(&meta);
    let old_depth = depth.map_or(1, |axis| meta.shape()[axis]);
    let new_depth = match p.depth {
        Some(depth) => depth as usize,
        None if p.z_scale > 0.0 => round(old_depth as f64 * p.z_scale).max(0) as usize,
        None => old_depth,
    };
    let scale_z = pictures > 1 && new_depth > 0 && new_depth != old_depth;
    // Scaling in Z, or an image with channels, means the whole image and a
    // new one.
    let hyperstack = pictures > 1 && pictures != old_depth;
    let whole_stack = pictures > 1 && (p.process || scale_z || hyperstack);
    let create = p.create || scale_z;

    if !create {
        // Image > Scale without a new window: the contents are scaled
        // where they are, about the centre of the selection.
        let in_scope: Vec<usize> = if whole_stack {
            (0..count).collect()
        } else {
            p.position.planes(p.input, &meta)?
        };
        let method = if width > 1 && height > 1 {
            method
        } else {
            Interpolation::None
        };
        let mut result = image.clone();
        for (step, &plane) in in_scope.iter().enumerate() {
            ctx.check_cancelled()?;
            ctx.progress(step as f64 / in_scope.len() as f64, "Scaling");
            // What the reduced contents no longer cover becomes 0 in 8-bit
            // and RGB images and the bottom of the display range in others.
            let background = if meta.dtype() == DType::U8 {
                0.0
            } else {
                calipix_render::effective_display_range(&image, plane)?.0
            };
            let samples = image.read_plane(plane)?;
            let scaled = transform::scale_in_place(
                &samples, width, rect, x_scale, y_scale, method, background,
            )?;
            result = result.with_plane(plane, scaled)?;
        }
        ctx.workspace_mut().set_image(p.input, result)?;
        ctx.workspace_mut().set_roi(p.input, None)?;
        return Ok(Outputs::new());
    }

    let name = if p.title.trim().is_empty() {
        unique_name(ctx.workspace(), &meta.name)
    } else {
        p.title.clone()
    };
    let axes = scale_axes(&meta, (rect.x, rect.y), (x_scale, y_scale));
    let mut result = if whole_stack {
        // Scaler.createNewStack: each plane is cropped to the selection and
        // then scaled.
        let method = if width == 1 || height == 1 {
            Interpolation::None
        } else {
            method
        };
        let region = Rect::new(0, 0, rect.w, rect.h);
        let planes = each_plane(ctx, &image, "Scaling", |_, samples| {
            let part = calipix_core::crop_buffer(samples, width, rect)?;
            transform::resize(
                &part, rect.w, region, new_width, new_height, method, p.average,
            )
        })?;
        let out = resized_meta(&meta, new_width, new_height, axes)?;
        Image::from_planes(out, planes)?
    } else {
        // Scaler.scale: the plane on view, read through the selection.
        let in_scope = p.position.planes(p.input, &meta)?;
        let planes = in_scope
            .iter()
            .map(|&plane| {
                let samples = image.read_plane(plane)?;
                Ok(transform::resize(
                    &samples, width, rect, new_width, new_height, method, p.average,
                )?)
            })
            .collect::<Result<Vec<_>>>()?;
        // One picture: the planes on view are its channels.
        let mut out_axes = axes_of(&meta)[..2].to_vec();
        out_axes[0].1 = new_width;
        out_axes[1].1 = new_height;
        {
            let (x, y) = out_axes.split_at_mut(1);
            axes(&mut x[0].0, &mut y[0].0);
        }
        let mut channels = Vec::new();
        if planes.len() > 1 {
            out_axes.push((Axis::new(AxisKind::Channel), planes.len()));
        }
        for &plane in &in_scope {
            if let Some(entry) = meta.channels.get(meta.plane_channel(plane)?) {
                channels.push(entry.clone());
            }
        }
        let mut out = rebuilt(&meta, meta.dtype(), out_axes)?;
        out.color = match planes.len() {
            1 => ColorModel::Grayscale,
            _ => meta.color,
        };
        out.channels = channels;
        Image::from_planes(out, planes)?
    };
    if scale_z && let Some(axis) = depth {
        result = z_scaled(ctx, &result, axis, new_depth, method, p.average)?;
    }
    let mut named = result.meta().clone();
    named.name = name;
    let id = ctx.workspace_mut().insert(result.with_meta(named)?, None);
    Ok(Outputs::new().with("image", id))
}

// -------------------------------------------------------------- resize

#[derive(Params)]
struct ResizeParams {
    /// The image to resize.
    #[param(label = "Image")]
    input: DatasetId,
    /// New width; half the current width if left empty (or, with the
    /// aspect ratio kept and a height given, whatever keeps the shape).
    #[param(label = "Width", min = 1, unit = "pixels", no_persist, ijm = "width")]
    width: Option<i64>,
    /// New height; half the current height if left empty.
    #[param(label = "Height", min = 1, unit = "pixels", no_persist, ijm = "height")]
    height: Option<i64>,
    /// New number of slices; unchanged if left empty.
    #[param(label = "Depth", min = 1, no_persist, advanced, ijm = "depth")]
    depth: Option<i64>,
    /// New number of frames; unchanged if left empty.
    #[param(label = "Frames", min = 1, no_persist, advanced, ijm = "time")]
    frames: Option<i64>,
    /// Keep the shape of the image: the height follows from the width.
    #[param(label = "Constrain aspect ratio", default = true, ijm = "constrain")]
    constrain: bool,
    /// When shrinking, average the samples each new sample covers instead
    /// of picking one, which avoids jagged results.
    #[param(label = "Average when downsizing", default = true, ijm = "average")]
    average: bool,
    /// How samples between pixel centres are estimated.
    #[param(
        label = "Interpolation",
        default = "Bilinear",
        choices("None", "Bilinear", "Bicubic"),
        ijm = "interpolation"
    )]
    interpolation: String,
}

/// `Resizer.run` for Image > Adjust > Size.
fn resize(ctx: &mut CommandCtx<'_>, p: ResizeParams) -> Result<Outputs> {
    let method = interpolation(&p.interpolation)?;
    let image = ctx.image(p.input)?.clone();
    let meta = image.meta().clone();
    let width = meta.width();
    let rect = selection_rect(ctx, p.input)?.unwrap_or(Rect::new(0, 0, width, meta.height()));
    let (orig_w, orig_h) = (rect.w as f64, rect.h as f64);
    let (mut new_width, mut new_height) = (
        p.width.map_or(rect.w / 2, |w| w as usize),
        p.height.map_or(rect.h / 2, |h| h as usize),
    );
    if p.constrain {
        if p.width.is_none() && p.height.is_some() {
            new_width = round(new_height as f64 * (orig_w / orig_h)).max(0) as usize;
        } else {
            new_height = round(new_width as f64 * (orig_h / orig_w)).max(0) as usize;
        }
    }
    if new_width == 0 || new_height == 0 {
        return Err(Error::failed(
            "the resized image would be less than one pixel wide or high",
        ));
    }

    let planes = each_plane(ctx, &image, "Resizing", |_, samples| {
        transform::resize(
            samples, width, rect, new_width, new_height, method, p.average,
        )
    })?;
    let factors = (new_width as f64 / orig_w, new_height as f64 / orig_h);
    let out = resized_meta(
        &meta,
        new_width,
        new_height,
        scale_axes(&meta, (rect.x, rect.y), factors),
    )?;
    let mut result = Image::from_planes(out, planes)?;
    for (kind, wanted) in [(AxisKind::Z, p.depth), (AxisKind::Time, p.frames)] {
        let Some(wanted) = wanted else { continue };
        // A stack that is not a hyperstack is scaled along whichever axis
        // it has.
        let axis = match result.meta().axis_index(&kind) {
            Some(axis) => Some(axis),
            None if kind == AxisKind::Z => depth_axis(result.meta()),
            None => None,
        };
        if let Some(axis) = axis
            && result.meta().shape()[axis] != wanted as usize
            && result.meta().shape()[axis] > 1
        {
            result = z_scaled(ctx, &result, axis, wanted as usize, method, p.average)?;
        }
    }
    ctx.workspace_mut().set_image(p.input, result)?;
    ctx.workspace_mut().set_roi(p.input, None)?;
    Ok(Outputs::new())
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        CommandSpec::new("image.crop", "Crop")
            .summary("Cuts an image down to the selection.")
            .doc(
                "Every plane is cropped to the rectangle around the selection. A \
                 calibration origin, if one is set, moves with the image.",
            )
            .menu("Image", 110.0)
            .shortcut("Ctrl+Shift+X")
            .ijm_name("Crop")
            .effects(modifies()),
        crop,
    ))?;
    registry.register(typed(
        CommandSpec::new("image.duplicate", "Duplicate")
            .summary("Makes a copy of an image, or of the selected part of it, as a new image.")
            .doc(
                "Without Duplicate stack only the plane on view is copied (all the \
                 channels shown together count as one plane). With it, every plane or the \
                 given ranges of channels, slices and frames.",
            )
            .menu("Image", 120.0)
            .shortcut("Ctrl+Shift+D")
            .ijm_name("Duplicate...")
            .output("image", ParamType::Image)
            .effects(creates()),
        duplicate,
    ))?;
    registry.register(typed(
        CommandSpec::new("image.scale", "Scale")
            .summary("Enlarges or reduces an image, or the selected part of it, by given factors.")
            .doc(
                "By default the result is a new image of the new size. Without Create new \
                 image the image keeps its size and its contents are scaled about the \
                 centre of the selection.",
            )
            .menu("Image", 210.0)
            .ijm_name("Scale...")
            .output("image", ParamType::Image)
            .effects(creates()),
        scale,
    ))?;
    registry.register(typed(
        CommandSpec::new("image.resize", "Resize")
            .summary("Changes the size of an image in pixels, stretching its contents to fit.")
            .doc(
                "Every plane is resized. With a selection, the selected part is what is \
                 kept and resized. The pixel size is adjusted so that the image covers the \
                 same area.",
            )
            .menu("Image", 220.0)
            .ijm_name("Size...")
            .effects(modifies()),
        resize,
    ))?;
    registry.register(typed(
        CommandSpec::new("image.canvas_size", "Canvas Size")
            .summary("Adds a border around an image, or cuts its edges, without scaling it.")
            .menu("Image", 230.0)
            .ijm_name("Canvas Size...")
            .effects(modifies()),
        canvas_size,
    ))?;

    registry.register(typed(
        CommandSpec::new("image.flip_horizontal", "Flip Horizontally")
            .summary("Mirrors an image, or the selected part of it, left to right.")
            .menu("Image > Transform", 310.0)
            .ijm_name("Flip Horizontally")
            .effects(modifies()),
        |ctx, p: ScopeOnly| flip(ctx, &p.scope, true),
    ))?;
    registry.register(typed(
        CommandSpec::new("image.flip_vertical", "Flip Vertically")
            .summary("Mirrors an image, or the selected part of it, top to bottom.")
            .menu("Image > Transform", 320.0)
            .ijm_name("Flip Vertically")
            .effects(modifies()),
        |ctx, p: ScopeOnly| flip(ctx, &p.scope, false),
    ))?;
    registry.register(typed(
        CommandSpec::new("image.flip_z", "Flip Z")
            .summary("Reverses the order of an image's slices.")
            .doc("An image without slices has the order of its frames reversed.")
            .menu("Image > Transform", 330.0)
            .ijm_name("Flip Z")
            .effects(modifies()),
        flip_z,
    ))?;
    registry.register(typed(
        CommandSpec::new("image.rotate_right", "Rotate 90 Degrees Right")
            .summary("Turns every plane of an image a quarter turn clockwise.")
            .menu("Image > Transform", 410.0)
            .ijm_name("Rotate 90 Degrees Right")
            .effects(modifies()),
        |ctx, p: InputOnly| rotate_90(ctx, p, true),
    ))?;
    registry.register(typed(
        CommandSpec::new("image.rotate_left", "Rotate 90 Degrees Left")
            .summary("Turns every plane of an image a quarter turn anticlockwise.")
            .menu("Image > Transform", 420.0)
            .ijm_name("Rotate 90 Degrees Left")
            .effects(modifies()),
        |ctx, p: InputOnly| rotate_90(ctx, p, false),
    ))?;
    registry.register(typed(
        CommandSpec::new("image.rotate", "Rotate")
            .summary("Turns an image, or the selected part of it, by any angle.")
            .doc(
                "The image keeps its size unless Enlarge image is set: corners that turn \
                 out of it are lost and the parts nothing turns into become 0.",
            )
            .menu("Image > Transform", 430.0)
            .ijm_name("Rotate... ")
            .effects(modifies()),
        rotate,
    ))?;
    registry.register(typed(
        CommandSpec::new("image.translate", "Translate")
            .summary("Moves the contents of an image, or of the selection, by a given distance.")
            .doc("What moves out is lost, and what nothing moves into becomes 0.")
            .menu("Image > Transform", 510.0)
            .ijm_name("Translate...")
            .effects(modifies()),
        |ctx, p: TranslateParams| {
            let method = interpolation(&p.interpolation)?;
            // Translator, like the flips, moves the whole bounding
            // rectangle of a selection.
            p.scope
                .apply_within(ctx, "Translating", Extent::Bounds, |plane, info| {
                    transform::translate(plane, info.width, info.rect, p.x, p.y, method)
                })
        },
    ))?;
    Ok(())
}
