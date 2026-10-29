//! `cargo xtask ci`: the exact set of checks the CI workflow runs on every OS.

use std::process::Command;

use anyhow::Context as _;

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
            "--",
            "-D",
            "warnings",
        ],
    },
    Step {
        name: "test",
        program: None,
        args: &["test", "--workspace", "--locked"],
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
    let mut failed = Vec::new();

    for step in STEPS {
        println!("==> {}", step.name);
        let program = step.program.unwrap_or(&cargo);
        let status = Command::new(program)
            .args(step.args)
            .current_dir(&root)
            .status()
            .with_context(|| format!("could not start `{program} {}`", step.args.join(" ")))?;
        if !status.success() {
            failed.push(step.name);
        }
    }

    println!("==> docs-check");
    if !docs_check::run()? {
        failed.push("docs-check");
    }

    if failed.is_empty() {
        println!("ci: all steps passed");
        Ok(true)
    } else {
        eprintln!("ci: failed steps: {}", failed.join(", "));
        Ok(false)
    }
}
