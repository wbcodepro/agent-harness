//! What the interface learns from the desktop shell, and how the shell
//! talks to the page.
//!
//! The web view loads the interface from the in-process server
//! (`http://127.0.0.1:<port>/`). Before the page's own scripts run, an
//! initialization script defines `window.__HAME_DESKTOP__`, a frozen object
//! with the API token, the platform and the app version. The token is never
//! in an address, so it is not in the web view's history, and the script
//! defines nothing on any other origin (the web view never navigates to
//! one, but this does not rely on that).
//!
//! The shell tells the page things by dispatching a `hame:desktop` DOM
//! event (`{action: "open", session}` or `{action: "new"}`).

use serde_json::json;

/// The platform name the interface reads.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// The initialization script for a page served from `origin`.
pub fn init_script(origin: &str, token: &str, version: &str) -> String {
    let origin = json!(origin);
    let bridge = json!({
        "token": token,
        "platform": platform(),
        "version": version,
    });
    format!(
        "(function () {{\n\
         if (window.location.origin !== {origin}) return;\n\
         var bridge = {bridge};\n\
         Object.defineProperty(window, \"__HAME_DESKTOP__\", {{\n\
         value: Object.freeze(bridge), writable: false, configurable: false, enumerable: false\n\
         }});\n\
         }})();"
    )
}

/// A script that hands `detail` to the page as a `hame:desktop` event.
pub fn event_script(detail: &serde_json::Value) -> String {
    format!("window.dispatchEvent(new CustomEvent(\"hame:desktop\", {{ detail: {detail} }}));")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_checks_the_origin_and_quotes_its_values() {
        let script = init_script("http://127.0.0.1:4100", "ab12", "0.1.0");
        assert!(script.contains("window.location.origin !== \"http://127.0.0.1:4100\""));
        assert!(script.contains("\"token\":\"ab12\""));
        assert!(script.contains(&format!("\"platform\":\"{}\"", platform())));
        assert!(script.contains("Object.freeze(bridge)"));
        // A hostile value stays a string.
        let script = init_script("http://x\"; alert(1); \"", "t", "v");
        assert!(script.contains(r#""http://x\"; alert(1); \"""#));
    }

    #[test]
    fn events_carry_json() {
        let script = event_script(&json!({"action": "open", "session": "a\"b"}));
        assert_eq!(
            script,
            "window.dispatchEvent(new CustomEvent(\"hame:desktop\", { detail: \
             {\"action\":\"open\",\"session\":\"a\\\"b\"} }));"
        );
    }
}
