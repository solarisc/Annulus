//! SFTP operations and the transfer queue.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

use std::io;
use std::path::{Path, PathBuf};

use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as ProtocolError;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use annulus_ssh::{Connection, SshError};

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

    /// Downloads the remote file `remote` into the local folder `local_dir`, under the
    /// remote file's name. `progress(done, total)` is called after each chunk and stops
    /// the transfer by returning `false`.
    ///
    /// # Errors
    /// The remote name is unsafe as a local name, reading or writing failed, or the
    /// transfer was cancelled. A partial local file is removed.
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
        let mut target = local_dir.join(name);
        if target.exists() {
            match conflict {
                Conflict::Skip => return Ok(Outcome::Skipped),
                Conflict::Overwrite => {}
                Conflict::KeepBoth => {
                    target = (1..KEEP_BOTH_TRIES)
                        .map(|n| local_dir.join(keep_both_name(name, n)))
                        .find(|candidate| !candidate.exists())
                        .unwrap_or(target);
                }
            }
        }
        let local_error = |path: &Path| {
            let path = path.to_path_buf();
            move |source| SftpError::Local { path, source }
        };
        let mut source = self.session.open(remote).await?;
        let total = source.metadata().await.map_or(0, |attrs| attrs.len());
        let mut sink = if conflict == Conflict::Overwrite {
            tokio::fs::File::create(&target).await
        } else {
            tokio::fs::File::create_new(&target).await
        }
        .map_err(local_error(&target))?;

        let copied = copy(&mut source, &mut sink, total, progress).await;
        let flushed = sink.flush().await;
        drop(sink);
        let _ = source.close().await;
        match (copied, flushed) {
            (Ok(bytes), Ok(())) => Ok(Outcome::Done {
                to: target.display().to_string(),
                bytes,
            }),
            (copied, flushed) => {
                let _ = tokio::fs::remove_file(&target).await;
                Err(match (copied, flushed) {
                    (Err(Copy::Cancelled), _) => SftpError::Cancelled,
                    (Err(Copy::Read(error)), _) => SftpError::Remote(error.to_string()),
                    (Err(Copy::Write(error)), _) | (_, Err(error)) => local_error(&target)(error),
                    (Ok(_), Ok(())) => unreachable!("handled above"),
                })
            }
        }
    }

    /// Uploads the local file `local` into the remote folder `remote_dir`, under its
    /// local name. `progress` works as in [`Sftp::download`].
    ///
    /// # Errors
    /// Reading or writing failed, or the transfer was cancelled. A partial remote file
    /// is removed.
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
        let mut target = join(remote_dir, name);
        if self.session.try_exists(target.as_str()).await? {
            match conflict {
                Conflict::Skip => return Ok(Outcome::Skipped),
                Conflict::Overwrite => {}
                Conflict::KeepBoth => {
                    for n in 1..KEEP_BOTH_TRIES {
                        let candidate = join(remote_dir, &keep_both_name(name, n));
                        if !self.session.try_exists(candidate.as_str()).await? {
                            target = candidate;
                            break;
                        }
                    }
                }
            }
        }
        let mut source = tokio::fs::File::open(local).await.map_err(local_error)?;
        let total = source.metadata().await.map_err(local_error)?.len();
        let mut flags = OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE;
        if conflict != Conflict::Overwrite {
            // Never replace a file that appeared since the check above.
            flags |= OpenFlags::EXCLUDE;
        }
        let mut sink = self.session.open_with_flags(target.as_str(), flags).await?;
        let copied = copy(&mut source, &mut sink, total, progress).await;
        let closed = sink.shutdown().await;
        match (copied, closed) {
            (Ok(bytes), Ok(())) => Ok(Outcome::Done { to: target, bytes }),
            (copied, closed) => {
                let _ = self.session.remove_file(target.as_str()).await;
                Err(match (copied, closed) {
                    (Err(Copy::Cancelled), _) => SftpError::Cancelled,
                    (Err(Copy::Read(error)), _) => local_error(error),
                    (Err(Copy::Write(error)), _) | (_, Err(error)) => {
                        SftpError::Remote(error.to_string())
                    }
                    (Ok(_), Ok(())) => unreachable!("handled above"),
                })
            }
        }
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
    total: u64,
    progress: &mut (dyn FnMut(u64, u64) -> bool + Send),
) -> Result<u64, Copy>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0; CHUNK];
    let mut done = 0u64;
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
