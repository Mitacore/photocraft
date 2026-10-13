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
//!   also has its own limit;
//! - anything no geometric shape fits (long mixed paths, waves, zigzags, hairpins, narrow loops,
//!   spirals) gets freeform cleanup: cut at its sharp corners (found at two scales) and refitted
//!   with tangent-continuous cubic pieces within a small distance of the stroke, so it keeps its
//!   course, ends, peaks and corners and loses only the jitter. Marks too small on screen stay as
//!   drawn.
//!
//! Settings (right-click on the button; remembered with the other tool options): cleanup strength
//! (Light, Balanced, Strong: how much wobble freeform cleanup removes; corners stay at every
//! strength), the correction types (geometric shapes, freeform cleanup; each on its own, both on by
//! default; with both off holding still does nothing) and the hold time.

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

/// How strongly freeform cleanup irons out the hand's jitter (Smart Shape settings popup).
/// Corners and the ends stay where they were drawn at every strength.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strength {
    /// Closest to the stroke: only the finest tremor goes.
    Light,
    #[default]
    Balanced,
    /// Smoothest: small unintended wobbles go too.
    Strong,
}

impl Strength {
    pub const ALL: [Strength; 3] = [Strength::Light, Strength::Balanced, Strength::Strong];

    pub fn label(self) -> &'static str {
        match self {
            Strength::Light => "Light",
            Strength::Balanced => "Balanced",
            Strength::Strong => "Strong",
        }
    }

    /// The freeform tolerance relative to Balanced.
    fn factor(self) -> f64 {
        match self {
            Strength::Light => 0.55,
            Strength::Balanced => 1.0,
            Strength::Strong => 1.8,
        }
    }
}

/// What Smart Shape may do with a held stroke (its settings popup).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub strength: Strength,
    /// Lines, curves, polylines, circles, ellipses, triangles, quadrilaterals, rectangles.
    pub geometric: bool,
    /// Cleanup of strokes no geometric shape fits.
    pub freeform: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { strength: Strength::Balanced, geometric: true, freeform: true }
    }
}

impl Options {
    pub fn from_tool_options(o: &crate::state::ToolOptions) -> Self {
        Self { strength: o.smart_shape_strength, geometric: o.smart_shape_geometric, freeform: o.smart_shape_freeform }
    }

    /// Both correction types off: holding still changes nothing.
    pub fn none(&self) -> bool {
        !self.geometric && !self.freeform
    }
}

/// Points the stroke is resampled to for recognition.
const SAMPLES: usize = 64;
/// Normalised error limits (fractions of the stroke length or size).
const LINE_TOL: f64 = 0.03;
/// Line spread and curve error are also at most this many screen points, however long the stroke:
/// a long stroke with small waves is neither a line nor a curve through them.
const LINE_TOL_SCREEN: f64 = 8.0;
/// A polyline edge strays at most this far from the stroke (fraction of its size, and screen points).
const POLY_EDGE_DEV: f64 = 0.06;
const POLY_EDGE_DEV_SCREEN: f64 = 14.0;
const CURVE_TOL_SCREEN: f64 = 8.0;
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
/// Most cubic pieces a smoothed curve may have, and most vertices of a polyline (an L, V, Z, W or
/// M: up to six connected straight edges; closed polygons have three or four).
pub const MAX_SEGMENTS: usize = 8;
pub const MAX_POLY: usize = 7;
/// Freeform cleanup: largest distance of the cleaned path from the stroke, as a fraction of its
/// size (at least [`FREEFORM_TOL_MIN`] document pixels). Small enough to keep every intended bend,
/// large enough to iron out a hand's jitter.
const FREEFORM_TOL: f64 = 0.008;
const FREEFORM_TOL_MIN: f64 = 2.0;
/// The freeform tolerance is this many times the hand's typical jitter (see [`jitter`]), within
/// the limits above.
const FREEFORM_NOISE: f64 = 3.0;
/// ... and at most this many screen points, however long the stroke: a long stroke keeps the small
/// waves and bends along it (hand jitter is a few screen points at any zoom).
const FREEFORM_TOL_SCREEN: f64 = 6.0;
/// Freeform sampling step and corner window, at most this many screen points (same reason).
const FINE_STEP_SCREEN: f64 = 3.0;
const FINE_WINDOW_SCREEN: f64 = 24.0;
/// A freeform corner turns at least this much (radians) over its window, about as much over half
/// and over twice the window (see [`sharp_corners`]).
const FREEFORM_CORNER: f64 = 0.8; // 46°
/// Freeform splitting depth: never reached by balanced splits of at most [`FINE_MAX`] samples
/// (each keeps at least a quarter, so about 20 levels), only a guard for the recursion.
const FREEFORM_DEPTH: u32 = 32;
/// Most samples freeform cleanup works on (a stroke longer than that many steps is sampled a little
/// more coarsely), which bounds its work at the hold.
const FINE_MAX: usize = 2000;

type P = [f64; 2];

/// A corrected shape, in document coordinates.
#[derive(Clone, Debug, PartialEq)]
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
    /// Freeform cleanup of a stroke no geometric shape fits: cubic Béziers following it, as many as
    /// its course needs, smooth within each run and meeting at its sharp corners (peaks, zigzag
    /// tips). Ends where it began when `closed`.
    Freeform {
        seg: Vec<[P; 4]>,
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
            Shape::Freeform { .. } => "freeform",
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
            Shape::Freeform { seg, closed: false } => seg.first().map_or([0.0; 2], |s| s[0]),
            Shape::Freeform { seg, closed: true } => mean(&seg.iter().map(|s| s[0]).collect::<Vec<_>>()),
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
            Shape::Freeform { seg, closed: false } => seg.last().map(|s| s[3]),
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
            Shape::Freeform { ref seg, closed } => Shape::Freeform { seg: seg.iter().map(|s| s.map(map)).collect(), closed },
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
            Shape::Spline { seg, n } => pieces_points(seg.get(..n).unwrap_or_default()),
            Shape::Freeform { ref seg, .. } => pieces_points(seg),
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

/// Cubic pieces as a polyline about [`SPACING`] apart; each piece starts where the last ended, so
/// that point is kept once.
fn pieces_points(pieces: &[[P; 4]]) -> Vec<P> {
    let mut out = Vec::new();
    for p in pieces {
        let rough: f64 = (0..16).map(|i| dist(bezier(p, i as f64 / 16.0), bezier(p, (i + 1) as f64 / 16.0))).sum();
        let m = ((rough / SPACING).ceil() as usize).clamp(4, 4000);
        let skip = usize::from(!out.is_empty());
        out.extend((skip..m).map(|i| bezier(p, i as f64 / (m - 1) as f64)));
    }
    out
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
    let reach = path_len(pts);
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
        // A negative or vanishing handle would fold the curve back, and one longer than the piece
        // would throw it out in a loop: use the plain third instead.
        if !(l1 > 1e-6 && l2 > 1e-6 && l1 <= reach && l2 <= reach) {
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

/// How finely [`fit_pieces`] may split: the error each piece aims for, how many times it may halve
/// (2^`max_depth` pieces at most), the fewest samples a piece that may still split spans (at least
/// 8), and whether each split keeps at least a quarter of the piece on either side.
#[derive(Clone, Copy)]
struct Fit {
    tol: f64,
    max_depth: u32,
    min_span: usize,
    balanced: bool,
}

/// Fit `pts[lo..=hi]` with tangent-continuous cubic pieces, splitting at the worst point until
/// each piece is within `fit.tol`. Returns the largest error left.
#[allow(clippy::too_many_arguments)]
fn fit_pieces(pts: &[P], lo: usize, hi: usize, t1: P, t2: P, fit: Fit, depth: u32, out: &mut Vec<[P; 4]>) -> f64 {
    let Some(part) = pts.get(lo..=hi) else { return f64::MAX };
    let Some((ctrl, err, at)) = fit_tangent(part, t1, t2) else { return f64::MAX };
    if err <= fit.tol || hi - lo < fit.min_span.max(8) || depth >= fit.max_depth {
        out.push(ctrl);
        return err;
    }
    let edge = if fit.balanced { ((hi - lo) / 4).max(4) } else { 4 };
    let mid = (lo + at).clamp(lo + edge, hi - edge);
    let tc = match (pts.get(mid - 1), pts.get(mid + 1)) {
        (Some(&a), Some(&b)) => unit(sub(b, a)).unwrap_or(t1),
        _ => t1,
    };
    let left = fit_pieces(pts, lo, mid, t1, [-tc[0], -tc[1]], fit, depth + 1, out);
    let right = fit_pieces(pts, mid, hi, tc, t2, fit, depth + 1, out);
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
    let err = fit_pieces(pts, 0, n - 1, t1, t2, Fit { tol, max_depth: 3, min_span: 12, balanced: false }, 0, &mut pieces);
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

/// An open stroke with sharp corners as straight edges (an L, a V, a Z, a W): two to six edges,
/// each really straight: no point of the stroke further off its edge than a wobble, so a stroke
/// mixing corners with curved runs keeps its curves (freeform cleanup) instead.
fn fit_polyline(pts: &[P], size: f64, zoom: f64) -> Option<Shape> {
    let last = pts.len().checked_sub(1)?;
    let k = corner_window(pts.len());
    let mut idx = rdp(pts, 0.05 * size);
    idx.retain(|&i| i == 0 || i == last || local_turn(pts, i, k, false) >= VERTEX_TURN);
    let m = idx.len();
    if !(3..=MAX_POLY).contains(&m) {
        return None;
    }
    let verts = refine_vertices(pts, &idx, false, size);
    // A corner is where the stroke went: one far off it cuts across a round turn (a U-turn
    // squared off into a pointed V).
    if verts.iter().any(|&q| poly_dist(q, pts) > POLY_TOL * size) {
        return None;
    }
    // Nor does a polyline fold back on itself: an edge doubling back over the last (a scribble going
    // to and fro) is no corner.
    let folds = verts.windows(3).any(|t| unit(sub(t[1], t[0])).zip(unit(sub(t[2], t[1]))).is_some_and(|(u, v)| dot(u, v) < -0.94));
    if folds {
        return None;
    }
    let mut v = [[0.0; 2]; MAX_POLY];
    for (slot, q) in v.iter_mut().zip(&verts) {
        *slot = *q;
    }
    let shape = Shape::Poly { v, n: m, closed: false };
    let outline = shape.points();
    let straight = pts.iter().all(|&p| poly_dist(p, &outline) <= (POLY_EDGE_DEV * size).min(POLY_EDGE_DEV_SCREEN / zoom.max(1e-6)));
    (straight && outline_error(&shape, pts, size) <= POLY_TOL).then_some(shape)
}

/// Sharp corners of `pts` for freeform cleanup: where the turn over `k` samples is at least
/// [`FREEFORM_CORNER`], at least 70% of it over half the window and at least 70% of the turn over
/// twice the window, and the largest within the window. A smooth bend's turn grows with the
/// window and a real corner's does not; the narrow window alone mistakes jitter on a tight round
/// turn for corners, the wide one alone a round wave crest, so a corner must pass both. `closed`
/// wraps around.
fn sharp_corners(pts: &[P], k: usize, closed: bool) -> Vec<usize> {
    let n = pts.len();
    if n < 2 * k + 1 {
        return Vec::new();
    }
    let range: Vec<usize> = if closed { (0..n).collect() } else { (k..n - k).collect() };
    let turns: Vec<(usize, f64)> = range.into_iter().map(|i| (i, local_turn(pts, i, k, closed))).collect();
    // The turn over half the window, at the sample or a neighbour (one jittery sample must not hide
    // a corner).
    let half = |i: usize| (i.saturating_sub(1)..=i + 1).map(|j| local_turn(pts, j, (k / 2).max(1), closed)).fold(0.0, f64::max);
    let near = |i: usize, j: usize| {
        let d = i.abs_diff(j);
        if closed { d.min(n - d) <= k } else { d <= k }
    };
    turns
        .iter()
        .filter(|&&(i, a)| a >= FREEFORM_CORNER && half(i) >= 0.7 * a)
        .filter(|&&(i, a)| a >= 0.7 * local_turn(pts, i, if closed { 2 * k } else { (2 * k).min(i).min(n - 1 - i) }, closed))
        .filter(|&&(i, a)| !turns.iter().any(|&(j, b)| j != i && near(i, j) && (b > a || (b == a && j < i))))
        .map(|&(i, _)| i)
        .collect()
}

/// The stroke in even samples about every 0.4% of its size (at least 1 px, at most
/// [`FINE_STEP_SCREEN`] screen points; 48 to [`FINE_MAX`] of them): fine enough for every bend, with
/// the corner window for them (1.5% of the size, at least 8 px, at most [`FINE_WINDOW_SCREEN`] screen
/// points). `zoom`: screen points per document pixel.
fn fine_samples(raw: &[P], size: f64, zoom: f64) -> Option<(Vec<P>, usize)> {
    let zoom = zoom.max(1e-6);
    let len = path_len(raw);
    if len <= 1e-9 || size <= 1e-9 {
        return None;
    }
    let n = ((len / (size * 0.004).max(1.0).min(FINE_STEP_SCREEN / zoom)).ceil() as usize).clamp(48, FINE_MAX);
    let pts = resample(raw, n);
    let step = len / (n - 1) as f64;
    let k = (((size * 0.015).max(8.0).min(FINE_WINDOW_SCREEN / zoom) / step).round() as usize).clamp(2, (n / 8).max(2));
    Some((pts, k))
}

/// Freeform cleanup for a stroke no geometric shape fits (long mixed paths, waves, zigzags,
/// hairpins, narrow loops): the same path with the hand's jitter ironed out. It is cut at its sharp
/// corners (found at two scales, so peaks and zigzag tips stay sharp while smooth bends stay
/// round) and each run between them becomes tangent-continuous cubic pieces within a small
/// distance of the stroke (Schneider's fitting), so bends, peaks and valleys keep their place and
/// height. The ends stay where they were drawn; an open stroke stays open. `None` when even many
/// pieces can't follow it closely.
fn freeform(raw: &[P], size: f64, zoom: f64, strength: Strength, closed: bool) -> Option<Shape> {
    let (mut pts, k) = fine_samples(raw, size, zoom)?;
    let n = pts.len();
    let mut smooth_closure = false;
    if closed {
        // Close the loop exactly, and start it at a corner when it has one, so the closing point
        // is a corner too; without any, the closing point gets one tangent on both sides.
        if let Some(&f) = pts.first()
            && let Some(l) = pts.last_mut()
        {
            *l = f;
        }
        let ring = pts.get(..n - 1).unwrap_or_default();
        match sharp_corners(ring, k, true).first() {
            Some(&c) => {
                let mut turned: Vec<P> = ring.iter().skip(c).chain(ring.iter().take(c)).copied().collect();
                turned.push(*turned.first()?);
                pts = turned;
            }
            None => smooth_closure = true,
        }
    }
    let n = pts.len();
    let mut breaks = if closed { sharp_corners(pts.get(..n - 1).unwrap_or_default(), k, true) } else { sharp_corners(&pts, k, false) };
    breaks.push(0);
    breaks.push(n - 1);
    breaks.sort_unstable();
    breaks.dedup();
    // Direction leaving sample `i` towards sample `j`.
    let dir = |i: usize, j: usize| -> Option<P> { unit(sub(*pts.get(j)?, *pts.get(i)?)) };
    let closure = if smooth_closure { dir(n - 4, 3) } else { None };
    // The tolerance follows the hand's own jitter (a few times its typical size), so detail larger
    // than the jitter stays, however long the stroke; bounded by a share of the stroke's size and
    // a few screen points, then scaled by the cleanup strength.
    let cap = (size * FREEFORM_TOL).max(FREEFORM_TOL_MIN).min(FREEFORM_TOL_SCREEN / zoom.max(1e-6));
    let base = (FREEFORM_NOISE * jitter(raw)).clamp(FREEFORM_TOL_MIN.min(cap), cap) * strength.factor();
    // As many pieces as the course needs (a piece spans at least 8 samples, so at most n / 8),
    // splitting near the worst point but never at the very ends, so a long run with many bends is
    // halved down to its bends instead of running out of splits. A stroke too rough for the
    // tolerance is retried a little looser rather than left raw.
    let fit_at = |tol: f64| -> Option<Shape> {
        let fit = Fit { tol, max_depth: FREEFORM_DEPTH, min_span: 8, balanced: true };
        let mut pieces: Vec<[P; 4]> = Vec::new();
        let mut worst: f64 = 0.0;
        for w in breaks.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (Some(&pa), Some(&pb)) = (pts.get(a), pts.get(b)) else { return None };
            if b - a < 4 {
                pieces.push([pa, lerp(pa, pb, 1.0 / 3.0), lerp(pa, pb, 2.0 / 3.0), pb]);
                continue;
            }
            // Leaving an end of the run: towards the mean of its next few samples (at most a
            // third of the run), so one jittery sample can't set off a hook.
            let w = 4.min((b - a) / 3).max(1);
            let ahead = |from: usize, step: isize| -> Option<P> {
                let next: Vec<P> = (1..=w as isize).filter_map(|j| from.checked_add_signed(step * j).and_then(|i| pts.get(i)).copied()).collect();
                unit(sub(mean(&next), *pts.get(from)?))
            };
            let t1 = match closure.filter(|_| a == 0) {
                Some(t) => t,
                None => ahead(a, 1).or_else(|| dir(a, a + 3))?,
            };
            let t2 = match closure.filter(|_| b == n - 1) {
                Some(t) => [-t[0], -t[1]],
                None => ahead(b, -1).or_else(|| dir(b, b - 3))?,
            };
            worst = worst.max(fit_pieces(&pts, a, b, t1, t2, fit, 0, &mut pieces));
        }
        // The fit is measured at the samples only; between them a piece must not bulge away either.
        let strays = || pieces.iter().flat_map(|p| (1..16).map(|i| bezier(p, i as f64 / 16.0))).any(|q| poly_dist(q, &pts) > 2.0 * tol);
        if worst <= 2.0 * tol && !strays() { Some(Shape::Freeform { seg: pieces, closed }) } else { None }
    };
    [1.0, 1.6, 2.6].into_iter().find_map(|m| fit_at(base * m))
}

/// The typical size of the hand's jitter in `raw` (document pixels): the median distance of each
/// drawn point from the chord of its two neighbours. Smooth bends hardly move it (their offset
/// over one sample is tiny) and the median ignores the few corners.
fn jitter(raw: &[P]) -> f64 {
    let mut d: Vec<f64> = raw.windows(3).filter(|w| dist(w[0], w[2]) > 1e-9).map(|w| seg_dist(w[1], w[0], w[2])).collect();
    if d.is_empty() {
        return 0.0;
    }
    let mid = d.len() / 2;
    let (_, m, _) = d.select_nth_unstable_by(mid, f64::total_cmp);
    *m
}

/// Distance from `p` to the polyline `poly`.
fn poly_dist(p: P, poly: &[P]) -> f64 {
    poly.windows(2).map(|w| seg_dist(p, w[0], w[1])).fold(f64::MAX, f64::min)
}

/// Signed area (shoelace): negative when the outline runs counter-clockwise on screen (y down).
fn signed_area(pts: &[P]) -> f64 {
    pts.windows(2).map(|w| w[0][0] * w[1][1] - w[1][0] * w[0][1]).sum::<f64>() / 2.0
}

/// Does the stroke `pts` (ends already close together) close a loop, rather than double back
/// beside its start like a narrow U-turn? It does when its last quarter passes close by the start
/// (an overshooting circle), or its end heads towards the start (a loop left slightly open).
fn closes_on_itself(pts: &[P], size: f64) -> bool {
    let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else { return false };
    let n = pts.len();
    let passes = pts.iter().skip(n * 3 / 4).any(|&p| dist(p, first) <= 0.05 * size);
    let k = corner_window(n);
    let heads = match (pts.get(n.saturating_sub(1 + k)).and_then(|&p| unit(sub(last, p))), unit(sub(first, last))) {
        (Some(end), Some(to_start)) => dot(end, to_start) > 0.5,
        // The end is on the start: closed.
        (_, None) => true,
        _ => false,
    };
    passes || heads
}

/// Recognise the stroke `raw` (document points) drawn at `zoom` (screen points per document
/// pixel). `None`: no shape fits well enough, keep the freehand stroke.
pub fn recognize(raw: &[P], zoom: f64) -> Option<Shape> {
    recognize_with(raw, zoom, Options::default())
}

/// [`recognize`] with the Smart Shape settings: a confident geometric match first (when geometric
/// shapes are on), else the freeform cleanup (when it is on) at the chosen strength.
pub fn recognize_with(raw: &[P], zoom: f64, opts: Options) -> Option<Shape> {
    let raw: Vec<P> = raw.iter().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
    let pts = resample(&raw, SAMPLES);
    let (&first, &last) = (pts.first()?, pts.last()?);
    let (min, max) = pts.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let size = dist(min, max);
    let len = path_len(&pts);
    if opts.none() || size * zoom.max(1e-6) < MIN_EXTENT || len <= 1e-9 {
        return None;
    }
    let cleanup = |closed: bool| if opts.freeform { freeform(&raw, size, zoom, opts.strength, closed) } else { None };
    let closed = dist(first, last) <= CLOSED_GAP * len && closes_on_itself(&pts, size);
    if !closed {
        // No geometric shape fits: the freeform cleanup of the same path.
        let shape = if opts.geometric { recognize_open(&pts, &raw, size, zoom) } else { None };
        return shape.or_else(|| cleanup(false));
    }
    if !opts.geometric {
        return cleanup(true);
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
    let best = candidates.into_iter().flatten().map(|s| (outline_error(&s, &ring, size), s)).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, s)| s);
    // No geometric shape fits (a narrow loop, a long irregular outline): the freeform cleanup.
    best.or_else(|| cleanup(true))
}

fn recognize_open(pts: &[P], raw: &[P], size: f64, zoom: f64) -> Option<Shape> {
    let (&first, &last) = (pts.first()?, pts.last()?);
    let chord = dist(first, last);
    // Straight line: small perpendicular spread, and it runs one way along its axis (no doubling
    // back): the chord spans nearly all of its extent along the axis.
    let m = mean(pts);
    let (_, minor, angle) = principal(pts, m);
    let u = [angle.cos(), angle.sin()];
    let along: Vec<f64> = pts.iter().map(|&p| dot(sub(p, m), u)).collect();
    let extent = along.iter().copied().fold(f64::MIN, f64::max) - along.iter().copied().fold(f64::MAX, f64::min);
    if extent > 1e-9 && minor.sqrt() <= (LINE_TOL * extent).min(LINE_TOL_SCREEN / zoom.max(1e-6)) && chord >= 0.85 * extent {
        let proj = |p: P| {
            let t = dot(sub(p, m), u);
            [m[0] + u[0] * t, m[1] + u[1] * t]
        };
        return Some(Shape::Line { a: proj(first), b: proj(last) });
    }
    // Sharp corners: straight edges (an L, a V, a Z), never a curve forced round them. A corner is
    // sharp when its turn stays as large over half the window, on fine samples (a smooth bend's
    // turn halves with the window, so a tight U-turn stays a round U-turn).
    let sharp = fine_samples(raw, size, zoom).is_some_and(|(fine, k)| !sharp_corners(&fine, k, false).is_empty());
    if sharp {
        return fit_polyline(pts, size, zoom);
    }
    // Smooth: a clean flowing curve, in as many pieces as its bends need (finer samples, so a
    // wavy line keeps every wave).
    fit_spline(&resample(raw, 128), (CURVE_TOL * size).max(CURVE_TOL_MIN).min(CURVE_TOL_SCREEN / zoom.max(1e-6)))
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
        self.snapped.as_ref().map(|s| s.current.clone())
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
        s.base.clone()
    } else {
        let rot = (g[0] * q[1] - g[1] * q[0]).atan2(dot(g, q));
        // Circles turn freely: only their size follows.
        let rot = if matches!(s.base, Shape::Ellipse { rx, ry, .. } if (rx - ry).abs() <= 1e-9) { 0.0 } else { rot };
        s.base.transformed(pivot, (lq / lg).clamp(0.05, 20.0), rot)
    };
    if next != s.current {
        d.points = with_profile(&next.points(), &s.original);
        s.current = next;
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
    // Both correction types off: no timer, the stroke stays as drawn.
    if Options::from_tool_options(&app.ui.tool_options).none() {
        return None;
    }
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
    let shape = recognize_with(&raw, zoom, Options::from_tool_options(&app.ui.tool_options));
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
    h.snapped = Some(Snapped { base: shape.clone(), current: shape, grab, original, poses, times });
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
    resp.context_menu(|ui| settings(app, ui));
}

/// The settings popup (right-click on the button): cleanup strength, correction types and the hold
/// time. Remembered between launches with the other tool options (`tool_memory`).
fn settings(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let o = &mut app.ui.tool_options;
    ui.strong(tl!("Cleanup Strength"));
    ui.add_enabled_ui(o.smart_shape_freeform, |ui| {
        ui.horizontal(|ui| {
            for s in Strength::ALL {
                ui.selectable_value(&mut o.smart_shape_strength, s, tl!(s.label()));
            }
        });
    });
    ui.separator();
    ui.strong(tl!("Correction Types"));
    ui.checkbox(&mut o.smart_shape_geometric, tl!("Geometric Shapes"));
    ui.checkbox(&mut o.smart_shape_freeform, tl!("Freeform Cleanup"));
    if !o.smart_shape_geometric && !o.smart_shape_freeform {
        ui.weak(tl!("Both off: holding still leaves strokes exactly as drawn."));
    }
    ui.separator();
    ui.label(tl!("Smart Shape hold time"));
    ui.add(egui::Slider::new(&mut o.smart_shape_hold_ms, HOLD_RANGE).step_by(50.0).suffix(" ms"));
}

#[cfg(test)]
#[path = "smart_shape_tests.rs"]
mod tests;
