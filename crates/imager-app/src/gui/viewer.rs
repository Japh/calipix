//! The contents of an image tab: the viewer widget of `imager-view`, one
//! per open image, kept in step with the application state.
//!
//! The state says what to show (which image, plane and channels); the
//! widget owns how it is shown (zoom, pan, loaded tiles) and reports back
//! what its own controls changed. Without a GPU, as in the tests, the
//! widget draws through the CPU renderer; nothing here differs.

use std::collections::HashMap;

use eframe::egui;
use imager_command::DatasetId;
use imager_core::Image;
use imager_render::DisplayOptions;
use imager_view::{ImageView, Probe, Renderer, ViewOptions, ViewerGpu};

use super::ui;
use crate::state::{AppState, Zoom};

/// What the viewer reports back for the status bar and tab handling.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewerResponse {
    /// The image area was clicked, which makes its image the active one.
    pub clicked: bool,
    /// What is under the pointer.
    pub probe: Option<Probe>,
    /// Magnification: 1.0 is one image pixel per screen pixel.
    pub zoom: Option<f64>,
    /// Parts of the picture are still being read.
    pub loading: bool,
    /// True if the CPU renderer drew the picture rather than the GPU.
    pub on_cpu: bool,
}

struct Slot {
    view: ImageView,
    /// The image last handed to the widget, and the version of its
    /// samples, to tell a change of display from a change of data.
    shown: Option<(Image, u64)>,
    /// The last problem passed on to the status bar, so that it is
    /// reported once rather than every frame.
    reported: Option<String>,
    /// True until the first frame has chosen the opening zoom.
    opening: bool,
}

/// The zoom a newly opened image starts at: the largest standard step at
/// which the whole image still fits, so small images are not specks and
/// pixels stay evenly sized. `fit` is the zoom that exactly fills the view.
fn opening_zoom(fit: f64) -> f64 {
    imager_view::geometry::ZOOM_LEVELS
        .iter()
        .copied()
        .filter(|&step| step <= fit)
        .fold(None, |best: Option<f64>, step| {
            Some(best.map_or(step, |b| b.max(step)))
        })
        .unwrap_or(fit)
}

/// Views of the open images, one per dataset.
pub struct Viewer {
    gpu: Option<ViewerGpu>,
    options: ViewOptions,
    views: HashMap<DatasetId, Slot>,
}

impl Viewer {
    /// A viewer drawing with `gpu`, or through the CPU renderer without
    /// one. `force_cpu` keeps to the CPU renderer even with a GPU.
    pub fn new(gpu: Option<ViewerGpu>, force_cpu: bool) -> Viewer {
        let [r, g, b, _] = ui::IMAGE_BACKGROUND.to_array();
        Viewer {
            gpu,
            options: ViewOptions {
                force_cpu,
                background: [r, g, b],
                ..ViewOptions::default()
            },
            views: HashMap::new(),
        }
    }

    /// What the pictures are drawn with, for the About box.
    pub fn renderer_description(&self) -> String {
        match &self.gpu {
            Some(gpu) if self.options.force_cpu => {
                format!(
                    "CPU renderer (by request; {} is available)",
                    gpu.adapter_name()
                )
            }
            Some(gpu) if gpu.is_software() => format!(
                "CPU renderer ({} is a software adapter)",
                gpu.adapter_name()
            ),
            Some(gpu) => format!("{} ({})", gpu.adapter_name(), gpu.backend()),
            None => "CPU renderer (no GPU)".to_string(),
        }
    }

    /// Draws image `id` filling the available space of `ui`, with its
    /// position controls underneath.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut AppState,
        id: DatasetId,
    ) -> ViewerResponse {
        let Some(image) = state.displayed(id) else {
            return ViewerResponse::default();
        };
        let version = state.samples_version(id);
        let options = self.options.clone();
        let slot = self.views.entry(id).or_insert_with(|| Slot {
            view: ImageView::new(options),
            shown: None,
            reported: None,
            opening: true,
        });
        if slot.opening {
            slot.view.fit();
        }
        // New metadata over the same samples keeps everything loaded.
        if let Some((shown, shown_version)) = &slot.shown
            && !shown.ptr_eq(&image)
            && *shown_version == version
        {
            slot.view.metadata_changed(&image);
        }
        slot.shown = Some((image.clone(), version));

        let meta = image.meta();
        let view_state = state.view(id);
        let position = view_state.position_in(meta);
        if slot.view.position() != position {
            slot.view.set_position(&position);
        }
        let wanted = DisplayOptions {
            mode: imager_commands::view::channel_mode(meta),
            visible: view_state.visible.clone(),
            ..slot.view.display_options().clone()
        };
        if slot.view.display_options() != &wanted {
            slot.view.set_display_options(wanted);
        }

        // The image fills the tab, above a row of position controls for
        // images that have planes or channels to choose between.
        let gpu = self.gpu.as_ref();
        let shown = ui
            .with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                if imager_view::controls::has_controls(meta) {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(ui::MARGIN, 4))
                        .show(ui, |ui| slot.view.position_controls(ui, &image));
                }
                slot.view.ui(ui, &image, gpu)
            })
            .inner;
        if slot.opening {
            // The widget has just worked out the fitting zoom; step down to
            // a standard level and draw again.
            slot.opening = false;
            slot.view.set_zoom(opening_zoom(slot.view.zoom()));
            ui.ctx().request_repaint();
        }
        let name = &meta.name;
        shown.response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Image, true, format!("View of {name}"))
        });

        // The sliders and channel buttons under the image are the
        // widget's; what they changed goes back to the state.
        if slot.view.position() != position {
            state.set_position(id, slot.view.position());
        }
        let visible = &slot.view.display_options().visible;
        for (channel, &on) in visible.iter().enumerate() {
            if state.channel_visible(id, channel) != on {
                state.set_channel_visible(id, channel, on);
            }
        }

        if shown.error != slot.reported {
            if let Some(problem) = &shown.error {
                state.report_problem(format!("{name}: {problem}"));
            }
            slot.reported = shown.error.clone();
        }
        ViewerResponse {
            clicked: shown.response.clicked(),
            probe: shown.probe,
            zoom: Some(shown.zoom),
            loading: shown.loading,
            on_cpu: shown.renderer == Renderer::Cpu,
        }
    }

    /// Changes the magnification of one image's view.
    pub fn zoom(&mut self, id: DatasetId, zoom: Zoom) {
        let Some(slot) = self.views.get_mut(&id) else {
            return;
        };
        match zoom {
            Zoom::Fit => slot.view.fit(),
            Zoom::Actual => slot.view.actual_size(),
            Zoom::In => slot.view.zoom_in(),
            Zoom::Out => slot.view.zoom_out(),
        }
    }

    /// Steps one image's view along the slider used last. Returns false
    /// if it was already at the end.
    pub fn step(&mut self, state: &mut AppState, id: DatasetId, delta: i64) -> bool {
        let Some(slot) = self.views.get_mut(&id) else {
            return false;
        };
        let moved = slot.view.step_last_axis(delta);
        state.set_position(id, slot.view.position());
        moved
    }

    /// Frees the views of images that are no longer open.
    pub fn retain(&mut self, open: impl Fn(DatasetId) -> bool) {
        self.views.retain(|id, _| open(*id));
    }
}

#[cfg(test)]
mod tests {
    use super::opening_zoom;

    #[test]
    fn images_open_at_the_largest_standard_zoom_that_fits() {
        assert_eq!(opening_zoom(1.0), 1.0);
        assert_eq!(opening_zoom(1.46), 1.0);
        assert_eq!(opening_zoom(1.5), 1.5);
        assert_eq!(opening_zoom(31.0), 24.0);
        assert_eq!(opening_zoom(0.4), 1.0 / 3.0);
        // Larger than the largest step, or smaller than the smallest.
        assert_eq!(opening_zoom(100.0), 32.0);
        assert_eq!(opening_zoom(0.001), 0.001);
    }
}
