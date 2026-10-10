use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{DType, Error, Result};

/// What an image axis represents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AxisKind {
    X,
    Y,
    Z,
    Channel,
    Time,
    Other(String),
}

/// A named, calibrated image axis. Sample index `i` lies at physical
/// position `origin + i * scale`, in `unit`.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub kind: AxisKind,
    pub scale: f64,
    pub origin: f64,
    pub unit: Option<String>,
}

impl Axis {
    /// An uncalibrated axis (scale 1, origin 0, no unit).
    pub fn new(kind: AxisKind) -> Axis {
        Axis {
            kind,
            scale: 1.0,
            origin: 0.0,
            unit: None,
        }
    }

    pub fn calibrated(kind: AxisKind, scale: f64, unit: impl Into<String>) -> Axis {
        Axis {
            kind,
            scale,
            origin: 0.0,
            unit: Some(unit.into()),
        }
    }
}

/// A 256-entry RGB lookup table.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut {
    pub name: String,
    pub table: Arc<[[u8; 3]; 256]>,
}

impl Lut {
    pub fn from_fn(name: impl Into<String>, f: impl Fn(u8) -> [u8; 3]) -> Lut {
        let mut table = [[0u8; 3]; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            *entry = f(i as u8);
        }
        Lut {
            name: name.into(),
            table: Arc::new(table),
        }
    }

    pub fn grays() -> Lut {
        Lut::from_fn("Grays", |i| [i, i, i])
    }

    pub fn inverted(&self) -> Lut {
        let table = self.table.clone();
        Lut::from_fn(format!("{} (inverted)", self.name), move |i| {
            table[255 - i as usize]
        })
    }
}

/// How channels combine into a displayed colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorModel {
    /// One channel shown at a time through its lookup table.
    Grayscale,
    /// Channels are blended, each through its own lookup table.
    Composite,
    /// Exactly three channels holding red, green and blue.
    Rgb,
}

/// Per-channel display metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelMeta {
    pub name: Option<String>,
    pub lut: Option<Lut>,
    /// Sample values mapped to the darkest and brightest display levels.
    pub display_range: Option<(f64, f64)>,
}

/// Everything known about an image except its samples.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageMeta {
    pub name: String,
    dtype: DType,
    shape: Vec<usize>,
    axes: Vec<Axis>,
    pub color: ColorModel,
    /// Either empty or one entry per channel.
    pub channels: Vec<ChannelMeta>,
    pub properties: BTreeMap<String, String>,
}

impl ImageMeta {
    /// Creates metadata for an image with the given axes and their sizes.
    ///
    /// The first two axes must be X then Y, every size must be non-zero, and
    /// X, Y, Z, channel and time may each appear at most once.
    pub fn new(dtype: DType, axes: Vec<(Axis, usize)>) -> Result<ImageMeta> {
        let (axes, shape): (Vec<Axis>, Vec<usize>) = axes.into_iter().unzip();
        let meta = ImageMeta {
            name: String::new(),
            dtype,
            shape,
            axes,
            color: ColorModel::Grayscale,
            channels: Vec::new(),
            properties: BTreeMap::new(),
        };
        meta.validate()?;
        Ok(meta)
    }

    /// A single uncalibrated XY plane.
    pub fn plane(dtype: DType, width: usize, height: usize) -> Result<ImageMeta> {
        ImageMeta::xyczt(dtype, width, height, 1, 1, 1)
    }

    /// An uncalibrated image in ImageJ's XYCZT order. Channel, Z and time
    /// axes of size 1 are omitted.
    pub fn xyczt(
        dtype: DType,
        width: usize,
        height: usize,
        channels: usize,
        slices: usize,
        frames: usize,
    ) -> Result<ImageMeta> {
        let mut axes = vec![
            (Axis::new(AxisKind::X), width),
            (Axis::new(AxisKind::Y), height),
        ];
        for (kind, size) in [
            (AxisKind::Channel, channels),
            (AxisKind::Z, slices),
            (AxisKind::Time, frames),
        ] {
            if size != 1 {
                axes.push((Axis::new(kind), size));
            }
        }
        ImageMeta::new(dtype, axes)
    }

    pub fn with_name(mut self, name: impl Into<String>) -> ImageMeta {
        self.name = name.into();
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.axes.len() < 2
            || self.axes[0].kind != AxisKind::X
            || self.axes[1].kind != AxisKind::Y
        {
            return Err(Error::Shape(
                "the first two axes must be X then Y".to_string(),
            ));
        }
        if self.shape.contains(&0) {
            return Err(Error::Shape(format!(
                "zero-sized axis in shape {:?}",
                self.shape
            )));
        }
        for (i, axis) in self.axes.iter().enumerate() {
            let unique = !matches!(axis.kind, AxisKind::Other(_));
            if unique && self.axes[..i].iter().any(|a| a.kind == axis.kind) {
                return Err(Error::Shape(format!("duplicate {:?} axis", axis.kind)));
            }
        }
        let n_channels = self.size(&AxisKind::Channel);
        if !self.channels.is_empty() && self.channels.len() != n_channels {
            return Err(Error::Shape(format!(
                "{} channel descriptions for {n_channels} channels",
                self.channels.len()
            )));
        }
        if self.color == ColorModel::Rgb && n_channels != 3 {
            return Err(Error::Shape(format!(
                "RGB images need 3 channels, found {n_channels}"
            )));
        }
        Ok(())
    }

    pub fn dtype(&self) -> DType {
        self.dtype
    }

    /// Axis sizes, X first.
    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    pub fn axes(&self) -> &[Axis] {
        &self.axes
    }

    /// Mutable access to one axis' calibration. The axis kind must not be
    /// changed in a way that breaks [`validate`](Self::validate).
    pub fn axis_mut(&mut self, axis: usize) -> Option<&mut Axis> {
        self.axes.get_mut(axis)
    }

    pub fn ndim(&self) -> usize {
        self.shape.len()
    }

    pub fn width(&self) -> usize {
        self.shape[0]
    }

    pub fn height(&self) -> usize {
        self.shape[1]
    }

    /// Samples in one XY plane.
    pub fn plane_len(&self) -> usize {
        self.shape[0] * self.shape[1]
    }

    /// Sizes of the axes beyond X and Y.
    pub fn plane_shape(&self) -> &[usize] {
        &self.shape[2..]
    }

    /// Number of XY planes.
    pub fn plane_count(&self) -> usize {
        self.plane_shape().iter().product()
    }

    /// Index of the first axis of the given kind.
    pub fn axis_index(&self, kind: &AxisKind) -> Option<usize> {
        self.axes.iter().position(|a| &a.kind == kind)
    }

    /// Size along the axis of the given kind, or 1 if there is none.
    pub fn size(&self, kind: &AxisKind) -> usize {
        self.axis_index(kind).map_or(1, |i| self.shape[i])
    }

    /// Plane number for a position along the non-XY axes (axis 2 first).
    pub fn plane_index(&self, position: &[usize]) -> Result<usize> {
        let dims = self.plane_shape();
        if position.len() != dims.len() {
            return Err(Error::Shape(format!(
                "position has {} coordinates, image has {} non-XY axes",
                position.len(),
                dims.len()
            )));
        }
        let mut index = 0;
        let mut stride = 1;
        for (&p, &d) in position.iter().zip(dims) {
            if p >= d {
                return Err(Error::OutOfBounds(format!(
                    "position {position:?} outside {dims:?}"
                )));
            }
            index += p * stride;
            stride *= d;
        }
        Ok(index)
    }

    /// Inverse of [`plane_index`](Self::plane_index).
    pub fn plane_position(&self, plane: usize) -> Result<Vec<usize>> {
        if plane >= self.plane_count() {
            return Err(Error::OutOfBounds(format!(
                "plane {plane} of {}",
                self.plane_count()
            )));
        }
        let mut rest = plane;
        Ok(self
            .plane_shape()
            .iter()
            .map(|&d| {
                let p = rest % d;
                rest /= d;
                p
            })
            .collect())
    }

    /// Channel that plane number `plane` belongs to (0 if the image has no
    /// channel axis).
    pub fn plane_channel(&self, plane: usize) -> Result<usize> {
        let position = self.plane_position(plane)?;
        Ok(self
            .axis_index(&AxisKind::Channel)
            .map_or(0, |axis| position[axis - 2]))
    }

    /// Replaces dtype, axes and sizes, keeping everything else. Used by
    /// views and by operations that change the pixel type; the caller fixes
    /// up channel metadata before the result is validated.
    pub(crate) fn reshaped(&self, dtype: DType, axes: Vec<Axis>, shape: Vec<usize>) -> ImageMeta {
        ImageMeta {
            dtype,
            shape,
            axes,
            ..self.clone()
        }
    }

    /// Same image description with a different pixel type.
    pub fn with_dtype(&self, dtype: DType) -> ImageMeta {
        self.reshaped(dtype, self.axes.clone(), self.shape.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xyczt_omits_singletons() {
        let m = ImageMeta::xyczt(DType::U16, 8, 6, 3, 1, 5).unwrap();
        assert_eq!(m.shape(), &[8, 6, 3, 5]);
        assert_eq!(m.size(&AxisKind::Z), 1);
        assert_eq!(m.size(&AxisKind::Time), 5);
        assert_eq!(m.plane_count(), 15);
        assert_eq!(m.plane_len(), 48);
    }

    #[test]
    fn plane_index_round_trips() {
        let m = ImageMeta::xyczt(DType::U8, 2, 2, 3, 4, 5).unwrap();
        for plane in 0..m.plane_count() {
            let pos = m.plane_position(plane).unwrap();
            assert_eq!(m.plane_index(&pos).unwrap(), plane);
        }
        // Channel (axis 2) varies fastest, as in ImageJ's XYCZT ordering.
        assert_eq!(m.plane_index(&[1, 0, 0]).unwrap(), 1);
        assert_eq!(m.plane_index(&[0, 1, 0]).unwrap(), 3);
        assert_eq!(m.plane_index(&[0, 0, 1]).unwrap(), 12);
        assert!(m.plane_index(&[3, 0, 0]).is_err());
        assert!(m.plane_index(&[0, 0]).is_err());
        assert!(m.plane_position(60).is_err());
    }

    #[test]
    fn rejects_invalid_axes() {
        let x = || Axis::new(AxisKind::X);
        let y = || Axis::new(AxisKind::Y);
        assert!(ImageMeta::new(DType::U8, vec![(y(), 2), (x(), 2)]).is_err());
        assert!(ImageMeta::new(DType::U8, vec![(x(), 2)]).is_err());
        assert!(ImageMeta::new(DType::U8, vec![(x(), 2), (y(), 0)]).is_err());
        let z = || Axis::new(AxisKind::Z);
        assert!(ImageMeta::new(DType::U8, vec![(x(), 2), (y(), 2), (z(), 2), (z(), 2)]).is_err());
        let other = || Axis::new(AxisKind::Other("lambda".into()));
        assert!(
            ImageMeta::new(
                DType::U8,
                vec![(x(), 2), (y(), 2), (other(), 2), (other(), 2)]
            )
            .is_ok()
        );
    }

    #[test]
    fn rgb_needs_three_channels() {
        let mut m = ImageMeta::xyczt(DType::U8, 2, 2, 2, 1, 1).unwrap();
        m.color = ColorModel::Rgb;
        assert!(m.validate().is_err());
    }

    #[test]
    fn inverted_lut() {
        let lut = Lut::grays().inverted();
        assert_eq!(lut.table[0], [255, 255, 255]);
        assert_eq!(lut.table[255], [0, 0, 0]);
    }
}
