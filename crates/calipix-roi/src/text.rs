//! The text form of a selection. The grammar is documented at the top of
//! the crate.

use std::fmt;
use std::str::FromStr;

use crate::path::{Path, Segment};
use crate::{Error, Point, Roi};

fn write_points(f: &mut fmt::Formatter<'_>, points: &[Point]) -> fmt::Result {
    for (x, y) in points {
        write!(f, " {x},{y}")?;
    }
    Ok(())
}

fn write_width(f: &mut fmt::Formatter<'_>, width: f64) -> fmt::Result {
    if width != 1.0 {
        write!(f, " width {width}")?;
    }
    Ok(())
}

impl fmt::Display for Roi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind().name())?;
        match self {
            Roi::Rectangle {
                x,
                y,
                width,
                height,
            }
            | Roi::Oval {
                x,
                y,
                width,
                height,
            } => write!(f, " {x} {y} {width} {height}"),
            Roi::RoundedRectangle {
                x,
                y,
                width,
                height,
                corner_diameter,
            } => write!(f, " {x} {y} {width} {height} {corner_diameter}"),
            Roi::Polygon { points }
            | Roi::Freehand { points }
            | Roi::Traced { points }
            | Roi::Points { points } => write_points(f, points),
            Roi::Angle { points } => write_points(f, points),
            Roi::Line {
                x1,
                y1,
                x2,
                y2,
                width,
            } => {
                write!(f, " {x1} {y1} {x2} {y2}")?;
                write_width(f, *width)
            }
            Roi::Polyline { points, width } | Roi::Freeline { points, width } => {
                write_points(f, points)?;
                write_width(f, *width)
            }
            Roi::Ellipse {
                x1,
                y1,
                x2,
                y2,
                aspect_ratio,
            } => write!(f, " {x1} {y1} {x2} {y2} {aspect_ratio}"),
            Roi::RotatedRectangle {
                x1,
                y1,
                x2,
                y2,
                width,
            } => write!(f, " {x1} {y1} {x2} {y2} {width}"),
            Roi::Composite { path } => {
                for s in path.segments() {
                    match *s {
                        Segment::MoveTo(x, y) => write!(f, " M {x},{y}")?,
                        Segment::LineTo(x, y) => write!(f, " L {x},{y}")?,
                        Segment::QuadTo(a, b, x, y) => write!(f, " Q {a},{b} {x},{y}")?,
                        Segment::CubicTo(a, b, c, d, x, y) => {
                            write!(f, " C {a},{b} {c},{d} {x},{y}")?
                        }
                        Segment::Close => f.write_str(" Z")?,
                    }
                }
                Ok(())
            }
        }
    }
}

fn number<T: FromStr + Into<f64> + Copy>(word: &str) -> Option<T> {
    word.parse::<T>().ok().filter(|v| (*v).into().is_finite())
}

fn point<T: FromStr + Into<f64> + Copy>(word: &str) -> Option<(T, T)> {
    let (x, y) = word.split_once(',')?;
    Some((number(x)?, number(y)?))
}

impl FromStr for Roi {
    type Err = Error;

    fn from_str(text: &str) -> Result<Roi, Error> {
        let fail = |reason: &str| Error::Parse {
            text: text.to_string(),
            reason: reason.to_string(),
        };
        let mut words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return Err(fail("it is empty"));
        }
        let kind = words.remove(0);

        // Shapes given by plain numbers.
        let numbers = |words: &[&str]| -> Result<Vec<f64>, Error> {
            words
                .iter()
                .map(|w| number::<f64>(w))
                .collect::<Option<Vec<f64>>>()
                .ok_or_else(|| fail("expected numbers after the shape"))
        };
        // Shapes given by points, with an optional trailing `width w`.
        let points = |words: &[&str]| -> Result<Vec<Point>, Error> {
            words
                .iter()
                .map(|w| point::<f64>(w))
                .collect::<Option<Vec<Point>>>()
                .ok_or_else(|| fail("expected points written as x,y"))
        };
        let split_width = |words: &mut Vec<&str>| -> Result<f64, Error> {
            let n = words.len();
            if n >= 2 && words[n - 2] == "width" {
                let width = number::<f64>(words[n - 1])
                    .filter(|w| *w > 0.0)
                    .ok_or_else(|| fail("the line width must be a positive number"))?;
                words.truncate(n - 2);
                Ok(width)
            } else {
                Ok(1.0)
            }
        };
        let at_least = |points: Vec<Point>, n: usize, what: &str| -> Result<Vec<Point>, Error> {
            if points.len() >= n {
                Ok(points)
            } else {
                Err(fail(&format!("{what} needs at least {n} points")))
            }
        };

        match kind {
            "rectangle" | "oval" => {
                let &[x, y, width, height] = numbers(&words)?.as_slice() else {
                    return Err(fail(&format!(
                        "{} takes x, y, width and height",
                        if kind == "oval" { "an oval" } else { "a rectangle" }
                    )));
                };
                if !(width > 0.0 && height > 0.0) {
                    return Err(fail("width and height must be positive"));
                }
                Ok(if kind == "oval" {
                    Roi::oval(x, y, width, height)
                } else {
                    Roi::rectangle(x, y, width, height)
                })
            }
            "rounded-rectangle" => {
                let &[x, y, width, height, diameter] = numbers(&words)?.as_slice() else {
                    return Err(fail(
                        "a rounded rectangle takes x, y, width, height and corner diameter",
                    ));
                };
                if !(width > 0.0 && height > 0.0) {
                    return Err(fail("width and height must be positive"));
                }
                if !(diameter >= 1.0 && diameter <= 65535.0 && diameter.fract() == 0.0) {
                    return Err(fail("the corner diameter must be a whole number of pixels"));
                }
                Ok(Roi::RoundedRectangle {
                    x,
                    y,
                    width,
                    height,
                    corner_diameter: diameter as u32,
                })
            }
            "polygon" => Ok(Roi::Polygon {
                points: at_least(points(&words)?, 3, "a polygon")?,
            }),
            "freehand" => Ok(Roi::Freehand {
                points: at_least(points(&words)?, 3, "a freehand selection")?,
            }),
            "traced" => Ok(Roi::Traced {
                points: at_least(points(&words)?, 3, "a traced selection")?,
            }),
            "points" | "point" => Ok(Roi::Points {
                points: at_least(points(&words)?, 1, "a point selection")?,
            }),
            "angle" => {
                let &[a, b, c] = points(&words)?.as_slice() else {
                    return Err(fail("an angle takes three points"));
                };
                Ok(Roi::Angle { points: [a, b, c] })
            }
            "line" => {
                let width = split_width(&mut words)?;
                let &[x1, y1, x2, y2] = numbers(&words)?.as_slice() else {
                    return Err(fail("a line takes x1, y1, x2 and y2"));
                };
                Ok(Roi::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    width,
                })
            }
            "polyline" | "freeline" => {
                let width = split_width(&mut words)?;
                let points = at_least(points(&words)?, 2, "a line")?;
                Ok(if kind == "polyline" {
                    Roi::Polyline { points, width }
                } else {
                    Roi::Freeline { points, width }
                })
            }
            "ellipse" => {
                let &[x1, y1, x2, y2, aspect_ratio] = numbers(&words)?.as_slice() else {
                    return Err(fail("an ellipse takes x1, y1, x2, y2 and aspect ratio"));
                };
                if x1 == x2 && y1 == y2 {
                    return Err(fail("the two ends of the major axis must differ"));
                }
                if !(aspect_ratio > 0.0 && aspect_ratio <= 1.0) {
                    return Err(fail("the aspect ratio must be above 0 and at most 1"));
                }
                Ok(Roi::ellipse(x1, y1, x2, y2, aspect_ratio))
            }
            "rotated-rectangle" => {
                let &[x1, y1, x2, y2, width] = numbers(&words)?.as_slice() else {
                    return Err(fail("a rotated rectangle takes x1, y1, x2, y2 and width"));
                };
                if x1 == x2 && y1 == y2 {
                    return Err(fail("the two ends of the centre line must differ"));
                }
                if !(width > 0.0) {
                    return Err(fail("the width must be positive"));
                }
                Ok(Roi::rotated_rectangle(x1, y1, x2, y2, width))
            }
            "composite" => {
                let mut segments = Vec::new();
                let mut i = 0;
                let next = |i: &mut usize| -> Result<(f32, f32), Error> {
                    let p = words
                        .get(*i)
                        .and_then(|w| point::<f32>(w))
                        .ok_or_else(|| fail("expected a point written as x,y in the path"))?;
                    *i += 1;
                    Ok(p)
                };
                while i < words.len() {
                    let step = words[i];
                    i += 1;
                    segments.push(match step {
                        "M" => {
                            let (x, y) = next(&mut i)?;
                            Segment::MoveTo(x, y)
                        }
                        "L" => {
                            let (x, y) = next(&mut i)?;
                            Segment::LineTo(x, y)
                        }
                        "Q" => {
                            let (a, b) = next(&mut i)?;
                            let (x, y) = next(&mut i)?;
                            Segment::QuadTo(a, b, x, y)
                        }
                        "C" => {
                            let (a, b) = next(&mut i)?;
                            let (c, d) = next(&mut i)?;
                            let (x, y) = next(&mut i)?;
                            Segment::CubicTo(a, b, c, d, x, y)
                        }
                        "Z" => Segment::Close,
                        _ => return Err(fail("a path is made of M, L, Q, C and Z steps")),
                    });
                }
                if !matches!(segments.first(), Some(Segment::MoveTo(..))) {
                    return Err(fail("a path must start with M x,y"));
                }
                Ok(Roi::Composite {
                    path: Path::new(segments),
                })
            }
            _ => Err(fail(
                "unknown shape; use rectangle, rounded-rectangle, oval, polygon, freehand, \
                 traced, line, polyline, freeline, angle, points, ellipse, rotated-rectangle \
                 or composite",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips() {
        let roi = Roi::rectangle(1.5, 2.0, 30.0, 4.25);
        assert_eq!(roi.to_string(), "rectangle 1.5 2 30 4.25");
        assert_eq!(roi.to_string().parse::<Roi>(), Ok(roi));
        for bad in [
            "",
            "rectangle 1 2 3",
            "rectangle 1 2 0 4",
            "blob 1 2",
            "rectangle a b c d",
            "rectangle 1 2 NaN 4",
            "oval 1 2 3 inf",
            "polygon 1,2 3,4",
            "polygon 1,2 3,4 5",
            "points",
            "line 1 2 3",
            "line 1 2 3 4 width 0",
            "line 1 2 3 4 width",
            "angle 1,1 2,2",
            "ellipse 1 1 1 1 0.5",
            "ellipse 1 1 9 9 1.5",
            "rotated-rectangle 1 1 5 5 0",
            "rounded-rectangle 1 1 5 5 2.5",
            "composite L 1,2",
            "composite M 1,2 X",
            "composite M 1,2 C 1,2 3,4",
        ] {
            assert!(bad.parse::<Roi>().is_err(), "{bad}");
        }
    }

    #[test]
    fn every_shape_has_a_readable_form() {
        for text in [
            "rounded-rectangle 1 2 30 40 8",
            "oval 10 20 30.5 40",
            "polygon 40,5 60,10 50,40",
            "freehand 0.5,0.25 6,1 3,9",
            "traced 0,0 2,0 2,2 0,2",
            "line 2 40 60 44",
            "line 2 40 60 44 width 5",
            "polyline 1,1 5,9 12,3",
            "freeline 1,1 5,9 12,3 width 2.5",
            "angle 1,9 1,1 9,1",
            "points 33,17",
            "points 1,2 3.5,4",
            "ellipse 10 10 40 25 0.6",
            "rotated-rectangle 10 10 40 25 12",
            "composite M 0,0 L 10,0 L 10,10 L 0,10 Z M 3,3 L 7,3 L 7,7 L 3,7 Z",
            "composite M 0,5 C 0,2 2,0 5,0 Q 9,1 9,5 Z",
        ] {
            let roi: Roi = text.parse().unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(roi.to_string(), text);
        }
        assert_eq!("point 3,4".parse::<Roi>(), Ok(Roi::point(3.0, 4.0)));
    }
}
