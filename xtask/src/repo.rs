//! Reading the repository: its root, its files and the workspace crate graph.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};

/// The repository root, found from the xtask manifest so it works from any working directory.
pub fn root() -> anyhow::Result<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .context("xtask has no parent directory")
}

/// Every file git tracks or would track (respects `.gitignore`), as `/`-separated paths
/// relative to the root.
pub fn files(root: &Path) -> anyhow::Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()
        .context("could not run `git ls-files`")?;
    if !output.status.success() {
        bail!(
            "`git ls-files` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let listing = String::from_utf8(output.stdout).context("git listed a non-UTF-8 path")?;
    Ok(listing
        .split('\0')
        .filter(|path| !path.is_empty() && root.join(path).is_file())
        .map(str::to_owned)
        .collect())
}

/// A workspace member as `cargo metadata` describes it.
#[derive(Debug)]
pub struct Package {
    /// Package name, e.g. `annulus-term`.
    pub name: String,
    /// Names of all direct dependencies (normal, dev and build).
    pub dependencies: Vec<String>,
    /// Crate root files (`lib.rs`, `main.rs`) of the lib and bin targets.
    pub crate_roots: Vec<PathBuf>,
}

/// The workspace members, keyed by package name.
pub fn workspace_packages(root: &Path) -> anyhow::Result<BTreeMap<String, Package>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(root)
        .output()
        .context("could not run `cargo metadata`")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("`cargo metadata` printed invalid JSON")?;
    let packages = metadata["packages"]
        .as_array()
        .context("metadata has no packages")?;

    let mut result = BTreeMap::new();
    for package in packages {
        let name = package["name"]
            .as_str()
            .context("package without a name")?
            .to_owned();
        let dependencies = package["dependencies"]
            .as_array()
            .context("package without dependencies")?
            .iter()
            .filter_map(|dependency| dependency["name"].as_str().map(str::to_owned))
            .collect();
        let crate_roots = package["targets"]
            .as_array()
            .context("package without targets")?
            .iter()
            .filter(|target| {
                target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "lib" || kind == "bin"))
            })
            .filter_map(|target| target["src_path"].as_str().map(PathBuf::from))
            .collect();
        result.insert(
            name.clone(),
            Package {
                name,
                dependencies,
                crate_roots,
            },
        );
    }
    Ok(result)
}
