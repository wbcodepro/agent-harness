//! Runs `helper_checks.py` against the embedded helper: redirect
//! resolution, safe file names, and saving and reading workspace files
//! without following links. Needs Python only, not Playwright or a browser.

use std::path::Path;
use std::process::Command;

#[test]
fn the_helper_saves_and_reads_files_without_following_links_and_resolves_redirects_like_chrome() {
    let Some(python) = hame_kernel::python_program() else {
        eprintln!("SKIPPED: no Python");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("browser_helper.py");
    std::fs::write(&helper, hame_browser::HELPER_SOURCE).unwrap();
    let checks = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/helper_checks.py");
    let out = Command::new(python)
        .arg("-I")
        .arg(&checks)
        .arg(&helper)
        .output()
        .expect("run python");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
