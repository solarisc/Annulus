//! SCP command arguments and output; paths never pass through a shell.

use std::path::{Path, PathBuf};

use annulus_control::protocol::{TransferOutput, TransferRequest};

pub(super) fn failed(
    error: annulus_control::client::ClientError,
) -> annulus_control::client::ClientError {
    use annulus_control::protocol::TRANSFER_CLEANUP;
    let text = error.to_string();
    if text.contains(TRANSFER_CLEANUP) {
        error
    } else {
        annulus_control::client::ClientError::Refused(format!("{text}\n{TRANSFER_CLEANUP}"))
    }
}

fn remote(path: &str) -> Option<(u64, &str)> {
    let (host, path) = path.split_once(':')?;
    Some((host.parse().ok()?, path))
}

pub(super) fn parse(words: &[&str]) -> Result<TransferRequest, String> {
    let mut recursive = false;
    let mut overwrite = false;
    let mut paths = Vec::new();
    let mut options = true;
    for &word in words {
        match word {
            "--" if options => options = false,
            "-r" | "--recursive" if options => recursive = true,
            "--overwrite" if options => overwrite = true,
            _ if options && word.starts_with('-') => {
                return Err(format!("unknown scp option: {word}"));
            }
            _ => paths.push(word),
        }
    }
    let [source, destination] = paths.as_slice() else {
        return Err(
            "scp needs a source and destination; use <host-id>:<path> for one of them".into(),
        );
    };
    let (host, upload, local, remote) = match (remote(source), remote(destination)) {
        (None, Some((host, remote))) => (host, true, *source, remote),
        (Some((host, remote)), None) => (host, false, *destination, remote),
        _ => return Err("scp needs exactly one saved host path: <host-id>:<path>".into()),
    };
    if local.is_empty()
        || remote.is_empty()
        || local.chars().any(char::is_control)
        || remote.chars().any(char::is_control)
    {
        return Err("scp paths must be nonempty and have no control characters".into());
    }
    // Unknown host labels must not become relative local paths on Unix.
    if local.contains(':') && !is_drive_path(local) {
        return Err(
            "use a numeric saved host id; local paths containing ':' need a drive letter".into(),
        );
    }
    let mut local_path = PathBuf::from(local);
    let mut remote_path = remote.to_owned();
    if upload && remote.ends_with('/') {
        let name = local_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("the local source needs a file or directory name")?;
        remote_path.push_str(name);
    } else if !upload && (local.ends_with(['/', '\\']) || local_path.is_dir()) {
        let name = remote
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("");
        if matches!(name, "" | "." | "..") || name.contains(['\\', ':']) {
            return Err("the remote source needs a plain file or directory name".into());
        }
        local_path.push(name);
    }
    if !local_path.is_absolute() {
        local_path = std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(local_path);
    }
    let local = path_text(&local_path)?;
    Ok(TransferRequest {
        host,
        upload,
        local,
        remote: remote_path,
        recursive,
        overwrite,
    })
}

fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn path_text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "scp paths must be valid UTF-8".into())
}

pub(super) fn printed(answer: &TransferOutput, json: bool) -> String {
    if json {
        serde_json::to_string_pretty(answer).unwrap_or_default() + "\n"
    } else {
        format!(
            "Copied {} bytes to {} (tab {}).\n{}\n",
            answer.bytes, answer.to, answer.tab, answer.cleanup
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scp_keeps_spaces_and_shell_text_as_paths_in_both_directions() {
        let upload = parse(&["--recursive", "build files", "42:/tmp/a; echo x/"]).unwrap();
        assert_eq!(upload.host, 42);
        assert!(upload.upload && upload.recursive);
        assert!(!upload.overwrite);
        assert_eq!(upload.remote, "/tmp/a; echo x/build files");
        assert!(std::path::Path::new(&upload.local).is_absolute());
        let download = parse(&["--overwrite", "42:/srv/a file.zip", "copy.zip"]).unwrap();
        assert!(!download.upload);
        assert!(download.overwrite);
        assert_eq!(download.remote, "/srv/a file.zip");
        assert!(download.local.ends_with("copy.zip"));
    }

    #[test]
    fn scp_requires_one_saved_host_and_two_paths() {
        for args in [
            vec!["a", "b"],
            vec!["1:/a", "2:/b"],
            vec!["a", "1:"],
            vec!["a", "host:/b"],
            vec!["--unknown", "a", "1:/b"],
            vec!["a", "1:/b", "extra"],
            vec!["a", "1:/bad\nname"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
        // A Windows drive letter is a local path, never a saved host id.
        assert_eq!(remote(r"C:\work\file"), None);
    }

    #[test]
    fn scp_output_includes_safe_cleanup_advice_in_text_and_json() {
        let answer = annulus_control::protocol::TransferOutput {
            tab: 9,
            to: "/tmp/file".into(),
            bytes: 42,
            cleanup: annulus_control::protocol::TRANSFER_CLEANUP.into(),
        };
        for json in [false, true] {
            let text = printed(&answer, json);
            assert!(text.contains("42"));
            assert!(text.contains("only temporary files you created"));
            assert!(text.contains("source files"));
        }
    }
}
