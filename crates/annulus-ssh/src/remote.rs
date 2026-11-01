//! Commands run on a server for an action the user asked for (F-KEY-004), written once for a
//! Unix shell and once for Windows, whose OpenSSH runs them in cmd or PowerShell: there a
//! Unix command fails, or worse, half runs. Which one a server gets is asked of its shell
//! first ([`PROBE`]), since the identification line names neither the OS everywhere nor the
//! shell a Windows server was set to.

/// Prints `Windows_NT` in the shells Windows OpenSSH runs commands in (cmd expands `%OS%`,
/// PowerShell `$env:OS`), and something else in a Unix shell. It changes nothing.
pub const PROBE: &str = "echo %OS%$env:OS";

/// The kind of shell a server runs commands in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    /// sh, bash, zsh and the like; also Git Bash or Cygwin set as a Windows server's shell.
    Unix,
    /// cmd or PowerShell, the shells of Windows OpenSSH.
    Windows,
}

impl ShellKind {
    /// What [`PROBE`] printed says about the shell.
    #[must_use]
    pub fn from_probe(output: &[u8]) -> Self {
        if String::from_utf8_lossy(output).contains("Windows_NT") {
            ShellKind::Windows
        } else {
            ShellKind::Unix
        }
    }
}

/// One command, written for each kind of shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commands {
    /// For sh and its kin.
    pub unix: String,
    /// For cmd and PowerShell: see [`powershell`].
    pub windows: String,
}

impl Commands {
    /// The command for a shell of `kind`.
    #[must_use]
    pub fn for_shell(&self, kind: ShellKind) -> &str {
        match kind {
            ShellKind::Unix => &self.unix,
            ShellKind::Windows => &self.windows,
        }
    }
}

/// The longest command line cmd runs (`cmd /c` cuts the rest).
pub const CMD_MOST: usize = 8191;

/// A command line that runs `script` in Windows PowerShell, the same from cmd and from
/// PowerShell: the script travels base64-encoded (UTF-16LE, as `-EncodedCommand` wants), so
/// neither shell's quoting touches it. Such a run writes its uncaught errors and progress to
/// stderr as CLIXML, so `script` should silence progress and catch its errors.
#[must_use]
pub fn powershell(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {}",
        crate::proxy::base64(&utf16)
    )
}

/// `output` without PowerShell's CLIXML lines (`#< CLIXML` and the `<Objs …>` record after
/// it), which mean nothing to a person.
#[must_use]
pub fn without_clixml(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !line.starts_with("#< CLIXML") && !line.starts_with("<Objs")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_probe_tells_windows_shells_from_unix_ones() {
        // As cmd, PowerShell, sh and fish print it.
        assert_eq!(
            ShellKind::from_probe(b"Windows_NT$env:OS\r\n"),
            ShellKind::Windows
        );
        assert_eq!(
            ShellKind::from_probe(b"%OS%Windows_NT\r\n"),
            ShellKind::Windows
        );
        assert_eq!(ShellKind::from_probe(b"%OS%:OS\n"), ShellKind::Unix);
        assert_eq!(ShellKind::from_probe(b"\n"), ShellKind::Unix);
        let commands = Commands {
            unix: "uname".into(),
            windows: "ver".into(),
        };
        assert_eq!(commands.for_shell(ShellKind::Unix), "uname");
        assert_eq!(commands.for_shell(ShellKind::Windows), "ver");
    }

    /// Runs `command` the way a server's shell does: `cmd /c` or `powershell -c` on Windows
    /// (as Windows OpenSSH does, the command line untouched), `sh -c` elsewhere.
    pub(crate) fn run_in(shell: &str, command: &str) -> std::process::Command {
        let mut run = std::process::Command::new(shell);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            if shell == "cmd" {
                run.raw_arg("/c").raw_arg(command);
            } else {
                run.args(["-NoProfile", "-NonInteractive"])
                    .raw_arg("-c")
                    .raw_arg(command);
            }
        }
        #[cfg(not(windows))]
        run.arg("-c").arg(command);
        run
    }

    #[test]
    fn the_probe_names_the_shell_it_runs_in() {
        #[cfg(windows)]
        let shells = [
            ("cmd", ShellKind::Windows),
            ("powershell", ShellKind::Windows),
        ];
        #[cfg(not(windows))]
        let shells = [("sh", ShellKind::Unix)];
        for (shell, kind) in shells {
            let output = run_in(shell, PROBE).output().unwrap();
            assert_eq!(
                ShellKind::from_probe(&output.stdout),
                kind,
                "{shell}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_powershell_command_runs_the_same_from_cmd_and_powershell() {
        let command = powershell("[Console]::Out.WriteLine('it''s ' + (6 * 7)); exit 4");
        assert!(command.len() < CMD_MOST);
        for shell in ["cmd", "powershell"] {
            let output = run_in(shell, &command).output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim(),
                "it's 42",
                "{shell}"
            );
            assert!(!output.status.success(), "{shell}: the failure comes back");
        }
    }

    #[test]
    fn clixml_is_left_out_of_what_a_person_reads() {
        let output = "annulus-error: Access is denied.\r\n#< CLIXML\r\n<Objs Version=\"1.1.0.1\"><S S=\"Error\">boom</S></Objs>\r\n";
        assert_eq!(without_clixml(output), "annulus-error: Access is denied.");
    }
}
