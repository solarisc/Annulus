//! `cargo xtask standalone <program.exe>...`: fails if a Windows program needs Microsoft's C
//! runtime installed. A fresh Windows may not have it, and the program then doesn't start at all
//! (ADR-0024). The release job runs this over the programs it keeps.

use anyhow::{Context, Result, bail};

/// How the names of Microsoft's C runtime DLLs start, in lowercase.
const RUNTIME: [&str; 5] = ["vcruntime", "msvcp", "msvcr", "ucrtbase", "api-ms-win-crt-"];

pub fn run(programs: &[String]) -> Result<bool> {
    if programs.is_empty() {
        bail!("name the programs to check: cargo xtask standalone <program.exe>...");
    }
    let mut all_stand_alone = true;
    for program in programs {
        let file = std::fs::read(program).with_context(|| format!("could not read {program}"))?;
        let dlls = imports(&file).with_context(|| format!("could not read {program}'s imports"))?;
        // Every real program imports something, so an empty list means this reader is wrong.
        if dlls.is_empty() {
            bail!("{program} imports nothing, which no real program does");
        }
        let runtime = runtime_dlls(&dlls);
        if runtime.is_empty() {
            println!("{program} stands alone: it imports {}", dlls.join(", "));
        } else {
            eprintln!(
                "{program} needs Microsoft's C runtime installed: {}",
                runtime.join(", ")
            );
            all_stand_alone = false;
        }
    }
    Ok(all_stand_alone)
}

fn runtime_dlls(dlls: &[String]) -> Vec<&str> {
    dlls.iter()
        .map(String::as_str)
        .filter(|dll| {
            let dll = dll.to_ascii_lowercase();
            RUNTIME.iter().any(|start| dll.starts_with(start))
        })
        .collect()
}

/// The DLLs a 64-bit Windows program imports, read from its PE headers.
fn imports(file: &[u8]) -> Result<Vec<String>> {
    let pe = number(file, 0x3C, 4)?;
    if file.get(pe..pe + 4) != Some(&b"PE\0\0"[..]) {
        bail!("it has no PE header");
    }
    let section_count = number(file, pe + 6, 2)?;
    let optional_size = number(file, pe + 20, 2)?;
    let optional = pe + 24;
    if number(file, optional, 2)? != 0x20B {
        bail!("it isn't a 64-bit program");
    }
    // The second data directory; they start 112 bytes into a 64-bit optional header.
    let table = number(file, optional + 112 + 8, 4)?;
    let sections = optional + optional_size;
    // An address in the running program, as a place in the file.
    let place = |address: usize| -> Result<usize> {
        for section in (0..section_count).map(|index| sections + index * 40) {
            let start = number(file, section + 12, 4)?;
            if (start..start + number(file, section + 16, 4)?).contains(&address) {
                return Ok(address - start + number(file, section + 20, 4)?);
            }
        }
        bail!("address {address:#x} is in no section")
    };
    let mut dlls = Vec::new();
    // Each entry is 20 bytes, with the address of its DLL's name at 12. No name ends the table.
    let mut entry = place(table)?;
    loop {
        let name = number(file, entry + 12, 4)?;
        if name == 0 {
            return Ok(dlls);
        }
        let text = file.get(place(name)?..).unwrap_or_default();
        let text = text.split(|&byte| byte == 0).next().unwrap_or_default();
        dlls.push(String::from_utf8_lossy(text).into_owned());
        entry += 20;
    }
}

/// The little-endian number of `width` bytes at `at`.
fn number(file: &[u8], at: usize, width: usize) -> Result<usize> {
    let bytes = file
        .get(at..at + width)
        .with_context(|| format!("the file ends before byte {at:#x}"))?;
    Ok(bytes
        .iter()
        .rev()
        .fold(0, |value, &byte| (value << 8) | usize::from(byte)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(file: &mut [u8], at: usize, width: usize, value: usize) {
        file[at..at + width].copy_from_slice(&value.to_le_bytes()[..width]);
    }

    /// The least a 64-bit program can hold and still have imports: its headers, then one section
    /// with the import table and the names. The section's address differs from its place in the
    /// file, as in a real program.
    fn program_importing(dlls: &[&str]) -> Vec<u8> {
        const PE: usize = 0x40;
        const OPTIONAL: usize = PE + 24;
        const OPTIONAL_SIZE: usize = 240;
        const IN_FILE: usize = 0x200;
        const ADDRESS: usize = 0x1000;

        let mut section = vec![0; (dlls.len() + 1) * 20];
        for (index, dll) in dlls.iter().enumerate() {
            let name = ADDRESS + section.len();
            put(&mut section, index * 20 + 12, 4, name);
            section.extend(dll.bytes().chain([0]));
        }
        let mut file = vec![0; IN_FILE];
        put(&mut file, 0x3C, 4, PE);
        file[PE..PE + 4].copy_from_slice(b"PE\0\0");
        put(&mut file, PE + 6, 2, 1);
        put(&mut file, PE + 20, 2, OPTIONAL_SIZE);
        put(&mut file, OPTIONAL, 2, 0x20B);
        put(&mut file, OPTIONAL + 112 + 8, 4, ADDRESS);
        let header = OPTIONAL + OPTIONAL_SIZE;
        put(&mut file, header + 12, 4, ADDRESS);
        put(&mut file, header + 16, 4, section.len());
        put(&mut file, header + 20, 4, IN_FILE);
        file.extend(section);
        file
    }

    /// `run` over a temporary file holding `program`.
    fn run_over(name: &str, program: &[u8]) -> Result<bool> {
        let id = std::process::id();
        let path = std::env::temp_dir().join(format!("annulus-standalone-{id}-{name}.exe"));
        std::fs::write(&path, program)?;
        let result = run(&[path.display().to_string()]);
        let _ = std::fs::remove_file(&path);
        result
    }

    #[test]
    fn reads_the_dlls_a_program_imports() {
        let program = program_importing(&["KERNEL32.dll", "d2d1.dll"]);
        assert_eq!(imports(&program).unwrap(), ["KERNEL32.dll", "d2d1.dll"]);
    }

    #[test]
    fn tells_microsofts_runtime_from_windows_own_dlls() {
        let dlls = [
            "KERNEL32.dll",
            "VCRUNTIME140.dll",
            "d2d1.dll",
            "api-ms-win-crt-heap-l1-1-0.dll",
            "api-ms-win-core-synch-l1-2-0.dll",
            "vcruntime140_1.dll",
        ]
        .map(String::from);
        assert_eq!(
            runtime_dlls(&dlls),
            [
                "VCRUNTIME140.dll",
                "api-ms-win-crt-heap-l1-1-0.dll",
                "vcruntime140_1.dll"
            ]
        );
    }

    #[test]
    fn a_program_that_needs_the_runtime_fails_the_check() {
        let program = program_importing(&["KERNEL32.dll", "VCRUNTIME140.dll"]);
        assert!(!run_over("needs", &program).unwrap());
    }

    #[test]
    fn a_program_that_stands_alone_passes_the_check() {
        let program = program_importing(&["KERNEL32.dll", "d2d1.dll"]);
        assert!(run_over("alone", &program).unwrap());
    }

    #[test]
    fn a_file_that_is_no_windows_program_is_an_error_not_a_pass() {
        assert!(run_over("script", b"#!/bin/sh\n").is_err());
    }

    #[test]
    fn a_program_that_seems_to_import_nothing_is_an_error_not_a_pass() {
        assert!(run_over("empty", &program_importing(&[])).is_err());
    }
}
