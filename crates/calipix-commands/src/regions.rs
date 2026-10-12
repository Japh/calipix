//! Region commands: the list of selections kept with an image.
//!
//! An image has one selection, which commands act within, and a list of
//! saved regions (`Dataset::overlay`). ImageJ splits the list in two, the
//! ROI Manager and the overlay; here there is one. Regions are drawn over
//! the image, listed in the Regions panel, stored in a TIFF as ImageJ's
//! overlay, and read from and written to ROI Manager sets (`.zip`).
//!
//! Regions are named by their place in the list, counted from 1, singly
//! or as ranges: `3`, `2-4,9`. Every command here changes only the
//! workspace, so each is one step in History and is undone like any other.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{
    Args, Command, CommandCtx, CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs,
    ParamType, Params, Registry, Result, typed,
};
use calipix_core::Cell;
use calipix_ops::measure::LABEL;
use calipix_roi::{Color, RoiEntry};

#[derive(Params)]
struct AddParams {
    /// The image whose selection is kept as a region.
    #[param(label = "Image")]
    input: DatasetId,
    /// What to call the region. Without a name it is listed by its number.
    #[param(label = "Name", advanced, no_persist)]
    name: Option<String>,
}

#[derive(Params)]
struct ListParams {
    /// The image whose regions are meant.
    #[param(label = "Image")]
    input: DatasetId,
    /// The regions, by their numbers in the list: single regions and
    /// ranges separated by commas, such as `2-4,9`.
    #[param(label = "Regions", no_persist)]
    regions: String,
}

#[derive(Params)]
struct OneParams {
    /// The image the region belongs to.
    #[param(label = "Image")]
    input: DatasetId,
    /// The region's number in the list, counted from 1.
    #[param(label = "Region", default = 1, min = 1, no_persist)]
    region: i64,
}

#[derive(Params)]
struct RenameParams {
    /// The image the region belongs to.
    #[param(label = "Image")]
    input: DatasetId,
    /// The region's number in the list, counted from 1.
    #[param(label = "Region", default = 1, min = 1, no_persist)]
    region: i64,
    /// The new name. An empty name removes it, and the region is listed
    /// by its number again.
    #[param(label = "Name", no_persist)]
    name: String,
}

#[derive(Params)]
struct ColorParams {
    /// The image whose regions are recoloured.
    #[param(label = "Image")]
    input: DatasetId,
    /// The colour of the outline: a name such as `red`, `cyan` or
    /// `orange`, or `#rrggbb`. `none` goes back to the default colour.
    #[param(label = "Outline color", default = "cyan")]
    color: String,
    /// The colour the region is filled with, as a name, `#rrggbb` or, to
    /// let the image show through, `#aarrggbb`. `none` leaves it unfilled.
    #[param(label = "Fill color", default = "none", advanced)]
    fill: String,
    /// The regions to recolour, such as `2-4,9`; all of them if left out.
    #[param(label = "Regions", advanced, no_persist)]
    regions: Option<String>,
}

#[derive(Params)]
struct MeasureParams {
    /// The image to measure.
    #[param(label = "Image")]
    input: DatasetId,
    /// The regions to measure, such as `2-4,9`; all of them if left out.
    #[param(label = "Regions", advanced, no_persist)]
    regions: Option<String>,
    /// Channel to measure, counted from 1. 0, which interfaces give when
    /// every channel is shown together, measures the first.
    #[param(default = 0, min = 0, advanced, no_persist, position = "channel")]
    channel: i64,
    /// Slice to measure, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "slice")]
    slice: i64,
    /// Frame to measure, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "frame")]
    frame: i64,
    /// The table the rows are added to.
    #[param(label = "Table", default = "Results", advanced, no_persist)]
    table: String,
}

#[derive(Params)]
struct OpenParams {
    /// The image that gets the regions.
    #[param(label = "Image")]
    input: DatasetId,
    /// An ImageJ ROI Manager set (`.zip`) or a single selection file
    /// (`.roi`).
    #[param(label = "File", path = "open", no_persist)]
    path: PathBuf,
    /// Replace the regions the image has, instead of adding to them.
    #[param(label = "Replace existing regions", default = false, advanced)]
    replace: bool,
}

#[derive(Params)]
struct SaveParams {
    /// The image whose regions are saved.
    #[param(label = "Image")]
    input: DatasetId,
    /// Where to save them. A name ending in `.roi` saves one region as an
    /// ImageJ selection file; any other name saves a ROI Manager set
    /// (`.zip`).
    #[param(label = "File", path = "save", no_persist)]
    path: PathBuf,
    /// The regions to save, such as `2-4,9`; all of them if left out.
    #[param(label = "Regions", advanced, no_persist)]
    regions: Option<String>,
}

#[derive(Params)]
struct ClearParams {
    /// The image whose regions are removed.
    #[param(label = "Image")]
    input: DatasetId,
}

/// Reads `2-4,9` as places in a list of `len` regions, counted from 0.
/// A number past the end is an error rather than ignored: a recorded step
/// that no longer fits the list should say so.
fn parse_regions(text: &str, len: usize) -> std::result::Result<BTreeSet<usize>, String> {
    let mut regions = BTreeSet::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let number = |text: &str| {
            text.trim()
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| format!("`{part}` is not a region number or a range such as 2-4"))
        };
        let (first, last) = (number(first)?, number(last)?);
        if first > last {
            return Err(format!("`{part}` runs backwards"));
        }
        if last > len {
            return Err(match len {
                0 => format!("there is no region {last}; the image has none"),
                1 => format!("there is no region {last}; the image has 1"),
                len => format!("there is no region {last}; the image has {len}"),
            });
        }
        regions.extend(first - 1..last);
    }
    if regions.is_empty() {
        return Err("no regions were given".to_string());
    }
    Ok(regions)
}

/// A colour given as a name or as hexadecimal digits, the way ImageJ's
/// `Colors.decode` reads them. `none` is no colour.
fn parse_color(text: &str) -> std::result::Result<Option<Color>, String> {
    let text = text.trim();
    let named = match text.to_ascii_lowercase().as_str() {
        "none" | "" => return Ok(None),
        "black" => Some((0, 0, 0)),
        "blue" => Some((0, 0, 255)),
        "cyan" => Some((0, 255, 255)),
        "darkgray" | "darkgrey" => Some((64, 64, 64)),
        "gray" | "grey" => Some((128, 128, 128)),
        "green" => Some((0, 255, 0)),
        "lightgray" | "lightgrey" => Some((192, 192, 192)),
        "magenta" => Some((255, 0, 255)),
        "orange" => Some((255, 200, 0)),
        "pink" => Some((255, 175, 175)),
        "red" => Some((255, 0, 0)),
        "white" => Some((255, 255, 255)),
        "yellow" => Some((255, 255, 0)),
        _ => None,
    };
    if let Some((r, g, b)) = named {
        return Ok(Some(Color::rgb(r, g, b)));
    }
    let digits = text.strip_prefix('#').unwrap_or(text);
    let value = u32::from_str_radix(digits, 16).ok();
    match (digits.len(), value) {
        (6, Some(rgb)) => Ok(Some(Color(0xff00_0000 | rgb))),
        (8, Some(argb)) => Ok(Some(Color(argb))),
        _ => Err(format!(
            "`{text}` is not a colour; use a name such as red, cyan or orange, or #rrggbb"
        )),
    }
}

fn modifies() -> Effects {
    Effects {
        image: ImageEffect::Modifies,
        ..Effects::default()
    }
}

fn regions_of(ctx: &CommandCtx<'_>, id: DatasetId) -> Result<Vec<RoiEntry>> {
    Ok(ctx.workspace().get(id)?.overlay.clone())
}

fn no_regions() -> Error {
    Error::failed("this image has no regions; add one from a selection first")
}

/// The regions named by `text`, or all of them if it is absent.
fn chosen(text: Option<&str>, len: usize) -> Result<BTreeSet<usize>> {
    if len == 0 {
        return Err(no_regions());
    }
    match text.filter(|t| !t.trim().is_empty()) {
        None => Ok((0..len).collect()),
        Some(text) => parse_regions(text, len).map_err(|reason| Error::InvalidArg {
            param: "regions".to_string(),
            reason,
        }),
    }
}

/// The place of region `number` (from 1) in a list of `len`.
fn one(number: i64, len: usize) -> Result<usize> {
    if len == 0 {
        return Err(no_regions());
    }
    if number < 1 || number as u64 > len as u64 {
        return Err(Error::InvalidArg {
            param: "region".to_string(),
            reason: format!("{number} is outside 1 to {len}"),
        });
    }
    Ok(number as usize - 1)
}

fn count_output(len: usize) -> Outputs {
    Outputs::new().with("count", len)
}

fn run_add(ctx: &mut CommandCtx<'_>, p: AddParams) -> Result<Outputs> {
    let dataset = ctx.workspace().get(p.input)?;
    let roi = dataset.roi.clone().ok_or_else(|| {
        Error::failed("this image has no selection to add; draw or specify one first")
    })?;
    let mut regions = dataset.overlay.clone();
    let mut entry = RoiEntry::new(roi);
    entry.props.name = p
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    regions.push(entry);
    let count = regions.len();
    ctx.workspace_mut().set_overlay(p.input, regions)?;
    Ok(count_output(count).with("region", count))
}

/// Measures each chosen region with the Measure command itself, so that a
/// region's row is the row Measure gives for the same selection.
fn run_measure(
    measure: &Arc<dyn Command>,
    ctx: &mut CommandCtx<'_>,
    p: MeasureParams,
) -> Result<Outputs> {
    let dataset = ctx.workspace().get(p.input)?;
    let regions = dataset.overlay.clone();
    let selection = dataset.roi.clone();
    let chosen = chosen(p.regions.as_deref(), regions.len())?;
    let args = Args::new()
        .with("input", p.input)
        .with("channel", p.channel)
        .with("slice", p.slice)
        .with("frame", p.frame)
        .with("table", p.table.as_str());
    let before = ctx.workspace().table(&p.table).map_or(0, |t| t.len());
    for (done, &index) in chosen.iter().enumerate() {
        ctx.check_cancelled()?;
        ctx.progress(done as f64 / chosen.len() as f64, "Measuring regions");
        let entry = &regions[index];
        ctx.workspace_mut()
            .set_roi(p.input, Some(entry.roi.clone()))?;
        let rows = ctx.workspace().table(&p.table).map_or(0, |t| t.len());
        measure.run(ctx, &args).map_err(|e| match e {
            Error::Failed(reason) => Error::failed(format!("region {}: {reason}", index + 1)),
            other => other,
        })?;
        // As ImageJ's `Analyzer.getFileName`: a named selection's label
        // ends with its name.
        if let Some(name) = &entry.props.name
            && let Some(mut table) = ctx.workspace().table(&p.table).cloned()
            && table.has_column(LABEL)
        {
            for row in rows..table.len() {
                if let Some(label) = table.get(row, LABEL).as_text()
                    && !label.ends_with(name.as_str())
                {
                    let label = format!("{label}:{name}");
                    table.set(row, LABEL, Cell::from(label));
                }
            }
            ctx.workspace_mut().set_table(p.table.clone(), table);
        }
    }
    // Measuring is not meant to change the selection or, whatever "Add to
    // overlay" says in Set Measurements, to list the regions twice.
    ctx.workspace_mut().set_roi(p.input, selection)?;
    ctx.workspace_mut().set_overlay(p.input, regions)?;
    let after = ctx.workspace().table(&p.table).map_or(0, |t| t.len());
    Ok(Outputs::new()
        .with("table", p.table.as_str())
        .with("row", after)
        .with("rows", after.saturating_sub(before))
        .with("count", chosen.len()))
}

fn run_open(ctx: &mut CommandCtx<'_>, p: OpenParams) -> Result<Outputs> {
    let mut regions = regions_of(ctx, p.input)?;
    let shown = p.path.display();
    let bytes =
        std::fs::read(&p.path).map_err(|e| Error::failed(format!("cannot read {shown}: {e}")))?;
    let (opened, skipped) = if bytes.starts_with(b"PK") {
        let set = calipix_roi::read_zip(&bytes)
            .map_err(|e| Error::failed(format!("cannot open {shown}: {e}")))?;
        if set.entries.is_empty() {
            let why = set
                .skipped
                .first()
                .map_or_else(String::new, |(name, e)| format!(" ({name}: {e})"));
            return Err(Error::failed(format!(
                "{shown} holds no selection that can be read{why}"
            )));
        }
        (set.entries, set.skipped.len())
    } else {
        let mut entry = calipix_roi::decode(&bytes)
            .map_err(|e| Error::failed(format!("cannot open {shown}: {e}")))?;
        // A selection file without a stored name is called after the file,
        // as an entry of a set is.
        if entry.props.name.is_none() {
            entry.props.name = p
                .path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned());
        }
        (vec![entry], 0)
    };
    let added = opened.len();
    if p.replace {
        regions.clear();
    }
    regions.extend(opened);
    let count = regions.len();
    ctx.workspace_mut().set_overlay(p.input, regions)?;
    Ok(count_output(count)
        .with("opened", added)
        .with("skipped", skipped))
}

fn run_save(ctx: &mut CommandCtx<'_>, p: SaveParams) -> Result<Outputs> {
    let regions = regions_of(ctx, p.input)?;
    let chosen = chosen(p.regions.as_deref(), regions.len())?;
    let entries: Vec<RoiEntry> = chosen.iter().map(|&i| regions[i].clone()).collect();
    let single = p
        .path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("roi"));
    let bytes = if single {
        let [entry] = entries.as_slice() else {
            return Err(Error::failed(format!(
                "a .roi file holds one selection, but {} regions were to be saved; \
                 give the file a name ending in .zip to save them as a set",
                entries.len()
            )));
        };
        calipix_roi::encode(entry).map_err(Error::failed)?
    } else {
        calipix_roi::write_zip(&entries).map_err(Error::failed)?
    };
    std::fs::write(&p.path, bytes)
        .map_err(|e| Error::failed(format!("cannot write {}: {e}", p.path.display())))?;
    Ok(count_output(entries.len()))
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        CommandSpec::new("regions.add", "Add to Regions")
            .summary("Keeps the selection as a region of the image.")
            .doc(
                "Regions are drawn over the image, listed in the Regions panel and saved with \
                 the image. The selection itself stays as it is.",
            )
            .menu("Edit > Regions", 610.0)
            .shortcut("T")
            .ijm_name("Add to Manager")
            .output("region", ParamType::Int)
            .output("count", ParamType::Int)
            .effects(modifies()),
        run_add,
    ))?;

    registry.register(typed(
        CommandSpec::new("regions.update", "Update Region")
            .summary("Replaces the shape of a region by the selection.")
            .doc("The region keeps its name, colour and place in the list.")
            .menu("Edit > Regions", 620.0)
            .output("count", ParamType::Int)
            .effects(modifies()),
        |ctx, p: OneParams| {
            let dataset = ctx.workspace().get(p.input)?;
            let roi = dataset.roi.clone().ok_or_else(|| {
                Error::failed("this image has no selection to give the region; make one first")
            })?;
            let mut regions = dataset.overlay.clone();
            let index = one(p.region, regions.len())?;
            regions[index].roi = roi;
            let count = regions.len();
            ctx.workspace_mut().set_overlay(p.input, regions)?;
            Ok(count_output(count))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("regions.select", "Select Region")
            .summary("Makes a region the selection of the image.")
            .doc("The region stays in the list; the selection is a copy of its shape.")
            .menu("Edit > Regions", 630.0)
            .effects(modifies()),
        |ctx, p: OneParams| {
            let regions = regions_of(ctx, p.input)?;
            let index = one(p.region, regions.len())?;
            ctx.workspace_mut()
                .set_roi(p.input, Some(regions[index].roi.clone()))?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("regions.rename", "Rename Region")
            .summary("Gives a region a name.")
            .menu("Edit > Regions", 640.0)
            .effects(modifies()),
        |ctx, p: RenameParams| {
            let mut regions = regions_of(ctx, p.input)?;
            let index = one(p.region, regions.len())?;
            let name = p.name.trim();
            regions[index].props.name = (!name.is_empty()).then(|| name.to_string());
            ctx.workspace_mut().set_overlay(p.input, regions)?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("regions.set_color", "Set Region Color")
            .summary("Changes the colour regions are drawn in.")
            .menu("Edit > Regions", 650.0)
            .output("count", ParamType::Int)
            .effects(modifies()),
        |ctx, p: ColorParams| {
            let mut regions = regions_of(ctx, p.input)?;
            let chosen = chosen(p.regions.as_deref(), regions.len())?;
            let color = |param: &str, text: &str| {
                parse_color(text).map_err(|reason| Error::InvalidArg {
                    param: param.to_string(),
                    reason,
                })
            };
            let (stroke, fill) = (color("color", &p.color)?, color("fill", &p.fill)?);
            for &index in &chosen {
                regions[index].props.stroke_color = stroke;
                regions[index].props.fill_color = fill;
            }
            ctx.workspace_mut().set_overlay(p.input, regions)?;
            Ok(count_output(chosen.len()))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("regions.delete", "Delete Regions")
            .summary("Removes regions from the image.")
            .doc(
                "Regions are given by their numbers in the list, as single regions and ranges \
                 separated by commas. Those that remain are numbered again from 1.",
            )
            .menu("Edit > Regions", 710.0)
            .output("count", ParamType::Int)
            .effects(modifies()),
        |ctx, p: ListParams| {
            let regions = regions_of(ctx, p.input)?;
            let remove = chosen(Some(&p.regions), regions.len())?;
            let kept: Vec<RoiEntry> = regions
                .into_iter()
                .enumerate()
                .filter(|(index, _)| !remove.contains(index))
                .map(|(_, entry)| entry)
                .collect();
            let count = kept.len();
            ctx.workspace_mut().set_overlay(p.input, kept)?;
            Ok(count_output(count))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("regions.clear", "Clear Regions")
            .summary("Removes every region from the image.")
            .menu("Edit > Regions", 720.0)
            .ijm_name("Remove Overlay")
            .effects(modifies()),
        |ctx, p: ClearParams| {
            if regions_of(ctx, p.input)?.is_empty() {
                return Err(no_regions());
            }
            ctx.workspace_mut().set_overlay(p.input, Vec::new())?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("regions.open", "Open Regions")
            .summary(
                "Adds the selections of an ImageJ ROI Manager set or selection file as regions.",
            )
            .menu("Edit > Regions", 810.0)
            .output("count", ParamType::Int)
            .output("opened", ParamType::Int)
            .output("skipped", ParamType::Int)
            .effects(Effects {
                image: ImageEffect::Modifies,
                reads_fs: true,
                ..Effects::default()
            }),
        run_open,
    ))?;

    registry.register(typed(
        CommandSpec::new("regions.save", "Save Regions")
            .summary("Saves the regions as an ImageJ ROI Manager set, or one as a selection file.")
            .menu("Edit > Regions", 820.0)
            .output("count", ParamType::Int)
            .effects(Effects {
                writes_fs: true,
                ..Effects::default()
            }),
        run_save,
    ))?;

    // The Measure command is looked up once, from a registry of its own
    // module, and run for each region with this command's context.
    let mut measuring = Registry::new();
    crate::measure::register(&mut measuring)?;
    let measure = measuring.get("measure.measure")?.clone();
    registry.register(typed(
        CommandSpec::new("regions.measure", "Measure Regions")
            .summary("Adds a row to the Results table for each region of the image.")
            .doc(
                "Each region is measured on the plane on view exactly as Measure measures a \
                 selection of that shape, with the quantities chosen in Set Measurements. The \
                 selection is left as it was.",
            )
            .menu("Analyze", 15.0)
            .ijm_name("Measure Overlay")
            .output("table", ParamType::Str)
            .output("row", ParamType::Int)
            .output("rows", ParamType::Int)
            .output("count", ParamType::Int),
        move |ctx, p: MeasureParams| run_measure(&measure, ctx, p),
    ))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_parse_as_the_list_numbers_them() {
        let regions = |text: &str| parse_regions(text, 10).map(|r| r.into_iter().collect());
        assert_eq!(regions("1"), Ok(vec![0]));
        assert_eq!(regions("2-4, 9"), Ok(vec![1, 2, 3, 8]));
        assert!(regions("").is_err());
        assert!(regions("0").is_err());
        assert!(regions("4-2").unwrap_err().contains("backwards"));
        assert!(regions("11").unwrap_err().contains("no region 11"));
        assert!(regions("two").unwrap_err().contains("not a region number"));
    }

    #[test]
    fn colours_are_names_or_hexadecimal() {
        assert_eq!(parse_color("red"), Ok(Some(Color::rgb(255, 0, 0))));
        assert_eq!(parse_color(" Cyan "), Ok(Some(Color::rgb(0, 255, 255))));
        assert_eq!(parse_color("#00ff80"), Ok(Some(Color::rgb(0, 255, 128))));
        assert_eq!(parse_color("8000ff00"), Ok(Some(Color(0x8000_ff00))));
        assert_eq!(parse_color("none"), Ok(None));
        assert!(parse_color("#12").is_err());
        assert!(parse_color("mauve").unwrap_err().contains("not a colour"));
    }
}
