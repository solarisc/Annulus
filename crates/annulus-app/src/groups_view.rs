//! Host groups on the Hosts screen (F-HOST-003): the breadcrumb, group rows, the group
//! editor and the group picker of the host editor.

use annulus_vault::groups::{self, Group};
use annulus_vault::vault::Vault;

/// `uid`'s place in the tree, top first: "prod / db". Unknown uids read as the top level.
#[must_use]
pub fn path(groups: &[Group], uid: Option<&str>) -> String {
    let mut names: Vec<&str> = groups::chain(groups, uid)
        .iter()
        .map(|group| group.name.as_str())
        .collect();
    names.reverse();
    if names.is_empty() {
        "Top level".to_owned()
    } else {
        names.join(" / ")
    }
}

/// Every group as `(uid, path)`, sorted by path, for pickers; leaves out `skip` and what is
/// inside it (a group cannot move into itself).
#[must_use]
pub fn choices(groups: &[Group], skip: Option<&str>) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = groups
        .iter()
        .filter(|group| skip.is_none_or(|skip| !groups::is_within(groups, &group.uid, skip)))
        .map(|group| (group.uid.clone(), path(groups, Some(&group.uid))))
        .collect();
    list.sort_by_key(|(_, path)| path.to_lowercase());
    list
}

/// "3 hosts · 1 group": what a group holds directly.
#[must_use]
pub fn holds(hosts: usize, subgroups: usize) -> String {
    let hosts = match hosts {
        1 => "1 host".to_owned(),
        n => format!("{n} hosts"),
    };
    match subgroups {
        0 => hosts,
        1 => format!("{hosts} \u{b7} 1 group"),
        n => format!("{hosts} \u{b7} {n} groups"),
    }
}

/// Environment variables as the editors show them: one `NAME=value` per line.
#[must_use]
pub fn format_env(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads `NAME=value` lines (blank lines skipped), or says which line is wrong. Names are
/// letters, digits and `_`, not starting with a digit, as shells require.
pub fn parse_env(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut env = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let valid = |name: &str| {
            name.chars()
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        };
        match line.split_once('=') {
            Some((name, value)) if valid(name.trim()) => {
                env.push((name.trim().to_owned(), value.to_owned()));
            }
            _ => {
                return Err(format!(
                    "Environment line {}: write NAME=value (letters, digits and _).",
                    number + 1
                ));
            }
        }
    }
    Ok(env)
}

/// What deleting a group does with the hosts directly in it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deletion {
    /// They (and its subgroups) move up into its parent.
    MoveHostsUp,
    /// They are deleted with it; its subgroups still move up.
    WithHosts,
}

/// Deletes `group` from `vault` as `how` says.
pub(crate) fn delete_group(vault: &mut Vault, group: &Group, how: Deletion) -> Result<(), String> {
    if how == Deletion::WithHosts {
        let inside: Vec<u64> = vault
            .hosts()
            .iter()
            .filter(|host| host.group.as_deref() == Some(group.uid.as_str()))
            .map(|host| host.id)
            .collect();
        vault
            .remove_hosts(&inside)
            .map_err(|error| error.to_string())?;
    }
    vault
        .remove_group(group.id)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(uid: &str, name: &str, parent: Option<&str>) -> Group {
        Group {
            id: 0,
            uid: uid.into(),
            name: name.into(),
            parent: parent.map(Into::into),
            defaults: groups::Defaults::default(),
        }
    }

    #[test]
    fn paths_read_top_down_and_a_group_cannot_pick_itself_as_parent() {
        let list = vec![
            group("a", "prod", None),
            group("b", "db", Some("a")),
            group("c", "web", None),
        ];
        assert_eq!(path(&list, Some("b")), "prod / db");
        assert_eq!(path(&list, None), "Top level");
        let names: Vec<String> = choices(&list, None).into_iter().map(|c| c.1).collect();
        assert_eq!(names, ["prod", "prod / db", "web"]);
        let for_prod: Vec<String> = choices(&list, Some("a")).into_iter().map(|c| c.0).collect();
        assert_eq!(for_prod, ["c"], "not itself, not its subgroup");
        assert_eq!(holds(1, 0), "1 host");
        assert_eq!(holds(3, 2), "3 hosts \u{b7} 2 groups");
    }

    #[test]
    fn environment_lines_round_trip_and_bad_names_are_refused() {
        let env = parse_env("LANG=C.UTF-8\n\n  TZ=Etc/UTC\nEMPTY=\nURL=a=b").unwrap();
        assert_eq!(
            format_env(&env),
            "LANG=C.UTF-8\nTZ=Etc/UTC\nEMPTY=\nURL=a=b",
            "a value may hold = and may be empty"
        );
        assert_eq!(
            parse_env("OK=1\n2BAD=x"),
            Err("Environment line 2: write NAME=value (letters, digits and _).".into())
        );
        assert!(parse_env("no equals sign").is_err());
        assert!(parse_env("A B=1").is_err());
    }

    #[test]
    fn deleting_a_group_moves_its_hosts_up_or_takes_them_along() {
        let dir = std::env::temp_dir().join(format!("annulus-delgroup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cheap = annulus_vault::crypto::KdfParams {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        };
        let (mut vault, _) = Vault::create(dir.join("vault.anv"), "pw", cheap).unwrap();
        let add = |vault: &mut Vault, name: &str| {
            let mut group = vault.draft_group(None).unwrap();
            group.name = name.into();
            vault.upsert_group(group.clone()).unwrap();
            let mut host = vault.draft_host();
            (host.label, host.address) = (format!("{name}-1"), "x".into());
            host.group = Some(group.uid.clone());
            vault.upsert_host(host).unwrap();
            group
        };
        let keep = add(&mut vault, "keep");
        let drop = add(&mut vault, "drop");
        delete_group(&mut vault, &keep, Deletion::MoveHostsUp).unwrap();
        delete_group(&mut vault, &drop, Deletion::WithHosts).unwrap();
        let labels: Vec<&str> = vault.hosts().iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, ["keep-1"]);
        assert_eq!(vault.hosts()[0].group, None, "moved to the top level");
        assert_eq!(vault.groups().len(), 0);
    }
}
