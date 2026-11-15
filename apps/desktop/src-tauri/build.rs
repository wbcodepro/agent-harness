//! Tauri's build step. The app's own commands get permissions
//! (`allow-pick-folder`, `allow-desktop-ready`), which the capability the
//! app adds at launch grants to the interface's exact origin only.
fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["pick_folder", "desktop_ready"]),
        ),
    )
    .expect("the Tauri build step failed");
}
