use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

pub enum AuthCmd {
    Init {
        recipient: Option<String>,
        identity: Option<PathBuf>,
        force: bool,
    },
    Show {
        identity: Option<PathBuf>,
    },
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

fn token_age_path() -> Result<PathBuf> {
    Ok(token_dir()?.join("approve.token.age"))
}

fn token_hash_path() -> Result<PathBuf> {
    Ok(token_dir()?.join("approve.token.hash"))
}

fn default_identity_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("sops")
            .join("age")
            .join("keys.txt"),
    )
}

fn age_bin() -> String {
    std::env::var("STORES_AGE_BIN").unwrap_or_else(|_| "age".to_string())
}

// ---------------------------------------------------------------------------
// Recipient discovery
// ---------------------------------------------------------------------------

/// Auto-discover an age recipient from a sops/age keys.txt file by parsing the
/// `# public key: age1...` comment line.
fn discover_recipient(identity: &Path) -> Result<String> {
    let content = std::fs::read_to_string(identity)
        .with_context(|| format!("failed to read identity file {}", identity.display()))?;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# public key:") {
            let key = rest.trim();
            if key.starts_with("age1") {
                return Ok(key.to_string());
            }
        }
    }
    bail!(
        "could not auto-discover age recipient in {}; pass --recipient age1...",
        identity.display()
    )
}

// ---------------------------------------------------------------------------
// Identity validation
// ---------------------------------------------------------------------------

/// Reject a raw plaintext age identity (`AGE-SECRET-KEY-...`). Accept armored
/// (`-----BEGIN AGE ENCRYPTED FILE-----`) or plugin (`AGE-PLUGIN-...`) forms.
fn validate_identity_protected(identity: &Path) -> Result<()> {
    let content = std::fs::read_to_string(identity)
        .with_context(|| format!("failed to read identity file {}", identity.display()))?;

    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with("AGE-SECRET-KEY-") {
            bail!(
                "identity at {} is a raw plaintext age key (AGE-SECRET-KEY-...); \
                 stores auth init refuses to use unprotected identities. \
                 Remedy: passphrase-encrypt your age key (e.g. `age -p -o keys.age keys.txt`) \
                 or use a hardware-backed AGE-PLUGIN- identity.",
                identity.display()
            );
        }
    }
    Ok(())
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
        AuthCmd::Init {
            recipient,
            identity,
            force,
        } => init(recipient, identity, force),
        AuthCmd::Show { identity } => show(identity),
    }
}

fn init(recipient: Option<String>, identity: Option<PathBuf>, force: bool) -> Result<()> {
    // L016 fix: hex-encode the random secret so it round-trips cleanly as a
    // UTF-8 string through `--approve-token <T>`. Raw 32 random bytes almost
    // never form valid UTF-8, breaking the verify path that hashes the
    // string's bytes. Hex is 64 ASCII chars; safe for CLI args.
    let mut secret_raw = [0u8; 32];
    getrandom::getrandom(&mut secret_raw)
        .map_err(|e| anyhow!("failed to read system entropy: {e}"))?;
    let secret_hex = hex::encode(secret_raw);
    let recip = init_with_secret(recipient, identity, force, secret_hex.as_bytes())?;
    println!("  recipient: {}", recip);
    println!();
    println!("Note: ~/.config/stores/ is OUTSIDE this repo and is NOT gitignored");
    println!("by the project. Ensure your home directory is not under version control,");
    println!("or add ~/.config/stores/ to your global gitignore.");
    Ok(())
}

/// Internal: shared by the user-facing `init` and tests that need to assert
/// against a known plaintext secret. Returns the resolved recipient.
fn init_with_secret(
    recipient: Option<String>,
    identity: Option<PathBuf>,
    force: bool,
    secret: &[u8],
) -> Result<String> {
    let dir = token_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let age_path = token_age_path()?;
    let hash_path = token_hash_path()?;

    if !force && (age_path.exists() || hash_path.exists()) {
        bail!(
            "{} or {} already exists; pass --force to overwrite",
            age_path.display(),
            hash_path.display()
        );
    }

    // Resolve identity (used for recipient discovery + plaintext-key check).
    let identity_path = identity.or_else(default_identity_path);

    if let Some(ref id) = identity_path {
        if id.exists() {
            validate_identity_protected(id)?;
        }
    }

    // Resolve recipient: explicit > auto-discover from identity.
    let recip = match recipient {
        Some(r) => r,
        None => {
            let id = identity_path
                .as_ref()
                .ok_or_else(|| anyhow!("no --recipient and no identity file available"))?;
            if !id.exists() {
                bail!(
                    "no --recipient supplied and identity file {} does not exist; \
                     pass --recipient age1... or --identity <path>",
                    id.display()
                );
            }
            discover_recipient(id)?
        }
    };

    // Compute hash of plaintext secret.
    let mut hasher = Sha256::new();
    hasher.update(secret);
    let digest = hasher.finalize();
    let hex_hash = hex::encode(digest);

    // Encrypt via `age -r <recipient> -o <path>`, feeding the secret on stdin.
    encrypt_with_age(&recip, secret, &age_path)?;
    set_mode(&age_path, 0o600)?;

    // Write hex hash.
    std::fs::write(&hash_path, &hex_hash)
        .with_context(|| format!("failed to write {}", hash_path.display()))?;
    set_mode(&hash_path, 0o644)?;

    println!("Initialized approval token:");
    println!("  encrypted: {} (0600)", age_path.display());
    println!("  hash:      {} (0644)", hash_path.display());
    Ok(recip)
}

fn encrypt_with_age(recipient: &str, plaintext: &[u8], out: &Path) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let bin = age_bin();
    let mut child = Command::new(&bin)
        .arg("-r")
        .arg(recipient)
        .arg("-o")
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{bin}` (set STORES_AGE_BIN if needed)"))?;

    {
        let stdin = child.stdin.as_mut().ok_or_else(|| anyhow!("no stdin"))?;
        stdin
            .write_all(plaintext)
            .context("failed to write plaintext to age stdin")?;
    }

    let output = child.wait_with_output().context("age process failed")?;
    if !output.status.success() {
        bail!(
            "age encryption failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn show(identity: Option<PathBuf>) -> Result<()> {
    let age_path = token_age_path()?;
    if !age_path.exists() {
        bail!(
            "no approval token at {}; run `stores auth init` first",
            age_path.display()
        );
    }
    let identity = identity
        .or_else(default_identity_path)
        .ok_or_else(|| anyhow!("HOME not set; cannot locate identity file"))?;
    if !identity.exists() {
        bail!(
            "identity file {} not found; cannot decrypt token",
            identity.display()
        );
    }

    let bin = age_bin();
    let status = Command::new(&bin)
        .arg("-d")
        .arg("-i")
        .arg(&identity)
        .arg(&age_path)
        .status()
        .with_context(|| format!("failed to spawn `{bin}`"))?;
    if !status.success() {
        bail!("age decryption failed");
    }
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
///
/// On Err (corrupted hash file, IO error other than "not found"), we log a
/// one-line diagnostic to stderr before collapsing to `false`, so the user
/// can distinguish "wrong token" from "the hash file is unreadable" without
/// having to add `--debug` plumbing.
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

    /// Write a "wrapping" age stub: reads stdin, writes header + hex(stdin) to
    /// -o path. The hex form ensures raw plaintext bytes do NOT appear on disk
    /// (each byte becomes two ASCII nibbles), modeling real age's property
    /// without requiring an actual age binary in CI.
    fn write_wrapping_age(dir: &Path) -> PathBuf {
        let script = dir.join("age-stub.sh");
        let body = r#"#!/usr/bin/env bash
set -e
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2;;
    -r) shift 2;;
    -d) shift;;
    -i) shift 2;;
    *) shift;;
  esac
done
if [ -z "$out" ]; then
  cat
else
  printf 'AGE-MOCK-WRAPPED\n' > "$out"
  od -An -tx1 -v | tr -d ' \n' >> "$out"
  printf '\n' >> "$out"
fi
"#;
        fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    fn write_show_age(dir: &Path) -> PathBuf {
        let script = dir.join("age-show-stub.sh");
        let body = r#"#!/usr/bin/env bash
set -e
identity=""
while [ $# -gt 0 ]; do
  case "$1" in
    -d) shift;;
    -i) identity="$2"; shift 2;;
    *) shift;;
  esac
done
if [ "$identity" != "$EXPECT_IDENTITY" ]; then
  echo "wrong identity: $identity" >&2
  exit 42
fi
printf 'fixture-token\n'
"#;
        fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    fn contains_window(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || haystack.len() < needle.len() {
            return false;
        }
        haystack.windows(needle.len()).any(|w| w == needle)
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

    const AGE1: &str = "age1xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

    fn armored_id(path: &Path) {
        fs::write(
            path,
            "-----BEGIN AGE ENCRYPTED FILE-----\nblob\n-----END AGE ENCRYPTED FILE-----\n",
        )
        .unwrap();
    }

    #[test]
    fn refuses_init_on_raw_plaintext_identity() {
        let tdir = unique_dir("plaintext");
        let id = tdir.join("keys.txt");
        fs::write(
            &id,
            "# created: 2024-01-01\n# public key: age1exampleexampleexampleexampleexampleexampleexampleexample\nAGE-SECRET-KEY-1FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEXX\n",
        )
        .unwrap();
        let stub = write_wrapping_age(&tdir);

        with_env(
            &[
                ("STORES_TOKEN_DIR", tdir.to_str().unwrap()),
                ("STORES_AGE_BIN", stub.to_str().unwrap()),
            ],
            || {
                let err = init(None, Some(id.clone()), false).unwrap_err();
                let msg = err.to_string();
                assert!(
                    msg.contains("passphrase-encrypt your age key"),
                    "expected remedy hint, got: {msg}"
                );
            },
        );
    }

    #[test]
    fn init_writes_age_and_hash_files() {
        let tdir = unique_dir("ok");
        let id = tdir.join("id.txt");
        armored_id(&id);
        let stub = write_wrapping_age(&tdir);

        with_env(
            &[
                ("STORES_TOKEN_DIR", tdir.to_str().unwrap()),
                ("STORES_AGE_BIN", stub.to_str().unwrap()),
            ],
            || {
                let secret = [0xABu8; 32];
                init_with_secret(Some(AGE1.into()), Some(id.clone()), false, &secret).unwrap();
                let age = tdir.join("approve.token.age");
                let hash = tdir.join("approve.token.hash");
                assert!(age.exists());
                assert!(hash.exists());

                let hash_content = fs::read_to_string(&hash).unwrap();
                assert_eq!(hash_content.trim().len(), 64, "hash must be 64 hex chars");
                assert!(hash_content.trim().chars().all(|c| c.is_ascii_hexdigit()));

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let m_age = fs::metadata(&age).unwrap().permissions().mode() & 0o777;
                    let m_hash = fs::metadata(&hash).unwrap().permissions().mode() & 0o777;
                    assert_eq!(m_age, 0o600, "age file must be 0600");
                    assert_eq!(m_hash, 0o644, "hash file must be 0644");
                }
            },
        );
    }

    /// AC1.4 / Task 1.7(a): raw plaintext bytes must NOT appear on disk in the
    /// .age file. We use a generated 32-byte secret with a high-entropy pattern
    /// that is unlikely to occur in the hex-encoded mock ciphertext.
    #[test]
    fn raw_bytes_not_on_disk() {
        let tdir = unique_dir("noraw");
        let id = tdir.join("id.txt");
        armored_id(&id);
        let stub = write_wrapping_age(&tdir);

        with_env(
            &[
                ("STORES_TOKEN_DIR", tdir.to_str().unwrap()),
                ("STORES_AGE_BIN", stub.to_str().unwrap()),
            ],
            || {
                // Use a secret with bytes >= 0x80 — guaranteed non-ASCII, so it
                // cannot appear verbatim in the hex-encoded output of the stub.
                let secret: [u8; 32] = [
                    0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c,
                    0x8d, 0x8e, 0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99,
                    0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x9f,
                ];
                init_with_secret(Some(AGE1.into()), Some(id.clone()), false, &secret).unwrap();

                let on_disk = fs::read(tdir.join("approve.token.age")).unwrap();
                assert!(
                    !contains_window(&on_disk, &secret),
                    "raw 32-byte plaintext must not appear in encrypted file"
                );
            },
        );
    }

    /// AC1.5 / Task 1.7(c): hash file equals sha256(plaintext_bytes).
    #[test]
    fn hash_matches_sha256_of_plaintext() {
        let tdir = unique_dir("hashmatch");
        let id = tdir.join("id.txt");
        armored_id(&id);
        let stub = write_wrapping_age(&tdir);

        with_env(
            &[
                ("STORES_TOKEN_DIR", tdir.to_str().unwrap()),
                ("STORES_AGE_BIN", stub.to_str().unwrap()),
            ],
            || {
                let secret = [0x42u8; 32];
                init_with_secret(Some(AGE1.into()), Some(id.clone()), false, &secret).unwrap();

                let mut hasher = Sha256::new();
                hasher.update(secret);
                let expected = hex::encode(hasher.finalize());

                let stored = fs::read_to_string(tdir.join("approve.token.hash")).unwrap();
                assert_eq!(stored.trim(), expected);
            },
        );
    }

    /// Constant-time `verify_token` round-trip: token string in → SHA-256 →
    /// compare against on-disk hash.
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
        let id = tdir.join("id.txt");
        armored_id(&id);
        let stub = write_wrapping_age(&tdir);

        with_env(
            &[
                ("STORES_TOKEN_DIR", tdir.to_str().unwrap()),
                ("STORES_AGE_BIN", stub.to_str().unwrap()),
            ],
            || {
                let secret = [0u8; 32];
                init_with_secret(Some(AGE1.into()), Some(id.clone()), false, &secret).unwrap();
                let err = init_with_secret(Some(AGE1.into()), Some(id.clone()), false, &secret)
                    .unwrap_err();
                assert!(err.to_string().contains("--force"));
                // With force, succeeds.
                init_with_secret(Some(AGE1.into()), Some(id.clone()), true, &secret).unwrap();
            },
        );
    }

    /// Phase 2 / Task 2.5(a): plaintext token whose sha256 matches the on-disk
    /// hash returns true.
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

    /// Phase 2 / Task 2.5(b): wrong plaintext returns false.
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

    /// Phase 2 / Task 2.5(c): missing hash file → false (never panics).
    #[test]
    fn verify_approve_token_missing_hash_returns_false() {
        let tdir = unique_dir("vatok-miss");
        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            // Note: NO hash file written.
            assert!(!verify_approve_token("anything"));
        });
    }

    /// Phase 2 / Task 2.5(d): equal-prefix-different-suffix smoke-tests the
    /// constant-time path. (This does NOT prove timing-equality at the byte
    /// level — that requires statistical timing analysis — but it confirms the
    /// `subtle::ConstantTimeEq` code path is exercised and correctly returns
    /// false when only the suffix differs.)
    #[test]
    fn verify_approve_token_equal_prefix_different_suffix_returns_false() {
        let tdir = unique_dir("vatok-prefix");
        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let real = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 32 A's
            let mut h = Sha256::new();
            h.update(real.as_bytes());
            fs::write(tdir.join("approve.token.hash"), hex::encode(h.finalize())).unwrap();

            // Same prefix (first 28 chars), different last 4.
            let attacker = "AAAAAAAAAAAAAAAAAAAAAAAAAAAABBBB";
            assert!(!verify_approve_token(attacker));
            // Sanity: real token still verifies.
            assert!(verify_approve_token(real));
        });
    }

    #[test]
    fn auto_discovers_recipient_from_keys_txt() {
        let tdir = unique_dir("discover");
        let id = tdir.join("keys.txt");
        let pubkey = "age1examplepubkey00000000000000000000000000000000000000000000";
        fs::write(
            &id,
            format!("# created: 2024-01-01\n# public key: {pubkey}\n-----BEGIN AGE ENCRYPTED FILE-----\nblob\n"),
        )
        .unwrap();
        let stub = write_wrapping_age(&tdir);

        with_env(
            &[
                ("STORES_TOKEN_DIR", tdir.to_str().unwrap()),
                ("STORES_AGE_BIN", stub.to_str().unwrap()),
            ],
            || {
                let recip = init_with_secret(None, Some(id.clone()), false, &[1u8; 32]).unwrap();
                assert_eq!(recip, pubkey);
                assert!(tdir.join("approve.token.age").exists());
            },
        );
    }

    #[test]
    fn show_uses_explicit_identity() {
        let tdir = unique_dir("show-explicit");
        let id = tdir.join("custom.keys.txt");
        armored_id(&id);
        fs::write(tdir.join("approve.token.age"), "ciphertext\n").unwrap();
        let stub = write_show_age(&tdir);

        with_env(
            &[
                ("STORES_TOKEN_DIR", tdir.to_str().unwrap()),
                ("STORES_AGE_BIN", stub.to_str().unwrap()),
                ("EXPECT_IDENTITY", id.to_str().unwrap()),
            ],
            || show(Some(id.clone())).unwrap(),
        );
    }

    #[test]
    fn show_without_identity_uses_default_identity() {
        let tdir = unique_dir("show-default");
        fs::write(tdir.join("approve.token.age"), "ciphertext\n").unwrap();
        let home = unique_dir("home");
        let id = home
            .join(".config")
            .join("sops")
            .join("age")
            .join("keys.txt");
        fs::create_dir_all(id.parent().unwrap()).unwrap();
        armored_id(&id);
        let stub = write_show_age(&tdir);

        with_env(
            &[
                ("STORES_TOKEN_DIR", tdir.to_str().unwrap()),
                ("STORES_AGE_BIN", stub.to_str().unwrap()),
                ("EXPECT_IDENTITY", id.to_str().unwrap()),
                ("HOME", home.to_str().unwrap()),
            ],
            || show(None).unwrap(),
        );
    }

    #[test]
    fn show_errors_cleanly_for_missing_explicit_identity() {
        let tdir = unique_dir("show-missing");
        fs::write(tdir.join("approve.token.age"), "ciphertext\n").unwrap();
        let missing = tdir.join("missing.keys.txt");

        with_env(&[("STORES_TOKEN_DIR", tdir.to_str().unwrap())], || {
            let err = show(Some(missing.clone())).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("identity file") && msg.contains("not found"),
                "expected clean missing identity error, got: {msg}"
            );
        });
    }
}
