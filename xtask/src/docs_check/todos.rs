//! Every to-do marker names the feature it belongs to, e.g. `(F-SSH-014)`.
//! Markdown code spans and fences are exempt, so docs can show the format.

use super::{Problem, Tree, has_extension, markdown};

/// Spelled in pieces so this file does not flag itself.
const MARKERS: [&str; 2] = [concat!("TO", "DO"), concat!("FIX", "ME")];

const CHECKED_EXTENSIONS: [&str; 6] = [".rs", ".md", ".toml", ".yml", ".yaml", ".sh"];

pub fn check(tree: &Tree) -> Vec<Problem> {
    let mut problems = Vec::new();
    for (path, text) in &tree.texts {
        if !CHECKED_EXTENSIONS.iter().any(|ext| path.ends_with(ext)) {
            continue;
        }
        let stripped;
        let text = if has_extension(path, "md") {
            stripped = markdown::strip_code(text);
            stripped.as_str()
        } else {
            text.as_str()
        };
        for (index, line) in text.lines().enumerate() {
            for marker in MARKERS {
                if has_bare_marker(line, marker) {
                    problems.push(Problem::new(
                        path,
                        index + 1,
                        format!("{marker} must name a feature: {marker}(F-SSH-014)"),
                    ));
                }
            }
        }
    }
    problems
}

fn has_bare_marker(line: &str, marker: &str) -> bool {
    line.match_indices(marker).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = &line[at + marker.len()..];
        let standalone =
            !before.is_some_and(is_word_char) && !after.chars().next().is_some_and(is_word_char);
        standalone
            && !after
                .strip_prefix('(')
                .and_then(|rest| rest.split_once(')'))
                .is_some_and(|(id, _)| is_work_id(id))
    })
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A feature ID, `F-<AREA>-<NNN>`.
fn is_work_id(id: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if let Some((area, number)) = id.strip_prefix("F-").and_then(|rest| rest.rsplit_once('-')) {
        return !area.is_empty() && area.chars().all(|c| c.is_ascii_uppercase()) && digits(number);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::tree;
    use super::*;

    fn count(path: &str, text: &str) -> usize {
        check(&tree(&[(path, text)])).len()
    }

    fn todo(rest: &str) -> String {
        format!("// {}{rest}\n", MARKERS[0])
    }

    #[test]
    fn marker_with_feature_id_passes() {
        assert_eq!(count("a.rs", &todo("(F-SSH-007): later")), 0);
        assert_eq!(count("a.rs", &todo("(F-SSH-014): later")), 0);
    }

    #[test]
    fn bare_marker_fails() {
        assert_eq!(count("a.rs", &todo(": later")), 1);
        assert_eq!(count("a.rs", &todo("")), 1);
    }

    #[test]
    fn marker_with_malformed_id_fails() {
        assert_eq!(count("a.rs", &todo("(bob): later")), 1);
        assert_eq!(count("a.rs", &todo("(F-SSH): later")), 1);
        assert_eq!(count("a.rs", &todo("(F-ssh-1): later")), 1);
    }

    #[test]
    fn fixme_is_checked_too() {
        assert_eq!(count("a.toml", &format!("# {}\n", MARKERS[1])), 1);
    }

    #[test]
    fn marker_inside_a_word_is_not_a_marker() {
        assert_eq!(count("a.rs", &format!("let {}S = 1;\n", MARKERS[0])), 0);
    }

    #[test]
    fn markdown_code_may_show_bare_markers() {
        let text = format!("Write `{0}` with an id.\n```\n{0}\n```\n", MARKERS[0]);
        assert_eq!(count("a.md", &text), 0);
        assert_eq!(count("a.md", &format!("{} in prose\n", MARKERS[0])), 1);
    }

    #[test]
    fn unchecked_file_types_are_skipped() {
        assert_eq!(count("a.txt", &todo("")), 0);
    }
}
