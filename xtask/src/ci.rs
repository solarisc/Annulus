//! `cargo xtask ci`: the exact set of checks the CI workflow runs on every OS.

use std::fmt::Write as _;
use std::process::Command;
use std::time::Instant;

use crate::{docs_check, repo};

/// One external command run by CI.
struct Step {
    name: &'static str,
    /// `None` runs the cargo that runs xtask.
    program: Option<&'static str>,
    args: &'static [&'static str],
}

const STEPS: &[Step] = &[
    Step {
        name: "fmt",
        program: None,
        args: &["fmt", "--all", "--check"],
    },
    Step {
        name: "clippy",
        program: None,
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            // One line per diagnostic, so a failing CI log reads in a few lines.
            "--message-format=short",
            "--",
            "-D",
            "warnings",
        ],
    },
    Step {
        name: "test",
        program: None,
        args: &["test", "--workspace", "--locked", "--no-fail-fast"],
    },
    Step {
        name: "deny",
        program: None,
        args: &["deny", "--locked", "check"],
    },
    Step {
        name: "machete",
        // Called directly: `cargo machete` forwards "machete" as a path to scan.
        program: Some("cargo-machete"),
        args: &["--with-metadata"],
    },
];

/// Runs every step, even after a failure, so one run shows every problem. Returns whether all
/// steps passed.
pub fn run() -> anyhow::Result<bool> {
    let root = repo::root()?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut results = Vec::new();

    for step in STEPS {
        println!("==> {}", step.name);
        let started = Instant::now();
        let program = step.program.unwrap_or(&cargo);
        let passed = match Command::new(program)
            .args(step.args)
            .current_dir(&root)
            .status()
        {
            Ok(status) => status.success(),
            Err(error) => {
                eprintln!("could not start `{program}`: {error}");
                false
            }
        };
        results.push((step.name, passed, started.elapsed().as_secs()));
    }

    println!("==> docs-check");
    let started = Instant::now();
    let passed = docs_check::run()?;
    results.push(("docs-check", passed, started.elapsed().as_secs()));

    write_step_summary(&results);
    let failed: Vec<_> = results
        .iter()
        .filter(|(_, passed, _)| !passed)
        .map(|(name, _, _)| *name)
        .collect();

    if failed.is_empty() {
        println!("ci: all steps passed");
        Ok(true)
    } else {
        eprintln!("ci: failed steps: {}", failed.join(", "));
        Ok(false)
    }
}

/// Appends a pass/fail table to the CI run page when the CI run has a step summary.
fn write_step_summary(results: &[(&str, bool, u64)]) {
    let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let mut table = String::from("| Step | Result | Seconds |\n|---|---|---|\n");
    for (name, passed, seconds) in results {
        let result = if *passed { "pass" } else { "**FAIL**" };
        let _ = writeln!(table, "| {name} | {result} | {seconds} |");
    }
    let appended = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, table.as_bytes()));
    if let Err(error) = appended {
        eprintln!("could not write the step summary to {path}: {error}");
    }
}
