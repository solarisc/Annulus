//! What installing and removing do, step by step, over a [`Desk`]: the part of Windows they
//! touch beyond the files (the Apps & features entry, shortcuts, the user's PATH). The real one
//! is [`WindowsDesk`]; the tests use one that only writes down what it was asked.

use std::io::Read as _;
use std::path::{Path, PathBuf};

/// The key of our entry under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall`.
#[cfg(windows)]
pub const UNINSTALL_KEY: &str = "Annulus";
/// The uninstaller's name in the install folder: a copy of the installer.
pub const UNINSTALLER: &str = "uninstall.exe";
/// The app, which the shortcuts open.
pub const APP: &str = "annulus-app.exe";
/// The shortcut's name, in the Start menu and on the desktop.
pub const SHORTCUT: &str = "Annulus.lnk";

/// What the user chose on the first page.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Separate switches on one page.
pub struct Choices {
    /// Where the programs go.
    pub dir: PathBuf,
    /// A shortcut in the Start menu.
    pub start_menu: bool,
    /// A shortcut on the desktop.
    pub desktop: bool,
    /// The folder on the user's PATH, so `annulus-agent` runs from any terminal.
    pub path: bool,
    /// Open Annulus once installed.
    pub open_after: bool,
}

impl Choices {
    /// The defaults: the user's own programs folder, a Start menu shortcut, on the PATH, and
    /// opened when done.
    #[must_use]
    pub fn new() -> Self {
        Self {
            dir: default_dir(),
            start_menu: true,
            desktop: false,
            path: true,
            open_after: true,
        }
    }
}

impl Default for Choices {
    fn default() -> Self {
        Self::new()
    }
}

/// `%LOCALAPPDATA%\Programs\Annulus`: where Windows puts programs installed for one user, so
/// no administrator is needed.
#[must_use]
pub fn default_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map_or_else(std::env::temp_dir, |dirs| {
            dirs.data_local_dir().to_path_buf()
        })
        .join("Programs")
        .join("Annulus")
}

/// One value of the Apps & features entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Number(u32),
}

/// What installing touches beyond the install folder.
pub trait Desk {
    /// Writes our Apps & features entry.
    ///
    /// # Errors
    /// Windows refused.
    fn set_entry(&mut self, values: &[(&str, Value)]) -> Result<(), String>;
    /// Removes it; gone already is fine.
    ///
    /// # Errors
    /// Windows refused.
    fn remove_entry(&mut self) -> Result<(), String>;
    /// A shortcut at `link` that opens `target`.
    ///
    /// # Errors
    /// Windows refused.
    fn shortcut(&mut self, link: &Path, target: &Path) -> Result<(), String>;
    /// The user's PATH as stored (unexpanded); empty when there is none.
    ///
    /// # Errors
    /// Windows refused.
    fn path(&mut self) -> Result<String, String>;
    /// Stores the user's PATH and tells running programs it changed.
    ///
    /// # Errors
    /// Windows refused.
    fn set_path(&mut self, path: &str) -> Result<(), String>;
    /// The Start menu's programs folder and the desktop.
    fn places(&self) -> (PathBuf, PathBuf);
}

/// The user's PATH with `dir` added at its end; `None` when it is there already (in any case,
/// with or without a trailing backslash).
#[must_use]
pub fn path_with(path: &str, dir: &str) -> Option<String> {
    if path.split(';').any(|entry| same_dir(entry, dir)) {
        return None;
    }
    let path = path.trim_end_matches(';');
    Some(if path.is_empty() {
        dir.to_owned()
    } else {
        format!("{path};{dir}")
    })
}

/// The user's PATH without `dir`; `None` when it is not there.
#[must_use]
pub fn path_without(path: &str, dir: &str) -> Option<String> {
    let kept: Vec<&str> = path
        .split(';')
        .filter(|entry| !entry.is_empty() && !same_dir(entry, dir))
        .collect();
    let had = path.split(';').filter(|entry| !entry.is_empty()).count();
    (kept.len() != had).then(|| kept.join(";"))
}

fn same_dir(a: &str, b: &str) -> bool {
    let trim = |dir: &str| dir.trim().trim_end_matches(['\\', '/']).to_lowercase();
    trim(a) == trim(b)
}

/// Puts `files` (name, size, deflated bytes) into `choices.dir`, then the uninstaller (a copy
/// of `me`), the Apps & features entry, the shortcuts and the PATH entry chosen. Each file is
/// written beside its place first and takes its name only once whole, so a program in use is
/// never left half replaced. `step` hears how far it is (0 to 1) and what it is doing.
///
/// # Errors
/// What could not be done, as the installer says it. A running Annulus is named, so the user
/// knows to close it.
pub fn install(
    choices: &Choices,
    files: &[(&str, u64, &[u8])],
    me: &Path,
    desk: &mut dyn Desk,
    step: &mut dyn FnMut(f32, &str),
) -> Result<(), String> {
    if files.is_empty() {
        return Err("This installer carries no programs: it was built without them.".into());
    }
    let dir = &choices.dir;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("Could not make {}: {error}", dir.display()))?;
    #[allow(clippy::cast_precision_loss)] // A handful of files.
    let share = |done: usize| done as f32 / (files.len() + 2) as f32;
    let mut total = 0;
    for (done, (name, size, packed)) in files.iter().enumerate() {
        step(share(done), &format!("Copying {name}"));
        let target = dir.join(name);
        let part = dir.join(format!("{name}.part"));
        let mut bytes = Vec::with_capacity(usize::try_from(*size).unwrap_or(0));
        flate2::read::DeflateDecoder::new(*packed)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("The installer's copy of {name} is damaged: {error}"))?;
        if bytes.len() as u64 != *size {
            return Err(format!("The installer's copy of {name} is damaged."));
        }
        std::fs::write(&part, &bytes)
            .map_err(|error| format!("Could not write {}: {error}", part.display()))?;
        if let Err(error) = std::fs::rename(&part, &target) {
            let _ = std::fs::remove_file(&part);
            return Err(in_use(name, &error));
        }
        total += size;
    }
    step(share(files.len()), "Adding Annulus to Apps & features");
    let uninstaller = dir.join(UNINSTALLER);
    if me != uninstaller {
        std::fs::copy(me, &uninstaller).map_err(|error| in_use(UNINSTALLER, &error))?;
    }
    let shown = |path: &Path| path.display().to_string();
    let kilobytes = u32::try_from(total / 1024).unwrap_or(u32::MAX);
    desk.set_entry(&[
        ("DisplayName", Value::Text("Annulus".into())),
        (
            "DisplayVersion",
            Value::Text(env!("CARGO_PKG_VERSION").into()),
        ),
        ("Publisher", Value::Text("Annulus".into())),
        (
            "DisplayIcon",
            Value::Text(format!("{},0", shown(&dir.join(APP)))),
        ),
        ("InstallLocation", Value::Text(shown(dir))),
        (
            "UninstallString",
            Value::Text(format!("\"{}\" --uninstall", shown(&uninstaller))),
        ),
        ("EstimatedSize", Value::Number(kilobytes)),
        ("NoModify", Value::Number(1)),
        ("NoRepair", Value::Number(1)),
    ])?;
    step(share(files.len() + 1), "Adding shortcuts");
    let (menu, desktop) = desk.places();
    let app = dir.join(APP);
    if choices.start_menu {
        desk.shortcut(&menu.join(SHORTCUT), &app)?;
    }
    if choices.desktop {
        desk.shortcut(&desktop.join(SHORTCUT), &app)?;
    }
    if choices.path
        && let Some(path) = path_with(&desk.path()?, &shown(dir))
    {
        desk.set_path(&path)?;
    }
    step(1.0, "Done");
    Ok(())
}

/// Takes Annulus out of `dir`: its programs and licenses, the shortcuts, the PATH entry and
/// the Apps & features entry. The vault and settings stay (they live elsewhere), and so do
/// files of the user's own in `dir`. The uninstaller cannot delete itself while it runs:
/// [`schedule_self_removal`] does, a moment after it ends.
///
/// # Errors
/// A program is in use (Annulus is open), or Windows refused.
pub fn uninstall(dir: &Path, names: &[&str], desk: &mut dyn Desk) -> Result<(), String> {
    for name in names {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(in_use(name, &error)),
        }
    }
    let (menu, desktop) = desk.places();
    for link in [menu.join(SHORTCUT), desktop.join(SHORTCUT)] {
        let _ = std::fs::remove_file(link);
    }
    if let Some(path) = path_without(&desk.path()?, &dir.display().to_string()) {
        desk.set_path(&path)?;
    }
    desk.remove_entry()
}

/// What failing to replace or remove `name` means to the user.
fn in_use(name: &str, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        let what = if name == APP { "Annulus" } else { name };
        format!("{what} is in use. Close Annulus (and any annulus-agent running), then try again.")
    } else {
        format!("Could not replace {name}: {error}")
    }
}

/// After this process ends, deletes the uninstaller `me` and its folder `dir` if nothing else
/// is left in it (a command shell waits a moment, then does it).
pub fn schedule_self_removal(me: &Path, dir: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW`: no console flashes up.
        const NO_WINDOW: u32 = 0x0800_0000;
        let script = format!(
            "/C ping -n 3 127.0.0.1 >NUL & del /F /Q \"{}\" & rmdir \"{}\"",
            me.display(),
            dir.display()
        );
        let _ = std::process::Command::new("cmd.exe")
            .raw_arg(script)
            .creation_flags(NO_WINDOW)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = (me, dir);
    }
}

/// The real desk: this Windows account's registry, Start menu and desktop.
pub struct WindowsDesk;

#[cfg(windows)]
impl Desk for WindowsDesk {
    fn set_entry(&mut self, values: &[(&str, Value)]) -> Result<(), String> {
        use annulus_platform::install::{RegistryValue, set_uninstall_entry};
        let values: Vec<(&str, RegistryValue)> = values
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    Value::Text(text) => RegistryValue::Text(text.clone()),
                    Value::Number(number) => RegistryValue::Number(*number),
                };
                (*name, value)
            })
            .collect();
        set_uninstall_entry(UNINSTALL_KEY, &values).map_err(|error| error.to_string())
    }

    fn remove_entry(&mut self) -> Result<(), String> {
        annulus_platform::install::remove_uninstall_entry(UNINSTALL_KEY)
            .map_err(|error| error.to_string())
    }

    fn shortcut(&mut self, link: &Path, target: &Path) -> Result<(), String> {
        if let Some(folder) = link.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        annulus_platform::install::create_shortcut(link, target, "Terminals and SSH")
            .map_err(|error| format!("Could not make {}: {error}", link.display()))
    }

    fn path(&mut self) -> Result<String, String> {
        annulus_platform::install::user_path().map_err(|error| error.to_string())
    }

    fn set_path(&mut self, path: &str) -> Result<(), String> {
        annulus_platform::install::set_user_path(path).map_err(|error| error.to_string())
    }

    fn places(&self) -> (PathBuf, PathBuf) {
        let menu = directories::BaseDirs::new()
            .map(|dirs| {
                dirs.config_dir()
                    .join(r"Microsoft\Windows\Start Menu\Programs")
            })
            .unwrap_or_default();
        let desktop = directories::UserDirs::new()
            .and_then(|dirs| dirs.desktop_dir().map(Path::to_path_buf))
            .unwrap_or_default();
        (menu, desktop)
    }
}

#[cfg(not(windows))]
impl Desk for WindowsDesk {
    fn set_entry(&mut self, _: &[(&str, Value)]) -> Result<(), String> {
        Err(NOT_HERE.into())
    }

    fn remove_entry(&mut self) -> Result<(), String> {
        Err(NOT_HERE.into())
    }

    fn shortcut(&mut self, _: &Path, _: &Path) -> Result<(), String> {
        Err(NOT_HERE.into())
    }

    fn path(&mut self) -> Result<String, String> {
        Err(NOT_HERE.into())
    }

    fn set_path(&mut self, _: &str) -> Result<(), String> {
        Err(NOT_HERE.into())
    }

    fn places(&self) -> (PathBuf, PathBuf) {
        (PathBuf::new(), PathBuf::new())
    }
}

/// What the installer says on a system it is not for.
pub const NOT_HERE: &str =
    "This installer is for Windows. On this system, unpack the Annulus zip and run annulus-app.";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use std::io::Write as _;

    use super::*;

    /// A desk that only writes down what it was asked, so the tests never touch the real
    /// registry, Start menu or PATH.
    #[derive(Default)]
    struct Notes {
        entry: Option<Vec<(String, Value)>>,
        links: Vec<(PathBuf, PathBuf)>,
        path: String,
        root: PathBuf,
    }

    impl Desk for Notes {
        fn set_entry(&mut self, values: &[(&str, Value)]) -> Result<(), String> {
            self.entry = Some(
                values
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), value.clone()))
                    .collect(),
            );
            Ok(())
        }

        fn remove_entry(&mut self) -> Result<(), String> {
            self.entry = None;
            Ok(())
        }

        fn shortcut(&mut self, link: &Path, target: &Path) -> Result<(), String> {
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::fs::write(link, target.display().to_string()).unwrap();
            self.links.push((link.to_path_buf(), target.to_path_buf()));
            Ok(())
        }

        fn path(&mut self) -> Result<String, String> {
            Ok(self.path.clone())
        }

        fn set_path(&mut self, path: &str) -> Result<(), String> {
            path.clone_into(&mut self.path);
            Ok(())
        }

        fn places(&self) -> (PathBuf, PathBuf) {
            (self.root.join("menu"), self.root.join("desktop"))
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("annulus-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn packed(bytes: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn the_path_gains_the_folder_once_and_loses_only_it() {
        let dir = r"C:\Users\ana\AppData\Local\Programs\Annulus";
        assert_eq!(path_with("", dir).as_deref(), Some(dir));
        assert_eq!(path_with(r"C:\bin;", dir), Some(format!(r"C:\bin;{dir}")));
        assert_eq!(
            path_with(&format!(r"C:\bin;{}\", dir.to_lowercase()), dir),
            None,
            "there already, in another case and with a trailing backslash"
        );
        assert_eq!(
            path_without(&format!(r"C:\bin;{dir};%USERPROFILE%\tools"), dir).as_deref(),
            Some(r"C:\bin;%USERPROFILE%\tools"),
            "the others stay as they were, unexpanded"
        );
        assert_eq!(path_without(r"C:\bin", dir), None);
    }

    #[test]
    fn installing_puts_the_programs_shortcuts_path_and_entry_in_place_and_removing_takes_them() {
        let root = scratch("round");
        let me = root.join("annulus-setup.exe");
        std::fs::write(&me, b"setup").unwrap();
        let app = packed(b"the app");
        let agent = packed(b"the agent");
        let files: [(&str, u64, &[u8]); 2] = [(APP, 7, &app), ("annulus-agent.exe", 9, &agent)];
        let choices = Choices {
            dir: root.join("Programs").join("Annulus"),
            desktop: true,
            ..Choices::new()
        };
        let mut desk = Notes {
            path: r"C:\bin".into(),
            root: root.clone(),
            ..Notes::default()
        };
        let mut seen = Vec::new();
        install(&choices, &files, &me, &mut desk, &mut |share, what| {
            seen.push((share, what.to_owned()));
        })
        .unwrap();
        let dir = &choices.dir;
        assert_eq!(std::fs::read(dir.join(APP)).unwrap(), b"the app");
        assert_eq!(
            std::fs::read(dir.join("annulus-agent.exe")).unwrap(),
            b"the agent"
        );
        assert_eq!(std::fs::read(dir.join(UNINSTALLER)).unwrap(), b"setup");
        assert!(!dir.join(format!("{APP}.part")).exists());
        let entry = desk.entry.clone().unwrap();
        let get = |name: &str| {
            entry
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            get("UninstallString"),
            Some(Value::Text(format!(
                "\"{}\" --uninstall",
                dir.join(UNINSTALLER).display()
            )))
        );
        assert_eq!(desk.links.len(), 2, "Start menu and desktop");
        assert!(
            desk.links
                .iter()
                .all(|(_, target)| target == &dir.join(APP))
        );
        assert_eq!(desk.path, format!(r"C:\bin;{}", dir.display()));
        assert!(
            seen.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "{seen:?}"
        );
        assert_eq!(seen.last().map(|(share, _)| *share), Some(1.0));

        // Installing again over it (an update) changes nothing twice.
        install(&choices, &files, &me, &mut desk, &mut |_, _| {}).unwrap();
        assert_eq!(desk.path.matches("Annulus").count(), 1);

        // A file of the user's own in the folder stays when Annulus goes.
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        uninstall(dir, &[APP, "annulus-agent.exe"], &mut desk).unwrap();
        assert!(!dir.join(APP).exists() && !dir.join("annulus-agent.exe").exists());
        assert!(dir.join("notes.txt").exists());
        assert!(!root.join("menu").join(SHORTCUT).exists());
        assert!(!root.join("desktop").join(SHORTCUT).exists());
        assert_eq!(desk.path, r"C:\bin");
        assert_eq!(desk.entry, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_damaged_or_empty_installer_refuses_before_writing_anything() {
        let root = scratch("damaged");
        let me = root.join("annulus-setup.exe");
        std::fs::write(&me, b"setup").unwrap();
        let choices = Choices {
            dir: root.join("Annulus"),
            ..Choices::new()
        };
        let mut desk = Notes::default();
        let empty = install(&choices, &[], &me, &mut desk, &mut |_, _| {});
        assert!(empty.unwrap_err().contains("carries no programs"));
        let wrong = packed(b"short");
        let files: [(&str, u64, &[u8]); 1] = [(APP, 99, &wrong)];
        let damaged = install(&choices, &files, &me, &mut desk, &mut |_, _| {});
        assert!(damaged.unwrap_err().contains("damaged"));
        assert!(!choices.dir.join(APP).exists());
        assert_eq!(desk.entry, None, "no entry for an install that failed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn programs_go_where_windows_puts_one_users_programs() {
        let dir = default_dir();
        assert!(
            dir.ends_with(Path::new("Programs").join("Annulus")),
            "{}",
            dir.display()
        );
    }
}
