# NAFY (Linked Reference & Drawing Navigation): implementation notes

Base: `storytold/photocraft` main @ `6f85054288a564f9fbbccb99268ffdb34e581584` (see `UPSTREAM_COMMIT.txt`).
Started from the starter patch `photocraft-linked-navigation.patch`, applied by hand and then reworked.

## What it does
- **Window › Arrange › NAFY (Linked Reference & Drawing Navigation)**: off by default. It is enabled only when exactly 2
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

## Known limits (v1)
- With rulers on, the drawing pane is smaller by the ruler width, so the two panes are offset by that many points.
  The same document point stays at each pane's centre.
- Only one pair, and only equal sizes.
- Not verified locally: this machine has no Rust toolchain. Compilation and tests run in the workflow above.
