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

#[test]
fn ambiguous_or_accidental_strokes_stay_freehand() {
    // Handwriting-like zig-zag.
    let tooth = |i: usize| if (i / 5).is_multiple_of(2) { (i % 5) as f64 * 12.0 } else { (5 - i % 5) as f64 * 12.0 };
    let zigzag: Vec<P> = (0..41).map(|i| [20.0 + i as f64 * 5.0, 100.0 + tooth(i)]).collect();
    assert_eq!(recognize(&zigzag, 1.0), None, "zig-zag");
    // An L: two straight strokes with a corner.
    let mut l = line([20.0, 20.0], [20.0, 150.0], 30, 0.5);
    l.extend(line([20.0, 150.0], [150.0, 150.0], 30, 0.5));
    assert_eq!(recognize(&l, 1.0), None, "L");
    // A closed triangle.
    let tri = [[100.0, 20.0], [180.0, 160.0], [20.0, 160.0], [100.0, 20.0]];
    let mut t = Vec::new();
    for w in tri.windows(2) {
        t.extend(line(w[0], w[1], 25, 0.5));
    }
    assert_eq!(recognize(&t, 1.0), None, "triangle");
    // A spiral (two turns, growing).
    let spiral: Vec<P> = (0..120)
        .map(|i| {
            let t = std::f64::consts::TAU * 2.0 * i as f64 / 119.0;
            let r = 20.0 + 40.0 * i as f64 / 119.0;
            [150.0 + r * t.cos(), 150.0 + r * t.sin()]
        })
        .collect();
    assert_eq!(recognize(&spiral, 1.0), None, "spiral");
    // A scribble that goes back over itself.
    let back: Vec<P> = (0..40).map(|i| [50.0 + (i as f64 * 0.5).sin() * 60.0, 50.0 + i as f64 * 0.3]).collect();
    assert_eq!(recognize(&back, 1.0), None, "back and forth");
    // Degenerate input never panics.
    assert_eq!(recognize(&[], 1.0), None);
    assert_eq!(recognize(&[[5.0, 5.0]], 1.0), None);
    assert_eq!(recognize(&[[5.0, 5.0]; 30], 1.0), None);
    assert_eq!(recognize(&[[f64::NAN, 1.0], [f64::INFINITY, 2.0]], 1.0), None);
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
    // line or curve has put its end under the pointer.
    let p = match app.drag.as_ref().and_then(|d| d.smart.as_ref()).and_then(Hold::shape) {
        Some(Shape::Line { b, .. }) => b,
        Some(Shape::Curve { p }) => p[3],
        _ => pointer,
    };
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
    assert_eq!(hold(&mut app, 650.0), None, "due: corrected");
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
    let end = *pts.last().unwrap();
    let none = egui::Modifiers::NONE;
    // A tremor of a few points does not restart the hold.
    for (k, dy) in [1.5, -2.0, 2.5, -1.0].into_iter().enumerate() {
        app.stylus.clock_ms += 150.0;
        tool_event(&mut app, ToolEvent::Move { x: end[0], y: end[1] + dy, pressure: 1.0 }, none);
        assert!(tick(&mut app).is_some(), "still waiting at step {k}");
    }
    app.stylus.clock_ms += 150.0; // 750 ms since the pointer came to rest
    assert_eq!(tick(&mut app), None);
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
    assert_eq!(hold(&mut app, 60.0), None);
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
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, crate::sync_view::COMMAND, json!({})).unwrap();
    let (pts, _) = wobbly_line();
    let before = steps(&app);
    draw(&mut app, &pts, |_| 1.0);
    hold(&mut app, 800.0);
    release(&mut app, *pts.last().unwrap());
    assert_eq!(steps(&app), before + 1);
    assert!(crate::sync_view::is_on(&app) && app.ui.windows.len() == 1, "Sync View stays on");
    assert_eq!(app.session.documents().len(), 1, "one document");
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
    for tool in [Tool::Pencil] {
        h.state_mut().ui.tool = tool;
        h.run_steps(3);
        assert_eq!(h.query_all_by_label(TOOLTIP).count(), 1, "{tool:?}");
    }
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
