//! Scan-line polygon filling, ported from ImageJ's
//! `ij.process.PolygonFiller`.
//!
//! A pixel is filled when its centre lies inside the polygon by the
//! even-odd rule. Centres exactly on a left boundary are outside, on a
//! right boundary inside; on a horizontal boundary they are outside at the
//! top and inside at the bottom. The x positions of the edges are advanced
//! from row to row by repeated addition, exactly as ImageJ does, because
//! the rounding that accumulates decides borderline pixels.

/// The polygon to fill, in the coordinates of the mask being produced.
pub(crate) enum Polygon<'a> {
    /// `PolygonFiller.setPolygon(int[], int[], int)`.
    Int { x: &'a [i32], y: &'a [i32] },
    /// `PolygonFiller.setPolygon(float[], float[], int, double, double)`.
    /// Several polygons may follow each other, separated by NaN.
    Float {
        x: &'a [f32],
        y: &'a [f32],
        x_offset: f64,
        y_offset: f64,
    },
}

/// Rows above the window that are stepped through one by one, as ImageJ
/// would, before the filler jumps ahead. A selection that starts more
/// rows than this above the top of the image is not a real one.
const FAST_FORWARD_ROWS: i64 = 1 << 16;

/// The part of the full mask to produce.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Window {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

struct Edges {
    ex: Vec<f64>,
    ey1: Vec<i32>,
    ey2: Vec<i32>,
    slope: Vec<f64>,
    y_min: i32,
    y_max: i32,
}

/// `PolygonFiller.buildEdgeTable`.
fn build_edge_table(polygon: &Polygon<'_>) -> Edges {
    let mut e = Edges {
        ex: Vec::new(),
        ey1: Vec::new(),
        ey2: Vec::new(),
        slope: Vec::new(),
        y_min: i32::MAX,
        y_max: i32::MIN,
    };
    let mut push = |x: f64, y1: i32, y2: i32, slope: f64| {
        e.ex.push(x);
        e.ey1.push(y1);
        e.ey2.push(y2);
        e.slope.push(slope);
        e.y_min = e.y_min.min(y1);
        e.y_max = e.y_max.max(y2);
    };
    match *polygon {
        Polygon::Int { x, y } => {
            let n = x.len().min(y.len());
            for i in 0..n {
                let next = if i == n - 1 { 0 } else { i + 1 };
                let (mut y1, mut y2) = (y[i], y[next]);
                let (mut x1, mut x2) = (x[i], x[next]);
                if y1 == y2 {
                    continue; // horizontal lines contribute nothing
                }
                if y1 > y2 {
                    std::mem::swap(&mut y1, &mut y2);
                    std::mem::swap(&mut x1, &mut x2);
                }
                let slope = (f64::from(x2) - f64::from(x1)) / (f64::from(y2) - f64::from(y1));
                push(f64::from(x1) + 0.5 * slope + 1e-8, y1, y2, slope);
            }
        }
        Polygon::Float {
            x,
            y,
            x_offset,
            y_offset,
        } => {
            let n = x.len().min(y.len());
            let mut poly_start = 0;
            for i in 0..n {
                let mut next = if i == n - 1 { poly_start } else { i + 1 };
                if x[next].is_nan() {
                    next = poly_start; // after the last point, close the polygon
                }
                if x[i].is_nan() {
                    poly_start = i + 1;
                    continue;
                }
                let (mut y1f, mut y2f) = (f64::from(y[i]) + y_offset, f64::from(y[next]) + y_offset);
                let (mut x1f, mut x2f) = (f64::from(x[i]) + x_offset, f64::from(x[next]) + x_offset);
                let mut y1 = java_round(y1f);
                let mut y2 = java_round(y2f);
                if y1 == y2 || (y1 <= 0 && y2 <= 0) {
                    continue; // horizontal, or never reaches the first row
                }
                if y1 > y2 {
                    std::mem::swap(&mut y1, &mut y2);
                    std::mem::swap(&mut y1f, &mut y2f);
                    std::mem::swap(&mut x1f, &mut x2f);
                }
                let slope = (x2f - x1f) / (y2f - y1f);
                push(
                    x1f + (f64::from(y1) - y1f + 0.5) * slope + 1e-8,
                    y1,
                    y2,
                    slope,
                );
            }
        }
    }
    e
}

/// Java's `(int)Math.round(double)`: NaN gives 0 and out-of-range values
/// saturate at the `long` range before being narrowed.
fn java_round(v: f64) -> i32 {
    if v.is_nan() {
        return 0;
    }
    ((v + 0.5).floor() as i64) as i32
}

/// The active edge list. ImageJ keeps it in a fixed array whose unused
/// tail holds stale entries; an odd number of active edges (possible only
/// for degenerate input) makes it read one of them, which is reproduced.
struct Active {
    list: Vec<usize>,
    count: usize,
}

impl Active {
    fn sort(&mut self, ex: &[f64]) {
        // The selection sort of PolygonFiller.sortActiveEdges.
        for i in 0..self.count {
            let mut min = i;
            for j in i..self.count {
                if ex[self.list[j]] < ex[self.list[min]] {
                    min = j;
                }
            }
            self.list.swap(min, i);
        }
    }
}

/// Fills `polygon` into `out`, which holds the `window` part of a mask of
/// `width` x `height` pixels, row-major. Filled pixels are set to 255;
/// others are left as they are, so several fills accumulate.
///
/// `PolygonFiller.fillByteProcessorMask`.
pub(crate) fn fill(polygon: &Polygon<'_>, width: i32, height: i32, window: Window, out: &mut [u8]) {
    let mut e = build_edge_table(polygon);
    let edges = e.ex.len();
    if edges == 0 || window.w <= 0 || window.h <= 0 || width <= 0 || height <= 0 {
        return;
    }
    let mut active = Active {
        list: vec![0; edges + 1],
        count: 0,
    };
    let mut y_start = e.y_min.max(0);
    if i64::from(window.y) - i64::from(y_start) > FAST_FORWARD_ROWS {
        // The window lies far below the top of the polygon (which then
        // reaches far outside any image): start at the window, placing
        // the edges as ImageJ does when a polygon starts above row 0,
        // instead of stepping through every row above it.
        y_start = window.y;
    }
    if e.y_min != 0 || y_start > 0 {
        // PolygonFiller.shiftXValuesAndActivate
        for i in 0..edges {
            if e.ey1[i] < y_start && e.ey2[i] >= y_start {
                e.ex[i] += e.slope[i] * (f64::from(y_start) - f64::from(e.ey1[i]));
                active.list[active.count] = i;
                active.count += 1;
            }
        }
        active.sort(&e.ex);
    }
    let y_end = height
        .min(e.y_max.saturating_add(1))
        .min(window.y.saturating_add(window.h));
    let (wx0, wx1) = (window.x, window.x.saturating_add(window.w));
    let clamp = |ex: f64| ((ex + 0.5) as i32).clamp(0, width);
    for y in y_start..y_end {
        // removeInactiveEdges
        let mut i = 0;
        while i < active.count {
            let index = active.list[i];
            if y < e.ey1[index] || y >= e.ey2[index] {
                active.list.copy_within(i + 1..active.count, i);
                active.count -= 1;
            } else {
                i += 1;
            }
        }
        // activateEdges
        for edge in 0..edges {
            if y == e.ey1[edge] {
                let mut index = 0;
                while index < active.count && e.ex[edge] > e.ex[active.list[index]] {
                    index += 1;
                }
                active.list.copy_within(index..active.count, index + 1);
                active.list[index] = edge;
                active.count += 1;
            }
        }
        if y >= window.y {
            let row = ((y - window.y) as usize) * window.w as usize;
            let mut i = 0;
            while i < active.count {
                let x1 = clamp(e.ex[active.list[i]]).max(wx0);
                let x2 = clamp(e.ex[active.list[i + 1]]).min(wx1);
                if x2 > x1 {
                    let from = row + (x1 - wx0) as usize;
                    out[from..from + (x2 - x1) as usize].fill(255);
                }
                i += 2;
            }
        }
        // updateXCoordinates
        let mut previous = -f64::MAX;
        let mut sorted = true;
        for i in 0..active.count {
            let index = active.list[i];
            let x = e.ex[index] + e.slope[index];
            e.ex[index] = x;
            if x < previous {
                sorted = false;
            }
            previous = x;
        }
        if !sorted {
            active.sort(&e.ex);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_int(x: &[i32], y: &[i32], w: i32, h: i32) -> Vec<u8> {
        let mut out = vec![0; (w * h) as usize];
        let window = Window { x: 0, y: 0, w, h };
        fill(&Polygon::Int { x, y }, w, h, window, &mut out);
        out
    }

    #[test]
    fn a_square_fills_its_pixels() {
        let mask = fill_int(&[1, 4, 4, 1], &[1, 1, 3, 3], 5, 5);
        let count = mask.iter().filter(|&&v| v != 0).count();
        assert_eq!(count, 6);
        assert_eq!(mask[5 + 1], 255);
        assert_eq!(mask[5 + 4], 0);
    }

    #[test]
    fn a_window_equals_the_same_part_of_the_whole() {
        let (x, y) = ([0, 9, 4, 7, 1], [0, 2, 9, 5, 8]);
        let whole = fill_int(&x, &y, 9, 9);
        let window = Window {
            x: 2,
            y: 3,
            w: 5,
            h: 4,
        };
        let mut part = vec![0; 20];
        fill(&Polygon::Int { x: &x, y: &y }, 9, 9, window, &mut part);
        for row in 0..4 {
            for col in 0..5 {
                assert_eq!(part[row * 5 + col], whole[(row + 3) * 9 + col + 2]);
            }
        }
    }

    #[test]
    fn degenerate_input_does_not_panic() {
        fill_int(&[], &[], 4, 4);
        fill_int(&[1], &[1], 4, 4);
        fill_int(&[0, 3], &[0, 3], 4, 4);
        let (x, y) = ([f32::NAN, 1.0, f32::NAN, 2.0], [0.0, f32::NAN, 3.0, 9.0]);
        let mut out = vec![0; 16];
        let window = Window {
            x: 0,
            y: 0,
            w: 4,
            h: 4,
        };
        let polygon = Polygon::Float {
            x: &x,
            y: &y,
            x_offset: 0.0,
            y_offset: 0.0,
        };
        fill(&polygon, 4, 4, window, &mut out);
    }
}
