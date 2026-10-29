//! Annulus desktop binary: app state, view models, command registry, window, tab and pane management.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4. The UI is egui run by
//! eframe ([ADR-0015](../../docs/adr/0015-egui-instead-of-hand-built-ui.md)).

#![forbid(unsafe_code)]

mod forwards_screen;
mod hosts_screen;
mod lock_screen;
mod sftp_view;
mod shell;
mod ssh_dialogs;
mod terminal_view;

use std::process::ExitCode;

use eframe::egui;

/// The eframe app: the window's shell.
struct AnnulusApp {
    shell: shell::Shell,
}

impl eframe::App for AnnulusApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
    }
}

/// Where saved data lives: the OS's per-user data folder (ADR-0007 §Data locations), or the
/// working directory if the OS reports none.
fn data_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "", "Annulus").map_or_else(
        || std::path::PathBuf::from("annulus-data"),
        |dirs| dirs.data_dir().to_path_buf(),
    )
}

fn main() -> ExitCode {
    let data_dir = data_dir();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Annulus")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 400.0]),
        ..eframe::NativeOptions::default()
    };
    let started = eframe::run_native(
        "Annulus",
        options,
        Box::new(|_| {
            Ok(Box::new(AnnulusApp {
                shell: shell::Shell::new(&data_dir),
            }))
        }),
    );
    if started.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
