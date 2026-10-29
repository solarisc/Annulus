//! The crate graph follows the table in `docs/DESIGN.md` §4: the same set of crates, only the
//! allowed workspace dependencies, and `#![forbid(unsafe_code)]` everywhere but
//! `annulus-platform`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{Problem, Tree, markdown};
use crate::repo::Package;

const DESIGN: &str = "docs/DESIGN.md";
/// The one crate that may contain `unsafe` (CONTRIBUTING.md, principle 5).
const UNSAFE_CRATE: &str = "annulus-platform";
const SERVER: &str = "annulus-server";
const FORBID: &str = "#![forbid(unsafe_code)]";

/// What a crate may depend on, from the "May depend on" column.
#[derive(Debug, PartialEq, Eq)]
enum Allowed {
    /// "all client crates": any workspace crate except the server and xtask.
    AllClient,
    /// The crates named in backticks; empty for "none of ours" and "dev only".
    Only(BTreeSet<String>),
}

pub fn check(
    tree: &Tree,
    packages: &BTreeMap<String, Package>,
    root: &Path,
) -> anyhow::Result<Vec<Problem>> {
    let design = match tree.text(DESIGN) {
        Ok(text) => text,
        Err(problem) => return Ok(vec![problem]),
    };
    let mut problems = check_graph(&parse_table(design), packages);
    for package in packages
        .values()
        .filter(|package| package.name != UNSAFE_CRATE)
    {
        for crate_root in &package.crate_roots {
            let source = std::fs::read_to_string(crate_root)?;
            if !has_forbid_unsafe(&source) {
                let shown = crate_root.strip_prefix(root).unwrap_or(crate_root);
                problems.push(Problem::new(
                    &shown.to_string_lossy().replace('\\', "/"),
                    0,
                    format!("crate root must contain `{FORBID}`"),
                ));
            }
        }
    }
    Ok(problems)
}

fn has_forbid_unsafe(source: &str) -> bool {
    source.lines().any(|line| line.trim() == FORBID)
}

/// Rows of the §4 crate table: crate name → allowed dependencies.
fn parse_table(design: &str) -> BTreeMap<String, Allowed> {
    let mut table = BTreeMap::new();
    let mut in_section = false;
    for line in design.lines() {
        if line.starts_with("## ") {
            in_section = line.starts_with("## 4. Crates");
            continue;
        }
        if !in_section {
            continue;
        }
        let Some(cells) = markdown::table_cells(line) else {
            continue;
        };
        let (Some(name_cell), Some(allowed_cell)) = (cells.first(), cells.get(2)) else {
            continue;
        };
        let Some(name) = markdown::code_spans(name_cell).first().copied() else {
            continue;
        };
        let allowed = if allowed_cell.contains("all client crates") {
            Allowed::AllClient
        } else {
            Allowed::Only(
                markdown::code_spans(allowed_cell)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            )
        };
        table.insert(name.to_owned(), allowed);
    }
    table
}

fn check_graph(
    table: &BTreeMap<String, Allowed>,
    packages: &BTreeMap<String, Package>,
) -> Vec<Problem> {
    let mut problems = Vec::new();
    for name in table.keys().filter(|name| !packages.contains_key(*name)) {
        problems.push(Problem::new(
            DESIGN,
            0,
            format!("§4 lists `{name}`, which is not a workspace crate"),
        ));
    }
    for package in packages.values() {
        let Some(allowed) = table.get(&package.name) else {
            problems.push(Problem::new(
                DESIGN,
                0,
                format!("workspace crate `{}` is missing from §4", package.name),
            ));
            continue;
        };
        for dependency in package
            .dependencies
            .iter()
            .filter(|dep| packages.contains_key(*dep))
        {
            let permitted = match allowed {
                Allowed::AllClient => dependency != SERVER && dependency != "xtask",
                Allowed::Only(names) => names.contains(dependency),
            };
            if !permitted {
                problems.push(Problem::new(
                    DESIGN,
                    0,
                    format!(
                        "`{}` depends on `{dependency}`, which §4 does not allow",
                        package.name
                    ),
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
## 3. Overview
| `annulus-term` | stray | `annulus-vault` | no |
## 4. Crates
| Crate | Responsibility | May depend on | `unsafe` |
|---|---|---|---|
| `annulus-app` | binary | all client crates | no |
| `annulus-session` | sessions | `annulus-ssh`, `annulus-term` | no |
| `annulus-term` | VT | none of ours | no |
| `annulus-ssh` | ssh | none of ours | no |
| `annulus-server` | server | none of ours | no |
## 5. Threading
";

    fn package(name: &str, dependencies: &[&str]) -> (String, Package) {
        let dependencies = dependencies.iter().map(|d| (*d).to_owned()).collect();
        (
            name.to_owned(),
            Package {
                name: name.to_owned(),
                dependencies,
                crate_roots: Vec::new(),
            },
        )
    }

    fn messages(packages: Vec<(String, Package)>) -> Vec<String> {
        let packages = packages.into_iter().collect();
        check_graph(&parse_table(TABLE), &packages)
            .into_iter()
            .map(|p| p.message)
            .collect()
    }

    fn all(app_deps: &[&str], term_deps: &[&str]) -> Vec<(String, Package)> {
        vec![
            package("annulus-app", app_deps),
            package("annulus-session", &["annulus-ssh", "annulus-term", "tokio"]),
            package("annulus-term", term_deps),
            package("annulus-ssh", &[]),
            package("annulus-server", &[]),
        ]
    }

    #[test]
    fn only_rows_in_section_4_are_read() {
        let table = parse_table(TABLE);
        assert_eq!(
            table.get("annulus-term"),
            Some(&Allowed::Only(BTreeSet::new()))
        );
        assert_eq!(table.len(), 5);
    }

    #[test]
    fn allowed_dependencies_pass_and_external_crates_are_ignored() {
        assert_eq!(
            messages(all(&["annulus-session", "annulus-term"], &["vte"])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn pure_crate_depending_on_a_workspace_crate_fails() {
        assert_eq!(
            messages(all(&[], &["annulus-session"])),
            ["`annulus-term` depends on `annulus-session`, which §4 does not allow"]
        );
    }

    #[test]
    fn app_may_not_depend_on_the_server() {
        assert_eq!(messages(all(&["annulus-server"], &[])).len(), 1);
    }

    #[test]
    fn crate_missing_from_the_table_or_the_workspace_fails() {
        let mut packages = all(&[], &[]);
        packages.retain(|(name, _)| name != "annulus-ssh");
        packages.push(package("annulus-extra", &[]));
        let found = messages(packages);
        assert!(
            found.contains(&"§4 lists `annulus-ssh`, which is not a workspace crate".to_owned())
        );
        assert!(found.contains(&"workspace crate `annulus-extra` is missing from §4".to_owned()));
    }

    #[test]
    fn forbid_attribute_mentioned_in_prose_does_not_count() {
        assert!(has_forbid_unsafe("//! Docs\n\n#![forbid(unsafe_code)]\n"));
        assert!(!has_forbid_unsafe(
            "//! Mentions #![forbid(unsafe_code)] in prose\n"
        ));
    }
}
