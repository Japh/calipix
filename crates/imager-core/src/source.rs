use std::fmt;
use std::sync::Arc;

use crate::view::PatchView;
use crate::{Buffer, Error, ImageMeta, MemImage, Pixel, Result, crop_buffer};

/// A rectangle within an XY plane, in samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub fn new(x: usize, y: usize, w: usize, h: usize) -> Rect {
        Rect { x, y, w, h }
    }

    /// True if the rectangle is non-empty and lies within a
    /// `width` x `height` plane.
    pub fn fits(&self, width: usize, height: usize) -> bool {
        self.w > 0
            && self.h > 0
            && self.x.checked_add(self.w).is_some_and(|r| r <= width)
            && self.y.checked_add(self.h).is_some_and(|b| b <= height)
    }
}

/// Provider of an image's samples, one XY plane or region at a time.
///
/// Implementations may hold everything in memory, read lazily from a file,
/// or adapt another source. They must be immutable: the same request always
/// returns the same samples.
///
/// Callers go through [`Image`], which validates arguments and results, so
/// implementations may assume `plane` and `region` are in range.
pub trait PixelSource: Send + Sync + fmt::Debug {
    fn meta(&self) -> &ImageMeta;

    /// All samples of plane number `plane`, row-major.
    fn read_plane(&self, plane: usize) -> Result<Buffer>;

    /// The samples of `region` within a plane, row-major. The default reads
    /// the whole plane; tiled or striped sources should override this.
    fn read_region(&self, plane: usize, region: Rect) -> Result<Buffer> {
        crop_buffer(&self.read_plane(plane)?, self.meta().width(), region)
    }

    /// Natural tile size for region reads, if the source is tiled.
    fn tile_size(&self) -> Option<(usize, usize)> {
        None
    }

    /// The in-memory image behind this source, if that is what it is.
    fn as_mem(&self) -> Option<&MemImage> {
        None
    }
}

/// An immutable N-dimensional image. Cloning is cheap.
#[derive(Clone, Debug)]
pub struct Image(Arc<dyn PixelSource>);

impl Image {
    pub fn new(source: impl PixelSource + 'static) -> Image {
        Image(Arc::new(source))
    }

    /// An in-memory image from one buffer per plane.
    pub fn from_planes(meta: ImageMeta, planes: Vec<Buffer>) -> Result<Image> {
        Ok(Image::new(MemImage::new(meta, planes)?))
    }

    /// A single in-memory plane from row-major samples.
    pub fn from_vec<T: Pixel>(width: usize, height: usize, data: Vec<T>) -> Result<Image> {
        let meta = ImageMeta::plane(T::DTYPE, width, height)?;
        Image::from_planes(meta, vec![Buffer::from_vec(data)])
    }

    pub fn meta(&self) -> &ImageMeta {
        self.0.meta()
    }

    pub fn read_plane(&self, plane: usize) -> Result<Buffer> {
        let meta = self.meta();
        if plane >= meta.plane_count() {
            return Err(Error::OutOfBounds(format!(
                "plane {plane} of {}",
                meta.plane_count()
            )));
        }
        let buf = self.0.read_plane(plane)?;
        self.check(&buf, meta.plane_len())?;
        Ok(buf)
    }

    pub fn read_region(&self, plane: usize, region: Rect) -> Result<Buffer> {
        let meta = self.meta();
        if plane >= meta.plane_count() {
            return Err(Error::OutOfBounds(format!(
                "plane {plane} of {}",
                meta.plane_count()
            )));
        }
        if !region.fits(meta.width(), meta.height()) {
            return Err(Error::OutOfBounds(format!(
                "region {region:?} outside {}x{} plane",
                meta.width(),
                meta.height()
            )));
        }
        let buf = self.0.read_region(plane, region)?;
        self.check(&buf, region.w * region.h)?;
        Ok(buf)
    }

    fn check(&self, buf: &Buffer, expected_len: usize) -> Result<()> {
        let meta = self.meta();
        if buf.dtype() != meta.dtype() {
            return Err(Error::DType {
                expected: meta.dtype(),
                found: buf.dtype(),
            });
        }
        if buf.len() != expected_len {
            return Err(Error::Shape(format!(
                "source returned {} samples, expected {expected_len}",
                buf.len()
            )));
        }
        Ok(())
    }

    pub fn tile_size(&self) -> Option<(usize, usize)> {
        self.0.tile_size()
    }

    /// True if all samples are held in memory.
    pub fn is_in_memory(&self) -> bool {
        self.0.as_mem().is_some()
    }

    /// True if both handles refer to the same underlying source.
    pub fn ptr_eq(&self, other: &Image) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Reads every plane into memory, collapsing any chain of views. Returns
    /// a clone if the image is already in memory.
    pub fn materialize(&self) -> Result<Image> {
        if self.is_in_memory() {
            return Ok(self.clone());
        }
        let planes = (0..self.meta().plane_count())
            .map(|p| self.read_plane(p))
            .collect::<Result<Vec<_>>>()?;
        Image::from_planes(self.meta().clone(), planes)
    }

    /// A new in-memory image with `f` applied to every plane. `f` may change
    /// the pixel type as long as it does so consistently.
    pub fn map_planes(&self, f: impl Fn(usize, &Buffer) -> Result<Buffer>) -> Result<Image> {
        let planes = (0..self.meta().plane_count())
            .map(|p| f(p, &self.read_plane(p)?))
            .collect::<Result<Vec<_>>>()?;
        let dtype = planes[0].dtype();
        Image::from_planes(self.meta().with_dtype(dtype), planes)
    }

    /// A new image with one plane replaced. All other planes are shared
    /// with `self`.
    pub fn with_plane(&self, plane: usize, data: Buffer) -> Result<Image> {
        let meta = self.meta();
        if plane >= meta.plane_count() {
            return Err(Error::OutOfBounds(format!(
                "plane {plane} of {}",
                meta.plane_count()
            )));
        }
        self.check(&data, meta.plane_len())?;
        if let Some(mem) = self.0.as_mem() {
            let mut planes = mem.planes().to_vec();
            planes[plane] = data;
            return Image::from_planes(meta.clone(), planes);
        }
        Ok(Image::new(PatchView::new(self.clone(), plane, data)))
    }

    /// The same samples under different metadata. The pixel type and shape
    /// must be unchanged.
    pub fn with_meta(&self, meta: ImageMeta) -> Result<Image> {
        meta.validate()?;
        let current = self.meta();
        if meta.dtype() != current.dtype() || meta.shape() != current.shape() {
            return Err(Error::Shape(
                "replacement metadata must keep the pixel type and shape".to_string(),
            ));
        }
        crate::view::with_meta(self, meta)
    }
}
