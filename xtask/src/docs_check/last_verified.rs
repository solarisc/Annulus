//! Every doc that `docs/README.md` marks as Living carries a well-formed
//! `Last verified: YYYY-MM-DD @ <short-sha>` line near its top.

use super::{Problem, Tree, markdown};

const DOC_MAP: &str = "docs/README.md";
/// The line must appear within this many lines of the top, so readers see it at once.
const MAX_LINE: usize = 5;

pub fn check(tree: &Tree) -> Vec<Problem> {
    let map = match tree.text(DOC_MAP) {
        Ok(text) => text,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    for (index, line) in map.lines().enumerate() {
        let Some(cells) = markdown::table_cells(line) else {
            continue;
        };
        if cells.get(1) != Some(&"Living") {
            continue;
        }
        let Some(link) = cells
            .first()
            .and_then(|cell| markdown::links(cell).into_iter().next())
        else {
            problems.push(Problem::new(DOC_MAP, index + 1, "Living row has no link"));
            continue;
        };
        let path = resolve_from_docs(link.target);
        match tree.texts.get(&path) {
            None => problems.push(Problem::new(
                DOC_MAP,
                index + 1,
                format!("`{path}` does not exist"),
            )),
            Some(text) if !text.lines().take(MAX_LINE).any(is_last_verified) => {
                problems.push(Problem::new(
                    &path,
                    0,
                    format!("Living doc needs `Last verified: YYYY-MM-DD @ <sha>` in its first {MAX_LINE} lines"),
                ));
            }
            Some(_) => {}
        }
    }
    problems
}

/// Doc-map links are relative to `docs/`; only `../` (to the root) is used.
fn resolve_from_docs(target: &str) -> String {
    match target.strip_prefix("../") {
        Some(root_relative) => root_relative.to_owned(),
        None => format!("docs/{}", target.split('#').next().unwrap_or(target)),
    }
}

fn is_last_verified(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("Last verified: ") else {
        return false;
    };
    let Some((date, rest)) = rest.split_at_checked(10) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(" @ ") else {
        return false;
    };
    let sha_len = rest.chars().take_while(char::is_ascii_hexdigit).count();
    let after_sha = &rest[sha_len..];
    is_date(date)
        && (7..=40).contains(&sha_len)
        && (after_sha.is_empty() || after_sha.starts_with(' '))
}

fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::super::tree;
    use super::*;

    const MAP: &str = "| Doc | Kind | What |\n|---|---|---|\n| [A](A.md) | Living | a |\n| [R](REPORT.md) | Log | r |\n| [../C.md](../C.md) | Living | c |\n";

    fn messages(a: &str, c: &str) -> Vec<String> {
        check(&tree(&[
            (DOC_MAP, MAP),
            ("docs/A.md", a),
            ("docs/REPORT.md", "# Log\n"),
            ("C.md", c),
        ]))
        .into_iter()
        .map(|p| p.to_string())
        .collect()
    }

    const GOOD: &str = "# Doc\n\nLast verified: 2026-10-28 @ befd268 (planning)\n";

    #[test]
    fn living_docs_with_the_line_pass_and_logs_are_exempt() {
        assert_eq!(messages(GOOD, GOOD), Vec::<String>::new());
    }

    #[test]
    fn living_doc_without_the_line_fails() {
        let found = messages(GOOD, "# C\n\nNo line here.\n");
        assert_eq!(found.len(), 1);
        assert!(found[0].starts_with("C.md: Living doc needs"), "{found:?}");
    }

    #[test]
    fn malformed_date_or_sha_fails() {
        assert_eq!(
            messages("Last verified: 2026-1-28 @ befd268\n", GOOD).len(),
            1
        );
        assert_eq!(messages("Last verified: 2026-10-28 @ xyz\n", GOOD).len(), 1);
        assert_eq!(
            messages("Last verified: 2026-10-28 @ befd26\n", GOOD).len(),
            1
        );
    }

    #[test]
    fn line_buried_below_the_top_fails() {
        let late = format!("# A\n\n\n\n\n\n{}", GOOD.lines().nth(2).unwrap());
        assert_eq!(messages(&late, GOOD).len(), 1);
    }
}
