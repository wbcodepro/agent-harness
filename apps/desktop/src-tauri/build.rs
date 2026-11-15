//! Tauri's build step. The app's own commands get permissions
//! (`allow-pick-folder`, `allow-desktop-ready`), which the capability the
//! app adds at launch grants to the interface's exact origin only.
//!
//! A release build for macOS also needs a signing identity: macOS ties
//! permission grants to the exact signature of an ad-hoc build, so an
//! ad-hoc release would lose them on every update. Without
//! `APPLE_SIGNING_IDENTITY` the build stops, unless `HAME_ALLOW_ADHOC=1`
//! says it is a local build (the `build:app` and `build:smoke` scripts set
//! it).
fn main() {
    println!("cargo:rerun-if-env-changed=APPLE_SIGNING_IDENTITY");
    println!("cargo:rerun-if-env-changed=HAME_ALLOW_ADHOC");
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    if macos && release {
        let identity = std::env::var("APPLE_SIGNING_IDENTITY").unwrap_or_default();
        let local = std::env::var("HAME_ALLOW_ADHOC").as_deref() == Ok("1");
        if (identity.trim().is_empty() || identity.trim() == "-") && !local {
            panic!(
                "a macOS release build needs APPLE_SIGNING_IDENTITY (a stable signing \
                 identity); for a local ad-hoc build use `npm run build:app`, or set \
                 HAME_ALLOW_ADHOC=1"
            );
        }
    }
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["pick_folder", "desktop_ready"]),
        ),
    )
    .expect("the Tauri build step failed");
}
