# apps/desktop — hame's desktop app

A [Tauri v2](https://v2.tauri.app) app. It runs the hame web server
(`crates/hame-web`) **in its own process** and shows the interface of
`apps/ui` in the system web view (WKWebView on macOS, WebView2 on Windows,
WebKitGTK on Linux). No Chromium is bundled.

## Architecture

```
hame.app / hame-desktop.exe (one process)
├─ Tauri: window, tray (menu bar) icon, menus, notifications, links
├─ hame_web::start_with_token → axum on 127.0.0.1:<port>   (same server as `hame web`)
│     └─ serves crates/hame-web/ui-dist (embedded) and /api/…
└─ web view → http://127.0.0.1:<port>/   (token from an initialization script)
`<exe> tool-worker` / `<exe> sandbox-exec`: hame's confined tool workers re-run this binary
```

- **Server.** At launch the app makes a fresh 256-bit token (OS random
  source, `src/token.rs`) and starts the server on 127.0.0.1 with it. The
  token lives only in memory. The port of the last launch is reused when
  free (`<app data>/port`), so the interface's own storage (theme, drafts,
  what was read), which the web view keys by origin, survives restarts;
  otherwise any free port.
- **Token handoff.** The window loads `http://127.0.0.1:<port>/` with no
  token in the address. An initialization script, run before the page's
  scripts and only on that exact origin, defines a frozen
  `window.__HAME_DESKTOP__ = {token, platform, version}`
  (`src/bridge.rs`). So the token is never in an address, the history or a
  log. The server's checks are unchanged: token on every API call,
  requests addressed to `127.0.0.1`/`localhost` only (against DNS
  rebinding).
- **Content Security Policy.** The page keeps the server's strict policy
  (`crates/hame-web/src/assets.rs`: `default-src 'self'`, no inline
  scripts, `connect-src 'self'`, `frame-ancestors 'none'`). The app does not
  loosen it. Tauri's IPC therefore uses its `postMessage` channel (the
  page logs one warning about the `ipc:` protocol at the first call). The
  Tauri configuration sets no CSP of its own: it would cover only the
  placeholder page in `static/`, which is never shown.
- **What the page may call.** At launch the app adds a runtime capability
  for the exact origin `http://127.0.0.1:<port>/*` and the `main` window
  only: `pick_folder`, `desktop_ready`, and moving or zooming the window by
  its title area. Nothing else: no file system, shell or plugin commands.
- **Navigation.** The window stays on the server's origin. Other `http`,
  `https` and `mailto` links open in the default browser, new-window
  requests too (every link in a reply is `target=_blank`), and anything
  else is dropped. The opener plugin is used only as a Rust function; it is
  not registered, since its script would catch those clicks and call a
  command the page may not use.
- **Page events.** The shell tells the page to open a session or start
  one with `hame:desktop` DOM events. They wait in a queue (`src/page.rs`)
  until the interface calls `desktop_ready` from the effect that attaches
  its listener, and the queue waits again whenever a page load starts. So a
  link that launches the app, or a notification clicked during a reload,
  is not lost.
- **Activity.** The app reads `WebServer::activity()` (turn started or
  ended, approval asked, in any session), `running_count()` (the tray
  tooltip, no log read) and `running()` (with titles, only for the quit
  confirmation, off the main thread).
- **Tool workers.** hame runs every tool call in a confined
  `<exe> tool-worker` process (on Linux and Windows wrapped by
  `<exe> sandbox-exec`). `main.rs` answers those arguments before any of
  the app starts, with the same code as `hame`
  (`hame_sandbox::internal`).
- **Data.** Same as the `hame` command (`hame_app::default_home`):
  `$HAME_HOME`, else `~/.hame`. New
  sessions start in the home folder unless another folder is picked.

## Features

| Feature | Where |
|---|---|
| Single instance; a second launch shows the window and passes on its links | `tauri-plugin-single-instance` (with `deep-link`) |
| Closing the window hides it; turns keep running | `on_window_event` in `src/lib.rs` (Windows and Linux say so once per launch in a notification) |
| Tray / menu bar icon: Open hame, New Session, Start at Login, Quit hame; tooltip counts running turns | `src/shell.rs` (macOS uses a template image) |
| Quit with turns running asks first and names them; quitting stops them so their logs say cancelled | `src/quit.rs`, `request_quit` |
| Notification when a turn finishes, fails or needs approval (with the question) while the window is hidden or not focused; a click opens that session; at most one per session until the user comes back to the window | `src/notice.rs`, `tauri-plugin-notification` (`on_action`) |
| `hame://open`, `hame://open?session=<id>`, `hame://new` | `src/deeplink.rs`, `tauri-plugin-deep-link` |
| Start at login, off by default (tray check item); started that way, the app runs without showing its window | `tauri-plugin-autostart` (macOS: LaunchAgent, with `--hidden`) |
| Window size, position and maximized state remembered | `tauri-plugin-window-state` |
| Native folder picker for a new session's folder ("Other folder…") | `pick_folder`, `tauri-plugin-dialog` |
| macOS: app menu (About, Hide, Quit, New Session ⌘N, Close Window ⌘W, Edit, Reload, Full Screen, Window, Help); hidden inset title bar with the window buttons over the sidebar | `src/shell.rs`, `apps/ui` styles |
| Windows and Linux: default title bar, no menu bar | |
| Updater: behind the `updater` cargo feature, and registered only when an endpoint and a public key are configured; on Windows the turns are stopped before the installer takes over | `src/updater.rs` |

### The interface's side (apps/ui)

`apps/ui/src/desktop.ts` reads `window.__HAME_DESKTOP__`. When it is
present, the interface:

- takes the token from it (not from `?token=`);
- marks `<html data-desktop="macos|windows|linux">`; on macOS the sidebar's
  top row and the rail leave room for the window buttons, and the title
  areas carry `data-tauri-drag-region`;
- handles `hame:desktop` events: `{action: "open", session}` and
  `{action: "new"}`;
- uses the native picker for "Other folder…";
- calls `desktop_ready` once its `hame:desktop` listener is attached (from
  the effect in `App.tsx`), and the shell hands over what waited.

In a browser (`hame web`) none of this runs.

## Building

Prerequisites: Rust (stable, 1.89 or newer), Node 20 or newer, and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for the
platform.

```sh
cd apps/desktop
npm ci                 # the Tauri CLI (@tauri-apps/cli), nothing else
npm run dev            # debug build, runs it
npm run build          # release build and installers for this platform
npm run build:app      # macOS: only hame.app (no .dmg)
npm run build:debug    # debug build of the bundles
npm run build:smoke    # macOS: hame.app with the `smoke` feature, for the smoke test
npm run icons          # regenerate src-tauri/icons from icons/*.svg
```

The interface is not built here. It is embedded from
`crates/hame-web/ui-dist` (committed; `npm run build` at the repository
root regenerates it), so rebuild that first after changing `apps/ui`.

The Rust crate `src-tauri` is its own Cargo workspace (and excluded from
the root one). `cargo build --workspace` at the repository root, and the
core crates' CI on Linux and Windows, never compile a web view. It uses
`hame-web`, `hame-app`, `hame-sandbox` and `hame-kernel` by path. Unit
tests: `cargo test` in `src-tauri`. Cargo features: `updater` (see below)
and `smoke` (test builds only: writes `$HAME_DESKTOP_INFO_FILE`, logs
links instead of opening them).

- **macOS**: Xcode command line tools. Output in
  `src-tauri/target/release/bundle/macos/hame.app` and `…/dmg/`. The
  configuration signs ad hoc (`signingIdentity: "-"`), with the hardened
  runtime and the entitlements below, so local builds behave like signed
  ones; such a bundle runs on the machine that built it, and Gatekeeper
  refuses it elsewhere. The bundle carries
  `Entitlements.plist` (`com.apple.security.automation.apple-events`) and
  merges `Info.plist` (`NSAppleEventsUsageDescription`), which hame's
  desktop tool needs to script TextEdit, Numbers and Pages from the
  hardened app. Accessibility and screen recording have no Info.plist key;
  the user grants them in System Settings. `scripts/smoke-macos.py` runs a
  `build:smoke` bundle with a scripted model and checks the server, the
  window, hiding, a turn with approval, links (in-app and launching),
  a click on a link in a reply, and the quit confirmation. It needs
  Accessibility access for the terminal.
- **Windows**: Visual Studio Build Tools (C++), WebView2 (preinstalled on
  Windows 10/11). `npm run build` makes an NSIS installer (per-user
  install) in `src-tauri\target\release\bundle\nsis\`. Release builds use
  the GUI subsystem (no console window). Not yet built on Windows; see
  "Not done".
- **Linux**: `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev` (tray),
  `librsvg2-dev`, `libssl-dev`, `build-essential` (Debian/Ubuntu names).
  Bundles: `.deb` and AppImage. The app registers `hame://` at launch on
  Linux.

### Environment

| Variable | Effect |
|---|---|
| `HAME_HOME` | data directory, as for `hame` |
| `HAME_SCRIPT`, `HAME_SCRIPT_DELAY_MS` | replay a recorded model (tests; no provider is called) |
| `HAME_DESKTOP_INFO_FILE` | `smoke` builds only: the app writes `{pid, addr, token}` to this new file (mode 0600; an existing file or link is refused) once it serves. Release builds ignore it. |

## Signing, notarization and updates (later)

Nothing here is signed with a real certificate, and no keys are in the
repository.

- **macOS.** Needs an Apple Developer ID Application certificate. Set
  `APPLE_SIGNING_IDENTITY` (or `bundle.macOS.signingIdentity`), keep
  `hardenedRuntime: true`, and notarize with `APPLE_ID`, `APPLE_PASSWORD`
  (app-specific) and `APPLE_TEAM_ID`, or with `APPLE_API_KEY`,
  `APPLE_API_ISSUER` and `APPLE_API_KEY_PATH`. `tauri build` signs,
  notarizes and staples when these are set. The app is not sandboxed (App
  Sandbox would stop hame's own sandbox and the commands it runs), so it
  is distributed outside the Mac App Store. Check the result with
  `spctl -a -vv hame.app` and `codesign -dv --verbose=4 hame.app`.
- **Windows.** Sign the installer and the executable with an Authenticode
  certificate (`bundle.windows.certificateThumbprint` with
  `digestAlgorithm: sha256` and a `timestampUrl`, or a `signCommand`, for
  example for Azure Trusted Signing). Unsigned installers trigger
  SmartScreen.
- **Linux.** AppImages can carry a GPG signature (`SIGN=1`,
  `SIGN_KEY`); `.deb` packages are signed by the repository that serves
  them.
- **Updates.** Generate a key pair with `npx tauri signer generate` and
  keep the private key out of the repository (`TAURI_SIGNING_PRIVATE_KEY`,
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the release environment). Build
  releases with the feature and a configuration merged in, for example
  `npx tauri build --features updater --config updater.conf.json`:

  ```json
  {
    "bundle": { "createUpdaterArtifacts": true },
    "plugins": {
      "updater": {
        "pubkey": "<contents of the .pub file>",
        "endpoints": ["https://<your host>/hame/{{target}}/{{arch}}/{{current_version}}"],
        "requireSignedVersion": true
      }
    }
  }
  ```

  The app registers the updater only when built with the feature and such
  a configuration names a key and HTTPS endpoints. It then looks for an update at launch and asks
  before installing and restarting.

## Mobile (package E)

The same project is meant to build the iOS and Android apps (remote mode
only: no in-process server; the UI's remote transport through the relay).
Already in place: the crate is a library with `run()` marked
`#[cfg_attr(mobile, tauri::mobile_entry_point)]`, and `main.rs` is a thin
wrapper. The library is built as `rlib` only; the mobile builds add
`staticlib` (iOS) and `cdylib` (Android) to `crate-type`. What `tauri ios init` / `tauri android init`
will need:

- `tauri ios init`: Xcode, CocoaPods, the `aarch64-apple-ios` and
  `aarch64-apple-ios-sim` Rust targets, an Apple development team
  (`bundle.iOS.developmentTeam`) for devices. It generates
  `src-tauri/gen/apple` (commit it).
- `tauri android init`: Android Studio with SDK and NDK (`ANDROID_HOME`,
  `NDK_HOME`), JDK 17, and the Rust targets `aarch64-linux-android`,
  `armv7-linux-androideabi`, `i686-linux-android` and
  `x86_64-linux-android`. It generates `src-tauri/gen/android`.
- Code: the server, tray, menus, single instance, autostart, window state,
  updater, tool workers and `hame-app`/`hame-web` are desktop-only. Put
  them and their dependencies behind `cfg(desktop)` (Cargo
  `[target.'cfg(not(any(target_os = "ios", target_os = "android")))'.dependencies]`),
  and give mobile a `run()` that loads the bundled `ui-dist` (as
  `frontendDist`) with the remote transport. Notifications
  (`tauri-plugin-notification`, push through APNs/FCM) and deep links
  (`tauri-plugin-deep-link`, with associated domains / app links for
  `https` links) work on mobile too. A QR scanner needs a plugin such as
  `tauri-plugin-barcode-scanner`.
- Icons: `tauri icon` also writes iOS, Android and Windows Store icons.
  `npm run icons` deletes them (the Store icons are unused: no MSIX
  package) because `icons/hame.svg` has transparent margins, which
  suit macOS but would show as a white border on iOS. Give mobile a
  full-bleed variant, through `tauri icon`'s manifest (`default`,
  `bg_color`, `android_fg`).

## Not done

- **Windows and Linux builds are not run yet.** The code for both is
  written (sandbox helper, single instance, link registration, tray click
  to open, no menu bar). Hand the Windows build and run to the Windows
  machine (`docs/WINDOWS.md`).
- **Sign-in and host mode** (relay, pairing): later, with `crates/hame-sync`.
- **Quitting from the Dock's menu or at logout** goes through macOS
  directly and does not ask. Running turns are still stopped and logged as
  cancelled.
- **Notification clicks** are reported by the notification server. Windows
  reports them only while the toast is on screen, not from the Action
  Center. Unsigned or unbundled builds may show notifications under
  another app's name. On macOS the first notification makes the system
  ask whether hame may notify, and notifications are shown only once the
  user allows them.
- **Notification threads.** With a click handler registered,
  `tauri-plugin-notification` keeps one OS thread per shown notification
  until the user acts on it or it goes away (on macOS also polling the
  delivered notifications every 0.5 s on the main run loop), and cannot
  remove a shown notification on macOS or Windows. The app therefore shows
  at most one notification per session until the user comes back to the
  window, and remembers at most 32 for their clicks. Worth an issue
  upstream.
- **Two copies of the app on one Mac.** A `hame://` link goes to whichever
  registered copy LaunchServices prefers. If that is not the running one,
  it starts, hands over to the running instance and exits, and macOS
  hands the link to the copy that started, so the session is not opened
  (the window still comes to the front). With one installed copy this does
  not happen.
- **Splash.** The window opens after the server starts (well under a
  second), with no splash screen.
- **No signing, notarization, update server or keys**: see above.

## Dependencies

All are Apache-2.0 OR MIT, Tauri's own crates and npm package:

| Package | Version | Why |
|---|---|---|
| tauri, tauri-build | 2.12, 2.7 | the app shell (features `tray-icon`, `image-png`) |
| tauri-plugin-single-instance | 2.5 | one instance; forwards links |
| tauri-plugin-deep-link | 2.6 | `hame://` |
| tauri-plugin-notification | 2.8 | notifications and their clicks |
| tauri-plugin-dialog | 2.8 | folder picker, quit confirmation |
| tauri-plugin-autostart | 2.7 | start at login |
| tauri-plugin-window-state | 2.5 | window size and position |
| tauri-plugin-updater | 2.14 | updates, `updater` feature only |
| tauri-plugin-opener | 2.5 | links in the default browser (its Rust function only; the plugin is not registered) |
| @tauri-apps/cli (npm, dev only) | 2.12.4 | `tauri dev`, `tauri build`, `tauri icon` |

Also `getrandom` 0.3 (MIT OR Apache-2.0), `tokio`, `serde_json`,
`url` (MIT OR Apache-2.0), already used by the core crates. Their
transitive dependencies are mostly MIT and/or Apache-2.0, plus BSD-2/3-Clause,
ISC, Zlib, Unicode-3.0 (ICU data), and MPL-2.0 (file-level copyleft, used
unmodified) for `cssparser`, `dtoa-short`, `option-ext` and the `im-rc`
crates that Tauri's build tools pull in. `src-tauri/Cargo.lock` pins exact
versions.
