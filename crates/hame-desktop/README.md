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
| `observe` | Pruned accessibility tree of one window, refs `oN:eM`, dialogs and sheets, `complete=false` when pruned | Observe |
| `press`, `set_value`, `focus` | Accessibility actions on a ref or a unique live name/role match | Interact; Commit for controls named Save/Send/Delete/Submit/Purchase/Pay/Replace/…; plus FileWrite for the confirming button of a save panel |
| `menu` | Menu bar path such as `File > Save…` (brings a named window to the front first) | Interact; Commit as above, except titles ending in `…` (they only open a dialog) |
| `type` | Unicode text as keyboard events, in chunks, to the focused element | GlobalInput, desktop lease |
| `key` | A combination such as `cmd+s` or `cmd+shift+g` | GlobalInput, desktop lease |
| `screenshot` | A window (or a region) as PNG, optional `max_edge` downscale, capture transform recorded | Screen (Visual channel) |
| `click_at` | A pixel of a screenshot, mapped back to screen points | GlobalInput (Visual channel), desktop lease |
| `clipboard` | `get`, `set` (saves the previous contents), `restore` | Clipboard |
| `wait` | Window, window gone, dialog, element, element gone, value, title; polling with backoff | Observe |
| `batch` | Up to 20 steps through the tool pipeline; stops at the first failure, unexpected dialog, failed `expect` or cancellation, and returns the completed prefix with a fresh observation | per step |

Any action can declare `commit: true`, which adds Commit to what it needs. Refusals are `not_authorized: …` from `Authorization::refusal`.

Safety properties:

- A target is (bundle id, pid, CGWindowID, generation = app launch time). Refs are only valid with their observation and are re-checked live before every action: element still exists, still attached to the window, same role and name, enabled. Closed or replaced window: `stale_window`. Changed or detached element (for example the button of a sheet that was cancelled, whose element the app keeps alive): `stale_ref`. Two candidate windows or elements: `ambiguous`, with candidates.
- An inaccessible surface or a control without a press action is `unsupported` with a hint to use the Visual channel explicitly. The tool never switches from semantic control to coordinate input by itself.
- Physical input needs the desktop lease (`<hame home>/desktop.lease`, a cross-process file lock) and never steals focus: the target must be frontmost (`focus` first). Before every chunk the helper checks the frontmost app, the focused window or sheet, the focused element and the sheet count; any change stops with `interference` or `unexpected_dialog` and reports exactly what was typed. A `cancel` stops it between chunks.
- Modifier keys are only ever sent as flags on single events, never held, and the helper posts modifier releases when it exits; it restores a clipboard it changed when its input closes.
- Every acting operation is journalled through `ctx.journal` (`not_started`, `dispatched`, then `completed`, `failed` or `outcome_unknown`). Commits carry an idempotency key; a repeat while an earlier attempt may have taken effect is refused (`prior_effect`) until the caller passes `reconciled: "<op_id>"` after a newer observation.
- Accessibility names, values, clipboard text and pixels are labelled untrusted content.

## Tested apps and operations

All on macOS 26.6.2 with `cargo test -p hame-desktop --test gui -- --ignored --test-threads=1`:

| App | Operations exercised |
|---|---|
| TextEdit 1.20 (build 415; rich text and plain text documents) | `windows`, `observe` (document windows, save sheet, nested Go To Folder sheet, keep/delete sheet), `press` (buttons, close button, Open panel's New Document), `set_value` (text area, save-panel name and path fields; Vietnamese), `focus` (window, element), `menu` (`File > New`, `File > Save…`, `File > Save`, `File > Close`, `Window > Zoom`, `TextEdit > Quit TextEdit`), `type` (Vietnamese, ASCII; interrupted by a focus change; cancelled), `key` (`cmd+shift+g`, `return` inside the save panel), `screenshot` (full and `max_edge` 600), `click_at` (Bold button through a downscaled screenshot; refused after a resize), `wait` (dialog, element gone, title, value, window gone), `batch` (cancelled; stopped by the "keep this document" sheet; stopped by a failed `expect`) |
| Finder | `windows`, `menu` dry resolution (manual check only) |
| (system) | `clipboard` get/set/restore, `apps`, simulated missing Accessibility permission |

Not tested: Preview, Notes, Numbers, Pages, System Settings, Electron or Java apps, multiple displays, non-ANSI keyboard layouts.

## Known unsupported or limited

- Scrolling, drag, hover, selecting text ranges, choosing items in pop-up menus by value, and context menus are not implemented.
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
