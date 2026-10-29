//! Writing the vault's files so a crash or a power cut never leaves a half-written or empty
//! file, and nobody else on the machine can read them.
//!
//! The new text goes to a fresh temporary file (owner-only on Unix), is flushed to disk, the
//! previous file is kept as `<name>.bak`, then the temporary file takes the name and the folder
//! is flushed too. A reader sees the old file or the new one, never a mix.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Replaces `path` with `bytes` as described above; `backup` keeps the previous contents as
/// `<name>.bak` first.
///
/// # Errors
/// Any file-system error; `path` is then unchanged.
pub fn write_private(path: &Path, bytes: &[u8], backup: bool) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if backup && path.exists() {
            std::fs::copy(path, backup_path(path))?;
        }
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written?;
    // The rename itself survives a power cut once the folder is flushed (Unix; NTFS journals
    // it).
    #[cfg(unix)]
    if let Some(parent) = parent {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Where [`write_private`] keeps the previous contents of `path`.
#[must_use]
pub fn backup_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.bak"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("annulus-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_write_replaces_the_file_keeps_the_last_one_and_leaves_no_temporary() {
        let dir = folder("replace");
        let path = dir.join("vault.anv");
        write_private(&path, b"first", true).unwrap();
        assert!(
            !backup_path(&path).exists(),
            "nothing to keep the first time"
        );
        write_private(&path, b"second", true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), b"first");
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_can_read_the_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = folder("private").join("vault.anv");
        write_private(&path, b"secret", false).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_alone() {
        let dir = folder("failed");
        let path = dir.join("vault.anv");
        write_private(&path, b"kept", false).unwrap();
        // A folder where the backup would go makes the write fail before the rename.
        std::fs::create_dir_all(backup_path(&path)).unwrap();
        assert!(write_private(&path, b"lost", true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"kept");
    }
}
