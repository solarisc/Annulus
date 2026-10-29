//! Shell profiles (F-LOCAL-002): the local shells this computer has, found once at start.
//! Windows: PowerShell 7, Windows PowerShell, cmd, each WSL distribution, Git Bash.
//! macOS and Linux: the login shell, then the other shells in `/etc/shells`.

use std::path::{Path, PathBuf};

use crate::Program;

/// One way to open a local terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// The name shown in menus, e.g. "PowerShell 7" or "Ubuntu (WSL)".
    pub name: String,
    /// The program to run.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
}

impl Profile {
    /// What a terminal tab runs for this profile.
    #[must_use]
    pub fn program(&self) -> Program {
        Program::Command {
            program: self.program.clone(),
            args: self.args.clone(),
        }
    }
}

fn profile(name: &str, program: &Path, args: &[&str]) -> Profile {
    Profile {
        name: name.to_owned(),
        program: program.display().to_string(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

/// The first file named `name` in the `PATH` list `path`.
fn on_path(path: &str, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// WSL distribution names from `wsl.exe -l -q`, which prints UTF-16LE, one per line.
#[must_use]
pub fn parse_wsl_list(output: &[u8]) -> Vec<String> {
    let units: Vec<u16> = output
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16_lossy(&units)
        .lines()
        .map(|line| line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0'))
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Shells listed in `/etc/shells`: absolute paths, without comments or duplicates.
#[must_use]
pub fn parse_etc_shells(text: &str) -> Vec<String> {
    let mut shells: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('/') && !shells.iter().any(|known| known == line) {
            shells.push(line.to_owned());
        }
    }
    shells
}

/// A menu name for a Unix shell path: its file name, e.g. "zsh".
fn shell_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The shells found on this computer, the preferred one first. Never empty: if nothing is
/// found, the system default shell is offered.
#[must_use]
pub fn detect() -> Vec<Profile> {
    let mut found = if cfg!(windows) {
        detect_windows()
    } else {
        detect_unix()
    };
    if found.is_empty() {
        found.push(Profile {
            name: "Default shell".into(),
            program: if cfg!(windows) {
                "cmd.exe".into()
            } else {
                "/bin/sh".into()
            },
            args: Vec::new(),
        });
    }
    found
}

fn detect_windows() -> Vec<Profile> {
    let path = std::env::var("PATH").unwrap_or_default();
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let program_files =
        std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    let mut found = Vec::new();
    let pwsh = on_path(&path, "pwsh.exe").or_else(|| {
        Some(Path::new(&program_files).join(r"PowerShell\7\pwsh.exe")).filter(|p| p.is_file())
    });
    if let Some(pwsh) = pwsh {
        found.push(profile("PowerShell 7", &pwsh, &["-NoLogo"]));
    }
    let powershell =
        Path::new(&system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if powershell.is_file() {
        found.push(profile("Windows PowerShell", &powershell, &["-NoLogo"]));
    }
    let cmd = std::env::var("ComSpec").map_or_else(
        |_| Path::new(&system_root).join(r"System32\cmd.exe"),
        PathBuf::from,
    );
    if cmd.is_file() {
        found.push(profile("Command Prompt", &cmd, &[]));
    }
    let wsl = Path::new(&system_root).join(r"System32\wsl.exe");
    if wsl.is_file()
        && let Ok(output) = std::process::Command::new(&wsl).args(["-l", "-q"]).output()
    {
        for distro in parse_wsl_list(&output.stdout) {
            found.push(profile(&format!("{distro} (WSL)"), &wsl, &["-d", &distro]));
        }
    }
    let git_bash = Path::new(&program_files).join(r"Git\bin\bash.exe");
    if git_bash.is_file() {
        found.push(profile("Git Bash", &git_bash, &["--login", "-i"]));
    }
    found
}

fn detect_unix() -> Vec<Profile> {
    let mut paths: Vec<String> = Vec::new();
    if let Ok(login) = std::env::var("SHELL") {
        paths.push(login);
    }
    let listed = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    for shell in parse_etc_shells(&listed) {
        if !paths.contains(&shell) {
            paths.push(shell);
        }
    }
    paths
        .into_iter()
        .filter(|path| Path::new(path).is_file())
        .map(|path| Profile {
            name: shell_name(&path),
            program: path,
            args: vec!["-l".into()],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_output_is_utf16_with_a_byte_order_mark_and_blank_lines() {
        let text = "\u{feff}Ubuntu-22.04\r\ndocker-desktop\r\n\r\n";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(parse_wsl_list(&bytes), ["Ubuntu-22.04", "docker-desktop"]);
        assert_eq!(parse_wsl_list(&[]), Vec::<String>::new());
    }

    #[test]
    fn etc_shells_skips_comments_and_repeats() {
        let text = "# /etc/shells\n/bin/sh\n/bin/bash\n/usr/bin/zsh\n/bin/bash\nnologin\n";
        assert_eq!(
            parse_etc_shells(text),
            ["/bin/sh", "/bin/bash", "/usr/bin/zsh"]
        );
        assert_eq!(shell_name("/usr/bin/zsh"), "zsh");
    }

    #[test]
    fn something_is_always_offered() {
        let found = detect();
        assert_ne!(found.len(), 0);
        assert!(found.iter().all(|profile| !profile.name.is_empty()));
    }
}
