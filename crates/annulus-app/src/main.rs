//! Annulus desktop binary: app state, view models, command registry, window, tab and pane management.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4. The UI is our own toolkit
//! in our own window ([ADR-0017](../../docs/adr/0017-native-ui.md)).

#![forbid(unsafe_code)]
// A release build is a window app: no console window next to it on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod command_palette;
mod connect;
mod forwards_screen;
mod groups_view;
mod import_dialog;
mod job;
mod keychain_screen;
mod local_pane;
mod logs_screen;
mod native;
mod password_screen;
mod sftp_view;
mod snippets_screen;
mod ssh_task;
mod sync_screen;
mod terminal_view;

use std::process::ExitCode;

/// Where saved data lives: the OS's per-user data folder (ADR-0007 §Data locations), or the
/// working directory if the OS reports none.
fn data_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "", "Annulus").map_or_else(
        || std::path::PathBuf::from("annulus-data"),
        |dirs| dirs.data_dir().to_path_buf(),
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let light = args.iter().any(|arg| arg == "--light");
    let ran = if args.iter().any(|arg| arg == "--preview") {
        native::run_preview(light)
    } else {
        native::run_native(light, &data_dir())
    };
    if ran.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
