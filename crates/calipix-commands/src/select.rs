//! Selection commands: choosing the part of an image that later commands
//! act within or measure.
//!
//! Each command reads the image's selection from the workspace, makes a
//! new one and stores it; undo puts the old one back. The geometry is in
//! `calipix-roi`, which follows ImageJ: these commands give the selection
//! ImageJ's Edit > Selection commands give.

use std::path::PathBuf;

use calipix_command::{
    CommandCtx, CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs, Params, Registry,
    Result, typed,
};
use calipix_roi::{Roi, RoiEntry};

use crate::InputOnly;

#[derive(Params)]
struct SetParams {
    /// The image to select within.
    #[param(label = "Image")]
    input: DatasetId,
    /// The selection, for example `rectangle 10 20 30 40` for x, y, width
    /// and height, `oval 10 20 30 40`, `polygon 5,5 40,8 20,30` or
    /// `line 2 40 60 44`.
    #[param(label = "Selection", no_persist)]
    selection: Roi,
}

#[derive(Params)]
struct EnlargeParams {
    /// The image whose selection changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// How far to move the outline outwards; negative values shrink the
    /// selection.
    #[param(
        label = "Enlarge by",
        default = 15,
        min = -255,
        max = 255,
        unit = "pixels",
        ijm = "enlarge"
    )]
    pixels: i64,
}

#[derive(Params)]
struct ScaleParams {
    /// The image whose selection changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// Factor applied to horizontal distances.
    #[param(label = "X scale factor", default = 1.0, min = 0.0001, ijm = "x")]
    x_scale: f64,
    /// Factor applied to vertical distances.
    #[param(label = "Y scale factor", default = 1.0, min = 0.0001, ijm = "y")]
    y_scale: f64,
    /// Scale about the middle of the selection, so that it stays where it
    /// is, instead of about the image's top-left corner.
    #[param(default = false)]
    centered: bool,
}

#[derive(Params)]
struct RotateParams {
    /// The image whose selection changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// How far to turn the selection, clockwise; negative values turn it
    /// anticlockwise.
    #[param(default = 15.0, unit = "degrees")]
    angle: f64,
    /// Turn about the middle of the image instead of the middle of the
    /// selection.
    #[param(label = "Rotate around image center", default = false, ijm = "rotate")]
    around_image_center: bool,
}

#[derive(Params)]
struct TranslateParams {
    /// The image whose selection changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// Distance to move the selection to the right.
    #[param(label = "X offset", default = 0.0, unit = "pixels", ijm = "x")]
    x_offset: f64,
    /// Distance to move the selection down.
    #[param(label = "Y offset", default = 0.0, unit = "pixels", ijm = "y")]
    y_offset: f64,
}

#[derive(Params)]
struct OpenParams {
    /// The image that gets the selection.
    #[param(label = "Image")]
    input: DatasetId,
    /// An ImageJ selection file (`.roi`).
    #[param(label = "File", path = "open", no_persist)]
    path: PathBuf,
}

#[derive(Params)]
struct SaveParams {
    /// The image whose selection is saved.
    #[param(label = "Image")]
    input: DatasetId,
    /// Where to save the selection, as an ImageJ selection file (`.roi`).
    #[param(label = "File", path = "save", no_persist)]
    path: PathBuf,
}

fn modifies() -> Effects {
    Effects {
        image: ImageEffect::Modifies,
        ..Effects::default()
    }
}

/// The selection of image `id` and the image's width and height.
fn selection(ctx: &CommandCtx<'_>, id: DatasetId) -> Result<(Roi, usize, usize)> {
    let dataset = ctx.workspace().get(id)?;
    let meta = dataset.image.meta();
    let roi = dataset
        .roi
        .clone()
        .ok_or_else(|| Error::failed("this image has no selection; make one first"))?;
    Ok((roi, meta.width(), meta.height()))
}

/// As [`selection`], for commands that work on areas only.
fn area_selection(ctx: &CommandCtx<'_>, id: DatasetId) -> Result<(Roi, usize, usize)> {
    let (roi, width, height) = selection(ctx, id)?;
    if !roi.is_area() {
        return Err(Error::failed(format!(
            "this needs an area selection, but the selection is a {}",
            roi.kind()
        )));
    }
    Ok((roi, width, height))
}

fn set(ctx: &mut CommandCtx<'_>, id: DatasetId, roi: Roi) -> Result<Outputs> {
    ctx.workspace_mut().set_roi(id, Some(roi))?;
    Ok(Outputs::new())
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        CommandSpec::new("select.set", "Specify Selection")
            .summary("Selects a region given by its shape and coordinates.")
            .menu("Edit > Selection", 230.0)
            .effects(modifies()),
        |ctx, p: SetParams| set(ctx, p.input, p.selection),
    ))?;

    registry.register(typed(
        CommandSpec::new("select.all", "Select All")
            .summary("Selects the whole image.")
            .menu("Edit > Selection", 210.0)
            .shortcut("Ctrl+A")
            .ijm_name("Select All")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            let meta = ctx.image(p.input)?.meta();
            let all = Roi::rectangle(0.0, 0.0, meta.width() as f64, meta.height() as f64);
            set(ctx, p.input, all)
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.none", "Select None")
            .summary("Removes the selection.")
            .menu("Edit > Selection", 220.0)
            .shortcut("Ctrl+Shift+A")
            .ijm_name("Select None")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            ctx.workspace_mut().set_roi(p.input, None)?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.invert", "Invert Selection")
            .summary("Selects everything outside the selection instead.")
            .doc(
                "Needs an area selection. Without a selection the whole image is selected, \
                 and inverting a selection of the whole image removes it.",
            )
            .menu("Edit > Selection", 240.0)
            .ijm_name("Make Inverse")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            let dataset = ctx.workspace().get(p.input)?;
            let meta = dataset.image.meta();
            let (width, height) = (meta.width(), meta.height());
            let inverse = match &dataset.roi {
                None => Some(Roi::rectangle(0.0, 0.0, width as f64, height as f64)),
                Some(roi) if roi.is_area() => roi.inverse(width, height),
                Some(roi) => {
                    return Err(Error::failed(format!(
                        "only an area selection can be inverted, but the selection is a {}",
                        roi.kind()
                    )));
                }
            };
            ctx.workspace_mut().set_roi(p.input, inverse)?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.fit_ellipse", "Fit Ellipse")
            .summary(
                "Replaces an area selection by the ellipse with the same area, centre and \
                 orientation.",
            )
            .menu("Edit > Selection", 310.0)
            .ijm_name("Fit Ellipse")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            let (roi, width, height) = area_selection(ctx, p.input)?;
            let ellipse = roi
                .fitted_ellipse(width, height)
                .ok_or_else(|| Error::failed("the selection is outside the image"))?;
            set(ctx, p.input, ellipse)
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.convex_hull", "Make Convex Hull")
            .summary("Replaces the selection by the smallest convex polygon that contains it.")
            .menu("Edit > Selection", 320.0)
            .ijm_name("Convex Hull")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            let (roi, _, _) = selection(ctx, p.input)?;
            let hull = roi.convex_hull_roi().ok_or_else(|| {
                Error::failed(
                    "a convex hull needs an area, a point selection with three or more points, \
                     or a segmented or freehand line",
                )
            })?;
            set(ctx, p.input, hull)
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.to_bounding_box", "Make Bounding Box")
            .summary("Replaces the selection by the smallest rectangle that contains it.")
            .menu("Edit > Selection", 330.0)
            .ijm_name("To Bounding Box")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            let (roi, _, _) = selection(ctx, p.input)?;
            set(ctx, p.input, roi.bounding_box())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.line_to_area", "Convert Line to Area")
            .summary("Replaces a line selection by the area it covers at its width.")
            .menu("Edit > Selection", 340.0)
            .ijm_name("Line to Area")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            let (roi, _, _) = selection(ctx, p.input)?;
            if !roi.is_line() {
                return Err(Error::failed(format!(
                    "this needs a line selection, but the selection is a {}",
                    roi.kind()
                )));
            }
            let area = roi
                .to_area()
                .ok_or_else(|| Error::failed("the line covers no pixel"))?;
            set(ctx, p.input, area)
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.enlarge", "Enlarge Selection")
            .summary("Grows an area selection by a number of pixels, or shrinks it.")
            .doc(
                "A rectangle or oval keeps its shape. Any other selection becomes an outline \
                 along pixel edges, as in ImageJ.",
            )
            .menu("Edit > Selection", 410.0)
            .ijm_name("Enlarge...")
            .effects(modifies()),
        |ctx, p: EnlargeParams| {
            let (roi, _, _) = area_selection(ctx, p.input)?;
            let enlarged = roi.enlarged(p.pixels as i32).ok_or_else(|| {
                Error::failed(format!(
                    "shrinking the selection by {} pixels would leave nothing selected",
                    -p.pixels
                ))
            })?;
            set(ctx, p.input, enlarged)
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.scale", "Scale Selection")
            .summary("Makes the selection larger or smaller by a factor.")
            .doc(
                "Rectangles become polygons and ovals freehand selections, as in ImageJ. \
                 A centred scale rounds the result to whole pixels.",
            )
            .menu("Edit > Selection", 420.0)
            .ijm_name("Scale... ")
            .effects(modifies()),
        |ctx, p: ScaleParams| {
            let (roi, width, height) = selection(ctx, p.input)?;
            let center = if p.centered {
                Some(
                    roi.scale_center(width, height)
                        .ok_or_else(|| Error::failed("the selection is outside the image"))?,
                )
            } else {
                None
            };
            set(ctx, p.input, roi.scaled(p.x_scale, p.y_scale, center))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.rotate", "Rotate Selection")
            .summary("Turns the selection by an angle.")
            .doc("Rectangles become polygons and ovals freehand selections, as in ImageJ.")
            .menu("Edit > Selection", 430.0)
            .ijm_name("Rotate...")
            .effects(modifies()),
        |ctx, p: RotateParams| {
            let (roi, width, height) = selection(ctx, p.input)?;
            let (x, y) = if p.around_image_center {
                (width as f64 / 2.0, height as f64 / 2.0)
            } else {
                roi.rotation_center()
            };
            set(ctx, p.input, roi.rotated(p.angle, x, y))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.translate", "Translate Selection")
            .summary("Moves the selection by a number of pixels.")
            .menu("Edit > Selection", 440.0)
            .ijm_name("Translate... ")
            .effects(modifies()),
        |ctx, p: TranslateParams| {
            let (roi, _, _) = selection(ctx, p.input)?;
            set(ctx, p.input, roi.translated(p.x_offset, p.y_offset))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.open", "Open Selection")
            .summary("Reads a selection from an ImageJ selection file and applies it to the image.")
            .menu("Edit > Selection", 510.0)
            .effects(Effects {
                image: ImageEffect::Modifies,
                reads_fs: true,
                ..Effects::default()
            }),
        |ctx, p: OpenParams| {
            ctx.workspace().get(p.input)?;
            let bytes = std::fs::read(&p.path)
                .map_err(|e| Error::failed(format!("cannot read {}: {e}", p.path.display())))?;
            if bytes.starts_with(b"PK") {
                return Err(Error::failed(format!(
                    "{} is a set of selections; only single selection files (.roi) can be \
                     opened as the selection",
                    p.path.display()
                )));
            }
            let entry = calipix_roi::decode(&bytes)
                .map_err(|e| Error::failed(format!("cannot open {}: {e}", p.path.display())))?;
            set(ctx, p.input, entry.roi)
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.save", "Save Selection")
            .summary("Saves the selection as an ImageJ selection file.")
            .menu("Edit > Selection", 520.0)
            .ijm_name("Selection...")
            .effects(Effects {
                writes_fs: true,
                ..Effects::default()
            }),
        |ctx, p: SaveParams| {
            let (roi, _, _) = selection(ctx, p.input)?;
            let bytes = calipix_roi::encode(&RoiEntry::new(roi)).map_err(Error::failed)?;
            std::fs::write(&p.path, bytes)
                .map_err(|e| Error::failed(format!("cannot write {}: {e}", p.path.display())))?;
            Ok(Outputs::new())
        },
    ))?;

    Ok(())
}
