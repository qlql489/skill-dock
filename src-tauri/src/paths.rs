//! Path utilities.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Returns the data directory for SkillDock.
///
/// New installations use `~/.skill-dock/`. When upgrading from Skill Manager,
/// keep using the existing legacy directory so sources, installations and
/// preferences remain available without moving user data during startup.
pub fn data_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    let current = home.join(".skill-dock");
    if current.exists() {
        return Ok(current);
    }

    let legacy = home.join(".skill-manager");
    if legacy.exists() {
        return Ok(legacy);
    }

    Ok(current)
}

#[allow(dead_code)]
pub fn state_file() -> Result<PathBuf> {
    Ok(data_dir()?.join("state.json"))
}

pub fn repos_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("repos"))
}
