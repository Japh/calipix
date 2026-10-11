//! A minimal application around the viewer widget.
//!
//! ```text
//! cargo run -p imager-view --example viewer -- <image file>
//! cargo run -p imager-view --example viewer -- --hyperstack [--gray]
//! cargo run -p imager-view --example viewer -- --huge
//! ```
//!
//! `--hyperstack` shows a synthetic 16-bit image with 3 channels, 8 slices
//! and 5 frames; `--huge` a 20000 x 20000 16-bit image whose samples are
//! computed when a region is read, to exercise tiling.
//!
//! Keys: `+` `-` zoom, `0` fit, `1` 100%, Left/Right step the last-used
//! slider. The wheel zooms about the cursor; middle-drag or Space+drag pans.
//!
//! Environment variables for running unattended:
//!
//! - `IMAGER_VIEW_SCRIPT`: comma-separated actions applied one after
//!   another once the window is up: `zoom_in`, `zoom_out`, `fit`, `actual`,
//!   `zoom:<factor>`, `center:<x>:<y>`, `pan:<dx>:<dy>` (image pixels),
//!   `pos:<axis slot>:<index>`, `hide:<channel>`, `show:<channel>`.
//! - `IMAGER_VIEW_SHOT`: a PNG path. After the script, when nothing is
//!   loading any more, the window saves a screenshot of itself there,
//!   compares the image area with the CPU reference renderer, prints the
//!   result and statistics to standard error, and exits.
//! - `IMAGER_VIEW_CPU=1`: draw with the CPU renderer even if a GPU exists.
//! - `IMAGER_VIEW_SOFTWARE=1`: run egui itself on a software adapter
//!   (the viewer then uses the CPU renderer of its own accord).
//! - `IMAGER_VIEW_DELAY_MS`: delay added to every region read of `--huge`.
//!
//! wgpu's own `WGPU_BACKEND` (vulkan, dx12, gl) selects the backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_wgpu::wgpu;
use imager_core::{
    Axis, AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut, PixelSource,
    Rect, Result,
};
use imager_render::Rgba8;
use imager_view::geometry::{ViewState, Viewport, level_for_zoom};
use imager_view::mapping::{ChannelDisplay, DisplayPlan, render_tile_cpu};
use imager_view::tiles::TileGrid;
use imager_view::{ImageView, Probe, Renderer, ViewOptions, ViewerGpu};

// ------------------------------------------------------------ test images

/// 3 channels x 8 slices x 5 frames of 512 x 384, 16-bit, calibrated.
fn hyperstack(gray: bool) -> Image {
    let (w, h, channels, slices, frames) = (512usize, 384usize, 3usize, 8usize, 5usize);
    let um = |kind, scale| Axis::calibrated(kind, scale, "um");
    let mut meta = ImageMeta::new(
        DType::U16,
        vec![
            (um(AxisKind::X, 0.325), w),
            (um(AxisKind::Y, 0.325), h),
            (Axis::new(AxisKind::Channel), channels),
            (um(AxisKind::Z, 1.5), slices),
            (Axis::calibrated(AxisKind::Time, 30.0, "s"), frames),
        ],
    )
    .expect("valid axes")
    .with_name("synthetic hyperstack");
    if !gray {
        meta.color = ColorModel::Composite;
    }
    let ramps: [fn(u8) -> [u8; 3]; 3] = [|i| [i, 0, 0], |i| [0, i, 0], |i| [0, 0, i]];
    meta.channels = ["red blobs", "green rings", "blue ramp"]
        .iter()
        .zip(ramps)
        .map(|(name, ramp)| ChannelMeta {
            name: Some(name.to_string()),
            lut: Some(Lut::from_fn(*name, ramp)),
            display_range: Some((0.0, 4095.0)),
        })
        .collect();
    let mut planes = Vec::new();
    for t in 0..frames {
        for z in 0..slices {
            for c in 0..channels {
                let mut data = Vec::with_capacity(w * h);
                for y in 0..h {
                    for x in 0..w {
                        let (fx, fy) = (x as f32, y as f32);
                        let v = match c {
                            // Blobs that move with time and focus with Z.
                            0 => {
                                let cx = 120.0 + 60.0 * t as f32;
                                let cy = 100.0 + 25.0 * z as f32;
                                let r = 30.0 + 6.0 * z as f32;
                                let d2 = (fx - cx).powi(2) + (fy - cy).powi(2);
                                4000.0 * (-d2 / (2.0 * r * r)).exp()
                            }
                            // Rings whose spacing changes with Z.
                            1 => {
                                let d = ((fx - 256.0).powi(2) + (fy - 192.0).powi(2)).sqrt();
                                let phase = d / (6.0 + z as f32) + t as f32;
                                2000.0 + 1900.0 * phase.sin()
                            }
                            // A ramp, with a bright bar that steps with time.
                            _ => {
                                let bar = (x / 64) == t && y > 300;
                                if bar {
                                    4095.0
                                } else {
                                    fx * 6.0 + z as f32 * 100.0
                                }
                            }
                        };
                        data.push(v.clamp(0.0, 65535.0) as u16);
                    }
                }
                planes.push(Buffer::from_vec(data));
            }
        }
    }
    Image::from_planes(meta, planes).expect("consistent planes")
}

/// A 20000 x 20000 16-bit image computed on demand: never in memory.
#[derive(Debug)]
struct Huge {
    meta: ImageMeta,
    delay: Duration,
    regions: AtomicU64,
    samples: AtomicU64,
    largest: AtomicU64,
}

const HUGE_SIDE: usize = 20000;

/// The sample at `(x, y)`: 1000-pixel blocks in a checkerboard of two
/// brightness bands, a gradient inside each block, one-pixel grid lines
/// every 100 pixels, a bright diagonal and fine one-pixel texture.
fn huge_value(x: usize, y: usize) -> u16 {
    let (bx, by) = (x / 1000, y / 1000);
    let base = if (bx + by) % 2 == 0 { 9000 } else { 30000 };
    let gradient = ((x % 1000) * 12 + (y % 1000) * 8) as u32;
    let mut v = base + gradient;
    if x.is_multiple_of(100) || y.is_multiple_of(100) {
        v = 52000;
    }
    if x.abs_diff(y) < 6 || (x + y).abs_diff(HUGE_SIDE) < 6 {
        v = 65535;
    }
    // Fine detail, so that a wrong sample is visible at 100%.
    v += ((x * 7 + y * 13) % 5) as u32 * 300;
    v.min(65535) as u16
}

impl PixelSource for Huge {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, plane: usize) -> Result<Buffer> {
        self.read_region(plane, Rect::new(0, 0, HUGE_SIDE, HUGE_SIDE))
    }

    fn read_region(&self, _plane: usize, r: Rect) -> Result<Buffer> {
        let n = (r.w * r.h) as u64;
        self.regions.fetch_add(1, Ordering::Relaxed);
        self.samples.fetch_add(n, Ordering::Relaxed);
        self.largest.fetch_max(n, Ordering::Relaxed);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let mut v = Vec::with_capacity(r.w * r.h);
        for y in r.y..r.y + r.h {
            v.extend((r.x..r.x + r.w).map(|x| huge_value(x, y)));
        }
        Ok(Buffer::from_vec(v))
    }
}

fn huge() -> (Image, Arc<Huge>) {
    let mut meta = ImageMeta::plane(DType::U16, HUGE_SIDE, HUGE_SIDE)
        .expect("valid size")
        .with_name("synthetic 20000 x 20000");
    meta.channels = vec![ChannelMeta {
        name: None,
        lut: None,
        display_range: Some((0.0, 65535.0)),
    }];
    let delay = std::env::var("IMAGER_VIEW_DELAY_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or_default();
    let source = Arc::new(Huge {
        meta,
        delay,
        regions: AtomicU64::new(0),
        samples: AtomicU64::new(0),
        largest: AtomicU64::new(0),
    });
    (Image::new(Shared(source.clone())), source)
}

/// Lets the example keep a handle on the source for its counters.
#[derive(Debug)]
struct Shared(Arc<Huge>);

impl PixelSource for Shared {
    fn meta(&self) -> &ImageMeta {
        self.0.meta()
    }
    fn read_plane(&self, plane: usize) -> Result<Buffer> {
        self.0.read_plane(plane)
    }
    fn read_region(&self, plane: usize, region: Rect) -> Result<Buffer> {
        self.0.read_region(plane, region)
    }
}

// ------------------------------------------------------------------ the app

struct Shot {
    path: String,
    requested: bool,
    quiet_frames: u32,
}

struct App {
    image: Image,
    huge: Option<Arc<Huge>>,
    view: ImageView,
    gpu: Option<ViewerGpu>,
    adapter: String,
    probe: Option<Probe>,
    renderer: Renderer,
    loading: bool,
    error: Option<String>,
    script: Vec<String>,
    shot: Option<Shot>,
    frames: u64,
    started: Instant,
    first_complete: Option<Duration>,
    worst_frame: Duration,
    total_frame: Duration,
    image_rect: egui::Rect,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, image: Image, huge: Option<Arc<Huge>>) -> App {
        // The viewer's GPU resources, from the renderer egui itself uses.
        let gpu = cc
            .wgpu_render_state
            .as_ref()
            .map(ViewerGpu::from_render_state);
        let adapter = match cc.wgpu_render_state.as_ref() {
            Some(state) => {
                let info = state.adapter.get_info();
                state.device.on_uncaptured_error(Arc::new(|e| {
                    eprintln!("[viewer] WGPU ERROR: {e}");
                }));
                format!(
                    "{} ({:?}, {:?}), target {:?}, max texture {}",
                    info.name,
                    info.backend,
                    info.device_type,
                    state.target_format,
                    state.device.limits().max_texture_dimension_2d
                )
            }
            None => "no wgpu renderer".to_string(),
        };
        eprintln!("[viewer] adapter: {adapter}");
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let options = ViewOptions {
            force_cpu: env("IMAGER_VIEW_CPU").is_some(),
            ..ViewOptions::default()
        };
        App {
            image,
            huge,
            view: ImageView::new(options),
            gpu,
            adapter,
            probe: None,
            renderer: Renderer::Cpu,
            loading: false,
            error: None,
            script: env("IMAGER_VIEW_SCRIPT")
                .map(|s| s.split(',').map(|a| a.trim().to_string()).rev().collect())
                .unwrap_or_default(),
            shot: env("IMAGER_VIEW_SHOT").map(|path| Shot {
                path,
                requested: false,
                quiet_frames: 0,
            }),
            frames: 0,
            started: Instant::now(),
            first_complete: None,
            worst_frame: Duration::ZERO,
            total_frame: Duration::ZERO,
            image_rect: egui::Rect::NOTHING,
        }
    }

    fn run_action(&mut self, action: &str) {
        let parts: Vec<&str> = action.split(':').collect();
        let number = |i: usize| parts.get(i).and_then(|s| s.parse::<f64>().ok());
        match (parts[0], number(1), number(2)) {
            ("zoom_in", ..) => self.view.zoom_in(),
            ("zoom_out", ..) => self.view.zoom_out(),
            ("fit", ..) => self.view.fit(),
            ("actual", ..) => self.view.actual_size(),
            ("zoom", Some(z), _) => self.view.set_zoom(z),
            ("center", Some(x), Some(y)) => self.view.set_center([x, y]),
            ("pan", Some(dx), Some(dy)) => {
                let c = self.view.center();
                self.view.set_center([c[0] + dx, c[1] + dy]);
            }
            ("pos", Some(axis), Some(index)) => {
                let mut position = self.view.position().to_vec();
                if let Some(p) = position.get_mut(axis as usize) {
                    *p = index as usize;
                }
                self.view.set_position(&position);
            }
            ("hide", Some(c), _) => self.view.set_channel_visible(c as usize, false),
            ("show", Some(c), _) => self.view.set_channel_visible(c as usize, true),
            _ => eprintln!("[viewer] unknown script action {action:?}"),
        }
        eprintln!("[viewer] script: {action}");
    }

    /// What the image area should show, from the CPU renderer alone.
    fn reference(&mut self, width: u32, height: u32) -> Result<Rgba8> {
        let meta = self.image.meta().clone();
        if meta.plane_len() <= 32 << 20 {
            return self.view.render_cpu(&self.image, width, height);
        }
        // Too large to render whole: gather just the samples the view
        // shows, one per device pixel, and render those.
        let viewport = Viewport::new(0.0, 0.0, width as f64, height as f64);
        let state = ViewState {
            zoom: self.view.zoom(),
            center: self.view.center(),
        };
        let transform = state.snapped_transform(&viewport);
        let step = TileGrid::step(level_for_zoom(transform.zoom));
        let (w, h) = (meta.width(), meta.height());
        let mut samples = vec![0u16; (width * height) as usize];
        let mut inside = vec![false; samples.len()];
        for py in 0..height as usize {
            for px in 0..width as usize {
                if let Some((x, y)) = transform.displayed_pixel(px as f64, py as f64, step, w, h) {
                    let i = py * width as usize + px;
                    samples[i] = huge_value(x, y);
                    inside[i] = true;
                }
            }
        }
        let channel = meta.channels.first().cloned().unwrap_or_default();
        let plan = DisplayPlan {
            dtype: meta.dtype(),
            rgb: false,
            blend: imager_render::Blend::Sum,
            channels: vec![ChannelDisplay {
                plane: 0,
                channel: 0,
                range: channel.display_range.unwrap_or((0.0, 65535.0)),
                lut: channel.lut.unwrap_or_else(Lut::grays),
            }],
        };
        let mut out = render_tile_cpu(
            &plan,
            &[Buffer::from_vec(samples)],
            width as usize,
            height as usize,
        )?;
        let [r, g, b] = self.view.options().background;
        for (pixel, inside) in out.data.as_chunks_mut::<4>().0.iter_mut().zip(inside) {
            if !inside {
                pixel.copy_from_slice(&[r, g, b, 255]);
            }
        }
        Ok(out)
    }

    fn report(&mut self, shot: &egui::ColorImage, ppp: f32) {
        let stats = self.view.stats();
        eprintln!(
            "[viewer] renderer {:?}, zoom {:.4}, centre {:?}, position {:?}",
            self.renderer,
            self.view.zoom(),
            self.view.center(),
            self.view.position()
        );
        eprintln!("[viewer] {stats:?}");
        eprintln!(
            "[viewer] frames {}, first complete frame after {:?}, viewer widget per frame: mean {:?}, worst {:?}",
            self.frames,
            self.first_complete,
            self.total_frame / self.frames.max(1) as u32,
            self.worst_frame
        );
        if let Some(huge) = &self.huge {
            eprintln!(
                "[viewer] source: {} region reads, {} samples in total, largest region {} samples",
                huge.regions.load(Ordering::Relaxed),
                huge.samples.load(Ordering::Relaxed),
                huge.largest.load(Ordering::Relaxed)
            );
        }
        // Compare the image area of the screenshot with the reference.
        let area = Viewport::from_points(self.image_rect, ppp);
        let (x0, y0) = (area.min[0] as usize, area.min[1] as usize);
        let (w, h) = (area.size[0] as usize, area.size[1] as usize);
        let [sw, sh] = shot.size;
        if w == 0 || h == 0 || x0 + w > sw || y0 + h > sh {
            eprintln!("[viewer] COMPARE skipped: image area {x0},{y0} {w}x{h} outside {sw}x{sh}");
            return;
        }
        match self.reference(w as u32, h as u32) {
            Ok(reference) => {
                let (mut differing, mut worst) = (0usize, 0u8);
                for y in 0..h {
                    for x in 0..w {
                        let got = shot.pixels[(y0 + y) * sw + x0 + x].to_array();
                        let want = &reference.data[(y * w + x) * 4..][..3];
                        let d = (0..3).map(|c| got[c].abs_diff(want[c])).max().unwrap_or(0);
                        differing += (d != 0) as usize;
                        worst = worst.max(d);
                    }
                }
                eprintln!(
                    "[viewer] COMPARE screenshot vs CPU reference over {w}x{h} image area at {x0},{y0}: \
                     {differing} of {} pixels differ, largest difference {worst}",
                    w * h
                );
            }
            Err(e) => eprintln!("[viewer] COMPARE failed: {e}"),
        }
    }

    fn unattended(&mut self, ctx: &egui::Context) {
        // One scripted action every third frame, once the window is up.
        if self.frames >= 5
            && self.frames.is_multiple_of(3)
            && let Some(action) = self.script.pop()
        {
            self.run_action(&action);
        }
        let Some(shot) = &mut self.shot else {
            return;
        };
        ctx.request_repaint();
        let ready = self.script.is_empty() && !self.loading && self.frames >= 10;
        shot.quiet_frames = if ready { shot.quiet_frames + 1 } else { 0 };
        let timed_out = self.started.elapsed() > Duration::from_secs(120);
        if (shot.quiet_frames >= 5 || timed_out) && !shot.requested {
            shot.requested = true;
            if timed_out {
                eprintln!("[viewer] still loading after 120 s; taking the screenshot anyway");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let picture = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(picture) = picture else {
            return;
        };
        let path = shot.path.clone();
        self.shot = None;
        match save_png(&path, &picture) {
            Ok(()) => eprintln!(
                "[viewer] screenshot saved: {path} ({}x{})",
                picture.size[0], picture.size[1]
            ),
            Err(e) => eprintln!("[viewer] could not save {path}: {e}"),
        }
        self.report(&picture, ctx.pixels_per_point());
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn save_png(path: &str, picture: &egui::ColorImage) -> std::result::Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let [w, h] = picture.size;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(picture.as_raw())
        .map_err(|e| e.to_string())
}

fn probe_text(probe: &Probe) -> String {
    let values: Vec<String> = probe
        .values
        .iter()
        .map(|v| match v.value {
            Some(value) => format!("{value}"),
            None => "...".to_string(),
        })
        .collect();
    let unit = |u: &Option<String>| u.clone().map(|u| format!(" {u}")).unwrap_or_default();
    format!(
        "x={}, y={}  ({:.3}{}, {:.3}{})  value={}",
        probe.x,
        probe.y,
        probe.calibrated[0].value,
        unit(&probe.calibrated[0].unit),
        probe.calibrated[1].value,
        unit(&probe.calibrated[1].unit),
        values.join(", ")
    )
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frames += 1;
        let ctx = ui.ctx().clone();
        self.unattended(&ctx);

        // The application binds the shortcuts; the widget only has methods.
        let (zoom_in, zoom_out, fit, actual, left, right) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals),
                i.key_pressed(egui::Key::Minus),
                i.key_pressed(egui::Key::Num0),
                i.key_pressed(egui::Key::Num1),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
            )
        });
        if zoom_in {
            self.view.zoom_in();
        }
        if zoom_out {
            self.view.zoom_out();
        }
        if fit {
            self.view.fit();
        }
        if actual {
            self.view.actual_size();
        }
        if left {
            self.view.step_last_axis(-1);
        }
        if right {
            self.view.step_last_axis(1);
        }

        egui::Panel::top("tools").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Zoom In").clicked() {
                    self.view.zoom_in();
                }
                if ui.button("Zoom Out").clicked() {
                    self.view.zoom_out();
                }
                if ui.button("Fit").clicked() {
                    self.view.fit();
                }
                if ui.button("100%").clicked() {
                    self.view.actual_size();
                }
                let meta = self.image.meta();
                ui.label(format!(
                    "{}  {}x{} {}  {:?}",
                    meta.name,
                    meta.width(),
                    meta.height(),
                    meta.dtype(),
                    meta.color
                ));
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let status = match (&self.error, &self.probe) {
                    (Some(error), _) => error.clone(),
                    (None, Some(probe)) => probe_text(probe),
                    (None, None) => String::new(),
                };
                ui.label(status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let stats = self.view.stats();
                    ui.label(format!(
                        "{:?}{}  {} tiles, {:.0} MB",
                        self.renderer,
                        if self.loading { " (loading)" } else { "" },
                        stats.tiles,
                        (stats.sample_bytes + stats.gpu_bytes + stats.rgba_bytes) as f64 / 1e6
                    ))
                    .on_hover_text(&self.adapter);
                    // Double-click the zoom readout to fit.
                    let zoom = ui.add(
                        egui::Label::new(format!("{:.1}%", self.view.zoom() * 100.0))
                            .sense(egui::Sense::click()),
                    );
                    if zoom.double_clicked() {
                        self.view.fit();
                    }
                });
            });
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let start = Instant::now();
                let shown = self.view.show(ui, &self.image, self.gpu.as_ref());
                let took = start.elapsed();
                self.worst_frame = self.worst_frame.max(took);
                self.total_frame += took;
                self.image_rect = shown.response.rect;
                self.probe = shown.probe;
                self.renderer = shown.renderer;
                self.loading = shown.loading;
                self.error = shown.error;
                if !self.loading && self.first_complete.is_none() {
                    self.first_complete = Some(self.started.elapsed());
                }
            });
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let (image, huge_source) = if flag("--huge") {
        let (image, source) = huge();
        (image, Some(source))
    } else if flag("--hyperstack") {
        (hyperstack(flag("--gray")), None)
    } else if let Some(path) = args.iter().find(|a| !a.starts_with("--")) {
        match imager_io::open(path) {
            Ok(image) => (image, None),
            Err(e) => {
                eprintln!("Could not open {path}: {e}");
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("usage: viewer <image file> | --hyperstack [--gray] | --huge");
        std::process::exit(2);
    };
    eprintln!(
        "[viewer] image: {:?} shape {:?} {} {:?}, in memory: {}",
        image.meta().name,
        image.meta().shape(),
        image.meta().dtype(),
        image.meta().color,
        image.is_in_memory()
    );

    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 720.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    if std::env::var("IMAGER_VIEW_SOFTWARE").is_ok_and(|v| !v.is_empty())
        && let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
    {
        // In debug builds wgpu loads the D3D12 debug layer, which crashes
        // on this path on some machines (docs/spikes/s2-docking.md).
        setup
            .instance_descriptor
            .flags
            .remove(wgpu::InstanceFlags::DEBUG);
        setup.native_adapter_selector = Some(Arc::new(|adapters, _surface| {
            adapters
                .iter()
                .find(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
                .cloned()
                .ok_or_else(|| "no software adapter is available".to_string())
        }));
    }
    eframe::run_native(
        "ImageR2 viewer example",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, image, huge_source)))),
    )
}
