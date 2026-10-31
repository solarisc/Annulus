//! The local control door of a running Annulus (ADR-0020, F-AUTO): the line protocol, the
//! [`door::Door`] the app opens while the owner allows it, and the [`client::Client`] the
//! `annulus` CLI and the MCP server use.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

pub mod client;
pub mod door;
pub mod protocol;

use std::path::{Path, PathBuf};

/// The file in the data folder that names the open door and holds its token. It exists only
/// while the door is open.
pub const DOOR_FILE: &str = "control.json";

/// Where Annulus keeps its data: the OS's per-user data folder (ADR-0007 §Data locations), or
/// the working directory if the OS reports none. The app and the CLI agree on it this way.
#[must_use]
pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "Annulus").map_or_else(
        || PathBuf::from("annulus-data"),
        |dirs| dirs.data_dir().to_path_buf(),
    )
}

/// The door file of the data folder `dir`.
#[must_use]
pub fn door_file(dir: &Path) -> PathBuf {
    dir.join(DOOR_FILE)
}
