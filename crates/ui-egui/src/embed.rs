//! Hosting the editor inside another application's canvas.
//!
//! A host that owns the screen (the Creative Universe workspace) wants the editor in two parts: the *tool panels* (menus,
//! options bar, toolbox, dock) as floating panels beside a document, and the *document canvas* drawn where the document
//! already is on the host's own canvas, under the host's camera. [`PhotocraftApp::ui_embedded`] does that with the same
//! frame as [`eframe::App::ui`]: one lifecycle pass (jobs, shortcuts, dialogs, windows), the same panel functions, the same
//! `canvas_view`. Only the layout differs: each panel is laid out inside the rect the host gives it, and the canvas camera is
//! imposed by the host (document point (0, 0) at [`EmbedLayout::origin`], [`EmbedLayout::scale`] screen points per
//! document pixel) instead of coming from the editor's own pan and zoom.
//!
//! A camera change the editor itself asks for (View › Fit on Screen, the Navigator, a zoom shortcut) is not applied: it comes
//! back in [`EmbedOutcome::camera`] so the host can move its own camera, and the next frame imposes it.

use egui::{Pos2, Rect, Ui};

use crate::{PhotocraftApp, canvas, panels};

/// Where each tool panel goes, in the host's screen points. A `None` panel is not drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmbedPanels {
    pub menu: Option<Rect>,
    pub options: Option<Rect>,
    pub toolbox: Option<Rect>,
    pub dock: Option<Rect>,
}

/// The editor's layout for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbedLayout {
    pub panels: EmbedPanels,
    /// Where the document canvas is drawn (the host's view of the document, clipped to what is visible).
    pub canvas: Rect,
    /// Screen position of document point (0, 0).
    pub origin: Pos2,
    /// Screen points per document pixel.
    pub scale: f32,
}

/// A camera, in the terms of [`EmbedLayout`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmbedCamera {
    pub origin: Pos2,
    pub scale: f32,
}

/// What the editor wants back from the host after a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EmbedOutcome {
    /// The editor changed its own camera this frame: the host should adopt it.
    pub camera: Option<EmbedCamera>,
}

/// How big each panel likes to be, in points: the menu bar's and options bar's height, the toolbox's and the dock's width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmbedSizes {
    pub menu_height: f32,
    pub options_height: f32,
    pub toolbox_width: f32,
    pub dock_width: f32,
}

impl PhotocraftApp {
    /// One frame in the host's layout. Call it instead of `ui`, after `logic`, once per frame.
    pub fn ui_embedded(&mut self, ui: &mut Ui, frame: &mut eframe::Frame, layout: &EmbedLayout) -> EmbedOutcome {
        self.frame_ui(ui, frame, Some(layout))
    }

    /// Preferred panel sizes under the current theme and preferences.
    pub fn embed_sizes(&self, ctx: &egui::Context) -> EmbedSizes {
        let t = crate::theme::Tokens::get(ctx);
        let narrow = self.session.prefs().workspace.narrow_options_bar();
        let (w1, bx) = if t.pro { (40.0, 30.0) } else { (50.0, 36.0) };
        EmbedSizes {
            menu_height: crate::titlebar::bar_height(self, ctx, if t.pro { 32.0 } else { 38.0 }),
            options_height: panels::options_bar_height(t.pro, narrow),
            toolbox_width: if self.ui.panels.toolbar_double { w1 + bx + 2.0 } else { w1 },
            dock_width: if t.pro { 290.0 } else { 300.0 } + 44.0,
        }
    }
}

/// Lay `add` out inside `rect`, never drawing outside it.
fn in_rect(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect);
        add(ui);
    });
}

pub(crate) fn show(app: &mut PhotocraftApp, ui: &mut Ui, layout: &EmbedLayout) -> EmbedOutcome {
    if !app.ui.view.hides_chrome() {
        if let Some(r) = layout.panels.menu.filter(|_| app.ui.panels.menu_bar) {
            in_rect(ui, r, |ui| panels::title_bar(app, ui));
        }
        if let Some(r) = layout.panels.options.filter(|_| app.ui.panels.options_bar) {
            in_rect(ui, r, |ui| panels::options_bar(app, ui));
        }
        if let Some(r) = layout.panels.toolbox.filter(|_| app.ui.panels.toolbar) {
            in_rect(ui, r, |ui| panels::toolbar(app, ui));
        }
        if let Some(r) = layout.panels.dock.filter(|_| app.ui.panels.dock) {
            in_rect(ui, r, |ui| panels::right_dock(app, ui));
        }
    }
    canvas::embedded(app, ui, layout)
}
