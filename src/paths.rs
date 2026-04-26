use std::path::PathBuf;
use anyhow::{bail, Result};

pub fn stores_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(".stores"))
}

pub fn db_path() -> Result<PathBuf> {
    Ok(stores_dir()?.join("db.sqlite"))
}

pub fn manifest_path() -> Result<PathBuf> {
    Ok(stores_dir()?.join("manifest.yaml"))
}

/// Check that `.stores/` has been initialized (db + manifest both present).
/// Returns an error directing the user to run `stores init` if not.
pub fn ensure_initialized() -> Result<()> {
    let dir = stores_dir()?;
    if !dir.exists() || !db_path()?.exists() || !manifest_path()?.exists() {
        bail!(
            ".stores/ is not initialized in '{}'; run `stores init` first",
            std::env::current_dir()?.display()
        );
    }
    Ok(())
}
