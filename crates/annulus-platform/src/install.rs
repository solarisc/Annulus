//! What the Windows installer (`annulus-setup`) asks of Windows for the current user: its entry
//! in Apps & features, shortcuts, the user's PATH, and whether apps are in light mode. Nothing
//! here needs an administrator: everything is under `HKEY_CURRENT_USER` or the user's folders.

#![allow(unsafe_code)]

use std::path::Path;

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, LPARAM, WIN32_ERROR, WPARAM,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE,
    REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
use windows::core::{HSTRING, Interface as _, PCWSTR, w};

/// Where Apps & features finds what was installed for this user.
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// One value of a registry key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryValue {
    /// A string.
    Text(String),
    /// A number (`REG_DWORD`).
    Number(u32),
}

fn failed(code: WIN32_ERROR) -> std::io::Error {
    #[allow(clippy::cast_possible_wrap)] // Windows error codes are small.
    std::io::Error::from_raw_os_error(code.0 as i32)
}

fn check(code: WIN32_ERROR) -> std::io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(failed(code))
    }
}

/// `text` as the UTF-16 bytes, ending in a nul, that a registry string holds.
fn wide_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// An open key under `HKEY_CURRENT_USER`, closed when dropped.
struct Key(HKEY);

impl Key {
    /// Opens `path` to write, making it when missing.
    fn create(path: &str) -> std::io::Result<Self> {
        let mut key = HKEY::default();
        // SAFETY: the subkey is a nul-terminated string alive for the call, and `key` is a
        // local the call writes the handle into.
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(path),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &raw mut key,
                None,
            )
        })?;
        Ok(Key(key))
    }

    fn set(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> std::io::Result<()> {
        // SAFETY: an open key; the name and data live for the call, and the data's length is
        // the slice's.
        check(unsafe { RegSetValueExW(self.0, &HSTRING::from(name), None, kind, Some(data)) })
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegCreateKeyExW and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// Writes `values` into this user's Apps & features entry `key`, making it when missing.
///
/// # Errors
/// Windows refused.
pub fn set_uninstall_entry(key: &str, values: &[(&str, RegistryValue)]) -> std::io::Result<()> {
    let entry = Key::create(&format!(r"{UNINSTALL}\{key}"))?;
    for (name, value) in values {
        match value {
            RegistryValue::Text(text) => entry.set(name, REG_SZ, &wide_bytes(text))?,
            RegistryValue::Number(number) => entry.set(name, REG_DWORD, &number.to_le_bytes())?,
        }
    }
    Ok(())
}

/// Removes this user's Apps & features entry `key`; gone already is fine.
///
/// # Errors
/// Windows refused.
pub fn remove_uninstall_entry(key: &str) -> std::io::Result<()> {
    let path = HSTRING::from(format!(r"{UNINSTALL}\{key}"));
    // SAFETY: a nul-terminated subkey of HKEY_CURRENT_USER, alive for the call.
    let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &path) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(code)
}

/// A string value of `path` under `HKEY_CURRENT_USER`, unexpanded; `None` when missing.
fn read_text(path: &str, name: &str) -> std::io::Result<Option<String>> {
    let (path, name) = (HSTRING::from(path), HSTRING::from(name));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let mut size = 0_u32;
    // SAFETY: a size query: no buffer, the size written into a local.
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &path,
            &name,
            flags,
            None,
            None,
            Some(&raw mut size),
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(code)?;
    let mut buffer = vec![0_u16; (size as usize).div_ceil(2) + 1];
    let mut bytes = u32::try_from(buffer.len() * 2).unwrap_or(u32::MAX);
    // SAFETY: the buffer holds `bytes` bytes, which the call is told and never exceeds.
    check(unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &path,
            &name,
            flags,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    })?;
    let units = (bytes as usize / 2).min(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..units]);
    Ok(Some(text.trim_end_matches('\0').to_owned()))
}

/// This user's PATH as stored, its `%VARIABLES%` unexpanded; empty when there is none.
///
/// # Errors
/// Windows refused.
pub fn user_path() -> std::io::Result<String> {
    Ok(read_text("Environment", "Path")?.unwrap_or_default())
}

/// Stores this user's PATH (as one that may hold `%VARIABLES%`) and tells running programs,
/// Explorer first, that the environment changed, so a terminal opened next has it.
///
/// # Errors
/// Windows refused.
pub fn set_user_path(path: &str) -> std::io::Result<()> {
    Key::create("Environment")?.set("Path", REG_EXPAND_SZ, &wide_bytes(path))?;
    // SAFETY: a broadcast with a static nul-terminated string; programs that hang are skipped
    // after five seconds.
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            None,
        );
    }
    Ok(())
}

/// Makes a shortcut at `link` (a `.lnk` file) that opens `target` in its own folder, with its
/// icon and `description` as its tooltip.
///
/// # Errors
/// Windows refused.
pub fn create_shortcut(link: &Path, target: &Path, description: &str) -> std::io::Result<()> {
    let target_text = HSTRING::from(target.as_os_str());
    let folder = HSTRING::from(target.parent().unwrap_or(target).as_os_str());
    let make = || -> windows::core::Result<()> {
        // SAFETY: COM on this thread first (a second call only reports it is on), then the
        // shell's own link object, set and saved through its interfaces; every string lives
        // across the call it is given to.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let shortcut: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            shortcut.SetPath(&target_text)?;
            shortcut.SetWorkingDirectory(&folder)?;
            shortcut.SetIconLocation(&target_text, 0)?;
            shortcut.SetDescription(&HSTRING::from(description))?;
            shortcut
                .cast::<IPersistFile>()?
                .Save(&HSTRING::from(link.as_os_str()), true)
        }
    };
    make().map_err(|error| std::io::Error::other(error.message()))
}

/// Whether Windows shows apps in light mode (Settings, Personalization, Colors).
#[must_use]
pub fn apps_use_light_theme() -> bool {
    let path = HSTRING::from(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = HSTRING::from("AppsUseLightTheme");
    let mut value = 1_u32;
    let mut size = 4_u32;
    // SAFETY: a DWORD read into a local four bytes long, as the call is told.
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &path,
            &name,
            RRF_RT_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    code != ERROR_SUCCESS || value != 0
}

#[cfg(test)]
mod tests {
    #[test]
    fn registry_strings_are_utf16_with_their_nul() {
        assert_eq!(super::wide_bytes("A\u{e9}"), [0x41, 0, 0xe9, 0, 0, 0]);
    }
}
