use anyhow::Result;

use crate::db;
use crate::manifest::Manifest;
use crate::paths::{db_path, manifest_path, stores_dir};

pub fn run() -> Result<()> {
    let stores_dir = stores_dir()?;
    let db_path = db_path()?;
    let manifest_path = manifest_path()?;

    // Check if already fully initialized
    if db_path.exists() && manifest_path.exists() {
        println!("Already initialized at {}", stores_dir.display());
        return Ok(());
    }

    // Create .stores/ directory if needed
    if !stores_dir.exists() {
        std::fs::create_dir_all(&stores_dir)?;
    }

    // Create db.sqlite if missing
    if !db_path.exists() {
        db::open(&db_path)?;
        println!("Created {} (WAL)", db_path.display());
    }

    // Create manifest.yaml if missing
    if !manifest_path.exists() {
        Manifest::empty().save()?;
        println!("Created {}", manifest_path.display());
    }

    Ok(())
}
