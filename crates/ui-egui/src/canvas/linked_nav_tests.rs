//! Window › Arrange › NAFY through the whole app: a reference (left) and a drawing
//! (right) of the same size share one camera. Real frames and egui input, like a user's.

use std::sync::Arc;

use egui::{Modifiers, PointerButton, Pos2, Rect, vec2};
use egui_kittest::Harness;
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::state::{Tool, View};

const LINK: &str = "window.arrange.linkNavigation";

/// A real app window with a reference and then a drawing (active) of these sizes.
fn harness(reference: [u32; 2], drawing: [u32; 2]) -> Harness<'static, PhotocraftApp> {
    let mut h = Harness::builder().with_size(vec2(1200.0, 700.0)).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.prefs.edit(|p| p.interface.language = "en".into());
        for (name, [w, h]) in [("reference", reference), ("drawing", drawing)] {
            app.run("file.new", json!({"width": w, "height": h, "name": name, "background": "white"})).unwrap();
        }
        app.sync_views();
        app.ui.extras.rulers = false;
        app
    });
    h.run_steps(4);
    h
}

fn invoke(h: &mut Harness<'static, PhotocraftApp>, id: &str, params: Value) -> Result<Value, String> {
    let ctx = h.ctx.clone();
    let r = crate::menus::invoke(h.state_mut(), &ctx, id, params);
    h.run_steps(3);
    r
}

fn linked() -> Harness<'static, PhotocraftApp> {
    let mut h = harness([640, 360], [640, 360]);
    invoke(&mut h, LINK, json!({})).unwrap();
    assert!(crate::view_cmds::linked_pair(h.state()).is_some());
    h
}

/// `(drawing, reference)` indices.
fn pair(h: &Harness<'_, PhotocraftApp>) -> (usize, usize) {
    crate::view_cmds::linked_pair(h.state()).expect("linked")
}

fn camera(v: &View) -> (f32, [f32; 2], f32) {
    (v.zoom, v.center, v.rotation)
}

fn cameras(h: &Harness<'_, PhotocraftApp>) -> ((f32, [f32; 2], f32), (f32, [f32; 2], f32)) {
    let (d, r) = pair(h);
    (camera(&h.state().ui.views[d]), camera(&h.state().ui.views[r]))
}

/// `(drawing pane, reference pane)` on screen.
fn panes(h: &Harness<'_, PhotocraftApp>) -> (Rect, Rect) {
    let app = h.state();
    let cells = crate::canvas::arranged_cells(app, app.last_canvas_rect).expect("arranged");
    let (d, r) = pair(h);
    let at = |doc| cells.iter().find(|(i, _)| *i == doc).map(|(_, c)| *c).expect("pane");
    (at(d), at(r))
}

fn click(h: &mut Harness<'static, PhotocraftApp>, p: Pos2) {
    h.event(egui::Event::PointerMoved(p));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

fn drag(h: &mut Harness<'static, PhotocraftApp>, from: Pos2, to: Pos2) {
    h.event(egui::Event::PointerMoved(from));
    h.event(egui::Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for i in 1..=6 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 6.0)));
        h.step();
    }
    h.event(egui::Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn wheel(h: &mut Harness<'static, PhotocraftApp>, at: Pos2, dy: f32, modifiers: Modifiers) {
    h.event(egui::Event::PointerMoved(at));
    h.step();
    h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, dy), phase: egui::TouchPhase::Move, modifiers });
    h.run_steps(3);
}

/// What navigation must never change: each document's content and history.
fn snapshot(h: &Harness<'_, PhotocraftApp>) -> Vec<(Arc<photocraft_doc::Document>, u64, usize)> {
    h.state().session.documents().iter().map(|st| (st.doc.clone(), st.revision, st.history.past_len())).collect()
}

fn unchanged(before: &[(Arc<photocraft_doc::Document>, u64, usize)], h: &Harness<'_, PhotocraftApp>) {
    for ((doc, rev, steps), now) in before.iter().zip(snapshot(h)) {
        assert!(Arc::ptr_eq(doc, &now.0), "navigation replaced a document");
        assert_eq!((*rev, *steps), (now.1, now.2), "navigation changed a revision or the history");
    }
}

#[test]
fn reference_is_left_and_the_active_drawing_right() {
    let h = linked();
    let app = h.state();
    let (d, r) = pair(&h);
    assert_eq!(app.session.documents()[d].doc.name, "drawing", "the document active when linking draws");
    assert_eq!(app.session.documents()[r].doc.name, "reference");
    assert_eq!(app.session.active_index(), Some(d));
    assert_eq!(app.ui.view.arrange, "twoUpVertical");
    let (drawing, reference) = panes(&h);
    assert!(reference.max.x <= drawing.min.x + 1.0, "reference {reference:?} left of drawing {drawing:?}");
    assert!((reference.width() - drawing.width()).abs() < 1.0 && (reference.height() - drawing.height()).abs() < 1.0, "equal panes");
}

#[test]
fn drawing_zoom_pan_and_rotation_drive_the_reference_live() {
    let mut h = linked();
    let before = snapshot(&h);
    let (d, _) = pair(&h);
    // Fit on linking: both show the same fitted camera.
    let (a, b) = cameras(&h);
    assert_eq!(a, b);
    // View › 200%, Rotate View 28°, Reset View: commands on the drawing.
    invoke(&mut h, "view.twoHundredPercent", json!({})).unwrap();
    invoke(&mut h, "view.rotateView", json!({"angle": 28.0})).unwrap();
    let (a, b) = cameras(&h);
    assert_eq!((a.0, a.2), (2.0, 28.0));
    assert_eq!(a, b, "zoom and rotation follow");
    // Any camera change on the drawing (navigator, scroll bars, control channel) follows next frame.
    {
        let v = &mut h.state_mut().ui.views[d];
        v.zoom = 2.75;
        v.center = [421.5, 183.25];
    }
    h.run_steps(2);
    let (a, b) = cameras(&h);
    assert_eq!(b, (2.75, [421.5, 183.25], 28.0));
    assert_eq!(a, b);
    invoke(&mut h, "view.resetView", json!({})).unwrap();
    let (a, b) = cameras(&h);
    assert_eq!((a.2, b.2), (0.0, 0.0));
    unchanged(&before, &h);
}

#[test]
fn hand_drag_and_wheel_on_either_pane_move_both() {
    let mut h = linked();
    let before = snapshot(&h);
    invoke(&mut h, "view.rotateView", json!({"angle": 28.0})).unwrap();
    h.state_mut().ui.tool = Tool::Hand;
    h.run_steps(2);
    // Pan while rotated, on the drawing.
    let (drawing, reference) = panes(&h);
    let start = cameras(&h).0;
    drag(&mut h, drawing.center(), drawing.center() + vec2(-120.0, -40.0));
    let (a, b) = cameras(&h);
    assert_ne!(a.1, start.1, "the Hand panned the drawing");
    assert_eq!(a, b, "the reference panned with it");
    // Scroll over the reference: it is view only, so the drawing takes the move and both follow.
    let start = a;
    wheel(&mut h, reference.center(), -80.0, Modifiers::NONE);
    let (a, b) = cameras(&h);
    assert_ne!(a.1, start.1, "scrolling the reference pans the pair");
    assert_eq!(a, b);
    // ⌥-scroll zooms around the pointer on the drawing.
    let start = a;
    wheel(&mut h, drawing.center() + vec2(60.0, 30.0), 120.0, Modifiers::ALT);
    let (a, b) = cameras(&h);
    assert_ne!(a.0, start.0, "⌥-scroll zoomed");
    assert_eq!(a, b);
    unchanged(&before, &h);
}

#[test]
fn the_reference_never_becomes_the_editable_document() {
    let mut h = linked();
    let (d, r) = pair(&h);
    let (_, reference) = panes(&h);
    // Clicking the reference pane.
    h.state_mut().ui.tool = Tool::Brush;
    let reference_doc = h.state().session.documents()[r].doc.clone();
    click(&mut h, reference.center());
    assert_eq!(h.state().session.active_index(), Some(d), "a click on the reference doesn't activate it");
    assert!(Arc::ptr_eq(&reference_doc, &h.state().session.documents()[r].doc), "nor paints on it");
    // A tab click (or anything else) activating it directly.
    h.state_mut().session.set_active(r);
    h.run_steps(2);
    assert_eq!(h.state().session.active_index(), Some(d));
    // Ctrl+Tab / Window › next document.
    let ctrl = Modifiers { ctrl: true, command: !cfg!(target_os = "macos"), ..Modifiers::NONE };
    h.key_press_modifiers(ctrl, egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(h.state().session.active_index(), Some(d));
    invoke(&mut h, "window.nextDocument", json!({})).unwrap();
    assert_eq!(h.state().session.active_index(), Some(d));
    // Reordering tabs keeps the pair by identity: still drawing right, reference left.
    h.state_mut().session.move_document(0, 1);
    h.run_steps(2);
    let (d2, r2) = pair(&h);
    assert_eq!(h.state().session.documents()[d2].doc.name, "drawing");
    assert_eq!(h.state().session.documents()[r2].doc.name, "reference");
    assert_eq!(h.state().session.active_index(), Some(d2));
}

#[test]
fn painting_changes_only_the_drawing() {
    let mut h = linked();
    let (d, r) = pair(&h);
    let reference = (h.state().session.documents()[r].doc.clone(), h.state().session.documents()[r].revision);
    let drawing_rev = h.state().session.documents()[d].revision;
    {
        let app = h.state_mut();
        app.run("select.rect", json!({"x": 10, "y": 10, "width": 40, "height": 40})).unwrap();
        app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
    }
    h.run_steps(2);
    let docs = h.state().session.documents();
    assert!(docs[d].revision > drawing_rev, "the drawing was painted");
    assert!(Arc::ptr_eq(&reference.0, &docs[r].doc) && docs[r].revision == reference.1, "the reference is untouched");
    assert!(crate::view_cmds::linked_pair(h.state()).is_some(), "painting keeps the link");
}

#[test]
fn unlinked_views_navigate_independently_as_before() {
    let mut h = linked();
    let (d, r) = pair(&h);
    invoke(&mut h, LINK, json!({})).unwrap();
    assert!(h.state().ui.view.linked_navigation.is_none());
    assert_eq!(h.state().ui.view.arrange, "twoUpVertical", "unlinking keeps the layout");
    let reference = camera(&h.state().ui.views[r]);
    h.state_mut().ui.views[d].zoom = 3.0;
    h.state_mut().ui.views[d].rotation = 15.0;
    h.run_steps(2);
    assert_eq!(camera(&h.state().ui.views[r]), reference, "the reference keeps its own camera");
    // A click on the other pane activates it again, as before linking existed.
    let cells = crate::canvas::arranged_cells(h.state(), h.state().last_canvas_rect).unwrap();
    let other = cells.iter().find(|(i, _)| *i == r).map(|(_, c)| *c).unwrap();
    click(&mut h, other.center());
    assert_eq!(h.state().session.active_index(), Some(r));
}

#[test]
fn closing_resizing_or_rearranging_turns_the_link_off_safely() {
    // Closing the reference.
    let mut h = linked();
    let (_, r) = pair(&h);
    h.state_mut().run("file.close", json!({"document": r})).unwrap();
    h.run_steps(2);
    assert!(h.state().ui.view.linked_navigation.is_none());
    assert_eq!(h.state().session.documents().len(), 1);
    assert_eq!(h.state().session.active().unwrap().doc.name, "drawing", "the drawing survives");
    // Resizing the drawing.
    let mut h = linked();
    h.state_mut().run("image.imageSize", json!({"width": 320, "height": 180})).unwrap();
    h.run_steps(2);
    assert!(h.state().ui.view.linked_navigation.is_none());
    // A third document.
    let mut h = linked();
    h.state_mut().run("file.new", json!({"width": 640, "height": 360})).unwrap();
    h.state_mut().sync_views();
    h.run_steps(2);
    assert!(h.state().ui.view.linked_navigation.is_none());
    // Another arrangement, tabs, or a floating window.
    for id in ["window.arrange.twoUpHorizontal", "window.arrange.consolidateAllToTabs", "window.arrange.newWindowForDocument"] {
        let mut h = linked();
        invoke(&mut h, id, json!({})).unwrap();
        assert!(h.state().ui.view.linked_navigation.is_none(), "{id} unlinks");
        assert_eq!(crate::view_cmds::checked(h.state(), LINK), Some(false));
    }
}

#[test]
fn different_resolutions_are_refused_not_scaled() {
    let mut h = harness([1920, 1080], [1280, 720]);
    assert_eq!(crate::view_cmds::is_enabled(h.state(), LINK), Some(false));
    let err = invoke(&mut h, LINK, json!({})).unwrap_err();
    assert!(err.contains("same pixel size"), "{err}");
    assert!(h.state().ui.view.linked_navigation.is_none());
    assert_eq!(h.state().ui.view.arrange, "tabs", "a refused link changes nothing");
    assert!(invoke(&mut h, LINK, json!({"on": "yes"})).is_err(), "bad params are an error, not a panic");
}
