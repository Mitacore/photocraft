//! Sync View: the options bar's two-frames button (and Window › Arrange › Sync View). It turns on
//! PhotoCraft's linked navigation (`window.arrange.linkNavigation`, `view_cmds::link_navigation`):
//! two open documents of the same size side by side in the document area, the reference on the
//! left and the active drawing on the right, sharing zoom, pan and rotation. Only the camera is
//! shared; each document keeps its own layers, pixels and history. No extra OS window is opened.

use serde_json::json;

use crate::PhotocraftApp;

/// The command the button runs: linked navigation.
pub const COMMAND: &str = "window.arrange.linkNavigation";
pub const TOOLTIP: &str = "Sync View — Link the zoom, pan and rotation of two open documents";

/// Is linked navigation on?
pub fn is_on(app: &PhotocraftApp) -> bool {
    app.ui.view.linked_navigation.is_some()
}

/// The options bar button, at the end of every tool's options: enabled with two open documents
/// of the same size (or while on, to turn it off), highlighted while on. Sized like the bar's
/// other icon toggles (24 px), with a slightly larger glyph so its frames and arrows stay readable.
pub fn button(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let on = is_on(app);
    // A crowded bar (a narrow window, a tool with many options) leaves it out rather than make the
    // bar scroll; it is also Window › Arrange › Sync View. While on it always shows, to turn it off.
    if !on && ui.cursor().min.x + 30.0 > ui.clip_rect().right() {
        return;
    }
    let enabled = on || crate::view_cmds::is_enabled(app, COMMAND).unwrap_or(false);
    let tip = crate::shortcuts::tip_label(app, TOOLTIP, COMMAND);
    ui.add_space(2.0);
    let clicked = ui.add_enabled_ui(enabled, |ui| crate::icons::button_with_icon_size(ui, "sync-view", 24.0, 16.0, on, &tip)).inner.clicked();
    if clicked && let Err(e) = crate::menus::invoke(app, ui.ctx(), COMMAND, json!({})) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

#[cfg(test)]
#[path = "sync_view_tests.rs"]
mod tests;
