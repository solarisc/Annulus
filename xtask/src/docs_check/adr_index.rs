//! The ADR index in `docs/adr/README.md` lists every ADR file exactly once, with the file's
//! own title and status.

use std::collections::BTreeMap;

use super::{Problem, Tree, has_extension, markdown};

const INDEX: &str = "docs/adr/README.md";
const DIR: &str = "docs/adr/";

/// An ADR as its own file describes it.
struct Record<'a> {
    title: &'a str,
    status: String,
}

pub fn check(tree: &Tree) -> Vec<Problem> {
    let index = match tree.text(INDEX) {
        Ok(text) => text,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    let mut records = BTreeMap::new();
    for (path, text) in &tree.texts {
        let Some(name) = path.strip_prefix(DIR) else {
            continue;
        };
        if name.contains('/')
            || !has_extension(name, "md")
            || !name.starts_with(|c: char| c.is_ascii_digit())
        {
            continue;
        }
        match parse_record(name, text) {
            Ok(record) => {
                records.insert(name.to_owned(), record);
            }
            Err(message) => problems.push(Problem::new(path, 1, message)),
        }
    }

    let mut listed = BTreeMap::new();
    for (index_line, line) in index.lines().enumerate() {
        let Some(cells) = markdown::table_cells(line) else {
            continue;
        };
        let [number_cell, title, status] = cells.as_slice() else {
            continue;
        };
        let Some(link) = markdown::links(number_cell).into_iter().next() else {
            continue;
        };
        let line_no = index_line + 1;
        let file = link.target;
        if listed.insert(file.to_owned(), line_no).is_some() {
            problems.push(Problem::new(
                INDEX,
                line_no,
                format!("`{file}` is listed twice"),
            ));
        }
        let Some(record) = records.get(file) else {
            problems.push(Problem::new(
                INDEX,
                line_no,
                format!("`{file}` does not exist"),
            ));
            continue;
        };
        if !file.starts_with(
            number_cell
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or(""),
        ) {
            problems.push(Problem::new(
                INDEX,
                line_no,
                format!("number does not match `{file}`"),
            ));
        }
        if *title != record.title {
            problems.push(Problem::new(
                INDEX,
                line_no,
                format!("title `{title}` differs from the file's `{}`", record.title),
            ));
        }
        if !record.status.starts_with(&normalize_status(status)) {
            problems.push(Problem::new(
                INDEX,
                line_no,
                format!(
                    "status `{status}` differs from the file's `{}`",
                    record.status
                ),
            ));
        }
    }
    for name in records.keys().filter(|name| !listed.contains_key(*name)) {
        problems.push(Problem::new(
            INDEX,
            0,
            format!("`{name}` is missing from the index"),
        ));
    }
    problems
}

fn parse_record<'a>(name: &str, text: &'a str) -> Result<Record<'a>, String> {
    let number = name.split('-').next().unwrap_or_default();
    let heading = text.lines().next().unwrap_or_default();
    let title = heading
        .strip_prefix(&format!("# ADR-{number}: "))
        .ok_or_else(|| format!("first line must be `# ADR-{number}: <title>`"))?;
    let status = text
        .lines()
        .find_map(|line| line.strip_prefix("- Status: "))
        .ok_or("missing a `- Status: ` line")?;
    Ok(Record {
        title,
        status: normalize_status(status),
    })
}

/// Turns `[ADR-0012](0012-x.md)` into `0012` so the index can say `Superseded by 0012`.
fn normalize_status(status: &str) -> String {
    let mut out = String::new();
    let mut rest = status;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find("](") else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        out.push_str(&after[..close]);
        rest = after[close..]
            .find(')')
            .map_or("", |end| &after[close + end + 1..]);
    }
    out.push_str(rest);
    out.replace("ADR-", "")
}

#[cfg(test)]
mod tests {
    use super::super::tree;
    use super::*;

    const INDEX_HEAD: &str = "| ADR | Title | Status |\n|---|---|---|\n";

    fn messages(index_rows: &str, adrs: &[(&str, &str)]) -> Vec<String> {
        let index = format!("{INDEX_HEAD}{index_rows}");
        let mut files = vec![(INDEX, index.as_str())];
        files.extend_from_slice(adrs);
        check(&tree(&files))
            .into_iter()
            .map(|p| p.message)
            .collect()
    }

    const ADR_1: (&str, &str) = (
        "docs/adr/0001-a.md",
        "# ADR-0001: Alpha\n\n- Status: Accepted (owner)\n",
    );
    const ADR_2: (&str, &str) = (
        "docs/adr/0002-b.md",
        "# ADR-0002: Beta\n\n- Status: Superseded by [ADR-0003](0003-c.md)\n",
    );

    #[test]
    fn matching_index_passes() {
        let rows = "| [0001](0001-a.md) | Alpha | Accepted |\n| [0002](0002-b.md) | Beta | Superseded by 0003 |\n";
        assert_eq!(messages(rows, &[ADR_1, ADR_2]), Vec::<String>::new());
    }

    #[test]
    fn unlisted_adr_is_reported() {
        let rows = "| [0001](0001-a.md) | Alpha | Accepted |\n";
        assert_eq!(
            messages(rows, &[ADR_1, ADR_2]),
            ["`0002-b.md` is missing from the index"]
        );
    }

    #[test]
    fn stale_title_and_status_are_reported() {
        let rows = "| [0001](0001-a.md) | Alpha (old) | Proposed |\n";
        let found = messages(rows, &[ADR_1]);
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn index_row_for_missing_file_is_reported() {
        let found = messages("| [0009](0009-z.md) | Zed | Accepted |\n", &[]);
        assert_eq!(found, ["`0009-z.md` does not exist"]);
    }

    #[test]
    fn adr_without_status_line_is_reported() {
        let found = messages("", &[("docs/adr/0004-d.md", "# ADR-0004: Delta\n")]);
        assert!(
            found.contains(&"missing a `- Status: ` line".to_owned()),
            "{found:?}"
        );
    }
}
