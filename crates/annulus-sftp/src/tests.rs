//! Runs the client against russh-sftp's server half over an in-memory pipe, with the
//! server's files in a temp folder.

// The server trait is async; this disk-backed one does its I/O synchronously.
#![allow(clippy::unused_async_trait_impl)]

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use russh_sftp::protocol::{
    Attrs, Data, File as NameEntry, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
};

use super::*;

/// A tiny SFTP server rooted at a local folder; remote `/x` is `root/x`.
struct DiskServer {
    root: PathBuf,
    files: HashMap<String, fs::File>,
    dirs: HashMap<String, bool>,
    next: u32,
}

impl DiskServer {
    fn local(&self, path: &str) -> PathBuf {
        let relative = path.trim_start_matches('/');
        if relative.is_empty() || relative == "." {
            self.root.clone()
        } else {
            self.root.join(relative)
        }
    }

    fn ok(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en-US".into(),
        }
    }
}

fn status(error: &std::io::Error) -> StatusCode {
    match error.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

impl russh_sftp::server::Handler for DiskServer {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let path = if path == "." { "/".to_owned() } else { path };
        Ok(Name {
            id,
            files: vec![NameEntry::dummy(path)],
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        if !self.local(&path).is_dir() {
            return Err(StatusCode::NoSuchFile);
        }
        self.dirs.insert(path.clone(), false);
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if self.dirs.insert(handle.clone(), true) != Some(false) {
            return Err(StatusCode::Eof);
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(self.local(&handle)).map_err(|e| status(&e))? {
            let entry = entry.map_err(|e| status(&e))?;
            let meta = fs::symlink_metadata(entry.path()).map_err(|e| status(&e))?;
            files.push(NameEntry::new(
                entry.file_name().to_string_lossy(),
                FileAttributes::from(&meta),
            ));
        }
        Ok(Name { id, files })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.local(&filename);
        let file = fs::OpenOptions::new()
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE))
            .create(pflags.contains(OpenFlags::CREATE) && !pflags.contains(OpenFlags::EXCLUDE))
            .create_new(pflags.contains(OpenFlags::EXCLUDE))
            .open(path)
            .map_err(|e| status(&e))?;
        self.next += 1;
        let handle = format!("f{}", self.next);
        self.files.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset)).map_err(|e| status(&e))?;
        let mut data = vec![0; len as usize];
        let read = file.read(&mut data).map_err(|e| status(&e))?;
        if read == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(read);
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset)).map_err(|e| status(&e))?;
        file.write_all(&data).map_err(|e| status(&e))?;
        Ok(Self::ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.files.remove(&handle);
        self.dirs.remove(&handle);
        Ok(Self::ok(id))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let file = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        let meta = file.metadata().map_err(|e| status(&e))?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = fs::metadata(self.local(&path)).map_err(|e| status(&e))?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = fs::symlink_metadata(self.local(&path)).map_err(|e| status(&e))?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        fs::create_dir(self.local(&path)).map_err(|e| status(&e))?;
        Ok(Self::ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        fs::remove_file(self.local(&filename)).map_err(|e| status(&e))?;
        Ok(Self::ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        fs::remove_dir(self.local(&path)).map_err(|e| status(&e))?;
        Ok(Self::ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        fs::rename(self.local(&oldpath), self.local(&newpath)).map_err(|e| status(&e))?;
        Ok(Self::ok(id))
    }
}

/// A fresh empty folder for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("annulus-sftp-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A client connected to a [`DiskServer`] serving `root`.
async fn client(root: PathBuf) -> Sftp {
    let (near, far) = tokio::io::duplex(1 << 20);
    russh_sftp::server::run(
        far,
        DiskServer {
            root,
            files: HashMap::new(),
            dirs: HashMap::new(),
            next: 0,
        },
    )
    .await;
    Sftp::over(near).await.unwrap()
}

fn no_progress() -> impl FnMut(u64, u64) -> bool + Send {
    |_, _| true
}

#[tokio::test]
async fn listing_puts_directories_first_and_reports_sizes() {
    let root = temp_dir("list");
    fs::write(root.join("b.txt"), b"hello").unwrap();
    fs::write(root.join("a.txt"), b"").unwrap();
    fs::create_dir(root.join("zdir")).unwrap();
    let sftp = client(root).await;

    assert_eq!(sftp.home().await.unwrap(), "/");
    let entries = sftp.list("/").await.unwrap();
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["zdir", "a.txt", "b.txt"]);
    assert!(entries[0].is_dir());
    assert_eq!(entries[2].kind, Kind::File);
    assert_eq!(entries[2].size, 5);
}

#[tokio::test]
async fn a_download_copies_the_bytes_and_reports_progress() {
    let remote = temp_dir("down-remote");
    let local = temp_dir("down-local");
    // Larger than one chunk, so the copy loop runs more than once.
    let content: Vec<u8> = (0..(CHUNK * 2 + 17))
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    fs::write(remote.join("data.bin"), &content).unwrap();
    let sftp = client(remote).await;

    let mut calls = Vec::new();
    let outcome = sftp
        .download(
            "/data.bin",
            &local,
            Conflict::KeepBoth,
            &mut |done, total| {
                calls.push((done, total));
                true
            },
        )
        .await
        .unwrap();

    assert_eq!(fs::read(local.join("data.bin")).unwrap(), content);
    assert!(matches!(outcome, Outcome::Done { bytes, .. } if bytes == content.len() as u64));
    assert_eq!(
        calls.last(),
        Some(&(content.len() as u64, content.len() as u64))
    );
}

#[tokio::test]
async fn a_download_never_replaces_a_local_file_unless_told_to() {
    let remote = temp_dir("conflict-remote");
    let local = temp_dir("conflict-local");
    fs::write(remote.join("notes.txt"), b"remote").unwrap();
    fs::write(local.join("notes.txt"), b"local").unwrap();
    let sftp = client(remote).await;

    let skipped = sftp
        .download("/notes.txt", &local, Conflict::Skip, &mut no_progress())
        .await
        .unwrap();
    assert_eq!(skipped, Outcome::Skipped);
    sftp.download("/notes.txt", &local, Conflict::KeepBoth, &mut no_progress())
        .await
        .unwrap();
    assert_eq!(fs::read(local.join("notes.txt")).unwrap(), b"local");
    assert_eq!(fs::read(local.join("notes (1).txt")).unwrap(), b"remote");

    sftp.download(
        "/notes.txt",
        &local,
        Conflict::Overwrite,
        &mut no_progress(),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(local.join("notes.txt")).unwrap(), b"remote");
}

#[tokio::test]
async fn a_cancelled_download_leaves_no_partial_file() {
    let remote = temp_dir("cancel-remote");
    let local = temp_dir("cancel-local");
    fs::write(remote.join("big.bin"), vec![7u8; CHUNK * 3]).unwrap();
    let sftp = client(remote).await;

    let result = sftp
        .download("/big.bin", &local, Conflict::KeepBoth, &mut |_, _| false)
        .await;

    assert!(matches!(result, Err(SftpError::Cancelled)));
    assert!(!local.join("big.bin").exists());
}

/// The part-file stamp of `path` as a transfer sees it: size and modification time.
fn stamp_of(path: &Path) -> String {
    let meta = fs::metadata(path).unwrap();
    let modified = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("{}-{modified}", meta.len())
}

#[tokio::test]
async fn a_kept_part_of_the_same_file_is_continued_and_a_stale_one_is_not() {
    let remote = temp_dir("resume-remote");
    let local = temp_dir("resume-local");
    let body: Vec<u8> = (0..CHUNK * 3)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    fs::write(remote.join("big.bin"), &body).unwrap();
    let stamp = stamp_of(&remote.join("big.bin"));
    // An earlier try got the first chunk before the connection dropped.
    fs::write(
        local.join(format!(".big.bin.{stamp}.annulus-part")),
        &body[..CHUNK],
    )
    .unwrap();
    // A part of an older version of the file must not be continued.
    fs::write(local.join(".big.bin.1-1.annulus-part"), b"stale").unwrap();
    let sftp = client(remote).await;

    let mut first = None;
    let outcome = sftp
        .download("/big.bin", &local, Conflict::KeepBoth, &mut |done, _| {
            first.get_or_insert(done);
            true
        })
        .await
        .unwrap();
    assert!(matches!(outcome, Outcome::Done { bytes, .. } if bytes == body.len() as u64));
    // Without the kept chunk the first report would be at most one chunk.
    assert!(
        first.unwrap() > CHUNK as u64,
        "it started after the kept chunk"
    );
    assert_eq!(fs::read(local.join("big.bin")).unwrap(), body);
    assert_eq!(names(&local), [".big.bin.1-1.annulus-part", "big.bin"]);
}

#[tokio::test]
async fn an_upload_continues_a_kept_remote_part() {
    let remote = temp_dir("resume-up-remote");
    let local = temp_dir("resume-up-local");
    let body: Vec<u8> = (0..CHUNK * 2 + 7)
        .map(|i| u8::try_from(i % 249).unwrap())
        .collect();
    fs::write(local.join("up.bin"), &body).unwrap();
    let stamp = stamp_of(&local.join("up.bin"));
    fs::write(
        remote.join(format!(".up.bin.{stamp}.annulus-part")),
        &body[..CHUNK],
    )
    .unwrap();
    let sftp = client(remote.clone()).await;

    let mut first = None;
    sftp.upload(
        &local.join("up.bin"),
        "/",
        Conflict::KeepBoth,
        &mut |done, _| {
            first.get_or_insert(done);
            true
        },
    )
    .await
    .unwrap();
    assert!(
        first.unwrap() > CHUNK as u64,
        "it started after the kept chunk"
    );
    assert_eq!(fs::read(remote.join("up.bin")).unwrap(), body);
    assert_eq!(names(&remote), ["up.bin"]);
}

/// The file names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_cancelled_overwrite_keeps_the_original_on_both_sides() {
    let remote = temp_dir("keep-remote");
    let local = temp_dir("keep-local");
    fs::write(remote.join("notes.txt"), vec![1u8; CHUNK * 2]).unwrap();
    fs::write(local.join("notes.txt"), b"local").unwrap();
    let sftp = client(remote.clone()).await;

    let down = sftp
        .download("/notes.txt", &local, Conflict::Overwrite, &mut |_, _| false)
        .await;
    assert!(matches!(down, Err(SftpError::Cancelled)));
    assert_eq!(fs::read(local.join("notes.txt")).unwrap(), b"local");
    assert_eq!(names(&local), ["notes.txt"]);

    fs::write(local.join("big.txt"), vec![2u8; CHUNK * 2]).unwrap();
    fs::write(remote.join("big.txt"), b"remote").unwrap();
    let up = sftp
        .upload(
            &local.join("big.txt"),
            "/",
            Conflict::Overwrite,
            &mut |_, _| false,
        )
        .await;
    assert!(matches!(up, Err(SftpError::Cancelled)));
    assert_eq!(fs::read(remote.join("big.txt")).unwrap(), b"remote");
    assert_eq!(names(&remote), ["big.txt", "notes.txt"]);
}

#[tokio::test]
async fn an_upload_lands_under_its_local_name_and_respects_conflicts() {
    let remote = temp_dir("up-remote");
    let local = temp_dir("up-local");
    fs::create_dir(remote.join("inbox")).unwrap();
    fs::write(remote.join("inbox/report.txt"), b"old").unwrap();
    fs::write(local.join("report.txt"), b"new").unwrap();
    let sftp = client(remote.clone()).await;

    let outcome = sftp
        .upload(
            &local.join("report.txt"),
            "/inbox",
            Conflict::KeepBoth,
            &mut no_progress(),
        )
        .await
        .unwrap();
    assert_eq!(
        outcome,
        Outcome::Done {
            to: "/inbox/report (1).txt".into(),
            bytes: 3
        }
    );
    assert_eq!(fs::read(remote.join("inbox/report.txt")).unwrap(), b"old");
    assert_eq!(
        fs::read(remote.join("inbox/report (1).txt")).unwrap(),
        b"new"
    );

    sftp.upload(
        &local.join("report.txt"),
        "/inbox",
        Conflict::Overwrite,
        &mut no_progress(),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(remote.join("inbox/report.txt")).unwrap(), b"new");
}

#[tokio::test]
async fn a_folder_uploads_with_everything_in_it() {
    let remote = temp_dir("tree-remote");
    let local = temp_dir("tree-local");
    fs::create_dir_all(local.join("site/assets/img")).unwrap();
    fs::write(local.join("site/index.html"), b"<html>").unwrap();
    fs::write(local.join("site/assets/app.css"), b"body{}").unwrap();
    fs::write(local.join("site/assets/img/logo.svg"), b"<svg/>").unwrap();
    fs::create_dir(remote.join("www")).unwrap();
    let sftp = client(remote.clone()).await;
    let mut seen = (0, 0);
    let mut progress = |done, total| {
        seen = (done, total);
        true
    };
    let outcome = sftp
        .upload_tree(
            &local.join("site"),
            "/www",
            Conflict::KeepBoth,
            &mut progress,
        )
        .await
        .unwrap();
    assert_eq!(
        outcome,
        Outcome::Done {
            to: "/www/site".into(),
            bytes: 18
        }
    );
    assert_eq!(seen, (18, 18), "progress counts every file");
    assert_eq!(
        fs::read(remote.join("www/site/index.html")).unwrap(),
        b"<html>"
    );
    assert_eq!(
        fs::read(remote.join("www/site/assets/img/logo.svg")).unwrap(),
        b"<svg/>"
    );
    // Again into the same folder: the folders are reused, the files follow the conflict rule.
    sftp.upload_tree(
        &local.join("site"),
        "/www",
        Conflict::Skip,
        &mut no_progress(),
    )
    .await
    .unwrap();
    assert_eq!(names(&remote.join("www/site")), ["assets", "index.html"]);
}

#[tokio::test]
async fn folders_can_be_made_renamed_and_removed_with_their_contents() {
    let remote = temp_dir("ops");
    let sftp = client(remote.clone()).await;

    sftp.create_dir("/a").await.unwrap();
    sftp.create_dir("/a/b").await.unwrap();
    fs::write(remote.join("a/b/f.txt"), b"x").unwrap();
    sftp.rename("/a", "/c").await.unwrap();
    assert!(remote.join("c/b/f.txt").exists());

    sftp.remove("/c").await.unwrap();
    assert!(!remote.join("c").exists());
}

#[test]
fn remote_names_that_would_escape_or_break_the_local_folder_are_refused() {
    for bad in [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "C:x",
        "file.txt:stream",
        "CON",
        "nul.txt",
        "x\u{1}",
        "trailing.",
        "space ",
    ] {
        assert!(!is_safe_local_name(bad), "{bad:?} should be refused");
    }
    for good in ["report.txt", ".bashrc", "console.log", "Résumé (2).pdf"] {
        assert!(is_safe_local_name(good), "{good:?} should be allowed");
    }
}

#[test]
fn path_helpers_handle_the_root_and_trailing_slashes() {
    assert_eq!(join("/", "a"), "/a");
    assert_eq!(join("/home/u", "a"), "/home/u/a");
    assert_eq!(parent("/home/u/"), "/home");
    assert_eq!(parent("/home"), "/");
    assert_eq!(parent("/"), "/");
    assert_eq!(file_name("/var/log/syslog"), "syslog");
    assert_eq!(keep_both_name("report.txt", 2), "report (2).txt");
    assert_eq!(keep_both_name(".bashrc", 1), ".bashrc (1)");
    assert_eq!(keep_both_name("Makefile", 3), "Makefile (3)");
}
