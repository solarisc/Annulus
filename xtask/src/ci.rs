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

/// The external steps, cheapest first: a run stops at the first failure, so a formatting slip
/// fails in seconds instead of after the tests. `docs-check` runs in-process between `fmt` and
/// these.
const STEPS: &[Step] = &[
    Step {
        name: "machete",
        // Called directly: `cargo machete` forwards "machete" as a path to scan.
        program: Some("cargo-machete"),
        args: &["--with-metadata"],
    },
    Step {
        name: "deny",
        program: None,
        args: &["deny", "--locked", "check"],
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
        // Stops at the first test binary with a failure.
        args: &["test", "--workspace", "--locked"],
    },
];

const FMT: Step = Step {
    name: "fmt",
    program: None,
    args: &["fmt", "--all", "--check"],
};

/// How a step ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed,
    /// Not run: a step before it failed.
    Skipped,
}

/// Runs `steps` in order until one fails; the rest are skipped. Returns each step's name,
/// outcome and seconds.
fn run_in_order(
    steps: &mut [(&'static str, &mut dyn FnMut() -> bool)],
) -> Vec<(&'static str, Outcome, u64)> {
    let mut failed = false;
    steps
        .iter_mut()
        .map(|(name, step)| {
            if failed {
                return (*name, Outcome::Skipped, 0);
            }
            println!("==> {name}");
            let started = Instant::now();
            let outcome = if step() {
                Outcome::Passed
            } else {
                failed = true;
                Outcome::Failed
            };
            (*name, outcome, started.elapsed().as_secs())
        })
        .collect()
}

/// Runs the steps cheapest first and stops at the first failure. Returns whether all passed.
pub fn run() -> anyhow::Result<bool> {
    let root = repo::root()?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let external = |step: &Step| {
        let program = step.program.unwrap_or(&cargo);
        match Command::new(program)
            .args(step.args)
            .current_dir(&root)
            .status()
        {
            Ok(status) => status.success(),
            Err(error) => {
                eprintln!("could not start `{program}`: {error}");
                false
            }
        }
    };
    let mut docs = || {
        docs_check::run().unwrap_or_else(|error| {
            eprintln!("docs-check: {error:#}");
            false
        })
    };
    let mut fmt = || external(&FMT);
    let mut rest: Vec<Box<dyn FnMut() -> bool>> = STEPS
        .iter()
        .map(|step| Box::new(move || external(step)) as Box<dyn FnMut() -> bool>)
        .collect();
    let mut steps: Vec<(&'static str, &mut dyn FnMut() -> bool)> =
        vec![(FMT.name, &mut fmt), ("docs-check", &mut docs)];
    for (step, run) in STEPS.iter().zip(rest.iter_mut()) {
        steps.push((step.name, run.as_mut()));
    }
    let results = run_in_order(&mut steps);

    write_step_summary(&results);
    match results
        .iter()
        .find(|(_, outcome, _)| *outcome == Outcome::Failed)
    {
        None => {
            println!("ci: all steps passed");
            Ok(true)
        }
        Some((name, _, _)) => {
            eprintln!("ci: {name} failed; the steps after it were skipped");
            Ok(false)
        }
    }
}

/// Appends a pass/fail table to the CI run page when the CI run has a step summary.
fn write_step_summary(results: &[(&str, Outcome, u64)]) {
    let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let mut table = String::from("| Step | Result | Seconds |\n|---|---|---|\n");
    for (name, outcome, seconds) in results {
        let result = match outcome {
            Outcome::Passed => "pass",
            Outcome::Failed => "**FAIL**",
            Outcome::Skipped => "skipped",
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_failing_step_stops_the_run_and_the_rest_are_skipped() {
        let (mut a, mut b, mut c) = (|| true, || false, || panic!("never runs after a failure"));
        let results = run_in_order(&mut [("a", &mut a), ("b", &mut b), ("c", &mut c)]);
        let outcomes: Vec<_> = results
            .iter()
            .map(|(name, outcome, _)| (*name, *outcome))
            .collect();
        assert_eq!(
            outcomes,
            [
                ("a", Outcome::Passed),
                ("b", Outcome::Failed),
                ("c", Outcome::Skipped)
            ]
        );
    }

    #[test]
    fn the_cheap_checks_come_before_clippy_and_the_tests() {
        let names: Vec<_> = STEPS.iter().map(|step| step.name).collect();
        assert_eq!(names, ["machete", "deny", "clippy", "test"]);
        assert!(
            !STEPS
                .iter()
                .any(|step| step.args.contains(&"--no-fail-fast")),
            "the tests stop at the first failing binary"
        );
    }
}
