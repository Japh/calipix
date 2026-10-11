//! The CPU fallback path (tiles rendered by `calipix-render`, assembled on
//! the CPU) compared with the tile-free reference. Needs no GPU.

use calipix_core::{Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Lut, RealPixel};
use calipix_render::render_plane;
use calipix_view::{ImageView, ViewOptions};

fn plane<T: RealPixel>(w: usize, h: usize, seed: usize, span: f64, offset: f64) -> Buffer {
    Buffer::from_vec(
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let v = ((x * 37 + y * 101 + seed * 977) % 4096) as f64 / 4096.0;
                T::from_f64(v * span + offset)
            })
            .collect::<Vec<T>>(),
    )
}

fn views() -> Vec<(f64, [f64; 2], [u32; 2])> {
    vec![
        (1.0, [100.0, 80.0], [260, 190]),
        (1.0, [40.0, 130.0], [120, 90]),
        (1.5, [111.0, 70.0], [200, 150]),
        (4.0, [20.5, 20.25], [160, 120]),
        (0.75, [100.0, 80.0], [200, 150]),
        (0.5, [100.0, 80.0], [150, 110]),
        (1.0 / 3.0, [100.0, 80.0], [90, 70]),
        (0.2, [100.0, 80.0], [64, 48]),
    ]
}

fn check(name: &str, image: &Image, position: &[usize]) {
    let meta = image.meta();
    let (w, h) = (meta.width() as u32, meta.height() as u32);
    let options = ViewOptions {
        tile_size: 64,
        ..ViewOptions::default()
    };
    // Whole image at 100%: exactly render_plane.
    {
        let mut view = ImageView::new(options.clone());
        view.set_position(position);
        let got = view.render_cpu_tiled(image, w, h).unwrap();
        let want = render_plane(image, position).unwrap();
        assert!(got == want, "{name}: whole image");
    }
    let mut view = ImageView::new(options);
    view.set_position(position);
    for (zoom, center, size) in views() {
        view.set_zoom(zoom);
        view.set_center(center);
        let got = view.render_cpu_tiled(image, size[0], size[1]).unwrap();
        let want = view.render_cpu(image, size[0], size[1]).unwrap();
        assert!(
            got == want,
            "{name}: zoom {zoom} centre {center:?} size {size:?}"
        );
    }
    view.fit();
    let got = view.render_cpu_tiled(image, 173, 97).unwrap();
    let want = view.render_cpu(image, 173, 97).unwrap();
    assert!(got == want, "{name}: fit");
}

#[test]
fn every_real_pixel_type_is_drawn_as_the_reference_draws_it() {
    let (w, h) = (200, 160);
    fn single<T: RealPixel>(w: usize, h: usize, span: f64, offset: f64) -> Image {
        Image::from_planes(
            ImageMeta::plane(T::DTYPE, w, h).unwrap(),
            vec![plane::<T>(w, h, 1, span, offset)],
        )
        .unwrap()
    }
    check("u8", &single::<u8>(w, h, 255.0, 0.0), &[]);
    check("i8", &single::<i8>(w, h, 255.0, -128.0), &[]);
    check("u16", &single::<u16>(w, h, 65535.0, 0.0), &[]);
    check("i16", &single::<i16>(w, h, 65535.0, -32768.0), &[]);
    check("u32", &single::<u32>(w, h, 4e9, 0.0), &[]);
    check("i32", &single::<i32>(w, h, 4e9, -2e9), &[]);
    check("u64", &single::<u64>(w, h, 1e12, 0.0), &[]);
    check("i64", &single::<i64>(w, h, 1e12, -5e11), &[]);
    check("f32", &single::<f32>(w, h, 3.5, -1.25), &[]);
    check("f64", &single::<f64>(w, h, 1e-3, 100.0), &[]);
}

#[test]
fn display_ranges_luts_rgb_and_composites() {
    let (w, h) = (200, 160);
    // An inverted range and a colour table on 32-bit data.
    let mut meta = ImageMeta::plane(DType::I32, w, h).unwrap();
    meta.channels = vec![ChannelMeta {
        name: None,
        lut: Some(Lut::from_fn("ramp", |i| [i, 255 - i, i / 2])),
        display_range: Some((-1000.0, 250_000.0)),
    }];
    let image = Image::from_planes(meta, vec![plane::<i32>(w, h, 2, 5e5, -1e5)]).unwrap();
    check("i32 ranged", &image, &[]);

    let mut meta = ImageMeta::xyczt(DType::U8, w, h, 3, 2, 1).unwrap();
    meta.color = ColorModel::Rgb;
    let planes = (0..6).map(|p| plane::<u8>(w, h, p, 255.0, 0.0)).collect();
    let rgb = Image::from_planes(meta, planes).unwrap();
    check("rgb", &rgb, &[0, 1]);

    // More channels than the shader blends, in a type it cannot read.
    let mut meta = ImageMeta::xyczt(DType::F64, w, h, 9, 1, 1).unwrap();
    meta.color = ColorModel::Composite;
    meta.channels = (0..9)
        .map(|c| ChannelMeta {
            name: None,
            lut: Some(Lut::from_fn("part", move |i| {
                let mut rgb = [0; 3];
                rgb[c % 3] = i / 2;
                rgb
            })),
            display_range: (c % 2 == 0).then_some((0.0, 1.0)),
        })
        .collect();
    let planes = (0..9).map(|p| plane::<f64>(w, h, p, 1.0, 0.0)).collect();
    let composite = Image::from_planes(meta, planes).unwrap();
    check("nine channels", &composite, &[0]);
}

#[test]
fn complex_images_report_an_error() {
    let image = Image::from_vec(4, 4, vec![calipix_core::Complex32::new(1.0, 2.0); 16]).unwrap();
    let mut view = ImageView::default();
    let error = view.render_cpu_tiled(&image, 8, 8).unwrap_err().to_string();
    assert!(error.contains("complex"), "{error}");
}
