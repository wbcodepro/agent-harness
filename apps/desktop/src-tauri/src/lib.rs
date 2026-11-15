//! hame's desktop app.
//!
//! It runs the hame web server (`crates/hame-web`) in this process on
//! 127.0.0.1, with a token made fresh at every launch, and shows its
//! interface in the system web view. Closing the window hides it while turns
//! keep running; the tray (menu bar) icon opens it again and quits. The app
//! notifies when a turn ends or needs approval while the window is not in
//! front, opens `hame://` links, can start at login, and remembers the
//! window's size and position. See `apps/desktop/README.md`.

mod bridge;
mod deeplink;
mod helper;
mod notice;
mod quit;
mod shell;
mod token;
mod updater;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use hame_web::{Activity, WebServer};
use serde_json::json;
use tauri::ipc::CapabilityBuilder;
use tauri::{AppHandle, Manager, RunEvent, WebviewWindow, WindowEvent};
use tauri_plugin_deep_link::DeepLinkExt as _;
use tauri_plugin_dialog::{DialogExt as _, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::NotificationExt as _;

pub use helper::internal_command;

use deeplink::DeepLink;
use shell::MAIN;

/// How long quitting waits for stopped turns to record that they stopped.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// Set to a file path, the app writes the server's address and token there
/// (mode 0600) once it serves. For the smoke test of a built app; never set
/// in normal use.
const INFO_FILE_ENV: &str = "HAME_DESKTOP_INFO_FILE";

/// What the app keeps while it runs.
struct Desktop {
    server: WebServer,
    /// A quit confirmation is showing, or the app is quitting.
    quitting: AtomicBool,
    /// The app decided to exit; the exit request that follows goes through.
    may_exit: AtomicBool,
    /// The session each shown notification is about, by notification id.
    notified: Mutex<HashMap<i32, String>>,
    next_notification: AtomicI32,
    /// Whether the user was told the app keeps running when the window
    /// closes (Windows and Linux, once per launch).
    told_hidden: AtomicBool,
}

/// hame's data directory: `$HAME_HOME`, else `~/.hame` (or `~/.zcode`
/// from before the rename, where it still is), as for the `hame` command.
fn hame_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HAME_HOME") {
        return Some(PathBuf::from(home));
    }
    let user = std::env::home_dir()?;
    let home = user.join(".hame");
    let before = user.join(".zcode");
    if !home.exists() && before.is_dir() {
        return Some(before);
    }
    Some(home)
}

/// The folder new sessions work in unless the user picks another: the
/// user's home folder.
fn default_folder() -> Result<PathBuf, String> {
    let user = std::env::home_dir().ok_or("cannot find your home folder")?;
    hame_kernel::real_path(&user).map_err(|e| e.to_string())
}

/// The port the server used last time, so the interface's own storage
/// (theme, drafts, what was read), which the web view keeps per origin,
/// survives a restart. Any free port when it is taken.
fn saved_port(file: &Path) -> u16 {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|port| *port >= 1024)
        .unwrap_or(0)
}

async fn start_server(home: PathBuf, cwd: PathBuf, port: u16) -> Result<WebServer, String> {
    let options = || hame_app::Options {
        max_steps: 0,
        ..hame_app::Options::default()
    };
    let token = token::new_token();
    if port != 0
        && let Ok(server) =
            hame_web::start_with_token(home.clone(), cwd.clone(), options(), port, token.clone())
                .await
    {
        return Ok(server);
    }
    hame_web::start_with_token(home, cwd, options(), 0, token).await
}

fn write_info_file(server: &WebServer) {
    let Some(path) = std::env::var_os(INFO_FILE_ENV) else {
        return;
    };
    let info = json!({
        "pid": std::process::id(),
        "addr": server.addr.to_string(),
        "token": server.token,
    });
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, info.to_string().as_bytes()));
    if let Err(e) = written {
        eprintln!("hame desktop: cannot write {}: {e}", path.to_string_lossy());
    }
}

/// What the page may call: the folder picker, the ready signal, and moving
/// the window by its title area. Only from the server's exact origin.
fn capability(origin: &str) -> CapabilityBuilder {
    CapabilityBuilder::new("interface")
        .remote(format!("{origin}/*"))
        .local(false)
        .window(MAIN)
        .permission("allow-pick-folder")
        .permission("allow-desktop-ready")
        .permission("core:window:allow-start-dragging")
        .permission("core:window:allow-internal-toggle-maximize")
}

/// Lets the user choose the working folder for a new session.
#[tauri::command]
async fn pick_folder(window: WebviewWindow, start: Option<String>) -> Option<String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut dialog = window
        .dialog()
        .file()
        .set_title("Choose the folder hame works in")
        .set_parent(&window);
    if let Some(start) = start.filter(|start| Path::new(start).is_dir()) {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder(move |folder| {
        let _ = sender.send(folder);
    });
    let folder = receiver.await.ok().flatten()?.into_path().ok()?;
    Some(folder.to_string_lossy().into_owned())
}

/// The page loaded and reached the shell.
#[tauri::command]
fn desktop_ready() {
    eprintln!("hame desktop: the interface is ready");
}

fn open_link(app: &AppHandle, link: DeepLink) {
    match link {
        DeepLink::Open { session: None } => {
            shell::show_main(app);
        }
        DeepLink::Open {
            session: Some(session),
        } => shell::tell_page(app, json!({"action": "open", "session": session})),
        DeepLink::New => shell::tell_page(app, json!({"action": "new"})),
    }
}

fn session_of(activity: &Activity) -> &str {
    match activity {
        Activity::TurnStarted { session }
        | Activity::TurnEnded { session, .. }
        | Activity::Approval { session, .. } => session,
    }
}

/// Follows the server's activity: the tray's tooltip, and a notification
/// when a turn ends or asks while the window is not in front.
fn follow_activity(app: AppHandle, mut activity: tokio::sync::broadcast::Receiver<Activity>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let event = match activity.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            let Some(desktop) = app.try_state::<Desktop>() else {
                continue;
            };
            shell::show_running(&app, desktop.server.running().len());
            if shell::in_front(&app) {
                continue;
            }
            let title = desktop.server.title(session_of(&event));
            let Some(shown) = notice::notice(&event, title.as_deref()) else {
                continue;
            };
            let id = desktop.next_notification.fetch_add(1, Ordering::Relaxed);
            desktop
                .notified
                .lock()
                .expect("notified")
                .insert(id, shown.session.clone());
            if let Err(e) = app
                .notification()
                .builder()
                .id(id)
                .title(shown.title)
                .body(shown.body)
                .show()
            {
                eprintln!("hame desktop: cannot notify: {e}");
            }
        }
    });
}

/// Quits, first asking when turns are running.
fn request_quit(app: &AppHandle) {
    let Some(desktop) = app.try_state::<Desktop>() else {
        app.exit(0);
        return;
    };
    if desktop.quitting.swap(true, Ordering::SeqCst) {
        // Already asking, or already quitting.
        return;
    }
    let Some(asked) = quit::confirmation(&desktop.server.running()) else {
        exit_now(app);
        return;
    };
    let mut dialog = app
        .dialog()
        .message(asked.message)
        .title(asked.title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(asked.quit, asked.keep));
    if let Some(window) = shell::show_main(app) {
        dialog = dialog.parent(&window);
    }
    let handle = app.clone();
    dialog.show(move |quit| {
        if quit {
            exit_now(&handle);
        } else if let Some(desktop) = handle.try_state::<Desktop>() {
            desktop.quitting.store(false, Ordering::SeqCst);
        }
    });
}

/// Stops the running turns (so their logs say they were stopped), then
/// exits.
fn exit_now(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(desktop) = handle.try_state::<Desktop>() {
            desktop.server.stop_turns(STOP_WAIT).await;
            desktop.may_exit.store(true, Ordering::SeqCst);
        }
        handle.exit(0);
    });
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        shell::item::OPEN => {
            shell::show_main(app);
        }
        shell::item::NEW => shell::tell_page(app, json!({"action": "new"})),
        shell::item::QUIT => request_quit(app),
        shell::item::AUTOSTART => shell::toggle_autostart(app),
        shell::item::RELOAD => {
            if let Some(window) = app.get_webview_window(MAIN) {
                let _ = window.reload();
            }
        }
        shell::item::HELP => shell::open_help(),
        _ => {}
    }
}

fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != MAIN {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        // Closing hides: turns keep running, the tray brings it back.
        api.prevent_close();
        let _ = window.hide();
        let app = window.app_handle();
        if !cfg!(target_os = "macos")
            && let Some(desktop) = app.try_state::<Desktop>()
            && !desktop.told_hidden.swap(true, Ordering::Relaxed)
        {
            let _ = app
                .notification()
                .builder()
                .title("hame is still running")
                .body("Turns keep running. Open hame from its icon in the notification area; quit it there.")
                .show();
        }
    }
}

fn fail(app: &AppHandle, message: String) {
    eprintln!("hame desktop: {message}");
    let handle = app.clone();
    app.dialog()
        .message(message)
        .title("hame cannot start")
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));
}

fn setup(app: &mut tauri::App, updates: bool) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let (home, cwd) = match (hame_home(), default_folder()) {
        (Some(home), Ok(cwd)) => (home, cwd),
        (None, _) => {
            fail(
                &handle,
                "Cannot find your home folder. Set HAME_HOME.".into(),
            );
            return Ok(());
        }
        (_, Err(e)) => {
            fail(&handle, format!("Cannot use your home folder: {e}"));
            return Ok(());
        }
    };
    let port_file = app.path().app_data_dir()?.join("port");
    let server =
        match tauri::async_runtime::block_on(start_server(home, cwd, saved_port(&port_file))) {
            Ok(server) => server,
            Err(e) => {
                fail(&handle, format!("The hame server did not start: {e}"));
                return Ok(());
            }
        };
    if let Some(parent) = port_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&port_file, server.addr.port().to_string());
    let origin = format!("http://{}", server.addr);
    eprintln!("hame desktop: serving the interface on {origin}");
    write_info_file(&server);

    // A link the app was started with: a session to open first.
    let mut links: Vec<DeepLink> = app
        .deep_link()
        .get_current()
        .ok()
        .flatten()
        .unwrap_or_default()
        .iter()
        .filter_map(deeplink::parse)
        .collect();
    let fragment = match links.first() {
        Some(DeepLink::Open {
            session: Some(session),
        }) => format!("#/s/{session}"),
        _ => String::new(),
    };
    if !fragment.is_empty() {
        links.remove(0);
    }

    app.add_capability(capability(&origin))?;
    shell::create_window(&handle, &origin, &server.token, &fragment)?;
    shell::tray(&handle)?;

    let activity = server.activity();
    app.manage(Desktop {
        server,
        quitting: AtomicBool::new(false),
        may_exit: AtomicBool::new(false),
        notified: Mutex::new(HashMap::new()),
        next_notification: AtomicI32::new(1),
        told_hidden: AtomicBool::new(false),
    });
    follow_activity(handle.clone(), activity);

    // A click on a notification opens its session.
    let clicked = handle.clone();
    app.notification().on_action(move |performed| {
        let session = performed.notification().and_then(|shown| {
            clicked
                .try_state::<Desktop>()?
                .notified
                .lock()
                .expect("notified")
                .get(&shown.id())
                .cloned()
        });
        match session {
            Some(session) => {
                shell::tell_page(&clicked, json!({"action": "open", "session": session}))
            }
            None => {
                shell::show_main(&clicked);
            }
        }
    })?;

    // Links while running: macOS hands them to this process; on Windows and
    // Linux a second launch forwards them here (single instance).
    let linked = handle.clone();
    app.deep_link().on_open_url(move |event| {
        for link in event.urls().iter().filter_map(deeplink::parse) {
            open_link(&linked, link);
        }
    });
    #[cfg(any(target_os = "linux", all(windows, debug_assertions)))]
    if let Err(e) = app.deep_link().register_all() {
        eprintln!("hame desktop: cannot register hame:// links: {e}");
    }
    for link in links {
        open_link(&handle, link);
    }

    if updates {
        updater::check(&handle);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    let updates =
        updater::configured(&serde_json::to_value(&context.config().plugins.0).unwrap_or_default());

    let mut builder = tauri::Builder::default()
        // First, so a second launch only hands over its arguments.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Links among the arguments reach the deep-link plugin.
            shell::show_main(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        );
    if updates {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder.menu(shell::app_menu);
    }
    let app = builder
        .invoke_handler(tauri::generate_handler![pick_folder, desktop_ready])
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_window_event(on_window_event)
        .setup(move |app| setup(app, updates))
        .build(context)
        .expect("hame desktop: the app could not be built");
    app.run(|app, event| match event {
        RunEvent::ExitRequested { code, api, .. } => {
            let decided = app
                .try_state::<Desktop>()
                .is_none_or(|desktop| desktop.may_exit.load(Ordering::SeqCst));
            if code.is_none() && !decided {
                api.prevent_exit();
                request_quit(app);
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            shell::show_main(app);
        }
        RunEvent::Exit => {
            // Quitting some other way (the Dock's menu, logging out): still
            // stop the turns so their logs say so.
            if let Some(desktop) = app.try_state::<Desktop>() {
                tauri::async_runtime::block_on(desktop.server.stop_turns(STOP_WAIT));
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saved_port_is_used_when_it_is_sensible() {
        let dir = std::env::temp_dir().join(format!("hame-desktop-port-{}", token::new_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("port");
        assert_eq!(saved_port(&file), 0, "no file: any port");
        std::fs::write(&file, "48123\n").unwrap();
        assert_eq!(saved_port(&file), 48123);
        for bad in ["80", "abc", "70000", ""] {
            std::fs::write(&file, bad).unwrap();
            assert_eq!(saved_port(&file), 0, "{bad:?}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_the_page_on_the_servers_origin_may_call_the_shell() {
        // The capability is built for the exact origin, not a pattern for
        // any port.
        let tauri::utils::acl::capability::CapabilityFile::Capability(capability) =
            tauri::ipc::RuntimeCapability::build(capability("http://127.0.0.1:4100"))
        else {
            panic!("one capability");
        };
        let text = serde_json::to_string(&capability).unwrap();
        assert!(text.contains("\"http://127.0.0.1:4100/*\""), "{text}");
        assert!(text.contains("\"local\":false"), "{text}");
        assert!(text.contains("allow-pick-folder"));
        assert!(!text.contains("core:default"));
        assert!(!text.contains("127.0.0.1:*"));
    }
}
