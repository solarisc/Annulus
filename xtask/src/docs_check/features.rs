//! Every `Done` row in `docs/FEATURES.md` names tests, and every named test exists as a `fn`
//! somewhere in the workspace.

use std::collections::HashSet;

use super::{Problem, Tree, has_extension, markdown};

const FEATURES: &str = "docs/FEATURES.md";

pub fn check(tree: &Tree) -> Vec<Problem> {
    let text = match tree.text(FEATURES) {
        Ok(text) => text,
        Err(problem) => return vec![problem],
    };
    let functions = function_names(
        tree.texts
            .iter()
            .filter(|(path, _)| has_extension(path, "rs"))
            .map(|(_, t)| t.as_str()),
    );
    check_rows(text, &functions)
}

fn check_rows(text: &str, functions: &HashSet<&str>) -> Vec<Problem> {
    let mut problems = Vec::new();
    let mut columns: Option<(usize, usize)> = None;
    for (index, line) in text.lines().enumerate() {
        let Some(cells) = markdown::table_cells(line) else {
            if !line.trim_start().starts_with('|') {
                columns = None;
            }
            continue;
        };
        if cells.first() == Some(&"ID") {
            let status = cells.iter().position(|cell| *cell == "Status");
            let tests = cells.iter().position(|cell| *cell == "Tests");
            columns = status.zip(tests);
            continue;
        }
        let Some((status_col, tests_col)) = columns else {
            continue;
        };
        if cells.get(status_col) != Some(&"Done") {
            continue;
        }
        let id = cells.first().copied().unwrap_or_default();
        let line_no = index + 1;
        let names = markdown::code_spans(cells.get(tests_col).copied().unwrap_or_default());
        if names.is_empty() {
            problems.push(Problem::new(
                FEATURES,
                line_no,
                format!("{id} is Done but names no tests (write them as `test_name`)"),
            ));
        }
        for name in names {
            let function = name.rsplit("::").next().unwrap_or(name);
            if !functions.contains(function) {
                problems.push(Problem::new(
                    FEATURES,
                    line_no,
                    format!("{id} names test `{name}`, but no `fn {function}` exists"),
                ));
            }
        }
    }
    problems
}

/// Names of every `fn` declared in the given Rust sources.
fn function_names<'a>(sources: impl Iterator<Item = &'a str>) -> HashSet<&'a str> {
    let mut names = HashSet::new();
    for source in sources {
        let mut rest = source;
        while let Some(at) = rest.find("fn ") {
            let preceded_by_word = rest[..at].chars().next_back().is_some_and(is_ident_char);
            rest = &rest[at + 3..];
            if preceded_by_word {
                continue;
            }
            let end = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
            if end > 0 {
                names.insert(&rest[..end]);
            }
        }
    }
    names
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "| ID | Feature | Status | Tests |\n|---|---|---|---|\n";

    fn messages(rows: &str, sources: &[&str]) -> Vec<String> {
        let functions = function_names(sources.iter().copied());
        check_rows(&format!("{HEAD}{rows}"), &functions)
            .into_iter()
            .map(|p| p.message)
            .collect()
    }

    #[test]
    fn done_row_with_existing_tests_passes() {
        let rows = "| F-X-001 | x | Done | `a::changed_key_blocks`, `parses_config` |\n";
        let sources = [
            "#[test]\nfn changed_key_blocks() {}",
            "pub(crate) fn parses_config() {}",
        ];
        assert_eq!(messages(rows, &sources), Vec::<String>::new());
    }

    #[test]
    fn done_row_naming_a_missing_test_fails() {
        let found = messages(
            "| F-X-001 | x | Done | `ghost_test` |\n",
            &["fn other() {}"],
        );
        assert_eq!(
            found,
            ["F-X-001 names test `ghost_test`, but no `fn ghost_test` exists"]
        );
    }

    #[test]
    fn done_row_without_tests_fails() {
        assert_eq!(messages("| F-X-001 | x | Done | — |\n", &[]).len(), 1);
    }

    #[test]
    fn rows_that_are_not_done_are_not_checked() {
        let rows = "| F-X-001 | x | Planned | `ghost` |\n| F-X-002 | y | In progress | — |\n";
        assert_eq!(messages(rows, &[]), Vec::<String>::new());
    }

    #[test]
    fn identifier_suffix_is_not_a_function_declaration() {
        let found = messages(
            "| F-X-001 | x | Done | `ghost` |\n",
            &["let tfn ghost = 1;"],
        );
        assert_eq!(found.len(), 1);
    }
}
