use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use crate::codegen::ddl::ddl_for;
use crate::db;
use crate::manifest::{InstalledStore, Manifest};
use crate::paths::{db_path, ensure_initialized};
use crate::schema::flatten::leaf_args;
use crate::schema::Schema;

/// Entry point for `stores install <path-or-bundled-name>`.
///
/// If `path` is a single component that matches a bundled store name, install
/// from the embedded schema. Otherwise treat it as a filesystem path.
pub fn run(path: &Path) -> Result<()> {
    // Check if this looks like a bare bundled name (no path separator, no .yaml)
    let path_str = path.to_string_lossy();
    if !path_str.contains(std::path::MAIN_SEPARATOR)
        && !path_str.contains('/')
        && !path_str.ends_with(".yaml")
    {
        let name = path_str.as_ref();
        if let Some(&(_, schema_yaml)) = BUNDLED_STORE_SCHEMAS.iter().find(|(n, _)| *n == name) {
            return install_bundled(name, schema_yaml);
        }
    }

    // 1. Resolve to canonical absolute path
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot resolve path '{}'", path.display()))?;

    // 2. Read and parse schema.yaml
    let schema_file = canonical.join("schema.yaml");
    let yaml = std::fs::read_to_string(&schema_file)
        .with_context(|| format!("cannot read '{}'", schema_file.display()))?;
    let schema = Schema::from_yaml(&yaml)
        .with_context(|| format!("schema parse error in '{}'", schema_file.display()))?;

    // 3. Run leaf_args uniqueness check (errors if leaves collide)
    leaf_args(&schema).with_context(|| {
        format!("schema '{}' has leaf-arg collisions", schema.name)
    })?;

    // 4. Ensure .stores/ is initialized; open DB
    ensure_initialized()?;
    let db = db_path()?;
    let conn = db::open(&db)?;

    // 5. Load manifest and check for conflicts
    let mut manifest = Manifest::load()?;

    // Check for re-install by same canonical path
    let canonical_str = canonical.to_string_lossy();
    if let Some(existing) = manifest
        .stores
        .iter()
        .find(|s| s.schema_path == canonical)
    {
        bail!(
            "store '{}' is already installed from this path ({}); \
             v0.1 has no migrations — to reinstall, remove it from the manifest manually",
            existing.name,
            canonical_str
        );
    }

    // Check for name collision (same name, different path)
    if let Some(existing) = manifest.stores.iter().find(|s| s.name == schema.name) {
        bail!(
            "a store named '{}' is already installed (from {}); \
             v0.1 has no migrations — store names must be unique",
            existing.name,
            existing.schema_path.display()
        );
    }

    // 6. Codegen DDL
    let ddl = ddl_for(&schema);

    // 7. Execute DDL inside a transaction
    conn.execute_batch(&format!("BEGIN;\n{ddl}\nCOMMIT;"))
        .with_context(|| format!("failed to apply DDL for store '{}'", schema.name))?;

    // 8. Build and save manifest entry
    let installed_at = chrono_now();
    let entry = InstalledStore {
        name: schema.name.clone(),
        schema_path: canonical,
        installed_at,
        table_name: schema.name.clone(),
        scope: schema.scope,
    };
    manifest.stores.push(entry);
    manifest.save()?;

    // 9. Print success
    println!(
        "Installed store '{}' (table: {})",
        schema.name, schema.name
    );

    Ok(())
}

/// Install a bundled store from embedded YAML content.
fn install_bundled(name: &str, schema_yaml: &str) -> Result<()> {
    let schema = Schema::from_yaml(schema_yaml)
        .with_context(|| format!("bundled schema parse error for '{name}'"))?;

    leaf_args(&schema).with_context(|| {
        format!("bundled schema '{}' has leaf-arg collisions", schema.name)
    })?;

    ensure_initialized()?;
    let db = db_path()?;
    let conn = db::open(&db)?;

    let mut manifest = Manifest::load()?;

    // For bundled stores, use a synthetic path: "bundled:<name>"
    // We store a relative sentinel so the manifest records the source clearly.
    let sentinel_path = std::path::PathBuf::from(format!("bundled:{name}"));

    if let Some(existing) = manifest.stores.iter().find(|s| s.schema_path == sentinel_path) {
        bail!(
            "bundled store '{}' is already installed (from {}); \
             v0.1 has no migrations — to reinstall, remove it from the manifest manually",
            existing.name,
            existing.schema_path.display()
        );
    }
    if let Some(existing) = manifest.stores.iter().find(|s| s.name == schema.name) {
        bail!(
            "a store named '{}' is already installed (from {}); \
             v0.1 has no migrations — store names must be unique",
            existing.name,
            existing.schema_path.display()
        );
    }

    let ddl = ddl_for(&schema);
    conn.execute_batch(&format!("BEGIN;\n{ddl}\nCOMMIT;"))
        .with_context(|| format!("failed to apply DDL for bundled store '{}'", schema.name))?;

    let installed_at = chrono_now();
    let entry = InstalledStore {
        name: schema.name.clone(),
        schema_path: sentinel_path,
        installed_at,
        table_name: schema.name.clone(),
        scope: schema.scope,
    };
    manifest.stores.push(entry);
    manifest.save()?;

    println!(
        "Installed bundled store '{}' (table: {})",
        schema.name, schema.name
    );

    Ok(())
}

/// Return current UTC time as ISO-8601 string (seconds precision).
fn chrono_now() -> String {
    // Use std::time to avoid adding a chrono dependency.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Format as ISO-8601: YYYY-MM-DDTHH:MM:SSZ
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Minimal UTC calendar decomposition (no external dep).
fn unix_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s = secs % 60;
    let total_min = secs / 60;
    let mi = total_min % 60;
    let total_hr = total_min / 60;
    let h = total_hr % 24;
    let days = total_hr / 24;

    // Days since 1970-01-01
    let (y, mo, d) = days_to_ymd(days);
    (y, mo, d, h as u32, mi as u32, s as u32)
}

fn days_to_ymd(mut days: u64) -> (u32, u32, u32) {
    // Use the proleptic Gregorian calendar algorithm.
    // Start from 1970-01-01.
    let mut year = 1970u32;
    loop {
        let dy = days_in_year(year) as u64;
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let mut month = 1u32;
    loop {
        let dm = days_in_month(year, month) as u64;
        if days < dm {
            break;
        }
        days -= dm;
        month += 1;
    }
    (year, month, days as u32 + 1)
}

fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

fn days_in_year(y: u32) -> u32 {
    if is_leap(y) { 366 } else { 365 }
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap(y) { 29 } else { 28 },
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrono_now_format() {
        let ts = chrono_now();
        // Must match YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(ts.len(), 20, "unexpected length: {ts}");
        assert!(ts.ends_with('Z'), "must end with Z: {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
    }
}
