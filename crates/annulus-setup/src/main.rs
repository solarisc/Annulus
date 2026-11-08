//! `annulus-setup`: the Windows installer (ADR-0027). It puts Annulus and `annulus-agent` in
//! `%LOCALAPPDATA%\Programs\Annulus` for the current user, with a Start menu shortcut, the
//! folder on the user's PATH and an Apps & features entry; no administrator is needed. Its
//! copy in that folder, `uninstall.exe --uninstall`, takes them away again and leaves the
//! vault and settings. The programs travel inside it, deflated (`build.rs`). It builds on every
//! OS, so the workspace does; elsewhere than Windows it says so instead of installing.

#![forbid(unsafe_code)]
// A window, never a console behind it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod install;
mod ui;

mod payload {
    include!(concat!(env!("OUT_DIR"), "/payload.rs"));
}

use std::process::ExitCode;

fn main() -> ExitCode {
    let me = std::env::current_exe().unwrap_or_default();
    let remove = std::env::args().any(|arg| arg == "--uninstall");
    let mode = match me.parent() {
        Some(dir) if remove => ui::Mode::Remove(dir.to_path_buf()),
        _ => ui::Mode::Install,
    };
    let setup = ui::Setup::new(mode, me);
    let title = setup.title();
    match annulus_platform::window::run(title, (560.0, 480.0), setup) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
