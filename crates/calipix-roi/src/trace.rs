//! Turning a mask into outlines along pixel edges: ImageJ's
//! `ThresholdToSelection`, which makes the selections of Create Selection,
//! Enlarge and Line to Area.

use std::collections::VecDeque;

/// An outline under construction: ImageJ's `ThresholdToSelection.Outline`,
/// a list of corner points that grows at both ends and drops points lying
/// on a straight line.
#[derive(Default)]
struct Outline {
    points: VecDeque<(i32, i32)>,
}

fn collinear(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> bool {
    (b.0 - a.0) * (c.1 - b.1) == (b.1 - a.1) * (c.0 - b.0)
}

impl Outline {
    fn append(&mut self, x: i32, y: i32) {
        let n = self.points.len();
        if n >= 2 && collinear(self.points[n - 2], self.points[n - 1], (x, y)) {
            self.points[n - 1] = (x, y);
        } else {
            self.points.push_back((x, y));
        }
    }

    fn prepend(&mut self, x: i32, y: i32) {
        if self.points.len() >= 2 && collinear(self.points[1], self.points[0], (x, y)) {
            self.points[0] = (x, y);
        } else {
            self.points.push_front((x, y));
        }
    }

    /// `Outline.getPolygon`: the closed polygon, without the points that
    /// merging outlines left on straight edges.
    fn polygon(self) -> Vec<(i32, i32)> {
        let mut p: Vec<(i32, i32)> = self.points.into();
        if p.len() < 3 {
            return p;
        }
        let mut first = 0;
        let mut last = p.len();
        let mut i = 1;
        let mut j = 1;
        while i + 1 < last {
            if collinear(p[j - 1], p[j], p[j + 1]) {
                last -= 1;
            } else {
                if i != j {
                    p[i] = p[j];
                }
                i += 1;
            }
            j += 1;
        }
        if collinear(p[j - 1], p[j], p[first]) {
            last -= 1;
        } else {
            p[i] = p[j];
        }
        if last - first > 2 && collinear(p[last - 1], p[first], p[first + 1]) {
            first += 1;
        }
        p[first..last].to_vec()
    }
}

/// The outlines being built, addressed by number so that two corners can
/// hold the same one.
#[derive(Default)]
struct Outlines {
    all: Vec<Option<Outline>>,
}

impl Outlines {
    fn start(&mut self) -> usize {
        self.all.push(Some(Outline::default()));
        self.all.len() - 1
    }

    fn at(&mut self, id: Option<usize>) -> &mut Outline {
        self.all[id.expect("an outline is open at this corner")]
            .as_mut()
            .expect("the outline has not been closed")
    }

    fn close(&mut self, id: Option<usize>) -> Vec<(i32, i32)> {
        self.all[id.expect("an outline is open at this corner")]
            .take()
            .expect("the outline has not been closed")
            .polygon()
    }

    /// Joins outline `tail` to the end of outline `head`; `head` keeps
    /// its number.
    fn join(&mut self, head: usize, tail: usize, keep: usize) {
        let mut points = self.all[head].take().expect("open").points;
        points.extend(self.all[tail].take().expect("open").points);
        self.all[keep] = Some(Outline { points });
    }
}

/// Traces the boundaries between selected and unselected pixels of a mask
/// into closed polygons along pixel edges, outer boundaries and holes
/// alike: ImageJ's `ThresholdToSelection.getRoi`. Filling the result by
/// the even-odd rule gives back the mask.
///
/// `data` is `width` x `height`, row-major, non-zero meaning selected;
/// the polygons are offset by `(x0, y0)`. All outlines are built at once,
/// row by row from the top, exactly as ImageJ builds them, so that the
/// polygons have ImageJ's vertex order and starting points (the perimeter
/// ImageJ reports for a traced outline depends on where it starts).
pub(crate) fn trace_mask(
    data: &[u8],
    width: usize,
    height: usize,
    x0: i32,
    y0: i32,
) -> Vec<Vec<(i32, i32)>> {
    let (w, h) = (width, height);
    let selected = |x: usize, y: usize| data[y * w + x] != 0;
    let mut polygons: Vec<Vec<(i32, i32)>> = Vec::new();
    let mut outlines = Outlines::default();
    // outline[x] is the unclosed outline at the top-left corner of pixel
    // (x, y); outline[x + 1] the one at its top-right corner.
    let mut outline: Vec<Option<usize>> = vec![None; w + 2];
    let mut prev_row = vec![false; w + 2];
    let mut this_row = vec![false; w + 2];
    for y in 0..=h {
        std::mem::swap(&mut prev_row, &mut this_row);
        // Where two pixels touch only at a corner (north-west to
        // south-east), the outline continues round the corner instead of
        // closing: the column to resume at, and the outline to resume.
        let mut x_after_corner: Option<usize> = None;
        let mut o_after_corner: Option<usize> = None;
        this_row[1] = y < h && w > 0 && selected(0, y);
        for x in 0..=w {
            if y < h && x + 1 < w {
                this_row[x + 2] = selected(x + 1, y);
            } else if x + 1 < w {
                this_row[x + 2] = false;
            }
            let (xi, yi) = (x as i32, y as i32);
            let after_corner = x_after_corner == Some(x);
            let resume = |o: Option<usize>| if after_corner { o } else { None };
            let can_turn = x + 1 < w && y < h && !after_corner;
            if this_row[x + 1] {
                // Pixel (x, y) is selected.
                if !prev_row[x + 1] {
                    // Upper edge of a selected area.
                    if outline[x].is_none() {
                        if outline[x + 1].is_none() {
                            let id = Some(outlines.start());
                            outline[x] = id;
                            outline[x + 1] = id;
                            outlines.at(id).append(xi + 1, yi);
                            outlines.at(id).append(xi, yi);
                        } else {
                            outline[x] = outline[x + 1].take();
                            outlines.at(outline[x]).append(xi, yi);
                        }
                    } else if outline[x + 1].is_none() {
                        if after_corner {
                            outline[x + 1] = outline[x];
                            outline[x] = o_after_corner;
                            outlines.at(outline[x]).append(xi, yi);
                            outlines.at(outline[x + 1]).prepend(xi + 1, yi);
                        } else {
                            outline[x + 1] = outline[x].take();
                            outlines.at(outline[x + 1]).prepend(xi + 1, yi);
                        }
                    } else if outline[x + 1] == outline[x] {
                        if can_turn && !this_row[x + 2] && prev_row[x + 2] {
                            outline[x] = None;
                            outlines.at(outline[x + 1]).prepend(xi + 1, yi);
                            x_after_corner = Some(x + 1);
                            o_after_corner = outline[x + 1];
                        } else {
                            // The end of a hole above.
                            polygons.push(outlines.close(outline[x]));
                            outline[x + 1] = None;
                            outline[x] = resume(o_after_corner);
                        }
                    } else {
                        // Two outlines meet: the right one goes in front
                        // of the left one.
                        let left = outline[x].expect("checked above");
                        let right = outline[x + 1].expect("checked above");
                        outlines.join(right, left, left);
                        if let Some(x1) =
                            (0..=w).find(|&x1| x1 != x + 1 && outline[x1] == Some(right))
                        {
                            outline[x1] = Some(left);
                            outline[x + 1] = None;
                            outline[x] = resume(o_after_corner);
                        }
                        if o_after_corner == Some(right) {
                            o_after_corner = Some(left);
                        }
                    }
                }
                if !this_row[x] {
                    // Left edge.
                    outlines.at(outline[x]).append(xi, yi + 1);
                }
            } else {
                // Pixel (x, y) is not selected.
                if prev_row[x + 1] {
                    // Lower edge of a selected area.
                    if outline[x].is_none() {
                        if outline[x + 1].is_none() {
                            let id = Some(outlines.start());
                            outline[x] = id;
                            outline[x + 1] = id;
                            outlines.at(id).append(xi, yi);
                            outlines.at(id).append(xi + 1, yi);
                        } else {
                            outline[x] = outline[x + 1].take();
                            outlines.at(outline[x]).prepend(xi, yi);
                        }
                    } else if outline[x + 1].is_none() {
                        if after_corner {
                            outline[x + 1] = outline[x];
                            outline[x] = o_after_corner;
                            outlines.at(outline[x]).prepend(xi, yi);
                            outlines.at(outline[x + 1]).append(xi + 1, yi);
                        } else {
                            outline[x + 1] = outline[x].take();
                            outlines.at(outline[x + 1]).append(xi + 1, yi);
                        }
                    } else if outline[x + 1] == outline[x] {
                        if can_turn && this_row[x + 2] && !prev_row[x + 2] {
                            outline[x] = None;
                            outlines.at(outline[x + 1]).append(xi + 1, yi);
                            x_after_corner = Some(x + 1);
                            o_after_corner = outline[x + 1];
                        } else {
                            // A filled outline is complete.
                            polygons.push(outlines.close(outline[x]));
                            outline[x + 1] = None;
                            outline[x] = resume(o_after_corner);
                        }
                    } else if can_turn && this_row[x + 2] && !prev_row[x + 2] {
                        outlines.at(outline[x]).append(xi + 1, yi);
                        outlines.at(outline[x + 1]).prepend(xi + 1, yi);
                        x_after_corner = Some(x + 1);
                        o_after_corner = outline[x];
                        outline[x] = None;
                    } else {
                        // Two outlines meet: the right one goes behind
                        // the left one.
                        let left = outline[x].expect("checked above");
                        let right = outline[x + 1].expect("checked above");
                        outlines.join(left, right, left);
                        if let Some(x1) =
                            (0..=w).find(|&x1| x1 != x + 1 && outline[x1] == Some(right))
                        {
                            outline[x1] = Some(left);
                            outline[x + 1] = None;
                            outline[x] = resume(o_after_corner);
                        }
                        if o_after_corner == Some(right) {
                            o_after_corner = Some(left);
                        }
                    }
                }
                if this_row[x] {
                    // Right edge.
                    outlines.at(outline[x]).prepend(xi, yi + 1);
                }
            }
        }
    }
    for polygon in &mut polygons {
        for p in polygon.iter_mut() {
            *p = (p.0 + x0, p.1 + y0);
        }
    }
    polygons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Path, Roi};

    fn retraced(rows: &[&[u8]]) -> (usize, Vec<u8>, Vec<u8>) {
        let (w, h) = (rows[0].len(), rows.len());
        let data: Vec<u8> = rows
            .iter()
            .flat_map(|r| r.iter().map(|&c| u8::from(c == b'#') * 255))
            .collect();
        let polygons = trace_mask(&data, w, h, 0, 0);
        let path = Path::from_polygons(
            polygons
                .iter()
                .map(|p| p.iter().map(|&(x, y)| (x as f32, y as f32))),
        );
        let mut full = vec![0u8; w * h];
        if let Some(mask) = (Roi::Composite { path }).mask(w, h) {
            for y in 0..mask.rect.h {
                for x in 0..mask.rect.w {
                    full[(y + mask.rect.y) * w + x + mask.rect.x] = mask.data[y * mask.rect.w + x];
                }
            }
        }
        (polygons.len(), data, full)
    }

    #[test]
    fn tracing_gives_back_the_mask() {
        // A ring with a hole and a pixel touching it at a corner, which
        // ImageJ joins to the ring's outline.
        let (count, data, full) = retraced(&[b"...#.", b"###..", b"#.#..", b"###..", b"....."]);
        assert_eq!(count, 2);
        assert_eq!(full, data);
        for rows in [
            &[b"#.#.#".as_slice(), b".#.#.", b"#.#.#", b".#.#.", b"#.#.#"][..],
            &[b"#####".as_slice(), b"#...#", b"#.#.#", b"#...#", b"#####"],
            &[b"..#..".as_slice(), b".#.#.", b"#...#", b".#.#.", b"..#.."],
            &[b"#".as_slice()],
            &[b"....".as_slice(), b"...."],
        ] {
            let (_, data, full) = retraced(rows);
            assert_eq!(full, data);
        }
    }

    #[test]
    fn a_square_starts_where_imagej_starts_it() {
        let data = vec![255u8; 6];
        assert_eq!(
            trace_mask(&data, 3, 2, 10, 20),
            [[(13, 22), (13, 20), (10, 20), (10, 22)]]
        );
    }
}
