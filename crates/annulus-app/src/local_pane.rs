//! The local side of the SFTP tab (F-SFTP-001): a folder on this computer, browsed like the
//! remote one. Downloads land in the folder shown; the selected file uploads to the remote
//! folder shown.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

use annulus_platform::window::Waker;

/// One entry of a local folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalEntry {
    /// The file name.
    pub name: String,
    /// A folder (or a link to one).
    pub is_dir: bool,
    /// A symbolic link.
    pub link: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// Last modification, seconds since the Unix epoch.
    pub modified: Option<u32>,
    /// Last access, seconds since the Unix epoch.
    pub accessed: Option<u32>,
    /// Creation, where the file system keeps it.
    pub created: Option<u32>,
    /// What it allows: `rwxr-xr-x` on Unix; on Windows Read-only or Read/write, and
    /// whether hidden or a system file.
    pub permissions: Option<String>,
    /// The owner's name (or id) on Unix.
    pub owner: Option<String>,
    /// The group's name (or id) on Unix.
    pub group: Option<String>,
}

impl LocalEntry {
    /// The row the SFTP tab's columns show for it.
    #[must_use]
    pub(crate) fn row(&self) -> crate::sftp_view::Row {
        use annulus_sftp::Kind;
        crate::sftp_view::Row {
            name: self.name.clone(),
            kind: match (self.link, self.is_dir) {
                (true, to_dir) => Kind::Link { to_dir },
                (false, true) => Kind::Dir,
                (false, false) => Kind::File,
            },
            size: self.size,
            modified: self.modified,
            accessed: self.accessed,
            created: self.created,
            permissions: self.permissions.clone(),
            owner: self.owner.clone(),
            group: self.group.clone(),
        }
    }
}

/// Seconds since the Unix epoch of `time`, when it fits.
fn epoch(time: std::io::Result<std::time::SystemTime>) -> Option<u32> {
    time.ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| u32::try_from(since.as_secs()).ok())
}

/// What `meta` allows, and who owns it, as the columns show them.
fn access(meta: &std::fs::Metadata) -> (Option<String>, Option<String>, Option<String>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let owners = unix_owners();
        let named = |id: u32, names: &std::collections::HashMap<u32, String>| {
            names.get(&id).cloned().unwrap_or_else(|| id.to_string())
        };
        (
            Some(crate::sftp_view::mode_text(meta.mode())),
            Some(named(meta.uid(), &owners.users)),
            Some(named(meta.gid(), &owners.groups)),
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        (Some(windows_access(meta.file_attributes())), None, None)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = meta;
        (None, None, None)
    }
}

/// Windows file attributes as words: Read-only or Read/write, then hidden and system.
#[cfg_attr(not(windows), allow(dead_code))] // Tested everywhere, used on Windows.
fn windows_access(attributes: u32) -> String {
    const READ_ONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    let mut words = String::from(if attributes & READ_ONLY == 0 {
        "Read/write"
    } else {
        "Read-only"
    });
    if attributes & HIDDEN != 0 {
        words.push_str(", hidden");
    }
    if attributes & SYSTEM != 0 {
        words.push_str(", system");
    }
    words
}

/// This computer's user and group names, read once from `/etc/passwd` and `/etc/group`.
#[cfg(unix)]
fn unix_owners() -> &'static annulus_sftp::Owners {
    static OWNERS: std::sync::OnceLock<annulus_sftp::Owners> = std::sync::OnceLock::new();
    OWNERS.get_or_init(|| {
        let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
        annulus_sftp::Owners {
            users: annulus_sftp::id_names(&read("/etc/passwd")),
            groups: annulus_sftp::id_names(&read("/etc/group")),
        }
    })
}

/// `path` with the home folder `home` written as `~`, the way a shell prompt shows it:
/// `/home/me/app` is `~/app`, `C:\Users\me\Downloads` is `~\Downloads`.
#[must_use]
pub fn tilde(path: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return path.to_owned();
    }
    match path.strip_prefix(home) {
        Some("") => "~".to_owned(),
        Some(rest) if rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => path.to_owned(),
    }
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
        let link = meta.file_type().is_symlink();
        // A link is a folder when what it points at is one.
        let is_dir = if link {
            std::fs::metadata(entry.path()).is_ok_and(|target| target.is_dir())
        } else {
            meta.is_dir()
        };
        let (permissions, owner, group) = access(&meta);
        entries.push(LocalEntry {
            name,
            is_dir,
            link,
            size: if is_dir { 0 } else { meta.len() },
            modified: epoch(meta.modified()),
            accessed: epoch(meta.accessed()),
            created: epoch(meta.created()),
            permissions,
            owner,
            group,
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
    worker: Option<Sender<LocalRequest>>,
    replies: Receiver<LocalReply>,
    next_request: u64,
    latest_list: u64,
    dirty: bool,
    pub(crate) loading: bool,
}

/// A local file change that finished on the pane's worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocalChange {
    Created(String),
    Renamed(String),
    Deleted(String),
}

enum LocalWork {
    List,
    CreateDir(String),
    Rename { from: String, to: String },
    Delete { path: PathBuf, folder: bool },
}

struct LocalRequest {
    id: u64,
    dir: PathBuf,
    hidden: bool,
    work: LocalWork,
    waker: Option<Waker>,
}

struct LocalReply {
    id: u64,
    dir: PathBuf,
    entries: std::io::Result<Vec<LocalEntry>>,
    change: Option<Result<LocalChange, String>>,
}

impl LocalPane {
    /// A pane showing `dir`.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        let (requests, work) = mpsc::channel();
        let (news, replies) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("annulus-local-files".to_owned())
            .spawn(move || local_worker(&work, &news))
            .map(|_| requests)
            .ok();
        Self {
            path_field: dir.display().to_string(),
            dir,
            entries: Vec::new(),
            error: None,
            worker,
            replies,
            next_request: 0,
            latest_list: 0,
            dirty: true,
            loading: false,
        }
    }

    /// A ready pane for the built-in preview, which must render the same first frame each time.
    pub(crate) fn new_loaded(dir: PathBuf) -> Self {
        let mut pane = Self::new(dir);
        pane.apply_list(read_dir(&pane.dir, false));
        pane.dirty = false;
        pane
    }

    fn apply_list(&mut self, result: std::io::Result<Vec<LocalEntry>>) {
        self.loading = false;
        match result {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Cannot read {}: {error}", self.dir.display())),
        }
        self.path_field = self.dir.display().to_string();
    }

    fn request(&mut self, work: LocalWork, hidden: bool, waker: Option<&Waker>) {
        self.next_request = self.next_request.wrapping_add(1).max(1);
        self.latest_list = self.next_request;
        self.dirty = false;
        self.loading = true;
        let request = LocalRequest {
            id: self.next_request,
            dir: self.dir.clone(),
            hidden,
            work,
            waker: waker.cloned(),
        };
        let sent = self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.send(request).is_ok());
        if !sent {
            self.loading = false;
            self.error = Some("The local file worker stopped".to_owned());
        }
    }

    /// Reads the shown folder on the local file worker.
    pub fn refresh(&mut self, hidden: bool, waker: Option<&Waker>) {
        self.request(LocalWork::List, hidden, waker);
    }

    /// Shows `dir` and reads it on the local file worker.
    pub(crate) fn navigate(&mut self, dir: PathBuf, hidden: bool, waker: Option<&Waker>) {
        self.dir = dir;
        self.path_field = self.dir.display().to_string();
        self.entries.clear();
        self.error = None;
        self.refresh(hidden, waker);
    }

    /// Marks the shown folder for a read on the next frame, when a waker is at hand.
    pub(crate) fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Starts a read that a background transfer requested.
    pub(crate) fn refresh_if_dirty(&mut self, hidden: bool, waker: Option<&Waker>) {
        if self.dirty {
            self.refresh(hidden, waker);
        }
    }

    pub(crate) fn create_dir(&mut self, name: String, hidden: bool, waker: Option<&Waker>) {
        self.request(LocalWork::CreateDir(name), hidden, waker);
    }

    pub(crate) fn rename(&mut self, from: String, to: String, hidden: bool, waker: Option<&Waker>) {
        self.request(LocalWork::Rename { from, to }, hidden, waker);
    }

    pub(crate) fn delete(
        &mut self,
        path: PathBuf,
        folder: bool,
        hidden: bool,
        waker: Option<&Waker>,
    ) {
        self.request(LocalWork::Delete { path, folder }, hidden, waker);
    }

    /// Applies finished reads and returns finished file changes.
    pub(crate) fn drain(&mut self) -> Vec<Result<LocalChange, String>> {
        let replies: Vec<_> = self.replies.try_iter().collect();
        let mut changes = Vec::new();
        for reply in replies {
            if reply.id == self.latest_list && reply.dir == self.dir {
                self.apply_list(reply.entries);
            }
            if let Some(change) = reply.change {
                changes.push(change);
            }
        }
        changes
    }
}

fn local_worker(requests: &Receiver<LocalRequest>, replies: &Sender<LocalReply>) {
    while let Ok(request) = requests.recv() {
        let change = match request.work {
            LocalWork::List => None,
            LocalWork::CreateDir(name) => Some(
                std::fs::create_dir(request.dir.join(&name))
                    .map(|()| LocalChange::Created(name))
                    .map_err(|error| format!("Could not create the folder: {error}")),
            ),
            LocalWork::Rename { from, to } => {
                let target = request.dir.join(&to);
                let result = if target.symlink_metadata().is_ok() && from != to {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "that name is taken",
                    ))
                } else {
                    std::fs::rename(request.dir.join(from), target)
                };
                Some(
                    result
                        .map(|()| LocalChange::Renamed(to))
                        .map_err(|error| format!("Could not rename the item: {error}")),
                )
            }
            LocalWork::Delete { path, folder } => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into(),
                );
                let result = if folder {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_file(path)
                };
                Some(
                    result
                        .map(|()| LocalChange::Deleted(name))
                        .map_err(|error| format!("Could not delete the item: {error}")),
                )
            }
        };
        let entries = read_dir(&request.dir, request.hidden);
        let reply = LocalReply {
            id: request.id,
            dir: request.dir,
            entries,
            change,
        };
        if replies.send(reply).is_err() {
            break;
        }
        if let Some(waker) = request.waker {
            waker();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_folder_is_written_as_a_tilde() {
        assert_eq!(tilde("/home/me/app", "/home/me"), "~/app");
        assert_eq!(tilde("/home/me", "/home/me/"), "~");
        assert_eq!(
            tilde("/home/meadow", "/home/me"),
            "/home/meadow",
            "not a prefix by name"
        );
        assert_eq!(
            tilde("C:\\Users\\me\\Downloads", "C:\\Users\\me"),
            "~\\Downloads"
        );
        assert_eq!(tilde("/srv/app", ""), "/srv/app");
    }

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

    #[test]
    fn this_computers_files_say_what_they_allow_and_when_they_were_used() {
        let dir = std::env::temp_dir().join(format!("annulus-local-access-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("locked.txt");
        std::fs::write(&file, b"x").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();
        let entry = read_dir(&dir, false).unwrap().remove(0);
        let shown = entry.permissions.clone().unwrap();
        if cfg!(windows) {
            assert_eq!(shown, "Read-only");
        } else {
            assert!(shown.starts_with("r-"), "{shown}");
            assert!(entry.owner.is_some() && entry.group.is_some());
        }
        assert!(entry.modified.is_some() && entry.accessed.is_some());
        assert_eq!(entry.row().cell(crate::sftp_view::Column::Type), "TXT file");
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)] // Only so the test can clean up.
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_attributes_read_as_words() {
        assert_eq!(windows_access(0x20), "Read/write");
        assert_eq!(windows_access(0x1 | 0x2), "Read-only, hidden");
        assert_eq!(windows_access(0x2 | 0x4), "Read/write, hidden, system");
    }

    #[test]
    fn changes_run_on_the_worker_and_refresh_the_cached_list() {
        let dir = std::env::temp_dir().join(format!(
            "annulus-local-worker-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut pane = LocalPane::new(dir.clone());
        let wait = |pane: &mut LocalPane| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(change) = pane.drain().into_iter().next() {
                    break change.unwrap();
                }
                assert!(
                    std::time::Instant::now() < until,
                    "the worker did not reply"
                );
                std::thread::yield_now();
            }
        };

        pane.create_dir("first".to_owned(), false, None);
        assert_eq!(wait(&mut pane), LocalChange::Created("first".to_owned()));
        assert_eq!(pane.entries[0].name, "first");

        pane.rename("first".to_owned(), "second".to_owned(), false, None);
        assert_eq!(wait(&mut pane), LocalChange::Renamed("second".to_owned()));
        assert_eq!(pane.entries[0].name, "second");

        pane.delete(dir.join("second"), true, false, None);
        assert_eq!(wait(&mut pane), LocalChange::Deleted("second".to_owned()));
        assert_eq!(pane.entries, Vec::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pending_folder_read_has_a_loading_state() {
        let dir = std::env::temp_dir().join(format!(
            "annulus-local-loading-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut pane = LocalPane::new(dir.clone());
        pane.refresh(false, None);
        assert!(pane.loading);

        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pane.loading {
            pane.drain();
            assert!(
                std::time::Instant::now() < until,
                "the worker did not reply"
            );
            std::thread::yield_now();
        }
        assert!(pane.error.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
