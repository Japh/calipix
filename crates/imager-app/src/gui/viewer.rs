//! The contents of an image tab. This is a stand-in that renders the first
//! plane on the CPU; the GPU viewer replaces it behind the same interface:
//! [`Viewer::show`], [`Viewer::zoom`] and [`Viewer::retain`].

use std::collections::HashMap;

use eframe::egui;
use imager_command::{Dataset, DatasetId};
use imager_core::Image;

use super::ui;
use crate::state::Zoom;

/// What the viewer reports back for the status bar and tab handling.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewerResponse {
    /// The image area was clicked, which makes its image the active one.
    pub clicked: bool,
    /// Pixel under the pointer, as (x, y) from the top-left corner.
    pub cursor: Option<(usize, usize)>,
    /// Screen pixels per image pixel.
    pub scale: Option<f32>,
}

/// Space left around an image that is fitted to its tab.
const FIT_MARGIN: f32 = 8.0;

struct View {
    /// The image the texture was made from. Images are immutable
    /// snapshots, so a pointer comparison detects every change.
    image: Image,
    texture: Result<egui::TextureHandle, String>,
    /// Screen pixels per image pixel, or `None` to fit the tab.
    zoom: Option<f32>,
    /// The scale last drawn at, which zooming in and out start from.
    shown_scale: f32,
}

/// Views of the open images, one per dataset.
#[derive(Default)]
pub struct Viewer {
    views: HashMap<DatasetId, View>,
}

impl Viewer {
    pub fn new() -> Viewer {
        Viewer::default()
    }

    /// Draws `dataset` filling the available space of `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui, dataset: &Dataset) -> ViewerResponse {
        let view = self.views.entry(dataset.id).or_insert_with(|| View {
            image: dataset.image.clone(),
            texture: upload(ui.ctx(), dataset),
            zoom: None,
            shown_scale: 1.0,
        });
        if !view.image.ptr_eq(&dataset.image) {
            view.image = dataset.image.clone();
            view.texture = upload(ui.ctx(), dataset);
        }

        let (area, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
        let name = &dataset.image.meta().name;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Image, true, format!("View of {name}"))
        });
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, ui::IMAGE_BACKGROUND);

        let texture = match &view.texture {
            Ok(texture) => texture,
            Err(problem) => {
                painter.text(
                    area.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("⚠ {problem}"),
                    egui::TextStyle::Body.resolve(ui.style()),
                    ui::ON_IMAGE_BACKGROUND,
                );
                return ViewerResponse {
                    clicked: response.clicked(),
                    ..ViewerResponse::default()
                };
            }
        };
        let size = texture.size_vec2();
        // Fitted images keep a little clear of the edges of the tab.
        let room = (area.size() - egui::Vec2::splat(2.0 * FIT_MARGIN)).max(egui::Vec2::splat(1.0));
        let fit = (room.x / size.x).min(room.y / size.y);
        let scale = view.zoom.unwrap_or(fit).max(f32::MIN_POSITIVE);
        view.shown_scale = scale;
        let shown = egui::Rect::from_center_size(area.center(), size * scale);
        let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(texture.id(), shown, whole, egui::Color32::WHITE);

        let cursor = response
            .hover_pos()
            .filter(|pos| shown.contains(*pos))
            .map(|pos| {
                let at = (pos - shown.min) / scale;
                let clamp = |v: f32, len: f32| (v.floor().max(0.0) as usize).min(len as usize - 1);
                (clamp(at.x, size.x), clamp(at.y, size.y))
            });
        ViewerResponse {
            clicked: response.clicked(),
            cursor,
            scale: Some(scale),
        }
    }

    /// Changes the magnification of one image's view.
    pub fn zoom(&mut self, id: DatasetId, zoom: Zoom) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        view.zoom = match zoom {
            Zoom::Fit => None,
            Zoom::Actual => Some(1.0),
            Zoom::In => Some((view.shown_scale * 2.0).min(64.0)),
            Zoom::Out => Some((view.shown_scale / 2.0).max(1.0 / 64.0)),
        };
    }

    /// Frees the views of images that are no longer open.
    pub fn retain(&mut self, open: impl Fn(DatasetId) -> bool) {
        self.views.retain(|id, _| open(*id));
    }
}

/// Renders the first plane of the image on the CPU and uploads it.
fn upload(ctx: &egui::Context, dataset: &Dataset) -> Result<egui::TextureHandle, String> {
    let image = &dataset.image;
    let meta = image.meta();
    let limit = ctx.input(|i| i.max_texture_side);
    if meta.width() > limit || meta.height() > limit {
        return Err(format!(
            "{} is {} x {}, larger than this build can display ({limit} pixels a side)",
            meta.name,
            meta.width(),
            meta.height()
        ));
    }
    let position = vec![0; meta.ndim() - 2];
    let rendered = imager_render::render_plane(image, &position)
        .map_err(|e| format!("{} could not be displayed: {e}", meta.name))?;
    let pixels =
        egui::ColorImage::from_rgba_unmultiplied([rendered.width, rendered.height], &rendered.data);
    // Nearest-neighbour sampling: scientific images must show real pixels.
    Ok(ctx.load_texture(
        format!("image-{}", dataset.id),
        pixels,
        egui::TextureOptions::NEAREST,
    ))
}
