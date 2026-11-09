//! Annulus desktop binary: app state, view models, command registry, window, tab and pane management.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4. The UI is our own toolkit
//! in our own window ([ADR-0017](../../docs/adr/0017-native-ui.md)).

#![forbid(unsafe_code)]
// A release build is a window app: no console window next to it on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_skill;
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

fn main() -> ExitCode {
    annulus_platform::process::harden();
    let args: Vec<String> = std::env::args().collect();
    let light = args.iter().any(|arg| arg == "--light");
    let ran = if args.iter().any(|arg| arg == "--preview") {
        native::run_preview(light)
    } else {
        // The same folder the `annulus-agent` command looks in for the control door.
        native::run_native(light, &annulus_control::data_dir())
    };
    if ran.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
