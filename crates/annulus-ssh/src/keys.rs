//! SSH keys for the keychain (F-KEY-001…003): generate Ed25519 keys, import OpenSSH and
//! PEM private keys (decrypting them once with their passphrase, since the vault encrypts
//! everything it stores), and show public keys and fingerprints.

use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PrivateKey};
use zeroize::Zeroizing;

use crate::remote::{self, Commands};

/// A private key in OpenSSH format, unencrypted (the vault encrypts it), and its public
/// key line.
pub struct KeyPair {
    /// `-----BEGIN OPENSSH PRIVATE KEY-----…`, wiped on drop.
    pub private: Zeroizing<String>,
    /// `ssh-ed25519 AAAA… comment`, for `authorized_keys`.
    pub public: String,
    /// `SHA256:…`.
    pub fingerprint: String,
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyPair")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

/// Why a key could not be used.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The key is encrypted and no passphrase (or a wrong one) was given.
    #[error("the key is protected by a passphrase; enter it to import the key")]
    NeedsPassphrase,
    /// Not a private key this app can read.
    #[error("not a private key in OpenSSH or PEM format")]
    Invalid,
    /// No randomness to generate a key.
    #[error("could not generate a key")]
    Generate,
}

fn pair(key: &PrivateKey) -> Result<KeyPair, KeyError> {
    let private = key
        .to_openssh(LineEnding::LF)
        .map_err(|_| KeyError::Invalid)?;
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|_| KeyError::Invalid)?;
    Ok(KeyPair {
        private: Zeroizing::new(private.to_string()),
        public,
        fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
    })
}

/// The kinds of key the keychain can generate (F-KEY-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// Ed25519: small and fast; the default.
    Ed25519,
    /// ECDSA on NIST P-256, for servers that need a NIST curve.
    EcdsaP256,
    /// ECDSA on NIST P-384.
    EcdsaP384,
    /// ECDSA on NIST P-521.
    EcdsaP521,
    /// RSA, 3072 bits (signing with SHA-512), for old servers.
    Rsa3072,
    /// RSA, 4096 bits; slower to make and use than 3072.
    Rsa4096,
}

impl KeyKind {
    /// Every kind, default first.
    pub const ALL: [KeyKind; 6] = [
        KeyKind::Ed25519,
        KeyKind::EcdsaP256,
        KeyKind::EcdsaP384,
        KeyKind::EcdsaP521,
        KeyKind::Rsa3072,
        KeyKind::Rsa4096,
    ];

    /// The name shown in the keychain.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            KeyKind::Ed25519 => "Ed25519",
            KeyKind::EcdsaP256 => "ECDSA P-256",
            KeyKind::EcdsaP384 => "ECDSA P-384",
            KeyKind::EcdsaP521 => "ECDSA P-521",
            KeyKind::Rsa3072 => "RSA 3072",
            KeyKind::Rsa4096 => "RSA 4096",
        }
    }
}

/// A new key of `kind` with `comment` (often `user@device`). RSA takes a few seconds.
///
/// # Errors
/// No randomness.
pub fn generate(kind: KeyKind, comment: &str) -> Result<KeyPair, KeyError> {
    let ecdsa = |curve| PrivateKey::random(&mut rand::rng(), Algorithm::Ecdsa { curve });
    let rsa = |bits| {
        russh::keys::ssh_key::private::RsaKeypair::random(&mut rand::rng(), bits)
            .and_then(|pair| PrivateKey::new(pair.into(), comment))
    };
    let mut key = match kind {
        KeyKind::Ed25519 => PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519),
        KeyKind::EcdsaP256 => ecdsa(EcdsaCurve::NistP256),
        KeyKind::EcdsaP384 => ecdsa(EcdsaCurve::NistP384),
        KeyKind::EcdsaP521 => ecdsa(EcdsaCurve::NistP521),
        KeyKind::Rsa3072 => rsa(3072),
        KeyKind::Rsa4096 => rsa(4096),
    }
    .map_err(|_| KeyError::Generate)?;
    key.set_comment(comment);
    pair(&key)
}

/// Reads a private key (OpenSSH, or PEM such as PKCS#1/PKCS#8), decrypting it with
/// `passphrase` if it is protected.
///
/// # Errors
/// [`KeyError::NeedsPassphrase`] when it is encrypted and the passphrase is missing or
/// wrong; [`KeyError::Invalid`] otherwise.
pub fn import(text: &str, passphrase: Option<&str>) -> Result<KeyPair, KeyError> {
    let passphrase = passphrase.filter(|value| !value.is_empty());
    if let Ok(key) = russh::keys::decode_secret_key(text.trim(), passphrase) {
        return pair(&key);
    }
    // Without a passphrase, an encrypted key fails to decode: tell that apart from text
    // that is not a key at all.
    let encrypted = text.contains("ENCRYPTED")
        || PrivateKey::from_openssh(text.trim()).is_ok_and(|key| key.is_encrypted());
    Err(if encrypted {
        KeyError::NeedsPassphrase
    } else {
        KeyError::Invalid
    })
}

/// What [`install_commands`] prints once the key is in place. A shell that ran nothing of
/// ours (a menu, a shell of another kind) may still end with status 0, so success is this
/// word, not the status alone.
pub const INSTALLED: &str = "annulus-installed";

/// What [`remove_commands`] prints once the key is gone, before the number of lines that
/// held it (0 when it was not there).
pub const REMOVED: &str = "annulus-removed ";

/// What [`remove_commands`] prints when a fresh read still finds the key.
pub const STILL_THERE: &str = "annulus-still-there";

/// What the Windows commands print before the reason they failed.
pub const FAILED: &str = "annulus-error: ";

/// The commands that add `public` (an `authorized_keys` line) to the server's
/// `authorized_keys` once, with the permissions sshd insists on (F-KEY-004), and print
/// [`INSTALLED`]. Running them again changes nothing.
///
/// On Windows the file is the one sshd reads for the user: `%USERPROFILE%\.ssh` for most, but
/// for an administrator `%ProgramData%\ssh\administrators_authorized_keys` while the stock
/// `Match Group administrators` block of `sshd_config` is on. It is written as UTF-8 without
/// a byte order mark, and only its owner, SYSTEM and the Administrators may change it (only
/// SYSTEM and the Administrators for the shared file).
#[must_use]
pub fn install_commands(public: &str) -> Commands {
    let line = public.split_whitespace().collect::<Vec<_>>().join(" ");
    let quoted = line.replace('\'', r"'\''");
    let file = "~/.ssh/authorized_keys";
    let unix = format!(
        "umask 077 && mkdir -p ~/.ssh && touch {file} && chmod 700 ~/.ssh && chmod 600 {file} \
         && {{ [ ! -s {file} ] || [ -z \"$(tail -c 1 {file})\" ] || echo >> {file}; }} \
         && {{ grep -qxF '{quoted}' {file} || printf '%s\\n' '{quoted}' >> {file}; }} \
         && echo {INSTALLED}"
    );
    let body = WINDOWS_INSTALL.replace("@LINE@", &powershell_quoted(&line));
    Commands {
        unix,
        windows: remote::powershell(&windows_script(&body)),
    }
}

/// The commands that take `public` off the server's `authorized_keys` (deleting a key,
/// F-KEY-004): every line holding its key data, whatever its options or comment, and nothing
/// else. The file is rewritten in place, so its mode, owner and any link (on Windows its
/// permissions) stay. They print [`REMOVED`] and the count once a fresh read finds the key
/// gone, or [`STILL_THERE`] (exiting 3) when it is not; they exit 2 when the file cannot be
/// read or written. On Windows the file is the one [`install_commands`] picks. `None` for a
/// line without base64 key data.
#[must_use]
pub fn remove_commands(public: &str) -> Option<Commands> {
    let data = public.split_whitespace().nth(1)?;
    if !data
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    {
        return None;
    }
    let unix = format!(
        "f=~/.ssh/authorized_keys; [ -e \"$f\" ] || {{ echo '{REMOVED}0'; exit 0; }}; \
         n=$(grep -cF -- '{data}' \"$f\"); [ $? -le 1 ] || exit 2; \
         if [ \"$n\" -gt 0 ]; then t=\"$f.annulus-$$\"; \
         (umask 077 && grep -vF -- '{data}' \"$f\" > \"$t\"); \
         [ $? -le 1 ] && cat \"$t\" > \"$f\" || {{ rm -f \"$t\"; exit 2; }}; rm -f \"$t\"; fi; \
         grep -qF -- '{data}' \"$f\" && {{ echo {STILL_THERE}; exit 3; }}; echo \"{REMOVED}$n\""
    );
    let body = WINDOWS_REMOVE.replace("@DATA@", data);
    Some(Commands {
        unix,
        windows: remote::powershell(&windows_script(&body)),
    })
}

/// `text` inside a PowerShell single-quoted string: each quote doubled, the typographic ones
/// too, which PowerShell also takes for quotes.
fn powershell_quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            quoted.push(c);
        }
        quoted.push(c);
    }
    quoted
}

/// A Windows key script: `$f` set to the `authorized_keys` sshd reads for the user (`$shared`
/// when it is the administrators' file), then `body`; any error is printed after [`FAILED`]
/// as one plain line (no CLIXML) and exits 2.
fn windows_script(body: &str) -> String {
    format!(
        r"$ProgressPreference = 'SilentlyContinue'
$ErrorActionPreference = 'Stop'
try {{
  $f = Join-Path $env:USERPROFILE '.ssh\authorized_keys'
  $config = Join-Path $env:ProgramData 'ssh\sshd_config'
  $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  $shared = $admin -and (Test-Path -LiteralPath $config) -and (Select-String -LiteralPath $config -Pattern '^\s*Match\s+Group\s+administrators\b' -Quiet)
  if ($shared) {{ $f = Join-Path $env:ProgramData 'ssh\administrators_authorized_keys' }}
{body}
}} catch {{
  $e = $_.Exception
  while ($e.InnerException) {{ $e = $e.InnerException }}
  [Console]::Out.WriteLine('{FAILED}' + $e.Message)
  exit 2
}}
"
    )
}

/// Adds `@LINE@` to `$f` once, then lets only its owner, SYSTEM and the Administrators (only
/// the last two for the shared file) change it, by SID, since group names are translated.
const WINDOWS_INSTALL: &str = r#"  $line = '@LINE@'
  New-Item -ItemType Directory -Force -Path (Split-Path $f) | Out-Null
  $text = ''
  if (Test-Path -LiteralPath $f) { $text = [IO.File]::ReadAllText($f) }
  if (@($text -split "`r?`n" | ForEach-Object { $_.Trim() }) -cnotcontains $line) {
    if ($text -and -not $text.EndsWith("`n")) { $line = "`r`n" + $line }
    [IO.File]::AppendAllText($f, $line + "`r`n", (New-Object Text.UTF8Encoding $false))
  }
  $grant = '*S-1-5-32-544:F', '*S-1-5-18:F'
  if (-not $shared) { $grant += '*' + [Security.Principal.WindowsIdentity]::GetCurrent().User.Value + ':F' }
  icacls $f /inheritance:r /grant $grant | Out-Null
  if ($LASTEXITCODE) { throw "could not limit who may change $f" }
  [Console]::Out.WriteLine('annulus-installed')
  exit 0"#;

/// Takes every line holding `@DATA@` off `$f`, rewriting it in place (it keeps its
/// permissions), then reads it again.
const WINDOWS_REMOVE: &str = r#"  $data = '@DATA@'
  $n = 0
  if (Test-Path -LiteralPath $f) {
    $text = [IO.File]::ReadAllText($f)
    $nl = "`n"
    if ($text.Contains("`r`n")) { $nl = "`r`n" }
    $lines = $text -split "`r?`n"
    $kept = @($lines | Where-Object { -not $_.Contains($data) })
    $n = $lines.Count - $kept.Count
    if ($n) {
      $bytes = (New-Object Text.UTF8Encoding $false).GetBytes($kept -join $nl)
      $out = [IO.File]::Open($f, 'Truncate', 'Write')
      try { $out.Write($bytes, 0, $bytes.Length) } finally { $out.Dispose() }
    }
    if ([IO.File]::ReadAllText($f).Contains($data)) {
      [Console]::Out.WriteLine('annulus-still-there')
      exit 3
    }
  }
  [Console]::Out.WriteLine("annulus-removed $n")
  exit 0"#;

/// Decodes a stored private key for signing.
pub(crate) fn decode(private: &str) -> Option<PrivateKey> {
    russh::keys::decode_secret_key(private, None).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_key_imports_back_to_the_same_public_key() {
        let generated = generate(KeyKind::Ed25519, "me@laptop").unwrap();
        assert!(generated.public.starts_with("ssh-ed25519 "));
        assert!(generated.public.ends_with(" me@laptop"));
        assert!(generated.fingerprint.starts_with("SHA256:"));
        let again = import(&generated.private, None).unwrap();
        assert_eq!(again.public, generated.public);
        assert!(!format!("{generated:?}").contains("PRIVATE"));
    }

    #[test]
    fn ecdsa_and_rsa_keys_generate_and_decode_for_signing() {
        for (kind, prefix) in [
            (KeyKind::EcdsaP256, "ecdsa-sha2-nistp256 "),
            (KeyKind::EcdsaP384, "ecdsa-sha2-nistp384 "),
            (KeyKind::EcdsaP521, "ecdsa-sha2-nistp521 "),
            (KeyKind::Rsa3072, "ssh-rsa "),
        ] {
            let generated = generate(kind, "me@laptop").unwrap();
            assert!(generated.public.starts_with(prefix), "{}", generated.public);
            assert!(decode(&generated.private).is_some(), "{}", kind.name());
            assert_eq!(
                import(&generated.private, None).unwrap().public,
                generated.public
            );
        }
    }

    /// The PowerShell script a Windows command carries.
    fn script_of(command: &str) -> String {
        use base64::Engine as _;
        let encoded = command.rsplit(' ').next().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let utf16: Vec<u16> = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&utf16).unwrap()
    }

    #[test]
    fn the_install_commands_quote_the_key_line() {
        let commands = install_commands("ssh-ed25519 AAAA it's me\u{2019}\n");
        assert!(
            commands
                .unix
                .contains("'ssh-ed25519 AAAA it'\\''s me\u{2019}'"),
            "{}",
            commands.unix
        );
        assert!(commands.unix.contains("chmod 600 ~/.ssh/authorized_keys"));
        assert!(
            commands.unix.contains("grep -qxF"),
            "only added when missing"
        );
        let script = script_of(&commands.windows);
        assert!(
            script.contains("'ssh-ed25519 AAAA it''s me\u{2019}\u{2019}'"),
            "{script}"
        );
        assert!(script.contains("-cnotcontains"), "only added when missing");
    }

    #[test]
    fn on_windows_an_administrators_key_goes_where_sshd_reads_it() {
        let public = "ssh-ed25519 AAAAC3Nz me";
        let install = script_of(&install_commands(public).windows);
        let remove = script_of(&remove_commands(public).unwrap().windows);
        for script in [&install, &remove] {
            assert!(script.contains("Match\\s+Group\\s+administrators"));
            assert!(script.contains(r"'ssh\administrators_authorized_keys'"));
            assert!(
                script.contains(FAILED),
                "errors come back as one plain line"
            );
        }
        // Only SYSTEM and the Administrators, by SID, may change the shared file.
        assert!(install.contains("'*S-1-5-32-544:F', '*S-1-5-18:F'"));
        assert!(install.contains("/inheritance:r"));
        assert!(install.contains(INSTALLED));
        assert!(remove.contains(REMOVED.trim_end()) && remove.contains(STILL_THERE));
    }

    #[test]
    fn a_windows_command_for_a_long_key_fits_cmds_command_line() {
        // An RSA 4096 public key: 716 characters of key data.
        let public = format!(
            "ssh-rsa AAAAB3{} someone@a-long-computer-name",
            "x".repeat(710)
        );
        assert!(install_commands(&public).windows.len() < remote::CMD_MOST);
        assert!(remove_commands(&public).unwrap().windows.len() < remote::CMD_MOST);
    }

    #[test]
    fn the_remove_commands_match_the_key_data_and_refuse_anything_else() {
        let commands = remove_commands("ssh-ed25519 AAAAC3Nz/a+b= me@laptop").unwrap();
        let command = &commands.unix;
        assert!(command.contains("grep -vF -- 'AAAAC3Nz/a+b='"), "{command}");
        assert!(
            !command.contains("me@laptop"),
            "matched by its data, not its comment"
        );
        assert!(
            command.contains("cat \"$t\" > \"$f\""),
            "rewritten in place"
        );
        assert!(script_of(&commands.windows).contains("'AAAAC3Nz/a+b='"));
        assert!(!script_of(&commands.windows).contains("me@laptop"));
        assert_eq!(remove_commands("ssh-ed25519"), None);
        assert_eq!(remove_commands("ssh-ed25519 AAAA';rm -rf ~;' x"), None);
    }

    /// Installs a key twice, then takes it off twice, in each shell this OS's servers run
    /// commands in (sh; cmd and PowerShell on Windows), in a home of its own.
    #[test]
    fn the_key_commands_add_a_key_once_and_take_off_only_it() {
        use crate::remote::{ShellKind, tests::run_in};
        #[cfg(windows)]
        let shells = [
            ("cmd", ShellKind::Windows),
            ("powershell", ShellKind::Windows),
        ];
        #[cfg(not(windows))]
        let shells = [("sh", ShellKind::Unix)];
        let key = "ssh-ed25519 GONE it's me";
        let (install, remove) = (install_commands(key), remove_commands(key).unwrap());
        for (shell, kind) in shells {
            let home = std::env::temp_dir().join(format!(
                "annulus-key-commands-{}-{shell}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(home.join(".ssh")).unwrap();
            let file = home.join(".ssh").join("authorized_keys");
            // An older line of the same key with options, and a last line not ended.
            std::fs::write(
                &file,
                "ssh-ed25519 KEEP1 other\nfrom=\"10.0.0.1\" ssh-ed25519 GONE old\nssh-rsa KEEP2 x",
            )
            .unwrap();
            let run = |command: &str| {
                let output = run_in(shell, command)
                    .env("HOME", &home)
                    .env("USERPROFILE", &home)
                    // No sshd_config there: the user's own file, administrator or not.
                    .env("ProgramData", home.join("programdata"))
                    .output()
                    .unwrap();
                let out = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                (output.status.code(), out)
            };
            for _ in 0..2 {
                assert_eq!(
                    run(install.for_shell(kind)),
                    (Some(0), INSTALLED.to_owned()),
                    "{shell}"
                );
            }
            let text = std::fs::read_to_string(&file).unwrap();
            assert!(!text.starts_with('\u{feff}'), "{shell}: no byte order mark");
            assert_eq!(
                text.lines().collect::<Vec<_>>(),
                [
                    "ssh-ed25519 KEEP1 other",
                    "from=\"10.0.0.1\" ssh-ed25519 GONE old",
                    "ssh-rsa KEEP2 x",
                    key
                ],
                "{shell}: added once, after the unended line"
            );
            #[cfg(windows)]
            {
                let acl = std::process::Command::new("icacls")
                    .arg(&file)
                    .output()
                    .unwrap();
                assert!(
                    !String::from_utf8_lossy(&acl.stdout).contains("(I)"),
                    "{shell}: no permission inherited from the folder"
                );
            }
            assert_eq!(
                run(remove.for_shell(kind)),
                (Some(0), format!("{REMOVED}2")),
                "{shell}"
            );
            assert_eq!(
                std::fs::read_to_string(&file)
                    .unwrap()
                    .lines()
                    .collect::<Vec<_>>(),
                ["ssh-ed25519 KEEP1 other", "ssh-rsa KEEP2 x"],
                "{shell}"
            );
            // Not there any more: nothing to do, and it says so.
            assert_eq!(
                run(remove.for_shell(kind)),
                (Some(0), format!("{REMOVED}0")),
                "{shell}"
            );
            let _ = std::fs::remove_dir_all(&home);
        }
    }

    #[test]
    fn an_encrypted_key_needs_its_passphrase_and_is_stored_decrypted() {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let encrypted = key
            .encrypt(&mut rand::rng(), "s3cret")
            .unwrap()
            .to_openssh(LineEnding::LF)
            .unwrap();
        assert_eq!(
            import(&encrypted, None).unwrap_err(),
            KeyError::NeedsPassphrase
        );
        assert_eq!(
            import(&encrypted, Some("wrong")).unwrap_err(),
            KeyError::NeedsPassphrase
        );
        let imported = import(&encrypted, Some("s3cret")).unwrap();
        assert!(
            decode(&imported.private).is_some(),
            "usable without the passphrase"
        );
        assert_eq!(import("hello", None).unwrap_err(), KeyError::Invalid);
    }
}
