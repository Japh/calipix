//! The best-fitting ellipse of a region (`ij.process.EllipseFitter`).

use calipix_core::Rect;

/// The ellipse with the same area and second-order moments as a region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipse {
    /// Centre, in pixel coordinates.
    pub x_center: f64,
    pub y_center: f64,
    /// Lengths of the axes, in pixels.
    pub major: f64,
    pub minor: f64,
    /// Angle between the major axis and the X axis, in degrees,
    /// counter-clockwise, from 0 up to 180.
    pub angle: f64,
}

/// `EllipseFitter.fit` and `getEllipseParam`: the ellipse fitted to the
/// pixels of `rect` that `mask` selects (one byte per pixel, non-zero
/// inside), or to the whole rectangle without a mask.
///
/// ImageJ's constants are kept as written there (a truncated square root
/// of pi, 0.08333333 for one twelfth), because they show in the results.
pub fn fit_ellipse(rect: Rect, mask: Option<&[u8]>) -> Ellipse {
    const HALF_PI: f64 = 1.5707963267949;
    let (left, top) = (rect.x as f64, rect.y as f64);
    let (width, height) = (rect.w, rect.h);
    let Some(mask) = mask else {
        let sqrt_pi = 1.772453851;
        let mut major = (width as f64 * 2.0) / sqrt_pi;
        let mut minor = (height as f64 * 2.0) / sqrt_pi;
        let mut angle = 0.0;
        if major < minor {
            std::mem::swap(&mut major, &mut minor);
            angle = 90.0;
        }
        return Ellipse {
            x_center: left + width as f64 / 2.0,
            y_center: top + height as f64 / 2.0,
            major,
            minor,
            angle,
        };
    };

    // computeSums
    let (mut xsum, mut ysum, mut x2sum, mut y2sum, mut xysum) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut bit_count = 0u64;
    for y in 0..height {
        let mut line_count = 0u64;
        let mut line_xsum = 0u64;
        let row = &mask[y * width..(y + 1) * width];
        for (x, _) in row.iter().enumerate().filter(|(_, m)| **m != 0) {
            line_count += 1;
            line_xsum += x as u64;
            x2sum += (x * x) as f64;
        }
        xsum += line_xsum as f64;
        ysum += (line_count * y as u64) as f64;
        xysum += line_xsum as f64 * y as f64;
        y2sum += (y * y) as f64 * line_count as f64;
        bit_count += line_count;
    }

    // getMoments; an empty mask leaves every moment at zero.
    let n = bit_count as f64;
    let (mut xm, mut ym, mut u20, mut u02, mut u11) = (0.0, 0.0, 0.0, 0.0, 0.0);
    if bit_count != 0 {
        x2sum += 0.08333333 * n;
        y2sum += 0.08333333 * n;
        let (x1, y1) = (xsum / n, ysum / n);
        xm = x1;
        ym = y1;
        u20 = x2sum / n - x1 * x1;
        u02 = y2sum / n - y1 * y1;
        u11 = xysum / n - x1 * y1;
    }

    let mut m4 = 4.0 * (u02 * u20 - u11 * u11).abs();
    if m4 < 0.000001 {
        m4 = 0.000001;
    }
    let mut a11 = u02 / m4;
    let a12 = u11 / m4;
    let mut a22 = u20 / m4;

    let mut tmp = a11 - a22;
    if tmp == 0.0 {
        tmp = 0.000001;
    }
    let mut theta = 0.5 * (2.0 * a12 / tmp).atan();
    if theta < 0.0 {
        theta += HALF_PI;
    }
    if a12 > 0.0 {
        theta += HALF_PI;
    } else if a12 == 0.0 {
        if a22 > a11 {
            theta = 0.0;
            std::mem::swap(&mut a11, &mut a22);
        } else if a11 != a22 {
            theta = HALF_PI;
        }
    }
    tmp = theta.sin();
    if tmp == 0.0 {
        tmp = 0.000001;
    }
    let z = a12 * theta.cos() / tmp;
    let mut major = (1.0 / (a22 + z).abs()).sqrt();
    let mut minor = (1.0 / (a11 - z).abs()).sqrt();
    // Scale so that the ellipse has the area of the region.
    let scale = (n / (std::f64::consts::PI * major * minor)).sqrt();
    major = major * scale * 2.0;
    minor = minor * scale * 2.0;
    let mut angle = 180.0 * theta / std::f64::consts::PI;
    if angle == 180.0 {
        angle = 0.0;
    }
    if major < minor {
        std::mem::swap(&mut major, &mut minor);
    }
    Ellipse {
        x_center: left + xm + 0.5,
        y_center: top + ym + 0.5,
        major,
        minor,
        angle,
    }
}
