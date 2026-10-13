use std::sync::Arc;

use serde_json::json;

use super::*;

/// An app with `sizes.len()` documents of those sizes (the last one active).
fn setup(sizes: &[[u32; 2]]) -> (PhotocraftApp, egui::Context) {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    for (i, [w, h]) in sizes.iter().enumerate() {
        app.run("file.new", json!({"width": w, "height": h, "name": format!("doc{i}"), "background": "white"})).unwrap();
    }
    app.sync_views();
    (app, egui::Context::default())
}

fn click(app: &mut PhotocraftApp, ctx: &egui::Context) -> Result<serde_json::Value, String> {
    crate::menus::invoke(app, ctx, COMMAND, json!({}))
}

fn enabled(app: &PhotocraftApp) -> bool {
    crate::view_cmds::is_enabled(app, COMMAND).unwrap_or(false)
}

#[test]
fn the_button_needs_two_documents_of_the_same_size() {
    assert!(crate::icons::exists("sync-view"));
    assert!(crate::menus::is_live(COMMAND));
    for sizes in [&[][..], &[[640, 360]][..], &[[640, 360], [800, 600]][..], &[[640, 360], [640, 360], [640, 360]][..]] {
        let (mut app, ctx) = setup(sizes);
        assert!(!enabled(&app), "{} document(s): disabled", sizes.len());
        assert!(click(&mut app, &ctx).is_err(), "refused, nothing scaled");
        assert!(!is_on(&app) && app.ui.windows.is_empty());
    }
    let (app, _) = setup(&[[640, 360], [640, 360]]);
    assert!(enabled(&app));
    // The menu item: Window › Arrange › Sync View, no NAFY name anywhere.
    let items = crate::menus::menu_items(&app);
    let item = items.iter().find(|i| i.id == COMMAND).unwrap();
    assert_eq!(item.label, "Sync View");
    assert_eq!(item.path, ["Window", "Arrange"]);
    assert_eq!(item.checked, Some(false));
    assert!(!items.iter().any(|i| i.label.contains("NAFY")), "no NAFY name in the menus");
    assert!(!items.iter().any(|i| i.id == "window.arrange.syncView"), "no second, competing command");
    for lang in crate::i18n::Lang::all().filter(|l| l.complete_menus()) {
        for text in ["Sync View", TOOLTIP] {
            assert!(crate::i18n::has(lang, text), "{}: untranslated {text}", lang.code());
        }
    }
}

#[test]
fn one_click_links_the_two_documents_inside_the_workspace() {
    let (mut app, ctx) = setup(&[[640, 360], [640, 360]]);
    // The active document (the drawing) has its own camera: linking must not zoom or pan it.
    app.ui.views[1].zoom = 1.75;
    app.ui.views[1].center = [200.0, 150.0];
    app.ui.views[1].rotation = 12.0;
    app.ui.views[1].fit_pending = false;
    let docs: Vec<_> = app.session.documents().iter().map(|d| d.doc.clone()).collect();
    click(&mut app, &ctx).unwrap();
    assert!(is_on(&app));
    assert_eq!(crate::view_cmds::checked(&app, COMMAND), Some(true), "active state");
    assert_eq!(app.ui.view.arrange, "twoUpVertical", "side by side in the document area");
    assert!(app.ui.windows.is_empty(), "no extra OS window");
    assert_eq!(app.session.documents().len(), 2, "both documents stay open, nothing duplicated");
    let v = &app.ui.views[1];
    assert_eq!((v.zoom, v.center, v.rotation, v.fit_pending), (1.75, [200.0, 150.0], 12.0, false), "no automatic zoom or pan");
    for (before, now) in docs.iter().zip(app.session.documents()) {
        assert!(Arc::ptr_eq(before, &now.doc), "linking changes no document");
    }
    // Off again: both documents stay, each with its own camera from then on.
    click(&mut app, &ctx).unwrap();
    assert!(!is_on(&app));
    assert_eq!(crate::view_cmds::checked(&app, COMMAND), Some(false));
    assert_eq!(app.session.documents().len(), 2, "turning it off closes nothing");
    // Editing the other document now leaves the first one alone.
    app.session.set_active(0);
    let first = app.session.documents()[1].doc.clone();
    app.run("layer.new.layer", json!({})).unwrap();
    assert_eq!(app.session.documents()[0].doc.layers.len(), 2, "a layer in document A");
    assert!(Arc::ptr_eq(&first, &app.session.documents()[1].doc), "document B unchanged");
    // On again: stable, the drawing is the active document.
    click(&mut app, &ctx).unwrap();
    assert!(is_on(&app));
    let link = app.ui.view.linked_navigation.unwrap();
    assert_eq!(link.drawing, app.session.documents()[0].doc.id);
    assert!(app.ui.windows.is_empty());
}

/// The whole app, drawn: the button ends the tool options bar (right after the Brush's Smart Shape
/// toggle, in the same row), is disabled until two same-size documents are open, and a click turns
/// linked navigation on and off without opening any window.
#[test]
fn the_button_sits_at_the_end_of_the_options_bar() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 900.0)).with_max_steps(64).build_eframe(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.prefs.edit(|p| p.interface.language = "en".into());
        app.ui.tool = crate::state::Tool::Brush;
        app.run("file.new", json!({"width": 640, "height": 360})).unwrap();
        app.sync_views();
        app
    });
    h.run_steps(4);
    assert!(h.get_by_label(TOOLTIP).accesskit_node().is_disabled(), "one document: disabled");
    h.state_mut().run("file.new", json!({"width": 640, "height": 360})).unwrap();
    h.state_mut().sync_views();
    h.run_steps(4);
    let sync = h.get_by_label(TOOLTIP);
    assert!(!sync.accesskit_node().is_disabled(), "two same-size documents: enabled");
    let sync = sync.rect();
    let smart = h.get_by_label(crate::smart_shape::TOOLTIP).rect();
    assert!(sync.left() >= smart.right() && sync.left() - smart.right() < 16.0, "right after Smart Shape: {sync:?} / {smart:?}");
    assert!((sync.center().y - smart.center().y).abs() < 2.0, "same row");
    assert_eq!((sync.width().round(), sync.height().round()), (24.0, 24.0), "the bar's icon size");
    h.get_by_label(TOOLTIP).click();
    h.run_steps(3);
    assert!(is_on(h.state()), "a click links the documents");
    assert!(h.state().ui.windows.is_empty(), "inside the workspace, no OS window");
    h.get_by_label(TOOLTIP).click();
    h.run_steps(3);
    assert!(!is_on(h.state()), "and unlinks them");
    assert_eq!(h.state().session.documents().len(), 2);
    // Every tool's bar with room ends with it (tools whose bars return early included).
    for tool in [crate::state::Tool::Move, crate::state::Tool::Hand, crate::state::Tool::Pen, crate::state::Tool::SpotHealing] {
        h.state_mut().ui.tool = tool;
        h.run_steps(3);
        assert_eq!(h.query_all_by_label(TOOLTIP).count(), 1, "{tool:?}: one Sync View button");
    }
    // A bar already full (the Crop tool's) leaves it out rather than scroll; never twice.
    h.state_mut().ui.tool = crate::state::Tool::Crop;
    h.run_steps(3);
    assert!(h.query_all_by_label(TOOLTIP).count() <= 1, "Crop: at most one Sync View button");
}
