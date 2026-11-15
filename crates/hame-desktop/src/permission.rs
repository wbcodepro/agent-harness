//! The macOS permissions desktop control needs, checked before anything is
//! sent.
//!
//! Accessibility (reading and operating other apps' windows) and Screen
//! Recording (screenshots) are granted per app in System Settings, and
//! macOS charges them to the app the user started: the terminal that runs
//! `hame`, or the hame desktop app. The helper reports both in its
//! handshake; an operation that needs one that is missing is refused at
//! once with `permission_denied` naming that app, instead of running and
//! timing out.
//!
//! An app that embeds hame (the desktop app) registers itself with
//! [`embed`]: its name for the messages, and a hook that hears about each
//! refusal so it can ask macOS to prompt and show where to grant it.

use std::sync::OnceLock;

use crate::protocol::HelperError;

/// A permission desktop control needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    Accessibility,
    ScreenRecording,
}

impl Permission {
    /// Where it is granted, under System Settings.
    pub fn settings_path(self) -> &'static str {
        match self {
            Permission::Accessibility => "Privacy & Security > Accessibility",
            Permission::ScreenRecording => "Privacy & Security > Screen & System Audio Recording",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Permission::Accessibility => "Accessibility",
            Permission::ScreenRecording => "Screen Recording",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Permission::Accessibility => "accessibility",
            Permission::ScreenRecording => "screen_recording",
        }
    }
}

/// Who macOS charges when nothing embeds hame.
pub const DEFAULT_HOST: &str = "the terminal or app that runs hame";

type Hook = Box<dyn Fn(Permission) + Send + Sync>;

struct Embedding {
    host: String,
    hook: Hook,
}

static EMBEDDING: OnceLock<Embedding> = OnceLock::new();

/// Registers the app hame runs inside: `host` names it in permission
/// messages (as the user finds it in System Settings), and `hook` runs on
/// every refusal for lack of a permission (on the tool's thread; keep it
/// short). Takes effect once; returns whether it did.
pub fn embed(host: impl Into<String>, hook: impl Fn(Permission) + Send + Sync + 'static) -> bool {
    EMBEDDING
        .set(Embedding {
            host: host.into(),
            hook: Box::new(hook),
        })
        .is_ok()
}

/// Who macOS charges for this process's permissions.
pub fn host() -> &'static str {
    EMBEDDING
        .get()
        .map_or(DEFAULT_HOST, |embedding| embedding.host.as_str())
}

pub(crate) fn report(permission: Permission) {
    if let Some(embedding) = EMBEDDING.get() {
        (embedding.hook)(permission);
    }
}

/// The permission a helper operation needs, if any. Operations that only
/// list apps, read the clipboard, name folders or script an app's own
/// dictionary need none.
pub fn needed_for(op: &str) -> Option<Permission> {
    match op {
        "windows" | "window_state" | "focus_context" | "hit_test" | "script_document"
        | "observe" | "resolve" | "press" | "set_value" | "focus" | "select" | "menu" | "type"
        | "key" | "click_at" | "wait" => Some(Permission::Accessibility),
        "screenshot" => Some(Permission::ScreenRecording),
        _ => None,
    }
}

/// The refusal for a missing permission.
pub fn denied(permission: Permission) -> HelperError {
    let mut error = HelperError::new(
        "permission_denied",
        format!(
            "{} permission is not granted to {}. Grant it in System Settings > {}, then try \
             again. If it is listed and switched on already, switch it off and on (an \
             updated or rebuilt app needs the grant again). Nothing was sent.",
            permission.label(),
            host(),
            permission.settings_path(),
        ),
    );
    error.details = serde_json::json!({"permission": permission.detail()});
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_that_read_windows_need_accessibility_and_screenshots_screen_recording() {
        for op in [
            "observe",
            "press",
            "type",
            "script_document",
            "wait",
            "windows",
        ] {
            assert_eq!(needed_for(op), Some(Permission::Accessibility), "{op}");
        }
        assert_eq!(needed_for("screenshot"), Some(Permission::ScreenRecording));
        for op in [
            "hello",
            "permissions",
            "apps",
            "app_info",
            "frontmost",
            "clipboard",
            "script_save",
            "display_names",
        ] {
            assert_eq!(needed_for(op), None, "{op}");
        }
    }

    #[test]
    fn a_refusal_says_where_to_grant_and_that_nothing_was_sent() {
        let error = denied(Permission::Accessibility);
        assert_eq!(error.code, "permission_denied");
        assert!(error.message.contains("Privacy & Security > Accessibility"));
        assert!(error.message.contains(host()));
        assert!(error.message.contains("Nothing was sent"));
        assert_eq!(error.details["permission"], "accessibility");
        let error = denied(Permission::ScreenRecording);
        assert!(error.message.contains("Screen & System Audio Recording"));
    }
}
