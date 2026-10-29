//! The local side of the SFTP tab (F-SFTP-001): a folder on this computer, browsed like the
//! remote one. Downloads land in the folder shown; the selected file uploads to the remote
//! folder shown.

use std::path::{Path, PathBuf};

/// One entry of a local folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalEntry {
    /// The file name.
    pub name: String,
    /// A folder.
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
}

/// The entries of `dir`: folders first, then files, by name; hidden ones (dot files) only
/// when `hidden`.
pub fn read_dir(dir: &Path, hidden: bool) -> std::io::Result<Vec<LocalEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        entries.push(LocalEntry {
            name,
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// The local pane's state.
pub struct LocalPane {
    /// The folder shown.
    pub dir: PathBuf,
    pub(crate) entries: Vec<LocalEntry>,
    pub(crate) error: Option<String>,
    pub(crate) path_field: String,
}

impl LocalPane {
    /// A pane showing `dir`.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        let mut pane = Self {
            path_field: dir.display().to_string(),
            dir,
            entries: Vec::new(),
            error: None,
        };
        pane.refresh(false);
        pane
    }

    /// Reads the folder again (after a download, or on Refresh).
    pub fn refresh(&mut self, hidden: bool) {
        match read_dir(&self.dir, hidden) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Cannot read {}: {error}", self.dir.display())),
        }
        self.path_field = self.dir.display().to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_first_and_dot_files_hide_unless_asked() {
        let dir = std::env::temp_dir().join(format!("annulus-local-pane-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Zeta")).unwrap();
        std::fs::write(dir.join("alpha.txt"), b"12345").unwrap();
        std::fs::write(dir.join(".hidden"), b"").unwrap();
        let names: Vec<String> = read_dir(&dir, false)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["Zeta", "alpha.txt"]);
        let all = read_dir(&dir, true).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().find(|e| e.name == "alpha.txt").unwrap().size, 5);
    }
}
