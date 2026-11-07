//! SFTP operations and the transfer queue.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use annulus_ssh::{Connection, SshError};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as ProtocolError;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

#[cfg(test)]
mod tests;

/// Bytes read or written per request.
const CHUNK: usize = 256 * 1024;

/// How many `name (n)` variants keep-both tries before giving up.
const KEEP_BOTH_TRIES: u32 = 10_000;

/// Why an SFTP operation failed.
#[derive(Debug, thiserror::Error)]
pub enum SftpError {
    /// Opening the SFTP channel failed.
    #[error(transparent)]
    Ssh(#[from] SshError),
    /// The server refused the request or the channel broke.
    #[error("{0}")]
    Remote(String),
    /// Reading or writing the local file failed.
    #[error("{}: {source}", path.display())]
    Local {
        /// The local file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// A remote file name that would escape the target folder or is not valid here.
    #[error("the name {0:?} cannot be saved on this computer")]
    UnsafeName(String),
    /// The progress callback asked to stop. The partial file was removed.
    #[error("cancelled")]
    Cancelled,
}

impl From<ProtocolError> for SftpError {
    fn from(error: ProtocolError) -> Self {
        SftpError::Remote(error.to_string())
    }
}

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link; `to_dir` when it resolves to a directory (F-SFTP-008).
    Link {
        /// The link points at a directory.
        to_dir: bool,
    },
    /// A device, socket or pipe.
    Other,
}

/// One entry of a remote directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The file name, without the directory.
    pub name: String,
    /// What it is.
    pub kind: Kind,
    /// Size in bytes (0 when the server does not say).
    pub size: u64,
    /// Last modification, seconds since the Unix epoch.
    pub modified: Option<u32>,
    /// Unix permission bits, without the file type.
    pub permissions: Option<u32>,
}

impl Entry {
    /// True for directories and links to directories: opening it navigates.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, Kind::Dir | Kind::Link { to_dir: true })
    }
}

/// What to do when the target of a transfer already exists (F-SFTP-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// Replace it.
    Overwrite,
    /// Leave it and do not transfer.
    Skip,
    /// Save under a free name: `name (1).ext`, `name (2).ext`, …
    KeepBoth,
}

/// How a transfer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Copied `bytes` to `to` (a local path or a remote path).
    Done {
        /// Where the copy is.
        to: String,
        /// Bytes copied.
        bytes: u64,
    },
    /// The target existed and the policy was [`Conflict::Skip`].
    Skipped,
}

/// An open SFTP session.
pub struct Sftp {
    session: SftpSession,
    /// Part files of transfers running now, so two never write the same one.
    in_flight: Mutex<HashSet<String>>,
}

/// Marks a part file busy until dropped.
struct Busy<'a> {
    set: &'a Mutex<HashSet<String>>,
    path: String,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.path);
        }
    }
}

impl Sftp {
    /// Opens SFTP on an authenticated connection.
    ///
    /// # Errors
    /// The server has no SFTP subsystem or the handshake failed.
    pub async fn open(connection: &Connection) -> Result<Self, SftpError> {
        Self::over(connection.sftp_stream().await?).await
    }

    /// Speaks SFTP over any byte stream (an SSH channel, or a pipe in tests).
    ///
    /// # Errors
    /// The version handshake failed.
    pub async fn over<S>(stream: S) -> Result<Self, SftpError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Ok(Sftp {
            session: SftpSession::new(stream).await?,
            in_flight: Mutex::new(HashSet::new()),
        })
    }

    /// The absolute path of the login directory.
    ///
    /// # Errors
    /// The server refused.
    pub async fn home(&self) -> Result<String, SftpError> {
        Ok(self.session.canonicalize(".").await?)
    }

    /// Lists `dir`: directories first, then files, each by name; `.` and `..` left out.
    ///
    /// # Errors
    /// The directory cannot be read.
    pub async fn list(&self, dir: &str) -> Result<Vec<Entry>, SftpError> {
        let mut entries = Vec::new();
        for item in self.session.read_dir(dir).await? {
            let name = item.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let attrs = item.metadata();
            let kind = if attrs.is_symlink() {
                let to_dir = self
                    .session
                    .metadata(join(dir, &name))
                    .await
                    .is_ok_and(|target| target.is_dir());
                Kind::Link { to_dir }
            } else {
                kind_of(&attrs)
            };
            entries.push(Entry {
                name,
                kind,
                size: attrs.len(),
                modified: attrs.mtime,
                permissions: attrs.permissions.map(|mode| mode & 0o7777),
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir()
                .cmp(&a.is_dir())
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    /// Creates the directory `path`.
    ///
    /// # Errors
    /// It exists, or the server refused.
    pub async fn create_dir(&self, path: &str) -> Result<(), SftpError> {
        Ok(self.session.create_dir(path).await?)
    }

    /// Renames or moves `from` to `to`.
    ///
    /// # Errors
    /// The server refused (some refuse when `to` exists).
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), SftpError> {
        Ok(self.session.rename(from, to).await?)
    }

    /// Sets the Unix permission bits of `path` (`0o644` and the like; type bits are
    /// ignored).
    ///
    /// # Errors
    /// The server refused, for example on a file of another user.
    pub async fn set_permissions(&self, path: &str, mode: u32) -> Result<(), SftpError> {
        let attrs = FileAttributes {
            permissions: Some(mode & 0o7777),
            ..FileAttributes::empty()
        };
        Ok(self.session.set_metadata(path, attrs).await?)
    }

    /// Removes `path`; a directory is removed with everything in it. Links are removed,
    /// never followed. The caller confirms with the user first.
    ///
    /// # Errors
    /// Something inside could not be removed; what was removed before stays removed.
    pub async fn remove(&self, path: &str) -> Result<(), SftpError> {
        let attrs = self.session.symlink_metadata(path).await?;
        if attrs.is_dir() {
            for entry in self.list(path).await? {
                Box::pin(self.remove(&join(path, &entry.name))).await?;
            }
            Ok(self.session.remove_dir(path).await?)
        } else {
            Ok(self.session.remove_file(path).await?)
        }
    }

    /// Marks `path` busy; `None` if another transfer is writing it.
    fn claim(&self, path: &str) -> Option<Busy<'_>> {
        let mut set = self.in_flight.lock().ok()?;
        set.insert(path.to_owned()).then(|| Busy {
            set: &self.in_flight,
            path: path.to_owned(),
        })
    }

    /// Downloads the remote file `remote` into the local folder `local_dir`, under the
    /// remote file's name. `progress(done, total)` is called after each chunk and stops
    /// the transfer by returning `false`.
    ///
    /// The bytes go to a hidden part file first, which takes the target's name only once
    /// complete: an existing file is replaced (with [`Conflict::Overwrite`]) only by a
    /// whole copy, never lost to a cancelled or failed one. The part file's name holds the
    /// remote file's size and modification time; when the connection fails it is kept, and
    /// the next download of the same, unchanged file continues from it (F-SFTP-004).
    ///
    /// # Errors
    /// The remote name is unsafe as a local name, reading or writing failed, or the
    /// transfer was cancelled. The part file is removed, unless reading from the server
    /// failed.
    pub async fn download(
        &self,
        remote: &str,
        local_dir: &Path,
        conflict: Conflict,
        progress: &mut (dyn FnMut(u64, u64) -> bool + Send),
    ) -> Result<Outcome, SftpError> {
        let name = file_name(remote);
        if !is_safe_local_name(name) {
            return Err(SftpError::UnsafeName(name.to_owned()));
        }
        let free = |path: &Path| !path.exists();
        let Some(target) = place(conflict, local_dir.join(name), &free, |n| {
            local_dir.join(keep_both_name(name, n))
        }) else {
            return Ok(Outcome::Skipped);
        };
        let local_error = |path: &Path| {
            let path = path.to_path_buf();
            move |source| SftpError::Local { path, source }
        };
        let mut source = self.session.open(remote).await?;
        let attrs = source.metadata().await.ok();
        let total = attrs.as_ref().map_or(0, FileAttributes::len);
        let stamp = format!("{total}-{}", attrs.and_then(|a| a.mtime).unwrap_or(0));
        let (part, mut sink, start, _busy) =
            self.local_part(local_dir, name, &stamp, total).await?;
        if start > 0 {
            source
                .seek(io::SeekFrom::Start(start))
                .await
                .map_err(|error| SftpError::Remote(error.to_string()))?;
        }

        let copied = copy(&mut source, &mut sink, start, total, progress).await;
        let flushed = sink.flush().await;
        drop(sink);
        let _ = source.close().await;
        let failure = match (copied, flushed) {
            (Ok(bytes), Ok(())) => {
                // Another file may have taken the name during the copy.
                let Some(target) = place(conflict, target, &free, |n| {
                    local_dir.join(keep_both_name(name, n))
                }) else {
                    let _ = tokio::fs::remove_file(&part).await;
                    return Ok(Outcome::Skipped);
                };
                match tokio::fs::rename(&part, &target).await {
                    Ok(()) => {
                        return Ok(Outcome::Done {
                            to: target.display().to_string(),
                            bytes,
                        });
                    }
                    Err(error) => local_error(&target)(error),
                }
            }
            (Err(Copy::Cancelled), _) => SftpError::Cancelled,
            // The server went away: keep what arrived, for the next try to continue.
            (Err(Copy::Read(error)), _) => return Err(SftpError::Remote(error.to_string())),
            (Err(Copy::Write(error)), _) | (_, Err(error)) => local_error(&part)(error),
        };
        let _ = tokio::fs::remove_file(&part).await;
        Err(failure)
    }

    /// The local part file for a download of `name` (`stamp` is the remote size and
    /// time): a kept one to continue, else a new one. Returns it open for appending, where
    /// to start, and the claim that keeps other transfers off it.
    async fn local_part(
        &self,
        dir: &Path,
        name: &str,
        stamp: &str,
        total: u64,
    ) -> Result<(PathBuf, tokio::fs::File, u64, Busy<'_>), SftpError> {
        let kept = dir.join(part_name(name, stamp, 0));
        if let Ok(meta) = tokio::fs::metadata(&kept).await
            && meta.is_file()
            && meta.len() <= total
            && let Some(busy) = self.claim(&kept.display().to_string())
        {
            let file = tokio::fs::OpenOptions::new()
                .append(true)
                .open(&kept)
                .await
                .map_err(|source| SftpError::Local {
                    path: kept.clone(),
                    source,
                })?;
            return Ok((kept, file, meta.len(), busy));
        }
        for n in 0..KEEP_BOTH_TRIES {
            let path = dir.join(part_name(name, stamp, n));
            let Some(busy) = self.claim(&path.display().to_string()) else {
                continue;
            };
            match tokio::fs::File::create_new(&path).await {
                Ok(file) => return Ok((path, file, 0, busy)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => return Err(SftpError::Local { path, source }),
            }
        }
        Err(SftpError::UnsafeName(name.to_owned()))
    }

    /// Uploads the local file `local` into the remote folder `remote_dir`, under its
    /// local name. `progress` works as in [`Sftp::download`], and so does the part file:
    /// an existing remote file is removed only once the new copy is complete, and a part
    /// file kept when the connection failed is continued by the next upload of the same,
    /// unchanged file.
    ///
    /// # Errors
    /// Reading or writing failed, or the transfer was cancelled. The part file is removed,
    /// unless writing to the server failed.
    pub async fn upload(
        &self,
        local: &Path,
        remote_dir: &str,
        conflict: Conflict,
        progress: &mut (dyn FnMut(u64, u64) -> bool + Send),
    ) -> Result<Outcome, SftpError> {
        let local_error = |source| SftpError::Local {
            path: local.to_path_buf(),
            source,
        };
        let name = local
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| SftpError::UnsafeName(local.display().to_string()))?;
        // Skip before copying anything; the target is chosen again once the copy is done.
        if self
            .place_remote(conflict, remote_dir, name)
            .await?
            .is_none()
        {
            return Ok(Outcome::Skipped);
        }
        let mut source = tokio::fs::File::open(local).await.map_err(local_error)?;
        let meta = source.metadata().await.map_err(local_error)?;
        let total = meta.len();
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());
        let (part, mut sink, start, _busy) = self
            .remote_part(remote_dir, name, &format!("{total}-{modified}"), total)
            .await?;
        if start > 0 {
            source
                .seek(io::SeekFrom::Start(start))
                .await
                .map_err(local_error)?;
        }
        let copied = copy(&mut source, &mut sink, start, total, progress).await;
        let closed = sink.shutdown().await;
        let failure = match (copied, closed) {
            (Ok(bytes), Ok(())) => {
                // Another file may have taken the name during the copy.
                let Some(target) = self.place_remote(conflict, remote_dir, name).await? else {
                    let _ = self.session.remove_file(part.as_str()).await;
                    return Ok(Outcome::Skipped);
                };
                // SFTP rename refuses an existing target on many servers, so the old file
                // goes first; the new one is complete by now.
                if conflict == Conflict::Overwrite
                    && self.session.try_exists(target.as_str()).await?
                {
                    self.session.remove_file(target.as_str()).await?;
                }
                match self.session.rename(part.as_str(), target.as_str()).await {
                    Ok(()) => return Ok(Outcome::Done { to: target, bytes }),
                    Err(error) => SftpError::from(error),
                }
            }
            (Err(Copy::Cancelled), _) => SftpError::Cancelled,
            (Err(Copy::Read(error)), _) => local_error(error),
            // The server went away: keep what arrived, for the next try to continue.
            (Err(Copy::Write(error)), _) | (_, Err(error)) => {
                return Err(SftpError::Remote(error.to_string()));
            }
        };
        let _ = self.session.remove_file(part.as_str()).await;
        Err(failure)
    }

    /// Downloads folder `remote` into `local_dir` with everything in it, as one transfer:
    /// each folder is made on this computer (or used as it is when there), each file comes
    /// as [`Self::download`] brings it under `conflict`, and links are skipped, never
    /// followed. A name this computer cannot hold safely (`..`, a separator, a device name)
    /// stops it before anything is written. `progress` sees the bytes of all the files.
    ///
    /// # Errors
    /// As [`Self::download`], or a folder cannot be listed or made; what was copied stays.
    pub async fn download_tree(
        &self,
        remote: &str,
        local_dir: &Path,
        conflict: Conflict,
        progress: &mut (dyn FnMut(u64, u64) -> bool + Send),
    ) -> Result<Outcome, SftpError> {
        let name = file_name(remote);
        if !is_safe_local_name(name) {
            return Err(SftpError::UnsafeName(name.to_owned()));
        }
        let top = local_dir.join(name);
        let (mut folders, mut files) = (Vec::new(), Vec::new());
        let mut stack = vec![(remote.to_owned(), top.clone())];
        while let Some((dir, local)) = stack.pop() {
            for entry in self.list(&dir).await? {
                if !is_safe_local_name(&entry.name) {
                    return Err(SftpError::UnsafeName(entry.name));
                }
                match entry.kind {
                    Kind::Dir => stack.push((join(&dir, &entry.name), local.join(&entry.name))),
                    Kind::File => files.push((join(&dir, &entry.name), local.clone(), entry.size)),
                    Kind::Link { .. } | Kind::Other => {}
                }
            }
            folders.push(local);
        }
        for folder in &folders {
            tokio::fs::create_dir_all(folder)
                .await
                .map_err(|source| SftpError::Local {
                    path: folder.clone(),
                    source,
                })?;
        }
        let total: u64 = files.iter().map(|(_, _, size)| size).sum();
        let (mut done, mut bytes) = (0_u64, 0_u64);
        for (path, dir, size) in files {
            let mut each = |copied: u64, _: u64| progress(done + copied, total);
            if let Outcome::Done { bytes: copied, .. } =
                self.download(&path, &dir, conflict, &mut each).await?
            {
                bytes += copied;
            }
            done += size;
            if !progress(done, total) {
                return Err(SftpError::Cancelled);
            }
        }
        Ok(Outcome::Done {
            to: top.display().to_string(),
            bytes,
        })
    }

    /// Uploads folder `local` into `remote_dir` with everything in it, as one transfer:
    /// each folder is made (or used as it is when there), each file goes as [`Self::upload`]
    /// sends it under `conflict`, and links are skipped, never followed. `progress` sees the
    /// bytes of all the files together.
    ///
    /// # Errors
    /// As [`Self::upload`], or a folder cannot be read or made; what was copied stays.
    pub async fn upload_tree(
        &self,
        local: &Path,
        remote_dir: &str,
        conflict: Conflict,
        progress: &mut (dyn FnMut(u64, u64) -> bool + Send),
    ) -> Result<Outcome, SftpError> {
        let name = local
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| SftpError::UnsafeName(local.display().to_string()))?;
        let top = join(remote_dir, name);
        // Everything is found first, so progress has a total; folders come before what is
        // in them.
        let (mut folders, mut files) = (Vec::new(), Vec::new());
        let mut stack = vec![(local.to_path_buf(), top.clone())];
        while let Some((dir, remote)) = stack.pop() {
            let read_error = |source| SftpError::Local {
                path: dir.clone(),
                source,
            };
            for entry in std::fs::read_dir(&dir).map_err(read_error)? {
                let entry = entry.map_err(read_error)?;
                let kind = entry.file_type().map_err(read_error)?;
                // A name that is not UTF-8 cannot be named on the host the same way.
                let Some(child) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if kind.is_dir() {
                    stack.push((entry.path(), join(&remote, &child)));
                } else if kind.is_file() {
                    let size = entry.metadata().map_err(read_error)?.len();
                    files.push((entry.path(), remote.clone(), size));
                }
            }
            folders.push(remote);
        }
        folders.sort_by_key(|folder| folder.matches('/').count());
        for folder in &folders {
            if !self.session.try_exists(folder.as_str()).await? {
                self.session.create_dir(folder.as_str()).await?;
            }
        }
        let total: u64 = files.iter().map(|(_, _, size)| size).sum();
        let (mut done, mut bytes) = (0_u64, 0_u64);
        for (path, dir, size) in files {
            let mut each = |copied: u64, _: u64| progress(done + copied, total);
            if let Outcome::Done { bytes: copied, .. } =
                self.upload(&path, &dir, conflict, &mut each).await?
            {
                bytes += copied;
            }
            done += size;
            if !progress(done, total) {
                return Err(SftpError::Cancelled);
            }
        }
        Ok(Outcome::Done { to: top, bytes })
    }

    /// The remote part file for an upload of `name` into `dir` (`stamp` is the local size
    /// and time): a kept one to continue, else a new one. Returns it open for writing at
    /// the end of what it holds, where to start, and the claim on it.
    async fn remote_part(
        &self,
        dir: &str,
        name: &str,
        stamp: &str,
        total: u64,
    ) -> Result<(String, russh_sftp::client::fs::File, u64, Busy<'_>), SftpError> {
        let kept = join(dir, &part_name(name, stamp, 0));
        if let Ok(attrs) = self.session.metadata(kept.as_str()).await
            && attrs.is_regular()
            && attrs.len() <= total
            && let Some(busy) = self.claim(&kept)
        {
            let mut file = self
                .session
                .open_with_flags(kept.as_str(), OpenFlags::WRITE)
                .await?;
            let start = attrs.len();
            file.seek(io::SeekFrom::Start(start))
                .await
                .map_err(|error| SftpError::Remote(error.to_string()))?;
            return Ok((kept, file, start, busy));
        }
        for n in 0..KEEP_BOTH_TRIES {
            let path = join(dir, &part_name(name, stamp, n));
            let Some(busy) = self.claim(&path) else {
                continue;
            };
            if self.session.try_exists(path.as_str()).await? {
                continue;
            }
            let flags = OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::EXCLUDE;
            let file = self.session.open_with_flags(path.as_str(), flags).await?;
            return Ok((path, file, 0, busy));
        }
        Err(SftpError::UnsafeName(name.to_owned()))
    }

    /// Where an upload of `name` into `dir` lands under `conflict`; `None` to skip.
    async fn place_remote(
        &self,
        conflict: Conflict,
        dir: &str,
        name: &str,
    ) -> Result<Option<String>, SftpError> {
        let target = join(dir, name);
        if !self.session.try_exists(target.as_str()).await? {
            return Ok(Some(target));
        }
        Ok(match conflict {
            Conflict::Skip => None,
            Conflict::Overwrite => Some(target),
            Conflict::KeepBoth => {
                let mut free = None;
                for n in 1..KEEP_BOTH_TRIES {
                    let candidate = join(dir, &keep_both_name(name, n));
                    if !self.session.try_exists(candidate.as_str()).await? {
                        free = Some(candidate);
                        break;
                    }
                }
                free
            }
        })
    }
}

/// Where a download lands under `conflict`, given whether a path is `free` and the
/// keep-both candidates; `None` to skip.
fn place(
    conflict: Conflict,
    target: PathBuf,
    free: &dyn Fn(&Path) -> bool,
    candidate: impl Fn(u32) -> PathBuf,
) -> Option<PathBuf> {
    if free(&target) {
        return Some(target);
    }
    match conflict {
        Conflict::Skip => None,
        Conflict::Overwrite => Some(target),
        Conflict::KeepBoth => (1..KEEP_BOTH_TRIES).map(candidate).find(|path| free(path)),
    }
}

/// The hidden name a transfer of `name` writes to until it is complete. `stamp` is the
/// source's size and modification time, so a kept part is continued only for the same,
/// unchanged source.
fn part_name(name: &str, stamp: &str, n: u32) -> String {
    if n == 0 {
        format!(".{name}.{stamp}.annulus-part")
    } else {
        format!(".{name}.{stamp}.{n}.annulus-part")
    }
}

/// Why [`copy`] stopped.
enum Copy {
    Read(io::Error),
    Write(io::Error),
    Cancelled,
}

async fn copy<R, W>(
    source: &mut R,
    sink: &mut W,
    start: u64,
    total: u64,
    progress: &mut (dyn FnMut(u64, u64) -> bool + Send),
) -> Result<u64, Copy>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0; CHUNK];
    let mut done = start;
    loop {
        let read = source.read(&mut buffer).await.map_err(Copy::Read)?;
        if read == 0 {
            return Ok(done);
        }
        sink.write_all(&buffer[..read]).await.map_err(Copy::Write)?;
        done += read as u64;
        if !progress(done, total.max(done)) {
            return Err(Copy::Cancelled);
        }
    }
}

fn kind_of(attrs: &FileAttributes) -> Kind {
    if attrs.is_dir() {
        Kind::Dir
    } else if attrs.is_regular() || attrs.permissions.is_none() {
        Kind::File
    } else {
        Kind::Other
    }
}

/// Joins a remote directory and a name with `/`.
#[must_use]
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The remote parent directory of `path`; the root is its own parent.
#[must_use]
pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(at) => trimmed[..at].to_owned(),
    }
}

/// The last component of a remote path.
#[must_use]
pub fn file_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
}

/// `report.txt` with `n = 2` becomes `report (2).txt`; names without an extension, and
/// dotfiles, get the number at the end.
#[must_use]
pub fn keep_both_name(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(at) if at > 0 => format!("{} ({n}){}", &name[..at], &name[at..]),
        _ => format!("{name} ({n})"),
    }
}

/// A remote name is written locally only if it stays inside the target folder and is
/// valid on every OS: no separators, no `..`, no drive or stream colon, no control
/// characters, and none of the names Windows reserves.
#[must_use]
pub fn is_safe_local_name(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or_default();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        && !RESERVED
            .iter()
            .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}
