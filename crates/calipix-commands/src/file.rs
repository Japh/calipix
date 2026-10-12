//! Opening, saving and closing images.
//!
//! # Selections stored in files
//!
//! ImageJ saves an image's selection and overlay in its TIFF files. The
//! TIFF reader keeps them, undecoded, in the image properties `imagej.roi`
//! and `imagej.overlay.N`; Open decodes them into the image's selection
//! and overlay, and Save and Save As write the current selection and
//! overlay back, so that ImageJ shows them.
//!
//! The workspace holds a selection as its shape only. What else ImageJ
//! stores with one (its name, colours, stroke width, position in a stack)
//! therefore stays in the properties the image was opened with: on saving,
//! a selection or overlay element that still has the shape it was opened
//! with is written back exactly as it was read, and anything new or
//! changed is written as a plain selection. Overlay elements Calipix has
//! no shape for (text and images) cannot be shown or changed here and are
//! written back as they were read.
//!
//! A stored selection that cannot be read does not stop the image from
//! opening: the `warning` output says what was left out and why.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calipix_command::{
    CommandSpec, Dataset, DatasetId, Effects, Error, ImageEffect, Outputs, ParamType, Params,
    Registry, Result, typed,
};
use calipix_core::Image;
use calipix_roi::{Roi, RoiEntry};

use crate::InputOnly;

/// The image property that holds the selection ImageJ stored in a file.
const ROI_PROPERTY: &str = "imagej.roi";
/// The image properties `imagej.overlay.0`, `imagej.overlay.1`, ... hold
/// the elements of the overlay ImageJ stored in a file.
const OVERLAY_PROPERTY: &str = "imagej.overlay.";

/// The overlay elements stored in the properties, in order, as hex.
fn stored_overlay(properties: &BTreeMap<String, String>) -> Vec<&str> {
    (0..)
        .map_while(|i| properties.get(&format!("{OVERLAY_PROPERTY}{i}")))
        .map(String::as_str)
        .collect()
}

/// One element of an image's overlay as the workspace holds it.
///
/// This and [`OverlayElement::entry`] are the only places that convert
/// between what ImageJ stores about an overlay element ([`RoiEntry`]: the
/// shape with its name, colours and position) and what
/// `Workspace::set_overlay` takes. The workspace holds the shape alone
/// today; once it holds entries, `from_entry` and `entry` become the
/// identity and the stored bytes no longer need to be matched on saving.
type OverlayElement = Roi;

/// The workspace's form of a decoded overlay element.
fn from_entry(entry: RoiEntry) -> OverlayElement {
    entry.roi
}

/// What is known of an overlay element of the workspace, for encoding.
fn entry_of(element: &OverlayElement) -> RoiEntry {
    RoiEntry::new(element.clone())
}

/// What an image opened from a file brings with it.
#[derive(Default)]
struct Stored {
    roi: Option<Roi>,
    overlay: Vec<OverlayElement>,
    /// What could not be read, for the user.
    notes: Vec<String>,
}

/// Decodes the selection and overlay a file stored with `image`.
fn stored_selections(image: &Image) -> Stored {
    let properties = &image.meta().properties;
    let mut stored = Stored::default();
    if let Some(hex) = properties.get(ROI_PROPERTY) {
        match calipix_roi::decode_hex(hex) {
            Ok(entry) => stored.roi = Some(entry.roi),
            Err(e) => stored.notes.push(format!(
                "the selection stored in the file was not read: {e}"
            )),
        }
    }
    for (i, hex) in stored_overlay(properties).into_iter().enumerate() {
        match calipix_roi::decode_hex(hex) {
            Ok(entry) => stored.overlay.push(from_entry(entry)),
            Err(e) => stored.notes.push(format!(
                "element {} of the overlay was left out: {e}",
                i + 1
            )),
        }
    }
    stored
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut hex, byte| {
        write!(hex, "{byte:02x}").expect("writing to a string cannot fail");
        hex
    })
}

/// The image as it is saved: with the dataset's selection and overlay in
/// the properties the TIFF writer stores them from, and what could not be
/// stored.
fn with_selections(dataset: &Dataset) -> Result<(Image, Vec<String>)> {
    let image = &dataset.image;
    let mut meta = image.meta().clone();
    let mut notes = Vec::new();

    // What the image was opened with, to keep what ImageJ stored about
    // selections that have not changed.
    let old_roi = meta.properties.get(ROI_PROPERTY).cloned();
    let old_overlay: Vec<String> = stored_overlay(&meta.properties)
        .into_iter()
        .map(str::to_string)
        .collect();
    meta.properties
        .retain(|key, _| key != ROI_PROPERTY && !key.starts_with(OVERLAY_PROPERTY));

    let encode = |entry: &RoiEntry, what: &str, notes: &mut Vec<String>| -> Option<String> {
        match calipix_roi::encode(entry) {
            Ok(bytes) => Some(to_hex(&bytes)),
            Err(e) => {
                notes.push(format!("{what} was not saved: {e}"));
                None
            }
        }
    };
    let same_shape =
        |hex: &str, roi: &Roi| calipix_roi::decode_hex(hex).is_ok_and(|entry| &entry.roi == roi);

    if let Some(roi) = &dataset.roi {
        let hex = match old_roi {
            Some(old) if same_shape(&old, roi) => Some(old),
            _ => encode(&RoiEntry::new(roi.clone()), "the selection", &mut notes),
        };
        if let Some(hex) = hex {
            meta.properties.insert(ROI_PROPERTY.to_string(), hex);
        }
    }

    // Each element of the overlay takes the stored element of the same
    // shape, if one is left; the elements Calipix has no shape for keep
    // their place among those.
    let mut unused: Vec<Option<String>> = old_overlay.into_iter().map(Some).collect();
    let mut written: Vec<String> = Vec::new();
    for (i, element) in dataset.overlay.iter().enumerate() {
        let entry = entry_of(element);
        let roi = &entry.roi;
        let found = unused
            .iter()
            .position(|old| old.as_deref().is_some_and(|hex| same_shape(hex, roi)));
        match found {
            Some(at) => {
                // Elements that could not be shown and came before it.
                for earlier in &mut unused[..at] {
                    if earlier.as_deref().is_some_and(unsupported) {
                        written.extend(earlier.take());
                    }
                }
                written.extend(unused[at].take());
            }
            None => {
                let what = format!("element {} of the overlay", i + 1);
                written.extend(encode(&entry, &what, &mut notes));
            }
        }
    }
    for rest in &mut unused {
        if rest.as_deref().is_some_and(unsupported) {
            written.extend(rest.take());
        }
    }
    for (i, hex) in written.into_iter().enumerate() {
        meta.properties
            .insert(format!("{OVERLAY_PROPERTY}{i}"), hex);
    }

    if &meta == image.meta() {
        return Ok((image.clone(), notes));
    }
    Ok((image.with_meta(meta)?, notes))
}

/// True for a stored selection of a kind ImageJ has and Calipix does not
/// (text, an image).
fn unsupported(hex: &str) -> bool {
    matches!(
        calipix_roi::decode_hex(hex),
        Err(calipix_roi::Error::Unsupported(_))
    )
}

/// Saves `dataset` to `path` with its selection and overlay.
fn save(dataset: &Dataset, path: &std::path::Path) -> Result<Outputs> {
    let (image, notes) = with_selections(dataset)?;
    calipix_io::save(&image, path).map_err(Error::failed)?;
    Ok(with_warning(Outputs::new(), &notes))
}

fn with_warning(outputs: Outputs, notes: &[String]) -> Outputs {
    if notes.is_empty() {
        outputs
    } else {
        outputs.with("warning", notes.join("; "))
    }
}

#[derive(Params)]
struct OpenParams {
    /// The image file to open.
    #[param(label = "File", no_persist)]
    path: PathBuf,
}

#[derive(Params)]
struct SaveAsParams {
    /// The image to save.
    #[param(label = "Image")]
    input: DatasetId,
    /// Where to save it. The extension chooses the file format.
    #[param(label = "File", path = "save", no_persist)]
    path: PathBuf,
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        CommandSpec::new("file.open", "Open")
            .summary("Opens an image file.")
            .menu("File", 10.0)
            .shortcut("Ctrl+O")
            .output("image", ParamType::Image)
            .output("warning", ParamType::Str)
            .effects(Effects {
                image: ImageEffect::Creates,
                reads_fs: true,
                ..Effects::default()
            }),
        |ctx, p: OpenParams| {
            let image = calipix_io::open(&p.path).map_err(Error::failed)?;
            let stored = stored_selections(&image);
            let id = ctx.workspace_mut().insert(image, Some(p.path));
            if stored.roi.is_some() {
                ctx.workspace_mut().set_roi(id, stored.roi)?;
            }
            if !stored.overlay.is_empty() {
                ctx.workspace_mut().set_overlay(id, stored.overlay)?;
            }
            Ok(with_warning(
                Outputs::new().with("image", id),
                &stored.notes,
            ))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("file.save", "Save")
            .summary("Saves an image to the file it was opened from or last saved to.")
            .menu("File", 20.0)
            .shortcut("Ctrl+S")
            .output("warning", ParamType::Str)
            .effects(Effects {
                writes_fs: true,
                ..Effects::default()
            }),
        |ctx, p: InputOnly| {
            let dataset = ctx.workspace().get(p.input)?;
            let Some(path) = dataset.path.clone() else {
                return Err(Error::failed(
                    "this image has not been saved to a file yet; use Save As",
                ));
            };
            save(dataset, &path)
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("file.save_as", "Save As")
            .summary("Saves an image to a file in the format given by its extension.")
            .menu("File", 21.0)
            .shortcut("Ctrl+Shift+S")
            .output("warning", ParamType::Str)
            .effects(Effects {
                writes_fs: true,
                ..Effects::default()
            }),
        |ctx, p: SaveAsParams| {
            let outputs = save(ctx.workspace().get(p.input)?, &p.path)?;
            ctx.workspace_mut().set_path(p.input, Some(p.path))?;
            Ok(outputs)
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("file.close", "Close")
            .summary("Closes an image without saving it.")
            .menu("File", 30.0)
            .shortcut("Ctrl+W")
            .ijm_name("Close")
            .effects(Effects {
                image: ImageEffect::Modifies,
                ..Effects::default()
            }),
        |ctx, p: InputOnly| {
            ctx.workspace_mut().remove(p.input)?;
            Ok(Outputs::new())
        },
    ))?;

    Ok(())
}
