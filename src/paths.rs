use std::path::PathBuf;
use anyhow::Result;

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
