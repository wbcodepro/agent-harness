//! The window, the tray (menu bar) icon and the app menu.

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt as _;
use url::Url;

use crate::bridge;

/// The label of the one window.
pub const MAIN: &str = "main";
const TRAY: &str = "tray";

/// Menu item ids, shared by the tray menu and the app menu.
pub mod item {
    pub const OPEN: &str = "open";
    pub const NEW: &str = "new";
    pub const QUIT: &str = "quit";
    pub const AUTOSTART: &str = "autostart";
    pub const RELOAD: &str = "reload";
    pub const HELP: &str = "help";
}

const REPOSITORY: &str = "https://github.com/wbcodepro/agent-harness";

/// Whether `url` is on the in-process server's origin.
pub fn same_origin(url: &Url, origin: &str) -> bool {
    url.origin().ascii_serialization() == origin
}

/// Whether a link the page cannot show may go to the default browser: web
/// and mail links only, never `file:` or custom schemes.
pub fn may_open_outside(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "mailto")
}

/// Opens a link the page cannot show in the default browser. (The opener
/// plugin itself is not registered: its script would catch every
/// `target=_blank` click and call a command the page may not use.)
fn open_outside(url: &Url) {
    if !may_open_outside(url) {
        return;
    }
    // The smoke test checks that a click on a link in a reply gets here,
    // without opening a browser.
    if cfg!(feature = "smoke") {
        eprintln!("hame desktop: would open {url}");
        return;
    }
    if let Err(e) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
        eprintln!("hame desktop: cannot open {url}: {e}");
    }
}

/// Creates the window on the interface at `origin`, hidden until the saved
/// size and position are restored, then shown unless `hidden` (started at
/// login). `loading` runs whenever a page load begins.
pub fn create_window<R: Runtime>(
    app: &AppHandle<R>,
    origin: &str,
    token: &str,
    hidden: bool,
    loading: impl Fn(&AppHandle<R>) + Send + Sync + 'static,
) -> tauri::Result<WebviewWindow<R>> {
    let url: Url = format!("{origin}/")
        .parse()
        .map_err(|e: url::ParseError| tauri::Error::Anyhow(e.into()))?;
    let version = app.package_info().version.to_string();
    let navigation_origin = origin.to_string();
    let builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(url))
        .title("hame")
        .inner_size(1280.0, 820.0)
        .min_inner_size(390.0, 480.0)
        .visible(false)
        // Files dropped on the page reach the composer as ordinary drops.
        .disable_drag_drop_handler()
        .initialization_script(bridge::init_script(origin, token, &version))
        // The page stays on the server's origin; links elsewhere open in the
        // browser.
        .on_navigation(move |url| {
            if same_origin(url, &navigation_origin) {
                return true;
            }
            open_outside(url);
            false
        })
        .on_new_window(|url, _features| {
            open_outside(&url);
            NewWindowResponse::Deny
        })
        .on_page_load(move |webview, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Started) {
                loading(webview.app_handle());
            }
        });
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(18.0, 30.0));
    let window = builder.build()?;
    if !hidden {
        window.show()?;
        window.set_focus()?;
    }
    Ok(window)
}

/// Shows the window and brings it to the front.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    let window = app.get_webview_window(MAIN)?;
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    Some(window)
}

/// Whether the user is looking at the window.
pub fn in_front<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.get_webview_window(MAIN).is_some_and(|window| {
        window.is_visible().unwrap_or(false)
            && window.is_focused().unwrap_or(false)
            && !window.is_minimized().unwrap_or(false)
    })
}

/// The tray (Windows, Linux) or menu bar (macOS) icon.
pub fn tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, item::OPEN, "Open hame", true, None::<&str>)?,
            &MenuItem::with_id(app, item::NEW, "New Session", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &CheckMenuItem::with_id(
                app,
                item::AUTOSTART,
                "Start at Login",
                true,
                autostart,
                None::<&str>,
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, item::QUIT, "Quit hame", true, None::<&str>)?,
        ],
    )?;
    let builder = TrayIconBuilder::with_id(TRAY)
        .tooltip("hame")
        .menu(&menu)
        // macOS opens a menu bar icon's menu on a click; Windows and Linux
        // open the window and keep the menu on the right button.
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
                && !cfg!(target_os = "macos")
            {
                show_main(tray.app_handle());
            }
        });
    #[cfg(target_os = "macos")]
    let builder = builder
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/tray-template.png"
        ))?)
        .icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let builder = match app.default_window_icon() {
        Some(icon) => builder.icon(icon.clone()),
        None => builder,
    };
    builder.build(app)?;
    Ok(())
}

/// Says in the tray's tooltip how many turns are running.
pub fn show_running<R: Runtime>(app: &AppHandle<R>, running: usize) {
    if let Some(tray) = app.tray_by_id(TRAY) {
        let tooltip = match running {
            0 => "hame".to_string(),
            1 => "hame: 1 turn running".to_string(),
            n => format!("hame: {n} turns running"),
        };
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

/// Turns start at login on or off, from the tray's check item.
pub fn toggle_autostart<R: Runtime>(app: &AppHandle<R>) {
    let manager = app.autolaunch();
    let result = if manager.is_enabled().unwrap_or(false) {
        manager.disable()
    } else {
        manager.enable()
    };
    if let Err(e) = result {
        eprintln!("hame desktop: cannot change start at login: {e}");
    }
}

pub fn open_help() {
    if let Ok(url) = Url::parse(REPOSITORY) {
        open_outside(&url);
    }
}

/// The macOS menu bar. Windows and Linux have none (the window keeps the
/// whole height for the interface; the tray has the app's actions). Quit
/// is the app's own item, so it can ask first when turns are running.
#[cfg(target_os = "macos")]
pub fn app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    use tauri::menu::{AboutMetadata, Submenu};

    let about = AboutMetadata {
        name: Some("hame".into()),
        version: Some(app.package_info().version.to_string()),
        copyright: Some("MIT License".into()),
        website: Some(REPOSITORY.into()),
        website_label: Some("Source code".into()),
        ..AboutMetadata::default()
    };
    let hame = Submenu::with_items(
        app,
        "hame",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About hame"), Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some("Hide hame"))?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, item::QUIT, "Quit hame", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &MenuItem::with_id(app, item::NEW, "New Session", true, Some("CmdOrCtrl+N"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &MenuItem::with_id(app, item::RELOAD, "Reload", true, Some("CmdOrCtrl+R"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, Some("Zoom"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, item::OPEN, "hame", true, None::<&str>)?,
        ],
    )?;
    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[&MenuItem::with_id(
            app,
            item::HELP,
            "hame on GitHub",
            true,
            None::<&str>,
        )?],
    )?;
    Menu::with_items(app, &[&hame, &file, &edit, &view, &window, &help])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_mail_links_leave_for_the_browser() {
        for allowed in ["https://example.com/x", "http://a.b/", "mailto:a@b.c"] {
            assert!(may_open_outside(&Url::parse(allowed).unwrap()), "{allowed}");
        }
        for refused in ["file:///etc/passwd", "hame://new", "javascript:alert(1)"] {
            assert!(
                !may_open_outside(&Url::parse(refused).unwrap()),
                "{refused}"
            );
        }
    }

    #[test]
    fn only_the_servers_origin_is_shown_in_the_window() {
        let origin = "http://127.0.0.1:4100";
        for inside in [
            "http://127.0.0.1:4100/",
            "http://127.0.0.1:4100/#/s/abc",
            "http://127.0.0.1:4100/assets/x.js?y=1",
        ] {
            assert!(
                same_origin(&Url::parse(inside).unwrap(), origin),
                "{inside}"
            );
        }
        for outside in [
            "http://127.0.0.1:4101/",
            "http://localhost:4100/",
            "https://127.0.0.1:4100/",
            "https://example.com/",
            "file:///etc/passwd",
            "http://127.0.0.1:4100@evil.example/",
        ] {
            assert!(
                !same_origin(&Url::parse(outside).unwrap(), origin),
                "{outside}"
            );
        }
    }
}
