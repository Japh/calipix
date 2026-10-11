//! From sample values to display levels.
//!
//! Every displayed sample first becomes a *display level* from 0 to 255,
//! which then indexes a 256-entry lookup table (or, for RGB images, is the
//! colour component itself). [`LevelMap`] is that first step. Which formula
//! applies depends on the pixel type; see [`LevelRule`].

use imager_core::{Buffer, DType, Error, RealPixel, Result, dispatch_real};

/// The formula that turns a sample into a display level.
///
/// In every formula `(int)` is Java's conversion from `double`: it truncates
/// toward zero, saturates at the limits of a 32-bit integer and turns NaN
/// into 0 (Rust's `as i32` does the same). All arithmetic is in double
/// precision. For every rule, a display range that is not `max >= min`
/// (including one containing NaN) maps every sample to level 0; ImageJ
/// either ignores such a range or shows wrapped values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelRule {
    /// Unsigned 8-bit samples that are not part of an RGB image
    /// (`ByteProcessor.setMinAndMax`, which rescales the lookup table):
    ///
    /// ```text
    /// level = 0                                              if v < min
    ///         255                                            if v > max
    ///         clamp((int)(256.0 * (v - rmin) / (max - min)), 0, 255)   otherwise
    /// rmin  = floor(min + 0.5)
    /// ```
    ///
    /// The comparisons and the divisor use `min` and `max` as given; only
    /// the subtraction uses the rounded minimum. With the default range
    /// 0 to 255 the level equals the sample. When `min == max` the sample
    /// equal to both gets level 0.
    Byte,
    /// Every other integer type. ImageJ has this rule for unsigned 16-bit
    /// only (`ShortProcessor.create8BitImage`); ImageR2 applies it to signed
    /// and to 32 and 64-bit integers as well:
    ///
    /// ```text
    /// min2  = floor(min + 0.5)        max2 = floor(max + 0.5)
    /// level = min((int)(max(v - min2, 0) * (256.0 / (max2 - min2 + 1)) + 0.5), 255)
    /// ```
    ///
    /// 64-bit samples are converted to double first, so values beyond 2^53
    /// lose their low bits.
    Integer,
    /// Floating point samples (`FloatProcessor.create8BitImage`):
    ///
    /// ```text
    /// level = min((int)(max(v - min, 0) * (255.0 / (max - min)) + 0.5), 255)
    /// ```
    ///
    /// where `max(NaN, 0)` stays NaN, so NaN samples get level 0, and
    /// positive infinity gets 255. For 32-bit samples `min` and `max` are
    /// first rounded to single precision, as ImageJ stores them; 64-bit
    /// samples use them unchanged. When `min == max`, samples above get
    /// level 255 and all others 0.
    Float,
    /// The components of an 8-bit RGB image (`ColorProcessor.setMinAndMax`,
    /// which in ImageJ rewrites the pixels; ImageR2 only displays them
    /// differently):
    ///
    /// ```text
    /// level = clamp((int)(256.0 * (v - (int)min) / (max - min)), 0, 255)
    /// ```
    ///
    /// With the default range 0 to 255 the level equals the component.
    RgbByte,
}

impl LevelRule {
    /// The rule for samples of type `dtype`. `rgb` says whether they are a
    /// component of an RGB image, which only matters for unsigned 8-bit.
    pub fn for_dtype(dtype: DType, rgb: bool) -> Result<LevelRule> {
        Ok(match dtype {
            DType::C32 | DType::C64 => return Err(complex_error()),
            DType::U8 if rgb => LevelRule::RgbByte,
            DType::U8 => LevelRule::Byte,
            DType::F32 | DType::F64 => LevelRule::Float,
            _ => LevelRule::Integer,
        })
    }
}

pub(crate) fn complex_error() -> Error {
    Error::Unsupported("complex images cannot be displayed directly".to_string())
}

/// A [`LevelRule`] with its display range: the complete mapping from a
/// sample value to a display level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelMap {
    rule: LevelRule,
    /// The range after the rounding the rule prescribes (see `new`).
    min: f64,
    max: f64,
    /// `Byte` and `RgbByte` only: the minimum used in the subtraction.
    offset: f64,
}

impl LevelMap {
    /// The mapping for samples of type `dtype` shown with display range
    /// `(min, max)`. `rgb` is as for [`LevelRule::for_dtype`].
    pub fn new(dtype: DType, rgb: bool, (min, max): (f64, f64)) -> Result<LevelMap> {
        let rule = LevelRule::for_dtype(dtype, rgb)?;
        // Math.round, which rounds halves up.
        let round = |v: f64| (v + 0.5).floor();
        Ok(match rule {
            LevelRule::Byte => LevelMap {
                rule,
                min,
                max,
                offset: round(min),
            },
            LevelRule::RgbByte => LevelMap {
                rule,
                min,
                max,
                offset: min.trunc(),
            },
            LevelRule::Integer => LevelMap {
                rule,
                min: round(min),
                max: round(max),
                offset: 0.0,
            },
            LevelRule::Float if dtype == DType::F32 => LevelMap {
                rule,
                min: min as f32 as f64,
                max: max as f32 as f64,
                offset: 0.0,
            },
            LevelRule::Float => LevelMap {
                rule,
                min,
                max,
                offset: 0.0,
            },
        })
    }

    pub fn rule(&self) -> LevelRule {
        self.rule
    }

    /// The display range the formula uses: `(min2, max2)` for
    /// [`LevelRule::Integer`], the single-precision values for 32-bit
    /// floats, and otherwise the range as given.
    pub fn range(&self) -> (f64, f64) {
        (self.min, self.max)
    }

    /// The display level of one sample value.
    pub fn level(&self, v: f64) -> u8 {
        let (min, max) = (self.min, self.max);
        // Also catches NaN in either bound.
        if max.partial_cmp(&min).is_none_or(|o| o.is_lt()) {
            return 0;
        }
        match self.rule {
            LevelRule::Byte => {
                if v < min {
                    0
                } else if v > max {
                    255
                } else {
                    ((256.0 * (v - self.offset) / (max - min)) as i32).clamp(0, 255) as u8
                }
            }
            LevelRule::RgbByte => {
                ((256.0 * (v - self.offset) / (max - min)) as i32).clamp(0, 255) as u8
            }
            LevelRule::Integer => {
                let value = (v - min).max(0.0);
                ((value * (256.0 / (max - min + 1.0)) + 0.5) as i32).min(255) as u8
            }
            LevelRule::Float => {
                let value = v - min;
                let value = if value < 0.0 { 0.0 } else { value };
                ((value * (255.0 / (max - min)) + 0.5) as i32).min(255) as u8
            }
        }
    }

    /// The level of each possible 8-bit sample, for the two 8-bit rules.
    pub fn table(&self) -> Option<[u8; 256]> {
        matches!(self.rule, LevelRule::Byte | LevelRule::RgbByte)
            .then(|| std::array::from_fn(|v| self.level(v as f64)))
    }

    /// The display level of every sample of `plane`.
    pub fn levels(&self, plane: &Buffer) -> Result<Vec<u8>> {
        fn each<T: RealPixel>(map: &LevelMap, samples: &[T]) -> Vec<u8> {
            samples.iter().map(|v| map.level(v.to_f64())).collect()
        }
        match plane {
            Buffer::U8(samples) => {
                let table: [u8; 256] = std::array::from_fn(|v| self.level(v as f64));
                Ok(samples.iter().map(|&v| table[v as usize]).collect())
            }
            // A table pays for itself once there are more samples than entries.
            Buffer::U16(samples) if samples.len() > 1 << 16 => {
                let table: Vec<u8> = (0..=u16::MAX).map(|v| self.level(v as f64)).collect();
                Ok(samples.iter().map(|&v| table[v as usize]).collect())
            }
            _ => dispatch_real!(plane, |s| Ok(each(self, s)), complex => Err(complex_error())),
        }
    }
}

/// The display range used when an image does not specify one, and what the
/// B&C "Reset" button restores: the full range 0 to 255 for unsigned 8-bit
/// data (`ByteProcessor.resetMinAndMax`), and the smallest and largest
/// non-NaN samples of the plane otherwise (`ShortProcessor.findMinAndMax`,
/// `FloatProcessor.findMinAndMax`). Infinite samples count, so a plane
/// containing one gets an infinite range and displays as level 0 throughout,
/// as in ImageJ. A plane of only NaN gives `(0, 0)`.
pub fn default_display_range(plane: &Buffer) -> Result<(f64, f64)> {
    if plane.dtype() == DType::U8 {
        return Ok((0.0, 255.0));
    }
    fn range<T: RealPixel>(samples: &[T]) -> (f64, f64) {
        let mut range: Option<(f64, f64)> = None;
        for v in samples.iter().map(|v| v.to_f64()).filter(|v| !v.is_nan()) {
            range = Some(match range {
                None => (v, v),
                Some((lo, hi)) => (lo.min(v), hi.max(v)),
            });
        }
        range.unwrap_or((0.0, 0.0))
    }
    dispatch_real!(plane, |s| Ok(range(s)), complex => Err(complex_error()))
}

/// Maps every sample to a display level from 0 to 255 for the given display
/// range, using the [`LevelRule`] of the plane's pixel type (not the RGB
/// rule; see [`LevelMap::new`] for that).
pub fn to_levels(plane: &Buffer, range: (f64, f64)) -> Result<Vec<u8>> {
    LevelMap::new(plane.dtype(), false, range)?.levels(plane)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels<T: imager_core::Pixel>(samples: Vec<T>, range: (f64, f64)) -> Vec<u8> {
        to_levels(&Buffer::from_vec(samples), range).unwrap()
    }

    #[test]
    fn eight_bit_default_is_identity() {
        let plane = Buffer::from_vec((0..=255u8).collect());
        let range = default_display_range(&plane).unwrap();
        assert_eq!(range, (0.0, 255.0));
        let levels = to_levels(&plane, range).unwrap();
        assert!(levels.iter().enumerate().all(|(i, &l)| l as usize == i));
    }

    #[test]
    fn eight_bit_range_rescales_the_lut() {
        // ByteProcessor.setMinAndMax(50, 200): index = (int)(256 * (i - 50) / 150).
        // 51 -> 1.70 -> 1; 125 -> 128; 199 -> 254.29 -> 254; 200 -> 256 -> 255.
        assert_eq!(
            levels(
                vec![0u8, 49, 50, 51, 125, 199, 200, 201, 255],
                (50.0, 200.0)
            ),
            [0, 0, 0, 1, 128, 254, 255, 255, 255]
        );
        // The comparisons use the unrounded range and the subtraction the
        // rounded minimum: (10.4, 99.6) has rmin = 10 and divisor 89.2.
        // 10 < 10.4 -> 0; 11 -> (int)(256 * 1 / 89.2) = 2; 99 -> (int)255.4;
        // 100 > 99.6 -> 255.
        assert_eq!(
            levels(vec![10u8, 11, 99, 100], (10.4, 99.6)),
            [0, 2, 255, 255]
        );
        // (10.5, 99.5): rmin = 11, so 11 -> 0 and 12 -> (int)(256 / 89) = 2.
        assert_eq!(levels(vec![10u8, 11, 12], (10.5, 99.5)), [0, 0, 2]);
        // A range wider than the type: (-20, 300), divisor 320.
        // 0 -> (int)(256 * 20 / 320) = 16; 255 -> (int)220 = 220.
        assert_eq!(levels(vec![0u8, 255], (-20.0, 300.0)), [16, 220]);
    }

    #[test]
    fn eight_bit_degenerate_ranges() {
        // min == max: below is 0, above is 255, equal is (int)(0/0) = 0.
        assert_eq!(levels(vec![99u8, 100, 101], (100.0, 100.0)), [0, 0, 255]);
        // Unlike 16-bit and float, (0, 0) is not "reset" for 8-bit data.
        assert_eq!(levels(vec![0u8, 1, 255], (0.0, 0.0)), [0, 255, 255]);
        // max < min shows nothing.
        assert_eq!(levels(vec![0u8, 100, 255], (200.0, 100.0)), [0, 0, 0]);
        assert_eq!(levels(vec![0u8, 255], (f64::NAN, 100.0)), [0, 0]);
    }

    #[test]
    fn sixteen_bit_follows_imagej_scaling() {
        // ShortProcessor: scale = 256 / (max - min + 1), level = (int)(v * scale + 0.5).
        assert_eq!(
            levels(vec![100u16, 101, 227, 228, 355, 50, 400], (100.0, 355.0)),
            [0, 1, 127, 128, 255, 0, 255]
        );
        // A range wider than 256 values: scale = 256 / 1001.
        assert_eq!(levels(vec![0u16, 500, 1000], (0.0, 1000.0)), [0, 128, 255]);
        // The range is rounded half up, not truncated: (99.5, 354.5) is (100, 355).
        assert_eq!(
            levels(vec![100u16, 101, 227, 228, 355], (99.5, 354.5)),
            [0, 1, 127, 128, 255]
        );
        // min == max: scale = 256, so anything above saturates.
        assert_eq!(levels(vec![599u16, 600, 601], (600.0, 600.0)), [0, 0, 255]);
        assert_eq!(levels(vec![1u16, 2], (5.0, 3.0)), [0, 0]);
    }

    #[test]
    fn other_integers_follow_the_sixteen_bit_rule() {
        // (-100, 155) spans 256 values: level = v + 100.
        assert_eq!(
            levels(vec![-200i16, -100, -99, 0, 155, 300], (-100.0, 155.0)),
            [0, 0, 1, 100, 255, 255]
        );
        assert_eq!(levels(vec![-128i8, 0, 127], (-128.0, 127.0)), [0, 128, 255]);
        // scale = 256 / 2^32; 2^31 * scale + 0.5 = 128.5.
        assert_eq!(
            levels(vec![0u32, 1 << 31, u32::MAX], (0.0, u32::MAX as f64)),
            [0, 128, 255]
        );
        assert_eq!(
            levels(vec![i64::MIN, 0, i64::MAX], (-1e6, 1e6)),
            [0, 128, 255]
        );
        // Negative halves round up, as Math.round does: -2.5 -> -2.
        let map = LevelMap::new(DType::I32, false, (-2.5, 2.5)).unwrap();
        assert_eq!(map.range(), (-2.0, 3.0));
    }

    #[test]
    fn float_follows_imagej_scaling() {
        // FloatProcessor: scale = 255 / (max - min), level = (int)(v * scale + 0.5).
        assert_eq!(
            levels(
                vec![
                    -1.0f32,
                    0.0,
                    0.5,
                    1.0,
                    2.0,
                    f32::NAN,
                    f32::INFINITY,
                    f32::NEG_INFINITY
                ],
                (0.0, 1.0)
            ),
            [0, 0, 128, 255, 255, 0, 255, 0]
        );
        // A flat range: nothing at or below it shows, anything above saturates.
        assert_eq!(levels(vec![3.0f32, 2.0, 3.5], (3.0, 3.0)), [0, 0, 255]);
        // An infinite range has scale 0: everything is level 0, or NaN -> 0.
        assert_eq!(
            levels(
                vec![0.0f32, 1e30, f32::INFINITY],
                (f64::NEG_INFINITY, f64::INFINITY)
            ),
            [0, 0, 0]
        );
        assert_eq!(levels(vec![0.25f64, 0.75], (0.0, 1.0)), [64, 191]);
    }

    #[test]
    fn float_range_is_single_precision_for_f32_only() {
        let narrow = LevelMap::new(DType::F32, false, (0.1, 0.7)).unwrap();
        assert_eq!(narrow.range(), (0.1f32 as f64, 0.7f32 as f64));
        let wide = LevelMap::new(DType::F64, false, (0.1, 0.7)).unwrap();
        assert_eq!(wide.range(), (0.1, 0.7));
        // 0.1f32 is slightly above 0.1, so the f32 sample 0.1 sits exactly on
        // the minimum.
        assert_eq!(narrow.level(0.1f32 as f64), 0);
    }

    #[test]
    fn rgb_components_use_the_color_processor_table() {
        let level = |v: u8, range| {
            LevelMap::new(DType::U8, true, range)
                .unwrap()
                .level(v as f64)
        };
        assert!((0..=255u8).all(|v| level(v, (0.0, 255.0)) == v));
        // (int)(256 * (v - 50) / 150), clamped.
        assert_eq!(level(49, (50.0, 200.0)), 0);
        assert_eq!(level(51, (50.0, 200.0)), 1);
        assert_eq!(level(199, (50.0, 200.0)), 254);
        assert_eq!(level(200, (50.0, 200.0)), 255);
        // The minimum is truncated, not rounded: (40.7, 180.2) subtracts 40
        // and divides by 139.5. 41 -> (int)1.83 = 1.
        assert_eq!(level(41, (40.7, 180.2)), 1);
        assert_eq!(level(40, (40.7, 180.2)), 0);
        // min == max: 0/0 -> 0, positive/0 -> 255.
        assert_eq!(level(100, (100.0, 100.0)), 0);
        assert_eq!(level(101, (100.0, 100.0)), 255);
        assert_eq!(level(99, (100.0, 100.0)), 0);
        let map = LevelMap::new(DType::U8, true, (50.0, 200.0)).unwrap();
        assert_eq!(map.table().unwrap()[125], 128);
        assert!(
            LevelMap::new(DType::U16, true, (0.0, 1.0))
                .unwrap()
                .table()
                .is_none()
        );
    }

    #[test]
    fn large_sixteen_bit_planes_use_a_table() {
        let samples: Vec<u16> = (0..70_000u32).map(|i| (i % 65_536) as u16).collect();
        let map = LevelMap::new(DType::U16, false, (1000.0, 5000.0)).unwrap();
        let levels = map.levels(&Buffer::from_vec(samples.clone())).unwrap();
        assert!(
            samples
                .iter()
                .zip(&levels)
                .all(|(&v, &l)| l == map.level(v as f64))
        );
    }

    #[test]
    fn default_range_ignores_nan() {
        let plane = Buffer::from_vec(vec![f32::NAN, 2.0, -3.0]);
        assert_eq!(default_display_range(&plane).unwrap(), (-3.0, 2.0));
        let plane = Buffer::from_vec(vec![7u16, 900]);
        assert_eq!(default_display_range(&plane).unwrap(), (7.0, 900.0));
        let plane = Buffer::from_vec(vec![f64::NAN; 3]);
        assert_eq!(default_display_range(&plane).unwrap(), (0.0, 0.0));
        let plane = Buffer::from_vec(vec![1.0f32, f32::INFINITY]);
        assert_eq!(default_display_range(&plane).unwrap(), (1.0, f64::INFINITY));
        let plane = Buffer::from_vec(vec![-5i8, 9]);
        assert_eq!(default_display_range(&plane).unwrap(), (-5.0, 9.0));
    }

    #[test]
    fn complex_samples_are_rejected() {
        let plane = Buffer::zeros(DType::C32, 2);
        assert!(to_levels(&plane, (0.0, 1.0)).is_err());
        assert!(default_display_range(&plane).is_err());
    }
}
