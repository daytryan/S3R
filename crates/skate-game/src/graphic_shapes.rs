//! The Graphic Creator's shape library beyond the first fifteen: about 150 hand-made vector shapes
//! (basic, arrows, stars, animals, symbols, frames, patterns, skate things), each a few lines of
//! geometry in the unit box (-0.5..0.5, y down). The website's library was never archived, so these
//! are our own drawings.
//!
//! A shape is filled with the non-zero rule: every contour added with `fill` is wound the same way,
//! every `hole` the opposite way (`build` turns them round as needed), so overlapping fills simply
//! join up and a hole cuts out whatever fill it sits inside. Holes must not overlap each other,
//! and a hole must lie wholly inside the fills it cuts. (`Shape(index)` draws indices 15 and up
//! with this rule; the first fifteen keep their own.)
use tiny_skia::{Path, PathBuilder};

type P = (f32, f32);

/// Contours of one shape.
#[derive(Default)]
pub(crate) struct B {
    fills: Vec<Vec<P>>,
    holes: Vec<Vec<P>>,
}
impl B {
    fn fill(&mut self, pts: Vec<P>) -> &mut Self {
        self.fills.push(pts);
        self
    }
    fn fills(&mut self, parts: Vec<Vec<P>>) -> &mut Self {
        self.fills.extend(parts);
        self
    }
    fn hole(&mut self, pts: Vec<P>) -> &mut Self {
        self.holes.push(pts);
        self
    }
    fn build(self) -> Option<Path> {
        let mut path = PathBuilder::new();
        let contours = self.fills.into_iter().map(|c| (c, false)).chain(self.holes.into_iter().map(|c| (c, true)));
        for (mut pts, hole) in contours {
            if pts.len() < 3 {
                continue;
            }
            // Fills run one way round (positive area), holes the other.
            let area: f32 = pts.iter().zip(pts.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum();
            if if hole { area > 0. } else { area < 0. } {
                pts.reverse();
            }
            path.move_to(pts[0].0, pts[0].1);
            for p in &pts[1..] {
                path.line_to(p.0, p.1);
            }
            path.close();
        }
        path.finish()
    }
}

// ---- geometry ----

/// Points round an ellipse's arc from angle `a0` to `a1` (degrees; 0 is right, 90 is down), both ends included.
fn arc(cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32) -> Vec<P> {
    let n = ((a1 - a0).abs() / 5.).ceil().max(2.) as usize;
    (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect()
}
fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<P> {
    let mut pts = arc(cx, cy, rx, ry, -90., 270.);
    pts.pop();
    pts
}
fn circle(cx: f32, cy: f32, r: f32) -> Vec<P> {
    ellipse(cx, cy, r, r)
}
fn rect(x: f32, y: f32, w: f32, h: f32) -> Vec<P> {
    vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
}
fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<P> {
    let mut pts = arc(x + w - r, y + r, r, r, -90., 0.);
    pts.extend(arc(x + w - r, y + h - r, r, r, 0., 90.));
    pts.extend(arc(x + r, y + h - r, r, r, 90., 180.));
    pts.extend(arc(x + r, y + r, r, r, 180., 270.));
    pts
}
/// A regular polygon, its first corner `rot` degrees round from straight up.
fn ngon(cx: f32, cy: f32, r: f32, n: usize, rot: f32) -> Vec<P> {
    star(cx, cy, r, r, n, rot)
}
/// A star of `n` points, outer radius `ro`, inner `ri`, the first point `rot` degrees round from straight up.
fn star(cx: f32, cy: f32, ro: f32, ri: f32, n: usize, rot: f32) -> Vec<P> {
    (0..n * 2)
        .filter(|i| ro != ri || i % 2 == 0)
        .map(|i| {
            let r = if i % 2 == 0 { ro } else { ri };
            let a = (rot - 90. + 180. * i as f32 / n as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}
/// An annular sector: radii `r0` < `r1` from angle `a0` to `a1`.
fn band(cx: f32, cy: f32, r0: f32, r1: f32, a0: f32, a1: f32) -> Vec<P> {
    let mut pts = arc(cx, cy, r1, r1, a0, a1);
    pts.extend(arc(cx, cy, r0, r0, a1, a0));
    pts
}
fn rot(pts: Vec<P>, deg: f32) -> Vec<P> {
    let (s, c) = deg.to_radians().sin_cos();
    pts.into_iter().map(|(x, y)| (x * c - y * s, x * s + y * c)).collect()
}
fn rot_at(pts: Vec<P>, deg: f32, cx: f32, cy: f32) -> Vec<P> {
    shift(rot(shift(pts, -cx, -cy), deg), cx, cy)
}
fn shift(pts: Vec<P>, dx: f32, dy: f32) -> Vec<P> {
    pts.into_iter().map(|(x, y)| (x + dx, y + dy)).collect()
}
fn scale(pts: Vec<P>, sx: f32, sy: f32) -> Vec<P> {
    pts.into_iter().map(|(x, y)| (x * sx, y * sy)).collect()
}
fn flip_x(pts: Vec<P>) -> Vec<P> {
    scale(pts, -1., 1.)
}
/// A thick line between two points, square ended.
fn seg(x0: f32, y0: f32, x1: f32, y1: f32, w: f32) -> Vec<P> {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = dx.hypot(dy).max(1e-6);
    let (nx, ny) = (-dy / len * w / 2., dx / len * w / 2.);
    vec![(x0 + nx, y0 + ny), (x1 + nx, y1 + ny), (x1 - nx, y1 - ny), (x0 - nx, y0 - ny)]
}
/// A thick line through points, with round joints and ends.
fn stroke(pts: &[P], w: f32) -> Vec<Vec<P>> {
    let mut out: Vec<Vec<P>> = pts.windows(2).map(|s| seg(s[0].0, s[0].1, s[1].0, s[1].1, w)).collect();
    out.extend(pts.iter().map(|p| ngon(p.0, p.1, w / 2., 12, 0.)));
    out
}
/// A path drawn with a pen, like an SVG path.
struct Pen(Vec<P>);
fn pen(x: f32, y: f32) -> Pen {
    Pen(vec![(x, y)])
}
impl Pen {
    fn last(&self) -> P {
        *self.0.last().expect("a pen starts somewhere")
    }
    fn l(mut self, x: f32, y: f32) -> Self {
        self.0.push((x, y));
        self
    }
    fn c(mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) -> Self {
        let (x0, y0) = self.last();
        for i in 1..=14 {
            let t = i as f32 / 14.;
            let u = 1. - t;
            let (a, b, c, d) = (u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t);
            self.0.push((a * x0 + b * x1 + c * x2 + d * x, a * y0 + b * y1 + c * y2 + d * y));
        }
        self
    }
    fn q(mut self, x1: f32, y1: f32, x: f32, y: f32) -> Self {
        let (x0, y0) = self.last();
        for i in 1..=14 {
            let t = i as f32 / 14.;
            let u = 1. - t;
            self.0.push((u * u * x0 + 2. * u * t * x1 + t * t * x, u * u * y0 + 2. * u * t * y1 + t * t * y));
        }
        self
    }
    fn arc(mut self, cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32) -> Self {
        self.0.extend(arc(cx, cy, rx, ry, a0, a1));
        self
    }
    fn done(self) -> Vec<P> {
        self.0
    }
    /// The path is the right half of a shape that is the same on the left; mirror it to close.
    /// Its first and last points sit on the middle line.
    fn sym(self) -> Vec<P> {
        let mut pts = self.0;
        let on_axis = |p: &P| p.0.abs() < 1e-4;
        let skip_last = pts.last().is_some_and(on_axis);
        let skip_first = pts.first().is_some_and(on_axis);
        let end = pts.len() - usize::from(skip_last);
        let start = usize::from(skip_first);
        let mirrored: Vec<P> = pts[start..end].iter().rev().map(|p| (-p.0, p.1)).collect();
        pts.extend(mirrored);
        pts
    }
}
/// The part of a polygon where `a * x + b * y <= c`.
fn clip(poly: Vec<P>, a: f32, b: f32, c: f32) -> Vec<P> {
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let (dp, dq) = (a * p.0 + b * p.1 - c, a * q.0 + b * q.1 - c);
        if dp <= 0. {
            out.push(p);
        }
        if (dp < 0.) != (dq < 0.) && (dp - dq).abs() > 1e-9 {
            let t = dp / (dp - dq);
            out.push((p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t));
        }
    }
    out
}
/// `n` copies of a contour turned round the middle.
fn spin(pts: &[P], n: usize) -> Vec<Vec<P>> {
    (0..n).map(|k| rot(pts.to_vec(), 360. * k as f32 / n as f32)).collect()
}

macro_rules! table {
    ($($name:literal => $draw:expr,)*) => {
        const TABLE: &[(&str, fn(&mut B))] = &[$(($name, $draw as fn(&mut B))),*];
    };
}

table! {
    // ---- basic ----
    "Pentagon" => |b| { b.fill(ngon(0., 0.045, 0.5, 5, 0.)); },
    "Octagon" => |b| { b.fill(ngon(0., 0., 0.54, 8, 22.5)); },
    "Parallelogram" => |b| { b.fill(vec![(-0.5, 0.3), (-0.25, -0.3), (0.5, -0.3), (0.25, 0.3)]); },
    "Trapezoid" => |b| { b.fill(vec![(-0.5, 0.3), (-0.3, -0.3), (0.3, -0.3), (0.5, 0.3)]); },
    "Right Triangle" => |b| { b.fill(vec![(-0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)]); },
    "Semicircle" => |b| { b.fill(pen(-0.5, 0.25).arc(0., 0.25, 0.5, 0.5, 180., 360.).done()); },
    "Quarter Circle" => |b| { b.fill(pen(-0.5, 0.5).arc(-0.5, 0.5, 1., 1., 270., 360.).done()); },
    "Oval" => |b| { b.fill(ellipse(0., 0., 0.34, 0.5)); },
    "Pill" => |b| { b.fill(rrect(-0.5, -0.22, 1., 0.44, 0.22)); },
    "Crescent" => |b| {
        // A circle of radius 0.5 with a circle of radius 0.42 bitten out of it, 0.2 to the right.
        let (d, r1, r2) = (0.2f32, 0.5f32, 0.42f32);
        let x = (d * d + r1 * r1 - r2 * r2) / (2. * d);
        let y = (r1 * r1 - x * x).sqrt();
        let (a1, a2) = (y.atan2(x).to_degrees(), y.atan2(x - d).to_degrees());
        b.fill(pen(x, -y).arc(0., 0., r1, r1, -a1, a1 - 360.).arc(d, 0., r2, r2, a2, 360. - a2).done());
    },
    "Lens" => |b| { b.fill(pen(0., -0.5).c(0.45, -0.25, 0.45, 0.25, 0., 0.5).sym()); },
    "Teardrop" => |b| { b.fill(pen(0., -0.5).c(0.12, -0.3, 0.35, -0.1, 0.35, 0.15).arc(0., 0.15, 0.35, 0.35, 0., 90.).sym()); },
    "Squircle" => |b| {
        b.fill((0..96).map(|i| {
            let a = (i as f32 * 360. / 96.).to_radians();
            let f = |v: f32| 0.5 * v.signum() * v.abs().sqrt();
            (f(a.cos()), f(a.sin()))
        }).collect());
    },
    "Square Frame" => |b| { b.fill(rect(-0.5, -0.5, 1., 1.)).hole(rect(-0.34, -0.34, 0.68, 0.68)); },
    "Rounded Frame" => |b| { b.fill(rrect(-0.5, -0.5, 1., 1., 0.22)).hole(rrect(-0.35, -0.35, 0.7, 0.7, 0.1)); },
    "Half Ring" => |b| { b.fill(band(0., 0.25, 0.26, 0.5, 180., 360.)); },
    "Quarter Ring" => |b| { b.fill(band(-0.5, 0.5, 0.58, 1., 270., 360.)); },
    "Target" => |b| {
        b.fill(circle(0., 0., 0.5)).hole(circle(0., 0., 0.4)).fill(circle(0., 0., 0.3)).hole(circle(0., 0., 0.2)).fill(circle(0., 0., 0.1));
    },
    "Corner Frame" => |b| {
        let l = vec![(-0.5, -0.5), (-0.14, -0.5), (-0.14, -0.38), (-0.38, -0.38), (-0.38, -0.14), (-0.5, -0.14)];
        b.fills(spin(&l, 4));
    },
    "Plus" => |b| { b.fill(rect(-0.09, -0.5, 0.18, 1.)).fill(rect(-0.5, -0.09, 1., 0.18)); },
    "Slash" => |b| { b.fill(vec![(-0.5, 0.5), (-0.5, 0.32), (0.32, -0.5), (0.5, -0.5), (0.5, -0.32), (-0.32, 0.5)]); },
    "Tall Ring" => |b| { b.fill(ellipse(0., 0., 0.34, 0.5)).hole(ellipse(0., 0., 0.23, 0.39)); },
    "Hourglass" => |b| { b.fill(vec![(-0.4, -0.5), (0.4, -0.5), (0.05, 0.), (0.4, 0.5), (-0.4, 0.5), (-0.05, 0.)]); },
    "Bowtie" => |b| { b.fill(vec![(-0.5, -0.4), (0., -0.05), (0.5, -0.4), (0.5, 0.4), (0., 0.05), (-0.5, 0.4)]); },

    // ---- arrows ----
    "Arrow Up" => |b| { b.fill(vec![(0., -0.5), (0.45, 0.), (0.18, 0.), (0.18, 0.5), (-0.18, 0.5), (-0.18, 0.), (-0.45, 0.)]); },
    "Double Arrow" => |b| {
        b.fill(vec![(-0.5, 0.), (-0.2, -0.3), (-0.2, -0.12), (0.2, -0.12), (0.2, -0.3), (0.5, 0.), (0.2, 0.3), (0.2, 0.12), (-0.2, 0.12), (-0.2, 0.3)]);
    },
    "Four-Way Arrow" => |b| {
        let h = vec![(-0.5, 0.), (-0.3, -0.18), (-0.3, -0.07), (0.3, -0.07), (0.3, -0.18), (0.5, 0.), (0.3, 0.18), (0.3, 0.07), (-0.3, 0.07), (-0.3, 0.18)];
        b.fill(h.clone()).fill(rot(h, 90.));
    },
    "Double Chevron" => |b| {
        let c = vec![(-0.5, -0.5), (-0.3, -0.5), (-0.05, 0.), (-0.3, 0.5), (-0.5, 0.5), (-0.25, 0.)];
        b.fill(c.clone()).fill(shift(c, 0.5, 0.));
    },
    "Pointer" => |b| { b.fill(vec![(-0.4, -0.5), (0.5, 0.), (-0.4, 0.5)]); },
    "Notched Arrow" => |b| { b.fill(vec![(-0.5, -0.22), (0.05, -0.22), (0.05, -0.5), (0.5, 0.), (0.05, 0.5), (0.05, 0.22), (-0.5, 0.22), (-0.3, 0.)]); },
    "Elbow Arrow" => |b| {
        b.fill(vec![(-0.4, 0.5), (-0.4, -0.3), (0.1, -0.3), (0.1, -0.5), (0.5, -0.2), (0.1, 0.1), (0.1, -0.1), (-0.2, -0.1), (-0.2, 0.5)]);
    },
    "U-Turn Arrow" => |b| {
        b.fill(pen(-0.35, 0.5).l(-0.35, -0.1).arc(0., -0.1, 0.35, 0.35, 180., 360.).l(0.35, 0.).l(0.5, 0.).l(0.26, 0.4).l(0.02, 0.).l(0.17, 0.)
            .l(0.17, -0.1).arc(0., -0.1, 0.17, 0.17, 360., 180.).l(-0.17, 0.5).done());
    },
    "Refresh Arrow" => |b| {
        let pt = |r: f32, a: f32| (r * a.to_radians().cos(), r * a.to_radians().sin());
        let (tip, out, inn) = (pt(0.31, 335.), pt(0.5, 290.), pt(0.12, 290.));
        b.fill(pen(pt(0.4, 20.).0, pt(0.4, 20.).1).arc(0., 0., 0.4, 0.4, 20., 290.).l(out.0, out.1).l(tip.0, tip.1)
            .l(inn.0, inn.1).l(pt(0.22, 290.).0, pt(0.22, 290.).1).arc(0., 0., 0.22, 0.22, 290., 20.).done());
    },
    "Swoosh Arrow" => |b| {
        b.fill(pen(-0.5, 0.4).c(-0.4, -0.15, -0.05, -0.32, 0.2, -0.3).l(0.2, -0.45).l(0.5, -0.12).l(0.2, 0.2).l(0.2, 0.0)
            .c(-0.05, -0.02, -0.3, 0.1, -0.5, 0.4).done());
    },
    "Dart" => |b| {
        let shaft = rect(-0.6, -0.045, 1.1, 0.09);
        let head = vec![(0.72, 0.), (0.36, -0.22), (0.36, 0.22)];
        let fletch = vec![(-0.72, -0.24), (-0.46, -0.24), (-0.3, 0.), (-0.46, 0.24), (-0.72, 0.24), (-0.56, 0.)];
        for part in [shaft, head, fletch] {
            b.fill(scale(rot(part, -45.), 0.69, 0.69));
        }
    },
    "Arrowhead" => |b| { b.fill(vec![(0., -0.5), (0.45, 0.5), (0., 0.28), (-0.45, 0.5)]); },
    "Cursor" => |b| { b.fill(vec![(-0.3, -0.5), (-0.3, 0.35), (-0.1, 0.18), (0.05, 0.5), (0.2, 0.43), (0.05, 0.12), (0.3, 0.12)]); },
    "Fast Forward" => |b| { b.fill(vec![(-0.5, -0.4), (0., 0.), (-0.5, 0.4)]).fill(vec![(0., -0.4), (0.5, 0.), (0., 0.4)]); },
    "Thin Arrow" => |b| { b.fill(vec![(-0.5, -0.05), (0.2, -0.05), (0.2, -0.2), (0.5, 0.), (0.2, 0.2), (0.2, 0.05), (-0.5, 0.05)]); },
    "Diagonal Arrow" => |b| {
        b.fill(scale(rot(vec![(-0.5, -0.14), (0.1, -0.14), (0.1, -0.4), (0.5, 0.), (0.1, 0.4), (0.1, 0.14), (-0.5, 0.14)], -45.), 0.95, 0.95));
    },

    // ---- stars and bursts ----
    "Star 4" => |b| { b.fill(star(0., 0., 0.5, 0.15, 4, 0.)); },
    "Star 6" => |b| { b.fill(star(0., 0., 0.5, 0.26, 6, 0.)); },
    "Star 8" => |b| { b.fill(star(0., 0., 0.5, 0.3, 8, 0.)); },
    "Sparkle" => |b| { b.fill(pen(0., -0.5).q(0.05, -0.05, 0.5, 0.).q(0.05, 0.05, 0., 0.5).q(-0.05, 0.05, -0.5, 0.).q(-0.05, -0.05, 0., -0.5).done()); },
    "Fat Star" => |b| { b.fill(star(0., 0.02, 0.52, 0.3, 5, 0.)); },
    "Thin Star" => |b| { b.fill(star(0., 0.02, 0.52, 0.14, 5, 0.)); },
    "Sharp Burst" => |b| { b.fill(star(0., 0., 0.5, 0.36, 16, 0.)); },
    "Scallop Badge" => |b| {
        b.fill((0..180).map(|i| {
            let a = (i as f32 * 2.).to_radians();
            let r = 0.45 + 0.05 * (12. * a).cos();
            (r * a.cos(), r * a.sin())
        }).collect());
    },
    "Starburst 24" => |b| { b.fill(star(0., 0., 0.5, 0.42, 24, 0.)); },
    "Star Outline" => |b| { b.fill(star(0., 0.02, 0.52, 0.22, 5, 0.)).hole(star(0., 0.025, 0.3, 0.12, 5, 0.)); },
    "Pinwheel" => |b| { b.fills(spin(&[(0., 0.), (0., -0.5), (0.5, -0.5)], 4)); },
    "Explosion" => |b| {
        b.fill((0..22).map(|i| {
            let r = if i % 2 == 0 { 0.5 - 0.07 * ((i * 7) % 3) as f32 } else { 0.24 + 0.03 * ((i * 5) % 3) as f32 };
            let a = (i as f32 * 360. / 22. - 90.).to_radians();
            (r * a.cos(), r * a.sin())
        }).collect());
    },
    "Sun" => |b| {
        b.fill(circle(0., 0., 0.22)).fills(spin(&[(-0.07, -0.3), (0.07, -0.3), (0., -0.5)], 12));
    },
    "Flare" => |b| { b.fill(star(0., 0., 0.5, 0.07, 8, 0.)); },
    "Star Badge" => |b| { b.fill(circle(0., 0., 0.5)).hole(circle(0., 0., 0.44)).fill(star(0., 0.02, 0.34, 0.14, 5, 0.)); },

    // ---- animals ----
    "Cat" => |b| {
        b.fill(ellipse(0., 0.12, 0.42, 0.36));
        let ear = vec![(-0.42, 0.), (-0.4, -0.5), (-0.05, -0.2)];
        b.fill(ear.clone()).fill(flip_x(ear));
        b.hole(ellipse(-0.17, 0.08, 0.075, 0.1)).hole(ellipse(0.17, 0.08, 0.075, 0.1));
        b.hole(vec![(-0.06, 0.24), (0.06, 0.24), (0., 0.31)]);
    },
    "Dog" => |b| {
        b.fill(ellipse(0., 0.05, 0.3, 0.38));
        b.fill(rot_at(ellipse(-0.36, 0., 0.11, 0.28), 12., -0.36, 0.)).fill(rot_at(ellipse(0.36, 0., 0.11, 0.28), -12., 0.36, 0.));
        b.hole(ellipse(-0.13, -0.05, 0.045, 0.055)).hole(ellipse(0.13, -0.05, 0.045, 0.055));
        b.hole(ellipse(0., 0.2, 0.08, 0.055));
        b.hole(band(0., 0.22, 0.12, 0.15, 30., 150.));
    },
    "Bear" => |b| {
        b.fill(circle(0., 0.1, 0.38)).fill(circle(-0.3, -0.28, 0.15)).fill(circle(0.3, -0.28, 0.15));
        b.hole(circle(-0.3, -0.28, 0.07)).hole(circle(0.3, -0.28, 0.07));
        b.hole(circle(-0.14, 0.02, 0.045)).hole(circle(0.14, 0.02, 0.045));
        b.hole(ellipse(0., 0.2, 0.08, 0.055));
    },
    "Rabbit" => |b| {
        b.fill(ellipse(0., 0.2, 0.3, 0.27));
        b.fill(rot_at(ellipse(-0.14, -0.28, 0.085, 0.24), 8., -0.14, -0.28)).fill(rot_at(ellipse(0.14, -0.28, 0.085, 0.24), -8., 0.14, -0.28));
        b.hole(rot_at(ellipse(-0.14, -0.28, 0.035, 0.16), 8., -0.14, -0.28)).hole(rot_at(ellipse(0.14, -0.28, 0.035, 0.16), -8., 0.14, -0.28));
        b.hole(ellipse(-0.12, 0.15, 0.04, 0.05)).hole(ellipse(0.12, 0.15, 0.04, 0.05));
        b.hole(vec![(-0.05, 0.27), (0.05, 0.27), (0., 0.33)]);
    },
    "Mouse" => |b| {
        b.fill(circle(-0.3, -0.2, 0.2)).fill(circle(0.3, -0.2, 0.2)).fill(ellipse(0., 0.1, 0.28, 0.36));
        b.hole(circle(-0.3, -0.2, 0.1)).hole(circle(0.3, -0.2, 0.1));
        b.hole(ellipse(-0.12, 0.02, 0.04, 0.055)).hole(ellipse(0.12, 0.02, 0.04, 0.055));
        b.hole(ellipse(0., 0.38 - 0.1, 0.05, 0.04));
    },
    "Pig" => |b| {
        b.fill(circle(0., 0.05, 0.42));
        let ear = vec![(-0.42, -0.1), (-0.38, -0.46), (-0.1, -0.33)];
        b.fill(ear.clone()).fill(flip_x(ear));
        b.hole(ellipse(0., 0.14, 0.2, 0.14)).fill(ellipse(-0.07, 0.14, 0.03, 0.05)).fill(ellipse(0.07, 0.14, 0.03, 0.05));
        b.hole(ellipse(-0.2, -0.1, 0.04, 0.05)).hole(ellipse(0.2, -0.1, 0.04, 0.05));
    },
    "Frog" => |b| {
        b.fill(ellipse(0., 0.14, 0.5, 0.3)).fill(circle(-0.25, -0.2, 0.17)).fill(circle(0.25, -0.2, 0.17));
        b.hole(circle(-0.25, -0.24, 0.07)).hole(circle(0.25, -0.24, 0.07));
        b.hole(band(0., 0.05, 0.3, 0.34, 25., 155.));
        b.hole(circle(-0.05, 0.05, 0.018)).hole(circle(0.05, 0.05, 0.018));
    },
    "Owl" => |b| {
        b.fill(ellipse(0., 0.1, 0.4, 0.4));
        let tuft = vec![(-0.4, -0.1), (-0.38, -0.5), (-0.12, -0.28)];
        b.fill(tuft.clone()).fill(flip_x(tuft));
        b.hole(circle(-0.17, -0.02, 0.15)).hole(circle(0.17, -0.02, 0.15));
        b.fill(circle(-0.17, -0.02, 0.07)).fill(circle(0.17, -0.02, 0.07));
        b.hole(vec![(-0.06, 0.13), (0.06, 0.13), (0., 0.27)]);
    },
    "Fish" => |b| {
        b.fill(pen(-0.5, 0.).c(-0.3, -0.36, 0.1, -0.4, 0.28, -0.05).l(0.5, -0.3).l(0.42, 0.).l(0.5, 0.3).l(0.28, 0.05).c(0.1, 0.4, -0.3, 0.36, -0.5, 0.).done());
        b.hole(circle(-0.3, -0.05, 0.04));
    },
    "Fish Bone" => |b| {
        b.fill(rect(-0.3, -0.025, 0.62, 0.05)).fill(ellipse(-0.4, 0., 0.1, 0.14)).fill(vec![(0.3, 0.), (0.5, -0.2), (0.5, 0.2)]);
        for k in 0..4 {
            let x = -0.22 + 0.14 * k as f32;
            b.fill(seg(x, 0., x + 0.08, -0.22, 0.045)).fill(seg(x, 0., x + 0.08, 0.22, 0.045));
        }
        b.hole(circle(-0.43, -0.03, 0.028));
    },
    "Bird" => |b| {
        b.fill(rot_at(ellipse(-0.05, 0.12, 0.3, 0.22), -10., -0.05, 0.12));
        b.fill(vec![(-0.3, 0.08), (-0.5, -0.02), (-0.5, 0.24), (-0.28, 0.22)]);
        b.fill(circle(0.22, -0.1, 0.17)).fill(vec![(0.36, -0.14), (0.5, -0.08), (0.36, -0.03)]);
        b.fills(stroke(&[(-0.02, 0.3), (-0.02, 0.5)], 0.04)).fills(stroke(&[(0.1, 0.3), (0.1, 0.5)], 0.04));
        b.hole(circle(0.26, -0.13, 0.03));
    },
    "Gull" => |b| {
        b.fill(pen(-0.5, -0.1).c(-0.3, -0.35, -0.1, -0.3, 0., 0.05).c(0.1, -0.3, 0.3, -0.35, 0.5, -0.1).c(0.3, -0.2, 0.12, -0.1, 0., 0.22).c(-0.12, -0.1, -0.3, -0.2, -0.5, -0.1).done());
    },
    "Paw Print" => |b| {
        b.fill(ellipse(0., 0.2, 0.28, 0.22));
        b.fill(rot_at(ellipse(-0.33, -0.05, 0.1, 0.14), -20., -0.33, -0.05)).fill(ellipse(-0.12, -0.28, 0.1, 0.15));
        b.fill(ellipse(0.12, -0.28, 0.1, 0.15)).fill(rot_at(ellipse(0.33, -0.05, 0.1, 0.14), 20., 0.33, -0.05));
    },
    "Bone" => |b| {
        b.fill(rot(rect(-0.34, -0.09, 0.68, 0.18), -35.));
        for (x, y) in [(-0.35, -0.12), (-0.35, 0.12), (0.35, -0.12), (0.35, 0.12)] {
            b.fill(rot(circle(x, y, 0.13), -35.));
        }
    },
    "Butterfly" => |b| {
        let upper = pen(0.03, -0.04).c(0.1, -0.45, 0.5, -0.55, 0.5, -0.22).c(0.5, 0., 0.2, 0.05, 0.03, 0.03).done();
        let lower = pen(0.03, 0.03).c(0.3, 0.05, 0.42, 0.3, 0.3, 0.45).c(0.2, 0.55, 0.06, 0.3, 0.03, 0.08).done();
        b.fill(flip_x(upper.clone())).fill(upper).fill(flip_x(lower.clone())).fill(lower);
        b.fill(ellipse(0., 0.02, 0.04, 0.28));
        b.fills(stroke(&[(0., -0.25), (0.1, -0.45)], 0.025)).fills(stroke(&[(0., -0.25), (-0.1, -0.45)], 0.025));
        b.hole(circle(-0.28, -0.25, 0.07)).hole(circle(0.28, -0.25, 0.07));
    },
    "Bat" => |b| {
        b.fill(pen(0., -0.22).l(0.07, -0.38).l(0.14, -0.22).c(0.25, -0.3, 0.4, -0.28, 0.5, -0.18).c(0.45, -0.1, 0.45, 0., 0.42, 0.1)
            .c(0.36, 0., 0.3, 0.02, 0.26, 0.12).c(0.2, 0.02, 0.14, 0.02, 0.1, 0.15).c(0.08, 0.25, 0.04, 0.3, 0., 0.32).sym());
    },
    "Spider" => |b| {
        b.fill(ellipse(0., 0.1, 0.17, 0.22)).fill(circle(0., -0.15, 0.1));
        for s in [-1., 1.] {
            for (i, y) in [-0.14, -0.04, 0.06, 0.16].into_iter().enumerate() {
                let up = 0.2 - 0.05 * i as f32;
                b.fills(stroke(&[(0.08 * s, y), (0.28 * s, y - up), (0.48 * s, y + 0.1 + 0.05 * i as f32)], 0.045));
            }
        }
    },
    "Shark Fin" => |b| { b.fill(pen(-0.5, 0.5).c(-0.3, 0.2, -0.1, -0.3, 0.1, -0.5).c(0.1, -0.1, 0.3, 0.3, 0.5, 0.5).done()); },
    "Snail" => |b| {
        b.fill(circle(0.12, -0.1, 0.32)).hole(circle(0.12, -0.1, 0.22)).fill(circle(0.12, -0.1, 0.12));
        b.fill(rrect(-0.5, 0.2, 0.98, 0.2, 0.1)).fill(seg(-0.36, 0.25, -0.4, -0.15, 0.13)).fill(circle(-0.4, -0.2, 0.08));
        b.fills(stroke(&[(-0.42, -0.22), (-0.46, -0.42)], 0.025)).fills(stroke(&[(-0.36, -0.22), (-0.32, -0.42)], 0.025));
    },
    "Penguin" => |b| {
        b.fill(ellipse(0., 0.04, 0.34, 0.46));
        b.hole(ellipse(0., 0.14, 0.2, 0.3));
        b.hole(circle(-0.1, -0.26, 0.04)).hole(circle(0.1, -0.26, 0.04));
        b.fill(ellipse(-0.15, 0.48, 0.1, 0.04)).fill(ellipse(0.15, 0.48, 0.1, 0.04));
        b.fill(rot_at(ellipse(-0.4, 0.1, 0.07, 0.25), 15., -0.4, 0.1)).fill(rot_at(ellipse(0.4, 0.1, 0.07, 0.25), -15., 0.4, 0.1));
    },
    "Snake" => |b| {
        let line: Vec<P> = (0..=40).map(|i| {
            let t = i as f32 / 40.;
            (-0.42 + 0.84 * t, 0.2 * (t * 3. * std::f32::consts::PI).sin())
        }).collect();
        b.fills(stroke(&line, 0.12));
        b.fill(rot_at(ellipse(0.4, 0., 0.09, 0.07), 20., 0.4, 0.));
        b.fills(stroke(&[(-0.42, 0.), (-0.48, 0.06)], 0.03));
    },

    // ---- symbols ----
    "Skull" => |b| {
        b.fill(ellipse(0., -0.08, 0.38, 0.36)).fill(rrect(-0.2, 0.18, 0.4, 0.3, 0.06));
        b.hole(ellipse(-0.14, -0.05, 0.1, 0.11)).hole(ellipse(0.14, -0.05, 0.1, 0.11));
        b.hole(vec![(0., 0.06), (-0.05, 0.16), (0.05, 0.16)]);
        b.hole(rect(-0.1, 0.33, 0.04, 0.14)).hole(rect(-0.02, 0.33, 0.04, 0.14)).hole(rect(0.06, 0.33, 0.04, 0.14));
    },
    "Crossbones" => |b| {
        let mut bone = vec![rect(-0.5, -0.06, 1., 0.12)];
        bone.extend([(-0.5, -0.09), (-0.5, 0.09), (0.5, -0.09), (0.5, 0.09)].map(|(x, y)| circle(x, y, 0.1)));
        for angle in [45., -45.] {
            b.fills(bone.iter().map(|c| rot(c.clone(), angle)).collect());
        }
    },
    "Peace" => |b| {
        b.fill(circle(0., 0., 0.5)).hole(circle(0., 0., 0.43));
        b.fill(seg(0., -0.43, 0., 0.43, 0.07)).fill(seg(0., 0., -0.3, 0.3, 0.07)).fill(seg(0., 0., 0.3, 0.3, 0.07));
    },
    "Yin Yang" => |b| {
        b.fill(circle(0., 0., 0.5)).hole(circle(0., 0., 0.455));
        b.fill(pen(0., -0.5).arc(0., 0., 0.5, 0.5, 270., 450.).arc(0., 0.25, 0.25, 0.25, 90., 270.).arc(0., -0.25, 0.25, 0.25, 90., -90.).done());
        b.hole(circle(0., 0.25, 0.07)).fill(circle(0., -0.25, 0.07));
    },
    "Flame" => |b| {
        b.fill(pen(0.05, -0.5).c(0.12, -0.25, 0.42, -0.12, 0.4, 0.18).c(0.38, 0.4, 0.2, 0.5, 0., 0.5).c(-0.22, 0.5, -0.4, 0.38, -0.4, 0.15)
            .c(-0.4, -0.02, -0.28, -0.12, -0.2, -0.3).c(-0.14, -0.18, -0.08, -0.1, -0.06, 0.).c(-0.02, -0.12, 0.03, -0.3, 0.05, -0.5).done());
        b.hole(pen(0., 0.1).c(0.1, 0.18, 0.2, 0.3, 0.15, 0.38).c(0.1, 0.46, -0.1, 0.46, -0.15, 0.38).c(-0.2, 0.3, -0.05, 0.18, 0., 0.1).done());
    },
    "Cloud" => |b| {
        b.fill(rrect(-0.5, 0., 1., 0.32, 0.16)).fill(circle(-0.22, 0., 0.2)).fill(circle(0.02, -0.12, 0.28)).fill(circle(0.3, 0.02, 0.2));
    },
    "Rain Cloud" => |b| {
        let cloud = vec![rrect(-0.5, -0.1, 1., 0.3, 0.15), circle(-0.22, -0.1, 0.18), circle(0.02, -0.22, 0.26), circle(0.3, -0.08, 0.18)];
        b.fills(cloud.into_iter().map(|c| shift(c, 0., -0.02)).collect());
        for x in [-0.3, 0., 0.3] {
            b.fill(seg(x, 0.27, x - 0.07, 0.46, 0.07));
        }
    },
    "Snowflake" => |b| {
        for arm in [seg(0., 0., 0., -0.5, 0.06), seg(0., -0.32, 0.13, -0.42, 0.045), seg(0., -0.32, -0.13, -0.42, 0.045), seg(0., -0.17, 0.09, -0.25, 0.04), seg(0., -0.17, -0.09, -0.25, 0.04)] {
            b.fills(spin(&arm, 6));
        }
    },
    "Leaf" => |b| {
        b.fill(pen(-0.45, 0.45).c(-0.45, -0.1, -0.1, -0.5, 0.5, -0.45).c(0.45, 0.1, 0.1, 0.5, -0.45, 0.45).done());
        b.hole(seg(-0.3, 0.3, 0.3, -0.3, 0.03));
    },
    "Flower" => |b| { b.fills(spin(&ellipse(0., -0.27, 0.14, 0.22), 5)); },
    "Clover" => |b| {
        let heart = pen(0., 0.05).c(-0.2, -0.1, -0.3, -0.25, -0.22, -0.38).c(-0.14, -0.5, -0.04, -0.42, 0., -0.3)
            .c(0.04, -0.42, 0.14, -0.5, 0.22, -0.38).c(0.3, -0.25, 0.2, -0.1, 0., 0.05).done();
        b.fills(spin(&heart, 4)).fill(seg(0., 0., 0.12, 0.5, 0.06));
    },
    "Check Mark" => |b| { b.fill(vec![(-0.5, 0.05), (-0.33, -0.12), (-0.12, 0.1), (0.33, -0.4), (0.5, -0.23), (-0.12, 0.45)]); },
    "X Mark" => |b| {
        b.fill(rot(vec![(-0.09, -0.62), (0.09, -0.62), (0.09, -0.09), (0.62, -0.09), (0.62, 0.09), (0.09, 0.09), (0.09, 0.62), (-0.09, 0.62), (-0.09, 0.09), (-0.62, 0.09), (-0.62, -0.09), (-0.09, -0.09)], 45.));
    },
    "Exclamation" => |b| { b.fill(vec![(-0.13, -0.5), (0.13, -0.5), (0.08, 0.17), (-0.08, 0.17)]).fill(circle(0., 0.38, 0.11)); },
    "Question Mark" => |b| {
        let mut line = arc(0., -0.2, 0.23, 0.23, 160., 400.);
        line.extend([(0.06, 0.02), (0., 0.1), (0., 0.17)]);
        b.fills(stroke(&line, 0.13)).fill(circle(0., 0.38, 0.09));
    },
    "No Sign" => |b| {
        b.fill(circle(0., 0., 0.5)).hole(circle(0., 0., 0.4)).fill(seg(-0.3, -0.3, 0.3, 0.3, 0.1));
    },
    "Crown" => |b| {
        b.fill(vec![(-0.5, 0.4), (-0.5, -0.25), (-0.25, 0.05), (0., -0.4), (0.25, 0.05), (0.5, -0.25), (0.5, 0.4)]);
        b.fill(circle(-0.44, -0.28, 0.06)).fill(circle(0., -0.43, 0.07)).fill(circle(0.44, -0.28, 0.06));
        b.hole(rect(-0.4, 0.22, 0.8, 0.05));
    },
    "Gem" => |b| {
        b.fill(vec![(-0.5, -0.2), (-0.3, -0.45), (0.3, -0.45), (0.5, -0.2), (0., 0.5)]);
        b.hole(seg(-0.4, -0.2, 0.4, -0.2, 0.03)).hole(seg(-0.2, -0.12, 0., 0.34, 0.03)).hole(seg(0.2, -0.12, 0., 0.34, 0.03));
    },
    "Shield" => |b| { b.fill(pen(0., -0.5).c(0.2, -0.4, 0.35, -0.38, 0.5, -0.42).l(0.5, 0.).c(0.5, 0.25, 0.25, 0.42, 0., 0.5).sym()); },
    "Key" => |b| {
        b.fill(circle(-0.3, 0., 0.2)).hole(circle(-0.3, 0., 0.1));
        b.fill(rect(-0.12, -0.05, 0.6, 0.1)).fill(rect(0.24, 0.05, 0.08, 0.16)).fill(rect(0.38, 0.05, 0.08, 0.11));
    },
    "Padlock" => |b| {
        b.fill(rrect(-0.35, -0.05, 0.7, 0.55, 0.06));
        b.fill(pen(-0.28, 0.).l(-0.28, -0.1).arc(0., -0.1, 0.28, 0.28, 180., 360.).l(0.28, 0.).l(0.16, 0.).l(0.16, -0.1)
            .arc(0., -0.1, 0.16, 0.16, 360., 180.).l(-0.16, 0.).done());
        let (r, s) = (0.075f32, 0.03f32);
        let y = (r * r - s * s).sqrt();
        let a = y.atan2(s).to_degrees();
        b.hole(pen(s, 0.18 + y).arc(0., 0.18, r, r, a, 180. - a - 360.).l(-s * 1.4, 0.4).l(s * 1.4, 0.4).done());
    },
    "Bell" => |b| {
        b.fill(pen(0., -0.42).c(0.2, -0.42, 0.28, -0.25, 0.28, 0.).c(0.28, 0.2, 0.35, 0.28, 0.45, 0.32).l(0.45, 0.36).l(-0.45, 0.36).l(-0.45, 0.32)
            .c(-0.35, 0.28, -0.28, 0.2, -0.28, 0.).c(-0.28, -0.25, -0.2, -0.42, 0., -0.42).done());
        b.fill(circle(0., 0.44, 0.08)).fill(circle(0., -0.44, 0.06));
    },
    "Speech Bubble" => |b| { b.fill(rrect(-0.5, -0.45, 1., 0.7, 0.2)).fill(vec![(-0.25, 0.2), (-0.32, 0.5), (0.05, 0.2)]); },
    "Thought Bubble" => |b| {
        b.fill(rrect(-0.5, -0.38, 1., 0.46, 0.23)).fill(circle(-0.25, -0.2, 0.22)).fill(circle(0.05, -0.28, 0.24)).fill(circle(0.3, -0.18, 0.2));
        b.fill(circle(-0.2, 0.28, 0.09)).fill(circle(-0.32, 0.42, 0.055));
    },
    "Eye" => |b| {
        b.fill(pen(-0.5, 0.).c(-0.25, -0.4, 0.25, -0.4, 0.5, 0.).c(0.25, 0.4, -0.25, 0.4, -0.5, 0.).done());
        b.hole(circle(0., 0., 0.2)).fill(circle(0., 0., 0.09));
    },
    "Music Note" => |b| {
        b.fill(rot_at(ellipse(-0.2, 0.32, 0.2, 0.14), -20., -0.2, 0.32)).fill(rect(-0.03, -0.45, 0.07, 0.8));
        b.fill(pen(0.04, -0.45).c(0.2, -0.3, 0.4, -0.25, 0.35, 0.).c(0.3, -0.2, 0.18, -0.2, 0.04, -0.2).done());
    },
    "Anchor" => |b| {
        b.fill(circle(0., -0.38, 0.12)).hole(circle(0., -0.38, 0.06));
        b.fill(rect(-0.04, -0.28, 0.08, 0.7)).fill(rect(-0.2, -0.2, 0.4, 0.07)).fill(band(0., 0.05, 0.32, 0.4, 10., 170.));
        let fluke = vec![(0.3, 0.12), (0.5, 0.08), (0.42, -0.08)];
        b.fill(fluke.clone()).fill(flip_x(fluke));
    },
    "Flag" => |b| {
        b.fill(rect(-0.45, -0.5, 0.06, 1.));
        b.fill(pen(-0.39, -0.45).c(-0.1, -0.6, 0.1, -0.2, 0.45, -0.35).l(0.45, 0.05).c(0.1, -0.1, -0.1, 0.3, -0.39, 0.15).done());
    },
    "Banner" => |b| { b.fill(vec![(-0.5, -0.2), (0.5, -0.2), (0.38, 0.), (0.5, 0.2), (-0.5, 0.2), (-0.38, 0.)]); },
    "Gear" => |b| {
        let mut pts = Vec::new();
        for k in 0..8 {
            for (da, r) in [(-15., 0.38), (-9., 0.5), (9., 0.5), (15., 0.38)] {
                let a = (k as f32 * 45. + da).to_radians();
                pts.push((r * a.cos(), r * a.sin()));
            }
        }
        b.fill(pts).hole(circle(0., 0., 0.15));
    },
    "Crosshair" => |b| {
        b.fill(circle(0., 0., 0.38)).hole(circle(0., 0., 0.3)).fills(spin(&rect(-0.035, -0.5, 0.07, 0.28), 4)).fill(circle(0., 0., 0.05));
    },
    "Pause" => |b| { b.fill(rect(-0.32, -0.4, 0.24, 0.8)).fill(rect(0.08, -0.4, 0.24, 0.8)); },
    "Smiley" => |b| {
        b.fill(circle(0., 0., 0.5)).hole(ellipse(-0.17, -0.12, 0.07, 0.1)).hole(ellipse(0.17, -0.12, 0.07, 0.1)).hole(band(0., 0., 0.27, 0.35, 25., 155.));
    },
    "Rocket" => |b| {
        b.fill(pen(0., -0.5).c(0.2, -0.35, 0.22, -0.05, 0.18, 0.28).l(0.12, 0.36).l(0., 0.36).sym());
        let fin = vec![(0.18, 0.), (0.4, 0.3), (0.4, 0.4), (0.17, 0.26)];
        b.fill(fin.clone()).fill(flip_x(fin));
        b.fill(vec![(-0.08, 0.38), (0., 0.5), (0.08, 0.38)]);
        b.hole(circle(0., -0.12, 0.08));
    },
    "Ghost" => |b| {
        let mut pts = pen(-0.38, 0.4).l(-0.38, -0.1).arc(0., -0.1, 0.38, 0.38, 180., 360.).l(0.38, 0.4).done();
        pts.extend((0..=24).map(|i| {
            let t = i as f32 / 24.;
            (0.38 - 0.76 * t, 0.4 + 0.08 * (t * 3. * std::f32::consts::TAU).cos().min(0.999))
        }));
        b.fill(pts).hole(ellipse(-0.14, -0.12, 0.06, 0.09)).hole(ellipse(0.14, -0.12, 0.06, 0.09)).hole(ellipse(0., 0.1, 0.05, 0.07));
    },
    "Bomb" => |b| {
        b.fill(circle(-0.05, 0.1, 0.38)).fill(rot_at(rect(0.1, -0.3, 0.16, 0.12), 40., 0.18, -0.24));
        b.fills(stroke(&[(0.22, -0.3), (0.3, -0.42), (0.4, -0.38)], 0.04)).fill(star(0.41, -0.41, 0.09, 0.035, 4, 0.));
        b.hole(rot_at(ellipse(-0.22, -0.02, 0.05, 0.11), 30., -0.22, -0.02));
    },
    "Mustache" => |b| {
        b.fill(pen(0., -0.06).c(0.08, -0.18, 0.3, -0.22, 0.5, 0.).c(0.45, 0.18, 0.3, 0.12, 0.25, 0.02).c(0.15, 0.02, 0.06, 0.14, 0., 0.12).sym());
    },

    // ---- frames for letters and numbers ----
    "Round Badge" => |b| {
        b.fill(circle(0., 0., 0.5)).hole(circle(0., 0., 0.44)).fill(circle(0., 0., 0.4)).hole(circle(0., 0., 0.33));
    },
    "Shield Frame" => |b| {
        let shield = |k: f32| scale(pen(0., -0.5).c(0.2, -0.4, 0.35, -0.38, 0.5, -0.42).l(0.5, 0.).c(0.5, 0.25, 0.25, 0.42, 0., 0.5).sym(), k, k);
        b.fill(shield(1.)).hole(shift(shield(0.74), 0., 0.02));
    },
    "Hex Frame" => |b| { b.fill(ngon(0., 0., 0.5, 6, 30.)).hole(ngon(0., 0., 0.38, 6, 30.)); },
    "Octagon Frame" => |b| { b.fill(ngon(0., 0., 0.54, 8, 22.5)).hole(ngon(0., 0., 0.42, 8, 22.5)); },
    "Diamond Frame" => |b| { b.fill(ngon(0., 0., 0.5, 4, 0.)).hole(ngon(0., 0., 0.34, 4, 0.)); },
    "Oval Frame" => |b| { b.fill(ellipse(0., 0., 0.5, 0.36)).hole(ellipse(0., 0., 0.4, 0.26)); },
    "Plaque" => |b| {
        let chamfer = |h: f32, w: f32, c: f32| vec![(-w, -h + c), (-w + c, -h), (w - c, -h), (w, -h + c), (w, h - c), (w - c, h), (-w + c, h), (-w, h - c)];
        b.fill(chamfer(0.4, 0.5, 0.1)).hole(chamfer(0.3, 0.4, 0.05));
    },
    "Ticket" => |b| {
        b.fill(pen(-0.5, -0.3).l(0.5, -0.3).l(0.5, -0.1).arc(0.5, 0., 0.1, 0.1, 270., 90.).l(0.5, 0.3).l(-0.5, 0.3).l(-0.5, 0.1)
            .arc(-0.5, 0., 0.1, 0.1, 90., -90.).l(-0.5, -0.3).done());
        b.hole(rect(-0.38, -0.2, 0.76, 0.4));
    },
    "Tag" => |b| {
        b.fill(vec![(-0.5, -0.3), (0.2, -0.3), (0.5, 0.), (0.2, 0.3), (-0.5, 0.3)]);
        b.hole(vec![(-0.4, -0.2), (0.15, -0.2), (0.36, 0.), (0.15, 0.2), (-0.4, 0.2)]);
    },
    "Banner Frame" => |b| {
        b.fill(vec![(-0.5, -0.25), (0.5, -0.25), (0.38, 0.), (0.5, 0.25), (-0.5, 0.25), (-0.38, 0.)]);
        b.hole(rect(-0.3, -0.15, 0.6, 0.3));
    },

    // ---- patterns ----
    "Stripes" => |b| {
        for k in 0..4 {
            b.fill(rect(-0.5, -0.44 + 0.24 * k as f32, 1., 0.16));
        }
    },
    "Diagonal Stripes" => |b| {
        for k in 0..5 {
            let c0 = -0.9 + 0.4 * k as f32;
            b.fill(clip(clip(rect(-0.5, -0.5, 1., 1.), 1., -1., c0 + 0.2), -1., 1., -c0));
        }
    },
    "Chevron Stripes" => |b| {
        for y in [-0.45, -0.15, 0.15] {
            b.fill(vec![(-0.5, y), (0., y + 0.2), (0.5, y), (0.5, y + 0.12), (0., y + 0.32), (-0.5, y + 0.12)]);
        }
    },
    "Checkerboard" => |b| {
        for i in 0..4 {
            for j in 0..4 {
                if (i + j) % 2 == 0 {
                    b.fill(rect(-0.5 + 0.25 * i as f32, -0.5 + 0.25 * j as f32, 0.25, 0.25));
                }
            }
        }
    },
    "Dots" => |b| {
        for i in 0..3 {
            for j in 0..3 {
                b.fill(circle(-0.38 + 0.38 * i as f32, -0.38 + 0.38 * j as f32, 0.12));
            }
        }
    },
    "Zigzag" => |b| {
        let line: Vec<P> = (0..7).map(|i| (-0.45 + 0.15 * i as f32, if i % 2 == 0 { 0.2 } else { -0.2 })).collect();
        b.fills(stroke(&line, 0.1));
    },
    "Wave" => |b| {
        let line: Vec<P> = (0..=48).map(|i| {
            let t = i as f32 / 48.;
            (-0.45 + 0.9 * t, 0.18 * (t * 2. * std::f32::consts::TAU).sin())
        }).collect();
        b.fills(stroke(&line, 0.1));
    },
    "Hash" => |b| {
        for k in [-0.17, 0.17] {
            b.fill(rect(k - 0.05, -0.5, 0.1, 1.)).fill(rect(-0.5, k - 0.05, 1., 0.1));
        }
    },
    "Concentric Squares" => |b| {
        b.fill(rect(-0.5, -0.5, 1., 1.)).hole(rect(-0.4, -0.4, 0.8, 0.8)).fill(rect(-0.3, -0.3, 0.6, 0.6)).hole(rect(-0.2, -0.2, 0.4, 0.4)).fill(rect(-0.1, -0.1, 0.2, 0.2));
    },
    "Halftone" => |b| {
        for row in 0..4 {
            for col in 0..4 {
                b.fill(circle(-0.375 + 0.25 * col as f32, -0.375 + 0.25 * row as f32, 0.03 + 0.03 * row as f32));
            }
        }
    },
    "Scales" => |b| {
        for row in 0..4 {
            for col in 0..4 {
                let x = -0.375 + 0.25 * col as f32 + if row % 2 == 1 { 0.125 } else { 0. };
                if x > 0.4 {
                    continue;
                }
                b.fills(stroke(&arc(x, -0.33 + 0.2 * row as f32, 0.115, 0.115, 0., 180.), 0.045));
            }
        }
    },
    "Honeycomb" => |b| {
        let (r, gap) = (0.17f32, 0.045f32);
        let mut centres = vec![(0., 0.)];
        centres.extend((0..6).map(|k| {
            let a = (k as f32 * 60.).to_radians();
            (r * 3f32.sqrt() * a.cos(), r * 3f32.sqrt() * a.sin())
        }));
        for (x, y) in centres {
            b.fill(ngon(x, y, r, 6, 30.)).hole(ngon(x, y, r - gap, 6, 30.));
        }
    },
    "Spiral" => |b| {
        let line: Vec<P> = (0..=120).map(|i| {
            let t = i as f32 / 120.;
            let (r, a) = (0.04 + 0.4 * t, t * 3. * std::f32::consts::TAU);
            (r * a.cos(), r * a.sin())
        }).collect();
        b.fills(stroke(&line, 0.07));
    },
    "Sawtooth" => |b| {
        b.fill(vec![(-0.5, 0.3), (-0.375, -0.3), (-0.25, 0.3), (-0.125, -0.3), (0., 0.3), (0.125, -0.3), (0.25, 0.3), (0.375, -0.3), (0.5, 0.3)]);
    },

    // ---- skating ----
    "Deck" => |b| {
        b.fill(rrect(-0.2, -0.5, 0.4, 1., 0.2));
        for (x, y) in [(-0.1, -0.32), (0.1, -0.32), (-0.1, -0.24), (0.1, -0.24), (-0.1, 0.24), (0.1, 0.24), (-0.1, 0.32), (0.1, 0.32)] {
            b.hole(circle(x, y, 0.018));
        }
    },
    "Board Side" => |b| {
        b.fills(stroke(&[(-0.465, -0.14), (-0.42, -0.02), (-0.34, 0.05), (0.34, 0.05), (0.42, -0.02), (0.465, -0.14)], 0.07));
        for x in [-0.3, 0.3] {
            b.fill(rect(x - 0.05, 0.08, 0.1, 0.1)).fill(circle(x, 0.28, 0.13)).hole(circle(x, 0.28, 0.045));
        }
    },
    "Wheel" => |b| {
        b.fill(circle(0., 0., 0.5)).hole(circle(0., 0., 0.34)).fill(circle(0., 0., 0.22)).hole(circle(0., 0., 0.07));
    },
    "Truck" => |b| {
        b.fill(rrect(-0.3, -0.42, 0.6, 0.12, 0.03)).fill(rect(-0.06, -0.3, 0.12, 0.2));
        b.fill(vec![(-0.1, -0.1), (0.1, -0.1), (0.3, 0.1), (0.5, 0.1), (0.5, 0.22), (-0.5, 0.22), (-0.5, 0.1), (-0.3, 0.1)]);
        b.hole(circle(-0.2, -0.36, 0.02)).hole(circle(0.2, -0.36, 0.02));
    },
    "Skate Shoe" => |b| {
        b.fill(pen(-0.5, -0.2).l(-0.5, 0.2).l(0.5, 0.2).c(0.5, 0.05, 0.45, -0.02, 0.3, -0.05).c(0.15, -0.08, 0.05, -0.2, -0.05, -0.3)
            .l(-0.2, -0.32).l(-0.3, -0.2).done());
        b.hole(circle(0.05, -0.12, 0.022)).hole(circle(0.15, -0.06, 0.022)).hole(circle(0.25, -0.01, 0.022));
        b.hole(seg(-0.46, 0.14, 0.46, 0.14, 0.02));
    },
    "Helmet" => |b| {
        b.fill(pen(-0.5, 0.2).arc(0., 0.2, 0.5, 0.5, 180., 360.).l(0.5, 0.32).l(-0.5, 0.32).done());
        for x in [-0.25, -0.05, 0.15] {
            b.hole(rrect(x, -0.15, 0.1, 0.28, 0.04));
        }
    },
    "Half Pipe" => |b| {
        b.fill(pen(-0.5, -0.4).l(-0.5, 0.5).l(0.5, 0.5).l(0.5, -0.4).l(0.38, -0.4).arc(0., -0.4, 0.38, 0.7, 0., 180.).l(-0.38, -0.4).done());
    },
    "Quarter Pipe" => |b| { b.fill(pen(-0.5, 0.5).l(0.5, 0.5).l(0.5, -0.5).arc(-0.5, -0.5, 1., 1., 0., 90.).done()); },
    "Stairs" => |b| {
        b.fill(vec![(-0.5, 0.5), (-0.5, 0.2), (-0.25, 0.2), (-0.25, -0.05), (0., -0.05), (0., -0.3), (0.25, -0.3), (0.25, -0.5), (0.5, -0.5), (0.5, 0.5)]);
    },
    "Handrail" => |b| {
        b.fill(seg(-0.5, -0.2, 0.5, -0.2, 0.07)).fill(rect(-0.4, -0.17, 0.05, 0.67)).fill(rect(0.35, -0.17, 0.05, 0.67));
        b.fill(rect(-0.5, 0.45, 1., 0.05));
    },
    "Kicker" => |b| { b.fill(pen(-0.5, 0.4).l(0.5, 0.4).l(0.5, -0.35).q(0., 0.3, -0.5, 0.3).done()); },
    "Bowl" => |b| {
        b.fill(pen(-0.5, -0.3).l(-0.5, 0.5).l(0.5, 0.5).l(0.5, -0.3).l(0.42, -0.3).arc(0.12, -0.1, 0.3, 0.3, 0., 90.).l(-0.12, 0.2)
            .arc(-0.12, -0.1, 0.3, 0.3, 90., 180.).l(-0.42, -0.3).done());
    },
    "Bearing" => |b| {
        b.fill(circle(0., 0., 0.5)).hole(circle(0., 0., 0.38)).fill(circle(0., 0., 0.28)).hole(circle(0., 0., 0.12));
        for k in 0..8 {
            let a = (k as f32 * 45.).to_radians();
            b.fill(circle(0.33 * a.cos(), 0.33 * a.sin(), 0.05));
        }
    },
    "Cap" => |b| {
        b.fill(pen(-0.45, 0.22).arc(-0.05, 0.22, 0.4, 0.4, 180., 360.).l(0.35, 0.22).done());
        b.fill(vec![(0.2, 0.12), (0.5, 0.2), (0.5, 0.3), (0.1, 0.28)]).fill(circle(-0.05, -0.2, 0.04));
        b.hole(seg(-0.4, 0.12, 0.3, 0.12, 0.025));
    },
    "Spray Can" => |b| {
        b.fill(rrect(-0.2, -0.2, 0.4, 0.7, 0.05)).fill(rect(-0.12, -0.3, 0.24, 0.1)).fill(rect(-0.07, -0.42, 0.14, 0.12));
        b.hole(rect(-0.14, 0., 0.28, 0.25));
        b.fill(circle(0.25, -0.38, 0.04)).fill(circle(0.36, -0.44, 0.035)).fill(circle(0.36, -0.32, 0.035)).fill(circle(0.46, -0.38, 0.03));
    },
    "Boombox" => |b| {
        b.fill(rrect(-0.5, -0.2, 1., 0.6, 0.06));
        b.hole(circle(-0.28, 0.12, 0.15)).hole(circle(0.28, 0.12, 0.15)).hole(rect(-0.12, -0.1, 0.24, 0.14));
        b.fills(stroke(&arc(0., -0.2, 0.32, 0.25, 180., 360.), 0.05));
    },
    "Headphones" => |b| {
        b.fill(band(0., 0.05, 0.4, 0.47, 180., 360.)).fill(rrect(-0.5, 0.02, 0.14, 0.44, 0.06)).fill(rrect(0.36, 0.02, 0.14, 0.44, 0.06));
    },
}

/// The number of shapes here.
pub(crate) const COUNT: usize = TABLE.len();

/// Every name, in index order.
pub(crate) const NAMES: [&str; COUNT] = {
    let mut names = [""; COUNT];
    let mut i = 0;
    while i < COUNT {
        names[i] = TABLE[i].0;
        i += 1;
    }
    names
};

/// Shape `index` (0 is the first of these) as a path in the unit box, to be filled with the non-zero rule.
pub(crate) fn path(index: usize) -> Option<Path> {
    let mut b = B::default();
    (TABLE.get(index)?.1)(&mut b);
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_builds_inside_the_unit_box_with_a_real_size_and_its_own_name() {
        let mut seen = std::collections::HashSet::new();
        for (i, name) in NAMES.iter().enumerate() {
            assert!(!name.is_empty() && seen.insert(*name), "{i} {name}");
            let bounds = path(i).unwrap_or_else(|| panic!("{name} has no path")).bounds();
            assert!(bounds.left() >= -0.52 && bounds.top() >= -0.52 && bounds.right() <= 0.52 && bounds.bottom() <= 0.52, "{name} {bounds:?}");
            assert!(bounds.width() >= 0.3 || bounds.height() >= 0.3, "{name} is a speck {bounds:?}");
        }
        assert!(path(COUNT).is_none());
    }
}
