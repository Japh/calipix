//! The viewer widget: per-image view state, tile management, drawing on
//! the GPU or through the CPU renderer, interaction and the pixel probe.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_wgpu::wgpu;
use imager_core::{AxisKind, Buffer, ColorModel, Error, Image, Result};
use imager_render::{Rgba8, default_display_range};

use crate::geometry::{
    MAX_LEVEL, TIE_BIAS, ViewState, ViewTransform, Viewport, ZOOM_LEVELS, level_for_zoom,
    zoom_step_in, zoom_step_out,
};
use crate::gpu::{DisplayTextures, GpuFrame, OFFSCREEN_FORMAT, TileDraw, ViewerGpu};
use crate::loader::{Loaded, Loader, Waker, Work, plane_range};
use crate::mapping::{
    ChannelDisplay, DisplayPlan, SampleKind, channel_requests, gpu_can_draw, render_tile_cpu,
};
use crate::tiles::{ByteLru, TileCoord, TileGrid, TileKey, read_tile};

/// Settings of a view that rarely change.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewOptions {
    /// Side of a tile in samples. Clamped to the device's largest texture.
    pub tile_size: usize,
    /// Budget for cached tiles (samples, their GPU copies and CPU-rendered
    /// tiles together), in bytes. Tiles needed by the current frame are
    /// kept even if they exceed it.
    pub cache_bytes: usize,
    /// Always draw with the CPU renderer, even if a GPU is available.
    pub force_cpu: bool,
    /// Colour around the image: a neutral mid-dark grey in both themes.
    pub background: [u8; 3],
    /// Threads reading tiles of images that are not in memory.
    pub loader_threads: usize,
}

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions {
            tile_size: 1024,
            cache_bytes: 512 << 20,
            force_cpu: false,
            background: [58, 58, 58],
            loader_threads: 3,
        }
    }
}

/// Which renderer drew the last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    /// The fragment shader, from sample textures.
    Gpu,
    /// `imager-render`, uploaded as ordinary textures.
    Cpu,
}

/// One coordinate in calibrated units.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibrated {
    pub value: f64,
    pub unit: Option<String>,
}

/// The sample of one channel under the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelValue {
    pub channel: usize,
    /// `None` if the sample is not loaded yet, or the image is complex.
    pub value: Option<f64>,
}

/// What is under the cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    /// Pixel coordinates of the sample reported.
    ///
    /// This is the pixel under the cursor, except for an image that is not
    /// in memory viewed below 50% before its full-resolution tile has been
    /// loaded: then it is the sample displayed there, whose coordinates are
    /// multiples of the decimation step.
    pub x: usize,
    pub y: usize,
    /// The same position in the X and Y axes' calibrated units.
    pub calibrated: [Calibrated; 2],
    /// One value for a grayscale image (the channel shown), one per channel
    /// for composite and RGB images.
    pub values: Vec<ChannelValue>,
}

/// The result of showing a view for one frame.
pub struct ViewResponse {
    /// Interaction with the image area.
    pub response: egui::Response,
    /// The pixel under the cursor, if the cursor is over the image.
    pub probe: Option<Probe>,
    /// Magnification: 1.0 is 100%.
    pub zoom: f64,
    /// True while tiles or display ranges for this frame are still loading.
    pub loading: bool,
    /// Which renderer was used.
    pub renderer: Renderer,
    /// The most recent failure to read or draw the image, worded for the
    /// status bar.
    pub error: Option<String>,
}

/// Counters for diagnostics and measurements.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewStats {
    /// Sample tiles currently cached.
    pub tiles: usize,
    /// Bytes of cached samples.
    pub sample_bytes: usize,
    /// Bytes of sample textures on the GPU.
    pub gpu_bytes: usize,
    /// Bytes of CPU-rendered RGBA tiles.
    pub rgba_bytes: usize,
    /// Tiles read since the view was created.
    pub tiles_read: u64,
    /// Time spent reading tiles on the calling thread (images in memory,
    /// and offscreen renders); reads on worker threads are not timed.
    pub read_time: Duration,
    /// Tiles uploaded to the GPU since the view was created.
    pub uploads: u64,
    /// Time spent uploading tiles to the GPU.
    pub upload_time: Duration,
    /// Tiles rendered by the CPU renderer since the view was created.
    pub cpu_renders: u64,
    /// Time spent in the CPU renderer.
    pub cpu_render_time: Duration,
    /// Tiles removed from the cache to stay within budget.
    pub evictions: u64,
}

struct TileData {
    samples: Buffer,
    width: usize,
    height: usize,
    texture: Option<wgpu::Texture>,
}

#[derive(Clone, Copy, PartialEq)]
enum Pending {
    /// First display: 100% if the image fits, else fit.
    Initial,
    Fit,
    ActualSize,
    /// A zoom was chosen before the first display: keep it, but centre.
    Center,
}

/// What one frame draws.
struct Planned {
    transform: ViewTransform,
    grid: TileGrid,
    /// `None` while a display range is still being computed.
    plan: Option<DisplayPlan>,
    use_gpu: bool,
    /// Pairs of the tile area to fill and the tile whose samples fill it:
    /// the same tile, or a coarser one while the right one is loading.
    cover: Vec<(TileCoord, TileCoord)>,
    loading: bool,
}

/// View state for one image: zoom, pan, position along the axes beyond X
/// and Y, channel visibility and the tile cache. The application owns one
/// per open image and calls [`ui`](Self::ui) or [`show`](Self::show) each
/// frame.
pub struct ImageView {
    options: ViewOptions,
    state: ViewState,
    pending: Option<Pending>,
    position: Vec<usize>,
    visible: Vec<bool>,
    last_axis: Option<usize>,

    image: Option<Image>,
    tiles: ByteLru<TileKey, TileData>,
    gpu_bytes: usize,
    auto_ranges: HashMap<usize, (f64, f64)>,
    failed: HashMap<Work, String>,
    loader: Option<Loader>,
    display: Option<DisplayTextures>,
    display_plan: Option<DisplayPlan>,
    cpu_tiles: ByteLru<TileCoord, Rgba8>,
    cpu_plan: Option<DisplayPlan>,
    cpu_generation: u64,
    composed: Option<(ComposeKey, Composed)>,
    last_viewport: Option<Viewport>,
    wheel: f32,
    error: Option<String>,
    stats: ViewStats,
}

impl Default for ImageView {
    fn default() -> Self {
        ImageView::new(ViewOptions::default())
    }
}

impl ImageView {
    pub fn new(options: ViewOptions) -> ImageView {
        ImageView {
            options,
            state: ViewState {
                zoom: 1.0,
                center: [0.0, 0.0],
            },
            pending: Some(Pending::Initial),
            position: Vec::new(),
            visible: Vec::new(),
            last_axis: None,
            image: None,
            tiles: ByteLru::default(),
            gpu_bytes: 0,
            auto_ranges: HashMap::new(),
            failed: HashMap::new(),
            loader: None,
            display: None,
            display_plan: None,
            cpu_tiles: ByteLru::default(),
            cpu_plan: None,
            cpu_generation: 0,
            composed: None,
            last_viewport: None,
            wheel: 0.0,
            error: None,
            stats: ViewStats::default(),
        }
    }

    pub fn options(&self) -> &ViewOptions {
        &self.options
    }

    /// Changes the options. Cached tiles are dropped if the tile size
    /// changed.
    pub fn set_options(&mut self, options: ViewOptions) {
        if options.tile_size != self.options.tile_size {
            self.drop_tiles();
        }
        if options.force_cpu != self.options.force_cpu {
            self.cpu_tiles.clear();
            self.cpu_generation += 1;
        }
        self.options = options;
    }

    // ----------------------------------------------------------- zoom and pan

    /// Magnification: 1.0 is 100%, one image pixel per device pixel.
    pub fn zoom(&self) -> f64 {
        self.state.zoom
    }

    /// Sets the magnification, keeping the middle of the view in place.
    pub fn set_zoom(&mut self, zoom: f64) {
        if zoom.is_finite() && zoom > 0.0 {
            self.state.zoom = zoom.min(crate::geometry::MAX_ZOOM);
            self.keep_position_on_first_show();
        }
    }

    /// The image coordinate shown at the middle of the view.
    pub fn center(&self) -> [f64; 2] {
        self.state.center
    }

    pub fn set_center(&mut self, center: [f64; 2]) {
        if center.iter().all(|c| c.is_finite()) {
            self.state.center = center;
            if matches!(self.pending, Some(Pending::Initial | Pending::Center)) {
                self.pending = None;
            }
        }
    }

    /// Steps to the next larger zoom level, about the middle of the view.
    pub fn zoom_in(&mut self) {
        self.state.zoom = zoom_step_in(self.state.zoom);
        self.keep_position_on_first_show();
    }

    /// Steps to the next smaller zoom level, about the middle of the view.
    /// Stops at the smallest level, or where the whole image fits if that
    /// is smaller still.
    pub fn zoom_out(&mut self) {
        self.state.zoom = zoom_step_out(self.state.zoom, self.min_zoom());
        self.keep_position_on_first_show();
    }

    /// Shows the whole image as large as the view allows.
    pub fn fit(&mut self) {
        self.pending = Some(Pending::Fit);
    }

    /// Shows the image at 100%.
    pub fn actual_size(&mut self) {
        if matches!(self.pending, Some(Pending::Initial | Pending::Center)) {
            self.pending = Some(Pending::ActualSize);
        } else {
            self.state.zoom = 1.0;
            self.pending = None;
        }
    }

    /// A zoom request made before the image was first shown replaces the
    /// automatic initial zoom but still starts centred.
    fn keep_position_on_first_show(&mut self) {
        if self.pending == Some(Pending::Initial) {
            self.pending = Some(Pending::Center);
        }
    }

    fn min_zoom(&self) -> f64 {
        let smallest = ZOOM_LEVELS[0];
        match (&self.image, &self.last_viewport) {
            (Some(image), Some(viewport)) => {
                let meta = image.meta();
                smallest.min(ViewState::fit_zoom([meta.width(), meta.height()], viewport))
            }
            _ => smallest,
        }
    }

    // --------------------------------------------------------------- position

    /// The index along each axis beyond X and Y, in axis order.
    pub fn position(&self) -> &[usize] {
        &self.position
    }

    /// Moves to `position` (one index per axis beyond X and Y). Indices
    /// are clamped to the image when it is next shown.
    pub fn set_position(&mut self, position: &[usize]) {
        self.position = position.to_vec();
        if let Some(image) = &self.image {
            let shape = image.meta().plane_shape().to_vec();
            self.fit_position(&shape);
        }
    }

    /// Moves `delta` steps along the axis in slot `axis` of
    /// [`position`](Self::position), stopping at the ends. Returns true if
    /// the position changed.
    pub fn step_axis(&mut self, axis: usize, delta: i64) -> bool {
        let Some(image) = &self.image else {
            return false;
        };
        let Some(&size) = image.meta().plane_shape().get(axis) else {
            return false;
        };
        let Some(current) = self.position.get(axis).copied() else {
            return false;
        };
        let next = (current as i64 + delta).clamp(0, size as i64 - 1) as usize;
        self.last_axis = Some(axis);
        self.position[axis] = next;
        next != current
    }

    /// Steps along the slider used last (or the first non-channel axis),
    /// for Left, Right and Shift+wheel. Returns true if the position
    /// changed.
    pub fn step_last_axis(&mut self, delta: i64) -> bool {
        let axis = self.last_axis.or_else(|| {
            let meta = self.image.as_ref()?.meta();
            (2..meta.ndim())
                .find(|&i| meta.axes()[i].kind != AxisKind::Channel)
                .map(|i| i - 2)
        });
        axis.is_some_and(|axis| self.step_axis(axis, delta))
    }

    /// Whether channel `channel` contributes to a composite image.
    pub fn channel_visible(&self, channel: usize) -> bool {
        self.visible.get(channel).copied().unwrap_or(true)
    }

    pub fn set_channel_visible(&mut self, channel: usize, visible: bool) {
        if self.visible.len() <= channel {
            self.visible.resize(channel + 1, true);
        }
        self.visible[channel] = visible;
    }

    fn fit_position(&mut self, plane_shape: &[usize]) {
        if self.position.len() != plane_shape.len() {
            self.position = vec![0; plane_shape.len()];
        }
        for (p, &size) in self.position.iter_mut().zip(plane_shape) {
            *p = (*p).min(size - 1);
        }
    }

    // ------------------------------------------------------------ image state

    /// Switches to `image`, which the caller promises holds the same
    /// samples as the image shown so far: only metadata (display range,
    /// lookup table, calibration, name) differs. Cached tiles are kept, so
    /// adjusting brightness of a large lazily read image re-reads nothing.
    ///
    /// Without this call a new `Image` handle is treated as new samples.
    /// If the shape or pixel type differs the call has no effect.
    pub fn metadata_changed(&mut self, image: &Image) {
        if let Some(old) = &self.image {
            let (a, b) = (old.meta(), image.meta());
            if a.shape() == b.shape() && a.dtype() == b.dtype() {
                self.image = Some(image.clone());
            }
        }
    }

    /// Counters for diagnostics.
    pub fn stats(&self) -> ViewStats {
        ViewStats {
            tiles: self.tiles.len(),
            sample_bytes: self.tiles.bytes() - self.gpu_bytes,
            gpu_bytes: self.gpu_bytes,
            rgba_bytes: self.cpu_tiles.bytes(),
            ..self.stats
        }
    }

    fn drop_tiles(&mut self) {
        self.tiles.clear();
        self.gpu_bytes = 0;
        self.cpu_tiles.clear();
        self.cpu_plan = None;
        self.cpu_generation += 1;
        self.auto_ranges.clear();
        self.failed.clear();
        self.loader = None;
    }

    /// Adopts `image` as the image shown, dropping whatever was cached for
    /// a different one.
    fn sync_image(&mut self, image: &Image) {
        if self.image.as_ref().is_some_and(|old| old.ptr_eq(image)) {
            return;
        }
        let meta = image.meta();
        let same_size = self.image.as_ref().is_some_and(|old| {
            old.meta().width() == meta.width() && old.meta().height() == meta.height()
        });
        self.drop_tiles();
        self.error = None;
        if self
            .display
            .as_ref()
            .is_some_and(|d| d.dtype != meta.dtype())
        {
            self.display = None;
        }
        self.display_plan = None;
        self.fit_position(meta.plane_shape());
        self.visible.resize(meta.size(&AxisKind::Channel), true);
        if self.last_axis.is_some_and(|a| a >= self.position.len()) {
            self.last_axis = None;
        }
        // A different-sized image starts afresh; the first image keeps
        // whatever zoom and centre were set before it was shown.
        if self.image.is_some() && !same_size {
            self.pending = Some(Pending::Initial);
        }
        self.image = Some(image.clone());
    }

    /// Applies deferred zoom requests and keeps the pan within bounds.
    fn settle(&mut self, image: &Image, viewport: &Viewport) {
        let meta = image.meta();
        let size = [meta.width(), meta.height()];
        if viewport.is_empty() {
            return;
        }
        match self.pending.take() {
            Some(Pending::Initial) => {
                let fits = size[0] as f64 <= viewport.size[0] && size[1] as f64 <= viewport.size[1];
                self.state = if fits {
                    ViewState::actual_size(size[0], size[1])
                } else {
                    ViewState::fit(size, viewport)
                };
            }
            Some(Pending::Fit) => self.state = ViewState::fit(size, viewport),
            Some(Pending::ActualSize) => self.state = ViewState::actual_size(size[0], size[1]),
            Some(Pending::Center) => {
                self.state.center = [size[0] as f64 / 2.0, size[1] as f64 / 2.0];
            }
            None => {}
        }
        self.state.clamp(size, viewport);
        self.last_viewport = Some(*viewport);
    }

    fn grid(&self, image: &Image, gpu: Option<&ViewerGpu>) -> TileGrid {
        let meta = image.meta();
        let mut tile = self.options.tile_size.max(16);
        if let Some(gpu) = gpu {
            tile = tile.min(gpu.max_texture_side() as usize);
        }
        TileGrid::new(meta.width(), meta.height(), tile)
    }

    fn drain_loader(&mut self, grid: &TileGrid) {
        let Some(loader) = &self.loader else {
            return;
        };
        while let Some(loaded) = loader.try_recv() {
            match loaded {
                Loaded::Tile(key, Ok(samples)) => {
                    self.stats.tiles_read += 1;
                    store_tile(&mut self.tiles, grid, key, samples);
                }
                Loaded::Tile(key, Err(e)) => {
                    self.error = Some(e.clone());
                    self.failed.insert(Work::Tile(key), e);
                }
                Loaded::Range(plane, Ok(range)) => {
                    self.auto_ranges.insert(plane, range);
                }
                Loaded::Range(plane, Err(e)) => {
                    self.error = Some(e.clone());
                    self.failed.insert(Work::Range(plane), e);
                }
            }
        }
    }

    /// The default display range of a plane, if it is known or can be
    /// found without waiting on a file.
    fn auto_range(&mut self, image: &Image, plane: usize, blocking: bool) -> Option<(f64, f64)> {
        if let Some(&range) = self.auto_ranges.get(&plane) {
            return Some(range);
        }
        if self.failed.contains_key(&Work::Range(plane)) {
            return None;
        }
        let range = if image.is_in_memory() {
            // Exactly the reference renderer's default.
            image
                .read_plane(plane)
                .and_then(|samples| default_display_range(&samples))
        } else if blocking {
            plane_range(image, plane)
        } else {
            return None;
        };
        match range {
            Ok(range) => {
                self.auto_ranges.insert(plane, range);
                Some(range)
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.failed.insert(Work::Range(plane), e.to_string());
                None
            }
        }
    }

    /// Works out what to draw for `viewport` and starts loading what is
    /// missing. With `blocking`, everything is read before returning.
    fn plan_frame(
        &mut self,
        image: &Image,
        gpu: Option<&ViewerGpu>,
        viewport: &Viewport,
        waker: Option<Waker>,
        blocking: bool,
        allow_software: bool,
    ) -> Planned {
        self.sync_image(image);
        self.settle(image, viewport);
        let meta = image.meta();
        let grid = self.grid(image, gpu);
        self.drain_loader(&grid);
        let transform = self.state.snapped_transform(viewport);
        let mut planned = Planned {
            transform,
            grid,
            plan: None,
            use_gpu: false,
            cover: Vec::new(),
            loading: false,
        };
        let requests = match channel_requests(meta, &self.position, &self.visible) {
            Ok(requests) => requests,
            Err(e) => {
                self.error = Some(e.to_string());
                return planned;
            }
        };
        let read_now = blocking || image.is_in_memory();
        let mut wanted = Vec::new();

        let mut channels = Vec::with_capacity(requests.len());
        for request in &requests {
            let range = match request.range {
                Some(range) => Some(range),
                None => self.auto_range(image, request.plane, blocking),
            };
            match range {
                Some(range) => channels.push(ChannelDisplay {
                    plane: request.plane,
                    channel: request.channel,
                    range,
                    lut: request.lut.clone(),
                }),
                None => {
                    let work = Work::Range(request.plane);
                    if !self.failed.contains_key(&work) {
                        wanted.push(work);
                        planned.loading = true;
                    }
                }
            }
        }
        if channels.len() == requests.len() {
            let plan = DisplayPlan {
                dtype: meta.dtype(),
                channels,
            };
            planned.use_gpu = !self.options.force_cpu
                && gpu.is_some_and(|g| allow_software || !g.is_software())
                && gpu_can_draw(&plan);
            planned.plan = Some(plan);
        }

        let level = level_for_zoom(transform.zoom);
        let coords = transform
            .visible_rect(viewport, meta.width(), meta.height())
            .map(|visible| grid.tiles_in(visible, level))
            .unwrap_or_default();
        let mark = self.tiles.mark();
        for coord in coords {
            let mut have_all = true;
            for request in &requests {
                let key = coord.on_plane(request.plane);
                if self.tiles.get(&key).is_some() {
                    continue;
                }
                let work = Work::Tile(key);
                if self.failed.contains_key(&work) {
                    have_all = false;
                } else if read_now {
                    let start = Instant::now();
                    let read = read_tile(image, &grid, key);
                    self.stats.read_time += start.elapsed();
                    match read {
                        Ok(samples) => {
                            self.stats.tiles_read += 1;
                            store_tile(&mut self.tiles, &grid, key, samples);
                        }
                        Err(e) => {
                            self.error = Some(e.to_string());
                            self.failed.insert(work, e.to_string());
                            have_all = false;
                        }
                    }
                } else {
                    wanted.push(work);
                    planned.loading = true;
                    have_all = false;
                }
            }
            if have_all {
                planned.cover.push((coord, coord));
                continue;
            }
            // Until the tile arrives, show a coarser one that covers it.
            for up in 1..=MAX_LEVEL.saturating_sub(coord.level) {
                let coarser = coord.ancestor(up);
                let complete = requests
                    .iter()
                    .all(|r| self.tiles.contains(&coarser.on_plane(r.plane)));
                if complete {
                    for r in &requests {
                        self.tiles.get(&coarser.on_plane(r.plane));
                    }
                    planned.cover.push((coord, coarser));
                    break;
                }
            }
        }
        // Stand-ins are drawn first so that exact tiles win where the
        // rounded-out screen rectangles of neighbours overlap.
        planned.cover.sort_by_key(|(area, source)| area == source);

        if wanted.is_empty() {
            if let Some(loader) = &self.loader {
                loader.set_wanted(Vec::new());
            }
        } else {
            let threads = self.options.loader_threads;
            let loader = self
                .loader
                .get_or_insert_with(|| Loader::new(image.clone(), grid, threads));
            loader.set_waker(waker);
            loader.set_wanted(wanted);
        }
        self.evict(mark);
        planned
    }

    fn evict(&mut self, mark: u64) {
        let budget = self
            .options
            .cache_bytes
            .saturating_sub(self.cpu_tiles.bytes());
        if self.tiles.bytes() > budget {
            self.stats.evictions += self.tiles.evict(budget, mark) as u64;
            // Recount GPU bytes from what survived.
            self.gpu_bytes = self
                .tiles
                .values()
                .filter(|tile| tile.texture.is_some())
                .map(|tile| tile.samples.byte_len())
                .sum();
        }
    }

    /// The texture of a cached tile, uploading it on first use.
    fn texture(&mut self, gpu: &ViewerGpu, key: TileKey) -> Option<wgpu::Texture> {
        let start = Instant::now();
        let (texture, bytes) = {
            let tile = self.tiles.get(&key)?;
            if let Some(texture) = &tile.texture {
                return Some(texture.clone());
            }
            let texture = gpu.upload_tile(&tile.samples, tile.width, tile.height)?;
            tile.texture = Some(texture.clone());
            (texture, tile.samples.byte_len())
        };
        self.tiles.resize(&key, bytes * 2);
        self.gpu_bytes += bytes;
        self.stats.uploads += 1;
        self.stats.upload_time += start.elapsed();
        Some(texture)
    }

    /// Builds the GPU frame for a plan: uploads what is new and lays out
    /// one draw per covered tile.
    fn gpu_frame(
        &mut self,
        gpu: &ViewerGpu,
        planned: &Planned,
        viewport: &Viewport,
        format: wgpu::TextureFormat,
    ) -> Option<GpuFrame> {
        let plan = planned.plan.as_ref()?;
        SampleKind::of(plan.dtype)?;
        if self.display.as_ref().is_none_or(|d| d.dtype != plan.dtype) {
            self.display = Some(gpu.create_display(plan.dtype));
            self.display_plan = None;
        }
        if self.display_plan.as_ref() != Some(plan) {
            gpu.write_display(self.display.as_ref()?, plan);
            self.display_plan = Some(plan.clone());
        }
        let bounds = pixel_bounds(viewport);
        let t = planned.transform;
        let mut draws = Vec::with_capacity(planned.cover.len());
        for &(area, source) in &planned.cover {
            let (Some(area_rect), Some(source_rect)) =
                (planned.grid.tile_rect(area), planned.grid.tile_rect(source))
            else {
                continue;
            };
            let textures: Option<Vec<wgpu::Texture>> = plan
                .channels
                .iter()
                .map(|c| self.texture(gpu, source.on_plane(c.plane)))
                .collect();
            let Some(textures) = textures else {
                continue;
            };
            // The screen pixels the area's samples can reach.
            let area_step = TileGrid::step(area.level) as f64;
            let x0 = (t.origin[0] + area_rect.x as f64 * area_step * t.zoom).floor();
            let y0 = (t.origin[1] + area_rect.y as f64 * area_step * t.zoom).floor();
            let x1 = (t.origin[0] + (area_rect.x + area_rect.w) as f64 * area_step * t.zoom).ceil();
            let y1 = (t.origin[1] + (area_rect.y + area_rect.h) as f64 * area_step * t.zoom).ceil();
            let scissor = [
                (x0 as i32).max(bounds[0]),
                (y0 as i32).max(bounds[1]),
                (x1.min(bounds[2] as f64)) as i32,
                (y1.min(bounds[3] as f64)) as i32,
            ];
            if scissor[2] <= scissor[0] || scissor[3] <= scissor[1] {
                continue;
            }
            // Texel coordinates are measured from the first pixel drawn, so
            // single precision in the shader only ever spans one tile's
            // part of the screen.
            let base = [scissor[0], scissor[1]];
            let scale = t.zoom * TileGrid::step(source.level) as f64;
            let texel0 = [
                (base[0] as f64 + 0.5 - t.origin[0]) / scale - source_rect.x as f64 + TIE_BIAS,
                (base[1] as f64 + 0.5 - t.origin[1]) / scale - source_rect.y as f64 + TIE_BIAS,
            ];
            draws.push(TileDraw {
                textures,
                scissor,
                base,
                texel0,
                size: [source_rect.w as u32, source_rect.h as u32],
                texels_per_px: 1.0 / scale,
            });
        }
        GpuFrame::new(gpu, self.display.as_ref()?, plan, format, bounds, draws)
    }

    // ------------------------------------------------------ offscreen renders

    /// Draws what the widget would draw with this GPU into a `width` x
    /// `height` picture, reading every tile needed before returning: by the
    /// shader where it can draw the image (see
    /// [`crate::mapping::gpu_can_draw`]; a software adapter is not avoided
    /// here), otherwise CPU-rendered tiles copied through the GPU.
    /// [`ViewStats::cpu_renders`] tells which happened.
    pub fn render_gpu(
        &mut self,
        image: &Image,
        gpu: &ViewerGpu,
        width: u32,
        height: u32,
    ) -> Result<Rgba8> {
        let viewport = Viewport::new(0.0, 0.0, width as f64, height as f64);
        let planned = self.plan_frame(image, Some(gpu), &viewport, None, true, true);
        if let Some(e) = self.error.clone() {
            return Err(Error::Unsupported(e));
        }
        let not_uploaded = || Error::Unsupported("the image could not be uploaded".to_string());
        let frame = if planned.use_gpu {
            self.gpu_frame(gpu, &planned, &viewport, OFFSCREEN_FORMAT)
                .ok_or_else(not_uploaded)?
        } else {
            self.render_cpu_tiles(&planned, None);
            if let Some(e) = self.error.clone() {
                return Err(Error::Unsupported(e));
            }
            let picture = self.compose_cpu(&planned, &viewport);
            let texture = gpu
                .upload_picture(&picture, None)
                .ok_or_else(not_uploaded)?;
            GpuFrame::picture(gpu, &texture, OFFSCREEN_FORMAT, pixel_bounds(&viewport))
        };
        let data = frame.render_offscreen(width, height, self.options.background);
        if data.len() != (width * height * 4) as usize {
            return Err(Error::Unsupported(
                "the rendered picture could not be read back from the GPU".to_string(),
            ));
        }
        Ok(Rgba8 {
            width: width as usize,
            height: height as usize,
            data,
        })
    }

    /// Draws what the view shows into a `width` x `height` picture with
    /// the CPU renderer alone: whole planes through
    /// `imager_render::render_plane`, then one sample per device pixel as
    /// [`ViewTransform::displayed_pixel`] defines. Independent of tiles and
    /// of the GPU; it reads whole planes, so it suits tests, thumbnails and
    /// export rather than huge images.
    pub fn render_cpu(&mut self, image: &Image, width: u32, height: u32) -> Result<Rgba8> {
        let viewport = Viewport::new(0.0, 0.0, width as f64, height as f64);
        self.sync_image(image);
        self.settle(image, &viewport);
        let meta = image.meta();
        let (w, h) = (meta.width(), meta.height());
        let transform = self.state.snapped_transform(&viewport);
        let step = TileGrid::step(level_for_zoom(transform.zoom));

        let mut channels = Vec::new();
        let mut planes = Vec::new();
        for request in channel_requests(meta, &self.position, &self.visible)? {
            let samples = image.read_plane(request.plane)?;
            let range = match request.range {
                Some(range) => range,
                None => default_display_range(&samples)?,
            };
            channels.push(ChannelDisplay {
                plane: request.plane,
                channel: request.channel,
                range,
                lut: request.lut,
            });
            planes.push(samples);
        }
        let plan = DisplayPlan {
            dtype: meta.dtype(),
            channels,
        };
        let full = render_tile_cpu(&plan, &planes, w, h)?;
        let [r, g, b] = self.options.background;
        let mut data = [r, g, b, 255].repeat((width * height) as usize);
        for (py, row) in data.chunks_exact_mut(width as usize * 4).enumerate() {
            for (px, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                if let Some((x, y)) = transform.displayed_pixel(px as f64, py as f64, step, w, h) {
                    let at = (y * w + x) * 4;
                    out.copy_from_slice(&full.data[at..at + 4]);
                }
            }
        }
        Ok(Rgba8 {
            width: width as usize,
            height: height as usize,
            data,
        })
    }

    // ------------------------------------------------------------------ probe

    /// The sample of `plane` at pixel `(x, y)`, from memory or from cached
    /// tiles; never reads a file. Returns the coordinates of the sample
    /// found, which differ from `(x, y)` only if a decimated tile had to
    /// stand in.
    fn sample(
        &self,
        image: &Image,
        grid: &TileGrid,
        plane: usize,
        x: usize,
        y: usize,
        level: u8,
    ) -> Option<(usize, usize, f64)> {
        if image.is_in_memory() {
            let samples = image.read_plane(plane).ok()?;
            return Some((x, y, samples.get_f64(y * grid.width + x)?));
        }
        let mut levels = vec![0];
        if level > 0 {
            levels.push(level);
        }
        for level in levels {
            let step = TileGrid::step(level);
            let (lx, ly) = (x / step, y / step);
            let key = TileKey {
                plane,
                level,
                tx: (lx / grid.tile) as u32,
                ty: (ly / grid.tile) as u32,
            };
            if let Some(tile) = self.tiles.peek(&key) {
                let i = (ly % grid.tile) * tile.width + lx % grid.tile;
                return Some((lx * step, ly * step, tile.samples.get_f64(i)?));
            }
        }
        None
    }

    /// What is at the device-pixel position `screen` (same coordinates as
    /// the viewport), using only data already at hand.
    fn probe_at(&self, image: &Image, planned: &Planned, screen: [f64; 2]) -> Option<Probe> {
        let meta = image.meta();
        let (mut x, mut y) = planned
            .transform
            .pixel_at(screen, meta.width(), meta.height())?;
        let level = level_for_zoom(planned.transform.zoom);
        let channel_axis = meta.axis_index(&AxisKind::Channel);
        let channels: Vec<usize> = match (meta.color, channel_axis) {
            (ColorModel::Composite | ColorModel::Rgb, Some(_)) => {
                (0..meta.size(&AxisKind::Channel)).collect()
            }
            (_, Some(axis)) => vec![self.position.get(axis - 2).copied().unwrap_or(0)],
            (_, None) => vec![0],
        };
        let mut values = Vec::with_capacity(channels.len());
        let (cursor_x, cursor_y) = (x, y);
        for channel in channels {
            let mut position = self.position.clone();
            if let Some(axis) = channel_axis
                && let Some(p) = position.get_mut(axis - 2)
            {
                *p = channel;
            }
            let value = meta.plane_index(&position).ok().and_then(|plane| {
                self.sample(image, &planned.grid, plane, cursor_x, cursor_y, level)
            });
            if let Some((sx, sy, _)) = value {
                (x, y) = (sx, sy);
            }
            values.push(ChannelValue {
                channel,
                value: value.map(|(_, _, v)| v),
            });
        }
        let calibrated = [0, 1].map(|axis| {
            let a = &meta.axes()[axis];
            Calibrated {
                value: a.origin + [x, y][axis] as f64 * a.scale,
                unit: a.unit.clone(),
            }
        });
        Some(Probe {
            x,
            y,
            calibrated,
            values,
        })
    }

    // --------------------------------------------------------------- the widget

    /// Shows the image in all the space available and handles zoom, pan
    /// and the probe.
    ///
    /// - Wheel: zoom one step about the cursor. Ctrl+wheel and pinch zoom
    ///   smoothly. Shift+wheel steps the last-used position slider.
    /// - Middle-drag, or Space with a primary drag: pan.
    ///
    /// Pass the application's [`ViewerGpu`] if it has one; without it, or
    /// on a software adapter, or for images the shader cannot draw, the
    /// CPU renderer is used. Keyboard shortcuts are the application's to
    /// bind, to [`zoom_in`](Self::zoom_in), [`fit`](Self::fit) and so on.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        image: &Image,
        gpu: Option<&ViewerGpu>,
    ) -> ViewResponse {
        let size = ui.available_size().max(egui::vec2(1.0, 1.0));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let viewport = Viewport::from_points(rect, ppp);
        self.sync_image(image);
        self.settle(image, &viewport);
        self.interact(ui, &response, &viewport, ppp);

        let ctx = ui.ctx().clone();
        let waker: Waker = Arc::new(move || ctx.request_repaint());
        let planned = self.plan_frame(image, gpu, &viewport, Some(waker), false, false);
        let mut loading = planned.loading;

        let [r, g, b] = self.options.background;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(r, g, b));
        let mut renderer = Renderer::Cpu;
        if planned.use_gpu
            && let Some(gpu) = gpu
            && let Some(frame) = self.gpu_frame(gpu, &planned, &viewport, gpu.target_format())
        {
            renderer = Renderer::Gpu;
            painter.add(egui_wgpu::Callback::new_paint_callback(rect, frame));
        } else if planned.plan.is_some() {
            loading |= self.paint_cpu(ui.ctx(), &painter, rect, gpu, &planned, &viewport, ppp);
        }
        if renderer == Renderer::Gpu {
            // CPU-rendered tiles are of no use while the shader draws.
            self.cpu_tiles.clear();
            self.cpu_plan = None;
            self.composed = None;
        }

        let probe = response
            .hover_pos()
            .filter(|_| !viewport.is_empty())
            .and_then(|pos| {
                let screen = [pos.x as f64 * ppp as f64, pos.y as f64 * ppp as f64];
                self.probe_at(image, &planned, screen)
            });
        ViewResponse {
            response,
            probe,
            zoom: self.state.zoom,
            loading,
            renderer,
            error: self.error.clone(),
        }
    }

    /// Shows the image with its position controls underneath, when it has
    /// axes beyond X and Y to control.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        image: &Image,
        gpu: Option<&ViewerGpu>,
    ) -> ViewResponse {
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            if crate::controls::has_controls(image.meta()) {
                self.position_controls(ui, image);
            }
            self.ui(ui, image, gpu)
        })
        .inner
    }

    /// Sliders for Z, time and any other axis, and toggle buttons for
    /// channels; see [`crate::controls`]. Returns true if the position or
    /// channel visibility changed.
    pub fn position_controls(&mut self, ui: &mut egui::Ui, image: &Image) -> bool {
        self.sync_image(image);
        let changed = crate::controls::position_controls(
            ui,
            image.meta(),
            &mut self.position,
            &mut self.visible,
            &mut self.last_axis,
        );
        if changed {
            ui.ctx().request_repaint();
        }
        changed
    }

    fn interact(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        viewport: &Viewport,
        ppp: f32,
    ) {
        if viewport.is_empty() {
            return;
        }
        let Some(image) = self.image.clone() else {
            return;
        };
        let meta = image.meta();
        let size = [meta.width(), meta.height()];
        let to_px = |p: egui::Pos2| [p.x as f64 * ppp as f64, p.y as f64 * ppp as f64];

        let space = ui.input(|i| i.key_down(egui::Key::Space));
        let panning = response.dragged_by(egui::PointerButton::Middle)
            || (space && response.dragged_by(egui::PointerButton::Primary));
        if panning {
            let d = response.drag_delta();
            self.state
                .pan_by([d.x as f64 * ppp as f64, d.y as f64 * ppp as f64]);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if space && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        if let Some(pos) = response.hover_pos() {
            let anchor = to_px(pos);
            let (mut notches, mut steps) = (0.0f32, 0.0f32);
            let pinch = ui.input(|i| {
                for event in &i.events {
                    if let egui::Event::MouseWheel {
                        unit,
                        delta,
                        modifiers,
                        ..
                    } = event
                    {
                        // With Shift some platforms report the wheel as
                        // horizontal movement.
                        let amount = if delta.y != 0.0 { delta.y } else { delta.x };
                        let amount = match unit {
                            egui::MouseWheelUnit::Line => amount,
                            egui::MouseWheelUnit::Point => amount / 40.0,
                            egui::MouseWheelUnit::Page => amount * 3.0,
                        };
                        if modifiers.shift {
                            steps += amount;
                        } else if !modifiers.command && !modifiers.ctrl {
                            notches += amount;
                        }
                    }
                }
                i.zoom_delta()
            });
            if pinch != 1.0 && pinch.is_finite() && pinch > 0.0 {
                let zoom = (self.state.zoom * pinch as f64)
                    .clamp(self.min_zoom(), crate::geometry::MAX_ZOOM);
                self.state.zoom_about(zoom, anchor, viewport);
            }
            self.wheel += notches + steps;
            let whole = self.wheel.trunc();
            self.wheel -= whole;
            if steps != 0.0 {
                // Wheel down moves forward through the stack.
                self.step_last_axis(-(whole as i64));
            } else {
                for _ in 0..whole.abs() as usize {
                    let zoom = if whole > 0.0 {
                        zoom_step_in(self.state.zoom)
                    } else {
                        zoom_step_out(self.state.zoom, self.min_zoom())
                    };
                    self.state.zoom_about(zoom, anchor, viewport);
                }
            }
        }
        self.state.clamp(size, viewport);
    }

    /// Renders the tiles of a plan that have not been rendered yet with the
    /// CPU renderer. With a `budget`, stops after the first tile once that
    /// much time has passed. Returns true if some tiles remain.
    fn render_cpu_tiles(&mut self, planned: &Planned, budget: Option<Duration>) -> bool {
        let Some(plan) = &planned.plan else {
            return false;
        };
        if self.cpu_plan.as_ref() != Some(plan) {
            self.cpu_tiles.clear();
            self.cpu_plan = Some(plan.clone());
            self.cpu_generation += 1;
        }
        let started = Instant::now();
        let mut rendered = 0;
        let mark = self.cpu_tiles.mark();
        let mut unfinished = false;
        for &(_, source) in &planned.cover {
            if self.cpu_tiles.get(&source).is_some() {
                continue;
            }
            let Some(source_rect) = planned.grid.tile_rect(source) else {
                continue;
            };
            // Keep the interface responsive: after the first tile of a
            // frame, stop once the frame has used its share of time.
            if rendered > 0 && budget.is_some_and(|b| started.elapsed() > b) {
                unfinished = true;
                continue;
            }
            let samples: Option<Vec<Buffer>> = plan
                .channels
                .iter()
                .map(|c| {
                    self.tiles
                        .peek(&source.on_plane(c.plane))
                        .map(|tile| tile.samples.clone())
                })
                .collect();
            let Some(samples) = samples else {
                continue;
            };
            let render_start = Instant::now();
            match render_tile_cpu(plan, &samples, source_rect.w, source_rect.h) {
                Ok(rgba) => {
                    let bytes = rgba.data.len();
                    self.cpu_tiles.insert(source, rgba, bytes);
                    self.cpu_generation += 1;
                    rendered += 1;
                    self.stats.cpu_renders += 1;
                    self.stats.cpu_render_time += render_start.elapsed();
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        if self.cpu_tiles.evict(self.options.cache_bytes / 2, mark) > 0 {
            self.cpu_generation += 1;
        }
        unfinished
    }

    /// Assembles the picture of the viewport from CPU-rendered tiles: for
    /// every device pixel the sample the shader would show there (the same
    /// arithmetic, in double precision), or the background.
    fn compose_cpu(&self, planned: &Planned, viewport: &Viewport) -> Rgba8 {
        let (width, height) = (viewport.size[0] as usize, viewport.size[1] as usize);
        let [r, g, b] = self.options.background;
        let mut data = [r, g, b, 255].repeat(width * height);
        let t = planned.transform;
        // Positions relative to the viewport's corner.
        let origin = [t.origin[0] - viewport.min[0], t.origin[1] - viewport.min[1]];
        for &(area, source) in &planned.cover {
            let (Some(area_rect), Some(source_rect)) =
                (planned.grid.tile_rect(area), planned.grid.tile_rect(source))
            else {
                continue;
            };
            let Some(tile) = self.cpu_tiles.peek(&source) else {
                continue;
            };
            let area_step = TileGrid::step(area.level) as f64;
            let scale = t.zoom * TileGrid::step(source.level) as f64;
            // For each axis: the device pixels the area can reach, and the
            // texel of the source tile each of them shows, if any.
            let texels = |axis: usize, start: usize, len: usize, limit: usize, size: usize| {
                let lo = (origin[axis] + start as f64 * area_step * t.zoom)
                    .floor()
                    .max(0.0);
                let hi = (origin[axis] + (start + len) as f64 * area_step * t.zoom)
                    .ceil()
                    .min(limit as f64);
                let source_start = [source_rect.x, source_rect.y][axis] as f64;
                (lo as usize..hi.max(lo) as usize)
                    .filter_map(|p| {
                        let u = ((p as f64 + 0.5 - origin[axis]) / scale + TIE_BIAS).floor()
                            - source_start;
                        (u >= 0.0 && u < size as f64).then_some((p, u as usize))
                    })
                    .collect::<Vec<_>>()
            };
            let columns = texels(0, area_rect.x, area_rect.w, width, source_rect.w);
            let rows = texels(1, area_rect.y, area_rect.h, height, source_rect.h);
            for &(py, v) in &rows {
                let from = &tile.data[v * tile.width * 4..(v + 1) * tile.width * 4];
                let to = &mut data[py * width * 4..(py + 1) * width * 4];
                for &(px, u) in &columns {
                    to[px * 4..px * 4 + 4].copy_from_slice(&from[u * 4..u * 4 + 4]);
                }
            }
        }
        Rgba8 {
            width,
            height,
            data,
        }
    }

    /// Draws what the view shows into a `width` x `height` picture the way
    /// the widget does when it cannot use the shader: tiles rendered by
    /// `imager-render`, assembled on the CPU. Reads every tile needed
    /// before returning.
    pub fn render_cpu_tiled(&mut self, image: &Image, width: u32, height: u32) -> Result<Rgba8> {
        let viewport = Viewport::new(0.0, 0.0, width as f64, height as f64);
        let planned = self.plan_frame(image, None, &viewport, None, true, false);
        if let Some(e) = self.error.clone() {
            return Err(Error::Unsupported(e));
        }
        self.render_cpu_tiles(&planned, None);
        if let Some(e) = self.error.clone() {
            return Err(Error::Unsupported(e));
        }
        Ok(self.compose_cpu(&planned, &viewport))
    }

    /// Draws the plan through the CPU renderer: one viewport-sized picture,
    /// reassembled when the view or the tiles change. With a [`ViewerGpu`]
    /// the picture is copied to the screen texel for pixel; without one it
    /// is an ordinary egui texture. Returns true if some tiles are still to
    /// be rendered.
    #[allow(clippy::too_many_arguments)]
    fn paint_cpu(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: egui::Rect,
        gpu: Option<&ViewerGpu>,
        planned: &Planned,
        viewport: &Viewport,
        ppp: f32,
    ) -> bool {
        let unfinished = self.render_cpu_tiles(planned, Some(CPU_FRAME_BUDGET));
        let key = ComposeKey {
            transform: planned.transform,
            viewport: *viewport,
            cover: planned.cover.clone(),
            generation: self.cpu_generation,
        };
        if self.composed.as_ref().is_none_or(|(k, _)| *k != key) {
            let start = Instant::now();
            let picture = self.compose_cpu(planned, viewport);
            let previous = self.composed.take().map(|(_, composed)| composed);
            let on_gpu = gpu.and_then(|gpu| {
                let existing = match &previous {
                    Some(Composed::Gpu(texture)) => Some(texture.clone()),
                    _ => None,
                };
                gpu.upload_picture(&picture, existing)
            });
            let composed = match (on_gpu, previous) {
                (Some(texture), _) => Composed::Gpu(texture),
                (None, previous) => {
                    let picture = egui::ColorImage::from_rgba_unmultiplied(
                        [picture.width, picture.height],
                        &picture.data,
                    );
                    let options = egui::TextureOptions::NEAREST;
                    match previous {
                        Some(Composed::Egui(mut handle)) => {
                            handle.set(picture, options);
                            Composed::Egui(handle)
                        }
                        _ => Composed::Egui(ctx.load_texture("imager-view", picture, options)),
                    }
                }
            };
            self.composed = Some((key, composed));
            self.stats.cpu_render_time += start.elapsed();
        }
        match (&self.composed, gpu) {
            (Some((_, Composed::Gpu(texture))), Some(gpu)) => {
                let frame =
                    GpuFrame::picture(gpu, texture, gpu.target_format(), pixel_bounds(viewport));
                painter.add(egui_wgpu::Callback::new_paint_callback(rect, frame));
            }
            (Some((_, Composed::Egui(handle))), _) => {
                // Exactly the viewport's device pixels, one texel each.
                let at = |v: f64| (v / ppp as f64) as f32;
                let rect = egui::Rect::from_min_max(
                    egui::pos2(at(viewport.min[0]), at(viewport.min[1])),
                    egui::pos2(
                        at(viewport.min[0] + viewport.size[0]),
                        at(viewport.min[1] + viewport.size[1]),
                    ),
                );
                painter.image(
                    handle.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            _ => {}
        }
        if unfinished {
            ctx.request_repaint();
        }
        unfinished
    }
}

/// The composed viewport picture, as it is handed to the renderer.
enum Composed {
    /// Drawn by the viewer's own pipeline, texel for device pixel.
    Gpu(wgpu::Texture),
    /// An ordinary egui texture, when there is no [`ViewerGpu`].
    Egui(egui::TextureHandle),
}

/// What a composed viewport picture was made from; it is remade when any
/// of this changes.
#[derive(Clone, PartialEq)]
struct ComposeKey {
    transform: ViewTransform,
    viewport: Viewport,
    cover: Vec<(TileCoord, TileCoord)>,
    /// Counts changes to the set of CPU-rendered tiles.
    generation: u64,
}

/// A viewport as `[x0, y0, x1, y1]` in device pixels, end exclusive.
fn pixel_bounds(viewport: &Viewport) -> [i32; 4] {
    [
        viewport.min[0] as i32,
        viewport.min[1] as i32,
        (viewport.min[0] + viewport.size[0]) as i32,
        (viewport.min[1] + viewport.size[1]) as i32,
    ]
}

/// Time the CPU renderer may take per frame once something is on screen.
const CPU_FRAME_BUDGET: Duration = Duration::from_millis(12);

fn store_tile(
    tiles: &mut ByteLru<TileKey, TileData>,
    grid: &TileGrid,
    key: TileKey,
    samples: Buffer,
) {
    let coord = TileCoord {
        level: key.level,
        tx: key.tx,
        ty: key.ty,
    };
    // A tile that does not match the grid (the tile size changed while it
    // was being read) is of no use.
    let Some(rect) = grid.tile_rect(coord) else {
        return;
    };
    if samples.len() != rect.w * rect.h {
        return;
    }
    let bytes = samples.byte_len();
    let data = TileData {
        width: rect.w,
        height: rect.h,
        samples,
        texture: None,
    };
    tiles.insert(key, data, bytes);
}
