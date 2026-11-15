//! Updates through `tauri-plugin-updater`, off unless configured.
//!
//! The repository carries no update endpoint and no key. A release build
//! turns updates on with the `updater` cargo feature and a configuration
//! with both merged into the Tauri configuration (`tauri build --features
//! updater --config updater.conf.json`, see the README); only then is the
//! plugin registered and an update looked for at launch.
//! Updates are signed with the release key, whose public half is in that
//! configuration, and the signature must name the announced version.

use serde_json::Value;

/// Whether the configuration names an update endpoint and a public key.
pub fn configured(plugins: &Value) -> bool {
    let Some(updater) = plugins.get("updater") else {
        return false;
    };
    let key = updater
        .get("pubkey")
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty());
    let endpoints = updater
        .get("endpoints")
        .and_then(Value::as_array)
        .is_some_and(|endpoints| {
            !endpoints.is_empty()
                && endpoints
                    .iter()
                    .all(|endpoint| endpoint.as_str().is_some_and(|e| e.starts_with("https://")))
        });
    key && endpoints
}

/// Looks for an update and offers it. `before_exit` runs before the
/// Windows installer takes over and ends the process.
#[cfg(feature = "updater")]
pub fn check<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    before_exit: impl Fn() + Send + Sync + 'static,
) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    use tauri_plugin_updater::UpdaterExt;

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let update = match app.updater_builder().on_before_exit(before_exit).build() {
            Ok(updater) => updater.check().await,
            Err(e) => Err(e),
        };
        let update = match update {
            Ok(Some(update)) => update,
            Ok(None) => return,
            Err(e) => {
                eprintln!("hame desktop: cannot check for updates: {e}");
                return;
            }
        };
        let version = update.version.clone();
        let handle = app.clone();
        app.dialog()
            .message(format!(
                "hame {version} is available (this is {}). Install it and restart? Running turns stop.",
                update.current_version
            ))
            .title("Update available")
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Install and Restart".into(),
                "Later".into(),
            ))
            .show(move |install| {
                if !install {
                    return;
                }
                tauri::async_runtime::spawn(async move {
                    match update.download_and_install(|_, _| {}, || {}).await {
                        Ok(()) => handle.restart(),
                        Err(e) => eprintln!("hame desktop: the update failed: {e}"),
                    }
                });
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn updates_need_a_key_and_an_https_endpoint() {
        assert!(!configured(&json!({})));
        assert!(!configured(&json!({"updater": {}})));
        assert!(!configured(&json!({"updater": {"pubkey": "k"}})));
        assert!(!configured(
            &json!({"updater": {"pubkey": " ", "endpoints": ["https://u.example/latest.json"]}})
        ));
        assert!(!configured(
            &json!({"updater": {"pubkey": "k", "endpoints": []}})
        ));
        assert!(!configured(
            &json!({"updater": {"pubkey": "k", "endpoints": ["http://u.example/latest.json"]}})
        ));
        assert!(configured(
            &json!({"updater": {"pubkey": "k", "endpoints": ["https://u.example/latest.json"]}})
        ));
    }
}
