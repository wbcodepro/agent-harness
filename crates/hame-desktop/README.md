# hame-desktop

Native desktop execution for hame: one `desktop` tool that observes and operates macOS applications through their accessibility interfaces, with physical input and screenshots as explicit, separately authorized fallbacks.

## Platform

- macOS only. The brief assumed macOS 13+, but the helper uses ScreenCaptureKit screenshot APIs from macOS 14, so it builds and runs on **macOS 14 or later**; it is **tested only on macOS 26.6.2 (Apple M5, built-in Retina display at scale 2.0)**.
- On Linux and Windows the crate compiles and every action returns `unsupported`.
- The helper is a Swift program embedded in the crate. It is compiled on first use with `swiftc -O -swift-version 5` (Xcode command line tools; tested with Swift 6.4.2) into `<hame home>/helpers/hame-desktop-helper-<source hash>`, so an upgraded hame never runs a stale helper. Compilation takes about 4 seconds.

## Permissions

The helper runs as a child of the process that runs hame and inherits that process's privacy grants. Grant both to the terminal (or app) that runs hame, then restart it:

| Permission | Where | Needed for |
|---|---|---|
| Accessibility | System Settings > Privacy & Security > Accessibility | everything except `apps` and `screenshot` |
| Screen Recording | System Settings > Privacy & Security > Screen & System Audio Recording | `screenshot` (and so `click_at`) |

A missing Accessibility grant is reported as `permission_denied` with that path. A screenshot that comes back as one flat colour is reported as `blank_capture` and not stored; that is usually a missing or stale Screen Recording grant.

## Operations

| Action | What it does | Capability |
|---|---|---|
| `apps` | Running GUI applications, filtered to the granted scope | Observe |
| `windows` | An app's windows: CGWindowID, pid, title, frame, focused, minimized, sheets | Observe |
| `observe` | Pruned accessibility tree of one window, refs `oN:eM`, dialogs and sheets, `complete=false` when pruned. Table cells show their address (`cell C2 "1,5"`). A window whose scroll area holds only scroll bars (Numbers, Pages) is a canvas the app hides until asked: the helper sets `AXEnhancedUserInterface` on that app once, as screen readers do, and says so | Observe |
| `press`, `set_value`, `focus` | Accessibility actions on a ref or a unique live name/role match | Interact; Commit for controls named Save/Send/Delete/Submit/Purchase/Pay/Replace/…; plus FileWrite for the confirming button of a save panel |
| `select` | Table cells (`cell` "C2" or "A1:C4" on a table or one of its cells, through the table's own cell lookup, so empty cells are reachable) or a stretch of text (`text`, its `occurrence`-th appearance, in a text element); reads the selection back and says when the app took less (Numbers takes only the first cell of a range) | Interact |
| `menu` | Menu bar path such as `File > Save…` (brings a named window to the front first) | Interact; Commit as above, except titles ending in `…` (they only open a dialog) |
| `type` | Unicode text as keyboard events, in chunks, to the focused element (`\n` and `\r\n` are sent as the Return key, `\t` as Tab) | GlobalInput, desktop lease; a newline or space adds what Return or Space would need (below) |
| `key` | A combination such as `cmd+s` or `cmd+shift+g` | GlobalInput, desktop lease, plus the table below |
| `screenshot` | A window (or a region) as PNG, optional `max_edge` downscale, capture transform recorded | Screen (Visual channel) |
| `click_at` | A pixel of a screenshot, mapped back to screen points | GlobalInput (Visual channel), desktop lease; plus what pressing the control under the point would need (below) |
| `clipboard` | `get`, `set` (saves the previous contents), `restore` | Clipboard |
| `wait` | Window, window gone, dialog, element, element gone, value, title; polling with backoff | Observe |
| `batch` | Up to 20 steps through the tool pipeline; stops at the first failure, unexpected dialog, failed `expect` or cancellation, and returns the completed prefix with a fresh observation | per step |

Any action can declare `commit: true`, which adds Commit to what it needs. Refusals are `not_authorized: …` from `Authorization::refusal`, followed by why the missing capability is needed (for example `Why: return in a save panel needs file_write (it can save a file).`).

### Physical input that commits

A key, a typed newline or a pixel click does whatever the control it reaches does, so `key`, `type` and `click_at` need Commit, and FileWrite in a save panel, when pressing that control would (`caps::key_needs`, `type_needs`, `click_needs`). For keys that depend on focus, the tool asks the helper where keyboard focus is (`focus_context`: the focused element's role and name, and whether it is inside a sheet, dialog, open or save panel, all the way up to its window) right before dispatch, and passes what it classified by as `expect_context`; the helper re-reads the focus just before posting the event and refuses with `interference` if it changed. `click_at` hit-tests the point the same way (`hit_test`, `AXUIElementCopyElementAtPosition`, in the screen points the screenshot's capture transform maps to) and passes `expect_hit`; the helper refuses with `stale_capture` if another element is under the point when it clicks.

| Input | Needs, besides GlobalInput |
|---|---|
| `return`, `enter`, `keypadenter` | Commit, unless focus is an `AXTextArea` outside any sheet, dialog or panel in a document editor (`caps::LINE_BREAK_APPS`: TextEdit, Pages, Numbers, Keynote, Notes, Word); elsewhere a text area may be a message box whose Return sends. In a save panel (including the Go To Folder sheet on it) also FileWrite. With cmd, ctrl or opt always Commit |
| `space` (without cmd or ctrl) | Commit unless focus is a text input (text area, text field, secure field, combo box, search field); on a save panel's control also FileWrite |
| `delete`, `forwarddelete` (without cmd or ctrl) | Commit unless focus is a text input |
| `cmd+s`, `cmd+shift+s`, `cmd+opt+s` (any cmd+…+s) | Commit and FileWrite |
| `cmd+q`, `cmd+w`, `cmd+d`, `cmd+delete` (with any further modifiers) | Commit |
| `cmd+c`, `cmd+x`, `cmd+v` (with any further modifiers) | Clipboard |
| other keys | nothing more |
| `type` text with a newline (`\n`, `\r`, other control characters except tab, U+2028/2029) | as `return` |
| `type` text with a space | as `space` |
| `click_at` on a named control (button, checkbox, menu item, link, ...) or inside one | as `press` on it: Commit for a commit word, plus FileWrite for a save panel's confirming button |
| `click_at` on text, a text field, a table row or cell, a scroll area | nothing more; a double-click there inside a dialog or panel needs Commit (plus FileWrite in a save panel) |
| `click_at` on an unnamed control, a generic group, web content that is not a named control, or a point the hit test cannot resolve | Commit (plus FileWrite in a save panel, or when the hit test failed) |

Confirming a save panel (Save or Export pressed or clicked, Return or a typed newline in it) is also held to the grant's `folders` (default the workspace): the helper reports the panel's "Where:" pop-up (`save_where`, "folder — parent", or just the folder), the tool asks the helper for Finder's display names of the granted folders, their parents and direct subfolders (`display_names`), and refuses a panel pointing anywhere else, or one whose pop-up cannot be found (an expanded panel). The place is passed back (`expect_context.save_where`, `expect_hit.save_where`, `expect_save_where`) and the helper refuses with `interference` if the panel moved before dispatch. Only names are visible, so a different folder with the same names would pass.

When the focus or the hit test cannot be determined, the stricter set applies. A commit made this way carries an idempotency key (the window, the focused control or the control clicked, the key or text, and the focused field's value), so the same Return in the same field, or a second click on the same Send button, is refused as `prior_effect` until reconciled; for `click_at` a newer screenshot of the window counts as the newer observation. Typing ordinary text into a field and Return in a document editor's text area need only GlobalInput and carry no key. Known gaps: when typing follows Numbers' separate formula editor window, the grants were worked out for the original focus and are not recomputed (same app only); `observe` sets `AXEnhancedUserInterface` on apps that hide their content until asked and does not turn it off.

Safety properties:

- A target is (bundle id, pid, CGWindowID, generation = app launch time). Refs are only valid with their observation and are re-checked live before every action: element still exists, still attached to the window, same role and name, enabled. Closed or replaced window: `stale_window`. Changed or detached element (for example the button of a sheet that was cancelled, whose element the app keeps alive): `stale_ref`. Two candidate windows or elements: `ambiguous`, with candidates.
- An inaccessible surface or a control without a press action is `unsupported` with a hint to use the Visual channel explicitly. The tool never switches from semantic control to coordinate input by itself.
- Physical input needs the desktop lease (`<hame home>/desktop.lease`, a cross-process file lock) and never steals focus: the target must be frontmost (`focus` first). Before every chunk the helper checks the frontmost app, the focused window or sheet, the focused element and the sheet count; any change stops with `interference` or `unexpected_dialog` and reports exactly what was typed. A `cancel` stops it between chunks.
- Modifier keys are only ever sent as flags on single events, never held, and the helper posts modifier releases when it exits; it restores a clipboard it changed when its input closes.
- Every acting operation is journalled through `ctx.journal` (`not_started`, `dispatched`, then `completed`, `failed` or `outcome_unknown`). Commits, including the physical-input commits above, carry an idempotency key; a repeat while an earlier attempt may have taken effect is refused (`prior_effect`) until the caller passes `reconciled: "<op_id>"` after a newer observation (or, for `click_at`, a newer screenshot).
- Accessibility names, values, clipboard text and pixels are labelled untrusted content.

## Tested apps and operations

All on macOS 26.6.2 with `cargo test -p hame-desktop --test gui -- --ignored --test-threads=1`:

| App | Operations exercised |
|---|---|
| TextEdit 1.20 (build 415; rich text and plain text documents) | `windows`, `observe` (document windows, save sheet, nested Go To Folder sheet, keep/delete sheet), `press` (buttons, close button, Open panel's New Document), `set_value` (text area, save-panel name and path fields; Vietnamese), `focus` (window, element), `menu` (`File > New`, `File > Save…`, `File > Save`, `File > Close`, `Window > Zoom`, `TextEdit > Quit TextEdit`), `type` (Vietnamese, ASCII; interrupted by a focus change; cancelled), `key` (`cmd+shift+g`, `return` inside the save panel), `screenshot` (full and `max_edge` 600), `click_at` (Bold button through a downscaled screenshot; refused after a resize), `wait` (dialog, element gone, title, value, window gone), `batch` (cancelled; stopped by the "keep this document" sheet; stopped by a failed `expect`) |
| Finder | `windows`, `menu` dry resolution (manual check only) |
| (system) | `clipboard` get/set/restore, `apps`, simulated missing Accessibility permission |

| Numbers 14.4 | `observe` (canvas exposed, cells with addresses, chart elements, export sheet, save panel), `select` (cell, partial range reported), `type` (values with `\t`/`\n`, formulas through the separate formula editor window), `key` (shift+arrows to extend a selection), `menu` (`Insert > Chart > 2D Column`, `Table > Header Rows > 1`, `File > Export To > Excel…`), `press` (Title / Axis Name checkboxes, Export), `screenshot` + `click_at` (chart and axis titles), and the pilot tasks O01, O02, O04, X02, X04 |
| Pages 14.4 | `observe` (body text area), `select` (text), `type` (replacement), `key` (cmd+i), `menu` (`File > Export To > Word…` / `PDF…`), pilot O03, O05 |
| Preview 11.0 | opening the exported PDF (pilot O05) |

Not tested: Notes, Keynote, System Settings, Electron or Java apps, multiple displays, non-ANSI keyboard layouts. Microsoft Excel 16.78 was looked at and not used: it exposes no cell grid through accessibility.

## Known unsupported or limited

- Scrolling, drag, hover, choosing items in pop-up menus by value, and context menus are not implemented.
- `type` follows an editor window the typing itself opened (a window of the same app that did not exist when typing started and is not a document, dialog or sheet: Numbers' formula editor) and returns when it closes; focus moving into the element typing started on (a cell's in-place editor) is allowed too. Any other focus change still stops input.
- `key` maps keys by ANSI key codes; on other layouts letters may differ (use `type` for text).
- `type` posts to the target process, but open and save panels run out of process and only receive events through the system event stream; inside a sheet the helper posts there, after verifying focus. Secure text fields generally ignore synthesized events.
- `wait` polls with backoff (30 ms rising to 300 ms); it does not use AXObserver notifications.
- Region screenshots cannot be used with `click_at` (no window identity to re-check).
- Apps without accessibility support (many games, some custom-drawn UIs) are `unsupported`; only the Visual channel works there.
- Menu items whose enabled state is stale are refreshed by briefly opening their menu.
- Windows on another Space or minimized cannot be captured.

## For the integrator

```rust
let mut config = hame_desktop::DesktopConfig::new(hame_home, authorization);
// optional: config.helper_dir, config.helper_path (prebuilt), config.swiftc,
//           config.lease_path, config.lease_wait
let tool = hame_desktop::DesktopTool::new(config).with_pipeline(pipeline);
let cancel = tool.cancel_handle(); // CancelHandle::cancel() stops a running call
```

The tool is host-side (`host_side() == true`), `Effect::Control`, not concurrent, and expects `ToolContext::artifacts` (screenshots), `journal` and `session_id`.
