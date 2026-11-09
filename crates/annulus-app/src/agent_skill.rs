//! Installs Annulus's portable Agent Skill for local agent programs.

use std::path::{Path, PathBuf};

/// The skill shipped with the app.
const SKILL: &str = include_str!("../../../assets/skills/annulus/SKILL.md");

/// User skill roots that together cover Claude Code, Codex, Hermes and tools that read the
/// shared Agent Skills folder (Pi, Gemini CLI, Cursor, `OpenCode` and GitHub Copilot).
const ROOTS: [&str; 4] = [
    ".agents/skills",
    ".claude/skills",
    ".codex/skills",
    ".hermes/skills",
];

/// Installs the skill for the current user and returns the files written.
///
/// # Errors
/// The home folder could not be found, or a skill folder or file could not be written.
pub fn install() -> std::io::Result<Vec<PathBuf>> {
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home folder"))?;
    install_to(&home)
}

/// Installs into `home`; split out so the file layout can be checked without changing the
/// account that runs the tests.
fn install_to(home: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut written = Vec::with_capacity(ROOTS.len());
    for root in ROOTS {
        let folder = home.join(root).join("annulus");
        std::fs::create_dir_all(&folder)?;
        let path = folder.join("SKILL.md");
        std::fs::write(&path, SKILL)?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_annulus_skill_installs_for_named_and_shared_agent_harnesses() {
        let home =
            std::env::temp_dir().join(format!("annulus-skill-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let written = install_to(&home).unwrap();
        assert_eq!(written.len(), ROOTS.len());
        for path in written {
            assert_eq!(std::fs::read_to_string(path).unwrap(), SKILL);
        }
        assert!(home.join(".agents/skills/annulus/SKILL.md").is_file());
        assert!(home.join(".claude/skills/annulus/SKILL.md").is_file());
        assert!(home.join(".codex/skills/annulus/SKILL.md").is_file());
        assert!(home.join(".hermes/skills/annulus/SKILL.md").is_file());
        std::fs::remove_dir_all(home).unwrap();
    }
}
