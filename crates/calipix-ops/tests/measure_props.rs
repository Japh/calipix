//! Laws that measurements obey whatever the image.

use calipix_core::{Buffer, Rect};
use calipix_ops::measure::{
    Calibration, HistogramSpec, MeasureSettings, Measured, Measurements, Outline, PlaneContext,
    Region, Samples, Statistics, measure, outline_of, statistics,
};
use calipix_roi::Roi;
use proptest::prelude::*;

/// A plane of one of the pixel types ImageJ has, from the same numbers.
fn plane(kind: u8, values: &[u16]) -> Buffer {
    match kind % 3 {
        0 => Buffer::from_vec(values.iter().map(|&v| (v % 256) as u8).collect::<Vec<u8>>()),
        1 => Buffer::from_vec(values.to_vec()),
        _ => Buffer::from_vec(
            values
                .iter()
                .map(|&v| v as f32 / 7.0 - 300.0)
                .collect::<Vec<f32>>(),
        ),
    }
}

fn stats(
    plane: &Buffer,
    width: usize,
    height: usize,
    region: Region<'_>,
    cal: &Calibration,
    what: Measurements,
) -> Statistics {
    statistics(
        Samples::Gray(plane),
        width,
        height,
        region,
        cal,
        what,
        HistogramSpec::default(),
    )
    .unwrap()
}

fn near(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// A plane size, its samples and, per pixel, which of two disjoint masks
/// (or neither) it belongs to.
fn masked_plane() -> impl Strategy<Value = (usize, usize, Vec<u16>, Vec<u8>)> {
    (1usize..12, 1usize..12).prop_flat_map(|(w, h)| {
        (
            Just(w),
            Just(h),
            proptest::collection::vec(any::<u16>(), w * h),
            proptest::collection::vec(0u8..3, w * h),
        )
    })
}

proptest! {
    /// Area, pixel count and raw integrated density of two disjoint masks
    /// add up to those of their union; the extremes are the extremes.
    #[test]
    fn disjoint_masks_combine(
        (w, h, values, owner) in masked_plane(),
        kind in 0u8..3,
        pw in 0.1f64..4.0,
        ph in 0.1f64..4.0,
    ) {
        let a: Vec<u8> = owner.iter().map(|&o| if o == 1 { 255 } else { 0 }).collect();
        let b: Vec<u8> = owner.iter().map(|&o| if o == 2 { 255 } else { 0 }).collect();
        let both: Vec<u8> = owner.iter().map(|&o| if o != 0 { 255 } else { 0 }).collect();
        prop_assume!(a.contains(&255) && b.contains(&255));
        let plane = plane(kind, &values);
        let cal = Calibration { pixel_width: pw, pixel_height: ph, ..Calibration::default() };
        let rect = Rect::new(0, 0, w, h);
        let of = |mask: &[u8]| {
            stats(&plane, w, h, Region { rect, mask: Some(mask) }, &cal, Measurements::default())
        };
        let (sa, sb, su) = (of(&a), of(&b), of(&both));
        prop_assert_eq!(su.pixel_count, sa.pixel_count + sb.pixel_count);
        prop_assert!(near(su.area, sa.area + sb.area));
        let raw = |s: &Statistics| s.pixel_count as f64 * s.raw_mean;
        prop_assert!(near(raw(&su), raw(&sa) + raw(&sb)), "{} {} {}", raw(&su), raw(&sa), raw(&sb));
        prop_assert_eq!(su.min, sa.min.min(sb.min));
        prop_assert_eq!(su.max, sa.max.max(sb.max));
        let total: u64 = su.histogram.iter().sum();
        prop_assert_eq!(total, su.pixel_count);
    }

    /// Moving a selection together with the samples under it moves the
    /// centroid and centre of mass by the same amount and changes nothing
    /// that describes the samples or the shape.
    #[test]
    fn translation_moves_only_positions(
        (w, h, values, owner) in masked_plane(),
        kind in 0u8..3,
        (x0, y0, dx, dy) in (0usize..5, 0usize..5, 0usize..6, 0usize..6),
        ellipse in any::<bool>(),
        pw in 0.1f64..4.0,
    ) {
        let mask: Vec<u8> = owner.iter().map(|&o| if o != 0 { 255 } else { 0 }).collect();
        prop_assume!(mask.contains(&255));
        let (cw, ch) = (w + 11, h + 11);
        let canvas = |x: usize, y: usize| {
            let mut all = vec![9u16; cw * ch];
            for row in 0..h {
                for column in 0..w {
                    all[(y + row) * cw + x + column] = values[row * w + column];
                }
            }
            plane(kind, &all)
        };
        let cal = Calibration { pixel_width: pw, pixel_height: pw, ..Calibration::default() };
        let what = Measurements { fit_ellipse: ellipse, ..Measurements::default() };
        let at = |x: usize, y: usize| {
            let region = Region { rect: Rect::new(x, y, w, h), mask: Some(&mask) };
            stats(&canvas(x, y), cw, ch, region, &cal, what)
        };
        let (s, t) = (at(x0, y0), at(x0 + dx, y0 + dy));
        let (mx, my) = (dx as f64 * pw, dy as f64 * pw);
        prop_assert!(near(t.x_centroid, s.x_centroid + mx));
        prop_assert!(near(t.y_centroid, s.y_centroid + my));
        prop_assert!(near(t.roi_x, s.roi_x + mx) && near(t.roi_y, s.roi_y + my));
        // A centre of mass exists only where the samples do not sum to zero.
        if s.mean.abs() > 1.0 {
            let slack = 1e-6 * (1.0 + s.x_center_of_mass.abs() + s.y_center_of_mass.abs());
            prop_assert!((t.x_center_of_mass - s.x_center_of_mass - mx).abs() < slack + 1e-6 * mx);
            prop_assert!((t.y_center_of_mass - s.y_center_of_mass - my).abs() < slack + 1e-6 * my);
        }
        prop_assert_eq!(t.pixel_count, s.pixel_count);
        prop_assert_eq!(&t.histogram, &s.histogram);
        for (a, b) in [
            (t.area, s.area), (t.mean, s.mean), (t.std_dev, s.std_dev), (t.mode, s.mode),
            (t.min, s.min), (t.max, s.max), (t.median, s.median), (t.skewness, s.skewness),
            (t.kurtosis, s.kurtosis), (t.area_fraction, s.area_fraction),
            (t.roi_width, s.roi_width), (t.roi_height, s.roi_height),
            (t.major, s.major), (t.minor, s.minor), (t.angle, s.angle),
        ] {
            prop_assert!(a == b || (a.is_nan() && b.is_nan()), "{a} != {b}");
        }
    }

    /// A rectangle covering the whole image measures as no selection does.
    #[test]
    fn full_rectangle_equals_no_selection(
        (w, h, values, _) in masked_plane(),
        kind in 0u8..3,
        all in any::<bool>(),
    ) {
        let plane = plane(kind, &values);
        let cal = Calibration::default();
        let what = if all { Measurements::all() } else { Measurements::imagej_default() };
        let whole = stats(&plane, w, h, Region::whole(w, h), &cal, what);
        let rect = stats(&plane, w, h, Region::rect(Rect::new(0, 0, w, h)), &cal, what);
        prop_assert_eq!(format!("{:?}", whole), format!("{:?}", rect));

        // The same through Analyze > Measure: every column agrees.
        let context = PlaneContext::single("image");
        let image = Measured {
            samples: Samples::Gray(&plane),
            width: w,
            height: h,
            calibration: &cal,
            context: &context,
        };
        let settings = MeasureSettings { measurements: what, ..MeasureSettings::default() };
        let none = measure(image, Region::whole(w, h), &Outline::whole(w, h), &settings).unwrap();
        let selected = measure(
            image,
            Region::rect(Rect::new(0, 0, w, h)),
            &outline_of(&Roi::rectangle(0.0, 0.0, w as f64, h as f64), &cal).unwrap(),
            &settings,
        )
        .unwrap();
        prop_assert_eq!(format!("{:?}", none), format!("{:?}", selected));

        // A mask that selects everything gives the same samples too; only
        // the ellipse is then fitted by moments instead of by formula.
        let ones = vec![255u8; w * h];
        let masked = stats(
            &plane,
            w,
            h,
            Region { rect: Rect::new(0, 0, w, h), mask: Some(&ones) },
            &cal,
            Measurements::default(),
        );
        let plain = stats(&plane, w, h, Region::whole(w, h), &cal, Measurements::default());
        prop_assert_eq!(format!("{:?}", masked), format!("{:?}", plain));
    }
}
