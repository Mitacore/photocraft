use std::sync::Arc;

use serde_json::json;

use super::*;

fn setup(docs: usize) -> (PhotocraftApp, egui::Context) {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    for i in 0..docs {
        app.run("file.new", json!({"width": 640, "height": 360, "name": format!("doc{i}")})).unwrap();
    }
    app.sync_views();
    (app, egui::Context::default())
}

fn click(app: &mut PhotocraftApp, ctx: &egui::Context) -> Result<Value, String> {
    crate::menus::invoke(app, ctx, COMMAND, json!({}))
}

fn synced(app: &PhotocraftApp) -> &crate::state::DocWindow {
    let id = window(app).expect("Sync View on");
    app.ui.windows.iter().find(|w| w.id == id).expect("its window")
}

#[test]
fn the_button_needs_a_document_and_shows_its_state() {
    let (mut app, ctx) = setup(0);
    assert!(crate::icons::exists("sync-view"));
    assert!(crate::menus::is_live(COMMAND));
    assert_eq!(crate::view_cmds::is_enabled(&app, COMMAND), Some(false), "no document: disabled");
    assert!(click(&mut app, &ctx).is_err());
    assert!(!is_on(&app) && app.ui.windows.is_empty());
    app.run("file.new", json!({"width": 64, "height": 64})).unwrap();
    app.sync_views();
    assert_eq!(crate::view_cmds::is_enabled(&app, COMMAND), Some(true));
    assert_eq!(crate::view_cmds::checked(&app, COMMAND), Some(false));
    click(&mut app, &ctx).unwrap();
    assert_eq!(crate::view_cmds::checked(&app, COMMAND), Some(true), "active state while syncing");
    let items = crate::menus::menu_items(&app);
    let item = items.iter().find(|i| i.id == COMMAND).unwrap();
    assert_eq!((item.label.as_str(), item.checked), ("Sync View", Some(true)));
    for lang in crate::i18n::Lang::all().filter(|l| l.complete_menus()) {
        for text in ["Sync View", TOOLTIP] {
            assert!(crate::i18n::has(lang, text), "{}: untranslated {text}", lang.code());
        }
    }
    // The old reference/drawing pairing is no longer presented under its NAFY name.
    assert!(!items.iter().any(|i| i.label.contains("NAFY") || i.id == "window.arrange.linkNavigation"));
}

#[test]
fn one_click_opens_a_second_window_on_the_same_live_document() {
    let (mut app, ctx) = setup(2);
    app.session.set_active(0);
    let doc = app.session.documents()[0].doc.clone();
    let layers = doc.layers.len();
    app.ui.views[0].zoom = 1.5;
    app.ui.views[0].center = [100.0, 50.0];
    app.ui.views[0].rotation = 12.0;
    click(&mut app, &ctx).unwrap();
    assert!(is_on(&app));
    assert_eq!(app.ui.windows.len(), 1, "exactly one extra window");
    let w = synced(&app);
    assert_eq!(w.document, 0, "the active document, not another one");
    assert_eq!((w.view.zoom, w.view.center, w.view.rotation), (1.5, [100.0, 50.0], 12.0), "it opens on the same camera");
    assert!(!w.view.fit_pending);
    // No new document, file or layer: the window shows the session's own document.
    assert_eq!(app.session.documents().len(), 2);
    assert!(Arc::ptr_eq(&doc, &app.session.documents()[0].doc));
    assert_eq!(app.session.documents()[0].doc.layers.len(), layers);
    // Painting lands in that one document, which both windows draw.
    app.run("select.all", json!({})).unwrap();
    app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
    assert_eq!(app.session.documents().len(), 2);
    assert_eq!(synced(&app).document, 0);
    assert!(!Arc::ptr_eq(&doc, &app.session.documents()[0].doc), "the edit is in the shared document");
    assert!(is_on(&app), "editing keeps syncing");
}

#[test]
fn zoom_pan_and_rotation_follow_both_ways() {
    let (mut app, ctx) = setup(1);
    click(&mut app, &ctx).unwrap();
    // Main window → second window.
    app.ui.views[0].zoom = 2.0;
    app.ui.views[0].center = [10.0, 20.0];
    app.ui.views[0].rotation = 28.0;
    assert!(!tick(&mut app), "the main view didn't need a change");
    let w = synced(&app).view.clone();
    assert_eq!((w.zoom, w.center, w.rotation), (2.0, [10.0, 20.0], 28.0));
    // Second window → main window, when the user navigated there (pointer over it).
    let id = window(&app).unwrap();
    if let Some(w) = app.ui.windows.iter_mut().find(|w| w.id == id) {
        w.view.zoom = 0.5;
        w.view.center = [300.0, 200.0];
        w.view.rotation = -45.0;
    }
    note_window_input(&mut app, id, true);
    assert!(tick(&mut app), "the main view moved: repaint");
    let v = &app.ui.views[0];
    assert_eq!((v.zoom, v.center, v.rotation), (0.5, [300.0, 200.0], -45.0));
    // The window clamping its own camera (no pointer there) stays local: no feedback loop.
    if let Some(w) = app.ui.windows.iter_mut().find(|w| w.id == id) {
        w.view.center = [310.0, 190.0];
    }
    assert!(!tick(&mut app));
    assert_eq!(app.ui.views[0].center, [300.0, 200.0], "the main view stays put");
    assert_eq!(synced(&app).view.center, [310.0, 190.0], "the window keeps its own adjustment");
    assert!(!tick(&mut app), "and nothing bounces back on the next frame");
    // The main view moving again re-syncs the window.
    app.ui.views[0].center = [305.0, 205.0];
    tick(&mut app);
    assert_eq!(synced(&app).view.center, [305.0, 205.0]);
    // Both moved in the same frame: the main window wins, and both end equal.
    app.ui.views[0].zoom = 3.0;
    if let Some(w) = app.ui.windows.iter_mut().find(|w| w.id == id) {
        w.view.zoom = 4.0;
    }
    note_window_input(&mut app, id, true);
    tick(&mut app);
    assert_eq!((app.ui.views[0].zoom, synced(&app).view.zoom), (3.0, 3.0));
    // Nothing moved: nothing changes.
    let before = (app.ui.views[0].clone(), synced(&app).view.clone());
    assert!(!tick(&mut app));
    assert_eq!((app.ui.views[0].clone(), synced(&app).view.clone()), before);
}

#[test]
fn clicking_again_closes_only_the_second_window() {
    let (mut app, ctx) = setup(1);
    let other = crate::view_cmds::float_window(&mut app, 0, 1);
    click(&mut app, &ctx).unwrap();
    assert_eq!(app.ui.windows.len(), 2);
    let doc = app.session.documents()[0].doc.clone();
    click(&mut app, &ctx).unwrap();
    assert!(!is_on(&app));
    assert_eq!(crate::view_cmds::checked(&app, COMMAND), Some(false), "the button resets");
    assert_eq!(app.ui.windows.iter().map(|w| w.id).collect::<Vec<_>>(), [other], "another window stays open");
    assert_eq!(app.session.documents().len(), 1);
    assert!(Arc::ptr_eq(&doc, &app.session.documents()[0].doc), "the document is untouched");
    // Explicit on/off, and bad params.
    crate::menus::invoke(&mut app, &ctx, COMMAND, json!({"on": true})).unwrap();
    crate::menus::invoke(&mut app, &ctx, COMMAND, json!({"on": true})).unwrap();
    assert_eq!(app.ui.windows.len(), 2, "on while on opens no second copy");
    crate::menus::invoke(&mut app, &ctx, COMMAND, json!({"on": false})).unwrap();
    assert!(!is_on(&app));
    assert!(crate::menus::invoke(&mut app, &ctx, COMMAND, json!({"on": "yes"})).is_err());
}

/// The whole app, drawn: the button ends the tool options bar (right of the Brush's painting
/// symmetry button, in the same row), for any tool, and is disabled with no document.
#[test]
fn the_button_sits_at_the_end_of_the_options_bar() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 900.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.prefs.edit(|p| p.interface.language = "en".into());
        app.ui.tool = crate::state::Tool::Brush;
        app
    });
    h.run_steps(4);
    assert!(h.get_by_label(TOOLTIP).accesskit_node().is_disabled(), "no document: disabled");
    h.state_mut().run("file.new", json!({"width": 640, "height": 360})).unwrap();
    h.state_mut().sync_views();
    h.run_steps(4);
    let sync = h.get_by_label(TOOLTIP);
    assert!(!sync.accesskit_node().is_disabled());
    let (sync, symmetry) = (sync.rect(), h.get_by_label("Set painting symmetry options").rect());
    assert!(sync.left() >= symmetry.right(), "right of the symmetry button: {sync:?} / {symmetry:?}");
    assert!((sync.center().y - symmetry.center().y).abs() < 2.0, "same row: {sync:?} / {symmetry:?}");
    // The Brush's last control before it is the Smart Shape toggle: Sync View ends the bar.
    let smart = h.get_by_label(crate::smart_shape::TOOLTIP).rect();
    assert!(sync.left() >= smart.right() && sync.left() - smart.right() < 16.0, "right after Smart Shape: {sync:?} / {smart:?}");
    assert_eq!((sync.width().round(), sync.height().round()), (24.0, 24.0), "the bar's icon size");
    // Every tool's bar ends with it (tools whose bars return early included).
    for tool in [crate::state::Tool::Move, crate::state::Tool::Hand, crate::state::Tool::Pen, crate::state::Tool::SpotHealing, crate::state::Tool::Crop] {
        h.state_mut().ui.tool = tool;
        h.run_steps(3);
        assert_eq!(h.query_all_by_label(TOOLTIP).count(), 1, "{tool:?}: one Sync View button");
    }
}

#[test]
fn closing_the_window_or_the_document_turns_sync_off() {
    // The window closed by hand (`extra_windows` drops it).
    let (mut app, ctx) = setup(1);
    click(&mut app, &ctx).unwrap();
    let id = window(&app).unwrap();
    app.ui.windows.retain(|w| w.id != id);
    tick(&mut app);
    assert!(!is_on(&app));
    assert_eq!(crate::view_cmds::checked(&app, COMMAND), Some(false));
    assert_eq!(app.session.documents().len(), 1, "the document stays open");
    // The document closed: its window goes, and Sync View with it.
    let (mut app, ctx) = setup(2);
    click(&mut app, &ctx).unwrap();
    app.run("file.close", json!({})).unwrap();
    tick(&mut app);
    assert!(!is_on(&app) && app.ui.windows.is_empty());
    // Consolidate All to Tabs closes every floating window.
    let (mut app, ctx) = setup(1);
    click(&mut app, &ctx).unwrap();
    crate::menus::invoke(&mut app, &ctx, "window.arrange.consolidateAllToTabs", json!({})).unwrap();
    tick(&mut app);
    assert!(!is_on(&app));
}
