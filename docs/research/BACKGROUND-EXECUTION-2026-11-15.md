# Working on the computer without taking over the user's screen

Research date: 15 November 2026. Question: hame's desktop tool raises
windows and sends physical keyboard and mouse input, so while it works the
user cannot use the Mac. What ways exist for an agent to operate
applications on the same machine without competing for the user's screen,
keyboard and pointer, and which fit hame?

## Where hame takes over today

| Tool | Takes over the UI? | Why |
|---|---|---|
| `browser` | No | Playwright drives a headless Chrome (the default `[computer] headless = true`) with its own profile. |
| `desktop` `observe`, `press`, `set_value`, `select`, `menu`, `wait` | Mostly no | Accessibility (AX) reads and AX actions work on a background app; they do not move the pointer. Some apps raise a window or come forward when an AX action opens a sheet or menu. |
| `desktop` `focus` | Yes | `AXRaise` plus `NSRunningApplication.activate` (`DesktopHelper.swift` around line 1233). |
| `desktop` `type`, `key`, `click_at` | Yes | Refused unless the target is frontmost and focused; input goes through the HID event tap (`post(tap: .cghidEventTap)`), the same path as the real keyboard and mouse, under the cross-process desktop lease. The helper already has a `postToPid` path (`postUnicode`, `postKey`), but the safety checks require focus. |
| `desktop` `screenshot` | No | `SCScreenshotManager` with `SCContentFilter(desktopIndependentWindow:)` captures one window even when other windows cover it (Apple, WWDC22 session 10155). |

So the takeover comes from physical input and the `focus` that has to
precede it. In the pilot, desktop tasks used `type`/`key` mostly for save
panels (Go To Folder, Return) and for typing into documents.

## Approaches found

### 1. Background input on the user's own session (macOS)

- **Accessibility actions only.** `AXPress`, setting `AXValue` and choosing
  menu items work on background apps without focus. This is the most
  reliable documented route; it does not cover every control (canvases,
  games, some web content).
- **`CGEventPostToPid`** (public CoreGraphics): delivers synthesized events
  to one process instead of the HID stream, so the user's frontmost app
  keeps its keyboard. It works for many Cocoa apps but not all: apps check
  whether their window is key or main before handling clicks, and some
  ignore background key events (Apple forum report on Logic Pro; Stack
  Overflow on the older `CGEventPostToPSN`). Needs Accessibility
  permission for the sending binary.
- **Cua Driver** ([trycua/cua `libs/cua-driver`](https://github.com/trycua/cua/tree/main/libs/cua-driver), MIT;
  [write-up](https://github.com/trycua/cua/blob/main/blog/inside-macos-window-internals.md);
  [Show HN](https://news.ycombinator.com/item?id=47936312)): an MCP server
  that drives Mac apps "in the background without stealing the cursor".
  Its write-up (v0.1) describes AX actions for addressable elements, keys
  through `CGEvent.postToPid`, pixel clicks through the private SkyLight
  call `SLEventPostToPid`, "focus without raise" borrowed from yabai
  (`SLPSPostEventRecordTo` to flip AppKit's active state without
  `SetFrontProcess`), an off-screen primer click for Chromium's
  user-activation check, and a private AX call to keep occluded Electron
  trees live. Canvas apps (Blender, Unity, games) filter per-process
  routes, so Cua activates them and the cursor moves. Requires macOS 14+
  (its Spaces app macOS 26+). A fullscreen-cursor bug
  ([#3801](https://github.com/trycua/cua/issues/3801)) was closed in Nov
  2026. Telemetry is on by default (opt-out); the optional perception
  extension uses AGPL-3.0 OmniParser.
- **Risk:** the private SkyLight/CGS calls are undocumented and can break
  with a macOS update; apps behave differently per toolkit.

### 2. A separate screen on the same session

A virtual display ([CGVirtualDisplay](https://pkg.go.dev/github.com/go-macos/virtualdisplay),
private API; used by DeskPad, BetterDisplay, FluffyDisplay) gives the agent
screen space the user does not look at. It does not solve input: macOS has
one keyboard focus and one frontmost app per session, so physical input
still competes. Useful only together with approach 1.

### 3. An isolated virtual machine on the same Mac

- **Tart** ([cirruslabs/tart](https://github.com/cirruslabs/tart)) and
  **Lume** (part of trycua/cua) run macOS and Linux guests on Apple
  Silicon with Apple's Virtualization.framework, near-native speed, images
  from OCI registries. The guest has its own WindowServer, keyboard focus
  and pointer, so the agent can use physical input freely while the user
  works on the host.
- **Limits:** the macOS licence allows up to two additional macOS
  instances per Mac, for development, testing, macOS Server or personal
  non-commercial use, and Virtualization.framework refuses a third guest
  ([Eclectic Light](https://eclecticlight.co/2022/08/04/virtualisation-on-apple-silicon-macs-8-how-apple-limits-vms/)).
  Base images are about 25 GB; no nested virtualization; since macOS 15
  running a VM needs an unlocked login keychain. The user's apps, Apple ID
  (iWork, iCloud) and files are not in the guest unless installed or
  shared (virtiofs folders); Accessibility and Screen Recording must be
  granted inside the guest.
- **Fit:** best for untrusted tasks, evaluations (parallel GUI runs, two
  guests) and anything that must not touch the user's own data.

### 4. Linux: virtual X or Wayland displays and containers

The common pattern for computer-use agents: a desktop inside a container
with a virtual framebuffer. Anthropic's computer-use reference
implementation runs Xvfb, a VNC server and a desktop in Docker
([docs](https://docs.anthropic.com/en/docs/agents-and-tools/computer-use));
[E2B Desktop](https://docs.e2b.dev/use-cases/computer-use.md) (Xvfb, XFCE,
VNC/noVNC, cloud) and [Bytebot](https://docs.bytebot.ai/core-concepts/desktop-environment)
(self-hosted Docker desktop) do the same. On Wayland, wlroots compositors
have a headless backend: Sway can create a headless output
(`swaymsg create_output`) and WayVNC exposes it with virtual input
([wayland-devel](https://lists.freedesktop.org/archives/wayland-devel/2021-May/041841.html),
[WayVNC 0.8](https://Phoronix.com/news/WayVNC-0.8-Released)). Each agent
gets its own display, keyboard and pointer; the user's session is
untouched.

### 5. Windows: a second session for the agent

- **Agent workspace** (Windows 11, preview): agents run "in a separate
  Windows session, allowing agents to interact with apps in parallel to
  your own session" ([Microsoft support](https://support.microsoft.com/en-us/windows/ai/ai-features/experimental-agentic-features),
  [Windows blog](https://blogs.windows.com/windowsexperience/2025/10/16/securing-ai-agents-on-windows/)).
- **UFO² Picture-in-Picture** ([paper](https://arxiv.org/html/2504.14603v2),
  [docs](https://microsoft.github.io/UFO/ufo2/overview)): a nested desktop
  built on Windows' Remote Desktop loopback, so "user and agent don't
  compete for mouse/keyboard". The GitHub README listed it as coming soon
  while the docs list it; check the release.
- **`CreateDesktop`** (Win32) gives a separate desktop object, but
  `SendInput` only reaches the input desktop, so it is not a background
  input channel on its own; Windows Sandbox and Hyper-V are the VM
  options.

## Comparison for hame

| Approach | User keeps the Mac? | Same apps and data as the user? | Effort in hame | Main risk |
|---|---|---|---|---|
| AX-only "background mode" (no `focus`/`type`/`key`/`click_at` unless allowed) | Yes | Yes | Small: a policy flag, prefer `set_value`/`press`/`menu`, launch apps with `open -g -j` | Some tasks cannot finish without physical input |
| + `CGEventPostToPid` for keys and text | Yes, for apps that accept it | Yes | Medium: path exists in the helper; needs per-app tests and new focus checks (the target window must be key inside its app, not frontmost) | Silent drops in apps that ignore background events; must verify each effect |
| Cua Driver as an alternative backend (MCP) | Mostly | Yes | Small to integrate (hame speaks MCP) | Private APIs, telemetry default, grants and receipts would sit outside hame's checks unless wrapped |
| macOS VM (Tart/Lume) with hame's helper inside | Yes, completely | No (separate macOS) | Medium: run the helper over SSH (`helper_path` wrapper), share the workspace folder, provision TCC in the image | Licence limits and purposes, 25 GB images, Apple ID apps |
| Linux container with Xvfb/headless Sway | Yes | No | Large: hame's desktop tool is macOS-only today | New backend (AT-SPI or pixels) |
| Windows agent workspace / UFO² PiP | Yes | Partly | Large: no Windows desktop backend yet | Preview features |

## Verified on this Mac

Probes in `crates/hame-desktop/bench/background/` (Swift, run from the
terminal that holds Accessibility and Screen Recording; compile with
`swiftc -O -parse-as-library probeN.swift -o probeN` and pass an output
folder). macOS 26.6.2, TextEdit 1.20, Numbers and Pages 14.4, 15 November
2026. Every step records the frontmost application before and after. The
user was working in Chrome, VS Code and Telegram during the runs; the
probes post no mouse events, so the "cursor moved" flags in their output
are the user's own pointer.

**The target app never became frontmost** in any step of any run: AX
reads and writes, AX menu presses, `postToPid` text and keys, unhide,
minimize and window capture. The user's application kept the keyboard
throughout.

| What | Result | Notes |
|---|---|---|
| Launch with `open -g` (and `-j`) | works | document windows are visible to AX; `-j` did not keep a TextEdit document window hidden |
| AX read of text, cells, menus | works | Numbers and Pages need `AXEnhancedUserInterface` and a few seconds before their canvases appear in the tree |
| AX set value, TextEdit text area | works | the document is marked edited (its title shows "Edited"); saving it is a document command, see below |
| AX set value, Numbers cell / Pages body | does not work | Numbers refuses (-25200); Pages ignores it |
| `postToPid` text, TextEdit (background, not hidden) | works | Vietnamese included; macOS autocapitalization applies ("typed" became "Typed"); the document becomes edited |
| `postToPid` text, Numbers | works after selecting the cell by AX (`AXSelectedCells`) | B2 became 777 and Return moved the selection to B3, all in the background |
| `postToPid` text, Pages | works after placing the caret by AX | lands in whichever text area has the caret: the probe picked the header's, so targeting must choose the body |
| `postToPid` text into a minimized window | does not work | |
| `postToPid` key equivalents (cmd+s, cmd+shift+g) | do not work in the background | nothing saved; no Go To Folder sheet |
| AX press File > Save / Save As… with the app in the background | does nothing (AX reports success) | no save after 4 s, no sheet |
| AX press of other menu items (Edit > Select All, File > Close) in the background | does nothing (AX reports success) | menu commands in general, not only saving |
| AX press of a control inside the window (TextEdit's Bold button) in the background | works | value 0 → 1 |
| the same after `NSRunningApplication.activate()` | works (Save wrote the file; Save As… opened its sheet) | macOS did not bring TextEdit forward (Chrome stayed frontmost), so `activate()` changed the app's own active state only; macOS may honor the activation when the user is idle, which would raise the app, so this is a lead, not a safe default |
| Save panel: Go To Folder by `postToPid` key, even after `activate()` | does not work | panel navigation needs either the foreground or another route |
| Capture of a background, occluded or hidden-app window (`SCScreenshotManager`, `desktopIndependentWindow`) | works | full content for TextEdit, Numbers and Pages; set `showsCursor = false` or the pointer is drawn in |
| Capture of a minimized window | does not work | ScreenCaptureKit error -3811 |

Probe 6 (`probe6.swift`) checked the other menu items and the Bold button.

What follows for hame: reading, capturing and editing content (TextEdit
text; Numbers cells by selecting then typing; Pages text with the caret
placed) can run without taking the screen. **Document commands do not**:
Save, Save As, export and every save-panel step need the app active, and
key equivalents are not delivered to a background app. Those are the
steps the pilot's desktop tasks end with. A background mode therefore has
to either (a) hand the user a short, announced foreground step for saving
("hame needs TextEdit in front for about 5 seconds to save"), (b) use the
app's scripting interface (`save document … in file …` through Apple
Events, which needs the Automation consent and was not tested here, to
avoid a consent prompt on the user's screen), or (c) run in a VM where
taking the foreground costs the user nothing.

## Recommendation

1. **Now, small:** add `[computer] desktop_background = true`. With it the
   desktop tool never calls `focus`, sends text through `postToPid` (after
   placing the caret or selecting the cell by AX, and reading the value
   back), captures windows without raising them, and launches apps with
   `open -g`. Document commands (save, save panels, export) and key
   equivalents need the app active (verified above), so those steps stop
   with `needs_foreground` and ask the user for a short, announced
   foreground step instead of taking it. Measure how much of each pilot
   desktop task then runs without the foreground.
2. **Next, medium:** route keys and text through `postToPid` in background
   mode for apps that pass a per-app check (TextEdit, Numbers, Pages
   first), keeping hame's checks (focused element inside the target
   window, unexpected sheets, read-back of the value after typing) and
   recording which apps accept it. Do not adopt private SkyLight calls in
   the default path.
3. **For isolation and evaluation:** a "VM target": the same desktop
   helper running inside a Tart macOS guest, reached over SSH, with grants,
   journal and receipts still enforced on the host. It removes the shared
   GUI from evaluations (two guests, in parallel) and gives untrusted tasks
   a machine of their own, within the macOS licence terms.
4. Keep the browser headless by default (already the case).

Still unverified (deliberately not run on the user's machine): windows on
another Space; a second logged-in macOS user session (fast user
switching), which needs a new user account; Apple Events `save` in the
background, which needs an Automation consent prompt; a Tart or Lume VM
(a 25 GB image); Cua Driver itself; pixel clicks into background windows
(only the private `SLEventPostToPid` route is known to work).

## Implemented: `[computer] desktop_background = true`

hame's desktop tool now has the background mode recommended above
(`DesktopConfig.background`). Text and plain keys (no cmd, ctrl or opt)
go to the target app's process with `postToPid`; the helper requires the
target window to be the focused window inside its app (making it main
through accessibility when it is not, which does not bring the app
forward) instead of frontmost, keeps its checks for focus moving to
another element, unexpected sheets and dialogs, and refuses input for a
sheet or an open or save panel (drawn out of process) as
`needs_foreground`. `select` takes `caret: "before" | "after"` to place an
insertion point, and selecting cells no longer brings the window forward
in this mode. `focus`, `menu`, `click_at` and keys with cmd, ctrl or opt
are refused with `needs_foreground`, telling the model to finish what it
can and tell the user which step needs the app in front.

Verified by the GUI test `background_mode_types_without_taking_the_foreground`:
a document opened with `open -g` behind Finder gets an insertion point
after a phrase, Vietnamese text and a Return typed into it, cmd+s, a menu
command and `focus` are refused, Finder stays frontmost, and nothing is
saved.

**Saving in the background: the `save` action.** Apple Events reach an
app in the background without activating it. Checked by hand first:
`save (document of window id W) in POSIX file …` wrote a background
TextEdit document, and Numbers exported a background spreadsheet to a real
`.xlsx` (`export … as Microsoft Excel`) and saved a `.numbers`, with Finder
frontmost throughout. AppleScript window ids are the CGWindowIDs hame
already uses. hame's desktop tool now has `save` (TextEdit, Numbers,
Pages; in place, to a path, or exported as pdf, xlsx, csv or docx), with
`commit` and `file_write`, the destination checked by its real path
against the grant's folders, and no replacement of an existing file
without `overwrite`. The GUI test above continues: save in place, save a
copy into a granted folder, refusal for another folder and for an
existing file, Finder still frontmost. A modal dialog left open in the app
blocks Apple Events; `save` then times out and reports `outcome_unknown`
(seen once with a TextEdit left in a bad state by a manual experiment).
macOS asks once per app for the Automation permission of the process
running hame. Still to measure: the pilot's desktop tasks in background
mode.
