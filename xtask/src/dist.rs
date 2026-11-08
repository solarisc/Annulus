//! `cargo xtask dist`: the downloads of a release build (ADR-0027). The client's go in
//! `target/dist/client`, the server's in `target/dist/server`, so they are uploaded apart:
//!
//! - `annulus-<version>-<os>-<arch>.zip`: the client, portable: a folder `Annulus` with the app
//!   (on macOS `Annulus.app`), `annulus-agent` and the licenses. Unpack it anywhere and run.
//! - `annulus-setup-<version>-windows-<arch>.exe` (Windows): the installer, with the app and
//!   `annulus-agent` packed inside (`crates/annulus-setup`).
//! - `annulus-server-<version>-<os>-<arch>.zip`: the sync server alone, in a folder
//!   `annulus-server` with its license. Never in the client's downloads, and with no installer:
//!   it goes on servers.
//!
//! The zips are written here, by hand, with deflate; each entry keeps its Unix mode, so the
//! programs stay executable once unpacked on Linux or macOS.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

use crate::repo;

/// This build's version, the workspace's.
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run() -> Result<bool> {
    let root = repo::root()?;
    let release = repo::target_dir(&root)?.join("release");
    let dist = repo::target_dir(&root)?.join("dist");
    let (clients, servers) = (dist.join("client"), dist.join("server"));
    // Only this build's downloads: an older version's would be uploaded beside them.
    let _ = std::fs::remove_dir_all(&dist);
    std::fs::create_dir_all(&clients)?;
    std::fs::create_dir_all(&servers)?;
    let exe = std::env::consts::EXE_SUFFIX;
    let platform = platform();
    if !cargo(
        &root,
        &[
            "-p",
            "annulus-app",
            "-p",
            "annulus-cli",
            "-p",
            "annulus-server",
        ],
        None,
    )? {
        return Ok(false);
    }

    // The client: the app and the agent in one folder.
    let mut client = vec![
        (
            format!("Annulus/annulus-agent{exe}"),
            Source::File(release.join(format!("annulus-agent{exe}"))),
            true,
        ),
        (
            "Annulus/LICENSE-MIT".to_owned(),
            Source::File(root.join("LICENSE-MIT")),
            false,
        ),
        (
            "Annulus/LICENSE-APACHE".to_owned(),
            Source::File(root.join("LICENSE-APACHE")),
            false,
        ),
        (
            "Annulus/LICENSE-THEMES".to_owned(),
            Source::File(root.join("LICENSE-THEMES")),
            false,
        ),
    ];
    if cfg!(target_os = "macos") {
        if !crate::mac_app::run()? {
            return Ok(false);
        }
        client.push((
            "Annulus/Annulus.app".to_owned(),
            Source::Folder(release.join("Annulus.app")),
            false,
        ));
    } else {
        client.push((
            format!("Annulus/annulus-app{exe}"),
            Source::File(release.join(format!("annulus-app{exe}"))),
            true,
        ));
    }
    let client_zip = clients.join(format!("annulus-{VERSION}-{platform}.zip"));
    write_zip(&client_zip, &client)?;
    println!("wrote {}", client_zip.display());

    // The server, apart.
    let server = [
        (
            format!("annulus-server/annulus-server{exe}"),
            Source::File(release.join(format!("annulus-server{exe}"))),
            true,
        ),
        (
            "annulus-server/LICENSE".to_owned(),
            Source::File(root.join("crates/annulus-server/LICENSE")),
            false,
        ),
    ];
    let server_zip = servers.join(format!("annulus-server-{VERSION}-{platform}.zip"));
    write_zip(&server_zip, &server)?;
    println!("wrote {}", server_zip.display());

    // The installer, with the programs just built inside.
    if cfg!(windows) {
        if !cargo(&root, &["-p", "annulus-setup"], Some(&release))? {
            return Ok(false);
        }
        let setup = clients.join(format!("annulus-setup-{VERSION}-{platform}.exe"));
        std::fs::copy(release.join("annulus-setup.exe"), &setup)
            .with_context(|| format!("copying the installer to {}", setup.display()))?;
        println!("wrote {}", setup.display());
    }
    Ok(true)
}

/// `windows-x64`, `linux-x64`, `macos-arm64` and so on.
fn platform() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{}-{arch}", std::env::consts::OS)
}

/// `cargo build --release --locked` of `packages`, with the installer's payload folder when
/// given; whether it built.
fn cargo(root: &Path, packages: &[&str], payload: Option<&Path>) -> Result<bool> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = Command::new(cargo);
    command
        .args(["build", "--release", "--locked"])
        .args(packages)
        .current_dir(root);
    if let Some(folder) = payload {
        command.env("ANNULUS_SETUP_PAYLOAD", folder);
    }
    let status = command.status().context("could not run `cargo build`")?;
    Ok(status.success())
}

/// What goes into a zip at a name.
enum Source {
    File(PathBuf),
    /// A folder with everything in it, under the name.
    Folder(PathBuf),
}

/// Writes the zip `to` with `entries`: their names in it, where they come from, and whether a
/// file is a program (mode 755 where the file system does not say).
fn write_zip(to: &Path, entries: &[(String, Source, bool)]) -> Result<()> {
    let mut files = Vec::new();
    let mut folders = std::collections::BTreeSet::new();
    for (name, source, program) in entries {
        match source {
            Source::File(path) => files.push((name.clone(), path.clone(), *program)),
            Source::Folder(path) => {
                folders.insert(name.clone());
                walk(path, name, &mut files, &mut folders)?;
            }
        }
        // Every folder above it too, so the zip opens the same everywhere.
        let mut above = Path::new(name).parent();
        while let Some(folder) = above.filter(|folder| !folder.as_os_str().is_empty()) {
            folders.insert(folder.to_string_lossy().replace('\\', "/"));
            above = folder.parent();
        }
    }
    let mut zip = Zip::new();
    for folder in &folders {
        zip.folder(folder);
    }
    for (name, path, program) in files {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        zip.file(&name, &bytes, mode_of(program), modified(&path));
    }
    let bytes = zip.finish();
    std::fs::write(to, bytes).with_context(|| format!("writing {}", to.display()))
}

/// The files and folders in `dir`, named under `name`.
fn walk(
    dir: &Path,
    name: &str,
    files: &mut Vec<(String, PathBuf, bool)>,
    folders: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let inner = format!("{name}/{}", entry.file_name().to_string_lossy());
        if entry.file_type()?.is_dir() {
            folders.insert(inner.clone());
            walk(&entry.path(), &inner, files, folders)?;
        } else {
            let program = executable(&entry.path());
            files.push((inner, entry.path(), program));
        }
    }
    Ok(())
}

/// 755 for a program and 644 for the rest, whatever the file system says: a checkout on a
/// mounted Windows folder (CI's Linux container) reports 777 for every file.
fn mode_of(program: bool) -> u32 {
    if program { 0o755 } else { 0o644 }
}

/// Whether `path`, inside a folder taken whole (a macOS bundle), is a program: executable
/// for its owner where the file system keeps modes, never on Windows.
fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o100 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// Seconds since the Unix epoch `path` was last changed.
fn modified(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs())
}

/// A zip being written: deflated files and folders, names in UTF-8, no ZIP64 (each file and
/// the whole stay under 4 GiB).
pub struct Zip {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl Zip {
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            central: Vec::new(),
            count: 0,
        }
    }

    /// A folder (`name` without a trailing slash).
    pub fn folder(&mut self, name: &str) {
        self.entry(&format!("{name}/"), &[], 0o040_755, 0);
    }

    /// A file with `bytes`, Unix `mode` (`0o755`, `0o644`) and changed at `modified` (Unix
    /// seconds).
    pub fn file(&mut self, name: &str, bytes: &[u8], mode: u32, modified: u64) {
        self.entry(name, bytes, mode | 0o100_000, modified);
    }

    fn entry(&mut self, name: &str, bytes: &[u8], mode: u32, modified: u64) {
        let folder = name.ends_with('/');
        let crc = crc32fast::hash(bytes);
        let (method, data) = if folder {
            (0_u16, Vec::new())
        } else {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
            // Writing to a Vec cannot fail.
            let _ = encoder.write_all(bytes);
            (8, encoder.finish().unwrap_or_default())
        };
        let (clock, day) = dos_time(modified);
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let packed = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let name_len = u16::try_from(name.len()).unwrap_or(u16::MAX);
        let offset = u32::try_from(self.out.len()).unwrap_or(u32::MAX);
        // Bit 11: the name is UTF-8.
        let flags: u16 = 0x0800;
        let local = &mut self.out;
        local.extend(0x0403_4b50_u32.to_le_bytes());
        local.extend(20_u16.to_le_bytes());
        local.extend(flags.to_le_bytes());
        local.extend(method.to_le_bytes());
        local.extend(clock.to_le_bytes());
        local.extend(day.to_le_bytes());
        local.extend(crc.to_le_bytes());
        local.extend(packed.to_le_bytes());
        local.extend(size.to_le_bytes());
        local.extend(name_len.to_le_bytes());
        local.extend(0_u16.to_le_bytes());
        local.extend(name.as_bytes());
        local.extend(&data);
        let central = &mut self.central;
        central.extend(0x0201_4b50_u32.to_le_bytes());
        // Made on Unix (3), so unzip keeps the mode; ZIP 2.0.
        central.extend(0x0314_u16.to_le_bytes());
        central.extend(20_u16.to_le_bytes());
        central.extend(flags.to_le_bytes());
        central.extend(method.to_le_bytes());
        central.extend(clock.to_le_bytes());
        central.extend(day.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(packed.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(name_len.to_le_bytes());
        central.extend([0_u8; 8]);
        // The Unix mode above, and the DOS folder bit for Windows' unzip.
        let attributes = (mode << 16) | (u32::from(folder) * 0x10);
        central.extend(attributes.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
        self.count += 1;
    }

    /// The whole zip.
    pub fn finish(mut self) -> Vec<u8> {
        let start = u32::try_from(self.out.len()).unwrap_or(u32::MAX);
        let size = u32::try_from(self.central.len()).unwrap_or(u32::MAX);
        self.out.extend(&self.central);
        self.out.extend(0x0605_4b50_u32.to_le_bytes());
        self.out.extend([0_u8; 4]);
        self.out.extend(self.count.to_le_bytes());
        self.out.extend(self.count.to_le_bytes());
        self.out.extend(size.to_le_bytes());
        self.out.extend(start.to_le_bytes());
        self.out.extend(0_u16.to_le_bytes());
        self.out
    }
}

/// Unix seconds as a zip's DOS time and date (UTC; 1980 at the earliest).
fn dos_time(seconds: u64) -> (u16, u16) {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let of_day = seconds % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    if year < 1980 {
        return (0, (1 << 5) | 1);
    }
    let field = |value: i64| u16::try_from(value).unwrap_or(0);
    let time = field(i64::try_from(of_day / 3600).unwrap_or(0)) << 11
        | field(i64::try_from(of_day / 60 % 60).unwrap_or(0)) << 5
        | field(i64::try_from(of_day % 60 / 2).unwrap_or(0));
    let date = field((year - 1980).min(127)) << 9 | field(month) << 5 | field(day);
    (time, date)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Read as _;

    use super::*;

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    /// The entries of `zip` as an unzip finds them: through the central directory to each
    /// one's local header, its bytes inflated and checked against its CRC.
    fn read(zip: &[u8]) -> Vec<(String, Vec<u8>, u32)> {
        let end = zip.len() - 22;
        assert_eq!(u32_at(zip, end), 0x0605_4b50, "the end record is last");
        let count = usize::from(u16_at(zip, end + 10));
        let mut at = u32_at(zip, end + 16) as usize;
        let mut entries = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(zip, at), 0x0201_4b50);
            let method = u16_at(zip, at + 10);
            let crc = u32_at(zip, at + 16);
            let packed = u32_at(zip, at + 20) as usize;
            let name_len = usize::from(u16_at(zip, at + 28));
            let mode = u32_at(zip, at + 38) >> 16;
            let local = u32_at(zip, at + 42) as usize;
            let name = String::from_utf8(zip[at + 46..at + 46 + name_len].to_vec()).unwrap();
            assert_eq!(u32_at(zip, local), 0x0403_4b50);
            let data_at = local + 30 + usize::from(u16_at(zip, local + 26));
            let data = &zip[data_at..data_at + packed];
            let bytes = if method == 8 {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(data)
                    .read_to_end(&mut out)
                    .unwrap();
                out
            } else {
                data.to_vec()
            };
            assert_eq!(crc32fast::hash(&bytes), crc, "{name}");
            entries.push((name, bytes, mode));
            at += 46 + name_len;
        }
        entries
    }

    #[test]
    fn a_zip_holds_each_file_whole_in_its_folder_with_its_mode() {
        let mut zip = Zip::new();
        zip.folder("Annulus");
        let program = vec![7_u8; 100_000];
        zip.file("Annulus/annulus-agent", &program, 0o755, 1_800_000_000);
        zip.file("Annulus/LICENSE-MIT", b"MIT", 0o644, 0);
        let bytes = zip.finish();
        assert!(bytes.len() < 2_000, "deflated: {}", bytes.len());
        let entries = read(&bytes);
        let names: Vec<&str> = entries.iter().map(|(name, ..)| name.as_str()).collect();
        assert_eq!(
            names,
            ["Annulus/", "Annulus/annulus-agent", "Annulus/LICENSE-MIT"]
        );
        assert_eq!(entries[1].1, program);
        assert_eq!(entries[1].2, 0o100_755, "a program stays executable");
        assert_eq!(entries[2].2, 0o100_644);
        assert_eq!(entries[0].2 & 0o040_000, 0o040_000, "a folder");
    }

    #[test]
    fn the_client_and_server_are_written_apart_with_every_folder_named() {
        let dir = std::env::temp_dir().join(format!("annulus-dist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Annulus.app/Contents/MacOS")).unwrap();
        std::fs::write(dir.join("Annulus.app/Contents/MacOS/Annulus"), "app").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let binary = dir.join("Annulus.app/Contents/MacOS/Annulus");
            std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(dir.join("agent"), "agent").unwrap();
        let zip = dir.join("client.zip");
        write_zip(
            &zip,
            &[
                (
                    "Annulus/annulus-agent".into(),
                    Source::File(dir.join("agent")),
                    true,
                ),
                (
                    "Annulus/Annulus.app".into(),
                    Source::Folder(dir.join("Annulus.app")),
                    false,
                ),
            ],
        )
        .unwrap();
        let entries = read(&std::fs::read(&zip).unwrap());
        let names: Vec<&str> = entries.iter().map(|(name, ..)| name.as_str()).collect();
        for folder in [
            "Annulus/",
            "Annulus/Annulus.app/",
            "Annulus/Annulus.app/Contents/",
            "Annulus/Annulus.app/Contents/MacOS/",
        ] {
            assert!(names.contains(&folder), "{folder} in {names:?}");
        }
        assert!(names.contains(&"Annulus/Annulus.app/Contents/MacOS/Annulus"));
        let mode = |name: &str| entries.iter().find(|(n, ..)| n == name).unwrap().2;
        assert_eq!(mode("Annulus/annulus-agent"), 0o100_755, "a program");
        assert_eq!(
            mode("Annulus/Annulus.app/Contents/MacOS/Annulus") & 0o111,
            if cfg!(unix) { 0o111 } else { 0 },
            "a bundle's program as the file system says, here {}",
            std::env::consts::OS
        );
        assert!(!names.iter().any(|name| name.contains("server")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn times_become_dos_dates() {
        // 2026-11-07 21:30:10 UTC.
        let (time, date) = dos_time(1_794_087_010);
        assert_eq!((date >> 9) + 1980, 2026);
        assert_eq!((date >> 5) & 0xf, 11);
        assert_eq!(date & 0x1f, 7);
        assert_eq!(
            (time >> 11, (time >> 5) & 0x3f, (time & 0x1f) * 2),
            (21, 30, 10)
        );
        assert_eq!(dos_time(0), (0, (1 << 5) | 1), "before 1980: its first day");
    }
}
