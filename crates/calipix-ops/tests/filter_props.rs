//! Laws the filters obey whatever the image: filtering in strips equals
//! filtering in one piece, a constant image stays constant, and a
//! selection gets the samples the whole plane would get (where ImageJ's
//! algorithms allow that).

use calipix_core::{Buffer, Pixel, Rect, Result};
use calipix_ops::filter::{
    Direction, Frame, Kernel, Outliers, Rank, Strip, convolve, despeckle, filter_strips,
    find_edges, gaussian_blur, rank_filter, remove_nans, remove_outliers, shadows, sharpen, smooth,
    top_hat, unsharp_mask,
};
use proptest::prelude::*;

type Run = Box<dyn Fn(&Buffer, Frame) -> Result<Buffer> + Sync>;

/// A filter of one plane, with the number of rows above and below a
/// sample that its result can depend on.
struct Filter {
    name: String,
    halo: usize,
    run: Run,
}

fn filter(
    name: impl Into<String>,
    halo: usize,
    run: impl Fn(&Buffer, Frame) -> Result<Buffer> + Sync + 'static,
) -> Filter {
    Filter {
        name: name.into(),
        halo,
        run: Box::new(run),
    }
}

/// Rows a Gaussian reaches: the rows ImageJ blurs along X beyond the
/// rectangle (`ceil(5 sigma)`), and at least the length of the one-sided
/// kernel, within which of an end of a line ImageJ's loop adds the
/// (nearly zero) weight of the samples beyond that end.
fn gaussian_halo(sigma: f64, accuracy: f64) -> usize {
    let kernel = calipix_ops::filter::gaussian_kernel(sigma, accuracy, 1000);
    ((5.0 * sigma).ceil() as usize).max(kernel[0].len())
}

/// Rows a circular kernel reaches.
fn rank_halo(radius: f64) -> usize {
    let k = calipix_ops::filter::kernel_line_radii(radius);
    (k.len() - 2) / 4
}

const RANKS: [Rank; 5] = [
    Rank::Mean,
    Rank::Minimum,
    Rank::Maximum,
    Rank::Variance,
    Rank::Median,
];

/// Every neighbourhood filter, with a spread of parameters. `float` says
/// whether float-only filters belong.
fn filters(float: bool) -> Vec<Filter> {
    let mut all = Vec::new();
    for sigma in [0.4, 1.0, 2.3] {
        all.push(filter(
            format!("gaussian {sigma}"),
            gaussian_halo(sigma, 0.002),
            move |p, f| gaussian_blur(p, f, sigma, sigma, 0.002),
        ));
        all.push(filter(
            format!("unsharp {sigma}"),
            gaussian_halo(sigma, 0.01),
            move |p, f| unsharp_mask(p, f, sigma, 0.6),
        ));
    }
    all.push(filter(
        "gaussian 1.5 x 0.7",
        gaussian_halo(0.7, 0.0002),
        |p, f| gaussian_blur(p, f, 1.5, 0.7, 0.0002),
    ));
    for radius in [0.5, 1.0, 2.0, 3.5] {
        for rank in RANKS {
            all.push(filter(
                format!("{rank:?} {radius}"),
                rank_halo(radius),
                move |p, f| rank_filter(p, f, radius, rank),
            ));
        }
        for which in [Outliers::Bright, Outliers::Dark] {
            all.push(filter(
                format!("outliers {which:?} {radius}"),
                rank_halo(radius),
                move |p, f| remove_outliers(p, f, radius, 3.0, which, false),
            ));
        }
        for (light, keep) in [(false, false), (true, false), (false, true), (true, true)] {
            all.push(filter(
                format!("top hat {radius} {light} {keep}"),
                2 * rank_halo(radius),
                move |p, f| top_hat(p, f, radius, light, keep, false),
            ));
        }
        if float {
            all.push(filter(
                format!("remove nans {radius}"),
                rank_halo(radius),
                move |p, f| remove_nans(p, f, radius),
            ));
        }
    }
    all.push(filter("despeckle", 1, despeckle));
    for text in [
        "1 2 1; 2 4 2; 1 2 1",
        "1 2 3 2 1",
        "1;-2;4;-2;1",
        "0 1 0 0 2; 1 0 0 3 0; 0 0 -1 0 0",
    ] {
        let kernel = Kernel::parse(text).unwrap();
        all.push(filter(
            format!("convolve {text}"),
            kernel.height() / 2,
            move |p, f| convolve(p, f, &kernel, true),
        ));
    }
    all.push(filter("smooth", 1, smooth));
    all.push(filter("find edges", 1, find_edges));
    for rgb in [false, true] {
        all.push(filter(format!("sharpen rgb={rgb}"), 1, move |p, f| {
            sharpen(p, f, rgb)
        }));
        for direction in Direction::ALL {
            all.push(filter(
                format!("shadows {direction:?} rgb={rgb}"),
                1,
                move |p, f| shadows(p, f, direction, rgb),
            ));
        }
    }
    all
}

/// Runs a whole-plane filter through `filter_strips`.
fn in_strips<T: Pixel>(src: &[T], frame: Frame, rows: usize, filter: &Filter) -> Buffer {
    let out = filter_strips(src, frame, filter.halo, rows, |strip: Strip<'_, T>| {
        let r = strip.frame.rect;
        let filtered = (filter.run)(&Buffer::from_vec(strip.rows.to_vec()), strip.frame)?;
        let filtered = filtered.try_slice::<T>()?;
        Ok((r.y..r.y + r.h)
            .flat_map(|y| {
                let start = y * strip.frame.width + r.x;
                filtered[start..start + r.w].iter().copied()
            })
            .collect())
    });
    Buffer::from_vec(out.unwrap_or_else(|e| panic!("{}: {e}", filter.name)))
}

/// Equal sample for sample, with NaN equal to NaN.
fn same(a: &Buffer, b: &Buffer) -> bool {
    match (a, b) {
        (Buffer::F32(a), Buffer::F32(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(x, y)| x == y || (x.is_nan() && y.is_nan()))
        }
        _ => a == b,
    }
}

/// The index of the first sample of `rect` that differs by more than
/// `tolerance` relative to its size.
fn first_difference(
    a: &Buffer,
    b: &Buffer,
    width: usize,
    rect: Rect,
    tolerance: f64,
) -> Option<usize> {
    (rect.y..rect.y + rect.h)
        .flat_map(|y| (rect.x..rect.x + rect.w).map(move |x| y * width + x))
        .find(|&i| {
            let (x, y) = (a.get_f64(i).unwrap(), b.get_f64(i).unwrap());
            !(x == y || (x.is_nan() && y.is_nan()) || (x - y).abs() <= tolerance * y.abs().max(1.0))
        })
}

/// A plane size, a rectangle within it, and a strip height.
fn layout() -> impl Strategy<Value = (usize, usize, Rect, usize)> {
    (1usize..20, 1usize..20).prop_flat_map(|(w, h)| {
        (0..w, 0..h, 1usize..8).prop_flat_map(move |(x, y, rows)| {
            (1..=w - x, 1..=h - y).prop_map(move |(rw, rh)| (w, h, Rect::new(x, y, rw, rh), rows))
        })
    })
}

/// Float samples with the occasional NaN and infinity.
fn float_sample() -> impl Strategy<Value = f32> {
    prop_oneof![
        20 => -100f32..100.0,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(0f32),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn strips_equal_the_whole_u8(
        (w, h, rect, rows) in layout(),
        seed in proptest::collection::vec(any::<u8>(), 400),
    ) {
        let src: Vec<u8> = seed[..w * h].to_vec();
        let plane = Buffer::from_vec(src.clone());
        let frame = Frame::new(w, h, rect);
        for f in filters(false) {
            let whole = (f.run)(&plane, frame).unwrap();
            let tiled = in_strips(&src, frame, rows, &f);
            prop_assert!(same(&whole, &tiled), "{} {w}x{h} {rect:?} rows {rows}", f.name);
        }
    }

    #[test]
    fn strips_equal_the_whole_u16(
        (w, h, rect, rows) in layout(),
        seed in proptest::collection::vec(any::<u16>(), 400),
    ) {
        let src: Vec<u16> = seed[..w * h].to_vec();
        let plane = Buffer::from_vec(src.clone());
        let frame = Frame::new(w, h, rect);
        for f in filters(false) {
            let whole = (f.run)(&plane, frame).unwrap();
            let tiled = in_strips(&src, frame, rows, &f);
            prop_assert!(same(&whole, &tiled), "{} {w}x{h} {rect:?} rows {rows}", f.name);
        }
    }

    #[test]
    fn strips_equal_the_whole_f32(
        (w, h, rect, rows) in layout(),
        seed in proptest::collection::vec(float_sample(), 400),
    ) {
        let src: Vec<f32> = seed[..w * h].to_vec();
        let plane = Buffer::from_vec(src.clone());
        let frame = Frame::new(w, h, rect);
        for f in filters(true) {
            let whole = (f.run)(&plane, frame).unwrap();
            let tiled = in_strips(&src, frame, rows, &f);
            prop_assert!(same(&whole, &tiled), "{} {w}x{h} {rect:?} rows {rows}", f.name);
        }
    }

    /// The built-in strip height must not matter either: a rectangle gives
    /// what the whole plane gives inside it. Integer results are exact.
    #[test]
    fn a_selection_gets_what_the_whole_plane_gets_u16(
        (w, h, rect, _) in layout(),
        seed in proptest::collection::vec(any::<u16>(), 400),
    ) {
        let plane = Buffer::from_vec(seed[..w * h].to_vec());
        for f in filters(false) {
            // Top hat is included: its first pass covers the rectangle
            // grown by the kernel.
            let whole = (f.run)(&plane, Frame::whole(w, h)).unwrap();
            let part = (f.run)(&plane, Frame::new(w, h, rect)).unwrap();
            prop_assert_eq!(
                first_difference(&whole, &part, w, rect, 0.0), None,
                "{} {}x{} {:?}", f.name, w, h, rect
            );
        }
    }

    /// For float planes the same holds exactly, except that ImageJ's mean
    /// and variance carry running sums from the left edge of the rectangle,
    /// so their last bits depend on where the rectangle starts.
    #[test]
    fn a_selection_gets_what_the_whole_plane_gets_f32(
        (w, h, rect, _) in layout(),
        seed in proptest::collection::vec(-100f32..100.0, 400),
    ) {
        let plane = Buffer::from_vec(seed[..w * h].to_vec());
        for f in filters(true) {
            let whole = (f.run)(&plane, Frame::whole(w, h)).unwrap();
            let part = (f.run)(&plane, Frame::new(w, h, rect)).unwrap();
            let running_sums = f.name.starts_with("Mean") || f.name.starts_with("Variance");
            let tolerance = if running_sums { 1e-3 } else { 0.0 };
            prop_assert_eq!(
                first_difference(&whole, &part, w, rect, tolerance), None,
                "{} {}x{} {:?}", f.name, w, h, rect
            );
        }
    }

    #[test]
    fn samples_outside_the_rectangle_are_returned_unchanged(
        (w, h, rect, _) in layout(),
        seed in proptest::collection::vec(any::<u8>(), 400),
    ) {
        let src = seed[..w * h].to_vec();
        let plane = Buffer::from_vec(src.clone());
        for f in filters(false) {
            let out = (f.run)(&plane, Frame::new(w, h, rect)).unwrap();
            let out = out.as_slice::<u8>().unwrap();
            for (i, (&a, &b)) in src.iter().zip(out).enumerate() {
                let (x, y) = (i % w, i / w);
                let inside = x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
                prop_assert!(inside || a == b, "{} changed ({x}, {y}) outside {rect:?}", f.name);
            }
        }
    }

    #[test]
    fn a_constant_image_stays_constant(
        (w, h, rect, _) in layout(),
        level in any::<u16>(),
    ) {
        let frame = Frame::new(w, h, rect);
        let planes = [
            Buffer::from_vec(vec![level as u8; w * h]),
            Buffer::from_vec(vec![level; w * h]),
            Buffer::from_vec(vec![level as i16; w * h]),
            Buffer::from_vec(vec![level as f32 / 7.0 - 3000.0; w * h]),
            Buffer::from_vec(vec![level as f64 / 7.0 - 3000.0; w * h]),
        ];
        for plane in &planes {
            let float = plane.dtype().is_float();
            for f in filters(float) {
                let out = (f.run)(plane, frame).unwrap();
                // These measure variation, of which a constant image has none.
                let zero = f.name.starts_with("Variance")
                    || f.name == "find edges"
                    || (f.name.starts_with("top hat") && f.name.ends_with("false"));
                if zero {
                    let expected = if f.name.starts_with("top hat") && f.name.contains("true false") && !float {
                        // Light background: the top of the type's range.
                        plane.dtype().integer_range().unwrap().1
                    } else {
                        0.0
                    };
                    // In float, ImageJ's sums of a constant cancel only
                    // to rounding.
                    let slack = if float { 1e-5 * plane.get_f64(0).unwrap().abs() } else { 0.0 };
                    let all = (rect.y..rect.y + rect.h)
                        .flat_map(|y| (rect.x..rect.x + rect.w).map(move |x| y * w + x))
                        .all(|i| (out.get_f64(i).unwrap() - expected).abs() <= slack);
                    prop_assert!(all, "{} of constant {} is not {expected}", f.name, plane.dtype());
                } else {
                    // Float kernels sum to one only to single precision.
                    let tolerance = if float { 1e-5 } else { 0.0 };
                    prop_assert_eq!(
                        first_difference(&out, plane, w, rect, tolerance), None,
                        "{} of constant {} {:?}", f.name, plane.dtype(), plane.get_f64(0)
                    );
                }
            }
        }
    }
}

/// Where a selection does NOT get what the whole plane gets: a Gaussian
/// with sigma above 8.5 is computed on a downscaled grid that starts a
/// kernel radius before the selection, in ImageJ as here.
#[test]
fn a_large_gaussian_depends_on_where_the_selection_starts() {
    let (w, h) = (200, 160);
    let src: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 251) as f32).collect();
    let plane = Buffer::from_vec(src);
    let rect = Rect::new(91, 70, 30, 20);
    let blur =
        |sigma: f64, frame: Frame| gaussian_blur(&plane, frame, sigma, sigma, 0.0002).unwrap();
    let difference = |sigma: f64, tolerance: f64| {
        let whole = blur(sigma, Frame::whole(w, h));
        let part = blur(sigma, Frame::new(w, h, rect));
        first_difference(&whole, &part, w, rect, tolerance)
    };
    assert_eq!(difference(8.5, 0.0), None, "no downscaling up to sigma 8.5");
    assert!(difference(12.0, 0.0).is_some());
    assert_eq!(difference(12.0, 1e-2), None, "but only slightly");
}
