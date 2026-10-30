//! Developer automation for the Annulus workspace. CI runs `cargo xtask ci`, so local runs and CI
//! execute the same steps.

#![forbid(unsafe_code)]
// xtask is a CLI that reports to the terminal; CONTRIBUTING.md exempts it from the print lints.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod ci;
mod docs_check;
mod icon;
mod repo;
mod themes;

use std::process::ExitCode;

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  ci            fmt, clippy, tests, cargo deny, cargo machete, docs-check
  docs-check    links and anchors, ADR index, FEATURES test names, to-do markers,
                Last verified lines, crate dependency rules
  themes <dir>  regenerate the color scheme catalog from a tinted-theming/schemes checkout
  icon          redraw the app icon (annulus.ico and the exe's annulus.res) from our mark";

fn main() -> ExitCode {
    let command = std::env::args().nth(1);
    let result = match command.as_deref() {
        Some("ci") => ci::run(),
        Some("docs-check") => docs_check::run(),
        Some("icon") => icon::run(),
        Some("themes") => themes::run(std::env::args().nth(2).as_deref()),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("xtask: {error:#}");
            ExitCode::FAILURE
        }
    }
}
