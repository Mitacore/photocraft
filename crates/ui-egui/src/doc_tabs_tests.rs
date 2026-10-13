//! Document tabs and Smart Object contents: the active tab is the document that takes zoom, paint
//! and Undo; Edit Contents opens the contents as its own tab (never a second copy, never an OS
//! window); switching between parent and contents, saving and closing keep the two apart.

use std::sync::Arc;

use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, tool_event};
use crate::state::Tool;

fn app_with(n: usize, w: u32, h: u32) -> (PhotocraftApp, egui::Context) {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    for i in 0..n {
        app.run("file.new", json!({"width": w, "height": h, "name": format!("doc{i}"), "background": "transparent"})).unwrap();
    }
    app.sync_views();
    app.run("tools.setBrush", json!({"brush": {"size": 4, "hardness": 1.0}})).unwrap();
    app.ui.tool = Tool::Brush;
    (app, egui::Context::default())
}

/// A short Brush stroke on the active document.
fn paint(app: &mut PhotocraftApp, y: f64) {
    let none = egui::Modifiers::NONE;
    tool_event(app, ToolEvent::Down { x: 10.0, y, pressure: 1.0 }, none);
    tool_event(app, ToolEvent::Move { x: 50.0, y, pressure: 1.0 }, none);
    tool_event(app, ToolEvent::Up { x: 50.0, y }, none);
}

fn docs(app: &PhotocraftApp) -> Vec<Arc<photocraft_doc::Document>> {
    app.session.documents().iter().map(|d| d.doc.clone()).collect()
}

fn first_pixel(doc: &photocraft_doc::Document) -> [f32; 4] {
    photocraft_compose::flatten(doc).px[0]
}

#[test]
fn switching_tabs_sends_zoom_paint_and_undo_to_the_active_document_only() {
    let (mut app, ctx) = app_with(3, 120, 120);
    for v in &mut app.ui.views {
        v.zoom = 0.5;
    }
    for (step, i) in [0usize, 2, 1, 0, 2, 1].into_iter().enumerate() {
        assert!(app.session.set_active(i));
        // Zoom: only the active document's view.
        let zooms: Vec<f32> = app.ui.views.iter().map(|v| v.zoom).collect();
        crate::menus::invoke(&mut app, &ctx, "view.twoHundredPercent", json!({})).unwrap();
        for (k, v) in app.ui.views.iter().enumerate() {
            let want = if k == i { 2.0 } else { zooms[k] };
            assert_eq!(v.zoom, want, "step {step}: view {k}");
        }
        // Paint: only the active document.
        let before = docs(&app);
        paint(&mut app, 20.0 + 10.0 * step as f64);
        let after = docs(&app);
        for (k, (b, a)) in before.iter().zip(&after).enumerate() {
            assert_eq!(Arc::ptr_eq(b, a), k != i, "step {step}: document {k}");
        }
        // Undo: the same document, nothing else.
        assert!(app.session.undo());
        for (k, (a, u)) in after.iter().zip(docs(&app)).enumerate() {
            assert!(k == i || Arc::ptr_eq(a, &u), "step {step}: undo touched document {k}");
        }
        assert!(app.session.redo());
    }
    assert_eq!(app.session.documents().len(), 3, "no extra document");
    assert!(app.ui.windows.is_empty(), "no OS window");
}

#[test]
fn smart_object_contents_open_switch_save_and_close_as_their_own_tab() {
    let (mut app, _) = app_with(1, 40, 30);
    app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
    let smart = app.run("layer.smartObjects.convertToSmartObject", json!({})).unwrap()["layer"].as_u64().unwrap();
    let parent_id = app.session.active().unwrap().doc.id;
    // Open the contents: a second tab, active, in the document area.
    let r = app.run("layer.smartObjects.editContents", json!({"layer": smart})).unwrap();
    assert_eq!(r["document"], 1);
    assert_eq!((app.session.documents().len(), app.session.active_index()), (2, Some(1)));
    assert!(app.ui.windows.is_empty(), "no OS window");
    // To the parent and back (Edit Contents again switches to the open tab, no second copy).
    assert!(app.session.set_active(0));
    assert_eq!(app.session.active().unwrap().doc.id, parent_id);
    let r = app.run("layer.smartObjects.editContents", json!({"layer": smart})).unwrap();
    assert_eq!(r["document"], 1);
    assert_eq!((app.session.documents().len(), app.session.active_index()), (2, Some(1)), "no duplicate tab");
    // Edit the contents; Undo and Redo go to the contents, never the parent.
    app.run("edit.fill", json!({"color": "#0000ff"})).unwrap();
    let parent = app.session.documents()[0].doc.clone();
    assert!(app.session.undo());
    assert_eq!(first_pixel(&app.session.documents()[1].doc), [1.0, 0.0, 0.0, 1.0], "undo in the contents");
    assert!(app.session.redo());
    assert_eq!(first_pixel(&app.session.documents()[1].doc), [0.0, 0.0, 1.0, 1.0]);
    assert!(Arc::ptr_eq(&parent, &app.session.documents()[0].doc), "the parent is untouched until saved");
    // Save the contents: the parent updates, as one step in the parent.
    let parent_steps = app.session.documents()[0].history.past_len();
    app.run("layer.smartObjects.saveContents", json!({})).unwrap();
    assert_eq!(app.session.documents()[0].history.past_len(), parent_steps + 1);
    assert_eq!(first_pixel(&app.session.documents()[0].doc), [0.0, 0.0, 1.0, 1.0], "the parent shows the saved contents");
    assert_eq!(app.session.active_index(), Some(1), "saving keeps the contents tab active");
    // Close the contents tab: the parent stays open and becomes active.
    app.run("file.close", json!({"document": 1})).unwrap();
    assert_eq!(app.session.documents().len(), 1);
    assert_eq!(app.session.active().unwrap().doc.id, parent_id);
    assert_eq!(first_pixel(&app.session.active().unwrap().doc), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn opening_smart_object_contents_ends_sync_view_and_gives_them_their_own_tab() {
    let (mut app, ctx) = app_with(2, 40, 30);
    app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
    let smart = app.run("layer.smartObjects.convertToSmartObject", json!({})).unwrap()["layer"].as_u64().unwrap();
    crate::menus::invoke(&mut app, &ctx, crate::sync_view::COMMAND, json!({})).unwrap();
    assert!(crate::sync_view::is_on(&app));
    assert_eq!(app.ui.view.arrange, "twoUpVertical");
    let before = docs(&app);
    app.run("layer.smartObjects.editContents", json!({"layer": smart})).unwrap();
    assert!(!crate::sync_view::is_on(&app), "a third document ends linked navigation");
    assert_eq!(app.ui.view.arrange, "tabs", "back to tabs, as before linking");
    assert_eq!((app.session.documents().len(), app.session.active_index()), (3, Some(2)), "the contents are the active tab");
    assert!(app.ui.windows.is_empty(), "no OS window");
    for (k, d) in before.iter().enumerate() {
        assert!(Arc::ptr_eq(d, &app.session.documents()[k].doc), "opening the contents changes no document ({k})");
    }
}
