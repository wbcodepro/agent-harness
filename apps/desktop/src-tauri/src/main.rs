// No console window next to the app on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    // hame runs this executable for its sandboxed tool workers; those never
    // start the app.
    if let Some(code) = hame_desktop_app::internal_command() {
        return code;
    }
    hame_desktop_app::run();
    ExitCode::SUCCESS
}
