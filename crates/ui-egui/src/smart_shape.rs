//! Smart Shape (Brush and Pencil options bar, off by default): draw a stroke, keep the pointer
//! still at its end for [`DEFAULT_HOLD_MS`] (configurable), and the stroke becomes a clean
//! straight line, smooth (possibly wavy) curve, straight-edged polyline, circle, ellipse, triangle,
//! quadrilateral (perspective-like four-sided forms included), rectangle or square. While still
//! held, moving the pointer scales and turns the corrected shape about its centre (closed shapes)
//! or its start (lines, curves and polylines, whose end follows the pointer). Releasing commits
//! only the shape, as one `paint.stroke` and one Undo step; releasing before the hold keeps the
//! freehand stroke exactly as drawn.
//!
//! The shape goes through the normal drag / `LiveStroke` / commit path: its points replace the
//! drag's, the live preview restarts from the document (so the freehand marks vanish rather than
//! staying underneath), and the commit uses the same brush. Pen pressure, tilt and rotation are
//! carried over by mapping each shape point to the freehand point at the same fraction of the
//! stroke's length. Brush smoothing is skipped for a shape (it would round its corners).
//!
//! Recognition is a clean-room implementation of published methods; no third-party code is used:
//! - arc-length resampling of the stroke to a fixed number of points;
//! - straight lines: total least squares (principal component of the point covariance);
//! - curves: tangent-continuous cubic Béziers fitted by least squares with chord-length
//!   parameters and Newton reparameterisation, split at the worst point until each piece is close
//!   enough (P. J. Schneider, "An Algorithm for Automatically Fitting Digitized Curves", Graphics
//!   Gems, 1990), so a wavy line keeps every wave;
//! - circles and ellipses: centre and axes from the principal axes of the closed stroke, extents
//!   in that frame, accepted on the mean normalised radial error;
//! - rectangles and squares: corners as local maxima of the turning angle over a window, the
//!   orientation from the corner-to-corner edges (angles averaged modulo 90°), sides from the
//!   outline points nearest to them;
//! - triangles, quadrilaterals and open polylines: Ramer–Douglas–Peucker simplification (closed
//!   outlines split at their two farthest points), vertices kept only where the stroke turns
//!   sharply, then made exact as the meeting points of lines fitted to each side;
//! - closed candidates compete on the mean distance of the stroke from their outline, and each
//!   also has its own limit; anything nothing fits (handwriting, zig-zags, spirals, tiny marks)
//!   stays a freehand stroke.

use std::time::Duration;

use crate::PhotocraftApp;
use crate::canvas::Drag;
use crate::state::Tool;

/// Default hold before the stroke is corrected (ms).
pub const DEFAULT_HOLD_MS: f32 = 700.0;
/// Range the hold can be set to (ms).
pub const HOLD_RANGE: std::ops::RangeInclusive<f32> = 300.0..=2000.0;
/// The pointer counts as still within this many screen points of where it came to rest (a hand or
/// pen trembles a little while holding).
pub const STILL_RADIUS: f64 = 6.0;
/// A stroke smaller than this (screen points, bounding box diagonal) is never corrected.
pub const MIN_EXTENT: f64 = 16.0;
pub const TOOLTIP: &str = "Smart Shape — Hold at the end of a stroke to perfect its shape";

/// Points the stroke is resampled to for recognition.
const SAMPLES: usize = 64;
/// Normalised error limits (fractions of the stroke length or size).
const LINE_TOL: f64 = 0.03;
/// Largest distance of a smoothed curve from the stroke, as a fraction of its size (at least
/// [`CURVE_TOL_MIN`] document pixels; each cubic piece aims for it, up to twice is accepted once
/// the pieces run out). Tight enough that waves keep their height.
const CURVE_TOL: f64 = 0.012;
const CURVE_TOL_MIN: f64 = 1.5;
const ELLIPSE_TOL: f64 = 0.085;
const RECT_TOL: f64 = 0.05;
/// A stroke whose ends are closer than this fraction of its length is a closed shape.
const CLOSED_GAP: f64 = 0.15;
/// A local turn above this (radians) is a corner.
const CORNER_TURN: f64 = 0.96; // 55°
/// Spacing of the generated shape points (document pixels).
const SPACING: f64 = 2.0;
/// Mean distance of the stroke from a fitted polygon, as a fraction of the stroke's size.
const POLY_TOL: f64 = 0.035;
/// A polygon vertex must turn at least this much (radians, over the corner window): rounder
/// bends are curves, not corners.
const VERTEX_TURN: f64 = 0.6; // 34°
/// Most cubic pieces a smoothed curve may have, and most vertices of a polygon or polyline.
pub const MAX_SEGMENTS: usize = 8;
pub const MAX_POLY: usize = 5;

type P = [f64; 2];

/// A corrected shape, in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Line {
        a: P,
        b: P,
    },
    /// A smooth curve: `n` cubic Béziers (start, two controls, end), each starting where the last
    /// ended with the same tangent.
    Spline {
        seg: [[P; 4]; MAX_SEGMENTS],
        n: usize,
    },
    /// Straight edges through `n` vertices: a triangle or a quadrilateral when `closed`, else an
    /// open polyline (an L, a V, a zig of up to four edges).
    Poly {
        v: [P; MAX_POLY],
        n: usize,
        closed: bool,
    },
    /// A circle when `rx == ry`.
    Ellipse {
        c: P,
        rx: f64,
        ry: f64,
        angle: f64,
        start: f64,
        ccw: bool,
    },
    /// A square when `hw == hh`. `start` is the corner the outline starts from (0..4).
    Rect {
        c: P,
        hw: f64,
        hh: f64,
        angle: f64,
        start: usize,
        ccw: bool,
    },
}

impl Shape {
    /// The kind, as a user would name it.
    pub fn kind(&self) -> &'static str {
        match self {
            Shape::Line { .. } => "line",
            Shape::Spline { .. } => "curve",
            Shape::Poly { n: 3, closed: true, .. } => "triangle",
            Shape::Poly { closed: true, .. } => "quadrilateral",
            Shape::Poly { .. } => "polyline",
            Shape::Ellipse { rx, ry, .. } if (rx - ry).abs() <= 1e-9 => "circle",
            Shape::Ellipse { .. } => "ellipse",
            Shape::Rect { hw, hh, .. } if (hw - hh).abs() <= 1e-9 => "square",
            Shape::Rect { .. } => "rectangle",
        }
    }

    /// The point the shape scales and turns about while it is adjusted.
    fn pivot(&self) -> P {
        match self {
            Shape::Line { a, .. } => *a,
            Shape::Spline { seg, .. } => seg[0][0],
            Shape::Poly { v, closed: false, .. } => v[0],
            Shape::Poly { v, n, closed: true } => mean(v.get(..*n).unwrap_or_default()),
            Shape::Ellipse { c, .. } | Shape::Rect { c, .. } => *c,
        }
    }

    /// The end a line, curve or polyline is dragged by while adjusting (`None`: closed shapes).
    fn end(&self) -> Option<P> {
        match self {
            Shape::Line { b, .. } => Some(*b),
            Shape::Spline { seg, n } => seg.get(n.saturating_sub(1)).map(|s| s[3]),
            Shape::Poly { v, n, closed: false } => v.get(n.saturating_sub(1)).copied(),
            _ => None,
        }
    }

    /// The shape scaled by `s` and turned by `rot` radians about `pivot`.
    fn transformed(&self, pivot: P, s: f64, rot: f64) -> Shape {
        let (sin, cos) = rot.sin_cos();
        let map = |q: P| {
            let (dx, dy) = (q[0] - pivot[0], q[1] - pivot[1]);
            [pivot[0] + s * (dx * cos - dy * sin), pivot[1] + s * (dx * sin + dy * cos)]
        };
        match *self {
            Shape::Line { a, b } => Shape::Line { a: map(a), b: map(b) },
            Shape::Spline { seg, n } => Shape::Spline { seg: seg.map(|s| s.map(map)), n },
            Shape::Poly { v, n, closed } => Shape::Poly { v: v.map(map), n, closed },
            Shape::Ellipse { c, rx, ry, angle, start, ccw } => Shape::Ellipse { c: map(c), rx: rx * s, ry: ry * s, angle: angle + rot, start, ccw },
            Shape::Rect { c, hw, hh, angle, start, ccw } => Shape::Rect { c: map(c), hw: hw * s, hh: hh * s, angle: angle + rot, start, ccw },
        }
    }

    /// The outline as a polyline, about [`SPACING`] apart; closed shapes end where they start.
    pub fn points(&self) -> Vec<P> {
        let count = |len: f64, min: usize, max: usize| ((len / SPACING).ceil() as usize).clamp(min, max);
        match *self {
            Shape::Line { a, b } => {
                let n = count(dist(a, b), 2, 2000);
                (0..n).map(|i| lerp(a, b, i as f64 / (n - 1) as f64)).collect()
            }
            Shape::Spline { seg, n } => {
                let mut out = Vec::new();
                for p in seg.iter().take(n) {
                    let rough: f64 = (0..16).map(|i| dist(bezier(p, i as f64 / 16.0), bezier(p, (i + 1) as f64 / 16.0))).sum();
                    let m = count(rough, 4, 4000);
                    // Each piece starts where the last ended: that point is kept once.
                    let skip = usize::from(!out.is_empty());
                    out.extend((skip..m).map(|i| bezier(p, i as f64 / (m - 1) as f64)));
                }
                out
            }
            Shape::Poly { v, n, closed } => {
                let v = v.get(..n).unwrap_or_default();
                let mut out = Vec::new();
                let edges = if closed { n } else { n.saturating_sub(1) };
                for k in 0..edges {
                    let (Some(&a), Some(&b)) = (v.get(k), v.get((k + 1) % n.max(1))) else { break };
                    let m = count(dist(a, b), 2, 2000);
                    out.extend((0..m - 1).map(|s| lerp(a, b, s as f64 / (m - 1) as f64)));
                }
                // The last vertex (open) or the first again (closed) ends the outline exactly.
                if let Some(&last) = if closed { v.first() } else { v.last() } {
                    out.push(last);
                }
                out
            }
            Shape::Ellipse { c, rx, ry, angle, start, ccw } => {
                // Ramanujan's perimeter approximation.
                let h = ((rx - ry) / (rx + ry).max(1e-9)).powi(2);
                let perimeter = std::f64::consts::PI * (rx + ry) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).max(0.0).sqrt()));
                let n = count(perimeter, 24, 6000);
                let dir = if ccw { -1.0 } else { 1.0 };
                let (sin, cos) = angle.sin_cos();
                (0..=n)
                    .map(|i| {
                        let t = start + dir * std::f64::consts::TAU * i as f64 / n as f64;
                        let (x, y) = (rx * t.cos(), ry * t.sin());
                        [c[0] + x * cos - y * sin, c[1] + x * sin + y * cos]
                    })
                    .collect()
            }
            Shape::Rect { c, hw, hh, angle, start, ccw } => {
                let corners = rect_corners(c, hw, hh, angle);
                let mut out = Vec::new();
                for k in 0..4 {
                    let (i, j) = if ccw { ((start + 4 - k) % 4, (start + 7 - k) % 4) } else { ((start + k) % 4, (start + k + 1) % 4) };
                    let (a, b) = (corners[i], corners[j]);
                    let n = count(dist(a, b), 2, 2000);
                    // Each edge's last point is the next edge's first: keep it once.
                    out.extend((0..n - 1).map(|s| lerp(a, b, s as f64 / (n - 1) as f64)));
                }
                if let Some(first) = out.first().copied() {
                    out.push(first);
                }
                out
            }
        }
    }
}

/// The four corners of a rectangle, in order around it.
fn rect_corners(c: P, hw: f64, hh: f64, angle: f64) -> [P; 4] {
    let (sin, cos) = angle.sin_cos();
    [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]].map(|q| [c[0] + q[0] * cos - q[1] * sin, c[1] + q[0] * sin + q[1] * cos])
}

// ---------------------------------------------------------------------------------- geometry

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn dist(a: P, b: P) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn lerp(a: P, b: P, t: f64) -> P {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn path_len(pts: &[P]) -> f64 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Cumulative arc length at each point, as a fraction of the whole (0 at the start, 1 at the end).
fn arc_fractions(pts: &[P]) -> Vec<f64> {
    let mut acc = 0.0;
    let mut out = Vec::with_capacity(pts.len());
    let mut prev: Option<P> = None;
    for &p in pts {
        if let Some(q) = prev {
            acc += dist(q, p);
        }
        out.push(acc);
        prev = Some(p);
    }
    if acc > 0.0 {
        out.iter_mut().for_each(|f| *f /= acc);
    }
    out
}

/// `pts` resampled to `n` points evenly spaced along its length.
fn resample(pts: &[P], n: usize) -> Vec<P> {
    let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else { return Vec::new() };
    let total = path_len(pts);
    if n < 2 || total <= 1e-9 {
        return vec![first; n.max(1)];
    }
    let step = total / (n - 1) as f64;
    let mut out = vec![first];
    let mut need = step;
    let mut walked = 0.0;
    for w in pts.windows(2) {
        let seg = dist(w[0], w[1]);
        while seg > 0.0 && walked + seg >= need && out.len() < n - 1 {
            out.push(lerp(w[0], w[1], (need - walked) / seg));
            need += step;
        }
        walked += seg;
    }
    while out.len() < n {
        out.push(last);
    }
    out
}

fn mean(pts: &[P]) -> P {
    let n = pts.len().max(1) as f64;
    let s = pts.iter().fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
    [s[0] / n, s[1] / n]
}

/// Principal axes of the points about `m`: (larger variance, smaller variance, major axis angle).
fn principal(pts: &[P], m: P) -> (f64, f64, f64) {
    let n = pts.len().max(1) as f64;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in pts {
        let (dx, dy) = (p[0] - m[0], p[1] - m[1]);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let (sxx, sxy, syy) = (sxx / n, sxy / n, syy / n);
    let half = (sxx + syy) / 2.0;
    let r = (((sxx - syy) / 2.0).powi(2) + sxy * sxy).sqrt();
    (half + r, (half - r).max(0.0), 0.5 * (2.0 * sxy).atan2(sxx - syy))
}

/// `p` in the frame of a shape centred at `c` and turned by `angle`.
fn local(p: P, c: P, angle: f64) -> P {
    let (sin, cos) = angle.sin_cos();
    let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
    [dx * cos + dy * sin, -dx * sin + dy * cos]
}

fn bezier(p: &[P; 4], t: f64) -> P {
    let u = 1.0 - t;
    let (b0, b1, b2, b3) = (u * u * u, 3.0 * t * u * u, 3.0 * t * t * u, t * t * t);
    [b0 * p[0][0] + b1 * p[1][0] + b2 * p[2][0] + b3 * p[3][0], b0 * p[0][1] + b1 * p[1][1] + b2 * p[2][1] + b3 * p[3][1]]
}

fn bezier_d1(p: &[P; 4], t: f64) -> P {
    let u = 1.0 - t;
    let (a, b, c) = (3.0 * u * u, 6.0 * u * t, 3.0 * t * t);
    [a * (p[1][0] - p[0][0]) + b * (p[2][0] - p[1][0]) + c * (p[3][0] - p[2][0]), a * (p[1][1] - p[0][1]) + b * (p[2][1] - p[1][1]) + c * (p[3][1] - p[2][1])]
}

fn bezier_d2(p: &[P; 4], t: f64) -> P {
    let u = 1.0 - t;
    let (a, b) = (6.0 * u, 6.0 * t);
    [
        a * (p[2][0] - 2.0 * p[1][0] + p[0][0]) + b * (p[3][0] - 2.0 * p[2][0] + p[1][0]),
        a * (p[2][1] - 2.0 * p[1][1] + p[0][1]) + b * (p[3][1] - 2.0 * p[2][1] + p[1][1]),
    ]
}

fn unit(v: P) -> Option<P> {
    let l = v[0].hypot(v[1]);
    (l > 1e-12 && l.is_finite()).then(|| [v[0] / l, v[1] / l])
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: P, a: P, b: P) -> f64 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 > 1e-12 { (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, lerp(a, b, t))
}

/// Mean distance from `pts` to the outline of `shape`, as a fraction of `size`: how closely the
/// candidate follows what was drawn (the score candidates are ranked by).
fn outline_error(shape: &Shape, pts: &[P], size: f64) -> f64 {
    let outline = shape.points();
    let total: f64 = pts.iter().map(|&p| outline.windows(2).map(|w| seg_dist(p, w[0], w[1])).fold(f64::MAX, f64::min)).sum();
    let e = total / pts.len().max(1) as f64 / size.max(1e-9);
    if e.is_finite() { e } else { f64::MAX }
}

/// A cubic Bézier from the first to the last point of `pts`, leaving the start along unit `t1` and
/// arriving at the end against unit `t2` (both pointing into the curve), fitted by least squares
/// with Newton reparameterisation (Schneider's method). Returns it, its largest distance from the
/// points, and the index of the point that distance is at.
fn fit_tangent(pts: &[P], t1: P, t2: P) -> Option<([P; 4], f64, usize)> {
    let (&p0, &p3) = (pts.first()?, pts.last()?);
    let fallback = (dist(p0, p3).max(path_len(pts) / 2.0)) / 3.0;
    let mut ts = arc_fractions(pts);
    let mut ctrl = [p0, [p0[0] + t1[0] * fallback, p0[1] + t1[1] * fallback], [p3[0] + t2[0] * fallback, p3[1] + t2[1] * fallback], p3];
    for _ in 0..4 {
        let (mut c11, mut c12, mut c22, mut x1, mut x2) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (&t, &p) in ts.iter().zip(pts) {
            let u = 1.0 - t;
            let (b0, b1, b2, b3) = (u * u * u, 3.0 * t * u * u, 3.0 * t * t * u, t * t * t);
            let (a1, a2) = ([t1[0] * b1, t1[1] * b1], [t2[0] * b2, t2[1] * b2]);
            let rest = [p[0] - (b0 + b1) * p0[0] - (b2 + b3) * p3[0], p[1] - (b0 + b1) * p0[1] - (b2 + b3) * p3[1]];
            c11 += dot(a1, a1);
            c12 += dot(a1, a2);
            c22 += dot(a2, a2);
            x1 += dot(rest, a1);
            x2 += dot(rest, a2);
        }
        let det = c11 * c22 - c12 * c12;
        let (mut l1, mut l2) = (fallback, fallback);
        if det.abs() > 1e-12 {
            (l1, l2) = ((c22 * x1 - c12 * x2) / det, (c11 * x2 - c12 * x1) / det);
        }
        // A negative or vanishing handle would fold the curve back: use the plain third instead.
        if !(l1 > 1e-6 && l2 > 1e-6 && l1.is_finite() && l2.is_finite()) {
            (l1, l2) = (fallback, fallback);
        }
        ctrl[1] = [p0[0] + t1[0] * l1, p0[1] + t1[1] * l1];
        ctrl[2] = [p3[0] + t2[0] * l2, p3[1] + t2[1] * l2];
        for (t, p) in ts.iter_mut().zip(pts) {
            let (q, d1, d2) = (sub(bezier(&ctrl, *t), *p), bezier_d1(&ctrl, *t), bezier_d2(&ctrl, *t));
            let den = dot(d1, d1) + dot(q, d2);
            if den.abs() > 1e-12 {
                *t = (*t - dot(q, d1) / den).clamp(0.0, 1.0);
            }
        }
    }
    let (at, err) = ts.iter().zip(pts).map(|(&t, &p)| dist(bezier(&ctrl, t), p)).enumerate().fold((0, 0.0), |m, (i, e)| if e > m.1 { (i, e) } else { m });
    (ctrl.iter().all(|c| c[0].is_finite() && c[1].is_finite()) && err.is_finite()).then_some((ctrl, err, at))
}

/// Fit `pts[lo..=hi]` with tangent-continuous cubic pieces, splitting at the worst point until
/// each piece is within `tol` (at most three splits deep, so at most [`MAX_SEGMENTS`] pieces).
/// Returns the largest error left.
#[allow(clippy::too_many_arguments)]
fn fit_pieces(pts: &[P], lo: usize, hi: usize, t1: P, t2: P, tol: f64, depth: u32, out: &mut Vec<[P; 4]>) -> f64 {
    let Some(part) = pts.get(lo..=hi) else { return f64::MAX };
    let Some((ctrl, err, at)) = fit_tangent(part, t1, t2) else { return f64::MAX };
    if err <= tol || hi - lo < 12 || depth >= 3 {
        out.push(ctrl);
        return err;
    }
    let mid = (lo + at).clamp(lo + 4, hi - 4);
    let tc = match (pts.get(mid - 1), pts.get(mid + 1)) {
        (Some(&a), Some(&b)) => unit(sub(b, a)).unwrap_or(t1),
        _ => t1,
    };
    let left = fit_pieces(pts, lo, mid, t1, [-tc[0], -tc[1]], tol, depth + 1, out);
    let right = fit_pieces(pts, mid, hi, tc, t2, tol, depth + 1, out);
    left.max(right)
}

/// A smooth curve through `pts` (no corners): one or more tangent-continuous cubic pieces within
/// `tol` of the points. Loose, wavy strokes come out as clean flowing curves.
fn fit_spline(pts: &[P], tol: f64) -> Option<Shape> {
    let n = pts.len();
    if n < 8 {
        return None;
    }
    let (&first, &last) = (pts.first()?, pts.last()?);
    let t1 = unit(sub(*pts.get(3)?, first))?;
    let t2 = unit(sub(*pts.get(n - 4)?, last))?;
    let mut pieces = Vec::new();
    let err = fit_pieces(pts, 0, n - 1, t1, t2, tol, 0, &mut pieces);
    if err > 2.0 * tol || pieces.is_empty() || pieces.len() > MAX_SEGMENTS {
        return None;
    }
    let mut seg = [[[0.0; 2]; 4]; MAX_SEGMENTS];
    for (slot, piece) in seg.iter_mut().zip(&pieces) {
        *slot = *piece;
    }
    Some(Shape::Spline { seg, n: pieces.len() })
}

/// The turn (radians) at sample `i` between the chords from `k` samples before to `k` after.
/// `closed` wraps around; an open stroke's ends have no turn.
fn local_turn(pts: &[P], i: usize, k: usize, closed: bool) -> f64 {
    let n = pts.len();
    if n == 0 {
        return 0.0;
    }
    let at = |j: isize| -> Option<P> {
        if closed { pts.get(j.rem_euclid(n as isize) as usize).copied() } else { usize::try_from(j).ok().and_then(|j| pts.get(j)).copied() }
    };
    let (i, k) = (i as isize, k as isize);
    match (at(i - k), at(i), at(i + k)) {
        (Some(a), Some(b), Some(c)) => {
            let (u, v) = (sub(b, a), sub(c, b));
            let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
            if lu < 1e-9 || lv < 1e-9 { 0.0 } else { (dot(u, v) / (lu * lv)).clamp(-1.0, 1.0).acos() }
        }
        _ => 0.0,
    }
}

/// The corner window used for `n` samples.
fn corner_window(n: usize) -> usize {
    (n / 16).max(2)
}

/// Indices of corners: local maxima of the turn over a window of `k` samples. `closed` wraps.
fn corners(pts: &[P], closed: bool) -> Vec<(usize, f64)> {
    let n = pts.len();
    let k = corner_window(n);
    if n < 2 * k + 1 {
        return Vec::new();
    }
    let range: Vec<usize> = if closed { (0..n).collect() } else { (k..n.saturating_sub(k)).collect() };
    let turns: Vec<(usize, f64)> = range.into_iter().map(|i| (i, local_turn(pts, i, k, closed))).collect();
    let mut found: Vec<(usize, f64)> = Vec::new();
    for &(i, a) in &turns {
        if a < CORNER_TURN {
            continue;
        }
        // Non-maximum suppression within the window.
        let near = |j: usize| {
            let d = i.abs_diff(j);
            if closed { d.min(n - d) <= k } else { d <= k }
        };
        if turns.iter().any(|&(j, b)| j != i && near(j) && (b > a || (b == a && j < i))) {
            continue;
        }
        found.push((i, a));
    }
    found
}

/// Ramer–Douglas–Peucker: the indices of `pts` that keep every point within `eps` of the
/// simplified polyline (the first and last are always kept), in order.
fn rdp(pts: &[P], eps: f64) -> Vec<usize> {
    let n = pts.len();
    if n < 3 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0, n - 1)];
    while let Some((s, e)) = stack.pop() {
        if e <= s + 1 {
            continue;
        }
        let (Some(&a), Some(&b)) = (pts.get(s), pts.get(e)) else { continue };
        let far = (s + 1..e).filter_map(|i| pts.get(i).map(|&p| (i, seg_dist(p, a, b)))).fold((s, 0.0), |m, (i, d)| if d > m.1 { (i, d) } else { m });
        if far.1 > eps {
            keep[far.0] = true;
            stack.push((s, far.0));
            stack.push((far.0, e));
        }
    }
    keep.iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| i).collect()
}

/// Where the lines `(p1, d1)` and `(p2, d2)` (points and unit directions) cross; `None` when they
/// are close to parallel.
fn intersect(l1: (P, P), l2: (P, P)) -> Option<P> {
    let ((p1, d1), (p2, d2)) = (l1, l2);
    let det = d1[0] * d2[1] - d1[1] * d2[0];
    if det.abs() < 0.05 {
        return None;
    }
    let w = sub(p2, p1);
    let t = (w[0] * d2[1] - w[1] * d2[0]) / det;
    Some([p1[0] + d1[0] * t, p1[1] + d1[1] * t])
}

/// The vertices of a polygon (or open polyline) whose corners are `pts[idx]`, made exact: each
/// edge becomes the straight line fitted to the middle of its points, and each vertex the meeting
/// point of its two edges (an open polyline's ends lie on their one edge). A hand's rounded or
/// overshooting corners then land where the straight sides actually meet.
fn refine_vertices(pts: &[P], idx: &[usize], closed: bool, size: f64) -> Vec<P> {
    let (n, m) = (pts.len(), idx.len());
    let edges = if closed { m } else { m.saturating_sub(1) };
    let line = |e: usize| -> Option<(P, P)> {
        let (&i, &j) = (idx.get(e)?, idx.get((e + 1) % m)?);
        let len = if j > i { j - i } else { j + n - i };
        let trim = len / 6;
        let side: Vec<P> = (trim..=len - trim).filter_map(|t| pts.get((i + t) % n).copied()).collect();
        if side.len() < 2 {
            return None;
        }
        let c = mean(&side);
        let (_, _, angle) = principal(&side, c);
        Some((c, [angle.cos(), angle.sin()]))
    };
    let lines: Vec<Option<(P, P)>> = (0..edges).map(line).collect();
    let edge = |e: Option<usize>| e.and_then(|e| lines.get(e).copied().flatten());
    idx.iter()
        .enumerate()
        .map(|(v, &i)| {
            let orig = pts.get(i).copied().unwrap_or_default();
            let (before, after) = if closed { (Some((v + m - 1) % m), Some(v)) } else { (v.checked_sub(1), (v < edges).then_some(v)) };
            match (edge(before), edge(after)) {
                (Some(l1), Some(l2)) => intersect(l1, l2).filter(|q| dist(*q, orig) < 0.25 * size).unwrap_or(orig),
                (Some((c, d)), None) | (None, Some((c, d))) => {
                    let t = dot(sub(orig, c), d);
                    [c[0] + d[0] * t, c[1] + d[1] * t]
                }
                (None, None) => orig,
            }
        })
        .collect()
}

/// A triangle or quadrilateral for the closed outline `ring`: its corners found by simplifying it
/// (RDP, split at its two farthest points) and kept only where the outline really turns, then made
/// exact by fitting its sides. A near-rectangle comes out as the rectangle.
fn fit_polygon(ring: &[P], size: f64, first: P, ccw: bool) -> Option<Shape> {
    let n = ring.len();
    if n < 12 {
        return None;
    }
    let c = mean(ring);
    let farthest = |from: P| ring.iter().enumerate().max_by(|a, b| dist(*a.1, from).total_cmp(&dist(*b.1, from))).map(|(i, _)| i);
    let a = farthest(c)?;
    let b = farthest(*ring.get(a)?)?;
    let chain = |from: usize, to: usize| -> Vec<usize> {
        let mut out = vec![from];
        let mut i = from;
        while i != to && out.len() <= n {
            i = (i + 1) % n;
            out.push(i);
        }
        out
    };
    let mut idx = Vec::new();
    for ch in [chain(a, b), chain(b, a)] {
        let side: Vec<P> = ch.iter().filter_map(|&i| ring.get(i).copied()).collect();
        let keep = rdp(&side, 0.06 * size);
        idx.extend(keep.iter().take(keep.len().saturating_sub(1)).filter_map(|&k| ch.get(k).copied()));
    }
    let k = corner_window(n);
    idx.retain(|&i| local_turn(ring, i, k, true) >= VERTEX_TURN);
    idx.sort_unstable();
    idx.dedup();
    let m = idx.len();
    if !(3..=4).contains(&m) {
        return None;
    }
    if m == 4 {
        let found: Vec<(usize, f64)> = idx.iter().map(|&i| (i, local_turn(ring, i, k, true))).collect();
        if let Some(r) = fit_rect(ring, &found, size, first, ccw) {
            return Some(r);
        }
    }
    let verts = refine_vertices(ring, &idx, true, size);
    // Start at the vertex nearest to where the stroke began, going round as it was drawn.
    let s = verts.iter().enumerate().min_by(|a, b| dist(*a.1, first).total_cmp(&dist(*b.1, first))).map_or(0, |(i, _)| i);
    let mut v = [[0.0; 2]; MAX_POLY];
    for (j, slot) in v.iter_mut().take(m).enumerate() {
        *slot = *verts.get((s + j) % m)?;
    }
    let shape = Shape::Poly { v, n: m, closed: true };
    (outline_error(&shape, ring, size) <= POLY_TOL).then_some(shape)
}

/// An open stroke with sharp corners as straight edges (an L, a V, a Z): two to four edges.
fn fit_polyline(pts: &[P], size: f64) -> Option<Shape> {
    let last = pts.len().checked_sub(1)?;
    let k = corner_window(pts.len());
    let mut idx = rdp(pts, 0.05 * size);
    idx.retain(|&i| i == 0 || i == last || local_turn(pts, i, k, false) >= VERTEX_TURN);
    let m = idx.len();
    if !(3..=MAX_POLY).contains(&m) {
        return None;
    }
    let verts = refine_vertices(pts, &idx, false, size);
    let mut v = [[0.0; 2]; MAX_POLY];
    for (slot, q) in v.iter_mut().zip(&verts) {
        *slot = *q;
    }
    let shape = Shape::Poly { v, n: m, closed: false };
    (outline_error(&shape, pts, size) <= POLY_TOL).then_some(shape)
}

/// Signed area (shoelace): negative when the outline runs counter-clockwise on screen (y down).
fn signed_area(pts: &[P]) -> f64 {
    pts.windows(2).map(|w| w[0][0] * w[1][1] - w[1][0] * w[0][1]).sum::<f64>() / 2.0
}

/// Recognise the stroke `raw` (document points) drawn at `zoom` (screen points per document
/// pixel). `None`: no shape fits well enough, keep the freehand stroke.
pub fn recognize(raw: &[P], zoom: f64) -> Option<Shape> {
    let raw: Vec<P> = raw.iter().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
    let pts = resample(&raw, SAMPLES);
    let (&first, &last) = (pts.first()?, pts.last()?);
    let (min, max) = pts.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let size = dist(min, max);
    let len = path_len(&pts);
    if size * zoom.max(1e-6) < MIN_EXTENT || len <= 1e-9 {
        return None;
    }
    let closed = dist(first, last) <= CLOSED_GAP * len;
    if !closed {
        return recognize_open(&pts, &raw, size);
    }
    // Closed: drop the closing overlap so the outline is one loop.
    let ring: Vec<P> = pts.get(..pts.len().saturating_sub(1)).unwrap_or_default().to_vec();
    let found = corners(&ring, true);
    let ccw = signed_area(&pts) < 0.0;
    // A clean four-cornered box: the rectangle or square, as before.
    if found.len() == 4
        && found.iter().all(|&(_, a)| a < 2.2)
        && let Some(r) = fit_rect(&ring, &found, size, first, ccw)
    {
        return Some(r);
    }
    // Otherwise the candidates compete: an ellipse (when the outline has no more than two sharp
    // turns) and a triangle or quadrilateral. The one that follows the stroke more closely wins;
    // each must also pass its own limit, so an outline nothing fits stays freehand.
    let candidates = [if found.len() <= 2 { fit_ellipse(&ring, first, ccw) } else { None }, fit_polygon(&ring, size, first, ccw)];
    candidates.into_iter().flatten().map(|s| (outline_error(&s, &ring, size), s)).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, s)| s)
}

fn recognize_open(pts: &[P], raw: &[P], size: f64) -> Option<Shape> {
    let (&first, &last) = (pts.first()?, pts.last()?);
    let chord = dist(first, last);
    // Straight line: small perpendicular spread, and it runs one way along its axis (no doubling
    // back): the chord spans nearly all of its extent along the axis.
    let m = mean(pts);
    let (_, minor, angle) = principal(pts, m);
    let u = [angle.cos(), angle.sin()];
    let along: Vec<f64> = pts.iter().map(|&p| dot(sub(p, m), u)).collect();
    let extent = along.iter().copied().fold(f64::MIN, f64::max) - along.iter().copied().fold(f64::MAX, f64::min);
    if extent > 1e-9 && minor.sqrt() <= LINE_TOL * extent && chord >= 0.85 * extent {
        let proj = |p: P| {
            let t = dot(sub(p, m), u);
            [m[0] + u[0] * t, m[1] + u[1] * t]
        };
        return Some(Shape::Line { a: proj(first), b: proj(last) });
    }
    // Sharp corners: straight edges (an L, a V, a Z), never a curve forced round them. A corner is
    // sharp when its turn stays as large over half the window; a smooth bend's turn halves with it.
    let k = corner_window(pts.len());
    let sharp = corners(pts, false).into_iter().any(|(i, a)| local_turn(pts, i, (k / 2).max(1), false) >= 0.7 * a);
    if sharp {
        return fit_polyline(pts, size);
    }
    // Smooth: a clean flowing curve, in as many pieces as its bends need (finer samples, so a
    // wavy line keeps every wave).
    fit_spline(&resample(raw, 128), (CURVE_TOL * size).max(CURVE_TOL_MIN))
}

fn fit_ellipse(ring: &[P], first: P, ccw: bool) -> Option<Shape> {
    let m = mean(ring);
    let (_, _, angle) = principal(ring, m);
    let q: Vec<P> = ring.iter().map(|&p| local(p, m, angle)).collect();
    let (lo, hi) = q.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let (rx, ry) = ((hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0);
    if rx <= 1e-6 || ry <= 1e-6 || ry / rx < 0.15 {
        return None;
    }
    // The centre of the extents, back in document space.
    let mid = [(hi[0] + lo[0]) / 2.0, (hi[1] + lo[1]) / 2.0];
    let (sin, cos) = angle.sin_cos();
    let c = [m[0] + mid[0] * cos - mid[1] * sin, m[1] + mid[0] * sin + mid[1] * cos];
    let err = q.iter().map(|p| (((p[0] - mid[0]) / rx).hypot((p[1] - mid[1]) / ry) - 1.0).abs()).sum::<f64>() / q.len().max(1) as f64;
    if err > ELLIPSE_TOL {
        return None;
    }
    // Start where the stroke started, so pressure maps onto the same part of the outline.
    let s = local(first, c, angle);
    let start = (s[1] / ry).atan2(s[0] / rx);
    let (rx, ry) = if ry / rx >= 0.82 {
        let r = (rx + ry) / 2.0;
        (r, r)
    } else {
        (rx, ry)
    };
    Some(Shape::Ellipse { c, rx, ry, angle, start, ccw })
}

fn fit_rect(ring: &[P], found: &[(usize, f64)], size: f64, first: P, ccw: bool) -> Option<Shape> {
    let cs: Vec<P> = found.iter().filter_map(|&(i, _)| ring.get(i).copied()).collect();
    if cs.len() != 4 {
        return None;
    }
    // Every corner near a right angle (75°–105°): a trapezoid or another four-sided form is a
    // quadrilateral, not a box, even when its outline stays close to one on average.
    let square_corners = (0..4).all(|k| {
        let (Some(before), Some(here), Some(after)) = (cs.get((k + 3) % 4), cs.get(k), cs.get((k + 1) % 4)) else { return false };
        let (a, b) = (sub(*before, *here), sub(*after, *here));
        let (la, lb) = (a[0].hypot(a[1]), b[0].hypot(b[1]));
        la > 1e-9 && lb > 1e-9 && (dot(a, b) / (la * lb)).abs() < 0.26
    });
    if !square_corners {
        return None;
    }
    // Orientation: edge angles averaged modulo 90° (4θ), weighted by edge length.
    let (mut sx, mut sy) = (0.0, 0.0);
    for k in 0..4 {
        let e = sub(cs[(k + 1) % 4], cs[k]);
        let (w, a) = (e[0].hypot(e[1]), e[1].atan2(e[0]));
        sx += w * (4.0 * a).cos();
        sy += w * (4.0 * a).sin();
    }
    let angle = sy.atan2(sx) / 4.0;
    let m = mean(&cs);
    let q: Vec<P> = cs.iter().map(|&p| local(p, m, angle)).collect();
    let mut xs: Vec<f64> = q.iter().map(|p| p[0]).collect();
    let mut ys: Vec<f64> = q.iter().map(|p| p[1]).collect();
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    // A first box from the corners (each found within a sample of the true corner)...
    let mut sides = [(xs[0] + xs[1]) / 2.0, (xs[2] + xs[3]) / 2.0, (ys[0] + ys[1]) / 2.0, (ys[2] + ys[3]) / 2.0];
    // ...then each side at the mean of the outline points nearest to it, which averages out
    // the hand's wobble along the whole edge.
    let mut sum = [0.0; 4];
    let mut count = [0usize; 4];
    for p in ring.iter().map(|&p| local(p, m, angle)) {
        let d = [(p[0] - sides[0]).abs(), (p[0] - sides[1]).abs(), (p[1] - sides[2]).abs(), (p[1] - sides[3]).abs()];
        let k = (0..4).min_by(|&a, &b| d[a].total_cmp(&d[b])).unwrap_or(0);
        sum[k] += if k < 2 { p[0] } else { p[1] };
        count[k] += 1;
    }
    for ((side, s), c) in sides.iter_mut().zip(sum).zip(count) {
        if c > 0 {
            *side = s / c as f64;
        }
    }
    let [x0, x1, y0, y1] = sides;
    let (mut hw, mut hh) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    if hw <= 1e-6 || hh <= 1e-6 {
        return None;
    }
    let mid = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    let (sin, cos) = angle.sin_cos();
    let c = [m[0] + mid[0] * cos - mid[1] * sin, m[1] + mid[0] * sin + mid[1] * cos];
    // Mean distance from the outline to the rectangle's edges.
    let err = ring
        .iter()
        .map(|&p| {
            let l = local(p, c, angle);
            let (dx, dy) = (l[0].abs() - hw, l[1].abs() - hh);
            if dx > 0.0 || dy > 0.0 { dx.max(0.0).hypot(dy.max(0.0)) } else { dx.max(dy).abs() }
        })
        .sum::<f64>()
        / ring.len().max(1) as f64;
    if err > RECT_TOL * size {
        return None;
    }
    if (hw - hh).abs() <= 0.12 * hw.max(hh) {
        let s = (hw + hh) / 2.0;
        (hw, hh) = (s, s);
    }
    let start = rect_corners(c, hw, hh, angle).iter().enumerate().min_by(|a, b| dist(*a.1, first).total_cmp(&dist(*b.1, first))).map_or(0, |(i, _)| i);
    Some(Shape::Rect { c, hw, hh, angle, start, ccw })
}

// ------------------------------------------------------------------------- the held stroke

/// Smart Shape's state for one Brush or Pencil drag (`Drag::smart`).
#[derive(Clone, Debug, PartialEq)]
pub struct Hold {
    /// Where the pointer came to rest and when (ms, `Stylus::clock_ms`).
    rest: P,
    rest_ms: f64,
    /// Recognition already ran for this rest (it runs once per pause).
    tried: bool,
    snapped: Option<Snapped>,
    /// The drag's points changed wholesale: the live preview must restart.
    rebuild: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Snapped {
    /// The shape as recognised, and as currently adjusted.
    base: Shape,
    current: Shape,
    /// The point the pointer drags: a line's or curve's end, else where the pointer rested.
    grab: P,
    /// The freehand stroke: its points (with pressure), pen poses and times.
    original: Vec<[f64; 3]>,
    poses: Vec<[f32; 3]>,
    times: Vec<f64>,
}

impl Hold {
    pub fn is_snapped(&self) -> bool {
        self.snapped.is_some()
    }
    /// The corrected shape, while there is one.
    pub fn shape(&self) -> Option<Shape> {
        self.snapped.as_ref().map(|s| s.current)
    }
}

/// Does Smart Shape apply to a press with `tool`? Brush and Pencil with the option on.
pub fn applies(app: &PhotocraftApp, tool: Tool) -> bool {
    matches!(tool, Tool::Brush | Tool::Pencil) && app.ui.tool_options.smart_shape
}

/// The state for a new drag at `at` (`None` when Smart Shape is off or the press continues a
/// ⇧-click line).
pub(crate) fn start(app: &PhotocraftApp, tool: Tool, connects: bool, at: P) -> Option<Hold> {
    (applies(app, tool) && !connects).then_some(Hold { rest: at, rest_ms: app.stylus.clock_ms, tried: false, snapped: None, rebuild: false })
}

/// Is `d` showing a corrected shape (its points are the shape's)?
pub fn is_shape(d: &Drag) -> bool {
    d.smart.as_ref().is_some_and(Hold::is_snapped)
}

/// A pointer move of the drag, at `p` (document point), at time `now` (ms), `zoom` screen points per
/// document pixel. True when the move went to the corrected shape (the caller must not add it to
/// the stroke).
pub(crate) fn pointer_moved(d: &mut Drag, p: P, now: f64, zoom: f32) -> bool {
    let constrained = d.constrain.is_some();
    let Some(h) = d.smart.as_mut() else { return false };
    let Some(s) = h.snapped.as_mut() else {
        // ⇧ makes a straight line already: Smart Shape stays out of it.
        if constrained {
            d.smart = None;
            return false;
        }
        if dist(p, h.rest) * f64::from(zoom.max(1e-6)) > STILL_RADIUS {
            (h.rest, h.rest_ms, h.tried) = (p, now, false);
        }
        return false;
    };
    let pivot = s.base.pivot();
    let (g, q) = (sub(s.grab, pivot), sub(p, pivot));
    let (lg, lq) = (g[0].hypot(g[1]), q[0].hypot(q[1]));
    let next = if lg < 1e-6 || lq < 1e-6 {
        s.base
    } else {
        let rot = (g[0] * q[1] - g[1] * q[0]).atan2(dot(g, q));
        // Circles turn freely: only their size follows.
        let rot = if matches!(s.base, Shape::Ellipse { rx, ry, .. } if (rx - ry).abs() <= 1e-9) { 0.0 } else { rot };
        s.base.transformed(pivot, (lq / lg).clamp(0.05, 20.0), rot)
    };
    if next != s.current {
        s.current = next;
        d.points = with_profile(&next.points(), &s.original);
        h.rebuild = true;
    }
    true
}

/// `outline` with the freehand stroke's pressure at the same fraction of its length.
fn with_profile(outline: &[P], original: &[[f64; 3]]) -> Vec<[f64; 3]> {
    let from: Vec<P> = original.iter().map(|q| [q[0], q[1]]).collect();
    let (fo, fs) = (arc_fractions(&from), arc_fractions(outline));
    outline.iter().zip(fs).map(|(p, f)| [p[0], p[1], sample_at(&fo, f, |i| original.get(i).map_or(1.0, |q| q[2]))]).collect()
}

/// The value at fraction `f` of a sequence whose points sit at fractions `fo` (linear between).
fn sample_at(fo: &[f64], f: f64, value: impl Fn(usize) -> f64) -> f64 {
    let i = fo.partition_point(|&x| x < f);
    if i == 0 {
        return value(0);
    }
    let (Some(&a), Some(&b)) = (fo.get(i - 1), fo.get(i)) else { return value(fo.len().saturating_sub(1)) };
    let t = if b - a > 1e-12 { (f - a) / (b - a) } else { 0.0 };
    value(i - 1) * (1.0 - t) + value(i) * t
}

/// The nearest index at fraction `f`.
fn index_at(fo: &[f64], f: f64) -> usize {
    fo.partition_point(|&x| x < f).min(fo.len().saturating_sub(1))
}

/// Run once a frame while dragging: once the pointer has rested for the hold time, recognise the
/// stroke and, when a shape fits, replace the drag's points with it. Returns when the UI must run
/// its next frame: when the hold is due, or `Duration::ZERO` right after the stroke became a shape
/// (a still pointer brings no input, so without it the shape would only show on release); `None`
/// when nothing is pending.
pub fn tick(app: &mut PhotocraftApp) -> Option<Duration> {
    let hold = f64::from(app.ui.tool_options.smart_shape_hold_ms.clamp(*HOLD_RANGE.start(), *HOLD_RANGE.end()));
    let (now, zoom) = (app.stylus.clock_ms, f64::from(app.point_zoom()));
    let d = app.drag.as_ref()?;
    let h = d.smart.as_ref()?;
    if h.snapped.is_some() || h.tried || !d.tool.is_brushlike() {
        return None;
    }
    let waited = now - h.rest_ms;
    if waited < hold {
        return Some(Duration::from_millis((hold - waited).ceil().max(1.0) as u64));
    }
    let raw: Vec<P> = d.points.iter().map(|q| [q[0], q[1]]).collect();
    let shape = recognize(&raw, zoom);
    let (poses, times, t0) = (app.stylus.stroke.clone(), app.stylus.times.clone(), app.stylus.point_ms(0));
    let d = app.drag.as_mut()?;
    let h = d.smart.as_mut()?;
    h.tried = true;
    let shape = shape?;
    let original = d.points.clone();
    let points = with_profile(&shape.points(), &original);
    // Pen tilt and rotation follow the same mapping; the shape has no timing of its own.
    let fo = arc_fractions(&raw);
    let fs = arc_fractions(&points.iter().map(|q| [q[0], q[1]]).collect::<Vec<_>>());
    let new_poses: Vec<[f32; 3]> = fs.iter().map(|&f| poses.get(index_at(&fo, f)).or(poses.last()).copied().unwrap_or_default()).collect();
    // Lines and curves are adjusted by their end, which then follows the pointer exactly; closed
    // shapes scale and turn with the pointer's offset from where it rested.
    let grab = shape.end().or_else(|| raw.last().copied()).unwrap_or(h.rest);
    h.snapped = Some(Snapped { base: shape, current: shape, grab, original, poses, times });
    h.rebuild = true;
    let n = points.len();
    d.points = points;
    app.stylus.stroke = new_poses;
    app.stylus.times = vec![t0; n];
    Some(Duration::ZERO)
}

/// Take the drag's "restart the live preview" flag.
pub(crate) fn take_rebuild(app: &mut PhotocraftApp) -> bool {
    app.drag.as_mut().and_then(|d| d.smart.as_mut()).is_some_and(|h| std::mem::take(&mut h.rebuild))
}

/// The release of drag `d` (already taken from the app) at `p`: a corrected shape takes the final
/// pointer position and stays as it is; the freehand stroke is never added under it. True when `d`
/// is a shape. A preview that no longer matches the shape is dropped, so the commit renders it.
pub(crate) fn release(app: &mut PhotocraftApp, d: &mut Drag, p: P) -> bool {
    if !is_shape(d) {
        return false;
    }
    let now = app.stylus.clock_ms;
    pointer_moved(d, p, now, app.point_zoom());
    if d.smart.as_mut().is_some_and(|h| std::mem::take(&mut h.rebuild)) {
        app.live_stroke = None;
    }
    true
}

/// Esc while a shape shows: back to the freehand stroke (the drag goes on; holding still again
/// does not re-snap until the pointer moves).
pub(crate) fn revert(app: &mut PhotocraftApp) -> bool {
    let Some(d) = app.drag.as_mut() else { return false };
    let Some(h) = d.smart.as_mut() else { return false };
    let Some(s) = h.snapped.take() else { return false };
    h.tried = true;
    h.rebuild = true;
    d.points = s.original;
    app.stylus.stroke = s.poses;
    app.stylus.times = s.times;
    true
}

/// The options bar toggle (Brush and Pencil; a circle and square with a spark): blue while on;
/// right-click sets the hold time.
pub fn button(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let on = app.ui.tool_options.smart_shape;
    let resp = crate::icons::button_with_icon_size(ui, "smart-shape", 24.0, 16.0, on, TOOLTIP);
    if resp.clicked() {
        app.ui.tool_options.smart_shape = !on;
    }
    resp.context_menu(|ui| {
        ui.label(tl!("Smart Shape hold time"));
        let hold = &mut app.ui.tool_options.smart_shape_hold_ms;
        ui.add(egui::Slider::new(hold, HOLD_RANGE).step_by(50.0).suffix(" ms"));
    });
}

#[cfg(test)]
#[path = "smart_shape_tests.rs"]
mod tests;
