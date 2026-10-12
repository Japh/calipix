//! Laws that hold for every selection: its text form and its ImageJ file
//! form read back to the same selection, and nothing a file or a user can
//! supply makes the crate panic.

use calipix_roi::{
    Color, Combine, Path, Point, Roi, RoiEntry, RoiProps, Segment, decode, encode, read_zip,
    write_zip,
};
use proptest::collection::vec;
use proptest::prelude::*;

/// Coordinates that single precision holds exactly, sums included:
/// quarters between -1000 and 2000.
fn quarter() -> BoxedStrategy<f64> {
    (-4000i32..8000).prop_map(|k| f64::from(k) / 4.0).boxed()
}

fn whole() -> BoxedStrategy<f64> {
    (-1000i32..2000).prop_map(f64::from).boxed()
}

/// Positive sizes in quarters.
fn size() -> impl Strategy<Value = f64> {
    (1i32..4000).prop_map(|k| f64::from(k) / 4.0)
}

fn points(coord: BoxedStrategy<f64>, min: usize) -> impl Strategy<Value = Vec<Point>> {
    vec((coord.clone(), coord), min..12)
}

fn line_width() -> impl Strategy<Value = f64> {
    prop_oneof![Just(1.0), Just(2.0), Just(3.5), Just(10.0)]
}

fn path(coord: BoxedStrategy<f32>) -> impl Strategy<Value = Path> {
    let p = (coord.clone(), coord.clone());
    let step = prop_oneof![
        p.clone().prop_map(|(x, y)| Segment::LineTo(x, y)),
        p.clone().prop_map(|(x, y)| Segment::LineTo(x, y)),
        (p.clone(), p.clone()).prop_map(|((a, b), (x, y))| Segment::QuadTo(a, b, x, y)),
        (p.clone(), p.clone(), p.clone())
            .prop_map(|((a, b), (c, d), (x, y))| Segment::CubicTo(a, b, c, d, x, y)),
    ];
    let sub_path = (p, vec(step, 1..6), any::<bool>()).prop_map(|((x, y), steps, close)| {
        let mut segments = vec![Segment::MoveTo(x, y)];
        segments.extend(steps);
        if close {
            segments.push(Segment::Close);
        }
        segments
    });
    vec(sub_path, 1..4).prop_map(|parts| Path::new(parts.concat()))
}

/// Selections whose every number is given by `coord`, valid in the sense
/// of the text form: enough points, positive sizes.
fn roi(coord: BoxedStrategy<f64>) -> impl Strategy<Value = Roi> {
    let c = coord.clone();
    let c32 = coord
        .clone()
        .prop_map(|v| v.clamp(-3.0e38, 3.0e38) as f32)
        .boxed();
    prop_oneof![
        (c.clone(), c.clone(), size(), size()).prop_map(|(x, y, w, h)| Roi::rectangle(x, y, w, h)),
        (c.clone(), c.clone(), size(), size(), 1u32..2000)
            .prop_map(|(x, y, w, h, d)| Roi::rounded_rectangle(x, y, w, h, d)),
        (c.clone(), c.clone(), size(), size()).prop_map(|(x, y, w, h)| Roi::oval(x, y, w, h)),
        points(c.clone(), 3).prop_map(|points| Roi::Polygon { points }),
        points(c.clone(), 3).prop_map(|points| Roi::Freehand { points }),
        points(c.clone(), 3).prop_map(|points| Roi::Traced { points }),
        (c.clone(), c.clone(), c.clone(), c.clone(), line_width()).prop_map(
            |(x1, y1, x2, y2, width)| Roi::Line {
                x1,
                y1,
                x2,
                y2,
                width
            }
        ),
        (points(c.clone(), 2), line_width())
            .prop_map(|(points, width)| Roi::Polyline { points, width }),
        (points(c.clone(), 2), line_width())
            .prop_map(|(points, width)| Roi::Freeline { points, width }),
        points(c.clone(), 3).prop_map(|p| Roi::Angle {
            points: [p[0], p[1], p[2]]
        }),
        points(c.clone(), 1).prop_map(Roi::points),
        (c.clone(), c.clone(), c.clone(), c.clone(), 1u32..=4)
            .prop_filter("the axis has two ends", |(x1, y1, x2, y2, _)| (x1, y1) != (x2, y2))
            .prop_map(|(x1, y1, x2, y2, r)| Roi::ellipse(x1, y1, x2, y2, f64::from(r) / 4.0)),
        (c.clone(), c.clone(), c.clone(), c.clone(), size())
            .prop_filter("the axis has two ends", |(x1, y1, x2, y2, _)| (x1, y1) != (x2, y2))
            .prop_map(|(x1, y1, x2, y2, w)| Roi::rotated_rectangle(x1, y1, x2, y2, w)),
        path(c32).prop_map(Roi::composite),
    ]
}

fn props() -> impl Strategy<Value = RoiProps> {
    (
        proptest::option::of("[a-zA-Z0-9 \u{e9}\u{3b1}\u{4e2d}-]{1,12}"),
        (0u32..40).prop_map(|k| k as f32 / 4.0),
        proptest::option::of((1u32..=u32::MAX).prop_map(Color)),
        proptest::option::of((1u32..=u32::MAX).prop_map(Color)),
        (0i32..100, 0i32..5, 0i32..50, 0i32..20),
        0u16..=255,
        proptest::option::of("[a-z]{1,8}: [a-z0-9 ]{0,10}\n"),
    )
        .prop_map(|(name, stroke_width, stroke, fill, position, group, properties)| {
            let mut props = RoiProps::default();
            props.name = name;
            props.stroke_width = stroke_width;
            props.stroke_color = stroke;
            props.fill_color = fill;
            (props.position, props.channel, props.slice, props.frame) = position;
            props.group = group;
            props.properties = properties;
            props
        })
}

/// What the file format keeps of an entry: a line's width is its stroke
/// width, points have none.
fn storable(mut entry: RoiEntry) -> RoiEntry {
    let has_width = matches!(
        entry.roi,
        Roi::Line { .. } | Roi::Polyline { .. } | Roi::Freeline { .. }
    );
    let wide = entry.roi.line_width() > 1.0 || entry.props.stroke_width > 1.0;
    if entry.roi.is_points() || (has_width && wide) {
        entry.props.stroke_width = 0.0;
    }
    entry
}

fn numbers(roi: &Roi) -> Vec<f64> {
    roi.to_string()
        .split([' ', ','])
        .filter_map(|w| w.parse().ok())
        .collect()
}

proptest! {
    #[test]
    fn text_form_round_trips(roi in roi(prop_oneof![
        quarter(),
        any::<f64>().prop_filter("finite", |v| v.is_finite()),
        any::<f32>().prop_filter("finite", |v| v.is_finite()).prop_map(f64::from),
    ].boxed())) {
        let text = roi.to_string();
        let back: Roi = text.parse().map_err(|e| TestCaseError::fail(format!("{text}: {e}")))?;
        prop_assert_eq!(&back, &roi);
        prop_assert_eq!(back.to_string(), text);
    }

    #[test]
    fn roi_files_round_trip(roi in roi(quarter()), props in props()) {
        let entry = storable(RoiEntry { roi, props });
        let bytes = encode(&entry).unwrap();
        let back = decode(&bytes).map_err(|e| TestCaseError::fail(e.to_string()))?;
        match &entry.roi {
            // ImageJ stores an ellipse or rotated rectangle by the ends of
            // its axis as read back from its single-precision vertices,
            // so those come back to within rounding.
            Roi::Ellipse { .. } | Roi::RotatedRectangle { .. } => {
                prop_assert_eq!(back.roi.kind(), entry.roi.kind());
                for (a, b) in numbers(&back.roi).iter().zip(numbers(&entry.roi)) {
                    prop_assert!((a - b).abs() < 2e-3, "{} vs {}", back.roi, entry.roi);
                }
                prop_assert_eq!(&back.props, &entry.props);
            }
            _ => {
                prop_assert_eq!(&back, &entry);
                // Writing what was read gives the same bytes again.
                prop_assert_eq!(&encode(&back).unwrap(), &bytes);
            }
        }
    }

    #[test]
    fn whole_number_selections_are_stored_as_whole_numbers(roi in roi(whole())) {
        if let Roi::Rectangle { width, height, .. }
        | Roi::RoundedRectangle { width, height, .. }
        | Roi::Oval { width, height, .. } = &roi
        {
            prop_assume!(width.fract() == 0.0 && height.fract() == 0.0);
        }
        let entry = RoiEntry::new(roi);
        let back = decode(&encode(&entry).unwrap()).unwrap();
        prop_assert!(!back.props.sub_pixel);
        // The sub-pixel flag of a file ImageJ wrote survives too.
        let mut flagged = entry.clone();
        flagged.props.sub_pixel = true;
        let back = decode(&encode(&flagged).unwrap()).unwrap();
        let has_flag = !matches!(
            entry.roi,
            Roi::Line { .. } | Roi::Points { .. } | Roi::Ellipse { .. }
                | Roi::RotatedRectangle { .. } | Roi::Composite { .. }
        );
        prop_assert_eq!(back.props.sub_pixel, has_flag);
        prop_assert_eq!(back.roi.mask(64, 48), entry.roi.mask(64, 48));
    }

    #[test]
    fn roi_sets_round_trip(entries in vec((roi(quarter()), props()), 0..6)) {
        let entries: Vec<RoiEntry> = entries
            .into_iter()
            .enumerate()
            .map(|(i, (roi, mut props))| {
                props.name = Some(format!("{}-{i}", props.name.unwrap_or_default()));
                storable(RoiEntry { roi, props })
            })
            .filter(|e| !matches!(e.roi, Roi::Ellipse { .. } | Roi::RotatedRectangle { .. }))
            .collect();
        let bytes = write_zip(&entries).unwrap();
        if entries.is_empty() {
            prop_assert!(read_zip(&bytes).is_err());
        } else {
            let set = read_zip(&bytes).map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert!(set.skipped.is_empty());
            prop_assert_eq!(set.entries, entries);
        }
    }
}

/// Numbers of every size a file or a user could supply.
fn wild() -> BoxedStrategy<f64> {
    prop_oneof![
        4 => quarter().prop_map(|v| v / 8.0),
        1 => -2.0e9..2.0e9f64,
        1 => prop_oneof![Just(1e12), Just(-1e12), Just(3e38), Just(-3e38), Just(1e300), Just(-1e300)],
        1 => prop_oneof![Just(1e-30), Just(-0.0), Just(2147483647.0), Just(-2147483648.0), Just(65535.5)],
    ]
    .boxed()
}

/// Calls everything that works on a selection alone.
fn exercise(roi: &Roi) {
    let _ = roi.mask(64, 48);
    let _ = roi.pixel_bounds(64, 48);
    let _ = (roi.bounds(), roi.int_bounds(), roi.ij_type(), roi.vertices());
    let _ = (roi.length(), roi.length_scaled(0.5, 0.25));
    // The hull and calipers compare every pair of vertices; outlines
    // with thousands of them (curves a billion pixels long) are left
    // out to keep the test quick.
    if roi.outline().len() < 2000 {
        let _ = (roi.int_outline(), roi.convex_hull(), roi.convex_hull_roi());
        let _ = (roi.feret(), roi.feret_scaled(0.5, 0.25));
    }
    let _ = (roi.contour_centroid(), roi.angle(), roi.rotation_center());
    let _ = (roi.contains(3, 4), roi.contains(-1, 1_000_000));
    let _ = (roi.line_pixels(), roi.profile_positions(), roi.drawn_pixels());
    let _ = roi.to_string();
    let _ = encode(&RoiEntry::new(roi.clone()));
    let _ = (roi.ellipse_fit(64, 48), roi.fitted_ellipse(64, 48));
    let _ = (roi.scale_center(64, 48), roi.bounding_box());
    let _ = roi.inverse(64, 48);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn no_numbers_make_geometry_panic(roi in roi(wild()), scale in wild(), angle in wild()) {
        exercise(&roi);
        for made in [
            roi.to_area(),
            roi.enlarged(2),
            roi.enlarged(-1),
            roi.combined(&Roi::rectangle(1.0, 2.0, 30.0, 20.0), Combine::Xor),
            Some(roi.scaled(scale, 0.5, None)),
            Some(roi.scaled(0.5, scale, Some((10.0, 10.0)))),
            Some(roi.rotated(angle, 20.0, 20.0)),
            Some(roi.translated(scale, angle)),
        ]
        .into_iter()
        .flatten()
        {
            exercise(&made);
        }
    }

    #[test]
    fn no_bytes_make_decoding_panic(mut bytes in vec(any::<u8>(), 0..400), small in any::<bool>()) {
        let _ = decode(&bytes);
        let _ = read_zip(&bytes);
        // Past the magic number, every field is arbitrary.
        if bytes.len() >= 4 {
            bytes[..4].copy_from_slice(b"Iout");
        }
        if small && bytes.len() > 64 {
            // Plausible type, point count and second-header offset, so
            // that the coordinate paths are reached.
            bytes[6] %= 11;
            bytes[16] = 0;
            bytes[17] %= 40;
            bytes[60..63].fill(0);
        }
        if let Ok(entry) = decode(&bytes) {
            exercise(&entry.roi);
            let _ = encode(&entry);
        }
    }

    #[test]
    fn damaged_files_are_errors_or_selections_never_panics(
        roi in roi(quarter()),
        props in props(),
        cut in any::<prop::sample::Index>(),
        hits in vec((any::<prop::sample::Index>(), any::<u8>()), 1..4),
    ) {
        let bytes = encode(&storable(RoiEntry { roi, props })).unwrap();
        // Cut short anywhere: an error, or (if only the tail is missing)
        // a selection.
        let short = &bytes[..cut.index(bytes.len())];
        if short.len() < 64 {
            prop_assert!(decode(short).is_err());
        } else if let Ok(entry) = decode(short) {
            exercise(&entry.roi);
        }
        let mut damaged = bytes.clone();
        for (at, value) in hits {
            let at = at.index(damaged.len());
            damaged[at] = value;
        }
        if let Ok(entry) = decode(&damaged) {
            exercise(&entry.roi);
            let _ = encode(&entry);
        }
        let zip = write_zip(&[RoiEntry::new(Roi::oval(1.0, 2.0, 3.0, 4.0))]).unwrap();
        let _ = read_zip(&zip[..cut.index(zip.len())]);
    }
}
