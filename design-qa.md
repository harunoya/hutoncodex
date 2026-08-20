# Design QA

## Scope

- Target: Codex Remote desktop shell, Pair connection flow, combined model/reasoning picker, and Usage settings
- Visual source: `design-evidence/codex-reference.png`
- Final implementation: `design-evidence/codex-remote-deeper-final.png`
- Side-by-side comparison: `design-evidence/comparison-deeper-final.png`
- Focused state: `design-evidence/codex-remote-model-picker-final.png`
- Usage implementation: `design-evidence/codex-remote-usage-final.jpg`
- Usage daily-history state: `design-evidence/codex-remote-usage-daily-final.jpg`
- Usage shell comparison: `design-evidence/comparison-usage-shell-final.jpg`
- Usage iteration comparison: `design-evidence/comparison-usage-iteration-final-vertical.jpg`
- Native runtime: `design-evidence/tauri-runtime-deeper-final.png`
- Comparison viewport: 1296 x 824 CSS pixels at 1x density for source and implementation

The installed Codex 26.727.6591.0 bundle was used as the source of truth for the Usage screen where an authenticated live screenshot was unavailable. Its `usage-settings` module and Japanese locale supplied the section order, labels, 96 x 6 px progress geometry, remaining-percentage format, reset copy, credits, and daily-usage structure. The existing Codex screenshot remains the visual source for the surrounding desktop shell.

## Visual checks

- Typography: the installed Codex system stack and regular 400 base weight are applied; compact labels and muted secondary text follow the same hierarchy.
- Color: the main `#181818` surface, deep-purple sidebar/title bar, elevated composer/menu surfaces, white opacity levels, and restrained borders follow the Codex token family.
- Geometry: the 36 px application title bar, 275 px sidebar, 46 px task header, rounded main-canvas corner, conversation width, composer position, and Windows controls align with the reference.
- Motion: decorative entrance transforms, modal blur, oversized easing, and prominent scaling are absent. Motion is limited to state feedback, loading, and QR scanning.
- Components: desktop menu bar, sidebar shortcuts, project and pinned sections, task header, message bubble, composer footer, access control, model/reasoning picker, microphone/send actions, Pair tabs, inputs, status footer, and modal scrim were checked.
- Usage: the 940 x 690 settings frame, 212 px settings navigation, selected Usage row, card spacing, 96 x 6 px limit bars, remaining percentage labels, reset timestamps, credit/reset separation, and daily-usage rows were checked at the same 1296 x 824 viewport.
- Assets: UI icons use the same Lucide family already used by the installed Codex shell; this view has no raster content requiring replacement assets.

## Interaction checks

- Pair-code, QR Pair, and advanced WebSocket tabs switch correctly; Pair code and QR remain the primary paths and WebSocket stays under the advanced tab.
- Sidebar search is hidden by default, opens from the search action with focus, and closes cleanly.
- The combined model/reasoning trigger opens above the composer, exposes model choices and supported reasoning levels, updates the trigger, and closes with Escape.
- The compact sidebar Usage control opens the settings dialog, the close action returns to the task, refresh is connected to the official App Server reads, and the scrollable content exposes the complete daily history without clipping.
- Native Tauri startup, frameless title bar, window-control permissions, and the Pair dialog were exercised in the desktop runtime.
- Browser console was checked after the final interaction pass: no warnings or errors.

## Iteration history

- P1 resolved: the previous shell omitted the Codex application title bar and primary navigation density. The title bar, app menus, Windows controls, navigation shortcuts, and native frameless window behavior were added.
- P1 resolved: the sidebar hierarchy and task header did not match the reference proportions. Sidebar width, header height, search behavior, footer divider, and main-canvas radius now align.
- P2 resolved: the composer was missing Codex's access, microphone, and compact footer affordances. These controls and the `何でもどうぞ` prompt now match the reference structure.
- P2 resolved: model and reasoning controls differed from Codex. They are now one compact trigger with the same menu direction and a speed-to-intelligence effort scale.
- P2 resolved: custom title-bar drag handling covered interactive controls. Dragging is now isolated to the empty title-bar region so menus and buttons retain their click targets.
- P2 resolved: the first Usage pass nested available resets inside the credits card. The final pass moves it to Codex's standalone `利用上限のリセット` section and preserves the current Codex section rhythm.

No unresolved P0, P1, or P2 visual issues remain in the verified shell, Pair flow, model picker, and Usage settings. Remaining P3 differences are content-driven remote status text, account values, and task data rather than shell styling.

## Multiple connections and mobile addendum

### Evidence and normalization

- Source visual truth: `design-evidence/codex-reference.png` for the installed Codex shell, plus `design-evidence/codex-remote-baseline-1280x720.jpg` for the existing product state immediately surrounding the new connection control.
- Desktop implementation: `design-evidence/codex-remote-connections-desktop-final.jpg`.
- Desktop comparison: `design-evidence/comparison-connections-desktop-final.jpg`.
- Mobile conversation: `design-evidence/codex-remote-mobile-active-final.jpg`.
- Mobile connection manager: `design-evidence/codex-remote-mobile-connections-final.jpg`.
- Mobile task drawer: `design-evidence/codex-remote-mobile-drawer-final.jpg`.
- Mobile Pair-add sheet: `design-evidence/codex-remote-mobile-add-final.jpg`.
- Mobile comparisons: `design-evidence/comparison-mobile-connections-final.jpg` and `design-evidence/comparison-mobile-navigation-final.jpg`.
- Desktop source and implementation are both 1280 x 720 CSS px and 1280 x 720 image px at density 1.
- Mobile app content is 393 x 720 CSS px and normalized to 393 x 720 image px at density 1. The in-app browser stage was wider, so the centered 393 px app-owned region was cropped without scaling or visual alteration. The production layout uses the same <=600 px container rules.
- States compared: active conversation, connection manager open with three connected targets, mobile task drawer open, and mobile Pair-code add sheet open.

### Required fidelity surfaces

- Fonts and typography: the same installed Codex system stack, regular body weights, compact 9.5-13 px UI labels, restrained letter spacing, truncation, and two-line mobile hierarchy are retained. No mobile text overlaps or unreadable wrapping remains.
- Spacing and layout rhythm: the desktop manager uses the existing modal density and 64 px rows. Mobile uses a 393 px full-width canvas, 58 px bottom navigation, 44 px minimum destructive/add targets, a 356 px task drawer, and bottom sheets that remain inside the phone viewport.
- Colors and tokens: connection rows, modal scrim, green live status, red disconnect affordance, purple drawer, borders, and elevated surfaces reuse the existing Codex token family without decorative color drift.
- Image quality and assets: the flow contains no raster imagery. All interface symbols use the existing Lucide icon family; no emoji, placeholder art, custom SVG, CSS illustration, or generated raster substitute is present.
- Copy and content: `接続先`, connected-device count, `使用中`, Pair method, platform, `接続を追加`, and individual disconnect labels remain concise and actionable in Japanese.
- Responsiveness and accessibility: navigation, connection switching, Pair addition, drawer opening, close controls, selected state, disabled state, and individual disconnect are keyboard-labelled. The mobile layout keeps core controls visible and uses practical touch targets.

### Interaction verification

- Desktop: opened the manager, switched from `remote-workspace` to `devbox-tokyo`, reopened the manager, disconnected `personal-laptop`, and verified that the other connections remained available.
- Mobile: switched connections from the bottom sheet, reopened it from the bottom navigation, opened the Pair-add flow, closed it, opened the task drawer, and returned to the conversation.
- Native routing: the Tauri backend now stores live connections by connection ID; messages and disconnect requests target one ID instead of replacing the current socket.
- Browser console: no warning or error entries were present in the final desktop or mobile interaction passes.

### Comparison history

- P1 resolved: the native layer previously closed the current connection whenever another was added. It now keeps a connection map and removes only the requested ID; a dedicated Rust test verifies that another live connection survives.
- P2 resolved: the first responsive pass inherited the desktop-width sidebar state. Mobile preview now starts with the drawer closed and exposes it from the bottom navigation.
- P2 resolved: the first mobile drawer stayed at the desktop 275 px width. The final 356 px drawer restores the intended near-full-width mobile hierarchy and 44 px-class targets.
- P2 resolved: the first mobile connection overlay escaped the 393 px app region in the browser stage. The final bottom sheet is constrained to the phone canvas and keeps all three rows plus the add action visible.
- P2 resolved: the first Pair-add sheet showed stale metadata from the active connection. That unrelated server line was removed from the add-connection state.

No unresolved P0, P1, or P2 issues remain in the multiple-connection manager or the verified 393 px mobile states. A remaining P3 constraint is that actual Android/iOS packaging and device-specific camera permission behavior were not requested or exercised; this delivery covers the responsive Tauri/web UI.

final result: passed
