//! Property tests: a view, or any composition of views, must hold exactly
//! the samples that the corresponding index arithmetic picks out of the
//! source, and must agree with its own materialised copy.

use imager_core::{Axis, AxisKind, Buffer, ChannelMeta, ColorModel, DType, Image, ImageMeta, Rect};
use proptest::prelude::*;

/// An image whose sample at full position `p` encodes `p` uniquely.
fn coded_image(shape: &[usize]) -> Image {
    let kinds = [
        AxisKind::X,
        AxisKind::Y,
        AxisKind::Channel,
        AxisKind::Z,
        AxisKind::Time,
    ];
    let axes = shape
        .iter()
        .zip(kinds)
        .map(|(&n, kind)| (Axis::calibrated(kind, 0.5, "um"), n))
        .collect();
    let meta = ImageMeta::new(DType::U32, axes).unwrap();
    let planes = (0..meta.plane_count())
        .map(|plane| {
            let mut data = Vec::with_capacity(meta.plane_len());
            for y in 0..meta.height() {
                for x in 0..meta.width() {
                    data.push(((plane * 100 + y) * 100 + x) as u32);
                }
            }
            Buffer::from_vec(data)
        })
        .collect();
    Image::from_planes(meta, planes).unwrap()
}

/// Reads the sample at a full N-d position through the public API.
fn sample(image: &Image, position: &[usize]) -> u32 {
    let meta = image.meta();
    let plane = meta.plane_index(&position[2..]).unwrap();
    let buf = image.read_plane(plane).unwrap();
    buf.as_slice::<u32>().unwrap()[position[1] * meta.width() + position[0]]
}

fn positions(shape: &[usize]) -> Vec<Vec<usize>> {
    let mut all = vec![vec![]];
    for &n in shape {
        all = all
            .into_iter()
            .flat_map(|p| {
                (0..n).map(move |i| {
                    let mut q = p.clone();
                    q.push(i);
                    q
                })
            })
            .collect();
    }
    all
}

fn assert_same(a: &Image, b: &Image) {
    assert_eq!(a.meta(), b.meta());
    for plane in 0..a.meta().plane_count() {
        assert_eq!(a.read_plane(plane).unwrap(), b.read_plane(plane).unwrap());
    }
}

fn shape_strategy() -> impl Strategy<Value = Vec<usize>> {
    prop::collection::vec(1usize..=4, 2..=5)
}

/// One view operation, with parameters given as fractions so they can be
/// scaled to whatever shape the image has when the operation is applied.
#[derive(Clone, Debug)]
enum Op {
    Slice { axis: f64, index: f64 },
    Range { axis: f64, start: f64, len: f64 },
    Crop { x: f64, y: f64, w: f64, h: f64 },
    Permute { seed: u64 },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    let f = || 0.0f64..1.0;
    prop_oneof![
        (f(), f()).prop_map(|(axis, index)| Op::Slice { axis, index }),
        (f(), f(), f()).prop_map(|(axis, start, len)| Op::Range { axis, start, len }),
        (f(), f(), f(), f()).prop_map(|(x, y, w, h)| Op::Crop { x, y, w, h }),
        any::<u64>().prop_map(|seed| Op::Permute { seed }),
    ]
}

fn pick(fraction: f64, n: usize) -> usize {
    ((fraction * n as f64) as usize).min(n - 1)
}

/// Maps a position in a view to the matching position in its source.
type PositionMap = Box<dyn Fn(&[usize]) -> Vec<usize>>;

/// Applies `op` to `image`. Returns the view and a function mapping a
/// position in the view to the position in `image` it must equal.
fn apply(image: &Image, op: &Op) -> Option<(Image, PositionMap)> {
    let shape = image.meta().shape().to_vec();
    match *op {
        Op::Slice { axis, index } => {
            if shape.len() == 2 {
                return None;
            }
            let axis = 2 + pick(axis, shape.len() - 2);
            let index = pick(index, shape[axis]);
            let view = image.slice(axis, index).unwrap();
            Some((
                view,
                Box::new(move |p| {
                    let mut q = p.to_vec();
                    q.insert(axis, index);
                    q
                }),
            ))
        }
        Op::Range { axis, start, len } => {
            let axis = pick(axis, shape.len());
            let start = pick(start, shape[axis]);
            let len = 1 + pick(len, shape[axis] - start);
            let view = image.range(axis, start, len).unwrap();
            Some((
                view,
                Box::new(move |p| {
                    let mut q = p.to_vec();
                    q[axis] += start;
                    q
                }),
            ))
        }
        Op::Crop { x, y, w, h } => {
            let x = pick(x, shape[0]);
            let y = pick(y, shape[1]);
            let w = 1 + pick(w, shape[0] - x);
            let h = 1 + pick(h, shape[1] - y);
            let view = image.crop(Rect::new(x, y, w, h)).unwrap();
            Some((
                view,
                Box::new(move |p| {
                    let mut q = p.to_vec();
                    q[0] += x;
                    q[1] += y;
                    q
                }),
            ))
        }
        Op::Permute { seed } => {
            let mut order: Vec<usize> = (2..shape.len()).collect();
            // Deterministic shuffle driven by the seed.
            let mut s = seed;
            for i in (1..order.len()).rev() {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                order.swap(i, (s >> 33) as usize % (i + 1));
            }
            let view = image.permute(&order).unwrap();
            Some((
                view,
                Box::new(move |p| {
                    let mut q = p.to_vec();
                    for (i, &a) in order.iter().enumerate() {
                        q[a] = p[2 + i];
                    }
                    q
                }),
            ))
        }
    }
}

proptest! {
    #[test]
    fn composed_views_match_index_arithmetic(
        shape in shape_strategy(),
        ops in prop::collection::vec(op_strategy(), 1..5),
    ) {
        let source = coded_image(&shape);
        let mut view = source.clone();
        // Maps a position in the current view back to the source.
        let mut to_source: PositionMap = Box::new(|p| p.to_vec());
        for op in &ops {
            if let Some((next, step)) = apply(&view, op) {
                let previous = to_source;
                to_source = Box::new(move |p| previous(&step(p)));
                view = next;
            }
        }

        view.meta().validate().unwrap();
        for p in positions(view.meta().shape()) {
            prop_assert_eq!(sample(&view, &p), sample(&source, &to_source(&p)));
        }

        let materialized = view.materialize().unwrap();
        prop_assert!(materialized.is_in_memory());
        assert_same(&view, &materialized);
    }

    #[test]
    fn region_reads_match_plane_reads(
        shape in shape_strategy(),
        ops in prop::collection::vec(op_strategy(), 0..4),
        (x, y, w, h) in (0.0f64..1.0, 0.0f64..1.0, 0.0f64..1.0, 0.0f64..1.0),
    ) {
        let mut view = coded_image(&shape);
        for op in &ops {
            if let Some((next, _)) = apply(&view, op) {
                view = next;
            }
        }
        let meta = view.meta().clone();
        let rx = pick(x, meta.width());
        let ry = pick(y, meta.height());
        let rect = Rect::new(
            rx,
            ry,
            1 + pick(w, meta.width() - rx),
            1 + pick(h, meta.height() - ry),
        );
        for plane in 0..meta.plane_count() {
            let whole = view.read_plane(plane).unwrap();
            let expected = imager_core::crop_buffer(&whole, meta.width(), rect).unwrap();
            prop_assert_eq!(view.read_region(plane, rect).unwrap(), expected);
        }
    }
}

#[test]
fn crop_and_range_shift_calibration() {
    let image = coded_image(&[4, 4, 2, 3]);
    let cropped = image.crop(Rect::new(1, 2, 2, 2)).unwrap();
    assert_eq!(cropped.meta().axes()[0].origin, 0.5);
    assert_eq!(cropped.meta().axes()[1].origin, 1.0);
    let ranged = image.range(3, 2, 1).unwrap();
    assert_eq!(ranged.meta().axes()[3].origin, 1.0);
    assert_eq!(ranged.meta().axes()[3].unit.as_deref(), Some("um"));
}

#[test]
fn channel_metadata_follows_views() {
    let image = coded_image(&[2, 2, 3]);
    let mut meta = image.meta().clone();
    meta.color = ColorModel::Rgb;
    meta.channels = ["r", "g", "b"]
        .into_iter()
        .map(|n| ChannelMeta {
            name: Some(n.to_string()),
            ..ChannelMeta::default()
        })
        .collect();
    let rgb = image.with_meta(meta).unwrap();

    let green = rgb.slice(2, 1).unwrap();
    assert_eq!(green.meta().color, ColorModel::Grayscale);
    assert_eq!(green.meta().channels.len(), 1);
    assert_eq!(green.meta().channels[0].name.as_deref(), Some("g"));

    let gb = rgb.range(2, 1, 2).unwrap();
    assert_eq!(gb.meta().color, ColorModel::Composite);
    assert_eq!(gb.meta().channels[1].name.as_deref(), Some("b"));

    assert_eq!(rgb.range(2, 0, 3).unwrap().meta().color, ColorModel::Rgb);
}

#[test]
fn invalid_views_are_rejected() {
    let image = coded_image(&[3, 3, 2, 2]);
    assert!(image.slice(0, 0).is_err());
    assert!(image.slice(2, 2).is_err());
    assert!(image.slice(4, 0).is_err());
    assert!(image.range(2, 1, 2).is_err());
    assert!(image.range(2, 0, 0).is_err());
    assert!(image.crop(Rect::new(2, 2, 2, 2)).is_err());
    assert!(image.permute(&[2, 2]).is_err());
    assert!(image.permute(&[3]).is_err());
    assert!(image.permute(&[3, 2]).is_ok());
}

#[test]
fn with_plane_on_a_view_patches_lazily() {
    let image = coded_image(&[2, 2, 3]);
    let view = image.range(2, 1, 2).unwrap();
    assert!(!view.is_in_memory());
    let patched = view.with_plane(0, Buffer::from_vec(vec![7u32; 4])).unwrap();
    assert_eq!(
        patched.read_plane(0).unwrap().as_slice::<u32>().unwrap(),
        &[7; 4]
    );
    assert_eq!(
        patched
            .read_region(0, Rect::new(1, 1, 1, 1))
            .unwrap()
            .as_slice::<u32>()
            .unwrap(),
        &[7]
    );
    assert_eq!(patched.read_plane(1).unwrap(), image.read_plane(2).unwrap());
}
