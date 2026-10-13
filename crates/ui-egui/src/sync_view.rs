//! Sync View (the options bar's two-frames button, Window › Arrange › Sync View): one click opens a
//! second native window on the active document and keeps both cameras (zoom, centre, rotation)
//! in step, either way. It is the same live document, so painting and layer edits show in both;
//! nothing is duplicated. A second click, or closing that window, ends it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::state::View;

/// The synced window and the cameras (zoom, centre x/y, rotation) each view had after the last
/// sync. They are kept apart because the second window can clamp or fit the camera to its own
/// size; that local adjustment must not travel back and move the main view.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncView {
    pub window: u64,
    pub main: [f32; 4],
    pub win: [f32; 4],
    /// The pointer was over the second window this frame: a change of its camera is the user's
    /// navigation there, so it drives the main view. Set by `canvas::extra_windows`.
    #[serde(skip)]
    pub window_input: bool,
}

pub const COMMAND: &str = "window.arrange.syncView";

fn camera(v: &View) -> [f32; 4] {
    [v.zoom, v.center[0], v.center[1], v.rotation]
}

fn set_camera(v: &mut View, c: [f32; 4]) {
    let [zoom, x, y, rotation] = c;
    v.zoom = zoom;
    v.center = [x, y];
    v.rotation = rotation;
    v.fit_pending = false;
    v.fill_pending = false;
}

pub fn is_on(app: &PhotocraftApp) -> bool {
    app.ui.view.sync_view.is_some()
}

/// The synced window, if Sync View is on (`ui.windows` id).
pub fn window(app: &PhotocraftApp) -> Option<u64> {
    app.ui.view.sync_view.map(|s| s.window)
}

/// Toggle Sync View (`{"on": bool?}`, default: toggle). On: a second window on the active document,
/// starting on its camera. Off: close only that window; the document is untouched.
pub fn command(app: &mut PhotocraftApp, p: &Value) -> Result<Value, String> {
    let on = match p.get("on") {
        None | Some(Value::Null) => !is_on(app),
        Some(v) => v.as_bool().ok_or("on must be a boolean")?,
    };
    if !on {
        stop(app);
        return Ok(json!({"syncView": false}));
    }
    if let Some(w) = window(app) {
        return Ok(json!({"syncView": true, "window": w}));
    }
    let doc = app.session.active_index().ok_or("no document")?;
    let start = app.ui.views.get(doc).map(camera).ok_or("no document")?;
    let wid = crate::view_cmds::float_window(app, doc, 0);
    if let Some(w) = app.ui.windows.iter_mut().find(|w| w.id == wid) {
        set_camera(&mut w.view, start);
    }
    app.ui.view.sync_view = Some(SyncView { window: wid, main: start, win: start, window_input: false });
    Ok(json!({"syncView": true, "window": wid}))
}

fn stop(app: &mut PhotocraftApp) {
    if let Some(s) = app.ui.view.sync_view.take() {
        app.ui.windows.retain(|w| w.id != s.window);
    }
}

/// Copy whichever camera moved since the last call to the other view (the main view wins if both
/// did). The second window's camera drives the main view only when the user navigated there
/// (`window_input`); its own clamping or fitting to a smaller window stays local, so the two
/// views can never push each other back and forth. Turns Sync View off once its window is gone
/// (closed by hand, its document closed, or Consolidate All to Tabs). Returns true when the main
/// view changed, so it needs a repaint.
pub fn tick(app: &mut PhotocraftApp) -> bool {
    let Some(mut s) = app.ui.view.sync_view else { return false };
    let Some((doc, win)) = app.ui.windows.iter().find(|w| w.id == s.window && w.open).map(|w| (w.document, camera(&w.view))) else {
        app.ui.view.sync_view = None;
        return false;
    };
    let Some(main) = app.ui.views.get(doc).map(camera) else {
        stop(app);
        return false;
    };
    let mut main_moved = false;
    if main != s.main {
        // The main view moved: the second window follows.
        if let Some(w) = app.ui.windows.iter_mut().find(|w| w.id == s.window) {
            set_camera(&mut w.view, main);
        }
        (s.main, s.win) = (main, main);
    } else if win != s.win {
        if s.window_input {
            // Navigation in the second window: the main view follows.
            if let Some(v) = app.ui.views.get_mut(doc) {
                set_camera(v, win);
            }
            (s.main, s.win) = (win, win);
            main_moved = true;
        } else {
            // The window adjusted its own camera (clamping to its size): keep it there.
            s.win = win;
        }
    }
    s.window_input = false;
    app.ui.view.sync_view = Some(s);
    main_moved
}

/// Note that the pointer is over the second window this frame (`canvas::extra_windows`).
pub(crate) fn note_window_input(app: &mut PhotocraftApp, window: u64, input: bool) {
    if let Some(s) = app.ui.view.sync_view.as_mut().filter(|s| s.window == window) {
        s.window_input |= input;
    }
}

pub const TOOLTIP: &str = "Sync View — Open a synchronized window";

/// The options bar button, at the end of every tool's options: disabled without a document,
/// highlighted while syncing. Sized like the bar's other icon toggles (24 px), with a slightly
/// larger glyph so its two frames and arrows stay readable.
pub fn button(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let doc = app.session.active().is_some();
    let on = is_on(app);
    // A crowded bar (a narrow window, a tool with many options) leaves it out rather than make the
    // bar scroll; it is also Window › Arrange › Sync View. While on it always shows, to turn it off.
    if !on && ui.cursor().min.x + 30.0 > ui.clip_rect().right() {
        return;
    }
    let tip = crate::shortcuts::tip_label(app, TOOLTIP, COMMAND);
    ui.add_space(2.0);
    let clicked = ui.add_enabled_ui(doc || on, |ui| crate::icons::button_with_icon_size(ui, "sync-view", 24.0, 16.0, on, &tip)).inner.clicked();
    if clicked && let Err(e) = crate::menus::invoke(app, ui.ctx(), COMMAND, json!({})) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

#[cfg(test)]
#[path = "sync_view_tests.rs"]
mod tests;
