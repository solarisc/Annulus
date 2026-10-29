//! Snippets (F-SNIP-001…004): the Snippets screen to write and organize saved commands,
//! the panel beside a terminal (Ctrl+Shift+.) to run or paste one, and running one on
//! several saved hosts at once, a pane each.

use annulus_session::Program;

/// Panes one multi-host run opens at most.
pub const MAX_TARGETS: usize = 16;

/// Tabs to open: a title and a program for each pane.
pub type Panes = Vec<(String, Program)>;

/// `program` with `script` typed once its shell opens, after the host's own startup
/// command if it has one. Programs that are not Annulus SSH sessions are left alone.
#[must_use]
pub fn with_script(mut program: Program, script: &str) -> Program {
    if let Program::Ssh { startup, .. } = &mut program {
        let lines: Vec<&str> = [startup.as_deref().unwrap_or_default(), script.trim_end()]
            .into_iter()
            .filter(|line| !line.trim().is_empty())
            .collect();
        *startup = Some(lines.join("\n"));
    }
    program
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snippet_runs_after_the_hosts_own_startup_command() {
        let ssh = |startup: Option<&str>| Program::Ssh {
            target: annulus_session::Target {
                host: "web".into(),
                port: 22,
                user: "me".into(),
            },
            files: annulus_session::SshFiles {
                known_hosts: std::path::PathBuf::from("known_hosts"),
                key_dir: None,
            },
            password: None,
            identity: None,
            via: Vec::new(),
            proxy: None,
            reconnect: true,
            env: Vec::new(),
            startup: startup.map(Into::into),
            term: String::new(),
        };
        let startup = |program: Program| match program {
            Program::Ssh { startup, .. } => startup,
            _ => None,
        };
        assert_eq!(
            startup(with_script(ssh(None), "df -h\n")).as_deref(),
            Some("df -h")
        );
        assert_eq!(
            startup(with_script(ssh(Some("cd /srv")), "ls")).as_deref(),
            Some("cd /srv\nls")
        );
        assert_eq!(
            with_script(Program::DefaultShell, "ls"),
            Program::DefaultShell
        );
    }
}
