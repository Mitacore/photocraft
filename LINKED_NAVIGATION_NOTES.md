# Sync View and linked reference/drawing navigation: implementation notes

Base: `storytold/photocraft` main @ `6f85054288a564f9fbbccb99268ffdb34e581584`.
Started from the starter patch `photocraft-linked-navigation.patch`, applied by hand and then reworked.

## Sync View = linked reference/drawing navigation (the original NAFY, commit 1cab8a92)
- Shown as the two-frames button at the right end of the tool options bar and as **Window › Arrange › Sync View**.
  Both run `window.arrange.linkNavigation`, the original NAFY command. Tooltip: "Sync View — Link the zoom, pan and
  rotation of two open documents".
- The detached-window implementation (commits e52b43ba–5cc9f2ff: a second native window on the same document,
  `sync_view::tick`, the `extra_windows` hooks, `window.arrange.syncView`) was removed. `extra_windows` is upstream's
  again, and `sync_view.rs` now holds only the button.
- Since this restoration, linking keeps the drawing's camera (the original re-fitted it), so there is no automatic
  zoom or pan.
- When the link ends by itself because the documents changed (a third one opened, such as Smart Object contents, or
  one closed or resized), the arrangement from before linking (usually tabs) comes back, so every document is on its
  own tab. Turning Sync View off by hand keeps the 2-up layout.
- `window.arrange.linkNavigation` is off by default. It is enabled only when exactly 2
  documents are open and they have the same pixel size. Different sizes are refused and never scaled.
- When on, the layout switches to 2-up vertical. The document that was active when you linked becomes the **drawing**
  (right pane, editable). The other document becomes the **reference** (left pane, view only).
- The drawing's zoom, centre and rotation are copied to the reference every frame, in the same frame. Navigating over
  the reference (wheel, Hand, scroll bars) moves the drawing, and both panes follow on the next frame.
- The pair is held by `DocId`, not by tab index. A tab click, Ctrl+Tab, Window menu items, automation or a tab
  reorder can't make the reference editable: the drawing is put back as the active document.
- The link turns off by itself when: either document closes, a 3rd document opens, a size changes, another
  arrangement or tabs is chosen, or a floating document window opens.

## Files changed vs upstream
| File | Change |
|---|---|
| `crates/ui-egui/src/view_cmds.rs` | `LinkedNavigation` state, `linked_pair`, `enforce_linked_navigation`, command, enabled/checked |
| `crates/ui-egui/src/canvas.rs` | reference left / drawing right, live camera copy, reference never activates |
| `crates/ui-egui/src/menus.rs` | menu entry (shell command outside Photoshop's catalogue) + enforce after every command |
| `crates/ui-egui/src/lib.rs` | enforce after `PhotocraftApp::run` |
| `crates/ui-egui/src/i18n/*.tsv` | label translated for every catalogue (required by the coverage test) |
| `crates/ui-egui/src/view_cmds/tests.rs` | unit test |
| `crates/ui-egui/src/canvas/linked_nav_tests.rs` | whole-app tests: zoom/pan/rotation, wheel, Hand drag, clicks, Ctrl+Tab, close/resize/rearrange, sizes, no history/pixel changes |
| `.github/workflows/linked-navigation-windows.yml` | checks + unsigned portable Windows x64 zip artifact |

## Differences from the starter patch
- The patch picked the reference as `(active + 1) % 2` on every frame, so a tab click or Ctrl+Tab swapped the roles.
  The pair is now stored by document id and enforced after every command and every frame.
- The menu item is in `menus::UI_COMMANDS` and not in `menu_catalog.rs`. That catalogue is Photoshop's menu tree,
  and it drives the parity numbers.
- Translations were added, because `complete_languages_translate_every_menu_string` fails without them.
- Navigating the reference now moves the pair. In the patch, it was silently discarded.

## Sync View button
- **Behaviour:** enabled with two open documents of the same size, and also while on, so it can be turned off. It
  is highlighted while on.
- **Space:** a full bar (a narrow window, or the Crop tool) leaves the button out rather than scroll. The command
  stays in the menu.
- **Files:** `sync_view.rs`, `sync_view_tests.rs`, `assets/icons/sync-view.svg` (original, listed in
  ATTRIBUTION.md), `icon_data.rs`, `panels.rs`, `menus.rs`, `i18n/*.tsv`.
- **Tests for tabs and Smart Object contents** (`doc_tabs_tests.rs`):
  - Zoom, paint and Undo go to the active tab only.
  - Edit Contents opens one tab: no duplicate, no OS window. Undo and Redo stay in the contents.
  - Save Contents updates the parent as one step, and closing the contents keeps the parent.
  - Opening contents while Sync View is on ends the link and returns to tabs.
- **Fast Preview:** `.github/workflows/fast-preview-windows.yml` runs on every push. It builds a `dev`-profile exe as
  the artifact `PhotoCraft-FastPreview-Win64`, plus a parallel job with fmt and the focused tests. The full checks
  and release zip workflow is manual only now.

## Smart Shape (Brush and Pencil options bar, off by default)
- **Behaviour:** draw, then keep the pointer still for the hold time and the stroke becomes a line, curve, circle,
  ellipse, rectangle or square.
  - The hold time is 700 ms by default. Right-click the button to set it from 300 to 2000 ms.
  - The pointer counts as still within 6 screen points.
  - While still held, dragging adjusts the shape. A line or curve moves its end. A closed shape scales and turns
    about its centre (a circle only scales).
  - Releasing commits one `paint.stroke`, which is one Undo step. Releasing early keeps the freehand stroke as
    drawn.
  - Esc while a shape shows puts the freehand stroke back.
  - ⇧ lines and ⇧-click connections are left alone.
- **Integration:**
  - `Drag::smart` holds the hold state.
  - `tool_move` passes moves to the shape once one is showing. The Up path doesn't add the freehand release point.
  - `feed_live_stroke` restarts the live preview from the document when the points are replaced, so the freehand
    marks vanish instead of staying under the shape.
  - `stroke_params` sends `"smoothing": 0` for a shape. The brush setting itself is untouched.
  - Pen pressure, tilt and rotation map from the freehand stroke by arc-length fraction.
  - The per-frame `tick` in `canvas_view` wakes the UI with `request_repaint_after` when the hold is due.
- **Recognition** (`smart_shape.rs`) is clean-room, from published methods. No third-party code is used and no
  dependency was added. The stroke is resampled to 64 points by arc length.
  - **Line:** principal-axis fit. The spread must be ≤ 3 % of the extent, and the stroke must not double back.
  - **Curve:** cubic Bézier least squares with Newton reparameterisation (Schneider, Graphics Gems 1990). It needs
    no corners and at most 270° of turning.
  - **Circle and ellipse:** principal axes plus extents, accepted on the mean radial error.
  - **Rectangle and square:** 4 corners found as local maxima of the windowed turning angle. The orientation is
    averaged modulo 90°. Each side is the mean of the outline points nearest to it.
  - **Size limits:** strokes under 16 screen points, and anything that fits no shape well enough (handwriting,
    zig-zags, L shapes, triangles, spirals), stay freehand.
- **References and licences checked:**
  - Clip Studio Paint Smart Shape and Procreate QuickShape help pages: behaviour only.
  - CharlesCheval/supernote-snap: **CC BY-NC-SA 4.0**. Incompatible with MIT/Apache, so it is a behaviour
    reference only and no code was taken.
  - sibaiper/perfect_freehand (MIT, a Rust port of perfect-freehand): it builds a stroke outline polygon. That is
    not shape recognition, and it doesn't fit PhotoCraft's dab-based brush engine, so it is not used.
  - linebender/kurbo (Apache-2.0/MIT, already in Cargo.lock transitively) has curve fitting. A plain cubic fit was
    small enough to write directly, so it is not added as a dependency.
- **Files:** `smart_shape.rs`, `smart_shape_tests.rs`, `assets/icons/smart-shape.svg` (original, listed in
  ATTRIBUTION.md), `canvas.rs`, `state.rs` (`smart_shape`, `smart_shape_hold_ms`), `panels.rs`, `i18n/*.tsv`.

## channel_view fix (upstream flaky test)
`channel.setVisible` bumps the revision without changing pixels, so `channel_view::ensure` threw away its sampled
composite on every channel toggle. Upstream, `channel_eyes_reuse_composite_until_pixels_change` failed 4 of 30 runs
alone on Windows. The committed snapshot is now held weakly and reused, and the test passed 30 of 30 runs alone and
8 of 8 runs in the full suite.

## Known limits (v1)
- With rulers on, the drawing pane is smaller by the ruler width, so the two panes are offset by that many points.
  The same document point stays at each pane's centre.
- Only one pair, and only equal sizes.
- Not verified locally: this machine has no Rust toolchain. Compilation and tests run in the Windows-only workflow.
