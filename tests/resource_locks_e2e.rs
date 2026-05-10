use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Mutex, OnceLock};

fn lock() -> &'static Mutex<()> { static L: OnceLock<Mutex<()>> = OnceLock::new(); L.get_or_init(|| Mutex::new(())) }
fn bin() -> &'static str { env!("CARGO_BIN_EXE_stores") }

fn write_token(dir: &Path, token: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let mut h = Sha256::new(); h.update(token.as_bytes());
    std::fs::write(dir.join("approve.token.hash"), hex::encode(h.finalize())).unwrap();
}

fn run(repo: &Path, token_dir: &Path, args: &[&str]) -> Output {
    Command::new(bin()).current_dir(repo).env("STORES_TOKEN_DIR", token_dir).args(args).output().unwrap()
}

fn setup() -> (tempfile::TempDir, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".stores")).unwrap();
    let tok = tempfile::tempdir().unwrap();
    write_token(tok.path(), "ok-token");
    (tmp, tok)
}

#[test]
fn cli_roundtrip_busy_and_history() {
    let _g = lock().lock().unwrap();
    let (tmp, tok) = setup();
    let out = run(tmp.path(), tok.path(), &["resource-locks","acquire","--resource","main_branch","--owner","T999","--owner-kind","task","--ttl-secs","60","--invoker","human"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!token.is_empty());
    let list = run(tmp.path(), tok.path(), &["resource-locks","list"]);
    let body = String::from_utf8_lossy(&list.stdout);
    assert!(body.contains("main_branch") && body.contains(&token), "{body}");
    let busy = run(tmp.path(), tok.path(), &["resource-locks","acquire","--resource","main_branch","--owner","T999","--owner-kind","task","--invoker","human"]);
    assert!(!busy.status.success());
    let err = String::from_utf8_lossy(&busy.stderr);
    assert!(err.contains("ResourceLockBusy") || err.contains("BUSY"), "{err}");
    let rel = run(tmp.path(), tok.path(), &["resource-locks","release","--resource","main_branch","--token",&token,"--invoker","human"]);
    assert!(rel.status.success(), "{}", String::from_utf8_lossy(&rel.stderr));
    let empty = run(tmp.path(), tok.path(), &["resource-locks","list"]);
    assert!(String::from_utf8_lossy(&empty.stdout).trim().is_empty());
    let conn = rusqlite::Connection::open(tmp.path().join(".stores/db.sqlite")).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM transition_history WHERE store='resource_locks'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);
}

#[test]
fn cli_acquire_rejects_ai_autonomous() {
    let _g = lock().lock().unwrap(); let (tmp, tok) = setup();
    let o = run(tmp.path(), tok.path(), &["resource-locks","acquire","--resource","X","--owner","T999","--owner-kind","task","--invoker","ai_autonomous"]);
    assert!(!o.status.success()); assert!(String::from_utf8_lossy(&o.stderr).contains("ai_autonomous"));
}

#[test]
fn cli_release_rejects_ai_autonomous() {
    let _g = lock().lock().unwrap(); let (tmp, tok) = setup();
    let o = run(tmp.path(), tok.path(), &["resource-locks","release","--resource","X","--token","bad","--invoker","ai_autonomous"]);
    assert!(!o.status.success()); assert!(String::from_utf8_lossy(&o.stderr).contains("ai_autonomous"));
}

#[test]
fn cli_recover_stale_rejects_ai_autonomous() {
    let _g = lock().lock().unwrap(); let (tmp, tok) = setup();
    let o = run(tmp.path(), tok.path(), &["resource-locks","recover-stale","--invoker","ai_autonomous"]);
    assert!(!o.status.success()); assert!(String::from_utf8_lossy(&o.stderr).contains("ai_autonomous"));
}

#[test]
fn cli_ai_with_human_acquire_with_token_succeeds() {
    let _g = lock().lock().unwrap(); let (tmp, tok) = setup();
    let o = run(tmp.path(), tok.path(), &["resource-locks","acquire","--resource","X","--owner","T999","--owner-kind","task","--invoker","ai_with_human","--approve-token","ok-token"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn cli_recover_stale_emits_transition_history() {
    let _g = lock().lock().unwrap(); let (tmp, tok) = setup();
    let o = run(tmp.path(), tok.path(), &["resource-locks","acquire","--resource","X","--owner","T999","--owner-kind","task","--ttl-secs","0","--invoker","human"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    std::thread::sleep(std::time::Duration::from_secs(1));
    let r = run(tmp.path(), tok.path(), &["resource-locks","recover-stale","--invoker","ai_with_human","--approve-token","ok-token"]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let conn = rusqlite::Connection::open(tmp.path().join(".stores/db.sqlite")).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM transition_history WHERE store='resource_locks' AND verb='recover_stale' AND invoker='ai_with_human'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}
