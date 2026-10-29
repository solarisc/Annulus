//! The small subset of Markdown that docs-check needs: code stripping, heading anchors, links
//! and pipe tables.

use std::collections::HashSet;

/// Blanks out fenced code blocks and inline code spans, keeping every line in place so line
/// numbers stay correct.
pub fn strip_code(text: &str) -> String {
    let mut in_fence = false;
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence {
            out.push_str(&strip_inline_code(line));
        }
        out.push('\n');
    }
    out
}

fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        let ticks = rest[start..].len() - rest[start..].trim_start_matches('`').len();
        let after_open = &rest[start + ticks..];
        if let Some(end) = find_tick_run(after_open, ticks) {
            rest = &after_open[end + ticks..];
        } else {
            // An unmatched run is literal text (CommonMark).
            out.push_str(&rest[start..start + ticks]);
            rest = after_open;
        }
    }
    out.push_str(rest);
    out
}

/// Byte offset of the next run of exactly `len` backticks.
fn find_tick_run(text: &str, len: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if run == len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// The anchors GitHub generates for the headings in `text`, plus explicit `id="..."` anchors.
pub fn anchors(text: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for line in strip_code_keeping_headings(text).lines() {
        if let Some(heading) = heading_text(line) {
            let base = slug(heading);
            let count = counts.entry(base.clone()).or_insert(0);
            let anchor = if *count == 0 {
                base.clone()
            } else {
                format!("{base}-{count}")
            };
            *count += 1;
            seen.insert(anchor);
        }
        let mut rest = line;
        while let Some(at) = rest.find("id=\"") {
            rest = &rest[at + 4..];
            if let Some(end) = rest.find('"') {
                seen.insert(rest[..end].to_owned());
                rest = &rest[end..];
            }
        }
    }
    seen
}

/// Like [`strip_code`] but keeps inline code inside headings, which GitHub slugs as text.
fn strip_code_keeping_headings(text: &str) -> String {
    let mut in_fence = false;
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn heading_text(line: &str) -> Option<&str> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?;
    Some(rest.trim().trim_end_matches('#').trim_end())
}

/// GitHub's heading slug: lowercase, drop everything but letters, digits, `-`, `_` and spaces,
/// then turn spaces into hyphens.
pub fn slug(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' '))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// An inline link target found in a document.
#[derive(Debug, PartialEq, Eq)]
pub struct Link<'a> {
    /// 1-based line number.
    pub line: usize,
    /// The raw target between the parentheses, without any title.
    pub target: &'a str,
}

/// Inline link and image targets in `text` (which should already have code stripped).
pub fn links(text: &str) -> Vec<Link<'_>> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let mut rest = line;
        while let Some(at) = rest.find("](") {
            let after = &rest[at + 2..];
            let Some(end) = after.find(')') else { break };
            let raw = after[..end].trim();
            let target = raw.split_once(' ').map_or(raw, |(target, _title)| target);
            let target = target.trim_start_matches('<').trim_end_matches('>');
            found.push(Link {
                line: index + 1,
                target,
            });
            rest = &after[end..];
        }
    }
    found
}

/// A pipe-table row split into trimmed cells, or `None` for non-table lines and separators.
pub fn table_cells(line: &str) -> Option<Vec<&str>> {
    let inner = line.trim().strip_prefix('|')?.strip_suffix('|')?;
    let cells: Vec<&str> = inner.split('|').map(str::trim).collect();
    let is_separator = cells
        .iter()
        .all(|cell| !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':' | ' ')));
    (!is_separator).then_some(cells)
}

/// The text inside every `` `code` `` span of `cell`.
pub fn code_spans(cell: &str) -> Vec<&str> {
    cell.split('`').skip(1).step_by(2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_github_for_numbered_and_punctuated_headings() {
        assert_eq!(slug("4. Crates"), "4-crates");
        assert_eq!(
            slug("Keychain: every common auth method and key type (F-KEY)"),
            "keychain-every-common-auth-method-and-key-type-f-key"
        );
        assert_eq!(
            slug("Data-loss rules (hard requirements)"),
            "data-loss-rules-hard-requirements"
        );
        assert_eq!(slug("§7 `annulus-ui`"), "7-annulus-ui");
    }

    #[test]
    fn duplicate_headings_get_numbered_anchors() {
        let found = anchors("# Notes\n## Notes\n### Notes\n");
        assert!(found.contains("notes") && found.contains("notes-1") && found.contains("notes-2"));
    }

    #[test]
    fn headings_inside_code_fences_are_not_anchors() {
        let found = anchors("```\n# not a heading\n```\n# Real\n");
        assert!(found.contains("real"));
        assert!(!found.contains("not-a-heading"));
    }

    #[test]
    fn explicit_id_attributes_are_anchors() {
        assert!(anchors("<a id=\"custom\"></a>\n").contains("custom"));
    }

    #[test]
    fn strip_code_blanks_fences_and_spans_but_keeps_line_numbers() {
        let text = "a `x](y)` b\n```\n[l](gone.md)\n```\nc ``d`e`` f\n";
        let stripped = strip_code(text);
        assert_eq!(
            stripped.lines().collect::<Vec<_>>(),
            ["a  b", "", "", "", "c  f"]
        );
    }

    #[test]
    fn unmatched_backtick_is_kept_as_text() {
        assert_eq!(strip_code("a ` b\n"), "a ` b\n");
    }

    #[test]
    fn links_drop_titles_and_angle_brackets() {
        let found = links("see [a](x.md#y \"title\") and ![i](<img.png>)\n[b](z.md)\n");
        let targets: Vec<_> = found.iter().map(|link| (link.line, link.target)).collect();
        assert_eq!(targets, [(1, "x.md#y"), (1, "img.png"), (2, "z.md")]);
    }

    #[test]
    fn table_cells_skip_separator_rows() {
        assert_eq!(table_cells("| a | `b` |"), Some(vec!["a", "`b`"]));
        assert_eq!(table_cells("|---|:-:|"), None);
        assert_eq!(table_cells("not a table"), None);
    }
}
