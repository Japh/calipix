//! Lazy views: [`PixelSource`] adaptors that re-address another image's
//! samples without copying them.

use crate::{AxisKind, Buffer, ColorModel, Error, Image, ImageMeta, PixelSource, Rect, Result};

/// A view whose planes are a selection or reordering of the source's planes.
#[derive(Debug)]
struct PlaneMapView {
    src: Image,
    meta: ImageMeta,
    /// Source plane number for each plane of the view.
    map: Vec<usize>,
}

impl PixelSource for PlaneMapView {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, plane: usize) -> Result<Buffer> {
        self.src.read_plane(self.map[plane])
    }

    fn read_region(&self, plane: usize, region: Rect) -> Result<Buffer> {
        self.src.read_region(self.map[plane], region)
    }

    fn tile_size(&self) -> Option<(usize, usize)> {
        self.src.tile_size()
    }
}

/// A view of a rectangle of every plane.
#[derive(Debug)]
struct CropView {
    src: Image,
    meta: ImageMeta,
    rect: Rect,
}

impl PixelSource for CropView {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, plane: usize) -> Result<Buffer> {
        self.src.read_region(plane, self.rect)
    }

    fn read_region(&self, plane: usize, region: Rect) -> Result<Buffer> {
        let shifted = Rect::new(
            self.rect.x + region.x,
            self.rect.y + region.y,
            region.w,
            region.h,
        );
        self.src.read_region(plane, shifted)
    }

    fn tile_size(&self) -> Option<(usize, usize)> {
        self.src.tile_size()
    }
}

/// A view with one plane replaced by an in-memory buffer.
#[derive(Debug)]
pub(crate) struct PatchView {
    src: Image,
    plane: usize,
    data: Buffer,
}

impl PatchView {
    pub(crate) fn new(src: Image, plane: usize, data: Buffer) -> PatchView {
        PatchView { src, plane, data }
    }
}

impl PixelSource for PatchView {
    fn meta(&self) -> &ImageMeta {
        self.src.meta()
    }

    fn read_plane(&self, plane: usize) -> Result<Buffer> {
        if plane == self.plane {
            Ok(self.data.clone())
        } else {
            self.src.read_plane(plane)
        }
    }

    fn read_region(&self, plane: usize, region: Rect) -> Result<Buffer> {
        if plane == self.plane {
            crate::crop_buffer(&self.data, self.meta().width(), region)
        } else {
            self.src.read_region(plane, region)
        }
    }

    fn tile_size(&self) -> Option<(usize, usize)> {
        self.src.tile_size()
    }
}

pub(crate) fn with_meta(src: &Image, meta: ImageMeta) -> Result<Image> {
    let map = (0..meta.plane_count()).collect();
    Ok(Image::new(PlaneMapView {
        src: src.clone(),
        meta,
        map,
    }))
}

/// Builds a [`PlaneMapView`] from the view's metadata and a function giving,
/// for each position along the view's non-XY axes, the corresponding
/// position in the source.
fn plane_map_view(
    src: &Image,
    meta: ImageMeta,
    to_source: impl Fn(&[usize]) -> Vec<usize>,
) -> Result<Image> {
    meta.validate()?;
    let map = (0..meta.plane_count())
        .map(|plane| {
            let position = meta.plane_position(plane)?;
            src.meta().plane_index(&to_source(&position))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Image::new(PlaneMapView {
        src: src.clone(),
        meta,
        map,
    }))
}

fn check_axis(meta: &ImageMeta, axis: usize) -> Result<()> {
    if axis >= meta.ndim() {
        return Err(Error::OutOfBounds(format!(
            "axis {axis} of a {}-dimensional image",
            meta.ndim()
        )));
    }
    Ok(())
}

impl Image {
    /// Fixes `index` along a non-XY `axis`, producing a view with that axis
    /// removed.
    pub fn slice(&self, axis: usize, index: usize) -> Result<Image> {
        let src = self.meta();
        check_axis(src, axis)?;
        if axis < 2 {
            return Err(Error::Unsupported(
                "cannot slice along X or Y; use crop or range".to_string(),
            ));
        }
        if index >= src.shape()[axis] {
            return Err(Error::OutOfBounds(format!(
                "index {index} along axis {axis} of size {}",
                src.shape()[axis]
            )));
        }
        let mut axes = src.axes().to_vec();
        let mut shape = src.shape().to_vec();
        let removed = axes.remove(axis);
        shape.remove(axis);
        let mut meta = src.reshaped(src.dtype(), axes, shape);
        if removed.kind == AxisKind::Channel {
            if !meta.channels.is_empty() {
                meta.channels = vec![meta.channels[index].clone()];
            }
            meta.color = ColorModel::Grayscale;
        }
        plane_map_view(self, meta, |position| {
            let mut p = position.to_vec();
            p.insert(axis - 2, index);
            p
        })
    }

    /// Restricts `axis` to `len` samples starting at `start`. Works along
    /// any axis, including X and Y.
    pub fn range(&self, axis: usize, start: usize, len: usize) -> Result<Image> {
        let src = self.meta();
        check_axis(src, axis)?;
        let size = src.shape()[axis];
        if len == 0 || start.checked_add(len).is_none_or(|end| end > size) {
            return Err(Error::OutOfBounds(format!(
                "range {start}..{start}+{len} along axis {axis} of size {size}"
            )));
        }
        if axis == 0 {
            return self.crop(Rect::new(start, 0, len, src.height()));
        }
        if axis == 1 {
            return self.crop(Rect::new(0, start, src.width(), len));
        }
        let mut axes = src.axes().to_vec();
        let mut shape = src.shape().to_vec();
        axes[axis].origin += start as f64 * axes[axis].scale;
        shape[axis] = len;
        let is_channel = axes[axis].kind == AxisKind::Channel;
        let mut meta = src.reshaped(src.dtype(), axes, shape);
        if is_channel {
            if !meta.channels.is_empty() {
                meta.channels = meta.channels[start..start + len].to_vec();
            }
            if len == 1 {
                meta.color = ColorModel::Grayscale;
            } else if meta.color == ColorModel::Rgb && len != 3 {
                meta.color = ColorModel::Composite;
            }
        }
        plane_map_view(self, meta, |position| {
            let mut p = position.to_vec();
            p[axis - 2] += start;
            p
        })
    }

    /// A view of `rect` within every plane.
    pub fn crop(&self, rect: Rect) -> Result<Image> {
        let src = self.meta();
        if !rect.fits(src.width(), src.height()) {
            return Err(Error::OutOfBounds(format!(
                "crop {rect:?} outside {}x{} plane",
                src.width(),
                src.height()
            )));
        }
        let mut axes = src.axes().to_vec();
        let mut shape = src.shape().to_vec();
        axes[0].origin += rect.x as f64 * axes[0].scale;
        axes[1].origin += rect.y as f64 * axes[1].scale;
        shape[0] = rect.w;
        shape[1] = rect.h;
        let meta = src.reshaped(src.dtype(), axes, shape);
        meta.validate()?;
        Ok(Image::new(CropView {
            src: self.clone(),
            meta,
            rect,
        }))
    }

    /// Reorders the non-XY axes. `order[i]` is the source axis that becomes
    /// axis `2 + i` of the view; it must be a permutation of `2..ndim`.
    pub fn permute(&self, order: &[usize]) -> Result<Image> {
        let src = self.meta();
        let mut sorted = order.to_vec();
        sorted.sort_unstable();
        if sorted != (2..src.ndim()).collect::<Vec<_>>() {
            return Err(Error::Shape(format!(
                "{order:?} is not a permutation of the non-XY axes 2..{}",
                src.ndim()
            )));
        }
        let mut axes = src.axes()[..2].to_vec();
        let mut shape = src.shape()[..2].to_vec();
        for &a in order {
            axes.push(src.axes()[a].clone());
            shape.push(src.shape()[a]);
        }
        let meta = src.reshaped(src.dtype(), axes, shape);
        plane_map_view(self, meta, |position| {
            let mut p = vec![0; position.len()];
            for (i, &a) in order.iter().enumerate() {
                p[a - 2] = position[i];
            }
            p
        })
    }
}
