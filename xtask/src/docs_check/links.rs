//! Relative links in Markdown files point at files that exist, and their `#anchors` at headings
//! that exist.

use super::{Problem, Tree, has_extension, markdown};

pub fn check(tree: &Tree) -> Vec<Problem> {
    let mut problems = Vec::new();
    for (file, text) in tree
        .texts
        .iter()
        .filter(|(path, _)| has_extension(path, "md"))
    {
        let stripped = markdown::strip_code(text);
        for link in markdown::links(&stripped) {
            if let Some(message) = broken(tree, file, link.target) {
                problems.push(Problem::new(file, link.line, message));
            }
        }
    }
    problems
}

fn broken(tree: &Tree, file: &str, target: &str) -> Option<String> {
    if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
        return None;
    }
    let (path, anchor) = target
        .split_once('#')
        .map_or((target, None), |(p, a)| (p, Some(a)));
    let resolved = if path.is_empty() {
        file.to_owned()
    } else {
        let base = file.rsplit_once('/').map_or("", |(dir, _)| dir);
        match resolve(base, path) {
            Some(resolved) => resolved,
            None => return Some(format!("link `{target}` leaves the repository")),
        }
    };
    if !resolved.is_empty() && !tree.paths.contains(&resolved) {
        return Some(format!("link `{target}`: `{resolved}` does not exist"));
    }
    let anchor = anchor.filter(|anchor| !anchor.is_empty())?;
    let text = tree
        .texts
        .get(&resolved)
        .filter(|_| has_extension(&resolved, "md"))?;
    (!markdown::anchors(text).contains(anchor))
        .then(|| format!("link `{target}`: no heading with anchor `#{anchor}` in `{resolved}`"))
}

/// Joins `relative` onto the directory `base` and normalizes `.` and `..`. `None` if the result
/// would be outside the repository.
fn resolve(base: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::super::tree;
    use super::*;

    fn messages(files: &[(&str, &str)]) -> Vec<String> {
        check(&tree(files))
            .into_iter()
            .map(|p| p.to_string())
            .collect()
    }

    #[test]
    fn link_to_existing_file_and_heading_passes() {
        let files = [
            (
                "docs/a.md",
                "[d](DESIGN.md#4-crates) [up](../README.md) [self](#top)\n# Top\n",
            ),
            ("docs/DESIGN.md", "## 4. Crates\n"),
            ("README.md", ""),
        ];
        assert_eq!(messages(&files), Vec::<String>::new());
    }

    #[test]
    fn missing_file_is_reported_with_line() {
        let found = messages(&[("docs/a.md", "\n[x](gone.md)\n")]);
        assert_eq!(
            found,
            ["docs/a.md:2: link `gone.md`: `docs/gone.md` does not exist"]
        );
    }

    #[test]
    fn missing_anchor_is_reported() {
        let found = messages(&[("a.md", "[x](b.md#nope)\n"), ("b.md", "# Yes\n")]);
        assert_eq!(
            found,
            ["a.md:1: link `b.md#nope`: no heading with anchor `#nope` in `b.md`"]
        );
    }

    #[test]
    fn link_to_directory_passes() {
        let files = [("README.md", "[r](docs/notes/)\n"), ("docs/notes/x.md", "")];
        assert_eq!(messages(&files), Vec::<String>::new());
    }

    #[test]
    fn links_inside_code_and_external_links_are_ignored() {
        let text = "`[x](gone.md)`\n```\n[y](gone.md)\n```\n[z](https://example.com/a.md)\n";
        assert_eq!(messages(&[("a.md", text)]), Vec::<String>::new());
    }

    #[test]
    fn link_escaping_the_repository_is_reported() {
        assert_eq!(messages(&[("a.md", "[x](../../etc)\n")]).len(), 1);
    }
}
