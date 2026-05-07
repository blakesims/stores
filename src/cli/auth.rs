use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

pub enum AuthCmd {
    Init { force: bool },
    Show,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

pub fn token_dir() -> Result<PathBuf> {
    if let Ok(d) = std::env::var("STORES_TOKEN_DIR") {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var("HOME")
        .map_err(|_| anyhow!("HOME not set; cannot locate ~/.config/stores/"))?;
    Ok(PathBuf::from(home).join(".config").join("stores"))
}

fn token_path() -> Result<PathBuf> {
    Ok(token_dir()?.join("approve.token"))
}

fn token_hash_path() -> Result<PathBuf> {
    Ok(token_dir()?.join("approve.token.hash"))
}

fn legacy_token_age_path() -> Result<PathBuf> {
    Ok(token_dir()?.join("approve.token.age"))
}

// ---------------------------------------------------------------------------
// File mode helpers
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(p, perms)
        .with_context(|| format!("failed to chmod {}", p.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_p: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand entry
// ---------------------------------------------------------------------------

pub fn run(cmd: AuthCmd) -> Result<()> {
    match cmd {
        AuthCmd::Init { force } => init(force),
        AuthCmd::Show => show(),
    }
}

fn init(force: bool) -> Result<()> {
    // Hex-encode the random secret so it round-trips cleanly as a UTF-8
    // `--approve-token <T>` value. Hex is 64 ASCII chars for 32 random bytes.
    let mut secret_raw = [0u8; 32];
    getrandom::getrandom(&mut secret_raw)
        .map_err(|e| anyhow!("failed to read system entropy: {e}"))?;
    let secret_hex = hex::encode(secret_raw);
    init_with_secret(force, secret_hex.as_bytes())?;
    println!();
    println!("Note: ~/.config/stores/ is OUTSIDE this repo and is NOT gitignored");
    println!("by the project. Ensure your home directory is not under version control,");
    println!("or add ~/.config/stores/ to your global gitignore.");
    Ok(())
}

/// Internal: shared by the user-facing `init` and tests that need to assert
/// against a known plaintext secret.
fn init_with_secret(force: bool, secret: &[u8]) -> Result<()> {
    let dir = token_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let plain_path = token_path()?;
    let hash_path = token_hash_path()?;
    let legacy_age_path = legacy_token_age_path()?;

    if !force && (plain_path.exists() || hash_path.exists()) {
        bail!(
            "{} or {} already exists; pass --force to overwrite",
            plain_path.display(),
            hash_path.display()
        );
    }

    // Compute hash of plaintext secret.
    let mut hasher = Sha256::new();
    hasher.update(secret);
    let digest = hasher.finalize();
    let hex_hash = hex::encode(digest);

    // Write plaintext token and hash sidecar. The token is host-bound by mode 0600.
    std::fs::write(&plain_path, secret)
        .with_context(|| format!("failed to write {}", plain_path.display()))?;
    set_mode(&plain_path, 0o600)?;

    std::fs::write(&hash_path, &hex_hash)
        .with_context(|| format!("failed to write {}", hash_path.display()))?;
    set_mode(&hash_path, 0o644)?;

    if legacy_age_path.exists() {
        std::fs::remove_file(&legacy_age_path)
            .with_context(|| format!("failed to remove legacy {}", legacy_age_path.display()))?;
    }

    println!("Initialized approval token:");
    println!("  plaintext: {} (0600)", plain_path.display());
    println!("  hash:      {} (0644)", hash_path.display());
    Ok(())
}

fn show() -> Result<()> {
    let plain_path = token_path()?;
    if !plain_path.exists() {
        bail!(
            "no approval token at {}; run `stores auth init` first",
            plain_path.display()
        );
    }
    print!("{}", std::fs::read_to_string(&plain_path)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Verification helpers (used by transition gating in later phases)
// ---------------------------------------------------------------------------

/// Read the stored hex SHA-256 hash. Returns None if the hash file does not exist.
#[allow(dead_code)]
pub fn stored_hash() -> Result<Option<String>> {
    let p = token_hash_path()?;
    if !p.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&p)?;
    Ok(Some(s.trim().to_string()))
}

/// Constant-time check: does `token` (the plaintext) hash to the stored hash?
pub fn verify_token(token: &str) -> Result<bool> {
    use subtle::ConstantTimeEq;
    let stored = match stored_hash()? {
        Some(s) => s,
        None => return Ok(false),
    };
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let candidate = hex::encode(digest);
    Ok(candidate.as_bytes().ct_eq(stored.as_bytes()).into())
}

/// Phase 2 entry-point used by dispatch: returns `false` on every error path
/// (missing hash file, IO error, mismatch). Never panics, never propagates.
/// Constant-time compare is delegated to `verify_token`.
pub(crate) fn verify_approve_token(token: &str) -> bool {
    match verify_token(token) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("warning: approve-token verification failed to read/parse hash file: {e}");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir(tag: &str) -> PathBuf {
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("stores-auth-{}-{}-{}", tag, std::process::id(), ns));
        fs::create_dir_all(&p).unwrap();
        p
    }

    use crate::cli::test_support::ENV_LOCK;

    fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        f();
        for (k, v) in saved {
            match v {
                Some(val) => std::env::set_var(&k, val),
                None => std::env::remove_var(&k),
            }
        }
    }

    #[test]
    fn init_writes_plaintext_and_hash_files() {
        let tdir = unique_dir("ok");

        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let secret = b"0123456789abcdef0123456789abcdef";
            init_with_secret(false, secret).unwrap();
            let plain = tdir.join("approve.token");
            let hash = tdir.join("approve.token.hash");
            assert!(plain.exists());
            assert!(hash.exists());
            assert_eq!(fs::read(&plain).unwrap(), secret);

            let hash_content = fs::read_to_string(&hash).unwrap();
            assert_eq!(hash_content.trim().len(), 64, "hash must be 64 hex chars");
            assert!(hash_content.trim().chars().all(|c| c.is_ascii_hexdigit()));

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let m_plain = fs::metadata(&plain).unwrap().permissions().mode() & 0o777;
                let m_hash = fs::metadata(&hash).unwrap().permissions().mode() & 0o777;
                assert_eq!(m_plain, 0o600, "plaintext token file must be 0600");
                assert_eq!(m_hash, 0o644, "hash file must be 0644");
            }
        });
    }

    #[test]
    fn hash_matches_sha256_of_plaintext() {
        let tdir = unique_dir("hashmatch");

        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let secret = b"hash-me";
            init_with_secret(false, secret).unwrap();

            let mut hasher = Sha256::new();
            hasher.update(secret);
            let expected = hex::encode(hasher.finalize());

            let stored = fs::read_to_string(tdir.join("approve.token.hash")).unwrap();
            assert_eq!(stored.trim(), expected);
        });
    }

    #[test]
    fn verify_token_round_trip() {
        let tdir = unique_dir("verify");

        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let known = "literal-secret-token";
            let mut h = Sha256::new();
            h.update(known.as_bytes());
            let known_hash = hex::encode(h.finalize());
            fs::write(tdir.join("approve.token.hash"), &known_hash).unwrap();

            assert!(verify_token(known).unwrap(), "valid token must verify");
            assert!(
                !verify_token("wrong-token").unwrap(),
                "wrong token must reject"
            );
        });
    }

    #[test]
    fn refuses_overwrite_without_force() {
        let tdir = unique_dir("force");

        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let secret = b"literal-secret-token";
            init_with_secret(false, secret).unwrap();
            let err = init_with_secret(false, secret).unwrap_err();
            assert!(err.to_string().contains("--force"));
            init_with_secret(true, secret).unwrap();
        });
    }

    #[test]
    fn init_force_removes_legacy_age_token() {
        let tdir = unique_dir("legacy-age");

        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let legacy = tdir.join("approve.token.age");
            fs::write(&legacy, b"legacy encrypted token").unwrap();
            assert!(legacy.exists());

            init_with_secret(true, b"literal-secret-token").unwrap();

            assert!(!legacy.exists(), "legacy approve.token.age must be removed");
            assert!(tdir.join("approve.token").exists());
            assert!(tdir.join("approve.token.hash").exists());
        });
    }

    #[test]
    fn show_errors_cleanly_for_missing_plaintext_token() {
        let tdir = unique_dir("show-missing");

        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let err = show().unwrap_err();
            assert!(err.to_string().contains("no approval token"));
        });
    }

    #[test]
    fn verify_approve_token_matching_returns_true() {
        let tdir = unique_dir("vatok");
        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let plaintext = "correct-horse-battery-staple";
            let mut h = Sha256::new();
            h.update(plaintext.as_bytes());
            let hex_hash = hex::encode(h.finalize());
            fs::write(tdir.join("approve.token.hash"), &hex_hash).unwrap();
            assert!(verify_approve_token(plaintext));
        });
    }

    #[test]
    fn verify_approve_token_mismatching_returns_false() {
        let tdir = unique_dir("vatok-bad");
        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let mut h = Sha256::new();
            h.update(b"the-real-token");
            fs::write(tdir.join("approve.token.hash"), hex::encode(h.finalize())).unwrap();
            assert!(!verify_approve_token("not-the-real-token"));
        });
    }

    #[test]
    fn verify_approve_token_missing_hash_returns_false() {
        let tdir = unique_dir("vatok-miss");
        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            assert!(!verify_approve_token("anything"));
        });
    }

    #[test]
    fn verify_approve_token_equal_prefix_different_suffix_returns_false() {
        let tdir = unique_dir("vatok-prefix");
        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let real = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
            let mut h = Sha256::new();
            h.update(real.as_bytes());
            fs::write(tdir.join("approve.token.hash"), hex::encode(h.finalize())).unwrap();

            let attacker = "AAAAAAAAAAAAAAAAAAAAAAAAAAAABBBB";
            assert!(!verify_approve_token(attacker));
            assert!(verify_approve_token(real));
        });
    }
}
