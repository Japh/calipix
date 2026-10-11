//! The image viewer: draws image planes on the GPU inside an egui panel,
//! with zoom, pan and a pixel probe. Its output must agree with the CPU
//! reference renderer in `calipix-render`.
//!
//! # Use
//!
//! The application creates one [`ViewerGpu`] from eframe's render state and
//! owns one [`ImageView`] per open image:
//!
//! ```ignore
//! // In the eframe app creator:
//! let gpu = cc.wgpu_render_state.as_ref().map(calipix_view::ViewerGpu::from_render_state);
//! // Each frame, inside the image's panel:
//! let shown = view.show(ui, &image, gpu.as_ref());
//! if let Some(probe) = &shown.probe { /* status bar: x, y, values */ }
//! ```
//!
//! [`ImageView::show`] draws the image with sliders for Z and time and
//! channel buttons beneath it; [`ImageView::ui`] draws the image alone.
//! Zoom and position are plain state on the view, changed through methods
//! such as [`ImageView::zoom_in`] and [`ImageView::fit`], which the
//! application binds to its commands and shortcuts.
//!
//! # How it draws
//!
//! A plane is never assumed to fit one texture. It is cut into tiles
//! ([`tiles`]); only tiles that intersect the visible rectangle are read,
//! with region reads, on worker threads when the image is not in memory
//! ([`loader`]). Zoomed out, tiles are read decimated. Tiles are uploaded
//! as integer or float textures and mapped to colours in a fragment shader
//! ([`gpu`], [`mapping`]); where that is not possible the tiles are
//! rendered by `calipix-render` instead and shown as ordinary textures.

pub mod controls;
pub mod geometry;
mod gpu;
pub mod loader;
pub mod mapping;
pub mod tiles;
mod view;

pub use gpu::{OFFSCREEN_FORMAT, ViewerGpu};
pub use view::{
    Calibrated, ChannelValue, ImageView, Probe, Renderer, ViewOptions, ViewResponse, ViewStats,
};
