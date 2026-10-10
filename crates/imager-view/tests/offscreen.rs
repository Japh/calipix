//! Offscreen GPU renders compared with the CPU reference renderer. No
//! window is needed; the adapter is chosen by wgpu's usual environment
//! variables (`WGPU_BACKEND`, `WGPU_ADAPTER_NAME`). Run with `--nocapture`
//! to see the adapter and the measured differences and times.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use egui_wgpu::wgpu;
use imager_core::{
    Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut, PixelSource, Rect,
};
use imager_render::{
    Blend, ChannelMode, DisplayOptions, Reduction, Rgba8, render_plane, render_plane_with,
    render_region,
};
use imager_view::{ImageView, OFFSCREEN_FORMAT, ViewOptions, ViewerGpu};

static ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// One device for all tests. Created without wgpu's debug flag: in debug
/// builds that flag loads the D3D12 debug layer, which crashes headless
/// DX12 instance creation on some machines (see docs/spikes/s2-docking.md).
/// Validation stays on, and validation errors fail the tests.
fn gpu() -> &'static ViewerGpu {
    static GPU: OnceLock<ViewerGpu> = OnceLock::new();
    GPU.get_or_init(|| {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        descriptor.flags.remove(wgpu::InstanceFlags::DEBUG);
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(wgpu::util::initialize_adapter_from_env_or_default(
            &instance, None,
        ))
        .expect("no graphics adapter");
        // Default limits, as eframe requests: textures up to 8192 a side.
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("no device");
        device.on_uncaptured_error(std::sync::Arc::new(|e| {
            ERRORS.lock().unwrap().push(e.to_string());
        }));
        let info = adapter.get_info();
        println!(
            "ADAPTER {:?} backend={:?} type={:?} max_texture_dimension_2d={}",
            info.name,
            info.backend,
            info.device_type,
            device.limits().max_texture_dimension_2d
        );
        ViewerGpu::new(&device, &queue, &info, OFFSCREEN_FORMAT)
    })
}

fn no_gpu_errors() {
    let errors = ERRORS.lock().unwrap();
    assert!(errors.is_empty(), "wgpu errors: {errors:?}");
}

#[derive(Debug, Default)]
struct Diff {
    pixels: usize,
    differing: usize,
    max: u8,
}

fn diff(a: &Rgba8, b: &Rgba8) -> Diff {
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert_eq!(a.data.len(), b.data.len());
    let mut d = Diff {
        pixels: a.width * a.height,
        ..Diff::default()
    };
    for (p, q) in a
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.data.as_chunks::<4>().0)
    {
        let m = (0..4).map(|c| p[c].abs_diff(q[c])).max().unwrap();
        d.differing += (m != 0) as usize;
        d.max = d.max.max(m);
    }
    d
}

/// Deterministic noise in 0..2^32.
fn hash(x: usize, y: usize, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h
}

/// Left half a diagonal ramp, right half noise, so that both smooth
/// structure and every sample value appear.
fn pattern(x: usize, y: usize, w: usize, seed: u32, span: u32) -> u32 {
    if x < w / 2 {
        ((x * 37 + y * 101) as u32 + seed * 977) % span
    } else {
        hash(x, y, seed) % span
    }
}

fn u8_plane(w: usize, h: usize, seed: u32) -> Buffer {
    Buffer::from_vec(
        (0..w * h)
            .map(|i| pattern(i % w, i / w, w, seed, 256) as u8)
            .collect(),
    )
}

fn u16_plane(w: usize, h: usize, seed: u32) -> Buffer {
    Buffer::from_vec(
        (0..w * h)
            .map(|i| pattern(i % w, i / w, w, seed, 65536) as u16)
            .collect(),
    )
}

fn i16_plane(w: usize, h: usize, seed: u32) -> Buffer {
    Buffer::from_vec(
        (0..w * h)
            .map(|i| (pattern(i % w, i / w, w, seed, 65536) as i32 - 32768) as i16)
            .collect(),
    )
}

fn f32_plane(w: usize, h: usize, seed: u32, specials: bool) -> Buffer {
    Buffer::from_vec(
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                match hash(x, y, seed ^ 0x55) % 997 {
                    0 => f32::NAN,
                    1 if specials => f32::INFINITY,
                    2 if specials => f32::NEG_INFINITY,
                    _ => pattern(x, y, w, seed, 1 << 20) as f32 / 4096.0 - 100.0,
                }
            })
            .collect(),
    )
}

fn fire() -> Lut {
    // Not ImageJ's Fire; just a colourful table whose neighbouring entries
    // differ by several levels, so an off-by-one level is visible.
    Lut::from_fn("test fire", |i| {
        let t = i as u32;
        [
            (t * 3).min(255) as u8,
            ((t * t) / 255) as u8,
            ((t * 7) % 256) as u8,
        ]
    })
}

fn with_display(image: &Image, range: Option<(f64, f64)>, lut: Option<Lut>) -> Image {
    let mut meta = image.meta().clone();
    meta.channels = vec![ChannelMeta {
        name: None,
        lut,
        display_range: range,
    }];
    image.with_meta(meta).unwrap()
}

fn options(tile: usize) -> ViewOptions {
    ViewOptions {
        tile_size: tile,
        ..ViewOptions::default()
    }
}

/// Renders the image whole at 100% on the GPU and compares it with
/// `imager_render::render_plane` directly.
fn against_render_plane(image: &Image, position: &[usize], tile: usize) -> Diff {
    let meta = image.meta();
    let (w, h) = (meta.width() as u32, meta.height() as u32);
    let mut view = ImageView::new(options(tile));
    view.set_position(position);
    let got = view.render_gpu(image, gpu(), w, h).unwrap();
    let want = render_plane(image, position).unwrap();
    diff(&got, &want)
}

/// A way of looking at an image: zoom (`None` for fit) and the image point
/// at the middle of the view.
#[derive(Clone, Copy, Debug)]
struct Look {
    zoom: Option<f64>,
    center: Option<[f64; 2]>,
    size: [u32; 2],
}

fn look(zoom: f64, center: [f64; 2], size: [u32; 2]) -> Look {
    Look {
        zoom: Some(zoom),
        center: Some(center),
        size,
    }
}

fn apply(view: &mut ImageView, l: Look) {
    match l.zoom {
        Some(z) => view.set_zoom(z),
        None => view.fit(),
    }
    if let Some(c) = l.center {
        view.set_center(c);
    }
}

/// Renders a view on the GPU and with the tile-free CPU renderer.
fn against_cpu_view(view: &mut ImageView, image: &Image, l: Look) -> Diff {
    apply(view, l);
    let got = view.render_gpu(image, gpu(), l.size[0], l.size[1]).unwrap();
    let want = view.render_cpu(image, l.size[0], l.size[1]).unwrap();
    // The widget's CPU fallback draws the same picture as the reference.
    let fallback = view.render_cpu_tiled(image, l.size[0], l.size[1]).unwrap();
    assert!(
        fallback == want,
        "CPU fallback differs from the reference for {l:?}"
    );
    diff(&got, &want)
}

/// 100%, magnified and reduced looks at a `w` x `h` image, including zooms
/// whose pixel centres fall exactly on sample boundaries (150%, 75%).
fn looks(w: usize, h: usize) -> Vec<Look> {
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    vec![
        // 100%, image smaller than the view: centred on background.
        look(1.0, [cx, cy], [w as u32 + 37, h as u32 + 21]),
        // 100%, view smaller than the image, off-centre.
        look(1.0, [cx * 0.7 + 3.0, cy * 1.2], [211, 160]),
        look(1.5, [cx * 1.1, cy * 0.9], [320, 240]),
        look(3.0, [cx + 10.3, cy - 7.6], [333, 251]),
        look(32.0, [w as f64 - 3.2, 2.9], [200, 150]),
        look(0.75, [cx, cy], [300, 200]),
        look(0.5, [cx + 20.0, cy], [180, 140]),
        look(1.0 / 3.0, [cx, cy], [256, 256]),
        look(0.25, [cx, cy], [130, 90]),
        // Fit: an arbitrary zoom.
        Look {
            zoom: None,
            center: None,
            size: [201, 173],
        },
    ]
}

fn exact_everywhere(name: &str, image: &Image, tile: usize) {
    let meta = image.meta();
    let mut view = ImageView::new(options(tile));
    for l in looks(meta.width(), meta.height()) {
        let d = against_cpu_view(&mut view, image, l);
        assert_eq!(d.differing, 0, "{name} {l:?}: {d:?}");
    }
    no_gpu_errors();
}

#[test]
fn u8_equals_cpu_exactly() {
    let (w, h) = (300, 200);
    let base = Image::from_planes(
        ImageMeta::plane(DType::U8, w, h).unwrap(),
        vec![u8_plane(w, h, 1)],
    )
    .unwrap();
    let ranges = [
        None,
        Some((10.0, 200.0)),
        Some((0.0, 50.0)),
        Some((128.0, 128.0)),
    ];
    let luts = [None, Some(fire()), Some(Lut::grays().inverted())];
    for range in ranges {
        for lut in &luts {
            let image = with_display(&base, range, lut.clone());
            let d = against_render_plane(&image, &[], 128);
            assert_eq!(d.differing, 0, "u8 {range:?}: {d:?}");
        }
    }
    exact_everywhere(
        "u8",
        &with_display(&base, Some((10.0, 200.0)), Some(fire())),
        128,
    );
    exact_everywhere("u8 default", &base, 1024);
}

#[test]
fn u16_equals_cpu_exactly() {
    // Neither side is a multiple of the tile size.
    let (w, h) = (517, 389);
    let base = Image::from_planes(
        ImageMeta::plane(DType::U16, w, h).unwrap(),
        vec![u16_plane(w, h, 2)],
    )
    .unwrap();
    let ranges = [
        None,
        Some((100.0, 355.0)),
        // A range where exact integer arithmetic disagrees with ImageJ.
        Some((0.0, 25087.0)),
        Some((0.0, 65535.0)),
        Some((1000.0, 1000.0)),
        Some((0.0, 4095.0)),
        Some((30000.5, 30100.75)),
    ];
    for range in ranges {
        for lut in [None, Some(fire())] {
            let image = with_display(&base, range, lut);
            let d = against_render_plane(&image, &[], 256);
            assert_eq!(d.differing, 0, "u16 {range:?}: {d:?}");
        }
    }
    exact_everywhere(
        "u16",
        &with_display(&base, Some((0.0, 25087.0)), Some(fire())),
        256,
    );
    exact_everywhere("u16 auto", &base, 100);
}

#[test]
fn i16_equals_cpu_exactly() {
    let (w, h) = (260, 190);
    let base = Image::from_planes(
        ImageMeta::plane(DType::I16, w, h).unwrap(),
        vec![i16_plane(w, h, 3)],
    )
    .unwrap();
    let ranges = [
        None,
        Some((-32768.0, 32767.0)),
        Some((-100.0, 100.0)),
        Some((-25088.0, -1.0)),
        Some((5.0, 4000.0)),
    ];
    for range in ranges {
        for lut in [None, Some(fire())] {
            let image = with_display(&base, range, lut);
            let d = against_render_plane(&image, &[], 128);
            assert_eq!(d.differing, 0, "i16 {range:?}: {d:?}");
        }
    }
    exact_everywhere(
        "i16",
        &with_display(&base, Some((-100.0, 100.0)), Some(fire())),
        128,
    );
}

#[test]
fn f32_is_within_one_level() {
    let (w, h) = (400, 300);
    let base = Image::from_planes(
        ImageMeta::plane(DType::F32, w, h).unwrap(),
        vec![f32_plane(w, h, 4, true)],
    )
    .unwrap();
    // The default range is the data's own; infinities would make it one the
    // CPU renderer must draw, so that case uses finite data (and NaN).
    let finite = Image::from_planes(
        ImageMeta::plane(DType::F32, w, h).unwrap(),
        vec![f32_plane(w, h, 4, false)],
    )
    .unwrap();
    let mut cpu_drawn = ImageView::new(options(256));
    let got = cpu_drawn
        .render_gpu(&base, gpu(), w as u32, h as u32)
        .unwrap();
    assert!(cpu_drawn.stats().cpu_renders > 0);
    let want = render_plane(&base, &[]).unwrap();
    assert_eq!(diff(&got, &want).differing, 0);
    let ranges = [
        None,
        Some((0.0, 1.0)),
        Some((-3.5, 12.25)),
        Some((-100.0, 156.0)),
        // A minimum that is not a single-precision number, with a range
        // narrow enough for the difference to matter.
        Some((100.000_123_4, 100.013_456_7)),
        Some((0.1, 0.7)),
    ];
    let (mut worst, mut differing, mut pixels) = (0, 0, 0);
    for range in ranges {
        // With a gray table the colour difference is the level difference.
        let source = if range.is_some() { &base } else { &finite };
        let image = with_display(source, range, None);
        let d = against_render_plane(&image, &[], 256);
        println!("f32 range {range:?}: {d:?}");
        assert!(d.max <= 1, "f32 {range:?}: {d:?}");
        worst = worst.max(d.max);
        differing += d.differing;
        pixels += d.pixels;
    }
    println!("F32 TOTAL: max level difference {worst}, {differing} of {pixels} pixels differ");
    // Views: same tolerance.
    let image = with_display(&base, Some((-3.5, 12.25)), None);
    let mut view = ImageView::new(options(256));
    for l in looks(w, h) {
        let d = against_cpu_view(&mut view, &image, l);
        assert!(d.max <= 1, "f32 {l:?}: {d:?}");
    }
    no_gpu_errors();
}

fn rgb_image(w: usize, h: usize) -> Image {
    let mut meta = ImageMeta::xyczt(DType::U8, w, h, 3, 1, 1).unwrap();
    meta.color = ColorModel::Rgb;
    Image::from_planes(meta, (0..3).map(|c| u8_plane(w, h, 10 + c)).collect()).unwrap()
}

#[test]
fn rgb_equals_cpu_exactly() {
    let (w, h) = (310, 170);
    let image = rgb_image(w, h);
    for position in [[0], [2]] {
        let d = against_render_plane(&image, &position, 128);
        assert_eq!(d.differing, 0, "rgb: {d:?}");
    }
    // Per-channel display ranges.
    let mut meta = image.meta().clone();
    meta.channels = [(0.0, 255.0), (20.0, 180.0), (0.0, 99.0)]
        .iter()
        .map(|&range| ChannelMeta {
            name: None,
            lut: None,
            display_range: Some(range),
        })
        .collect();
    let ranged = image.with_meta(meta).unwrap();
    let d = against_render_plane(&ranged, &[0], 128);
    assert_eq!(d.differing, 0, "rgb with ranges: {d:?}");
    exact_everywhere("rgb", &ranged, 128);
}

fn composite(dtype: DType, w: usize, h: usize, channels: usize, slices: usize) -> Image {
    let mut meta = ImageMeta::xyczt(dtype, w, h, channels, slices, 1).unwrap();
    meta.color = ColorModel::Composite;
    let primaries: [fn(u8) -> [u8; 3]; 7] = [
        |i| [i, 0, 0],
        |i| [0, i, 0],
        |i| [0, 0, i],
        |i| [i, i, 0],
        |i| [0, i, i],
        |i| [i, 0, i],
        |i| [i, i, i],
    ];
    meta.channels = (0..channels)
        .map(|c| ChannelMeta {
            name: Some(format!("ch{c}")),
            lut: Some(Lut::from_fn(format!("lut{c}"), primaries[c % 7])),
            display_range: match c % 3 {
                0 => Some((0.0, 30000.0)),
                1 => Some((2000.0, 65535.0)),
                // Channel 2 uses its plane's own smallest and largest value.
                _ => None,
            },
        })
        .collect();
    let planes = (0..channels * slices)
        .map(|p| match dtype {
            DType::U16 => u16_plane(w, h, 20 + p as u32),
            _ => u8_plane(w, h, 20 + p as u32),
        })
        .collect();
    Image::from_planes(meta, planes).unwrap()
}

/// The composite as the CPU renderer draws it.
fn composite_reference(image: &Image, position: &[usize], visible: &[bool]) -> Rgba8 {
    let options = DisplayOptions {
        visible: visible.to_vec(),
        ..DisplayOptions::default()
    };
    render_plane_with(image, position, &options).unwrap()
}

#[test]
fn composite_equals_the_cpu_renderer_exactly() {
    let (w, h) = (280, 210);
    let image = composite(DType::U16, w, h, 3, 2);
    for (position, visible) in [
        ([0, 0], [true, true, true]),
        ([1, 1], [true, true, true]),
        ([0, 1], [true, false, true]),
        ([0, 0], [false, true, false]),
        ([0, 0], [false, false, false]),
    ] {
        let mut view = ImageView::new(options(128));
        view.set_position(&position);
        for (c, v) in visible.iter().enumerate() {
            view.set_channel_visible(c, *v);
        }
        let got = view.render_gpu(&image, gpu(), w as u32, h as u32).unwrap();
        let want = composite_reference(&image, &position, &visible);
        let d = diff(&got, &want);
        assert_eq!(d.differing, 0, "composite {position:?} {visible:?}: {d:?}");
        // And the viewer's own CPU path agrees with that reference.
        let cpu = view.render_cpu(&image, w as u32, h as u32).unwrap();
        assert_eq!(diff(&cpu, &want).differing, 0);
    }
    exact_everywhere("composite", &image, 128);

    // The most channels the shader blends.
    let seven = composite(DType::U8, 96, 64, 7, 1);
    let mut view = ImageView::new(options(64));
    let got = view.render_gpu(&seven, gpu(), 96, 64).unwrap();
    let want = composite_reference(&seven, &[0], &[true; 7]);
    assert_eq!(diff(&got, &want).differing, 0);
    assert_eq!(view.stats().cpu_renders, 0);
    // One more is left to the CPU renderer, and still shown exactly.
    let eight = composite(DType::U8, 96, 64, 8, 1);
    let got = view.render_gpu(&eight, gpu(), 96, 64).unwrap();
    let want = composite_reference(&eight, &[0], &[true; 8]);
    assert_eq!(diff(&got, &want).differing, 0);
    assert_eq!(view.stats().cpu_renders, 2);
    no_gpu_errors();
}

#[test]
fn hyperstack_positions_select_planes() {
    let (w, h) = (64, 48);
    let meta = ImageMeta::xyczt(DType::U16, w, h, 2, 3, 4).unwrap();
    let planes = (0..24).map(|p| u16_plane(w, h, 40 + p)).collect();
    let image = Image::from_planes(meta, planes).unwrap();
    for position in [[0, 0, 0], [1, 2, 3], [0, 1, 2], [1, 0, 3]] {
        let d = against_render_plane(&image, &position, 1024);
        assert_eq!(d.differing, 0, "position {position:?}: {d:?}");
    }
    no_gpu_errors();
}

#[test]
fn a_plane_larger_than_one_tile() {
    // 3 x 2 tiles at the default tile size, neither side a multiple of it.
    let (w, h) = (2500, 1300);
    let image = Image::from_planes(
        ImageMeta::plane(DType::U16, w, h).unwrap(),
        vec![u16_plane(w, h, 5)],
    )
    .unwrap();
    let image = with_display(&image, Some((0.0, 60000.0)), Some(fire()));
    let mut view = ImageView::default();
    assert_eq!(view.options().tile_size, 1024);
    // Create the device and the 16-bit pipeline before timing anything.
    let small = Image::from_vec(4, 4, vec![0u16; 16]).unwrap();
    ImageView::default()
        .render_gpu(&small, gpu(), 4, 4)
        .unwrap();

    // Whole plane at 100% against render_plane.
    let start = Instant::now();
    let got = view.render_gpu(&image, gpu(), w as u32, h as u32).unwrap();
    let first = start.elapsed();
    let stats = view.stats();
    let start = Instant::now();
    let again = view.render_gpu(&image, gpu(), w as u32, h as u32).unwrap();
    let second = start.elapsed();
    let start = Instant::now();
    let want = render_plane(&image, &[]).unwrap();
    let cpu = start.elapsed();
    println!(
        "LARGE {w}x{h} u16: first GPU render (read + upload + draw + read back) {first:?}, \
         of which reading tiles {:?} and upload {:?} for {} tiles; redraw + read back {second:?}; CPU render_plane {cpu:?}",
        stats.read_time, stats.upload_time, stats.uploads
    );
    assert_eq!(stats.uploads, 6);
    assert_eq!(stats.tiles, 6);
    assert_eq!(stats.gpu_bytes, w * h * 2);
    assert_eq!(diff(&got, &want).differing, 0);
    assert_eq!(diff(&again, &want).differing, 0);
    // Nothing was read or uploaded again.
    assert_eq!(view.stats().uploads, 6);

    // Views straddling tile boundaries, at 100%, magnified and reduced.
    for l in [
        look(1.0, [1024.0, 1024.0], [700, 500]),
        look(1.0, [2047.6, 650.2], [640, 480]),
        look(2.0, [1024.0, 1024.0], [512, 512]),
        look(1.5, [2048.0, 1023.0], [600, 400]),
        look(0.5, [1250.0, 650.0], [1250, 650]),
        look(0.75, [1300.0, 600.0], [900, 700]),
        look(0.25, [1250.0, 650.0], [640, 360]),
        Look {
            zoom: None,
            center: None,
            size: [777, 555],
        },
    ] {
        let d = against_cpu_view(&mut view, &image, l);
        assert_eq!(d.differing, 0, "large {l:?}: {d:?}");
    }
    no_gpu_errors();
}

#[test]
fn only_visible_tiles_are_read_and_the_budget_evicts() {
    let (w, h) = (2000, 1500);
    let image = Image::from_planes(
        ImageMeta::plane(DType::U8, w, h).unwrap(),
        vec![u8_plane(w, h, 6)],
    )
    .unwrap();
    // 250-sample tiles: 8 x 6 of them. Budget for about four with their
    // GPU copies.
    let mut view = ImageView::new(ViewOptions {
        tile_size: 250,
        cache_bytes: 4 * 2 * 250 * 250,
        ..ViewOptions::default()
    });
    let l = look(1.0, [125.0, 125.0], [200, 200]);
    assert_eq!(against_cpu_view(&mut view, &image, l).differing, 0);
    // One tile covers the view; nothing else was touched.
    assert_eq!(view.stats().tiles_read, 1);
    assert_eq!(view.stats().uploads, 1);
    // Walk across the plane: tiles keep being read, the cache stays small.
    for step in 0..7 {
        let l = look(1.0, [125.0 + 250.0 * step as f64, 125.0], [200, 200]);
        assert_eq!(against_cpu_view(&mut view, &image, l).differing, 0);
    }
    let stats = view.stats();
    assert_eq!(stats.tiles_read, 7);
    assert!(stats.evictions >= 3, "{stats:?}");
    assert!(stats.tiles <= 4, "{stats:?}");
    assert!(stats.sample_bytes + stats.gpu_bytes <= 4 * 2 * 250 * 250);
    // A frame needing more than the budget still draws everything.
    let l = look(1.0, [1000.0, 750.0], [1500, 1000]);
    assert_eq!(against_cpu_view(&mut view, &image, l).differing, 0);
    assert!(view.stats().tiles >= 24);
    assert!(view.stats().tiles_read >= 24 + 7 - 4);
    no_gpu_errors();
}

/// A procedural image that is not in memory: only region reads, counted.
#[derive(Debug)]
struct Procedural {
    meta: ImageMeta,
    largest_read: std::sync::Arc<Mutex<usize>>,
}

impl PixelSource for Procedural {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }
    fn read_plane(&self, plane: usize) -> imager_core::Result<Buffer> {
        self.read_region(
            plane,
            Rect::new(0, 0, self.meta.width(), self.meta.height()),
        )
    }
    fn read_region(&self, plane: usize, r: Rect) -> imager_core::Result<Buffer> {
        let mut largest = self.largest_read.lock().unwrap();
        *largest = (*largest).max(r.w * r.h);
        let w = self.meta.width();
        let mut v = Vec::with_capacity(r.w * r.h);
        for y in r.y..r.y + r.h {
            v.extend((r.x..r.x + r.w).map(|x| pattern(x, y, w, plane as u32, 65536) as u16));
        }
        Ok(Buffer::from_vec(v))
    }
}

#[test]
fn a_lazy_image_is_read_by_region_and_decimated() {
    let (w, h) = (3000, 2200);
    let mut meta = ImageMeta::plane(DType::U16, w, h).unwrap();
    meta.channels = vec![ChannelMeta {
        name: None,
        lut: Some(fire()),
        display_range: Some((0.0, 65535.0)),
    }];
    let largest_read = std::sync::Arc::new(Mutex::new(0));
    let image = Image::new(Procedural {
        meta,
        largest_read: largest_read.clone(),
    });
    let mut largest = 0;
    assert!(!image.is_in_memory());
    let mut view = ImageView::new(options(512));
    for l in [
        look(1.0, [1500.0, 1100.0], [400, 300]),
        look(0.25, [1500.0, 1100.0], [750, 550]),
        look(1.0 / 8.0, [1500.0, 1100.0], [375, 275]),
        look(2.0, [10.0, 10.0], [300, 300]),
    ] {
        apply(&mut view, l);
        let got = view
            .render_gpu(&image, gpu(), l.size[0], l.size[1])
            .unwrap();
        largest = largest.max(*largest_read.lock().unwrap());
        // The tile-free reference reads whole planes, on a separate view.
        let d = {
            let mut reference = ImageView::new(options(512));
            apply(&mut reference, l);
            let memory = image.materialize().unwrap();
            let want = reference.render_cpu(&memory, l.size[0], l.size[1]).unwrap();
            diff(&got, &want)
        };
        assert_eq!(d.differing, 0, "lazy {l:?}: {d:?}");
        *largest_read.lock().unwrap() = 0;
    }
    // The viewer itself never asked for more than one tile's worth.
    assert!(
        largest <= 512 * 512,
        "largest region read: {largest} samples"
    );
    no_gpu_errors();
}

#[test]
fn display_options_follow_the_cpu_renderer() {
    let (w, h) = (150, 110);
    let composite = composite(DType::U16, w, h, 3, 2);
    let mut gray = composite.meta().clone();
    gray.color = ColorModel::Grayscale;
    // Channel 1 loses its table, to exercise the default channel colours.
    gray.channels[1].lut = None;
    let gray = composite.with_meta(gray).unwrap();

    let mut cases = Vec::new();
    for image in [&composite, &gray] {
        for mode in [
            ChannelMode::Auto,
            ChannelMode::Composite,
            ChannelMode::Color,
            ChannelMode::Grayscale,
        ] {
            for blend in [Blend::Sum, Blend::Max, Blend::Min] {
                for visible in [vec![], vec![true, false, true], vec![false; 3]] {
                    cases.push((
                        image,
                        DisplayOptions {
                            mode,
                            visible,
                            blend,
                            ..DisplayOptions::default()
                        },
                    ));
                }
            }
        }
    }
    let (mut shader, mut cpu) = (0, 0);
    for (image, options) in cases {
        let mut view = ImageView::new(self::options(64));
        view.set_position(&[1, 1]);
        view.set_display_options(options.clone());
        let got = view.render_gpu(image, gpu(), w as u32, h as u32).unwrap();
        let want = render_plane_with(image, &[1, 1], &options).unwrap();
        let d = diff(&got, &want);
        assert_eq!(d.differing, 0, "{options:?}: {d:?}");
        if view.stats().cpu_renders == 0 {
            shader += 1;
        } else {
            cpu += 1;
        }
    }
    // Sums and single channels are drawn by the shader, the rest by the CPU.
    println!("DISPLAY OPTIONS: {shader} cases drawn by the shader, {cpu} by the CPU renderer");
    assert!(shader > 0 && cpu > 0);
    no_gpu_errors();
}

#[test]
fn eight_bit_rules_and_the_zero_range() {
    let (w, h) = (128, 96);
    // u8, non-RGB: the byte rule, including ranges with fractions.
    let base = Image::from_planes(
        ImageMeta::plane(DType::U8, w, h).unwrap(),
        vec![u8_plane(w, h, 60)],
    )
    .unwrap();
    for range in [
        (50.0, 200.0),
        (10.4, 99.6),
        (10.5, 99.5),
        (-20.0, 300.0),
        (0.0, 0.0),
        (200.0, 100.0),
    ] {
        let image = with_display(&base, Some(range), Some(fire()));
        let d = against_render_plane(&image, &[], 64);
        assert_eq!(d.differing, 0, "u8 {range:?}: {d:?}");
    }
    // u8 RGB components: the colour rule, which truncates the minimum.
    let rgb = rgb_image(w, h);
    let mut meta = rgb.meta().clone();
    meta.channels = [(40.7, 180.2), (0.0, 0.0), (50.0, 200.0)]
        .iter()
        .map(|&range| ChannelMeta {
            name: None,
            lut: None,
            display_range: Some(range),
        })
        .collect();
    let d = against_render_plane(&rgb.with_meta(meta).unwrap(), &[0], 64);
    assert_eq!(d.differing, 0, "rgb ranges: {d:?}");
    // On 16-bit and float data (0, 0) means the plane's own range.
    let sixteen = Image::from_planes(
        ImageMeta::plane(DType::U16, w, h).unwrap(),
        vec![u16_plane(w, h, 61)],
    )
    .unwrap();
    let d = against_render_plane(&with_display(&sixteen, Some((0.0, 0.0)), None), &[], 64);
    assert_eq!(d.differing, 0, "u16 zero range: {d:?}");
    let float = Image::from_planes(
        ImageMeta::plane(DType::F32, w, h).unwrap(),
        vec![f32_plane(w, h, 62, false)],
    )
    .unwrap();
    let d = against_render_plane(&with_display(&float, Some((0.0, 0.0)), None), &[], 64);
    assert!(d.max <= 1, "f32 zero range: {d:?}");
    no_gpu_errors();
}

#[test]
fn reduced_views_match_nearest_reduction() {
    // Zoomed out, the view shows one sample per device pixel, unaveraged.
    // Where a view is a whole-image reduction it equals the CPU renderer's
    // `Reduction::Nearest`: at any zoom from 50% up, and at exact powers of
    // two below that.
    let (w, h) = (480, 320);
    let image = Image::from_planes(
        ImageMeta::plane(DType::U16, w, h).unwrap(),
        vec![u16_plane(w, h, 70)],
    )
    .unwrap();
    let image = with_display(&image, Some((0.0, 65535.0)), Some(fire()));
    let options = DisplayOptions {
        reduction: Reduction::Nearest,
        ..DisplayOptions::default()
    };
    for (zoom, exact) in [
        (0.75, true),
        (0.5, true),
        (0.625, true),
        (0.25, true),
        (0.125, true),
        // Between powers of two below 50% the nearest kept sample is shown,
        // which is not always the nearest pixel.
        (0.375, false),
    ] {
        let (ow, oh) = ((w as f64 * zoom) as u32, (h as f64 * zoom) as u32);
        let mut view = ImageView::new(self::options(128));
        view.set_zoom(zoom);
        let got = view.render_gpu(&image, gpu(), ow, oh).unwrap();
        let region = Rect::new(0, 0, w, h);
        let want = render_region(&image, &[], region, ow as usize, oh as usize, &options).unwrap();
        let d = diff(&got, &want);
        println!("REDUCED zoom {zoom}: {d:?}");
        if exact {
            assert_eq!(d.differing, 0, "zoom {zoom}: {d:?}");
        }
        // Magnified, both are plain nearest neighbour.
    }
    for zoom in [2.0, 3.0, 1.5] {
        let (ow, oh) = ((w as f64 * zoom) as u32, (h as f64 * zoom) as u32);
        let mut view = ImageView::new(self::options(128));
        view.set_zoom(zoom);
        let got = view.render_gpu(&image, gpu(), ow, oh).unwrap();
        let region = Rect::new(0, 0, w, h);
        let want = render_region(&image, &[], region, ow as usize, oh as usize, &options).unwrap();
        assert_eq!(diff(&got, &want).differing, 0, "zoom {zoom}");
    }
    no_gpu_errors();
}
