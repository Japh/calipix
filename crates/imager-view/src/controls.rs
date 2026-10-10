//! Position controls shown under an image: a slider for each axis beyond
//! X and Y (Z, time, ...) and toggle buttons for channels.

use imager_core::{AxisKind, ColorModel, ImageMeta};

/// True if an image has anything for [`position_controls`] to show.
pub fn has_controls(meta: &ImageMeta) -> bool {
    meta.axes().iter().skip(2).any(|axis| match axis.kind {
        // An RGB image's channels are not separately viewable.
        AxisKind::Channel => meta.color != ColorModel::Rgb,
        _ => true,
    })
}

/// The short label of an axis' slider.
pub fn axis_label(kind: &AxisKind) -> String {
    match kind {
        AxisKind::X => "X".to_string(),
        AxisKind::Y => "Y".to_string(),
        AxisKind::Z => "Z".to_string(),
        AxisKind::Time => "T".to_string(),
        AxisKind::Channel => "C".to_string(),
        AxisKind::Other(name) => name.clone(),
    }
}

/// Draws the controls in one wrapping row and applies changes.
///
/// - Sliders show positions counted from 1, as ImageJ does.
/// - For a composite image each channel has a toggle button that shows or
///   hides it. For a grayscale image the buttons select the one channel
///   shown. RGB images have no channel buttons.
///
/// `position` holds one index per axis beyond X and Y, `visible` one flag
/// per channel, and `last_axis` is set to the slot of the slider that was
/// moved. Returns true if anything changed.
pub fn position_controls(
    ui: &mut egui::Ui,
    meta: &ImageMeta,
    position: &mut [usize],
    visible: &mut [bool],
    last_axis: &mut Option<usize>,
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for (index, axis) in meta.axes().iter().enumerate().skip(2) {
            let slot = index - 2;
            let size = meta.shape()[index];
            let Some(current) = position.get_mut(slot) else {
                continue;
            };
            if axis.kind != AxisKind::Channel {
                // The label goes before its slider, as in the layout sketch
                // in docs/ux.md, and names it for assistive technology.
                let label = ui.label(axis_label(&axis.kind));
                let mut shown = *current + 1;
                let slider = egui::Slider::new(&mut shown, 1..=size).integer();
                if ui.add(slider).labelled_by(label.id).changed() {
                    *current = shown.clamp(1, size) - 1;
                    *last_axis = Some(slot);
                    changed = true;
                }
                continue;
            }
            if meta.color == ColorModel::Rgb {
                continue;
            }
            ui.label("C");
            for c in 0..size {
                let name = meta.channels.get(c).and_then(|ch| ch.name.clone());
                let hint = match &name {
                    Some(name) => format!("Channel {}: {name}", c + 1),
                    None => format!("Channel {}", c + 1),
                };
                let text = format!("{}", c + 1);
                if meta.color == ColorModel::Composite {
                    let Some(on) = visible.get_mut(c) else {
                        continue;
                    };
                    if ui.toggle_value(on, text).on_hover_text(hint).changed() {
                        changed = true;
                    }
                } else if ui
                    .selectable_label(*current == c, text)
                    .on_hover_text(hint)
                    .clicked()
                    && *current != c
                {
                    *current = c;
                    changed = true;
                }
            }
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use imager_core::DType;

    #[test]
    fn controls_exist_only_for_extra_axes() {
        assert!(!has_controls(&ImageMeta::plane(DType::U8, 4, 4).unwrap()));
        assert!(has_controls(
            &ImageMeta::xyczt(DType::U8, 4, 4, 1, 3, 1).unwrap()
        ));
        assert!(has_controls(
            &ImageMeta::xyczt(DType::U8, 4, 4, 2, 1, 1).unwrap()
        ));
        let mut rgb = ImageMeta::xyczt(DType::U8, 4, 4, 3, 1, 1).unwrap();
        rgb.color = ColorModel::Rgb;
        assert!(!has_controls(&rgb));
        let mut rgb_stack = ImageMeta::xyczt(DType::U8, 4, 4, 3, 5, 1).unwrap();
        rgb_stack.color = ColorModel::Rgb;
        assert!(has_controls(&rgb_stack));
    }
}
