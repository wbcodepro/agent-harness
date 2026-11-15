//! Accessibility and Screen Recording for hame's desktop tool.
//!
//! macOS charges these permissions to the app the user started, here
//! hame.app (the helper the tool runs is its child). When the tool is
//! refused for lack of one, the app asks macOS to prompt for it (which also
//! lists hame in System Settings) and explains where to switch it on, once
//! per permission and launch. A grant of Accessibility is seen by the
//! running helper; for Screen Recording, which macOS reads once per
//! process, the tool has a separate helper look on a later screenshot.

use std::collections::HashSet;
use std::sync::Mutex;

use hame_desktop::permission::Permission;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt as _, MessageDialogButtons, MessageDialogKind};

/// How the tool's messages name this app.
pub const HOST: &str = "hame (the desktop app)";

/// The System Settings pane for a permission.
pub fn settings_url(permission: Permission) -> &'static str {
    match permission {
        Permission::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        Permission::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
    }
}

/// The explanation shown with the "Open Settings" button.
pub fn explanation(permission: Permission) -> String {
    let (what, needs) = match permission {
        Permission::Accessibility => (
            "Accessibility",
            "to read and operate other apps' windows (TextEdit, Numbers, Pages, …)",
        ),
        Permission::ScreenRecording => ("Screen Recording", "to take screenshots of windows"),
    };
    format!(
        "hame's desktop tool needs {what} permission {needs}. macOS asks for it for hame \
         itself, not for a terminal.\n\nSwitch on hame in System Settings > {}. If hame is \
         listed and switched on already, switch it off and on again, or remove it with − and \
         add it again: a grant made for an earlier build of hame does not count for this \
         one.\n\nThen ask hame to try again.",
        permission.settings_path()
    )
}

/// Permissions already asked about in this launch.
#[derive(Default)]
pub struct Asked(Mutex<HashSet<Permission>>);

impl Asked {
    /// True the first time `permission` is asked about.
    pub fn first(&self, permission: Permission) -> bool {
        self.0.lock().expect("asked").insert(permission)
    }
}

/// Registers the app with the desktop tool: its name in messages, and the
/// hook that runs when the tool is refused a permission.
pub fn register(app: &AppHandle) {
    let handle = app.clone();
    hame_desktop::permission::embed(HOST, move |permission| refused(&handle, permission));
}

fn refused(app: &AppHandle, permission: Permission) {
    let Some(asked) = app.try_state::<Asked>() else {
        return;
    };
    if !asked.first(permission) {
        return;
    }
    // macOS's own prompt, which also adds hame to the list in Settings.
    let _ = app.run_on_main_thread(move || system_prompt(permission));
    let mut dialog = app
        .dialog()
        .message(explanation(permission))
        .title(format!("hame needs {} permission", permission.label()))
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Open Settings".into(),
            "Not Now".into(),
        ));
    if let Some(window) = app.get_webview_window(crate::shell::MAIN)
        && window.is_visible().unwrap_or(false)
    {
        dialog = dialog.parent(&window);
    }
    dialog.show(move |open| {
        if open
            && let Err(e) = tauri_plugin_opener::open_url(settings_url(permission), None::<&str>)
        {
            eprintln!("hame desktop: cannot open System Settings: {e}");
        }
    });
}

#[cfg(target_os = "macos")]
fn system_prompt(permission: Permission) {
    match permission {
        Permission::Accessibility => macos::prompt_accessibility(),
        Permission::ScreenRecording => macos::request_screen_capture(),
    }
}

#[cfg(not(target_os = "macos"))]
fn system_prompt(_permission: Permission) {}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    //! The two system calls that make macOS prompt for a permission.
    use std::ffi::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
        static kAXTrustedCheckOptionPrompt: *const c_void;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> u8;
        fn CGRequestScreenCaptureAccess() -> u8;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }

    /// Asks whether this app is trusted for Accessibility, with macOS
    /// prompting (and listing the app in Settings) when it is not.
    pub fn prompt_accessibility() {
        // SAFETY: the keys and values are CoreFoundation constants that live
        // for the whole process; the dictionary is created with the standard
        // CFType callbacks, used for this one call and released after it.
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let options = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );
            if options.is_null() {
                return;
            }
            AXIsProcessTrustedWithOptions(options);
            CFRelease(options);
        }
    }

    /// Asks macOS to prompt for Screen Recording (once; later calls do
    /// nothing), unless it is granted.
    pub fn request_screen_capture() {
        // SAFETY: plain CoreGraphics calls without arguments.
        unsafe {
            if CGPreflightScreenCaptureAccess() == 0 {
                CGRequestScreenCaptureAccess();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_permission_opens_its_own_pane() {
        assert!(settings_url(Permission::Accessibility).ends_with("Privacy_Accessibility"));
        assert!(settings_url(Permission::ScreenRecording).ends_with("Privacy_ScreenCapture"));
    }

    #[test]
    fn the_explanation_names_the_pane_and_the_stale_grant_case() {
        let text = explanation(Permission::Accessibility);
        assert!(text.contains("Privacy & Security > Accessibility"));
        assert!(text.contains("earlier build"));
        assert!(text.contains("not for a terminal"));
        let text = explanation(Permission::ScreenRecording);
        assert!(text.contains("Screen & System Audio Recording"));
    }

    #[test]
    fn each_permission_is_asked_about_once() {
        let asked = Asked::default();
        assert!(asked.first(Permission::Accessibility));
        assert!(!asked.first(Permission::Accessibility));
        assert!(asked.first(Permission::ScreenRecording));
    }
}
