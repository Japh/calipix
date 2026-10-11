use crate::{Buffer, Error, ImageMeta, PixelSource, Result};

/// An image held entirely in memory, one [`Buffer`] per XY plane.
#[derive(Clone, Debug)]
pub struct MemImage {
    meta: ImageMeta,
    planes: Vec<Buffer>,
}

impl MemImage {
    pub fn new(meta: ImageMeta, planes: Vec<Buffer>) -> Result<MemImage> {
        meta.validate()?;
        if planes.len() != meta.plane_count() {
            return Err(Error::Shape(format!(
                "{} planes supplied for shape {:?}",
                planes.len(),
                meta.shape()
            )));
        }
        for plane in &planes {
            if plane.dtype() != meta.dtype() {
                return Err(Error::DType {
                    expected: meta.dtype(),
                    found: plane.dtype(),
                });
            }
            if plane.len() != meta.plane_len() {
                return Err(Error::Shape(format!(
                    "plane of {} samples in a {}x{} image",
                    plane.len(),
                    meta.width(),
                    meta.height()
                )));
            }
        }
        Ok(MemImage { meta, planes })
    }

    pub fn planes(&self) -> &[Buffer] {
        &self.planes
    }
}

impl PixelSource for MemImage {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, plane: usize) -> Result<Buffer> {
        Ok(self.planes[plane].clone())
    }

    fn as_mem(&self) -> Option<&MemImage> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DType, Image, Rect};

    fn stack() -> Image {
        let meta = ImageMeta::xyczt(DType::U8, 3, 2, 1, 2, 1).unwrap();
        let planes = vec![
            Buffer::from_vec(vec![0u8, 1, 2, 3, 4, 5]),
            Buffer::from_vec(vec![10u8, 11, 12, 13, 14, 15]),
        ];
        Image::from_planes(meta, planes).unwrap()
    }

    #[test]
    fn validates_planes() {
        let meta = ImageMeta::plane(DType::U8, 2, 2).unwrap();
        assert!(MemImage::new(meta.clone(), vec![]).is_err());
        assert!(MemImage::new(meta.clone(), vec![Buffer::zeros(DType::U16, 4)]).is_err());
        assert!(MemImage::new(meta.clone(), vec![Buffer::zeros(DType::U8, 5)]).is_err());
        assert!(MemImage::new(meta, vec![Buffer::zeros(DType::U8, 4)]).is_ok());
    }

    #[test]
    fn reads_planes_and_regions() {
        let img = stack();
        assert_eq!(img.read_plane(1).unwrap().as_slice::<u8>().unwrap()[0], 10);
        let r = img.read_region(1, Rect::new(1, 0, 2, 2)).unwrap();
        assert_eq!(r.as_slice::<u8>().unwrap(), &[11, 12, 14, 15]);
        assert!(img.read_plane(2).is_err());
        assert!(img.read_region(0, Rect::new(2, 0, 2, 1)).is_err());
        assert!(img.read_region(0, Rect::new(0, 0, 0, 1)).is_err());
    }

    #[test]
    fn with_plane_shares_untouched_planes() {
        let img = stack();
        let edited = img.with_plane(0, Buffer::from_vec(vec![9u8; 6])).unwrap();
        assert_eq!(
            edited.read_plane(0).unwrap().as_slice::<u8>().unwrap(),
            &[9; 6]
        );
        assert!(
            edited
                .read_plane(1)
                .unwrap()
                .shares_storage(&img.read_plane(1).unwrap())
        );
        // The original is untouched.
        assert_eq!(img.read_plane(0).unwrap().as_slice::<u8>().unwrap()[1], 1);
        assert!(img.with_plane(0, Buffer::zeros(DType::U8, 5)).is_err());
        assert!(img.with_plane(0, Buffer::zeros(DType::F32, 6)).is_err());
    }

    #[test]
    fn map_planes_can_change_type() {
        let img = stack();
        let out = img
            .map_planes(|_, b| {
                let s = b.try_slice::<u8>()?;
                Ok(Buffer::from_vec(
                    s.iter().map(|&v| v as f32 * 0.5).collect::<Vec<_>>(),
                ))
            })
            .unwrap();
        assert_eq!(out.meta().dtype(), DType::F32);
        assert_eq!(out.read_plane(1).unwrap().get_f64(5), Some(7.5));
    }

    #[test]
    fn with_meta_keeps_an_in_memory_image_in_memory_and_shares_planes() {
        let img = stack();
        let mut meta = img.meta().clone();
        meta.name = "renamed".to_string();
        let renamed = img.with_meta(meta).unwrap();
        assert!(renamed.is_in_memory());
        assert_eq!(renamed.meta().name, "renamed");
        assert!(
            renamed
                .read_plane(1)
                .unwrap()
                .shares_storage(&img.read_plane(1).unwrap())
        );
    }
}
