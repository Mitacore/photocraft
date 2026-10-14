use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::canvas::{ToolEvent, tool_event};

// ------------------------------------------------------------------------------ synthetic input

/// Deterministic wobble in -1..1, like a hand's.
fn wobble(i: usize) -> f64 {
    ((i as f64 * 0.9).sin() + (i as f64 * 2.3).cos() * 0.5) / 1.5
}

fn line(a: P, b: P, n: usize, amp: f64) -> Vec<P> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx.hypot(dy);
    let nrm = [-dy / l, dx / l];
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let w = amp * wobble(i);
            [a[0] + dx * t + nrm[0] * w, a[1] + dy * t + nrm[1] * w]
        })
        .collect()
}

/// An ellipse (circle when rx == ry) turned by `angle`, over `turns` turns from angle `from`.
fn ellipse(c: P, rx: f64, ry: f64, angle: f64, turns: f64, n: usize, amp: f64) -> Vec<P> {
    let (sin, cos) = angle.sin_cos();
    (0..n)
        .map(|i| {
            let t = 0.3 + std::f64::consts::TAU * turns * i as f64 / (n - 1) as f64;
            let k = 1.0 + amp * wobble(i) / rx.min(ry);
            let (x, y) = (rx * k * t.cos(), ry * k * t.sin());
            [c[0] + x * cos - y * sin, c[1] + x * sin + y * cos]
        })
        .collect()
}

/// A rectangle outline turned by `angle`, starting mid-way along its top edge.
fn rect(c: P, hw: f64, hh: f64, angle: f64, per_edge: usize, amp: f64) -> Vec<P> {
    let cs = rect_corners(c, hw, hh, angle);
    let mut out = Vec::new();
    let start = lerp(cs[0], cs[1], 0.5);
    let path = [start, cs[1], cs[2], cs[3], cs[0], start];
    let mut i = 0;
    for w in path.windows(2) {
        for s in 0..per_edge {
            let p = lerp(w[0], w[1], s as f64 / per_edge as f64);
            let d = sub(w[1], w[0]);
            let l = d[0].hypot(d[1]).max(1e-9);
            let k = amp * wobble(i);
            out.push([p[0] - d[1] / l * k, p[1] + d[0] / l * k]);
            i += 1;
        }
    }
    out.push(start);
    out
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

// ---------------------------------------------------------------------------------- recognition

#[test]
fn recognises_a_straight_line() {
    let s = recognize(&line([20.0, 30.0], [220.0, 80.0], 60, 2.0), 1.0).expect("a line");
    let Shape::Line { a, b } = s else { panic!("{s:?}") };
    assert_eq!(s.kind(), "line");
    assert!(dist(a, [20.0, 30.0]) < 4.0 && dist(b, [220.0, 80.0]) < 4.0, "{a:?} {b:?}");
}

#[test]
fn recognises_a_smooth_curve() {
    // A 100° arc of radius 100.
    let pts: Vec<P> = (0..50)
        .map(|i| {
            let t = (20.0 + 100.0 * i as f64 / 49.0).to_radians();
            let r = 100.0 + wobble(i);
            [150.0 + r * t.cos(), 150.0 + r * t.sin()]
        })
        .collect();
    let s = recognize(&pts, 1.0).expect("a curve");
    assert_eq!(s.kind(), "curve");
    let out = s.points();
    // The curve stays on the arc and keeps its ends.
    assert!(out.iter().all(|p| near(dist(*p, [150.0, 150.0]), 100.0, 4.0)), "off the arc");
    assert!(dist(out[0], pts[0]) < 2.0 && dist(*out.last().unwrap(), *pts.last().unwrap()) < 2.0);
}

#[test]
fn recognises_circles_and_ellipses() {
    let s = recognize(&ellipse([150.0, 150.0], 80.0, 80.0, 0.0, 1.05, 90, 2.0), 1.0).expect("a circle");
    let Shape::Ellipse { c, rx, ry, .. } = s else { panic!("{s:?}") };
    assert_eq!(s.kind(), "circle");
    assert!(dist(c, [150.0, 150.0]) < 4.0 && near(rx, 80.0, 5.0) && rx == ry, "{c:?} {rx} {ry}");

    let angle = 30f64.to_radians();
    let s = recognize(&ellipse([200.0, 160.0], 120.0, 50.0, angle, 1.0, 120, 1.5), 1.0).expect("an ellipse");
    let Shape::Ellipse { c, rx, ry, angle: a, .. } = s else { panic!("{s:?}") };
    assert_eq!(s.kind(), "ellipse");
    assert!(dist(c, [200.0, 160.0]) < 5.0 && near(rx, 120.0, 8.0) && near(ry, 50.0, 6.0), "{c:?} {rx} {ry}");
    // The major axis lies along 30° (either direction).
    let d = (a - angle).rem_euclid(std::f64::consts::PI);
    assert!(d < 0.1 || d > std::f64::consts::PI - 0.1, "angle {a}");
}

#[test]
fn recognises_rectangles_and_squares() {
    let angle = 10f64.to_radians();
    let s = recognize(&rect([150.0, 120.0], 100.0, 50.0, angle, 20, 1.5), 1.0).expect("a rectangle");
    let Shape::Rect { c, hw, hh, angle: a, .. } = s else { panic!("{s:?}") };
    assert_eq!(s.kind(), "rectangle");
    let (w, h) = if hw > hh { (hw, hh) } else { (hh, hw) };
    assert!(dist(c, [150.0, 120.0]) < 4.0 && near(w, 100.0, 6.0) && near(h, 50.0, 5.0), "{c:?} {hw} {hh}");
    let d = (a - angle).rem_euclid(std::f64::consts::FRAC_PI_2);
    assert!(d < 0.06 || d > std::f64::consts::FRAC_PI_2 - 0.06, "angle {a}");
    // The outline is closed and has the exact corners.
    let out = s.points();
    assert_eq!(out.first(), out.last());
    for corner in rect_corners(c, hw, hh, a) {
        assert!(out.iter().any(|p| dist(*p, corner) < 1e-6), "corner {corner:?}");
    }

    let s = recognize(&rect([120.0, 120.0], 60.0, 60.0, (-20f64).to_radians(), 16, 1.5), 1.0).expect("a square");
    assert_eq!(s.kind(), "square");
}

/// Strokes no geometric shape fits are cleaned along their own course, never forced into a
/// circle, polygon or line: the ends stay put, the course stays within a few pixels, corners stay.
#[test]
fn strokes_no_shape_fits_are_cleaned_not_forced() {
    // A zig-zag of four teeth.
    let tooth = |i: usize| if (i / 5).is_multiple_of(2) { (i % 5) as f64 * 12.0 } else { (5 - i % 5) as f64 * 12.0 };
    let zigzag: Vec<P> = (0..41).map(|i| [20.0 + i as f64 * 5.0, 100.0 + tooth(i)]).collect();
    let s = recognize(&zigzag, 1.0).expect("cleaned");
    assert_eq!(s.kind(), "freeform", "zig-zag: {s:?}");
    assert_follows(&s, &zigzag, 4.0, 4.0);
    for i in [5usize, 15, 25, 35] {
        let tip = [20.0 + i as f64 * 5.0, 160.0];
        assert!(s.points().iter().any(|p| dist(*p, tip) < 4.0), "tooth tip {tip:?} kept");
    }
    // A spiral (two turns, growing), open.
    let spiral: Vec<P> = (0..120)
        .map(|i| {
            let t = std::f64::consts::TAU * 2.0 * i as f64 / 119.0;
            let r = 20.0 + 40.0 * i as f64 / 119.0;
            [150.0 + r * t.cos(), 150.0 + r * t.sin()]
        })
        .collect();
    let s = recognize(&spiral, 1.0).expect("cleaned");
    assert!(matches!(s.kind(), "curve" | "freeform"), "spiral: {s:?}");
    assert_follows(&s, &spiral, 4.0, 4.0);
    // A scribble that goes back over itself.
    let back: Vec<P> = (0..40).map(|i| [50.0 + (i as f64 * 0.5).sin() * 60.0, 50.0 + i as f64 * 0.3]).collect();
    let s = recognize(&back, 1.0).expect("cleaned");
    assert!(matches!(s.kind(), "curve" | "freeform"), "back and forth: {s:?}");
    assert_follows(&s, &back, 5.0, 5.0);
    // Degenerate input never panics.
    assert_eq!(recognize(&[], 1.0), None);
    assert_eq!(recognize(&[[5.0, 5.0]], 1.0), None);
    assert_eq!(recognize(&[[5.0, 5.0]; 30], 1.0), None);
    assert_eq!(recognize(&[[f64::NAN, 1.0], [f64::INFINITY, 2.0]], 1.0), None);
}

/// The cleaned `shape` stays within `dev` of the stroke `drawn` everywhere, and leaves no part of
/// `drawn` further than `cover` away (nothing cut off or flattened).
fn assert_follows(shape: &Shape, drawn: &[P], dev: f64, cover: f64) {
    let out = shape.points();
    let worst = |of: &[P], to: &[P]| of.iter().map(|&p| (poly_dist(p, to), p)).fold((0.0, [0.0; 2]), |m, d| if d.0 > m.0 { d } else { m });
    let ((out_d, out_at), (in_d, in_at)) = (worst(&out, drawn), worst(drawn, &out));
    assert!(out_d <= dev, "{}: strays {out_d:.1} px from the stroke at {out_at:?}", shape.kind());
    assert!(in_d <= cover, "{}: leaves the stroke {in_d:.1} px behind at {in_at:?}", shape.kind());
}

/// Deterministic jitter in -0.5..0.5.
fn jitter(i: usize) -> f64 {
    ((i as f64 * 12.9898).sin() * 43758.5453).rem_euclid(1.0) - 0.5
}

/// `clean` as a hand draws it: sampled about every `step` px, each sample off by up to `amp` px.
fn noisy(clean: &[P], step: f64, amp: f64) -> Vec<P> {
    let n = ((path_len(clean) / step) as usize).max(2);
    resample(clean, n).iter().enumerate().map(|(i, p)| [p[0] + 2.0 * amp * jitter(i), p[1] + 2.0 * amp * jitter(i + 7919)]).collect()
}

/// Total turning (radians) of `pts` resampled to `n` points: jitter adds to it, smoothing removes it.
fn turning(pts: &[P], n: usize) -> f64 {
    resample(pts, n)
        .windows(3)
        .map(|w| {
            let (u, v) = (sub(w[1], w[0]), sub(w[2], w[1]));
            (u[0] * v[1] - u[1] * v[0]).atan2(dot(u, v)).abs()
        })
        .sum()
}

/// A dense polyline through `v` (about 1 px apart).
fn dense(v: &[P]) -> Vec<P> {
    let mut out = vec![v[0]];
    for w in v.windows(2) {
        let n = (dist(w[0], w[1]).ceil() as usize).max(1);
        out.extend((1..=n).map(|i| lerp(w[0], w[1], i as f64 / n as f64)));
    }
    out
}

/// The long mixed stroke of the hands-on sketch: three pointed peaks with round valleys, two
/// smooth waves, a hairpin into a long straight run back, a second hairpin, a straight run and a
/// zigzag tail. Returns it (about 1 px apart) and the points that must stay sharp.
fn mixed_stroke() -> (Vec<P>, Vec<P>) {
    let mut path = dense(&[[100.0, 400.0], [250.0, 400.0]]);
    // Peaks: height (1 - |u|)^1.5, pointed at u = 0, flat at u = ±1.
    for hump in 0..3 {
        let x0 = 250.0 + 160.0 * hump as f64;
        path.extend((1..=160).map(|i| {
            let u = (i as f64 - 80.0) / 80.0;
            [x0 + i as f64, 400.0 - 220.0 * (1.0 - u.abs()).powf(1.5)]
        }));
    }
    // Two smooth waves.
    path.extend((1..=320).map(|i| [730.0 + i as f64, 400.0 + 40.0 * (std::f64::consts::TAU * i as f64 / 160.0).sin()]));
    // Hairpin (radius 30) and the long run back.
    path.extend((1..=94).map(|i| {
        let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 94.0;
        [1050.0 + 30.0 * a.cos(), 430.0 + 30.0 * a.sin()]
    }));
    path.extend(dense(&[[1050.0, 460.0], [300.0, 460.0]]).into_iter().skip(1));
    // Second hairpin (radius 25), a straight run and a zigzag tail.
    path.extend((1..=78).map(|i| {
        let a = -std::f64::consts::FRAC_PI_2 - std::f64::consts::PI * i as f64 / 78.0;
        [300.0 + 25.0 * a.cos(), 485.0 + 25.0 * a.sin()]
    }));
    path.extend(dense(&[[300.0, 510.0], [900.0, 510.0], [950.0, 570.0], [1000.0, 510.0], [1050.0, 570.0]]).into_iter().skip(1));
    let sharp = vec![[330.0, 180.0], [490.0, 180.0], [650.0, 180.0], [950.0, 570.0], [1000.0, 510.0]];
    (path, sharp)
}

#[test]
fn a_long_mixed_noisy_stroke_is_cleaned_along_its_whole_course() {
    let (clean, sharp) = mixed_stroke();
    let drawn = noisy(&clean, 6.0, 2.5);
    let s = recognize(&drawn, 1.0).expect("cleaned");
    assert_eq!(s.kind(), "freeform", "{s:?}");
    assert!(matches!(s, Shape::Freeform { closed: false, .. }), "an open stroke stays open");
    let out = s.points();
    // Its ends exactly where the stroke began and ended.
    assert!(dist(out[0], drawn[0]) < 1e-6 && dist(*out.last().unwrap(), *drawn.last().unwrap()) < 1e-6);
    // The whole course, within a few pixels of what was meant: nothing truncated or flattened.
    assert_follows(&s, &clean, 12.0, 12.0);
    // Peaks and zigzag tips stay pointed.
    for p in sharp {
        assert!(out.iter().any(|q| dist(*q, p) < 8.0), "the corner at {p:?} is kept");
    }
    // And the jitter is gone: far less turning than the drawn stroke.
    let (before, after) = (turning(&drawn, 600), turning(&out, 600));
    assert!(after < 0.5 * before, "turning {before:.1} → {after:.1}");
}

#[test]
fn waves_come_out_the_same_whatever_the_sampling_density() {
    let wave: Vec<P> = (0..=400).map(|i| [50.0 + i as f64, 200.0 + 30.0 * (std::f64::consts::TAU * i as f64 / 100.0).sin()]).collect();
    for step in [1.5, 4.0, 9.0] {
        let drawn = noisy(&wave, step, 1.5);
        let s = recognize(&drawn, 1.0).expect("cleaned");
        assert!(matches!(s.kind(), "curve" | "freeform"), "step {step}: {s:?}");
        assert_follows(&s, &wave, 7.0, 7.0);
        let out = s.points();
        assert!(dist(out[0], drawn[0]) < 1e-6 && dist(*out.last().unwrap(), *drawn.last().unwrap()) < 1e-6, "step {step}: ends kept");
        // Every crest and trough keeps its height.
        for k in 0..4 {
            let crest = [50.0 + 25.0 + 100.0 * k as f64, 230.0];
            assert!(out.iter().any(|q| dist(*q, crest) < 6.0), "step {step}: crest {crest:?}");
        }
    }
}

#[test]
fn a_very_long_stroke_keeps_every_small_wave() {
    // 40 waves 50 px high over 4800 px: long and complex, yet each wave is small next to its length.
    let wave: Vec<P> = (0..=4800).map(|i| [50.0 + i as f64, 300.0 + 25.0 * (std::f64::consts::TAU * i as f64 / 120.0).sin()]).collect();
    let drawn = noisy(&wave, 5.0, 1.5);
    let s = recognize(&drawn, 1.0).expect("cleaned");
    let Shape::Freeform { seg, closed: false } = &s else { panic!("an open freeform path: {s:?}") };
    // More pieces than the 64 an earlier limit allowed: no cap truncates or rejects it.
    assert!(seg.len() > 64, "{} pieces", seg.len());
    let out = s.points();
    assert!(dist(out[0], drawn[0]) < 1e-6 && dist(*out.last().unwrap(), *drawn.last().unwrap()) < 1e-6);
    assert_follows(&s, &wave, 6.0, 6.0);
    for k in 0..40 {
        let crest = [50.0 + 30.0 + 120.0 * k as f64, 325.0];
        assert!(out.iter().any(|q| dist(*q, crest) < 5.0), "crest {k} kept");
    }
}

#[test]
fn a_narrow_hairpin_stays_an_open_round_u_turn() {
    // Two parallel runs 24 px apart, joined by a round turn: ends close together, yet not a loop.
    let mut u = dense(&[[100.0, 100.0], [400.0, 100.0]]);
    u.extend((1..=38).map(|i| {
        let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 38.0;
        [400.0 + 12.0 * a.cos(), 112.0 + 12.0 * a.sin()]
    }));
    u.extend(dense(&[[400.0, 124.0], [100.0, 124.0]]).into_iter().skip(1));
    let drawn = noisy(&u, 5.0, 1.5);
    let s = recognize(&drawn, 1.0).expect("cleaned");
    assert!(s.end().is_some(), "open, not closed into a loop: {s:?}");
    assert!(matches!(s.kind(), "curve" | "freeform"), "a round turn, not a pointed V: {s:?}");
    assert_follows(&s, &u, 5.0, 5.0);
    // The turn stays round: its tip is still about 12 px out from the runs' end.
    assert!(s.points().iter().any(|q| dist(*q, [412.0, 112.0]) < 4.0));
}

#[test]
fn narrow_closed_capsules_are_cleaned_not_collapsed() {
    // A long thin rounded outline (500 × 30), as in the sketch.
    let mut cap = dense(&[[100.0, 100.0], [600.0, 100.0]]);
    cap.extend((1..=47).map(|i| {
        let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 47.0;
        [600.0 + 15.0 * a.cos(), 115.0 + 15.0 * a.sin()]
    }));
    cap.extend(dense(&[[600.0, 130.0], [100.0, 130.0]]).into_iter().skip(1));
    cap.extend((1..=47).map(|i| {
        let a = std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 47.0;
        [100.0 + 15.0 * a.cos(), 115.0 + 15.0 * a.sin()]
    }));
    let drawn = noisy(&cap, 5.0, 1.5);
    let s = recognize(&drawn, 1.0).expect("cleaned");
    assert!(s.end().is_none(), "closed: {s:?}");
    assert!(!matches!(s.kind(), "ellipse" | "circle" | "line"), "never squashed into an ellipse or a line: {s:?}");
    // Both long sides stay apart (the 30 px thickness is kept) and the outline follows the stroke.
    assert_follows(&s, &cap, 6.0, 6.0);
}

#[test]
fn cleanup_strength_sets_how_much_wobble_goes_but_never_the_corners() {
    let (clean, sharp) = mixed_stroke();
    let drawn = noisy(&clean, 6.0, 2.5);
    let raw = turning(&drawn, 600);
    let mut smooth = Vec::new();
    for strength in Strength::ALL {
        let s = recognize_with(&drawn, 1.0, Options { strength, ..Options::default() }).unwrap_or_else(|| panic!("{strength:?}: not cleaned"));
        assert_eq!(s.kind(), "freeform", "{strength:?}");
        let out = s.points();
        assert!(dist(out[0], drawn[0]) < 1e-6 && dist(*out.last().unwrap(), *drawn.last().unwrap()) < 1e-6, "{strength:?}: ends kept");
        assert_follows(&s, &clean, 16.0, 16.0);
        for p in &sharp {
            assert!(out.iter().any(|q| dist(*q, *p) < 8.0), "{strength:?}: the corner at {p:?} is kept");
        }
        smooth.push(turning(&out, 600));
    }
    assert_eq!(smooth.len(), 3, "one measurement per strength");
    let (light, balanced, strong) = (smooth[0], smooth[1], smooth[2]);
    assert!(light < raw && strong < 0.85 * light, "raw {raw:.1}, light {light:.1}, balanced {balanced:.1}, strong {strong:.1}");
    assert!(balanced <= 1.05 * light && strong <= 1.05 * balanced, "light {light:.1}, balanced {balanced:.1}, strong {strong:.1}");
}

#[test]
fn geometric_shapes_and_freeform_cleanup_switch_on_and_off_independently() {
    let circle = ellipse([150.0, 150.0], 80.0, 80.0, 0.0, 1.05, 80, 2.0);
    let (clean, _) = mixed_stroke();
    let mixed = noisy(&clean, 6.0, 2.5);
    let with = |geometric: bool, freeform: bool| Options { geometric, freeform, ..Options::default() };
    // Both on (the default): the circle is a circle, the mixed stroke is cleaned.
    assert_eq!(recognize_with(&circle, 1.0, with(true, true)).map(|s| s.kind()), Some("circle"));
    assert_eq!(recognize_with(&mixed, 1.0, with(true, true)).map(|s| s.kind()), Some("freeform"));
    // Geometric only: shapes still snap, anything else stays as drawn.
    assert_eq!(recognize_with(&circle, 1.0, with(true, false)).map(|s| s.kind()), Some("circle"));
    assert_eq!(recognize_with(&mixed, 1.0, with(true, false)), None);
    // Freeform only: nothing becomes a geometric primitive; the circle is cleaned along its course.
    let s = recognize_with(&circle, 1.0, with(false, true)).expect("cleaned");
    assert!(matches!(s, Shape::Freeform { closed: true, .. }), "{s:?}");
    assert_eq!(recognize_with(&mixed, 1.0, with(false, true)).map(|s| s.kind()), Some("freeform"));
    // Both off: nothing changes.
    assert_eq!(recognize_with(&circle, 1.0, with(false, false)), None);
    assert_eq!(recognize_with(&mixed, 1.0, with(false, false)), None);
}

#[test]
fn with_both_correction_types_off_holding_still_keeps_the_freehand_stroke() {
    let mut app = setup(true);
    app.ui.tool_options.smart_shape_geometric = false;
    app.ui.tool_options.smart_shape_freeform = false;
    let pts = ellipse([120.0, 120.0], 70.0, 70.0, 0.0, 1.05, 80, 2.0);
    let before = steps(&app);
    draw(&mut app, &pts, |_| 1.0);
    assert_eq!(hold(&mut app, 800.0), None, "no timer, no correction");
    release(&mut app, *pts.last().unwrap());
    assert_eq!(steps(&app), before + 1);
    assert!(is_freehand(&stroke_points(&app), &pts), "every freehand point");
}

#[test]
fn connected_angular_strokes_become_polylines_with_exact_corners() {
    let z = [[50.0, 50.0], [250.0, 50.0], [60.0, 200.0], [260.0, 200.0]];
    let w = [[40.0, 40.0], [90.0, 200.0], [140.0, 80.0], [190.0, 200.0], [240.0, 40.0]];
    let zig = [[40.0, 200.0], [80.0, 80.0], [120.0, 200.0], [160.0, 80.0], [200.0, 200.0], [240.0, 80.0], [280.0, 200.0]];
    for verts in [&z[..], &w[..], &zig[..]] {
        let drawn = noisy(&dense(verts), 4.0, 1.5);
        let s = recognize(&drawn, 1.0).expect("corrected");
        assert_vertices(&s, "polyline", verts, 5.0);
    }
    // A corner joining a straight arm to a curved one: no straight edge is forced over the curve.
    let mut v = dense(&[[50.0, 50.0], [150.0, 250.0]]);
    v.extend((1..=200).map(|i| {
        let t = i as f64 / 200.0;
        let u = 1.0 - t;
        [u * u * 150.0 + 2.0 * u * t * 300.0 + t * t * 300.0, u * u * 250.0 + 2.0 * u * t * 250.0 + t * t * 80.0]
    }));
    let drawn = noisy(&v, 4.0, 1.5);
    let s = recognize(&drawn, 1.0).expect("cleaned");
    let corners = fine_samples(&drawn, 320.0, 1.0).map(|(f, k)| sharp_corners(&f, k, false).iter().filter_map(|&i| f.get(i).copied()).collect::<Vec<P>>());
    assert_eq!(s.kind(), "freeform", "corners found: {corners:?}");
    assert_follows(&s, &v, 5.0, 5.0);
    assert!(s.points().iter().any(|q| dist(*q, [150.0, 250.0]) < 5.0), "the corner stays sharp");
}

#[test]
fn skewed_quadrilaterals_keep_their_corners_and_are_not_squared() {
    let parallelogram = [[100.0, 100.0], [300.0, 100.0], [380.0, 240.0], [180.0, 240.0]];
    let perspective = [[150.0, 80.0], [300.0, 110.0], [340.0, 280.0], [80.0, 230.0]];
    for verts in [parallelogram, perspective] {
        let drawn = polygon(&verts, 24, 2.0);
        let s = recognize(&drawn, 1.0).expect("corrected");
        assert_vertices(&s, "quadrilateral", &verts, 6.0);
    }
}

#[test]
fn s_and_u_curves_come_out_as_smooth_open_curves() {
    let s_curve: Vec<P> = (0..=300).map(|i| [50.0 + i as f64, 200.0 + 80.0 * (std::f64::consts::TAU * i as f64 / 300.0).sin()]).collect();
    let u_curve: Vec<P> = (0..=240).map(|i| [80.0 + i as f64, 100.0 + (i as f64 - 120.0).powi(2) / 100.0]).collect();
    for clean in [s_curve, u_curve] {
        let drawn = noisy(&clean, 5.0, 1.5);
        let s = recognize(&drawn, 1.0).expect("cleaned");
        assert!(matches!(s.kind(), "curve" | "freeform"), "{s:?}");
        assert!(s.end().is_some(), "stays open");
        let out = s.points();
        assert!(dist(out[0], drawn[0]) < 1e-6 && dist(*out.last().unwrap(), *drawn.last().unwrap()) < 1e-6);
        assert_follows(&s, &clean, 5.0, 5.0);
        assert!(turning(&out, 300) < turning(&drawn, 300), "smoother than drawn");
    }
}

#[test]
fn a_held_freeform_stroke_is_cleaned_while_pressed_and_undoes_in_one_step() {
    let mut app = setup(true);
    // A wavy stroke with a zigzag end, inside the 240 × 240 document.
    let mut path: Vec<P> = (0..=150).map(|i| [20.0 + i as f64, 80.0 + 20.0 * (std::f64::consts::TAU * i as f64 / 50.0).sin()]).collect();
    path.extend(dense(&[[170.0, 80.0], [185.0, 140.0], [200.0, 80.0], [215.0, 140.0]]).into_iter().skip(1));
    let drawn = noisy(&path, 4.0, 1.5);
    let before = steps(&app);
    draw(&mut app, &drawn, |_| 1.0);
    assert_eq!(hold(&mut app, 800.0), Some(Duration::ZERO), "cleaned while held, repainted at once");
    let shape = app.drag.as_ref().and_then(|d| d.smart.as_ref()).and_then(Hold::shape).expect("cleaned");
    assert!(matches!(shape.kind(), "freeform" | "curve" | "polyline"), "{shape:?}");
    release(&mut app, *drawn.last().unwrap());
    assert_eq!(steps(&app), before + 1, "one step");
    assert_eq!(last_stroke(&app)["smoothing"], json!(0.0));
    let painted = |app: &PhotocraftApp| alpha(app, 70, 80) > 0.5 || alpha(app, 70, 79) > 0.5 || alpha(app, 70, 81) > 0.5;
    assert!(painted(&app));
    assert!(app.session.undo());
    assert!(!painted(&app), "undo removes it");
    assert!(app.session.redo());
    assert!(painted(&app));
}

/// A before/after sheet for review: each sample stroke as drawn (red), then corrected at Light,
/// Balanced and Strong cleanup (black), painted with the real brush engine and saved as a PNG to
/// `$SMART_SHAPE_SHEET`. Run on demand: `SMART_SHAPE_SHEET=sheet.png cargo test -p
/// photocraft-ui-egui --lib smart_shape_before_after_sheet -- --ignored`.
#[test]
#[ignore = "writes a review image; run on demand"]
fn smart_shape_before_after_sheet() {
    let Ok(path) = std::env::var("SMART_SHAPE_SHEET") else { return };
    let (mixed, _) = mixed_stroke();
    // Samples in a 1200 × 1000 panel: the long mixed stroke (y 180..570), then a row of smaller ones.
    let zig = dense(&[[100.0, 700.0], [150.0, 620.0], [200.0, 700.0], [250.0, 620.0], [300.0, 700.0], [350.0, 620.0], [400.0, 700.0]]);
    let tri = polygon(&[[510.0, 620.0], [600.0, 780.0], [420.0, 780.0]], 24, 0.0);
    let quad = polygon(&[[700.0, 620.0], [830.0, 640.0], [860.0, 780.0], [660.0, 760.0]], 24, 0.0);
    let s_curve: Vec<P> = (0..=240).map(|i| [920.0 + i as f64, 700.0 + 70.0 * (std::f64::consts::TAU * i as f64 / 240.0).sin()]).collect();
    let w = dense(&[[100.0, 820.0], [140.0, 960.0], [180.0, 860.0], [220.0, 960.0], [260.0, 820.0]]);
    let wave: Vec<P> = (0..=400).map(|i| [320.0 + i as f64, 900.0 + 30.0 * (std::f64::consts::TAU * i as f64 / 100.0).sin()]).collect();
    let mut cap = dense(&[[780.0, 880.0], [1100.0, 880.0]]);
    cap.extend((1..=47).map(|i| {
        let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 47.0;
        [1100.0 + 15.0 * a.cos(), 895.0 + 15.0 * a.sin()]
    }));
    cap.extend(dense(&[[1100.0, 910.0], [780.0, 910.0]]).into_iter().skip(1));
    cap.extend((1..=47).map(|i| {
        let a = std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 47.0;
        [780.0 + 15.0 * a.cos(), 895.0 + 15.0 * a.sin()]
    }));
    let samples = [
        noisy(&mixed, 6.0, 3.0),
        noisy(&zig, 6.0, 2.5),
        noisy(&tri, 6.0, 2.5),
        noisy(&quad, 6.0, 2.5),
        noisy(&s_curve, 5.0, 2.0),
        noisy(&w, 5.0, 2.0),
        noisy(&wave, 5.0, 2.0),
        noisy(&cap, 6.0, 2.0),
    ];
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.run("file.new", json!({"width": 4800, "height": 1020, "background": "white"})).unwrap();
    let mut paint = |pts: &[P], dx: f64, color: &str| {
        let points: Vec<[f64; 3]> = pts.iter().map(|p| [p[0] + dx, p[1], 1.0]).collect();
        app.run("paint.stroke", json!({"points": points, "color": color, "size": 5, "smoothing": 0.0})).unwrap();
    };
    for drawn in &samples {
        paint(drawn, 0.0, "#c03030");
        for (col, strength) in Strength::ALL.into_iter().enumerate() {
            let opts = Options { strength, ..Options::default() };
            let cleaned = recognize_with(drawn, 1.0, opts).map_or_else(|| drawn.clone(), |s| s.points());
            paint(&cleaned, 1200.0 * (col + 1) as f64, "#101010");
        }
    }
    let doc = app.session.active().unwrap().doc.clone();
    let png = photocraft_io::export(&doc, "png", &Default::default()).unwrap();
    std::fs::write(&path, &png.bytes).unwrap();
}

/// A closed polygon through `verts` drawn by hand: `per_side` points along each side, wobbling by
/// `amp`, back to the first vertex.
fn polygon(verts: &[P], per_side: usize, amp: f64) -> Vec<P> {
    let mut out = Vec::new();
    for (k, &a) in verts.iter().enumerate() {
        let b = verts[(k + 1) % verts.len()];
        let mut side = line(a, b, per_side + 1, amp);
        side.pop();
        out.extend(side);
    }
    out.push(verts[0]);
    out
}

/// `p` turned by `angle` about `c`.
fn turned(p: P, c: P, angle: f64) -> P {
    let (sin, cos) = angle.sin_cos();
    let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
    [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos]
}

/// The polygon's vertices, and that each expected vertex has one within `tol`.
fn assert_vertices(s: &Shape, kind: &str, want: &[P], tol: f64) {
    assert_eq!(s.kind(), kind, "{s:?}");
    let Shape::Poly { v, n, .. } = *s else { panic!("{s:?}") };
    assert_eq!(n, want.len(), "{s:?}");
    for w in want {
        assert!(v[..n].iter().any(|q| dist(*q, *w) < tol), "{kind}: no vertex near {w:?} in {:?}", &v[..n]);
    }
}

#[test]
fn recognises_triangles_upright_rotated_and_rough() {
    let tri = [[120.0, 30.0], [210.0, 190.0], [30.0, 190.0]];
    let s = recognize(&polygon(&tri, 24, 0.8), 1.0).expect("a triangle");
    assert_vertices(&s, "triangle", &tri, 4.0);
    // Turned 35° about its middle.
    let c = [120.0, 140.0];
    let rot: Vec<P> = tri.iter().map(|&p| turned(p, c, 35f64.to_radians())).collect();
    let s = recognize(&polygon(&rot, 24, 0.8), 1.0).expect("a rotated triangle");
    assert_vertices(&s, "triangle", &rot, 4.0);
    // Rough: sides wobbling by 2.5 px, an uneven triangle. The vertices land where the sides meet.
    let rough = [[60.0, 40.0], [230.0, 120.0], [80.0, 210.0]];
    let s = recognize(&polygon(&rough, 20, 2.5), 1.0).expect("a rough triangle");
    assert_vertices(&s, "triangle", &rough, 6.0);
    // The outline is closed and passes through its vertices exactly.
    let out = s.points();
    assert_eq!(out.first(), out.last());
    let Shape::Poly { v, .. } = s else { panic!("{s:?}") };
    for q in &v[..3] {
        assert!(out.iter().any(|p| dist(*p, *q) < 1e-9));
    }
}

#[test]
fn recognises_skewed_quadrilaterals_and_keeps_rectangles_and_squares() {
    // A perspective-like trapezoid: no right angles, so not a rectangle.
    let trap = [[80.0, 60.0], [180.0, 60.0], [240.0, 190.0], [20.0, 190.0]];
    let s = recognize(&polygon(&trap, 20, 1.0), 1.0).expect("a trapezoid");
    assert_vertices(&s, "quadrilateral", &trap, 5.0);
    // An irregular four-sided form.
    let quad = [[40.0, 40.0], [240.0, 70.0], [170.0, 220.0], [60.0, 160.0]];
    let s = recognize(&polygon(&quad, 22, 1.0), 1.0).expect("a quadrilateral");
    assert_vertices(&s, "quadrilateral", &quad, 6.0);
    // Rectangles and squares still come out as such (rotated too).
    let s = recognize(&rect([150.0, 120.0], 100.0, 50.0, 0.3, 20, 1.5), 1.0).unwrap();
    assert_eq!(s.kind(), "rectangle");
    let s = recognize(&rect([120.0, 120.0], 60.0, 60.0, 0.0, 16, 1.5), 1.0).unwrap();
    assert_eq!(s.kind(), "square");
}

#[test]
fn circles_and_thin_ellipses_never_turn_into_polygons() {
    let s = recognize(&ellipse([150.0, 150.0], 80.0, 80.0, 0.0, 1.05, 90, 2.0), 1.0).unwrap();
    assert_eq!(s.kind(), "circle");
    let s = recognize(&ellipse([200.0, 150.0], 120.0, 36.0, 0.4, 1.0, 120, 1.0), 1.0).unwrap();
    assert_eq!(s.kind(), "ellipse", "a thin ellipse is not a diamond");
}

#[test]
fn a_wavy_flowing_line_becomes_a_clean_smooth_curve() {
    // Two waves, drawn with a hand's jitter.
    let wave = |x: f64| 100.0 + 20.0 * (std::f64::consts::TAU * (x - 20.0) / 160.0).sin();
    let pts: Vec<P> = (0..120)
        .map(|i| {
            let x = 20.0 + 320.0 * i as f64 / 119.0;
            [x, wave(x) + 1.5 * wobble(i)]
        })
        .collect();
    let s = recognize(&pts, 1.0).expect("a curve");
    assert_eq!(s.kind(), "curve");
    let Shape::Spline { n, .. } = s else { panic!("{s:?}") };
    assert!(n >= 2, "several smooth pieces for several waves: {n}");
    let out = s.points();
    // It follows the waves (no flattening) and keeps the ends.
    assert!(out.iter().all(|p| (p[1] - wave(p[0])).abs() < 8.0), "off the waves");
    assert!(dist(out[0], pts[0]) < 1e-6 && dist(*out.last().unwrap(), *pts.last().unwrap()) < 1e-6);
    // And it is smooth: no kinks between neighbouring segments of the outline.
    let kink = out
        .windows(3)
        .filter_map(|w| {
            let (u, v) = (sub(w[1], w[0]), sub(w[2], w[1]));
            let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
            (lu > 1e-9 && lv > 1e-9).then(|| (dot(u, v) / (lu * lv)).clamp(-1.0, 1.0).acos())
        })
        .fold(0.0, f64::max);
    assert!(kink < 0.25, "a kink of {kink} rad");
}

#[test]
fn an_l_becomes_straight_edges_meeting_at_the_corner() {
    let mut l = line([20.0, 20.0], [20.0, 150.0], 30, 0.8);
    l.extend(line([20.0, 150.0], [150.0, 150.0], 30, 0.8));
    let s = recognize(&l, 1.0).expect("a polyline");
    assert_vertices(&s, "polyline", &[[20.0, 20.0], [20.0, 150.0], [150.0, 150.0]], 4.0);
    assert_eq!(s.end().map(|e| dist(e, [150.0, 150.0]) < 4.0), Some(true), "dragged by its end");
}

#[test]
fn a_held_triangle_commits_as_one_undoable_step() {
    let mut app = setup(true);
    let tri = [[120.0, 30.0], [210.0, 190.0], [30.0, 190.0]];
    let pts = polygon(&tri, 24, 1.0);
    let before = steps(&app);
    draw(&mut app, &pts, |_| 1.0);
    assert_eq!(hold(&mut app, 800.0), Some(Duration::ZERO), "corrected while held");
    assert_eq!(app.drag.as_ref().and_then(|d| d.smart.as_ref()).and_then(Hold::shape).map(|s| s.kind()), Some("triangle"));
    release(&mut app, *pts.last().unwrap());
    assert_eq!(steps(&app), before + 1);
    let committed = stroke_points(&app);
    let (a, b) = (committed[0], *committed.last().unwrap());
    assert!(dist([a[0], a[1]], [b[0], b[1]]) < 1e-6, "closed");
    for v in tri {
        assert!(committed.iter().any(|q| dist([q[0], q[1]], v) < 4.0), "a corner at {v:?}");
    }
    // Straight sides: the middle of the long side is painted, a point inside is not.
    assert!(alpha(&app, 120, 190) > 0.9 && alpha(&app, 120, 140) < 0.05);
    assert!(app.session.undo());
    assert!(alpha(&app, 120, 190) < 0.05);
    assert!(app.session.redo());
    assert!(alpha(&app, 120, 190) > 0.9);
}

#[test]
fn small_marks_need_enough_size_on_screen() {
    let tiny = ellipse([50.0, 50.0], 5.0, 5.0, 0.0, 1.02, 40, 0.1);
    assert_eq!(recognize(&tiny, 1.0), None, "14 px on screen: too small");
    assert_eq!(recognize(&tiny, 4.0).map(|s| s.kind()), Some("circle"), "zoomed in, it is a circle");
}

#[test]
fn adjusting_scales_and_turns_about_the_pivot() {
    let s = Shape::Line { a: [0.0, 0.0], b: [10.0, 0.0] };
    // Grabbed at (10, 0), dragged to (0, 20): twice as long, turned 90°.
    let t = s.transformed(s.pivot(), 2.0, std::f64::consts::FRAC_PI_2);
    let Shape::Line { a, b } = t else { panic!() };
    assert!(dist(a, [0.0, 0.0]) < 1e-9 && dist(b, [0.0, 20.0]) < 1e-9, "{b:?}");
}

// ---------------------------------------------------------------------------------- the stroke

fn setup(smart: bool) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.run("file.new", json!({"width": 240, "height": 240, "background": "transparent"})).unwrap();
    app.run("tools.setBrush", json!({"brush": {"size": 4, "hardness": 1.0}})).unwrap();
    // Smoothing on in the brush: a shape must still come out with sharp, exact geometry.
    app.session.tools.brush.smoothing.amount = 0.6;
    app.ui.tool = Tool::Brush;
    app.ui.tool_options.smart_shape = smart;
    app
}

/// Press, move through `pts` (8 ms apart, `pressure(i)`), and leave the pointer on the last point.
fn draw(app: &mut PhotocraftApp, pts: &[P], pressure: impl Fn(usize) -> f32) {
    let none = egui::Modifiers::NONE;
    app.stylus.clock_ms = 1000.0;
    tool_event(app, ToolEvent::Down { x: pts[0][0], y: pts[0][1], pressure: pressure(0) }, none);
    for (i, p) in pts.iter().enumerate().skip(1) {
        app.stylus.clock_ms = 1000.0 + 8.0 * i as f64;
        tool_event(app, ToolEvent::Move { x: p[0], y: p[1], pressure: pressure(i) }, none);
    }
}

/// Keep the pointer where it is for `ms` (frames: the clock advances and Smart Shape ticks).
fn hold(app: &mut PhotocraftApp, ms: f64) -> Option<Duration> {
    let pointer = end(app);
    let pressure = app.drag.as_ref().and_then(|d| d.points.last()).map_or(1.0, |q| q[2] as f32);
    app.stylus.clock_ms += ms;
    let r = tick(app);
    // The frame's still pointer (the canvas sends the latest position each frame). A corrected
    // line, curve or polyline has put its end under the pointer.
    let p = app.drag.as_ref().and_then(|d| d.smart.as_ref()).and_then(Hold::shape).and_then(|s| s.end()).unwrap_or(pointer);
    tool_event(app, ToolEvent::Move { x: p[0], y: p[1], pressure }, egui::Modifiers::NONE);
    r
}

/// Where the stroke ends now (the shape's end once corrected).
fn end(app: &PhotocraftApp) -> P {
    app.drag.as_ref().and_then(|d| d.points.last()).map(|q| [q[0], q[1]]).unwrap()
}

fn release(app: &mut PhotocraftApp, p: P) {
    tool_event(app, ToolEvent::Up { x: p[0], y: p[1] }, egui::Modifiers::NONE);
}

fn last_stroke(app: &PhotocraftApp) -> serde_json::Value {
    app.session.journal.iter().rev().find(|(id, _)| id == "paint.stroke").map(|(_, p)| p.clone()).expect("a stroke")
}

fn stroke_points(app: &PhotocraftApp) -> Vec<[f64; 3]> {
    last_stroke(app)["points"].as_array().unwrap().iter().map(|q| [q[0].as_f64().unwrap(), q[1].as_f64().unwrap(), q[2].as_f64().unwrap()]).collect()
}

fn alpha(app: &PhotocraftApp, x: i32, y: i32) -> f32 {
    app.session.documents()[0].doc.layers[0].surface().unwrap().rgba(x, y)[3]
}

fn steps(app: &PhotocraftApp) -> usize {
    app.session.documents()[0].history.past_len()
}

/// A wobbly horizontal line at y = 120 and its points furthest from it.
fn wobbly_line() -> (Vec<P>, Vec<P>) {
    let pts = line([30.0, 120.0], [210.0, 120.0], 50, 3.5);
    let peaks = pts.iter().copied().filter(|p| (p[1] - 120.0).abs() > 3.2).collect();
    (pts, peaks)
}

/// A pixel the freehand stroke paints at wobble peak `p` (half a pixel or more beyond it, away
/// from y = 120) that a straight line at y ≈ 120 with a 4 px brush leaves clear.
fn beyond(p: P) -> (i32, i32) {
    let y = if p[1] > 120.0 { p[1].ceil() as i32 } else { p[1].floor() as i32 - 1 };
    (p[0].floor() as i32, y)
}

#[test]
fn holding_still_turns_the_stroke_into_its_shape_as_one_undo_step() {
    let mut app = setup(true);
    let (pts, peaks) = wobbly_line();
    assert!(!peaks.is_empty());
    let before = steps(&app);
    draw(&mut app, &pts, |_| 1.0);
    // Not yet: the hold is due in about 700 ms.
    let wait = hold(&mut app, 100.0).expect("pending");
    assert!(wait.as_millis() >= 550 && wait.as_millis() <= 610, "{wait:?}");
    assert!(!app.drag.as_ref().is_some_and(is_shape));
    // Due: corrected while still held, and the canvas is asked to repaint at once (a still pointer
    // brings no input; without this the shape only showed on release).
    assert_eq!(hold(&mut app, 650.0), Some(Duration::ZERO), "due: corrected, repaint now");
    assert_eq!(tick(&mut app), None, "nothing pending once corrected");
    let d = app.drag.as_ref().unwrap();
    assert_eq!(d.smart.as_ref().and_then(Hold::shape).map(|s| s.kind()), Some("line"));
    // The live preview now shows the line only: rebuilt from the document with every shape point.
    let (live, fed) = crate::canvas::live_stroke_state(&app).expect("live preview");
    assert_eq!(fed, d.points.len());
    let live_alpha = |x: i32, y: i32| live.layers[0].surface().unwrap().rgba(x, y)[3];
    assert!(live_alpha(120, 120) > 0.9, "the line shows");
    for p in &peaks {
        let (x, y) = beyond(*p);
        assert!(live_alpha(x, y) < 0.05, "no freehand mark at {p:?} in the preview");
    }
    let e = end(&app);
    release(&mut app, e);
    // One stroke, one step: straight, smoothing off for it, nothing freehand left.
    assert_eq!(steps(&app), before + 1);
    let committed = stroke_points(&app);
    assert!(committed.len() >= 2 && committed.iter().all(|q| (q[1] - 120.0).abs() < 1.5), "straight");
    let (a, b) = (committed[0], *committed.last().unwrap());
    for q in &committed {
        // On the segment from the first to the last point.
        let t = ((q[0] - a[0]) * (b[0] - a[0]) + (q[1] - a[1]) * (b[1] - a[1])) / ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2));
        let on = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        assert!(dist(on, [q[0], q[1]]) < 1e-6, "collinear");
    }
    assert_eq!(last_stroke(&app)["smoothing"], json!(0.0));
    assert!(alpha(&app, 120, 120) > 0.9);
    for p in &peaks {
        let (x, y) = beyond(*p);
        assert!(alpha(&app, x, y) < 0.05, "no duplicate freehand stroke at {p:?}");
    }
    assert_eq!(app.session.tools.brush.smoothing.amount, 0.6, "the brush itself is unchanged");
    // Undo removes the whole shape; Redo brings it back.
    assert!(app.session.undo());
    assert!(alpha(&app, 120, 120) < 0.05 && steps(&app) == before);
    assert!(app.session.redo());
    assert!(alpha(&app, 120, 120) > 0.9 && steps(&app) == before + 1);
}

/// Through real frames and pointer events, as on screen: press, draw, then keep the button down
/// without moving. The stroke must become its shape while the button is still down, not on release.
#[test]
fn a_held_stroke_is_corrected_in_real_frames_before_release() {
    use egui::{Event, Modifiers, PointerButton, vec2};
    let mut a = setup(true);
    a.sync_views();
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1000.0, 700.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            let ctx = ui.ctx().clone();
            if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
        },
        a,
    );
    crate::PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::default());
    for _ in 0..4 {
        h.step();
    }
    let before = steps(h.state());
    let c = h.state().last_canvas_rect.center();
    let start = c - vec2(150.0, 0.0);
    h.event(Event::PointerMoved(start));
    h.step();
    h.event(Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    let mut end = start;
    for k in 1..=30i32 {
        end = start + vec2(10.0 * k as f32, if k % 2 == 0 { 2.0 } else { -2.0 });
        h.event(Event::PointerMoved(end));
        h.step();
    }
    assert!(h.state().drag.is_some() && !h.state().drag.as_ref().is_some_and(is_shape), "drawing, not corrected yet");
    // Hold still: frames go on with no input at all.
    for _ in 0..120 {
        h.step();
        if h.state().drag.as_ref().is_some_and(is_shape) {
            break;
        }
    }
    assert!(h.state().drag.as_ref().is_some_and(is_shape), "corrected while the button is still down");
    assert!(crate::canvas::live_stroke_state(h.state()).is_some(), "and previewed");
    h.event(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().drag.is_none());
    assert_eq!(steps(h.state()), before + 1, "one step");
    assert_eq!(last_stroke(h.state())["smoothing"], json!(0.0), "the committed stroke is the shape");
}

#[test]
fn releasing_early_keeps_the_freehand_stroke() {
    let mut app = setup(true);
    let (pts, peaks) = wobbly_line();
    draw(&mut app, &pts, |_| 1.0);
    assert!(hold(&mut app, 400.0).is_some(), "still waiting");
    release(&mut app, *pts.last().unwrap());
    assert!(is_freehand(&stroke_points(&app), &pts), "every freehand point, wobble and all");
    assert!(last_stroke(&app).get("smoothing").is_none(), "the brush's own smoothing");
    assert!(!peaks.is_empty());
}

/// `committed` is the freehand stroke `pts`: each of its points in order (a held pen may repeat the
/// last one, as without Smart Shape), nothing else.
fn is_freehand(committed: &[[f64; 3]], pts: &[P]) -> bool {
    let mut it = committed.iter().map(|q| [q[0], q[1]]).peekable();
    for p in pts {
        if it.next().is_none_or(|q| dist(q, *p) > 1e-9) {
            return false;
        }
    }
    let last = *pts.last().unwrap();
    it.all(|q| dist(q, last) < 1e-9)
}

#[test]
fn smart_shape_is_off_by_default_and_needs_brush_or_pencil() {
    assert!(!crate::state::ToolOptions::default().smart_shape, "off until enabled");
    assert_eq!(crate::state::ToolOptions::default().smart_shape_hold_ms, DEFAULT_HOLD_MS);
    let mut app = setup(false);
    let (pts, _) = wobbly_line();
    draw(&mut app, &pts, |_| 1.0);
    assert_eq!(hold(&mut app, 2000.0), None);
    assert!(!app.drag.as_ref().is_some_and(is_shape), "off: never corrected");
    release(&mut app, *pts.last().unwrap());
    assert!(is_freehand(&stroke_points(&app), &pts));
    // On, but the Eraser: not offered there.
    let mut app = setup(true);
    app.ui.tool = Tool::Eraser;
    draw(&mut app, &pts, |_| 1.0);
    hold(&mut app, 2000.0);
    assert!(!app.drag.as_ref().is_some_and(is_shape));
    release(&mut app, *pts.last().unwrap());
    // The Pencil corrects too.
    let mut app = setup(true);
    app.ui.tool = Tool::Pencil;
    draw(&mut app, &pts, |_| 1.0);
    hold(&mut app, 800.0);
    assert!(app.drag.as_ref().is_some_and(is_shape), "Pencil");
}

#[test]
fn small_movements_keep_the_hold_and_larger_ones_restart_it() {
    let mut app = setup(true);
    let (pts, _) = wobbly_line();
    draw(&mut app, &pts, |_| 1.0);
    let none = egui::Modifiers::NONE;
    // The pen comes to rest a little further on (past the still radius, so that is where it rests).
    let last = *pts.last().unwrap();
    let end = [last[0] + 8.0, last[1]];
    app.stylus.clock_ms += 8.0;
    tool_event(&mut app, ToolEvent::Move { x: end[0], y: end[1], pressure: 1.0 }, none);
    // A tremor of a few points around it does not restart the hold.
    for (k, dy) in [1.5, -2.0, 2.5, -1.0].into_iter().enumerate() {
        app.stylus.clock_ms += 150.0;
        tool_event(&mut app, ToolEvent::Move { x: end[0], y: end[1] + dy, pressure: 1.0 }, none);
        assert!(tick(&mut app).is_some(), "still waiting at step {k}");
    }
    app.stylus.clock_ms += 150.0; // 750 ms since the pointer came to rest
    assert_eq!(tick(&mut app), Some(Duration::ZERO), "corrected while held: repaint now");
    assert!(app.drag.as_ref().is_some_and(is_shape), "tremor kept the hold");

    // Moving on (more than the still radius) restarts the timer.
    let mut app = setup(true);
    draw(&mut app, &pts, |_| 1.0);
    app.stylus.clock_ms += 600.0;
    tool_event(&mut app, ToolEvent::Move { x: end[0] + 12.0, y: end[1], pressure: 1.0 }, none);
    app.stylus.clock_ms += 300.0;
    assert!(tick(&mut app).is_some(), "the move restarted the hold");
    assert!(!app.drag.as_ref().is_some_and(is_shape));
}

#[test]
fn the_hold_time_is_configurable() {
    let mut app = setup(true);
    app.ui.tool_options.smart_shape_hold_ms = 300.0;
    let (pts, _) = wobbly_line();
    draw(&mut app, &pts, |_| 1.0);
    assert!(hold(&mut app, 250.0).is_some());
    assert_eq!(hold(&mut app, 60.0), Some(Duration::ZERO));
    assert!(app.drag.as_ref().is_some_and(is_shape), "corrected after 310 ms");
    // Out-of-range settings are clamped, never zero.
    let mut other = setup(true);
    other.ui.tool_options.smart_shape_hold_ms = 0.0;
    draw(&mut other, &pts, |_| 1.0);
    assert!(hold(&mut other, 100.0).is_some(), "at least {} ms", HOLD_RANGE.start());
}

#[test]
fn dragging_while_held_adjusts_the_shape() {
    let mut app = setup(true);
    let (pts, _) = wobbly_line();
    draw(&mut app, &pts, |_| 1.0);
    hold(&mut app, 800.0);
    assert!(app.drag.as_ref().is_some_and(is_shape));
    let none = egui::Modifiers::NONE;
    // Drag the end 30 px further and 40 px down: the line follows from its start.
    tool_event(&mut app, ToolEvent::Move { x: 210.0, y: 160.0, pressure: 1.0 }, none);
    let shape = app.drag.as_ref().and_then(|d| d.smart.as_ref()).and_then(Hold::shape).unwrap();
    let Shape::Line { a, b } = shape else { panic!("{shape:?}") };
    assert!(dist(a, [30.0, 120.0]) < 3.0, "the start stays");
    assert!(dist(b, [210.0, 160.0]) < 3.0, "the end follows the pointer: {b:?}");
    // The live preview follows the adjusted shape.
    let (live, _) = crate::canvas::live_stroke_state(&app).unwrap();
    let mid = lerp(a, b, 0.5);
    assert!(live.layers[0].surface().unwrap().rgba(mid[0].round() as i32, mid[1].round() as i32)[3] > 0.9);
    release(&mut app, [210.0, 160.0]);
    let committed = stroke_points(&app);
    let last = committed.last().unwrap();
    assert!(dist([last[0], last[1]], [210.0, 160.0]) < 3.0, "committed as adjusted");
    // Where the line was before the adjustment (y = 120 near its end) nothing is left.
    assert!(alpha(&app, 180, 120) < 0.05, "not left at the first position");
    assert!(alpha(&app, mid[0].round() as i32, mid[1].round() as i32) > 0.9);
}

#[test]
fn a_stylus_keeps_its_pressure_along_the_shape() {
    let mut app = setup(true);
    app.session.tools.brush.pressure_size = true;
    let pts = ellipse([120.0, 120.0], 70.0, 70.0, 0.0, 1.04, 80, 1.5);
    let n = pts.len();
    // Light at the start, heavy at the end.
    let pressure = move |i: usize| 0.2 + 0.8 * i as f32 / (n - 1) as f32;
    draw(&mut app, &pts, pressure);
    hold(&mut app, 800.0);
    assert_eq!(app.drag.as_ref().and_then(|d| d.smart.as_ref()).and_then(Hold::shape).map(|s| s.kind()), Some("circle"));
    release(&mut app, *pts.last().unwrap());
    let committed = stroke_points(&app);
    let (first, last) = (committed[0][2], committed.last().unwrap()[2]);
    assert!(near(first, 0.2, 0.1) && near(last, 1.0, 0.1), "pressure ramp kept: {first} → {last}");
    assert!(committed.iter().all(|q| (0.19..=1.01).contains(&q[2])));
    assert!(committed.windows(2).filter(|w| w[1][2] + 1e-9 < w[0][2]).count() <= 2, "and it keeps rising");
    // On the circle, closed.
    assert!(committed.iter().all(|q| near(dist([q[0], q[1]], [120.0, 120.0]), 70.0, 5.0)));
    let (a, b) = (committed[0], *committed.last().unwrap());
    assert!(dist([a[0], a[1]], [b[0], b[1]]) < 1e-6, "closed outline");
}

#[test]
fn esc_puts_the_freehand_stroke_back() {
    let mut app = setup(true);
    let (pts, _) = wobbly_line();
    draw(&mut app, &pts, |_| 1.0);
    hold(&mut app, 800.0);
    assert!(app.drag.as_ref().is_some_and(is_shape));
    assert!(revert(&mut app));
    assert!(!app.drag.as_ref().is_some_and(is_shape));
    // Holding on does not snap again until the pointer moves.
    assert_eq!(hold(&mut app, 2000.0), None);
    assert!(!app.drag.as_ref().is_some_and(is_shape));
    release(&mut app, *pts.last().unwrap());
    assert!(is_freehand(&stroke_points(&app), &pts), "the freehand stroke, as drawn");
    assert!(last_stroke(&app).get("smoothing").is_none());
}

#[test]
fn shift_lines_and_shift_click_connections_are_left_alone() {
    let mut app = setup(true);
    let shift = egui::Modifiers::SHIFT;
    app.stylus.clock_ms = 0.0;
    tool_event(&mut app, ToolEvent::Down { x: 20.0, y: 20.0, pressure: 1.0 }, shift);
    tool_event(&mut app, ToolEvent::Move { x: 120.0, y: 25.0, pressure: 1.0 }, shift);
    assert!(app.drag.as_ref().is_some_and(|d| d.smart.is_none()), "⇧ constrains already");
    app.stylus.clock_ms = 2000.0;
    assert_eq!(tick(&mut app), None);
    release(&mut app, [120.0, 25.0]);
}

#[test]
fn works_with_sync_view_on() {
    let mut app = setup(true);
    // A second document of the same size, the drawing (active); linked with Sync View.
    app.run("file.new", json!({"width": 240, "height": 240, "background": "transparent"})).unwrap();
    app.sync_views();
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, crate::sync_view::COMMAND, json!({})).unwrap();
    assert!(crate::sync_view::is_on(&app));
    let reference = app.session.documents()[0].doc.clone();
    let before = app.session.documents()[1].history.past_len();
    let (pts, _) = wobbly_line();
    draw(&mut app, &pts, |_| 1.0);
    hold(&mut app, 800.0);
    assert!(app.drag.as_ref().is_some_and(is_shape));
    release(&mut app, *pts.last().unwrap());
    let docs = app.session.documents();
    assert_eq!(docs[1].history.past_len(), before + 1, "one step, on the drawing");
    assert!(docs[1].doc.layers[0].surface().unwrap().rgba(120, 120)[3] > 0.9, "the shape is on the drawing");
    assert!(Arc::ptr_eq(&reference, &docs[0].doc), "the reference document is untouched");
    assert!(crate::sync_view::is_on(&app) && app.ui.windows.is_empty(), "Sync View stays on, no OS window");
}

/// The options bar: the Smart Shape toggle closes the Brush and Pencil controls, right before
/// Sync View; a click turns it on and off; other tools don't offer it.
#[test]
fn the_toggle_sits_with_the_brush_controls() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 900.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.prefs.edit(|p| p.interface.language = "en".into());
        app.ui.tool = Tool::Brush;
        app.run("file.new", json!({"width": 640, "height": 360})).unwrap();
        app.sync_views();
        app
    });
    h.run_steps(4);
    let smart = h.get_by_label(TOOLTIP).rect();
    let symmetry = h.get_by_label("Set painting symmetry options").rect();
    let sync = h.get_by_label(crate::sync_view::TOOLTIP).rect();
    assert!(smart.left() >= symmetry.right() && sync.left() >= smart.right(), "symmetry | Smart Shape | Sync View");
    assert!((smart.center().y - sync.center().y).abs() < 2.0, "one row");
    assert_eq!((smart.width().round(), smart.height().round()), (24.0, 24.0));
    assert!(!h.state().ui.tool_options.smart_shape);
    h.get_by_label(TOOLTIP).click();
    h.run_steps(2);
    assert!(h.state().ui.tool_options.smart_shape, "a click turns it on");
    assert!(!h.get_by_label(TOOLTIP).accesskit_node().is_disabled());
    h.get_by_label(TOOLTIP).click();
    h.run_steps(2);
    assert!(!h.state().ui.tool_options.smart_shape, "and off again");
    h.state_mut().ui.tool = Tool::Pencil;
    h.run_steps(3);
    assert_eq!(h.query_all_by_label(TOOLTIP).count(), 1, "the Pencil has it too");
    for tool in [Tool::Eraser, Tool::Move, Tool::Hand] {
        h.state_mut().ui.tool = tool;
        h.run_steps(3);
        assert_eq!(h.query_all_by_label(TOOLTIP).count(), 0, "{tool:?}: not offered");
    }
}

#[test]
fn the_tooltip_and_setting_are_translated() {
    for lang in crate::i18n::Lang::all().filter(|l| l.complete_menus()) {
        for text in [TOOLTIP, "Smart Shape hold time"] {
            assert!(crate::i18n::has(lang, text), "{}: untranslated {text}", lang.code());
        }
    }
}
