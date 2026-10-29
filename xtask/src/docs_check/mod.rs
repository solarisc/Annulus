//! `cargo xtask docs-check`: keeps the docs honest. Each check is a pure function over file
//! contents; [`run`] reads the repository and reports what they find.

mod adr_index;
mod crate_rules;
mod features;
mod last_verified;
mod links;
mod markdown;
mod todos;

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use anyhow::Context as _;

use crate::repo;

/// One thing wrong with the docs or the tree, pointing at a file and line.
#[derive(Debug, PartialEq, Eq)]
pub struct Problem {
    /// Path relative to the repository root, `/`-separated.
    pub file: String,
    /// 1-based line, or 0 when the problem concerns the whole file.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

impl Problem {
    fn new(file: &str, line: usize, message: impl Into<String>) -> Self {
        Self {
            file: file.to_owned(),
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}: {}", self.file, self.message)
        } else {
            write!(f, "{}:{}: {}", self.file, self.line, self.message)
        }
    }
}

/// The repository as the checks see it: text files by path.
pub struct Tree {
    /// Every UTF-8 text file, keyed by `/`-separated relative path.
    pub texts: BTreeMap<String, String>,
    /// Every file and directory path that exists.
    pub paths: HashSet<String>,
}

impl Tree {
    fn text(&self, path: &str) -> Result<&str, Problem> {
        self.texts
            .get(path)
            .map(String::as_str)
            .ok_or_else(|| Problem::new(path, 0, "missing"))
    }
}

/// Whether `path` ends in `.ext`, ignoring ASCII case.
fn has_extension(path: &str, ext: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|found| found.eq_ignore_ascii_case(ext))
}

/// Runs every check and prints the problems. Returns whether there were none.
pub fn run() -> anyhow::Result<bool> {
    let root = repo::root()?;
    let mut texts = BTreeMap::new();
    let mut paths = HashSet::new();
    for path in repo::files(&root)? {
        let mut dir = path.as_str();
        while let Some((parent, _)) = dir.rsplit_once('/') {
            paths.insert(parent.to_owned());
            dir = parent;
        }
        // Binary files (fonts, images) are only needed as paths.
        if let Ok(text) = std::fs::read_to_string(root.join(&path)) {
            texts.insert(path.clone(), text);
        }
        paths.insert(path);
    }
    let tree = Tree { texts, paths };
    let packages = repo::workspace_packages(&root)?;

    let mut problems = Vec::new();
    problems.extend(links::check(&tree));
    problems.extend(adr_index::check(&tree));
    problems.extend(features::check(&tree));
    problems.extend(todos::check(&tree));
    problems.extend(last_verified::check(&tree));
    problems.extend(crate_rules::check(&tree, &packages, &root).context("crate rules")?);

    for problem in &problems {
        eprintln!("{problem}");
    }
    if problems.is_empty() {
        println!("docs-check: ok");
    } else {
        eprintln!("docs-check: {} problem(s)", problems.len());
    }
    Ok(problems.is_empty())
}

#[cfg(test)]
pub(crate) fn tree(files: &[(&str, &str)]) -> Tree {
    let mut paths = HashSet::new();
    for (path, _) in files {
        let mut dir = *path;
        while let Some((parent, _)) = dir.rsplit_once('/') {
            paths.insert(parent.to_owned());
            dir = parent;
        }
        paths.insert((*path).to_owned());
    }
    Tree {
        texts: files
            .iter()
            .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
            .collect(),
        paths,
    }
}
