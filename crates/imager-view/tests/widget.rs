//! The widget driven headlessly with `egui_kittest`, without a GPU: the
//! image is drawn by the CPU renderer as ordinary egui textures.

use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use imager_core::{
    Axis, AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut, PixelSource,
    Rect,
};
use imager_view::{ImageView, Probe, Renderer, ViewOptions};

struct App {
    view: ImageView,
    image: Image,
    controls: bool,
    rect: egui::Rect,
    probe: Option<Probe>,
    zoom: f64,
    loading: bool,
    renderer: Option<Renderer>,
    error: Option<String>,
}

impl App {
    fn new(image: Image) -> App {
        App {
            view: ImageView::default(),
            image,
            controls: false,
            rect: egui::Rect::NOTHING,
            probe: None,
            zoom: 0.0,
            loading: false,
            renderer: None,
            error: None,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let shown = if self.controls {
            self.view.show(ui, &self.image, None)
        } else {
            self.view.ui(ui, &self.image, None)
        };
        self.rect = shown.response.rect;
        self.probe = shown.probe;
        self.zoom = shown.zoom;
        self.loading = shown.loading;
        self.renderer = Some(shown.renderer);
        self.error = shown.error;
    }
}

fn harness(app: App) -> Harness<'static, App> {
    let mut h = Harness::builder()
        .with_size(vec2(480.0, 360.0))
        .with_max_steps(64)
        .build_ui_state(|ui, app: &mut App| app.ui(ui), app);
    h.run_steps(3);
    h
}

fn wheel(h: &Harness<'_, App>, lines: f32, modifiers: Modifiers) {
    h.event(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, lines),
        phase: TouchPhase::Move,
        modifiers,
    });
}

/// Samples that encode their own position.
fn value(x: usize, y: usize) -> u16 {
    (x + 300 * y) as u16
}

fn gradient(w: usize, h: usize) -> Image {
    Image::from_vec(w, h, (0..w * h).map(|i| value(i % w, i / w)).collect()).unwrap()
}

/// The image pixel the view shows under a position in points, from its
/// public state (the harness runs at one pixel per point).
fn expected_pixel(app: &App, pos: Pos2) -> (usize, usize) {
    let c = app.rect.center();
    let center = app.view.center();
    let origin_x = (c.x as f64 - center[0] * app.zoom).round();
    let origin_y = (c.y as f64 - center[1] * app.zoom).round();
    (
        ((pos.x as f64 - origin_x) / app.zoom).floor() as usize,
        ((pos.y as f64 - origin_y) / app.zoom).floor() as usize,
    )
}

#[test]
fn a_small_image_opens_at_actual_size_and_a_large_one_fits() {
    let h = harness(App::new(gradient(64, 48)));
    assert_eq!(h.state().zoom, 1.0);
    assert_eq!(h.state().renderer, Some(Renderer::Cpu));
    assert_eq!(h.state().error, None);
    assert!(!h.state().loading);

    let big = Image::from_vec(2000, 3000, vec![0u8; 6_000_000]).unwrap();
    let h = harness(App::new(big));
    let rect = h.state().rect;
    let fit = (rect.width() as f64 / 2000.0).min(rect.height() as f64 / 3000.0);
    assert!((h.state().zoom - fit).abs() < 1e-9);
}

#[test]
fn wheel_zooms_in_steps_about_the_cursor() {
    let mut app = App::new(gradient(300, 200));
    app.view.set_zoom(4.0);
    let mut h = harness(app);
    assert_eq!(h.state().zoom, 4.0);

    let cursor = pos2(140.0, 95.0);
    h.hover_at(cursor);
    h.run_steps(2);
    let before = h.state().probe.clone().expect("cursor is over the image");
    assert_eq!((before.x, before.y), expected_pixel(h.state(), cursor));

    // One notch up: the next step, and the same pixel still under the cursor.
    wheel(&h, 1.0, Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().zoom, 6.0);
    let after = h.state().probe.clone().unwrap();
    assert_eq!((after.x, after.y), (before.x, before.y));

    wheel(&h, 1.0, Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().zoom, 8.0);
    assert_eq!(
        h.state().probe.clone().map(|p| (p.x, p.y)),
        Some((before.x, before.y))
    );

    // Three notches down in one event: three steps back.
    wheel(&h, -3.0, Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().zoom, 3.0);
    assert_eq!(
        h.state().probe.clone().map(|p| (p.x, p.y)),
        Some((before.x, before.y))
    );

    // The wheel does nothing when the cursor is elsewhere.
    h.remove_cursor();
    h.run_steps(2);
    wheel(&h, 1.0, Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().zoom, 3.0);
    assert!(h.state().probe.is_none());
}

#[test]
fn zoom_methods_step_fit_and_reset() {
    let mut h = harness(App::new(gradient(300, 200)));
    assert_eq!(h.state().zoom, 1.0);
    h.state_mut().view.zoom_in();
    h.run_steps(2);
    assert_eq!(h.state().zoom, 1.5);
    h.state_mut().view.zoom_out();
    h.state_mut().view.zoom_out();
    h.run_steps(2);
    assert_eq!(h.state().zoom, 0.75);
    h.state_mut().view.fit();
    h.run_steps(2);
    let rect = h.state().rect;
    let fit = (rect.width() as f64 / 300.0).min(rect.height() as f64 / 200.0);
    assert!((h.state().zoom - fit).abs() < 1e-9);
    assert_eq!(h.state().view.zoom(), h.state().zoom);
    h.state_mut().view.actual_size();
    h.run_steps(2);
    assert_eq!(h.state().zoom, 1.0);
    // Zooming out stops at the smallest step.
    for _ in 0..40 {
        h.state_mut().view.zoom_out();
    }
    h.run_steps(2);
    assert_eq!(h.state().zoom, 1.0 / 72.0);
    for _ in 0..40 {
        h.state_mut().view.zoom_in();
    }
    h.run_steps(2);
    assert_eq!(h.state().zoom, 32.0);
}

#[test]
fn middle_drag_and_space_drag_pan() {
    let mut app = App::new(gradient(300, 200));
    app.view.set_zoom(4.0);
    let mut h = harness(app);
    let start = h.state().view.center();
    assert_eq!(start, [150.0, 100.0]);

    // Middle button: drag 80 right and 40 down moves the image with it.
    let from = pos2(200.0, 150.0);
    h.hover_at(from);
    h.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Middle,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    h.hover_at(pos2(240.0, 170.0));
    h.run_steps(2);
    h.hover_at(pos2(280.0, 190.0));
    h.run_steps(2);
    h.event(Event::PointerButton {
        pos: pos2(280.0, 190.0),
        button: PointerButton::Middle,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    let after = h.state().view.center();
    assert!(
        (after[0] - (start[0] - 80.0 / 4.0)).abs() < 1e-6,
        "{after:?}"
    );
    assert!(
        (after[1] - (start[1] - 40.0 / 4.0)).abs() < 1e-6,
        "{after:?}"
    );

    // A plain primary drag does not pan (it belongs to tools)...
    h.drag_at(from);
    h.run_steps(2);
    h.hover_at(pos2(100.0, 100.0));
    h.run_steps(2);
    h.drop_at(pos2(100.0, 100.0));
    h.run_steps(2);
    assert_eq!(h.state().view.center(), after);

    // ...but with Space held it does.
    h.key_down(egui::Key::Space);
    h.hover_at(from);
    h.drag_at(from);
    h.run_steps(2);
    h.hover_at(pos2(160.0, 150.0));
    h.run_steps(2);
    h.drop_at(pos2(160.0, 150.0));
    h.key_up(egui::Key::Space);
    h.run_steps(2);
    let spaced = h.state().view.center();
    assert!(
        (spaced[0] - (after[0] + 40.0 / 4.0)).abs() < 1e-6,
        "{spaced:?}"
    );
    assert_eq!(spaced[1], after[1]);

    // Panning cannot lose the image: a huge drag stops at its edge.
    h.state_mut().view.set_center([-5000.0, 9000.0]);
    h.run_steps(2);
    let rect = h.state().rect;
    let clamped = h.state().view.center();
    assert!((clamped[0] - rect.width() as f64 / 8.0).abs() < 1e-6);
    assert!((clamped[1] - (200.0 - rect.height() as f64 / 8.0)).abs() < 1e-6);
}

#[test]
fn probe_reports_pixel_value_and_calibration() {
    let (w, hgt) = (300, 200);
    let mut meta = ImageMeta::new(
        DType::U16,
        vec![
            (Axis::calibrated(AxisKind::X, 0.5, "um"), w),
            (
                Axis {
                    kind: AxisKind::Y,
                    scale: 0.25,
                    origin: 10.0,
                    unit: Some("um".to_string()),
                },
                hgt,
            ),
        ],
    )
    .unwrap();
    meta.name = "calibrated".to_string();
    let samples = Buffer::from_vec(
        (0..w * hgt)
            .map(|i| value(i % w, i / w))
            .collect::<Vec<_>>(),
    );
    let image = Image::from_planes(meta, vec![samples]).unwrap();
    let mut app = App::new(image);
    app.view.set_zoom(3.0);
    app.view.set_center([120.0, 90.0]);
    let mut h = harness(app);

    for pos in [pos2(31.0, 47.0), pos2(250.5, 199.5), pos2(400.0, 300.0)] {
        h.hover_at(pos);
        h.run_steps(2);
        let probe = h.state().probe.clone().expect("over the image");
        let (x, y) = expected_pixel(h.state(), pos);
        assert_eq!((probe.x, probe.y), (x, y));
        assert_eq!(probe.values.len(), 1);
        assert_eq!(probe.values[0].channel, 0);
        assert_eq!(probe.values[0].value, Some(value(x, y) as f64));
        assert_eq!(probe.calibrated[0].value, x as f64 * 0.5);
        assert_eq!(probe.calibrated[1].value, 10.0 + y as f64 * 0.25);
        assert_eq!(probe.calibrated[0].unit.as_deref(), Some("um"));
    }

    // Outside the image there is nothing to report.
    h.state_mut().view.set_zoom(0.5);
    h.run_steps(2);
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(2);
    assert!(h.state().probe.is_none());
}

fn stack(color: ColorModel) -> Image {
    // 3 channels, 4 slices, 5 frames; each plane a constant: its number.
    let (w, hgt) = (40, 30);
    let mut meta = ImageMeta::xyczt(DType::U16, w, hgt, 3, 4, 5).unwrap();
    meta.color = color;
    meta.channels = ["DAPI", "GFP", "RFP"]
        .iter()
        .map(|name| ChannelMeta {
            name: Some(name.to_string()),
            lut: Some(Lut::grays()),
            display_range: Some((0.0, 100.0)),
        })
        .collect();
    let planes = (0..60)
        .map(|p| Buffer::from_vec(vec![p as u16; w * hgt]))
        .collect();
    Image::from_planes(meta, planes).unwrap()
}

#[test]
fn composite_probe_reports_every_channel() {
    let mut app = App::new(stack(ColorModel::Composite));
    app.view.set_position(&[0, 2, 1]);
    let mut h = harness(app);
    h.hover_at(h.state().rect.center());
    h.run_steps(2);
    let probe = h.state().probe.clone().unwrap();
    // Plane number = c + 3 * z + 12 * t.
    let values: Vec<_> = probe.values.iter().map(|v| (v.channel, v.value)).collect();
    assert_eq!(values, [(0, Some(18.0)), (1, Some(19.0)), (2, Some(20.0))]);

    // Grayscale: only the channel shown.
    let mut app = App::new(stack(ColorModel::Grayscale));
    app.view.set_position(&[1, 3, 4]);
    let mut h = harness(app);
    h.hover_at(h.state().rect.center());
    h.run_steps(2);
    let probe = h.state().probe.clone().unwrap();
    let values: Vec<_> = probe.values.iter().map(|v| (v.channel, v.value)).collect();
    assert_eq!(values, [(1, Some(58.0))]);
}

#[test]
fn position_controls_and_stepping() {
    let mut app = App::new(stack(ColorModel::Composite));
    app.controls = true;
    let mut h = harness(app);
    assert_eq!(h.state().view.position(), [0, 0, 0]);
    // The image still gets most of the window; the controls sit below it.
    let rect = h.state().rect;
    assert!(rect.height() > 250.0 && rect.height() < 360.0, "{rect:?}");

    // Sliders for Z and T are present and labelled.
    h.get_by_role_and_label(Role::Label, "Z");
    h.get_by_role_and_label(Role::Label, "T");
    assert_eq!(h.query_all_by_role(Role::Slider).count(), 2);

    // Channel buttons toggle visibility in a composite.
    assert!(h.state().view.channel_visible(1));
    h.get_by_label("2").click();
    h.run_steps(3);
    assert!(!h.state().view.channel_visible(1));
    assert!(h.state().view.channel_visible(0) && h.state().view.channel_visible(2));
    h.get_by_label("2").click();
    h.run_steps(3);
    assert!(h.state().view.channel_visible(1));

    // Stepping: with no slider used yet, the first non-channel axis (Z).
    assert!(h.state_mut().view.step_last_axis(1));
    assert_eq!(h.state().view.position(), [0, 1, 0]);
    assert!(h.state_mut().view.step_axis(2, 3));
    assert_eq!(h.state().view.position(), [0, 1, 3]);
    // Now time was used last; stepping stops at the ends.
    assert!(h.state_mut().view.step_last_axis(5));
    assert_eq!(h.state().view.position(), [0, 1, 4]);
    assert!(!h.state_mut().view.step_last_axis(1));

    // Shift+wheel steps the last-used axis: down is forward.
    h.state_mut().view.set_position(&[0, 1, 1]);
    h.hover_at(h.state().rect.center());
    h.run_steps(2);
    let zoom = h.state().zoom;
    wheel(&h, -1.0, Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(h.state().view.position(), [0, 1, 2]);
    wheel(&h, 2.0, Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(h.state().view.position(), [0, 1, 0]);
    assert_eq!(h.state().zoom, zoom);
    // The probe follows the position: plane 0 + 3 * 1 + 12 * 0.
    let probe = h.state().probe.clone().unwrap();
    assert_eq!(probe.values[0].value, Some(3.0));

    // In a grayscale stack the channel buttons select the channel shown.
    let mut app = App::new(stack(ColorModel::Grayscale));
    app.controls = true;
    let mut h = harness(app);
    h.get_by_label("3").click();
    h.run_steps(3);
    assert_eq!(h.state().view.position(), [2, 0, 0]);
}

/// Not in memory, and slow enough that a frame cannot wait for it.
#[derive(Debug)]
struct Slow {
    meta: ImageMeta,
    largest: std::sync::Mutex<usize>,
}

impl PixelSource for Slow {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }
    fn read_plane(&self, _plane: usize) -> imager_core::Result<Buffer> {
        panic!("the viewer must not read whole planes of a lazy image");
    }
    fn read_region(&self, _plane: usize, r: Rect) -> imager_core::Result<Buffer> {
        let mut largest = self.largest.lock().unwrap();
        *largest = (*largest).max(r.w * r.h);
        drop(largest);
        std::thread::sleep(Duration::from_millis(30));
        let mut v = Vec::with_capacity(r.w * r.h);
        for y in r.y..r.y + r.h {
            v.extend((r.x..r.x + r.w).map(|x| value(x, y)));
        }
        Ok(Buffer::from_vec(v))
    }
}

#[test]
fn a_lazy_image_loads_in_the_background() {
    let (w, hgt) = (6000, 4000);
    let mut meta = ImageMeta::plane(DType::U16, w, hgt).unwrap();
    meta.channels = vec![ChannelMeta {
        name: None,
        lut: None,
        display_range: Some((0.0, 65535.0)),
    }];
    let image = Image::new(Slow {
        meta,
        largest: std::sync::Mutex::new(0),
    });
    let mut app = App::new(image);
    app.view = ImageView::new(ViewOptions {
        tile_size: 256,
        ..ViewOptions::default()
    });
    app.view.set_zoom(1.0);
    app.view.set_center([3000.0, 2000.0]);

    let start = Instant::now();
    let mut h = Harness::builder()
        .with_size(vec2(480.0, 360.0))
        .build_ui_state(|ui, app: &mut App| app.ui(ui), app);
    h.step();
    // The first frame returns at once, with tiles still to come.
    assert!(h.state().loading);
    assert!(start.elapsed() < Duration::from_millis(1500));
    h.hover_at(h.state().rect.center());

    let deadline = Instant::now() + Duration::from_secs(30);
    while h.state().loading {
        assert!(Instant::now() < deadline, "tiles never arrived");
        std::thread::sleep(Duration::from_millis(10));
        h.step();
    }
    h.step();
    assert_eq!(h.state().error, None);
    // The probe reads from the loaded tile.
    let probe = h.state().probe.clone().unwrap();
    assert_eq!((probe.x, probe.y), (3000, 2000));
    assert_eq!(probe.values[0].value, Some(value(3000, 2000) as f64));
    // Only tiles touching the 480 x 360 view were read: at most 3 x 3.
    let stats = h.state().view.stats();
    assert!(stats.tiles_read >= 4 && stats.tiles_read <= 9, "{stats:?}");
    assert_eq!(stats.rgba_bytes, stats.tiles * 256 * 256 * 4);
}
