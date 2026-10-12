//! Algebraic laws of the geometric transforms.

use calipix_core::{Buffer, DType, Rect, crop_buffer};
use calipix_ops::transform::{
    Interpolation, canvas, flip_horizontal, flip_vertical, resize, resize_z, rotate, rotate_left,
    rotate_right, scale_in_place, translate,
};
use proptest::prelude::*;

/// A plane of any real pixel type with its width.
fn plane() -> impl Strategy<Value = (Buffer, usize, usize)> {
    (1usize..12, 1usize..12, 0usize..10).prop_flat_map(|(w, h, kind)| {
        proptest::collection::vec(any::<i64>(), w * h).prop_map(move |values| {
            let buffer = match kind {
                0 => Buffer::from_vec(values.iter().map(|&v| v as u8).collect::<Vec<_>>()),
                1 => Buffer::from_vec(values.iter().map(|&v| v as i8).collect::<Vec<_>>()),
                2 => Buffer::from_vec(values.iter().map(|&v| v as u16).collect::<Vec<_>>()),
                3 => Buffer::from_vec(values.iter().map(|&v| v as i16).collect::<Vec<_>>()),
                4 => Buffer::from_vec(values.iter().map(|&v| v as u32).collect::<Vec<_>>()),
                5 => Buffer::from_vec(values.iter().map(|&v| v as i32).collect::<Vec<_>>()),
                6 => Buffer::from_vec(values.iter().map(|&v| v as u64).collect::<Vec<_>>()),
                7 => Buffer::from_vec(values.clone()),
                8 => Buffer::from_vec(
                    values
                        .iter()
                        .map(|&v| (v % 100_000) as f32 / 8.0)
                        .collect::<Vec<_>>(),
                ),
                _ => Buffer::from_vec(
                    values
                        .iter()
                        .map(|&v| (v % 100_000) as f64 / 8.0)
                        .collect::<Vec<_>>(),
                ),
            };
            (buffer, w, h)
        })
    })
}

/// A rectangle inside a `w` x `h` plane.
fn rect_in(w: usize, h: usize) -> impl Strategy<Value = Rect> {
    (0..w, 0..h).prop_flat_map(move |(x, y)| {
        (1..=w - x, 1..=h - y).prop_map(move |(rw, rh)| Rect::new(x, y, rw, rh))
    })
}

fn methods() -> impl Strategy<Value = Interpolation> {
    prop_oneof![
        Just(Interpolation::None),
        Just(Interpolation::Bilinear),
        Just(Interpolation::Bicubic)
    ]
}

proptest! {
    #[test]
    fn flipping_twice_is_the_identity(
        ((buf, w, h), rect) in plane().prop_flat_map(|p| {
            let rect = rect_in(p.1, p.2);
            (Just(p), rect)
        })
    ) {
        let whole = Rect::new(0, 0, w, h);
        for r in [whole, rect] {
            let once = flip_horizontal(&buf, w, r).unwrap();
            prop_assert_eq!(&flip_horizontal(&once, w, r).unwrap(), &buf);
            let once = flip_vertical(&buf, w, r).unwrap();
            prop_assert_eq!(&flip_vertical(&once, w, r).unwrap(), &buf);
        }
        // A flip only moves samples inside its rectangle.
        let flipped = flip_horizontal(&buf, w, rect).unwrap();
        let outside = |b: &Buffer| -> Vec<Option<f64>> {
            (0..w * h)
                .filter(|i| {
                    let (x, y) = (i % w, i / w);
                    !(x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h)
                })
                .map(|i| b.get_f64(i))
                .collect()
        };
        prop_assert_eq!(outside(&flipped), outside(&buf));
    }

    #[test]
    fn quarter_turns_compose((buf, w, h) in plane()) {
        // Four turns one way, and a turn each way, are the identity.
        let mut turned = buf.clone();
        let mut width = w;
        for _ in 0..4 {
            turned = rotate_right(&turned, width).unwrap();
            width = turned.len() / width;
        }
        prop_assert_eq!(&turned, &buf);
        let right = rotate_right(&buf, w).unwrap();
        prop_assert_eq!(&rotate_left(&right, h).unwrap(), &buf);
        // Two turns are both flips.
        let half = rotate_right(&right, h).unwrap();
        let whole = Rect::new(0, 0, w, h);
        let flips = flip_vertical(&flip_horizontal(&buf, w, whole).unwrap(), w, whole).unwrap();
        prop_assert_eq!(half, flips);
    }

    #[test]
    fn cropping_a_crop_is_one_crop(
        ((buf, w, _h), outer, (dx, dy, fw, fh)) in plane().prop_flat_map(|p| {
            let outer = rect_in(p.1, p.2);
            (Just(p), outer, (0.0..1.0f64, 0.0..1.0f64, 0.0..1.0f64, 0.0..1.0f64))
        })
    ) {
        // A rectangle inside the outer one, from four fractions.
        let x = (dx * outer.w as f64) as usize % outer.w;
        let y = (dy * outer.h as f64) as usize % outer.h;
        let inner = Rect::new(
            x,
            y,
            1 + (fw * (outer.w - x - 1) as f64) as usize,
            1 + (fh * (outer.h - y - 1) as f64) as usize,
        );
        let twice = crop_buffer(&crop_buffer(&buf, w, outer).unwrap(), outer.w, inner).unwrap();
        let direct = Rect::new(outer.x + inner.x, outer.y + inner.y, inner.w, inner.h);
        prop_assert_eq!(twice, crop_buffer(&buf, w, direct).unwrap());
    }

    #[test]
    fn scaling_by_one_is_the_identity(
        ((buf, w, h), rect, method, average) in plane().prop_flat_map(|p| {
            let rect = rect_in(p.1, p.2);
            (Just(p), rect, methods(), any::<bool>())
        })
    ) {
        // To the same size: a copy of the rectangle, whatever the method.
        let same = resize(&buf, w, rect, rect.w, rect.h, method, average).unwrap();
        prop_assert_eq!(same, crop_buffer(&buf, w, rect).unwrap());
        // Without interpolation, scaling in place by 1 changes nothing,
        // except that ImageJ reads the bottom row of the plane from the row
        // above it, which Calipix reproduces.
        let scaled = scale_in_place(&buf, w, rect, 1.0, 1.0, Interpolation::None, 0.0).unwrap();
        let rows = if rect.y + rect.h == h && h > 1 { h - 1 } else { h };
        for i in 0..rows * w {
            prop_assert_eq!(scaled.get_f64(i), buf.get_f64(i));
        }
        // Nor does a stack scaled in Z to its own depth.
        let stack = vec![buf.clone(), flip_horizontal(&buf, w, Rect::new(0, 0, w, h)).unwrap()];
        prop_assert_eq!(&resize_z(&stack, w, 2, method, average).unwrap(), &stack);
    }

    #[test]
    fn whole_translations_undo_each_other_inside_the_plane(
        (buf, w, h) in plane(),
        dx in -3i32..4,
        dy in -3i32..4,
        method in methods(),
    ) {
        let whole = Rect::new(0, 0, w, h);
        let moved = translate(&buf, w, whole, dx as f64, dy as f64, method).unwrap();
        let back = translate(&moved, w, whole, -dx as f64, -dy as f64, method).unwrap();
        // Samples that stayed inside the plane come back; the rest are 0.
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let i = (y as usize) * w + x as usize;
                let stayed = x + dx >= 0 && x + dx < w as i32 && y + dy >= 0 && y + dy < h as i32;
                let expected = if stayed { buf.get_f64(i) } else { Some(0.0) };
                prop_assert_eq!(back.get_f64(i), expected);
            }
        }
        prop_assert_eq!(&translate(&buf, w, whole, 0.0, 0.0, method).unwrap(), &buf);
    }

    #[test]
    fn a_canvas_round_trip_returns_the_plane(
        (buf, w, h) in plane(),
        left in 0usize..4,
        top in 0usize..4,
        right in 0usize..4,
        bottom in 0usize..4,
    ) {
        let (big_w, big_h) = (w + left + right, h + top + bottom);
        let big = canvas(&buf, w, big_w, big_h, left as i64, top as i64, 0.0).unwrap();
        prop_assert_eq!(big.len(), big_w * big_h);
        // Cutting the border off again is the same as cropping it.
        let back = canvas(&big, big_w, w, h, -(left as i64), -(top as i64), 0.0).unwrap();
        prop_assert_eq!(&back, &buf);
        prop_assert_eq!(&crop_buffer(&big, big_w, Rect::new(left, top, w, h)).unwrap(), &buf);
    }

    #[test]
    fn rotation_without_interpolation_only_rearranges_samples(
        (buf, w, h) in plane(),
        angle in -180.0f64..180.0,
    ) {
        let whole = Rect::new(0, 0, w, h);
        let turned = rotate(&buf, w, whole, angle, Interpolation::None, 0.0).unwrap();
        prop_assert_eq!(turned.dtype(), buf.dtype());
        let source: Vec<Option<f64>> = (0..w * h).map(|i| buf.get_f64(i)).collect();
        for i in 0..w * h {
            let v = turned.get_f64(i);
            prop_assert!(v == Some(0.0) || source.contains(&v));
        }
    }
}

#[test]
fn transforms_refuse_complex_planes_and_bad_shapes() {
    let complex = Buffer::zeros(DType::C32, 4);
    let whole = Rect::new(0, 0, 2, 2);
    assert!(rotate_right(&complex, 2).is_ok(), "moving samples needs no arithmetic");
    assert!(flip_horizontal(&complex, 2, whole).is_ok());
    assert!(rotate(&complex, 2, whole, 10.0, Interpolation::None, 0.0).is_err());
    assert!(resize(&complex, 2, whole, 4, 4, Interpolation::None, false).is_err());
    assert!(canvas(&complex, 2, 4, 4, 0, 0, 0.0).is_err());

    let plane = Buffer::from_vec(vec![1u8, 2, 3, 4, 5, 6]);
    assert!(flip_horizontal(&plane, 4, whole).is_err(), "width does not divide the plane");
    assert!(flip_vertical(&plane, 3, Rect::new(2, 0, 2, 2)).is_err(), "rectangle outside");
    assert!(resize(&plane, 3, Rect::new(0, 0, 3, 2), 0, 2, Interpolation::None, false).is_err());
    assert!(canvas(&plane, 3, 0, 2, 0, 0, 0.0).is_err());
    assert!(scale_in_place(&plane, 3, Rect::new(0, 0, 3, 2), 0.0, 1.0, Interpolation::None, 0.0).is_err());
    assert!(resize_z(&[], 3, 2, Interpolation::None, false).is_err());
}

#[test]
fn degenerate_planes_do_not_panic() {
    // One sample wide or high: ImageJ cannot interpolate these at all.
    for (w, h) in [(1usize, 5usize), (5, 1), (1, 1)] {
        let plane = Buffer::from_vec((0..(w * h) as u16).collect::<Vec<_>>());
        let whole = Rect::new(0, 0, w, h);
        for method in Interpolation::ALL {
            rotate(&plane, w, whole, 33.0, method, 0.0).unwrap();
            translate(&plane, w, whole, 0.5, 0.5, method).unwrap();
            scale_in_place(&plane, w, whole, 0.5, 0.5, method, 0.0).unwrap();
            scale_in_place(&plane, w, whole, 2.0, 2.0, method, 0.0).unwrap();
            for average in [false, true] {
                let out = resize(&plane, w, whole, 3, 4, method, average).unwrap();
                assert_eq!(out.len(), 12);
                let out = resize(&plane, w, whole, 1, 1, method, average).unwrap();
                assert_eq!(out.len(), 1);
            }
        }
    }
}
