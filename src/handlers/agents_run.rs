//! `stores agents run` daemon — Phase 4 of T014.
//!
//! Polls `transition_history` at a fixed interval, looks for entries that
//! match an agent's declared `subscribes_to` triple, atomically claims the
//! pair `(store, row_id, agent_name)` via INSERT into `dispatch_locks` (the
//! UNIQUE constraint is what gives us idempotency against parallel daemons),
//! and dispatches either a shell `command` or a `builtin:*` keyword.
//!
//! Builtins are stubbed in this phase (Phase 6 wires them).

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const STALE_DAEMON_MESSAGE: &str = "daemon binary stale after cargo install; restart required";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinaryIdentity {
    pub dev: u64,
    pub ino: u64,
}

pub trait BinaryIdentityProvider {
    fn identity(&self, path: &Path) -> Result<BinaryIdentity>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FsBinaryIdentityProvider;

impl BinaryIdentityProvider for FsBinaryIdentityProvider {
    fn identity(&self, path: &Path) -> Result<BinaryIdentity> {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::metadata(path)
            .with_context(|| format!("stat executable {}", path.display()))?;
        Ok(BinaryIdentity {
            dev: md.dev(),
            ino: md.ino(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonExeStatus {
    Fresh,
    Stale { message: &'static str },
}

pub struct DaemonExeGuard<P: BinaryIdentityProvider> {
    startup_identity: BinaryIdentity,
    launch_path: PathBuf,
    provider: P,
}

impl<P: BinaryIdentityProvider> DaemonExeGuard<P> {
    pub fn new(startup_identity: BinaryIdentity, launch_path: PathBuf, provider: P) -> Self {
        Self {
            startup_identity,
            launch_path,
            provider,
        }
    }

    pub fn startup_identity(&self) -> BinaryIdentity {
        self.startup_identity
    }

    pub fn launch_path(&self) -> &Path {
        &self.launch_path
    }

    pub fn current_status(&self) -> Result<DaemonExeStatus> {
        let current = self.provider.identity(&self.launch_path)?;
        if current == self.startup_identity {
            Ok(DaemonExeStatus::Fresh)
        } else {
            Ok(DaemonExeStatus::Stale {
                message: STALE_DAEMON_MESSAGE,
            })
        }
    }

    pub fn check_stale(&self) -> Result<Option<&'static str>> {
        match self.current_status()? {
            DaemonExeStatus::Fresh => Ok(None),
            DaemonExeStatus::Stale { message } => Ok(Some(message)),
        }
    }
}

impl DaemonExeGuard<FsBinaryIdentityProvider> {
    pub fn from_process() -> Result<Self> {
        let provider = FsBinaryIdentityProvider;
        let current_exe = std::env::current_exe().context("resolving current_exe")?;
        let private_path = ensure_private_daemon_binary(&current_exe, &provider)?;
        let mut startup_identity = provider.identity(&current_exe)?;
        #[cfg(debug_assertions)]
        if std::env::var_os("STORES_TEST_DAEMON_FORCE_STALE").is_some() {
            let private_identity = provider.identity(&private_path)?;
            startup_identity = BinaryIdentity {
                dev: private_identity.dev,
                ino: private_identity.ino.saturating_add(1),
            };
        }
        Ok(Self::new(startup_identity, private_path, provider))
    }
}

fn ensure_private_daemon_binary(
    current_exe: &Path,
    provider: &FsBinaryIdentityProvider,
) -> Result<PathBuf> {
    let private_path = crate::paths::ensure_daemon_binary_parent()?;

    // Existence shortcut: validate the existing binary before trusting it.
    // A partial write from a previous crashed seed would otherwise be returned
    // without any integrity check.
    if private_path.exists() {
        validate_stale_reexec_candidate(&private_path)
            .map_err(|f| anyhow!(private_candidate_validation_message(&f)))
            .with_context(|| {
                format!(
                    "existing private daemon binary {} failed validation; \
                     remove it manually to re-seed",
                    private_path.display()
                )
            })?;
        return Ok(private_path);
    }

    let source = resolve_launch_path_from_env().unwrap_or_else(|_| current_exe.to_path_buf());
    let source = if source.exists() {
        source
    } else {
        current_exe.to_path_buf()
    };

    // Atomic seed: copy to a unique temp file in the same directory, validate
    // it, then rename into place.  Using the same directory guarantees the
    // rename is atomic (same filesystem / mount point).  A process-id +
    // pseudo-random suffix ensures concurrent seeders don't collide on the
    // temp path itself.
    let parent = private_path
        .parent()
        .ok_or_else(|| anyhow!("private daemon binary path has no parent directory"))?;
    let tmp_name = format!(
        "stores.tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );
    let tmp_path = parent.join(&tmp_name);

    // Drop guard: if we exit this function with an error before the temp has
    // been consumed (renamed / hard-linked into place or explicitly removed),
    // the guard removes it so we never accumulate stale stores.tmp.<pid>.<nanos>
    // files in the private binary directory.  Call `guard.disarm()` once the
    // temp is no longer our responsibility.
    struct TmpGuard(Option<PathBuf>);
    impl TmpGuard {
        fn disarm(&mut self) {
            self.0 = None;
        }
    }
    impl Drop for TmpGuard {
        fn drop(&mut self) {
            if let Some(p) = self.0.take() {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    let mut guard = TmpGuard(Some(tmp_path.clone()));

    std::fs::copy(&source, &tmp_path).with_context(|| {
        format!(
            "seeding private daemon binary {} from {} (via temp {})",
            private_path.display(),
            source.display(),
            tmp_path.display()
        )
    })?;
    if let Ok(md) = std::fs::metadata(&source) {
        let _ = std::fs::set_permissions(&tmp_path, md.permissions());
    }
    validate_stale_reexec_candidate(&tmp_path)
        .map_err(|f| anyhow!(private_candidate_validation_message(&f)))?;

    // Test-only synchronization hook: STORES_TEST_SEED_RACE_DELAY_MS introduces
    // a sleep between the existence-shortcut and the hard_link so both
    // concurrent seeders are guaranteed to pass the shortcut before either
    // attempts the link.  Gated on debug_assertions so release builds compile
    // it out entirely — the env-var cannot leak into production release
    // binaries.
    #[cfg(debug_assertions)]
    {
        if let Ok(ms) = std::env::var("STORES_TEST_SEED_RACE_DELAY_MS") {
            if let Ok(n) = ms.parse::<u64>() {
                // Emit a greppable sentinel before the sleep+link so the
                // concurrent-seeders test can assert both seeders reached this
                // point (not just that the loser fired the AlreadyExists arm).
                eprintln!("stores::agents_run::seed_race: reached pre-link");
                if n > 0 {
                    std::thread::sleep(Duration::from_millis(n));
                }
            }
        }
    }

    // Use hard_link (not rename) as the install primitive so that concurrent
    // seeders are detectable.  On POSIX, rename(2) silently replaces the
    // destination if it already exists, making the AlreadyExists arm below dead
    // code on Linux/macOS.  hard_link(2) returns EEXIST when the destination
    // exists, so two concurrent seeders racing here will have one win (link
    // succeeds) and one lose (link returns AlreadyExists) — the race-handling
    // arm now actually fires.  Both tmp and dest are in the same directory
    // (same filesystem / mount point), so cross-device failures cannot occur.
    match std::fs::hard_link(&tmp_path, &private_path) {
        Ok(()) => {
            // We won the race; private_path now contains our validated copy.
            // Remove the temp first (guard still armed so drop retries on
            // failure), then disarm only on success.  If unlink fails the
            // guard remains armed and runs its own idempotent remove_file on
            // drop — that is the safety net for the failure path.
            std::fs::remove_file(&tmp_path).with_context(|| {
                format!("removing seed temp file {}", tmp_path.display())
            })?;
            guard.disarm();
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another concurrent daemon seeder won; discard our temp (guard
            // handles it on drop) and validate the winner before trusting it.
            // Unlink first while guard is still armed so drop retries on
            // failure; disarm only on success — the guard's idempotent
            // remove_file is the safety net when unlink fails.
            // Test-only sentinel (debug_assertions builds only): emit a
            // greppable line so the concurrent-seeders test can assert the
            // AlreadyExists arm actually fired.
            #[cfg(debug_assertions)]
            if std::env::var_os("STORES_TEST_SEED_RACE_DELAY_MS").is_some() {
                eprintln!("stores::agents_run::seed_race: loser observed AlreadyExists; validating winner");
            }
            std::fs::remove_file(&tmp_path).with_context(|| {
                format!("removing seed temp file (loser) {}", tmp_path.display())
            })?;
            guard.disarm();
            validate_stale_reexec_candidate(&private_path)
                .map_err(|f| anyhow!(private_candidate_validation_message(&f)))
                .with_context(|| {
                    format!(
                        "concurrent-seeded private daemon binary {} failed validation",
                        private_path.display()
                    )
                })?;
        }
        Err(e) => {
            // guard will clean up tmp_path on drop.
            return Err(e).with_context(|| {
                format!(
                    "linking seeded temp {} to {}",
                    tmp_path.display(),
                    private_path.display()
                )
            });
        }
    }

    let _ = provider.identity(&private_path)?;
    Ok(private_path)
}

fn private_candidate_validation_message(f: &CandidateValidationFailure) -> String {
    // Use the same rich format as candidate_validation_error_message so that
    // callers (tests and log consumers) see a consistent diagnostic shape:
    // path, size, command, exit_status, reason, stdout, stderr.
    candidate_validation_error_message(f)
}

pub fn resolve_launch_path_from_env() -> Result<PathBuf> {
    let argv0 = std::env::args()
        .next()
        .ok_or_else(|| anyhow!("argv[0] missing; cannot resolve daemon launch path"))?;
    resolve_launch_path(&argv0)
}

pub fn resolve_launch_path(argv0: &str) -> Result<PathBuf> {
    if argv0.contains('/') {
        return Ok(PathBuf::from(argv0));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(argv0);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Ok(PathBuf::from(argv0))
}

use crate::codegen::ddl::quote_ident;
use crate::flow::{
    decide, AgentEntry, AgentsYaml, BackoffKind, Decision, NotifyEvent, PoliciesYaml,
};

/// Base backoff quantum for retry rescheduling. Linear: `attempts * BASE`,
/// Exponential: `BASE * 2^(attempts-1)` (saturating).
const BASE_BACKOFF_SECS: u64 = 30;

fn daemon_binary_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn daemon_git_sha() -> &'static str {
    option_env!("VERGEN_GIT_SHA").unwrap_or("unknown")
}

/// Return true when `arg` is a bare secret flag (no `=value` embedded).
/// Covers: `--approve-token`, `--*-token`, `--*-secret`, `--*-key`, `--secret-*`, `--token`, `--key`.
fn is_secret_flag(arg: &str) -> bool {
    if !arg.starts_with("--") {
        return false;
    }
    let name = arg.trim_start_matches('-');
    // Exact matches
    if matches!(name, "approve-token" | "token" | "key") {
        return true;
    }
    // Prefix/suffix patterns
    name.starts_with("secret-")
        || name.ends_with("-token")
        || name.ends_with("-secret")
        || name.ends_with("-key")
}

/// Return true when `arg` is a secret flag in `--flag=value` form (value embedded).
fn is_secret_flag_with_value(arg: &str) -> bool {
    if let Some(eq) = arg.find('=') {
        is_secret_flag(&arg[..eq])
    } else {
        false
    }
}

fn filter_daemon_argv<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut filtered = Vec::new();
    let mut skip_next = false;
    for arg in args {
        let arg = arg.as_ref();
        if skip_next {
            // This arg is the value of the preceding secret flag — drop it.
            // But if it is itself a secret flag (adjacent secret flags), treat
            // it as a flag instead: drop it and arm another value-skip.
            skip_next = false;
            if is_secret_flag(arg) {
                // Adjacent secret flag: its value is next — arm skip again.
                skip_next = true;
            } else if is_secret_flag_with_value(arg) {
                // Adjacent `--flag=value` form; no next-skip needed.
            }
            // Either way: do not push to filtered.
            continue;
        }
        // `--flag=value` form: entire arg is secret, no next-skip needed.
        if is_secret_flag_with_value(arg) {
            continue;
        }
        // `--flag value` form: bare secret flag — drop flag and arm value-skip.
        if is_secret_flag(arg) {
            skip_next = true;
            continue;
        }
        // Legacy contains-check for non-standard embeddings (e.g. `prefix--secret-foo`).
        if arg.contains("--secret-") || arg.contains("--approve-token") {
            continue;
        }
        filtered.push(arg.to_string());
    }
    filtered
}

fn daemon_starts_table_exists(conn: &Connection) -> Result<bool> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='daemon_starts'",
            [],
            |r| r.get(0),
        )
        .context("checking daemon_starts table existence")?;
    Ok(exists > 0)
}

fn insert_daemon_startup<P: BinaryIdentityProvider>(
    conn: &Connection,
    args: &RunArgs,
    _daemon_epoch: &str,
    exe_guard: &DaemonExeGuard<P>,
) -> Result<()> {
    if !daemon_starts_table_exists(conn)? {
        return Ok(());
    }
    let started_at = crate::handlers::row::now_iso8601();
    let pid = i64::from(std::process::id());
    let binary_path = exe_guard.launch_path().display().to_string();
    let argv = serde_json::to_string(&filter_daemon_argv(std::env::args()))
        .context("serializing filtered daemon argv")?;
    let cwd = std::env::current_dir()
        .context("resolving daemon cwd")?
        .display()
        .to_string();
    // Unique pending placeholder: process-id + monotonic counter so two
    // concurrent daemon startups never collide on the UNIQUE constraint.
    // We update to the real D### derived from last_insert_rowid() before the
    // transaction ends, so the placeholder is never visible to readers.
    let seq = DAEMON_START_SEQ.fetch_add(1, Ordering::Relaxed);
    let pending = format!("__pending_{}_{}", std::process::id(), seq);
    conn.execute(
        "INSERT INTO daemon_starts \
         (display_id, status, created_at, updated_at, created_by, updated_by, \
          pid, started_at, binary_path, binary_version, git_sha, argv, log_file, cwd) \
         VALUES (?1, 'started', ?2, ?2, 'daemon', 'daemon', \
                 ?3, ?2, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            pending,
            started_at,
            pid,
            binary_path,
            daemon_binary_version(),
            daemon_git_sha(),
            argv,
            args.log_file.as_deref(),
            cwd,
        ],
    )
    .context("inserting daemon_starts audit row")?;
    let rowid = conn.last_insert_rowid();
    let display_id = format!("D{rowid:03}");
    conn.execute(
        "UPDATE daemon_starts SET display_id = ?1 WHERE id = ?2",
        rusqlite::params![display_id, rowid],
    )
    .context("updating daemon_starts display_id from rowid")?;
    Ok(())
}

/// Args parsed from the CLI.
pub struct RunArgs {
    pub poll_interval_ms: u64,
    pub detach: bool,
    pub log_file: Option<String>,
    /// Test/debug knob: stop the loop after this many poll iterations.
    pub max_iters: Option<usize>,
}

/// Process-wide shutdown flag; flipped by the SIGTERM handler. Public so
/// tests can flip it directly without sending a signal.
pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Per-process counter used to generate unique pending placeholders for
/// daemon_starts.display_id before the autoincrement rowid is known.
/// Monotonically increasing; combined with process ID to guarantee uniqueness
/// across concurrent processes sharing the same SQLite file.
static DAEMON_START_SEQ: AtomicU64 = AtomicU64::new(0);

/// Process-wide stale-binary flag. Set by the first stale detection inside
/// `poll_once_with_guard` so the fail-loud message fires exactly once even
/// when multiple auto-drive candidates are eligible in the same poll iteration.
pub static STALE_HALTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigterm(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn install_sigterm_handler() {
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_sigterm as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_sigterm as *const () as libc::sighandler_t,
        );
    }
}

/// Identity fields stored alongside the PID to detect PID reuse on stop.
///
/// Format (key=value, one per line, store-defined/opaque):
/// ```text
/// PID=12345
/// START_TIME_NS=1234567890   (Linux: /proc/self/stat field 22 in clock ticks;
///                             set to 0 on non-Linux)
/// EXE=/path/to/stores        (from /proc/self/exe or std::env::current_exe)
/// CWD=/path/to/project       (from std::env::current_dir; best-effort)
/// ```
///
/// This format is not user-facing; only `stores agents stop` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PidfileEntry {
    pub(crate) pid: i32,
    /// Linux: /proc/self/stat field 22 (clock ticks since boot).
    /// Zero on non-Linux (fallback to bare-PID semantics).
    pub(crate) start_time: u64,
    /// Absolute path of the stores executable. Empty string if unavailable.
    pub(crate) exe: String,
    /// Working directory at daemon start. Empty string if unavailable.
    pub(crate) cwd: String,
}

impl PidfileEntry {
    /// Build a `PidfileEntry` for the **current** process.
    fn for_current_process() -> Self {
        let pid = std::process::id() as i32;
        let start_time = read_self_start_time();
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Self { pid, start_time, exe, cwd }
    }

    /// Serialize to the on-disk key=value format.
    fn serialize(&self) -> String {
        format!(
            "PID={}\nSTART_TIME_NS={}\nEXE={}\nCWD={}\n",
            self.pid, self.start_time, self.exe, self.cwd
        )
    }

    /// Deserialize from the on-disk key=value format.
    /// Returns `Err` only if the PID field is missing/unparseable.
    fn deserialize(text: &str) -> Result<Self> {
        let mut pid: Option<i32> = None;
        let mut start_time: u64 = 0;
        let mut exe = String::new();
        let mut cwd = String::new();
        for line in text.lines() {
            if let Some(val) = line.strip_prefix("PID=") {
                pid = Some(
                    val.parse::<i32>()
                        .context("parsing PID field in agents pidfile")?,
                );
            } else if let Some(val) = line.strip_prefix("START_TIME_NS=") {
                start_time = val.parse::<u64>().unwrap_or(0);
            } else if let Some(val) = line.strip_prefix("EXE=") {
                exe = val.to_string();
            } else if let Some(val) = line.strip_prefix("CWD=") {
                cwd = val.to_string();
            }
        }
        let pid = pid.with_context(|| "PID field missing from agents pidfile")?;
        if pid <= 0 {
            bail!("agents daemon pid file contains non-positive pid: {pid}");
        }
        Ok(Self { pid, start_time, exe, cwd })
    }
}

/// Return the kernel's ticks-per-second (hz) via `sysconf(_SC_CLK_TCK)`.
/// Returns 0 on failure (which will cause callers to return 0 / skip conversion).
#[cfg(target_os = "linux")]
fn clk_tck() -> u64 {
    // SAFETY: sysconf is always safe to call with a valid name constant.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 { 0 } else { hz as u64 }
}

/// Convert raw `/proc/<pid>/stat` starttime clock ticks to nanoseconds.
/// Returns 0 if `hz` is 0 (prevents divide-by-zero; callers treat 0 as "unknown").
#[cfg(target_os = "linux")]
fn ticks_to_ns(ticks: u64, hz: u64) -> u64 {
    if hz == 0 { return 0; }
    ((ticks as u128) * 1_000_000_000u128 / (hz as u128)).min(u64::MAX as u128) as u64
}

/// Read `/proc/self/stat` field 22 (start_time) on Linux, converted to nanoseconds.
/// Returns 0 on non-Linux or on any read/parse failure.
#[cfg(target_os = "linux")]
fn read_self_start_time() -> u64 {
    read_proc_start_time(std::process::id() as i32)
}

#[cfg(not(target_os = "linux"))]
fn read_self_start_time() -> u64 {
    0
}

/// Read `/proc/<pid>/stat` field 22 (start_time) for an arbitrary pid on Linux,
/// converted to nanoseconds via `sysconf(_SC_CLK_TCK)`.
/// Returns 0 on any failure (including non-Linux stub).
#[cfg(target_os = "linux")]
pub(crate) fn read_proc_start_time(pid: i32) -> u64 {
    if pid <= 0 {
        return 0;
    }
    let contents = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    // /proc/<pid>/stat: "pid (comm) state ..." — comm can contain spaces/parens.
    // Field 22 (1-indexed) is start_time; skip past the closing ')' first.
    let after_comm = match contents.rfind(')') {
        Some(i) => &contents[i + 1..],
        None => return 0,
    };
    // /proc/pid/stat field numbers (1-indexed from the start of the line):
    //   1: pid  2: (comm)  3: state  4: ppid  5: pgroup  6: session
    //   7: tty_nr  8: tpgid  9: flags  10: minflt  11: cminflt  12: majflt
    //  13: cmajflt  14: utime  15: stime  16: cutime  17: cstime  18: priority
    //  19: nice  20: num_threads  21: itrealvalue  22: starttime
    //
    // After stripping `pid (comm)`, the remaining fields start at field 3 (state).
    // starttime (field 22) is at 0-based index (22 - 3) = 19 in the remaining tokens.
    let mut fields = after_comm.split_whitespace();
    // Skip 19 tokens (fields 3–21) to land on field 22 (starttime).
    for _ in 0..19 {
        if fields.next().is_none() {
            return 0;
        }
    }
    let ticks = fields.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    ticks_to_ns(ticks, clk_tck())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn read_proc_start_time(_pid: i32) -> u64 {
    0
}

/// Read and parse the pidfile, returning a `PidfileEntry`.
///
/// Supports both the legacy bare-PID format (`"12345\n"`) and the current
/// key=value format for backward compatibility during upgrades.
pub(crate) fn read_pidfile(path: &Path) -> Result<PidfileEntry> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading agents daemon pid file {}", path.display()))?;
    // Legacy bare-PID format: the entire trimmed content is a number.
    if text.trim().chars().all(|c| c.is_ascii_digit() || c == '-') {
        let pid: i32 = text
            .trim()
            .parse()
            .with_context(|| format!("parsing agents daemon pid file {}", path.display()))?;
        if pid <= 0 {
            bail!(
                "agents daemon pid file {} contains non-positive pid: {pid}",
                path.display()
            );
        }
        // Legacy format: start_time=0 (no identity check).
        return Ok(PidfileEntry { pid, start_time: 0, exe: String::new(), cwd: String::new() });
    }
    PidfileEntry::deserialize(&text)
        .with_context(|| format!("parsing agents daemon pid file {}", path.display()))
}

/// Atomically claim the pidfile slot before forking.
///
/// Uses `OpenOptions::create_new(true)` (O_CREAT|O_EXCL semantics) to ensure
/// only one concurrent caller wins the race — the OS makes this atomic. If the
/// file already exists we inspect the existing PID:
///
/// - Alive  → refuse with a clear error (daemon already running).
/// - Dead/zombie → remove the stale file and retry the atomic create once.
/// - Invalid content but file present → bail with a parse error.
///
/// This replaces the previous check-then-write pattern that allowed two
/// concurrent `agents run --detach` callers to both pass the live-PID check
/// and later overwrite each other's pidfile.
///
/// The file is written with a placeholder "0\n" so that downstream code sees a
/// valid (if not-yet-meaningful) PID; `PidfileGuard::write_current` overwrites
/// it with the real daemon PID immediately after fork.
fn prepare_detached_pidfile(path: &Path) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    // Attempt 1: atomic create. On success we hold the slot.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            // Placeholder; PidfileGuard::write_current overwrites with real PID.
            f.write_all(b"0\n")
                .with_context(|| format!("writing placeholder to {}", path.display()))?;
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another caller (or a stale file) holds the slot. Inspect it.
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("creating agents daemon pid file {}", path.display()));
        }
    }

    // The file exists. Read the existing PID.
    // A zombie PID is treated as "not live" — it has exited even though
    // kill(pid, 0) returns 0. Use pid_is_zombie to detect that case.
    match read_pidfile(path) {
        Ok(entry) if pid_is_alive(entry.pid) && !pid_is_zombie(entry.pid) => {
            bail!(
                "agents daemon already running for this project: live pid {} in {}",
                entry.pid,
                path.display()
            );
        }
        Ok(entry) => {
            // Dead or zombie — stale pidfile.
            eprintln!(
                "warning: removing stale agents daemon pid file {} (pid {} is not live)",
                path.display(),
                entry.pid
            );
            std::fs::remove_file(path).with_context(|| {
                format!("removing stale agents daemon pid file {}", path.display())
            })?;
        }
        Err(e) => {
            bail!("invalid agents daemon pid file {}: {:#}", path.display(), e);
        }
    }

    // Attempt 2: retry atomic create after stale removal.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            f.write_all(b"0\n")
                .with_context(|| format!("writing placeholder to {}", path.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another concurrent caller won the retry race — they have a live daemon.
            let pid = read_pidfile(path).map(|e| e.pid).unwrap_or(-1);
            bail!(
                "agents daemon already running for this project: live pid {pid} in {} (lost atomic claim retry)",
                path.display()
            );
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "creating agents daemon pid file {} (retry after stale removal)",
                    path.display()
                )
            });
        }
    }

    Ok(())
}

struct PidfileGuard {
    path: PathBuf,
    pid: i32,
}

impl PidfileGuard {
    fn write_current(path: PathBuf) -> Result<Self> {
        let entry = PidfileEntry::for_current_process();
        let pid = entry.pid;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, entry.serialize())
            .with_context(|| format!("writing agents daemon pid file {}", path.display()))?;
        Ok(Self { path, pid })
    }
}

impl Drop for PidfileGuard {
    fn drop(&mut self) {
        if let Ok(entry) = read_pidfile(&self.path) {
            if entry.pid == self.pid {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

fn current_process_owns_pidfile(path: &Path) -> bool {
    read_pidfile(path)
        .map(|entry| entry.pid == std::process::id() as i32)
        .unwrap_or(false)
}

fn stale_reexec_attempt_line(path: &Path) -> String {
    format!(
        "daemon binary stale; reexecing into {} (was version {})",
        path.display(),
        crate::version::build_identity()
    )
}

fn log_stale_reexec_attempt_once<P: BinaryIdentityProvider>(guard: &DaemonExeGuard<P>) {
    if !STALE_HALTED.swap(true, Ordering::SeqCst) {
        eprintln!("{}", stale_reexec_attempt_line(guard.launch_path()));
    }
}

fn cstring_arg(arg: &std::ffi::OsStr) -> Result<CString> {
    CString::new(arg.as_bytes()).context("daemon reexec argv contains interior NUL")
}

const STALE_REEXEC_VALIDATION_TIMEOUT: Duration = Duration::from_millis(1500);
const STALE_REEXEC_OUTPUT_LIMIT: usize = 512;

#[derive(Debug)]
struct CandidateValidationFailure {
    path: PathBuf,
    size: Option<u64>,
    command: String,
    /// Stringified exit status for diagnostics. Numeric codes are rendered as
    /// their integer value; synthetic cases use sentinel strings:
    ///   - `"timeout"` — process exceeded the validation deadline
    ///   - `"spawn_failed"` — `spawn()` / `wait()` I/O error; no OS exit code
    exit_status: Option<String>,
    reason: String,
    stdout: String,
    stderr: String,
}

fn bounded_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out: String = text.chars().take(STALE_REEXEC_OUTPUT_LIMIT).collect();
    if text.chars().count() > STALE_REEXEC_OUTPUT_LIMIT {
        out.push_str("...[truncated]");
    }
    out.replace('\n', "\\n")
}

pub(crate) fn validate_stores_binary_candidate(path: &Path) -> Result<()> {
    validate_stale_reexec_candidate(path)
        .map_err(|f| anyhow!(candidate_validation_error_message(&f)))
}

#[allow(clippy::result_large_err)]
fn validate_stale_reexec_candidate(
    path: &Path,
) -> std::result::Result<(), CandidateValidationFailure> {
    let size = std::fs::metadata(path).map(|m| m.len()).ok();
    let command = format!("{} --help", path.display());
    let mut cmd = std::process::Command::new(path);
    cmd.arg("--help")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let mut child = cmd.spawn().map_err(|e| CandidateValidationFailure {
        path: path.to_path_buf(),
        size,
        command: command.clone(),
        exit_status: Some("spawn_failed".to_string()),
        reason: format!("spawn error: {e}"),
        stdout: String::new(),
        stderr: String::new(),
    })?;

    let deadline = Instant::now() + STALE_REEXEC_VALIDATION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|e| CandidateValidationFailure {
                        path: path.to_path_buf(),
                        size,
                        command: command.clone(),
                        exit_status: Some("spawn_failed".to_string()),
                        reason: format!("collect output error: {e}"),
                        stdout: String::new(),
                        stderr: String::new(),
                    })?;
                let stdout = bounded_output(&output.stdout);
                let stderr = bounded_output(&output.stderr);
                let exit_status_str = Some(
                    output
                        .status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| output.status.to_string()),
                );
                if !output.status.success() {
                    return Err(CandidateValidationFailure {
                        path: path.to_path_buf(),
                        size,
                        command,
                        exit_status: exit_status_str,
                        reason: format!("non-success exit status: {}", output.status),
                        stdout,
                        stderr,
                    });
                }
                if output.stdout.is_empty() {
                    return Err(CandidateValidationFailure {
                        path: path.to_path_buf(),
                        size,
                        command,
                        exit_status: exit_status_str,
                        reason: "empty stdout from --help".to_string(),
                        stdout,
                        stderr,
                    });
                }
                let stdout_text = String::from_utf8_lossy(&output.stdout).to_lowercase();
                if !stdout_text.contains("schema-driven store framework") {
                    return Err(CandidateValidationFailure {
                        path: path.to_path_buf(),
                        size,
                        command,
                        exit_status: exit_status_str,
                        reason: "missing stores marker in --help stdout".to_string(),
                        stdout,
                        stderr,
                    });
                }
                return Ok(());
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    #[cfg(unix)]
                    unsafe {
                        let _ = libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
                    }
                    #[cfg(not(unix))]
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CandidateValidationFailure {
                        path: path.to_path_buf(),
                        size,
                        command,
                        exit_status: Some("timeout".to_string()),
                        reason: format!(
                            "timeout after {}ms",
                            STALE_REEXEC_VALIDATION_TIMEOUT.as_millis()
                        ),
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CandidateValidationFailure {
                    path: path.to_path_buf(),
                    size,
                    command,
                    exit_status: Some("spawn_failed".to_string()),
                    reason: format!("wait error: {e}"),
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
        }
    }
}

fn candidate_validation_error_message(f: &CandidateValidationFailure) -> String {
    format!(
        "candidate stores binary failed validation: path={} size={} command='{}' exit_status={} reason={} stdout='{}' stderr='{}'",
        f.path.display(),
        f.size.map(|s| s.to_string()).unwrap_or_else(|| "unknown".to_string()),
        f.command,
        f.exit_status.as_deref().unwrap_or("unknown"),
        f.reason,
        f.stdout,
        f.stderr
    )
}

fn log_candidate_validation_failure(f: &CandidateValidationFailure) {
    eprintln!(
        "candidate stores binary failed validation: path={} size={} command='{}' exit_status={} reason={} stdout='{}' stderr='{}'; {}",
        f.path.display(),
        f.size.map(|s| s.to_string()).unwrap_or_else(|| "unknown".to_string()),
        f.command,
        f.exit_status.as_deref().unwrap_or("unknown"),
        f.reason,
        f.stdout,
        f.stderr,
        STALE_DAEMON_MESSAGE
    );
}

fn handle_stale_daemon_reexec<P: BinaryIdentityProvider>(
    guard: &DaemonExeGuard<P>,
    argv: &[OsString],
) -> Result<()> {
    log_stale_reexec_attempt_once(guard);
    if let Err(failure) = validate_stale_reexec_candidate(guard.launch_path()) {
        log_candidate_validation_failure(&failure);
        bail!(STALE_DAEMON_MESSAGE);
    }
    let path_c = match cstring_arg(guard.launch_path().as_os_str()) {
        Ok(path) => path,
        Err(e) => {
            eprintln!(
                "daemon binary stale reexec fallback: exec setup failed: {:#}; {}",
                e, STALE_DAEMON_MESSAGE
            );
            bail!(STALE_DAEMON_MESSAGE);
        }
    };
    let argv_c: Vec<CString> = match argv.iter().map(|a| cstring_arg(a.as_os_str())).collect() {
        Ok(args) => args,
        Err(e) => {
            eprintln!(
                "daemon binary stale reexec fallback: exec setup failed: {:#}; {}",
                e, STALE_DAEMON_MESSAGE
            );
            bail!(STALE_DAEMON_MESSAGE);
        }
    };
    let mut argv_ptrs: Vec<*const libc::c_char> = argv_c.iter().map(|a| a.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());
    unsafe {
        libc::execv(path_c.as_ptr(), argv_ptrs.as_ptr());
    }
    let err = std::io::Error::last_os_error();
    eprintln!(
        "daemon binary stale reexec fallback: execv failed for {}: {}; {}",
        guard.launch_path().display(),
        err,
        STALE_DAEMON_MESSAGE
    );
    bail!(STALE_DAEMON_MESSAGE);
}

pub fn run_daemon(args: RunArgs) -> Result<()> {
    let stores_dir = crate::paths::stores_dir()?;

    // Load agents.yaml — fail-loud on parse error; missing file → empty registry.
    let agents_path = stores_dir.join("agents.yaml");
    let agents = if agents_path.exists() {
        crate::flow::agents_yaml::load_from_path(&agents_path)
            .context("loading .stores/agents.yaml")?
    } else {
        AgentsYaml::default_empty()
    };

    // Load policies.yaml — fail-loud on parse error; missing file → empty.
    let policies_path = stores_dir.join("policies.yaml");
    let policies = if policies_path.exists() {
        let bytes = std::fs::read_to_string(&policies_path)
            .with_context(|| format!("reading {}", policies_path.display()))?;
        crate::flow::policies_yaml::PoliciesYaml::from_yaml(&bytes)
            .context("parsing .stores/policies.yaml")?
    } else {
        PoliciesYaml {
            hash: String::new(),
            policies: vec![],
        }
    };

    let config_path = stores_dir.join("config.yaml");

    let pidfile = crate::paths::agents_pid_path()?;
    let mut pidfile_guard = None;
    if args.detach {
        prepare_detached_pidfile(&pidfile)?;
        detach_process(&args.log_file)?;
    }

    install_sigterm_handler();
    if args.detach {
        // Write before the initial stale-binary check: exec preserves this PID,
        // and the reexeced daemon can reacquire ownership of the same pidfile.
        pidfile_guard = Some(PidfileGuard::write_current(pidfile.clone())?);
    }

    // Collect argv and strip --detach (and --detach=...) before reexec so
    // that a self-reexecing daemon does not attempt to re-daemonize — it is
    // already detached. --invoker, --log-file, --meta, and all other flags are
    // preserved. (MEDIUM codex fix: Pi Option A.)
    let daemon_argv: Vec<OsString> = std::env::args_os()
        .filter(|a| {
            let s = a.to_string_lossy();
            s != "--detach" && !s.starts_with("--detach=")
        })
        .collect();

    // Construct the exe guard and perform an IMMEDIATE stale check BEFORE
    // opening the DB or running any migrations / seeds / sweeps. If the binary
    // on disk already differs from the one we were launched from, reexec now so
    // no stale code touches the substrate. (HIGH codex fix.)
    let exe_guard =
        DaemonExeGuard::from_process().context("constructing daemon executable guard")?;
    if exe_guard.check_stale()?.is_some() {
        handle_stale_daemon_reexec(&exe_guard, &daemon_argv)?;
    }

    if pidfile_guard.is_none() && current_process_owns_pidfile(&pidfile) {
        pidfile_guard = Some(PidfileGuard::write_current(pidfile)?);
    }
    let _pidfile_guard = pidfile_guard;

    // T040: capture the daemon process's start timestamp once. The watchdog's
    // silent-zombie scan uses this to skip rows whose dispatch_lock was
    // claimed by a prior daemon lifetime — those locks are not THIS daemon's
    // recovery target. Tests / debugging can override via STORES_DAEMON_EPOCH
    // (e.g. "1970-01-01T00:00:00Z" to disable the gate's effect).
    let daemon_epoch = std::env::var("STORES_DAEMON_EPOCH")
        .unwrap_or_else(|_| crate::handlers::row::now_iso8601());

    let db_path = crate::paths::db_path()?;
    let conn = crate::db::open(&db_path)?;
    insert_daemon_startup(&conn, &args, &daemon_epoch, &exe_guard)
        .context("recording daemon startup audit row")?;
    let claimer = format!("daemon-{}", std::process::id());

    // L134 / T050 Phase 1: ensure typed dispatch_locks columns + backfill
    // legacy rows BEFORE seed_starting_line so any new rows the seeder writes
    // see the typed schema (db::open also calls these for CLI flows; daemon
    // calls explicitly here for clarity / startup ordering).
    ensure_dispatch_locks_typed(&conn).context("L134: ensure typed dispatch_locks columns")?;
    backfill_legacy_locks(&conn).context("L134: backfill legacy dispatch_locks rows")?;

    // L116: snapshot the highest transition_history.id BEFORE seeding so the
    // seeder cannot claim transitions that fire after the daemon started
    // (e.g. between two `agents run --once` calls). Without this bound, a
    // user `confirm` between polls would race the seeder and lose its row to
    // skip-historical, silently swallowing the new transition.
    let max_th_id = snapshot_max_transition_id(&conn)
        .context("snapshotting MAX(transition_history.id) for starting-line bound")?;
    let seeded = seed_starting_line(&conn, &agents, max_th_id)
        .context("seeding starting-line dispatch_locks")?;
    eprintln!(
        "[daemon] seeded {} starting-line dispatch_locks (bound: th.id <= {})",
        seeded, max_th_id
    );

    // T048: startup-sweep — backfill auto_resolve for historically-shipped tasks
    // (status='schema_migrated' with non-empty linked_observations) whose linked
    // obs are still un-resolved. Idempotent; logs `[startup-sweep] resolved N
    // linked obs` before the first poll iteration.
    {
        let sweep_ctx = crate::flow::builtins::DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &config_path,
            policies_hash: &policies.hash,
        };
        if let Err(e) = crate::flow::builtins::auto_resolve_observation::startup_sweep(&sweep_ctx) {
            eprintln!("[startup-sweep] error: {:#}", e);
        }
    }

    let mut iter = 0usize;
    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            eprintln!(
                "[daemon] shutdown received, exiting after {} iterations",
                iter
            );
            break;
        }
        if exe_guard.check_stale()?.is_some() {
            handle_stale_daemon_reexec(&exe_guard, &daemon_argv)?;
        }
        match poll_once_with_guard(
            &conn,
            &agents,
            &policies,
            &config_path,
            &claimer,
            &daemon_epoch,
            Some(&exe_guard),
        ) {
            Ok(n) if n > 0 => eprintln!("[daemon] dispatched {} job(s) in iteration {}", n, iter),
            Ok(_) => {}
            Err(e) => {
                // Stale-binary errors are emitted inside poll_once_with_guard
                // (exactly once via STALE_HALTED); propagate as a hard bail so
                // the daemon exits rather than looping on a stale binary.
                if e.to_string() == STALE_DAEMON_MESSAGE {
                    handle_stale_daemon_reexec(&exe_guard, &daemon_argv)?;
                }
                eprintln!("[daemon] poll error: {}", e);
            }
        }
        iter += 1;
        if let Some(max) = args.max_iters {
            if iter >= max {
                break;
            }
        }
        sleep_interruptible(args.poll_interval_ms);
    }
    Ok(())
}

/// Sleep `ms` milliseconds in 50ms slices, returning early if SHUTDOWN is set.
fn sleep_interruptible(ms: u64) {
    let mut remaining = ms;
    while remaining > 0 && !SHUTDOWN.load(Ordering::SeqCst) {
        let chunk = remaining.min(50);
        std::thread::sleep(Duration::from_millis(chunk));
        remaining = remaining.saturating_sub(chunk);
    }
}

/// One poll iteration: scan `transition_history` for entries that match each
/// agent's subscriptions, gate via the policy layer, claim atomically, and
/// dispatch. Returns the number of dispatches performed (Halt-policied rows
/// do NOT count).
pub fn poll_once(
    conn: &Connection,
    agents: &AgentsYaml,
    policies: &PoliciesYaml,
    config_path: &Path,
    claimer: &str,
    daemon_epoch: &str,
) -> Result<usize> {
    poll_once_with_guard::<FsBinaryIdentityProvider>(
        conn,
        agents,
        policies,
        config_path,
        claimer,
        daemon_epoch,
        None,
    )
}

pub fn poll_once_with_guard<P: BinaryIdentityProvider>(
    conn: &Connection,
    agents: &AgentsYaml,
    policies: &PoliciesYaml,
    config_path: &Path,
    claimer: &str,
    daemon_epoch: &str,
    exe_guard: Option<&DaemonExeGuard<P>>,
) -> Result<usize> {
    let mut dispatched = 0;
    for agent in &agents.agents {
        for sub in &agent.subscribes_to {
            let mut stmt = conn.prepare(
                "SELECT id, row_id, display_id FROM transition_history \
                 WHERE store = ?1 AND from_status = ?2 AND to_status = ?3 \
                 ORDER BY id ASC",
            )?;
            let rows: Vec<(i64, i64, String)> = stmt
                .query_map(
                    rusqlite::params![&sub.store, &sub.transition.from, &sub.transition.to],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?
                .filter_map(|r| r.ok())
                .collect();
            for (transition_id, row_id, display_id) in rows {
                // Policy gate: read the row as JSON, run decide().
                // On Halt: ntfy + skip (do NOT claim or retry).
                let row_json = read_row_as_json(conn, &sub.store, row_id)
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                let decision = decide(
                    policies,
                    &sub.store,
                    &sub.transition.from,
                    &sub.transition.to,
                    &row_json,
                )
                .unwrap_or(Decision::Allow {
                    policy_id: "default-allow".into(),
                });
                let policy_id = match &decision {
                    Decision::Allow { policy_id } => policy_id.clone(),
                    Decision::Halt { policy_id } => {
                        let event = NotifyEvent {
                            row_id: display_id.clone(),
                            transition_attempted: format!(
                                "{}: {}→{}",
                                sub.store, sub.transition.from, sub.transition.to
                            ),
                            policy_id_or_actor_halt: policy_id.clone(),
                            summary: format!(
                                "policy '{}' halted dispatch to agent '{}'",
                                policy_id, agent.name
                            ),
                        };
                        let _ = crate::flow::notify_with_path(config_path, event);
                        continue;
                    }
                };

                // Per-subscription predicate gate (T022 P2). Runs AFTER the
                // policy decide() halt-check so existing halt+ntfy semantics
                // are preserved; runs BEFORE try_claim so a false predicate
                // costs no claim and no ntfy.
                if let Some(pred) = &sub.predicate {
                    match crate::flow::predicate::eval(pred, &row_json) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(e) => {
                            eprintln!(
                                "[daemon] predicate eval error for agent '{}' on {}/{}: {}",
                                agent.name, sub.store, display_id, e
                            );
                            continue;
                        }
                    }
                }

                // Pre-claim cap check for builtin:auto-drive (T022 P4 / Task
                // 4.5). The `drive.max_parallel` config gates concurrent
                // drives BEFORE we burn a claim; otherwise a row would be
                // claimed-and-skipped, which would prevent retry on the next
                // poll. Only the auto-drive builtin is special-cased.
                //
                // The stale check here is an early-out optimization (avoids
                // burning a claim on a stale binary). The load-bearing check
                // is the tight pre-spawn guard below (MAJOR 1 fix). Dedup via
                // STALE_HALTED ensures only one log line fires even when
                // multiple candidates match in the same poll.
                if agent.command == "builtin:auto-drive" {
                    if let Some(guard) = exe_guard {
                        if guard.check_stale()?.is_some() {
                            log_stale_reexec_attempt_once(guard);
                            return Err(anyhow!(STALE_DAEMON_MESSAGE));
                        }
                    }
                    let cap = crate::flow::config::resolve_drive_max_parallel(config_path);
                    let now = crate::handlers::row::now_iso8601();
                    let occupied = crate::flow::engine_runner::count_active_auto_drive_capacity(
                        conn,
                        &now,
                        agent.claim_window_secs,
                    )
                    .unwrap_or(0);
                    if occupied >= cap as usize {
                        continue;
                    }
                }
                if agent.command == "builtin:external-review"
                    && !crate::flow::builtins::external_review::cap_allows_or_log(
                        conn,
                        config_path,
                        &display_id,
                    )?
                {
                    continue;
                }

                let postcondition_id = agent
                    .command
                    .strip_prefix("builtin:")
                    .and_then(crate::flow::builtins::postcondition_for_builtin);
                let postcondition_args =
                    postcondition_id.map(|id| postcondition_args_for(id, &sub.store, &display_id));
                let claimed = try_claim(
                    conn,
                    &sub.store,
                    row_id,
                    &display_id,
                    &agent.name,
                    transition_id,
                    claimer,
                    daemon_epoch,
                    "try_claim",
                    postcondition_id,
                    postcondition_args.as_ref(),
                )?;
                if !claimed {
                    continue;
                }
                // Tight pre-spawn stale guard (MAJOR 1 / MAJOR 2 fix): check
                // immediately before run_dispatch so no work (claim, postcond
                // computation) can slip in between the guard and the spawn.
                // STALE_HALTED deduplicates the log line across candidates.
                if agent.command == "builtin:auto-drive" {
                    if let Some(guard) = exe_guard {
                        if guard.check_stale()?.is_some() {
                            log_stale_reexec_attempt_once(guard);
                            return Err(anyhow!(STALE_DAEMON_MESSAGE));
                        }
                    }
                }
                let exit_code = run_dispatch(
                    conn,
                    agents,
                    config_path,
                    agent,
                    &sub.store,
                    row_id,
                    &display_id,
                    &sub.transition.from,
                    &sub.transition.to,
                    &policy_id,
                    &policies.hash,
                    &row_json,
                );
                let (terminal_reason, mut status_str, code) =
                    terminal_from_dispatch_result(exit_code);
                if code != 0 {
                    status_str = enrich_investigator_failure_status(
                        conn,
                        &sub.store,
                        &display_id,
                        agent,
                        &status_str,
                    );
                }
                if code != 0 {
                    if let Some(until) = task_rate_limit_until(conn, &sub.store, row_id)? {
                        mark_claim_rate_limit_cooldown(
                            conn,
                            &sub.store,
                            row_id,
                            &display_id,
                            agent,
                            &until,
                            &status_str,
                        )?;
                    } else {
                        let _ = mark_claim_finished_typed(
                            conn,
                            &sub.store,
                            row_id,
                            &display_id,
                            agent,
                            &terminal_reason,
                            &status_str,
                        );
                    }
                } else if agent.name == "auto-drive"
                    && terminal_reason == "ok"
                    && crate::flow::builtins::auto_drive::has_pending_auto_drive_work(
                        conn,
                        &display_id,
                    )
                    .unwrap_or(false)
                {
                    let _ = mark_auto_drive_pending_handoff(
                        conn,
                        &sub.store,
                        row_id,
                        &display_id,
                        agent,
                    );
                } else {
                    let _ = mark_claim_finished_typed(
                        conn,
                        &sub.store,
                        row_id,
                        &display_id,
                        agent,
                        &terminal_reason,
                        &status_str,
                    );
                }
                if code != 0 {
                    route_failure_to_deploy_blocked(
                        conn,
                        &sub.store,
                        &display_id,
                        &agent.name,
                        &status_str,
                        &policies.hash,
                        &sub.transition.to,
                    );
                }
                dispatched += 1;
            }
        }
    }
    // T041: retry-on-failure pass. Re-dispatch failed dispatch_locks rows
    // up to retry_policy.max_attempts with the configured backoff. The lock
    // already exists (try_claim was won on the first attempt), so we do NOT
    // call try_claim here. Auto-drive cap-check is intentionally skipped on
    // the retry path: the lock is already taken; gating it again would mean
    // a transient flake mid-cap permanently strands the row.
    for agent in &agents.agents {
        let candidates = match find_retryable_locks(conn, agent) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[daemon] find_retryable_locks for '{}': {}", agent.name, e);
                continue;
            }
        };
        for c in candidates {
            // Atomic CAS claim — closes the multi-daemon race where two
            // daemons would otherwise both dispatch the same retry candidate.
            // If another daemon claimed it first, our UPDATE affects 0 rows
            // and we skip silently.
            match claim_for_retry(conn, &c, &agent.name) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    eprintln!(
                        "[daemon] claim_for_retry failed for '{}'/{}: {}",
                        agent.name, c.display_id, e
                    );
                    continue;
                }
            }
            let row_json = read_row_as_json(conn, &c.store, c.row_id)
                .unwrap_or(Value::Object(serde_json::Map::new()));
            let decision = decide(policies, &c.store, &c.from_status, &c.to_status, &row_json)
                .unwrap_or(Decision::Allow {
                    policy_id: "default-allow".into(),
                });
            let policy_id = match &decision {
                Decision::Allow { policy_id } => policy_id.clone(),
                Decision::Halt { policy_id } => {
                    let event = NotifyEvent {
                        row_id: c.display_id.clone(),
                        transition_attempted: format!(
                            "{}: {}→{}",
                            c.store, c.from_status, c.to_status
                        ),
                        policy_id_or_actor_halt: policy_id.clone(),
                        summary: format!(
                            "policy '{}' halted retry-dispatch to agent '{}'",
                            policy_id, agent.name
                        ),
                    };
                    let _ = crate::flow::notify_with_path(config_path, event);
                    // Park last_status='halted:<policy>' so future polls'
                    // find_retryable_locks (which filters to error/exit
                    // statuses) excludes this row — closes the storm where
                    // every poll re-emits the same Halt notification.
                    if let Err(e) = mark_retry_halted(conn, &c, &agent.name, policy_id) {
                        eprintln!(
                            "[daemon] mark_retry_halted failed for '{}'/{}: {}",
                            agent.name, c.display_id, e
                        );
                    }
                    continue;
                }
            };
            let sub_match = agent.subscribes_to.iter().find(|s| {
                s.store == c.store
                    && s.transition.from == c.from_status
                    && s.transition.to == c.to_status
            });
            if let Some(sub) = sub_match {
                if let Some(pred) = &sub.predicate {
                    match crate::flow::predicate::eval(pred, &row_json) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(e) => {
                            eprintln!(
                                "[daemon] predicate eval error (retry) agent '{}' on {}/{}: {}",
                                agent.name, c.store, c.display_id, e
                            );
                            continue;
                        }
                    }
                }
            }
            // T049: only NOW clear finished_at for auto-drive, after all retry
            // gates (decision halt, predicate match) have passed. This keeps
            // the lock open during the spawn-to-first-submit window so T040
            // watchdog catches a retry-spawned drive that dies pre-submit; a
            // gated-out retry leaves the lock with finished_at intact (no
            // orphan).
            if agent.name == "auto-drive" {
                if let Err(e) = open_auto_drive_retry_lock(conn, &c) {
                    eprintln!(
                        "[daemon] open_auto_drive_retry_lock failed for '{}'/{}: {}",
                        agent.name, c.display_id, e
                    );
                }
            }
            // Tight pre-spawn stale guard (retry path) — same pattern as
            // the forward-dispatch path above. STALE_HALTED deduplicates.
            if agent.command == "builtin:auto-drive" {
                if let Some(guard) = exe_guard {
                    if guard.check_stale()?.is_some() {
                        log_stale_reexec_attempt_once(guard);
                        return Err(anyhow!(STALE_DAEMON_MESSAGE));
                    }
                }
            }
            let exit_code = run_dispatch(
                conn,
                agents,
                config_path,
                agent,
                &c.store,
                c.row_id,
                &c.display_id,
                &c.from_status,
                &c.to_status,
                &policy_id,
                &policies.hash,
                &row_json,
            );
            let (terminal_reason, mut status_str, code) = terminal_from_dispatch_result(exit_code);
            if code != 0 {
                status_str = enrich_investigator_failure_status(
                    conn,
                    &c.store,
                    &c.display_id,
                    agent,
                    &status_str,
                );
            }
            if code != 0 {
                if let Some(until) = task_rate_limit_until(conn, &c.store, c.row_id)? {
                    mark_claim_rate_limit_cooldown(
                        conn,
                        &c.store,
                        c.row_id,
                        &c.display_id,
                        agent,
                        &until,
                        &status_str,
                    )?;
                } else if agent.name == "auto-drive"
                    && terminal_reason == "ok"
                    && crate::flow::builtins::auto_drive::has_pending_auto_drive_work(
                        conn,
                        &c.display_id,
                    )
                    .unwrap_or(false)
                {
                    let _ = mark_auto_drive_pending_handoff(conn, &c.store, c.row_id, &c.display_id, agent);
                } else {
                    let _ = mark_claim_finished_typed(
                        conn,
                        &c.store,
                        c.row_id,
                        &c.display_id,
                        agent,
                        &terminal_reason,
                        &status_str,
                    );
                }
            } else if agent.name == "auto-drive"
                && terminal_reason == "ok"
                && crate::flow::builtins::auto_drive::has_pending_auto_drive_work(
                    conn,
                    &c.display_id,
                )
                .unwrap_or(false)
            {
                let _ = mark_auto_drive_pending_handoff(conn, &c.store, c.row_id, &c.display_id, agent);
            } else {
                let _ = mark_claim_finished_typed(
                    conn,
                    &c.store,
                    c.row_id,
                    &c.display_id,
                    agent,
                    &terminal_reason,
                    &status_str,
                );
            }
            if code != 0 {
                route_failure_to_deploy_blocked(
                    conn,
                    &c.store,
                    &c.display_id,
                    &agent.name,
                    &status_str,
                    &policies.hash,
                    &c.to_status,
                );
            }
            let _ = c.attempts;
            let _ = c.transition_id;
            dispatched += 1;
        }
    }

    // T022 P5: drive watchdog sweep — reconcile dispatch_locks for `auto-drive`
    // whose grandchild PID is no longer alive. Errors are logged, not fatal.
    if let Err(e) = crate::flow::builtins::auto_drive::sweep_drive_watchdog(
        conn,
        agents,
        config_path,
        &policies.hash,
        daemon_epoch,
    ) {
        eprintln!("[daemon] drive watchdog sweep error: {}", e);
    }

    // Panics inside the engine-runner iteration (e.g. in classification, DDL
    // query, or heartbeat write) must not crash the daemon's main poll loop.
    // Catch all panics here and log without payload content (payload may
    // contain sensitive row data), then continue.
    let iter_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_engine_runner_iteration(conn, agents, config_path, &policies.hash, dispatched as i64)
    }));
    match iter_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            eprintln!("[engine-runner] actionability loop error: {}", e);
        }
        Err(_payload) => {
            eprintln!("[engine-runner] iteration panicked; daemon continuing");
        }
    }
    match crate::flow::builtins::external_review::visible_status_rows(conn) {
        Ok(rows) => {
            for row in rows {
                eprintln!("[daemon] {row}");
            }
        }
        Err(e) => eprintln!("[daemon] external-review status scan error: {}", e),
    }
    Ok(dispatched)
}

fn run_engine_runner_iteration(
    conn: &Connection,
    agents: &AgentsYaml,
    config_path: &Path,
    policies_hash: &str,
    base_dispatched: i64,
) -> Result<()> {
    for table in ["tasks", "intake", "observations"] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![table],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if exists == 0 {
            return Ok(());
        }
    }
    let tasks = crate::flow::builtins::load_tasks_schema()?;
    let intake_yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "intake")
        .map(|(_, y)| *y)
        .ok_or_else(|| anyhow!("bundled intake schema missing"))?;
    let observations_yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "observations")
        .map(|(_, y)| *y)
        .ok_or_else(|| anyhow!("bundled observations schema missing"))?;
    let intake = crate::schema::Schema::from_yaml(intake_yaml)?;
    let observations = crate::schema::Schema::from_yaml(observations_yaml)?;
    let started_at = crate::handlers::row::now_iso8601();
    let iteration: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(iteration), 0) + 1 FROM engine_runner_heartbeats",
            [],
            |r| r.get(0),
        )
        .unwrap_or(1);
    // Pass base_dispatched so the persisted heartbeat row and the log line below
    // both reflect the union of engine-runner redrives + daemon base dispatches.
    // scan_record_and_redrive_tasks starts with state-driven external-review
    // reconciliation, so every daemon poll tick enforces the T2/T3 in_review lane invariant.
    let result = crate::flow::engine_runner::scan_record_and_redrive_tasks(
        conn,
        crate::flow::engine_runner::ScannerSchemas {
            tasks: &tasks,
            intake: &intake,
            observations: &observations,
        },
        iteration,
        &started_at,
        agents,
        config_path,
        policies_hash,
        base_dispatched,
    )?;
    eprintln!(
        "[engine-runner] iter={} saw=tasks:{} intake:{} obs:{} actionable={} held={} dispatched={}",
        result.summary.iteration,
        result.summary.saw_tasks,
        result.summary.saw_intake,
        result.summary.saw_observations,
        result.summary.actionable,
        result.summary.held,
        result.summary.dispatched
    );
    Ok(())
}

/// Starting-line seeder (T026 P1, refined for L116). For each agent declared
/// in `agents.yaml`, IF the agent has never seen a `dispatch_locks` row,
/// seed the entire matching transition_history as `skip-historical` so that
/// when a brand-new subscriber comes online it doesn't fire retroactively on
/// pre-existing rows (the original L055 case). Otherwise the agent has run
/// before — DO NOT re-seed, because that would race the dispatcher on any
/// transition that fired after the agent's previous run (the L116 case: a
/// user `confirm` verb between two `agents run --once` calls that lands a
/// new transition_history row, which the seeder then mis-claims as
/// historical and the dispatcher loses the UNIQUE(store, row_id, agent_name)
/// race against).
///
/// The per-agent presence check keys on `agent_name` because a previously-
/// seeded agent will have at least the marker locks from its first seed.
/// This handles the four real cases:
///   - First daemon run, no history yet → no agent has locks; seeding is a
///     no-op (correct).
///   - First daemon run, history exists → no agent has locks; everything
///     gets seeded as historical (closes L055).
///   - Subsequent run, no new transitions → agents have locks; skip seeding
///     (correct, idempotent).
///   - Subsequent run, NEW transitions fired between runs → agents have
///     locks; skip seeding so the dispatcher can claim the new transitions
///     (closes L116).
///
/// `max_transition_id` is retained as belt-and-suspenders for the rare case
/// where a brand-new agent is added AND new transitions fire during the
/// daemon's startup window before this seeder finishes — the bound prevents
/// the seeder from claiming those mid-startup transitions. Callers should
/// snapshot MAX(id) BEFORE invoking this function.
///
/// Returns the count of newly-inserted skip-historical rows across all
/// agents that needed seeding.
pub fn seed_starting_line(
    conn: &Connection,
    agents: &AgentsYaml,
    max_transition_id: i64,
) -> Result<usize> {
    let now = crate::handlers::row::now_iso8601();
    let mut total = 0usize;
    for agent in &agents.agents {
        if agent_has_been_seeded(conn, &agent.name)? {
            continue;
        }
        for sub in &agent.subscribes_to {
            let n = conn.execute(
                "INSERT OR IGNORE INTO dispatch_locks \
                 (store, row_id, display_id, agent_name, transition_id, \
                  claimed_at, claimed_by, last_status, finished_at, \
                  daemon_epoch, claim_source, attempt, terminal_reason, next_retry_at) \
                 SELECT th.store, th.row_id, th.display_id, ?1, th.id, ?2, \
                        'starting-line-marker', 'skip-historical', ?2, \
                        '', 'legacy', 0, 'legacy_unknown', NULL \
                 FROM transition_history th \
                 WHERE th.store = ?3 AND th.from_status = ?4 AND th.to_status = ?5 \
                       AND th.id <= ?6",
                rusqlite::params![
                    &agent.name,
                    &now,
                    &sub.store,
                    &sub.transition.from,
                    &sub.transition.to,
                    max_transition_id,
                ],
            )?;
            total += n;
        }
    }
    Ok(total)
}

/// True iff at least one dispatch_locks row exists for this agent_name. Used
/// to decide whether the starting-line seeder should run for the agent —
/// agents with prior locks have already had their starting-line drawn.
fn agent_has_been_seeded(conn: &Connection, agent_name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM dispatch_locks WHERE agent_name = ?1)",
        rusqlite::params![agent_name],
        |r| r.get(0),
    )?;
    Ok(n == 1)
}

/// Snapshot the current MAX(transition_history.id) for use as the seeder's
/// upper bound. Returns 0 when the table is empty (any later transition will
/// have id >= 1, so a 0-bound never seeds anything — correct cold-start
/// semantics).
pub fn snapshot_max_transition_id(conn: &Connection) -> Result<i64> {
    let id: Option<i64> =
        conn.query_row("SELECT MAX(id) FROM transition_history", [], |r| r.get(0))?;
    Ok(id.unwrap_or(0))
}

/// L134 / T050 Phase 1: ensure the 9 typed lifecycle columns exist on
/// `dispatch_locks`. Idempotent: detects missing columns via
/// `PRAGMA table_info('dispatch_locks')` and ALTERs only what is missing.
/// Records a single 'L134-dispatch-locks-typed' row in `framework_migrations`
/// the first time a column is added.
pub fn ensure_dispatch_locks_typed(conn: &Connection) -> Result<()> {
    // Set of columns we expect to be present after this migration.
    let expected: &[(&str, &str)] = &[
        ("daemon_epoch", "TEXT"),
        (
            "claim_source",
            "TEXT CHECK(claim_source IN ('try_claim','retry_claim','manual','legacy'))",
        ),
        ("attempt", "INTEGER"),
        ("pid", "INTEGER"),
        ("heartbeat_at", "TEXT"),
        ("postcondition_id", "TEXT"),
        ("postcondition_args", "TEXT"),
        (
            "terminal_reason",
            "TEXT CHECK(terminal_reason IN ('ok','exit_nonzero','error','silent_zombie','timeout','halted','legacy_unknown','rate_limit'))",
        ),
        ("next_retry_at", "TEXT"),
    ];

    let mut existing: std::collections::HashSet<String> = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info('dispatch_locks')")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        for row in rows {
            existing.insert(row?);
        }
    }

    let mut added_any = false;
    for (name, type_clause) in expected {
        if !existing.contains(*name) {
            let sql = format!("ALTER TABLE dispatch_locks ADD COLUMN {name} {type_clause}");
            conn.execute(&sql, [])
                .with_context(|| format!("adding column {name} to dispatch_locks"))?;
            added_any = true;
        }
    }

    if added_any {
        let now = crate::handlers::row::now_iso8601();
        conn.execute(
            "INSERT OR IGNORE INTO framework_migrations (id, applied_at, note) \
             VALUES ('L134-dispatch-locks-typed', ?1, 'add typed lifecycle columns to dispatch_locks')",
            rusqlite::params![now],
        )?;
    }

    ensure_dispatch_locks_terminal_reason_allows_rate_limit(conn)?;
    Ok(())
}

fn ensure_dispatch_locks_terminal_reason_allows_rate_limit(conn: &Connection) -> Result<()> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='dispatch_locks'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(sql) = sql else { return Ok(()); };
    if sql.contains("'rate_limit'") {
        return Ok(());
    }

    conn.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE dispatch_locks__t100_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            store TEXT NOT NULL,
            row_id INTEGER NOT NULL,
            display_id TEXT NOT NULL,
            agent_name TEXT NOT NULL,
            transition_id INTEGER,
            claimed_at TEXT NOT NULL,
            claimed_by TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 1,
            last_status TEXT,
            finished_at TEXT,
            daemon_epoch TEXT,
            claim_source TEXT CHECK(claim_source IN ('try_claim','retry_claim','manual','legacy')),
            attempt INTEGER,
            pid INTEGER,
            heartbeat_at TEXT,
            postcondition_id TEXT,
            postcondition_args TEXT,
            terminal_reason TEXT CHECK(terminal_reason IN ('ok','exit_nonzero','error','silent_zombie','timeout','halted','legacy_unknown','rate_limit')),
            next_retry_at TEXT,
            UNIQUE(store, row_id, agent_name)
         );
         INSERT INTO dispatch_locks__t100_new (
            id, store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by,
            attempts, last_status, finished_at, daemon_epoch, claim_source, attempt, pid,
            heartbeat_at, postcondition_id, postcondition_args, terminal_reason, next_retry_at
         )
         SELECT id, store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by,
            attempts, last_status, finished_at, daemon_epoch, claim_source, attempt, pid,
            heartbeat_at, postcondition_id, postcondition_args, terminal_reason, next_retry_at
         FROM dispatch_locks;
         DROP TABLE dispatch_locks;
         ALTER TABLE dispatch_locks__t100_new RENAME TO dispatch_locks;
         COMMIT;",
    )
    .inspect_err(|_| {
        let _ = conn.execute_batch("ROLLBACK;");
    })?;

    let now = crate::handlers::row::now_iso8601();
    conn.execute(
        "INSERT OR IGNORE INTO framework_migrations (id, applied_at, note) \
         VALUES ('T100-dispatch-locks-rate-limit-terminal-reason', ?1, \
                 'allow dispatch_locks.terminal_reason rate_limit')",
        rusqlite::params![now],
    )?;
    Ok(())
}

/// L134 / T050 Phase 1: backfill legacy `dispatch_locks` rows with values for
/// the new typed columns. ONLY populates rows where `claim_source IS NULL`
/// (i.e. rows that predate the migration) — never modifies live rows. This is
/// observability-only: lock semantics are unchanged.
///
/// Returns the number of rows updated.
pub fn backfill_legacy_locks(conn: &Connection) -> Result<usize> {
    // Single UPDATE with CASE for terminal_reason derivation. Only touches
    // rows whose claim_source is NULL (legacy / pre-migration).
    let n = conn.execute(
        "UPDATE dispatch_locks SET \
            claim_source = 'legacy', \
            attempt = COALESCE(attempts, 1), \
            terminal_reason = CASE \
                WHEN last_status = 'ok' THEN 'ok' \
                WHEN last_status LIKE 'exit=%' AND last_status != 'exit=0' THEN 'exit_nonzero' \
                WHEN last_status LIKE 'exit=0%' THEN 'ok' \
                WHEN last_status LIKE 'error:%' THEN 'error' \
                WHEN last_status LIKE 'halted:%' THEN 'halted' \
                WHEN last_status = 'skip-historical' THEN 'legacy_unknown' \
                ELSE 'legacy_unknown' \
            END, \
            next_retry_at = NULL, \
            daemon_epoch = '' \
         WHERE claim_source IS NULL",
        [],
    )?;
    Ok(n)
}

/// Atomically claim `(store, row_id, agent_name)` by inserting a
/// `dispatch_locks` row. Returns `Ok(true)` if we won the claim,
/// `Ok(false)` if another claimer won (UNIQUE conflict).
#[allow(clippy::too_many_arguments)]
pub fn try_claim(
    conn: &Connection,
    store: &str,
    row_id: i64,
    display_id: &str,
    agent_name: &str,
    transition_id: i64,
    claimer: &str,
    daemon_epoch: &str,
    claim_source: &str,
    postcondition_id: Option<&str>,
    postcondition_args: Option<&Value>,
) -> Result<bool> {
    let now = crate::handlers::row::now_iso8601();
    // Invariant (T041): try_claim inserts attempts=0 explicitly so that the
    // post-dispatch UPDATE (mark_claim_finished) can ALWAYS do
    // `attempts = attempts + 1` without distinguishing first-run from retry.
    // First completion → attempts=1, each subsequent retry → +1.
    let res = conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, attempts, \
          daemon_epoch, claim_source, attempt, postcondition_id, postcondition_args) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, 0, ?10, ?11)",
        rusqlite::params![
            store,
            row_id,
            display_id,
            agent_name,
            transition_id,
            now,
            claimer,
            daemon_epoch,
            claim_source,
            postcondition_id,
            postcondition_args.map(|v| v.to_string()),
        ],
    );
    match res {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Ok(false)
        }
        Err(e) => Err(anyhow!("try_claim insert failed: {}", e)),
    }
}

/// T046: route a non-zero subscriber exit to a framework-fired
/// `mark_deploy_blocked` when the schema declares such a transition out of
/// the row's current state. Today only `tasks: accepted → deploy_blocked`
/// qualifies (subscribed by `accept-merge`); the routing is schema-driven
/// rather than agent-name-keyed so any future store/edge with the same
/// shape inherits it. No-op when the schema doesn't declare the edge.
///
/// `last_status` is the dispatch_locks status string (e.g. `"exit=11"` /
/// `"error: <e>"`). The helper records `actor_note` on the
/// `transition_history` row as `agent=<name> status=<last_status>` so the
/// audit row carries the exit code.
pub(crate) fn route_failure_to_deploy_blocked(
    conn: &Connection,
    store: &str,
    display_id: &str,
    agent_name: &str,
    last_status: &str,
    policies_hash: &str,
    subscription_to: &str,
) {
    // Only the `tasks` store declares `mark_deploy_blocked` today. Avoid the
    // schema-load cost (and a spurious bundled-schema-not-found error) for
    // other stores until generalization is in scope (out-of-scope per T046).
    if store != "tasks" {
        return;
    }
    let schema = match crate::flow::builtins::load_tasks_schema() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[daemon] route_failure_to_deploy_blocked: load schema: {}",
                e
            );
            return;
        }
    };
    let qtable = quote_ident(&schema.name);
    let current_status: Option<String> = conn
        .query_row(
            &format!("SELECT status FROM {} WHERE display_id = ?1", qtable),
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .ok();
    let current_status = match current_status {
        Some(s) => s,
        None => return,
    };
    // Tightened gate (T046 codex-revise): only route to deploy_blocked when
    // the failed subscriber's transition.to MATCHES the row's current state.
    // This pins the failure-routing to the subscriber whose effect was to
    // land the row in the from-state of the deploy_blocked edge — accept-merge
    // (in_review→accepted), schema-migrate (cargo_installed→schema_migrated /
    // ...→deploy_blocked) — and excludes hypothetical unrelated subscribers
    // that happen to fail while a row sits at one of these states.
    if subscription_to != current_status {
        return;
    }
    let has_edge = schema.lifecycle.transitions.iter().any(|t| {
        t.from == current_status
            && t.verb == "mark_deploy_blocked"
            && t.actor == Some(crate::schema::actor::Actor::Framework)
    });
    if !has_edge {
        return;
    }
    let blocked_reason = format!("subscriber '{}' failed: {}", agent_name, last_status);
    let actor_note = format!("agent={} status={}", agent_name, last_status);
    if let Err(e) = crate::flow::builtins::fire_mark_deploy_blocked_with_note(
        conn,
        display_id,
        &blocked_reason,
        policies_hash,
        Some(&actor_note),
    ) {
        eprintln!(
            "[daemon] route_failure_to_deploy_blocked: fire mark_deploy_blocked for {}: {}",
            display_id, e
        );
    }
}

/// Precise outcome from [`close_auto_drive_lock_ok`].
///
/// * `Closed` — the lock was written with `terminal_reason='ok'` and
///   `finished_at` set; the drive cycle is fully terminal.
/// * `PendingNext` — the task still has work outstanding (`next_agent` is
///   non-null and wrap_log not yet populated); the lock was left open with
///   `last_status='in_flight:pending_next'` for the daemon to re-dispatch.
/// * `Failed` — reserved for callers that map `Err` to a non-fatal outcome;
///   `close_auto_drive_lock_ok` itself never returns this variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockCloseOutcome {
    Closed,
    PendingNext,
    Failed,
}

/// T049: close the open auto-drive `dispatch_locks` row for `display_id`
/// with `last_status='ok'`. Called from inside the drive subprocess on its
/// first successful `compute_submit_*` call so a drive that dies between
/// spawn and first submit leaves the lock open for the watchdog.
///
/// Returns a [`LockCloseOutcome`] distinguishing the actually-closed case
/// from the pending-next case. Callers must not treat `PendingNext` as a
/// successful close — the lock is still in-flight.
///
/// Idempotent: the WHERE clause filters on `finished_at IS NULL`, so a
/// second call is a no-op zero-row UPDATE.
pub(crate) fn close_auto_drive_lock_ok(
    conn: &Connection,
    display_id: &str,
) -> Result<LockCloseOutcome> {
    let now = crate::handlers::row::now_iso8601();
    if crate::flow::builtins::auto_drive::has_pending_auto_drive_work(conn, display_id)
        .unwrap_or(false)
    {
        conn.execute(
            "UPDATE dispatch_locks SET last_status = 'in_flight:pending_next', finished_at = NULL, \
                                      claimed_at = ?1, attempts = attempts + 1, \
                                      terminal_reason = NULL, next_retry_at = NULL \
             WHERE store = 'tasks' AND display_id = ?2 AND agent_name = 'auto-drive' \
               AND finished_at IS NULL",
            rusqlite::params![now, display_id],
        )?;
        return Ok(LockCloseOutcome::PendingNext);
    }
    conn.execute(
        "UPDATE dispatch_locks SET last_status = 'ok', finished_at = ?1, \
                                  claimed_at = ?1, attempts = attempts + 1, \
                                  terminal_reason = 'ok', next_retry_at = NULL \
         WHERE store = 'tasks' AND display_id = ?2 AND agent_name = 'auto-drive' \
           AND finished_at IS NULL",
        rusqlite::params![now, display_id],
    )?;
    Ok(LockCloseOutcome::Closed)
}

/// Force-close the open auto-drive `dispatch_locks` row for `display_id`
/// with `terminal_reason='ok'` and `finished_at` set, bypassing the
/// `has_pending_auto_drive_work` check.
///
/// Called by the drive subprocess immediately before exiting after a successful
/// wrap submission. At that point the row is still at `in_review` so
/// `has_pending_auto_drive_work` always returns `true` (schema yields
/// `next_agent=wrap` for every `in_review` row); skipping that check is
/// correct and intentional here — the drive loop has already dispatched wrap
/// and recorded the `wrap_log` entry, so no further work remains for THIS
/// drive invocation.
///
/// **Watchdog discriminator:** this function writes `last_status='ok:wrap_completed'`
/// (rather than plain `'ok'`). The watchdog's pending-handoff sweep filters out
/// `last_status='ok:wrap_completed'` rows to avoid re-dispatching wrap after the
/// drive subprocess has already handed it off. Plain `'ok'` closed locks (from
/// old handoffs whose drive subprocess died before calling this function) are
/// still eligible for re-dispatch. The `terminal_reason` column remains `'ok'`
/// (its CHECK constraint cannot be broadened without table recreation; `last_status`
/// is free-text and serves as the typed discriminator here).
///
/// A1 invariant is preserved: wrap_log is NOT consulted as a control-flow
/// sentinel. The decision to close is made by the drive loop's
/// `dispatched_wrap_this_run` flag (current-cycle completion state), not by
/// inspecting historical wrap_log content.
///
/// Watchdog fallback: if the drive subprocess dies without calling this
/// (e.g. process kill), the `last_status` stays as whatever the prior in-flight
/// value was (`in_flight:pending_next` or similar, NOT `ok:wrap_completed`),
/// so the watchdog's pending-handoff sweep will correctly re-dispatch a fresh
/// drive — correct amend/re-entry semantics.
///
/// Idempotent: the WHERE clause filters on `finished_at IS NULL`, so a
/// second call is a no-op zero-row UPDATE.
pub(crate) fn force_close_auto_drive_lock_ok(conn: &Connection, display_id: &str) -> Result<()> {
    let now = crate::handlers::row::now_iso8601();
    conn.execute(
        "UPDATE dispatch_locks SET last_status = 'ok:wrap_completed', finished_at = ?1, \
                                  claimed_at = ?1, attempts = attempts + 1, \
                                  terminal_reason = 'ok', next_retry_at = NULL \
         WHERE store = 'tasks' AND display_id = ?2 AND agent_name = 'auto-drive' \
           AND finished_at IS NULL",
        rusqlite::params![now, display_id],
    )?;
    Ok(())
}

pub(crate) fn mark_claim_finished(
    conn: &Connection,
    store: &str,
    row_id: i64,
    agent_name: &str,
    last_status: &str,
) -> Result<()> {
    let terminal_reason = terminal_reason_from_legacy_status(last_status);
    let now = crate::handlers::row::now_iso8601();
    let completed_attempt = conn
        .query_row(
            "SELECT COALESCE(attempts, 0) + 1 FROM dispatch_locks \
             WHERE store = ?1 AND row_id = ?2 AND agent_name = ?3",
            rusqlite::params![store, row_id, agent_name],
            |r| r.get::<_, u32>(0),
        )
        .unwrap_or(1);
    conn.execute(
        "UPDATE dispatch_locks SET last_status = ?1, finished_at = ?2, attempts = attempts + 1, \
         attempt = ?3, terminal_reason = ?4, next_retry_at = NULL \
         WHERE store = ?5 AND row_id = ?6 AND agent_name = ?7",
        rusqlite::params![
            last_status,
            now,
            completed_attempt,
            terminal_reason,
            store,
            row_id,
            agent_name
        ],
    )?;
    Ok(())
}

fn terminal_from_dispatch_result(res: Result<i32>) -> (String, String, i32) {
    match res {
        Ok(0) => ("ok".to_string(), derive_last_status("ok", None), 0),
        Ok(code) if code > 0 => (
            "exit_nonzero".to_string(),
            derive_last_status("exit_nonzero", Some(&code.to_string())),
            code,
        ),
        Ok(code) => (
            "error".to_string(),
            derive_last_status("error", Some(&format!("exit code {code}"))),
            code,
        ),
        Err(e) => (
            "error".to_string(),
            derive_last_status("error", Some(&e.to_string())),
            -1,
        ),
    }
}

fn terminal_reason_from_legacy_status(last_status: &str) -> &'static str {
    if last_status == "ok" || last_status.starts_with("exit=0") {
        "ok"
    } else if last_status.starts_with("exit=") {
        "exit_nonzero"
    } else if last_status.starts_with("halted:") {
        "halted"
    } else if last_status.starts_with("drive_failed:silent_zombie_")
        || last_status == "drive_failed:pid_never_recorded"
    {
        "silent_zombie"
    } else if last_status.starts_with("error:") {
        "error"
    } else {
        "legacy_unknown"
    }
}

fn derive_last_status(terminal_reason: &str, detail: Option<&str>) -> String {
    match terminal_reason {
        "ok" => "ok".to_string(),
        "exit_nonzero" => format!("exit={}", detail.unwrap_or("1")),
        "error" => format!("error: {}", detail.unwrap_or("subscriber failed")),
        "silent_zombie" => format!(
            "drive_failed:{}",
            detail.unwrap_or("silent_zombie_pid_dead")
        ),
        "halted" => format!("halted:{}", detail.unwrap_or("policy")),
        other => other.to_string(),
    }
}

fn enrich_investigator_failure_status(
    conn: &Connection,
    store: &str,
    display_id: &str,
    agent: &AgentEntry,
    base_status: &str,
) -> String {
    if store != "observations" || agent.command != "builtin:investigator" {
        return base_status.to_string();
    }
    let reason: Option<String> = conn
        .query_row(
            "SELECT investigation_failure_reason FROM observations WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let Some(reason) = reason else {
        return base_status.to_string();
    };
    if reason.trim().is_empty() {
        base_status.to_string()
    } else {
        format!("{}: {}", base_status, reason.trim())
    }
}

fn postcondition_args_for(id: &str, store: &str, display_id: &str) -> Value {
    if id == "task_exists_for_linked_observation" {
        json!({"observation_id": display_id, "store": store})
    } else {
        json!({"display_id": display_id, "store": store})
    }
}

enum PostconditionFailure {
    Check(crate::flow::checks::CheckResult),
    Legacy(String),
}

fn run_postcondition_for_lock(
    conn: &Connection,
    store: &str,
    row_id: i64,
    agent_name: &str,
) -> Result<Option<PostconditionFailure>> {
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT postcondition_id, postcondition_args FROM dispatch_locks \
             WHERE store = ?1 AND row_id = ?2 AND agent_name = ?3",
            rusqlite::params![store, row_id, agent_name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((Some(id), args_text)) = row else {
        return Ok(None);
    };
    let args: Value = args_text
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(check) = crate::flow::checks::lookup(&id) {
        let result = check.evaluate(
            crate::flow::checks::CheckCtx {
                conn: Some(conn),
                companion: None,
            },
            &args,
        )?;
        return Ok(if result.is_pass() {
            None
        } else {
            Some(PostconditionFailure::Check(result))
        });
    }
    let Some(pred) = crate::flow::postconditions::lookup(&id) else {
        return Ok(Some(PostconditionFailure::Legacy(id)));
    };
    let ok = pred(conn, &args, None)?;
    Ok(if ok {
        None
    } else {
        Some(PostconditionFailure::Legacy(id))
    })
}

fn iso8601_add_secs(base: &str, secs: u64) -> Option<String> {
    let epoch = parse_iso8601_to_epoch(base)?
        .saturating_add(secs as i64)
        .max(0) as u64;
    let (y, mo, d, h, mi, se) = unix_to_ymd_hms(epoch);
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}Z"))
}

#[allow(clippy::manual_is_multiple_of)]
fn unix_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s = secs % 60;
    let total_min = secs / 60;
    let mi = total_min % 60;
    let total_hr = total_min / 60;
    let h = total_hr % 24;
    let mut days = total_hr / 24;
    let mut year = 1970u32;
    loop {
        let dy = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
            366
        } else {
            365
        };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let dim = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0usize;
    while month < 12 && days >= dim[month] {
        days -= dim[month];
        month += 1;
    }
    (
        year,
        (month + 1) as u32,
        (days + 1) as u32,
        h as u32,
        mi as u32,
        s as u32,
    )
}

fn next_retry_at_for(
    agent: &AgentEntry,
    terminal_reason: &str,
    attempt: u32,
    finished_at: &str,
) -> Option<String> {
    if matches!(terminal_reason, "exit_nonzero" | "error" | "silent_zombie")
        && attempt < agent.retry_policy.max_attempts
    {
        iso8601_add_secs(
            finished_at,
            compute_backoff_secs(agent.retry_policy.backoff, attempt),
        )
    } else {
        None
    }
}

fn mark_auto_drive_pending_handoff(
    conn: &Connection,
    store: &str,
    row_id: i64,
    display_id: &str,
    agent: &AgentEntry,
) -> Result<()> {
    let now = crate::handlers::row::now_iso8601();
    let completed_attempt = conn
        .query_row(
            "SELECT COALESCE(attempts, 0) + 1 FROM dispatch_locks \
             WHERE store = ?1 AND row_id = ?2 AND agent_name = ?3",
            rusqlite::params![store, row_id, &agent.name],
            |r| r.get::<_, u32>(0),
        )
        .unwrap_or(1);
    conn.execute(
        "UPDATE dispatch_locks SET last_status = 'in_flight:pending_next', finished_at = NULL, \
         attempts = attempts + 1, attempt = ?1, terminal_reason = NULL, next_retry_at = NULL \
         WHERE store = ?2 AND row_id = ?3 AND agent_name = ?4",
        rusqlite::params![completed_attempt, store, row_id, &agent.name],
    )?;
    let _ = (display_id, now);
    Ok(())
}

fn task_rate_limit_until(conn: &Connection, store: &str, row_id: i64) -> Result<Option<String>> {
    if store != "tasks" {
        return Ok(None);
    }
    let reason: Option<String> = match conn
        .query_row(
            "SELECT blocked_reason FROM tasks WHERE id=?1",
            rusqlite::params![row_id],
            |r| r.get(0),
        )
        .optional()
    {
        Ok(v) => v,
        Err(e) if e.to_string().contains("no such column: blocked_reason") => None,
        Err(e) => return Err(e.into()),
    };
    Ok(reason.and_then(|r| parse_rate_limit_blocked_reason(&r).map(str::to_string)))
}

fn parse_rate_limit_blocked_reason(reason: &str) -> Option<&str> {
    let rest = reason.strip_prefix("rate_limit:")?;
    let (_provider, until) = rest.split_once(':')?;
    if is_iso8601_cooldown(until) { Some(until) } else { None }
}

fn is_iso8601_cooldown(s: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
}

fn mark_claim_rate_limit_cooldown(
    conn: &Connection,
    store: &str,
    row_id: i64,
    display_id: &str,
    agent: &AgentEntry,
    until: &str,
    last_status: &str,
) -> Result<()> {
    let now = crate::handlers::row::now_iso8601();
    conn.execute(
        "UPDATE dispatch_locks SET last_status = ?1, finished_at = ?2, \
         terminal_reason = 'rate_limit', next_retry_at = ?3 \
         WHERE store = ?4 AND row_id = ?5 AND agent_name = ?6",
        rusqlite::params![last_status, now, until, store, row_id, &agent.name],
    )?;
    let _ = display_id;
    Ok(())
}

fn mark_claim_finished_typed(
    conn: &Connection,
    store: &str,
    row_id: i64,
    display_id: &str,
    agent: &AgentEntry,
    terminal_reason: &str,
    last_status: &str,
) -> Result<()> {
    let now = crate::handlers::row::now_iso8601();
    let mut reason = terminal_reason.to_string();
    let mut status = last_status.to_string();
    if reason == "ok" {
        if let Some(failed) = run_postcondition_for_lock(conn, store, row_id, &agent.name)? {
            reason = "error".to_string();
            status = match failed {
                PostconditionFailure::Check(result) => {
                    let payload = serde_json::to_string(&result)?;
                    format!("error: check failed: {payload}")
                }
                PostconditionFailure::Legacy(id) => format!("error: postcondition {id} failed"),
            };
        }
    }
    let completed_attempt = conn
        .query_row(
            "SELECT COALESCE(attempts, 0) + 1 FROM dispatch_locks \
             WHERE store = ?1 AND row_id = ?2 AND agent_name = ?3",
            rusqlite::params![store, row_id, &agent.name],
            |r| r.get::<_, u32>(0),
        )
        .unwrap_or(1);
    let next_retry_at = next_retry_at_for(agent, &reason, completed_attempt, &now);
    conn.execute(
        "UPDATE dispatch_locks SET last_status = ?1, finished_at = ?2, attempts = attempts + 1, \
         attempt = ?3, terminal_reason = ?4, next_retry_at = ?5 \
         WHERE store = ?6 AND row_id = ?7 AND agent_name = ?8",
        rusqlite::params![
            status,
            now,
            completed_attempt,
            reason,
            next_retry_at,
            store,
            row_id,
            &agent.name
        ],
    )?;
    let _ = display_id;
    Ok(())
}

pub(crate) fn mark_claim_silent_zombie(
    conn: &Connection,
    store: &str,
    row_id: i64,
    agent: Option<&AgentEntry>,
    agent_name: &str,
    reason: &str,
) -> Result<()> {
    let now = crate::handlers::row::now_iso8601();
    let last_status = derive_last_status("silent_zombie", Some(reason));
    let completed_attempt = conn
        .query_row(
            "SELECT COALESCE(attempts, 0) + 1 FROM dispatch_locks              WHERE store = ?1 AND row_id = ?2 AND agent_name = ?3",
            rusqlite::params![store, row_id, agent_name],
            |r| r.get::<_, u32>(0),
        )
        .unwrap_or(1);
    let next_retry_at =
        agent.and_then(|a| next_retry_at_for(a, "silent_zombie", completed_attempt, &now));
    conn.execute(
        "UPDATE dispatch_locks SET last_status = ?1, finished_at = ?2, attempts = attempts + 1, \
         attempt = ?3, terminal_reason = 'silent_zombie', next_retry_at = ?4 \
         WHERE store = ?5 AND row_id = ?6 AND agent_name = ?7",
        rusqlite::params![
            last_status,
            now,
            completed_attempt,
            next_retry_at,
            store,
            row_id,
            agent_name
        ],
    )?;
    Ok(())
}

/// Compute the backoff window in seconds for a retry that has already
/// completed `attempt` attempts. Linear scales linearly with attempt;
/// Exponential doubles per additional attempt (saturating).
fn compute_backoff_secs(kind: BackoffKind, attempt: u32) -> u64 {
    match kind {
        BackoffKind::Linear => BASE_BACKOFF_SECS.saturating_mul(attempt as u64),
        BackoffKind::Exponential => {
            // 2^(attempt-1), but bound shift so we don't UB and saturate at
            // a reasonable ceiling. attempt=0 → BASE * 1 (treat as "no wait
            // before first retry"), attempt=N → BASE * 2^(N-1).
            let shift = attempt.saturating_sub(1).min(32);
            BASE_BACKOFF_SECS.saturating_mul(1u64 << shift)
        }
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` (the format produced by `now_iso8601`) into
/// a unix epoch (seconds). Returns None on malformed input.
fn parse_iso8601_to_epoch(s: &str) -> Option<i64> {
    if s.len() < 20 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y: u32 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let mo: u32 = std::str::from_utf8(&b[5..7]).ok()?.parse().ok()?;
    let d: u32 = std::str::from_utf8(&b[8..10]).ok()?.parse().ok()?;
    let h: u32 = std::str::from_utf8(&b[11..13]).ok()?.parse().ok()?;
    let mi: u32 = std::str::from_utf8(&b[14..16]).ok()?.parse().ok()?;
    let se: u32 = std::str::from_utf8(&b[17..19]).ok()?.parse().ok()?;
    Some(ymd_hms_to_epoch(y, mo, d, h, mi, se))
}

#[allow(clippy::manual_is_multiple_of)]
fn ymd_hms_to_epoch(y: u32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    fn is_leap(y: u32) -> bool {
        (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
    }
    fn days_in_month(y: u32, m: u32) -> u32 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if is_leap(y) {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        }
    }
    let mut days: i64 = 0;
    if y >= 1970 {
        for yy in 1970..y {
            days += if is_leap(yy) { 366 } else { 365 };
        }
    } else {
        for yy in y..1970 {
            days -= if is_leap(yy) { 366 } else { 365 };
        }
    }
    for mm in 1..mo {
        days += days_in_month(y, mm) as i64;
    }
    days += (d.saturating_sub(1)) as i64;
    days * 86_400 + h as i64 * 3600 + mi as i64 * 60 + s as i64
}

#[cfg(test)]
fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One row eligible for a retry-dispatch.
#[derive(Debug)]
struct RetryCandidate {
    store: String,
    row_id: i64,
    display_id: String,
    transition_id: i64,
    attempts: u32,
    attempt_snapshot: u32,
    terminal_reason_snapshot: String,
    from_status: String,
    to_status: String,
}

/// Atomic compare-and-swap claim for retry. Returns true if this caller now
/// owns the candidate (attempt + terminal_reason match the snapshot and the
/// row is now `retrying`); false if another daemon already moved the row. The
/// CAS guard closes the multi-daemon race where find_retryable_locks would
/// otherwise hand the same candidate to two callers concurrently.
fn claim_for_retry(conn: &Connection, c: &RetryCandidate, agent_name: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE dispatch_locks SET last_status = 'retrying', claim_source = 'retry_claim', \
         attempt = ?4 + 1, terminal_reason = NULL, \
         next_retry_at = NULL, finished_at = NULL \
         WHERE store = ?1 AND row_id = ?2 AND agent_name = ?3 \
               AND attempt = ?4 AND terminal_reason = ?5",
        rusqlite::params![
            &c.store,
            c.row_id,
            agent_name,
            c.attempt_snapshot,
            &c.terminal_reason_snapshot
        ],
    )?;
    Ok(n == 1)
}

/// T049: open the auto-drive retry-claimed lock by clearing `finished_at`
/// IMMEDIATELY before `run_dispatch` actually spawns the retry drive. This
/// must run after all retry gates (decision halt, predicate match) so a
/// gated-out retry leaves the lock with `finished_at` intact (no orphan).
/// Auto-drive's lock-stays-open semantics require finished_at IS NULL during
/// the spawn-to-first-submit window so T040's watchdog catches a dead PID.
fn open_auto_drive_retry_lock(conn: &Connection, c: &RetryCandidate) -> Result<()> {
    conn.execute(
        "UPDATE dispatch_locks SET finished_at = NULL \
         WHERE store = ?1 AND row_id = ?2 AND agent_name = 'auto-drive' \
               AND last_status = 'retrying'",
        rusqlite::params![&c.store, c.row_id],
    )?;
    Ok(())
}

/// Mark a retry-claimed row as halted by policy. last_status carries the
/// halting policy id so future polls (filtered to 'exit=*' / 'error:*')
/// will NOT re-include this row — closing the Halt-notification storm
/// where the same row would re-emit notify on every poll forever.
fn mark_retry_halted(
    conn: &Connection,
    c: &RetryCandidate,
    agent_name: &str,
    policy_id: &str,
) -> Result<()> {
    let now = crate::handlers::row::now_iso8601();
    let last = format!("halted:{policy_id}");
    conn.execute(
        "UPDATE dispatch_locks SET last_status = ?1, finished_at = ?2, terminal_reason = 'halted', next_retry_at = NULL \
         WHERE store = ?3 AND row_id = ?4 AND agent_name = ?5",
        rusqlite::params![last, now, &c.store, c.row_id, agent_name],
    )?;
    Ok(())
}

/// Find dispatch_locks rows for `agent` whose typed retry gate has elapsed.
/// Legacy string scans/backoff parsing are intentionally absent: retryability
/// is carried by terminal_reason + next_retry_at written at completion time.
fn find_retryable_locks(conn: &Connection, agent: &AgentEntry) -> Result<Vec<RetryCandidate>> {
    let max = agent.retry_policy.max_attempts;
    let mut stmt = conn.prepare(
        "SELECT dl.store, dl.row_id, dl.display_id, COALESCE(dl.transition_id, 0), \
                COALESCE(dl.attempts, 0), dl.attempt, dl.terminal_reason, \
                COALESCE(th.from_status, ''), COALESCE(th.to_status, '') \
         FROM dispatch_locks dl \
         LEFT JOIN transition_history th ON th.id = dl.transition_id \
         WHERE dl.agent_name = ?1 \
               AND (dl.terminal_reason = 'rate_limit' OR dl.attempt < ?2) \
               AND dl.next_retry_at IS NOT NULL \
               AND dl.next_retry_at <= ?3 \
               AND dl.terminal_reason IN ('exit_nonzero','error','silent_zombie','rate_limit')",
    )?;
    let now_iso = crate::handlers::row::now_iso8601();
    let rows = stmt.query_map(rusqlite::params![&agent.name, max, now_iso], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, u32>(4)?,
            r.get::<_, u32>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (
            store,
            row_id,
            display_id,
            transition_id,
            attempts,
            attempt_snapshot,
            terminal_reason_snapshot,
            from_s,
            to_s,
        ) = row?;
        let matched = agent.subscribes_to.iter().any(|sub| {
            sub.store == store && sub.transition.from == from_s && sub.transition.to == to_s
        });
        if !matched {
            continue;
        }
        out.push(RetryCandidate {
            store,
            row_id,
            display_id,
            transition_id,
            attempts,
            attempt_snapshot,
            terminal_reason_snapshot,
            from_status: from_s,
            to_status: to_s,
        });
    }
    Ok(out)
}

/// Run an agent's command. For builtins this is a stub until Phase 6.
///
/// `policy_ref` and `policies_hash` are forwarded as env vars
/// `STORES_POLICY_REF` / `STORES_POLICIES_HASH` so any follow-on substrate
/// transition the dispatched subprocess performs can record them on
/// `transition_history` (see `transition.rs::run_in_tx`). This is the
/// daemon→subscriber→substrate plumbing for AC5.3 / Task 5.2.
#[allow(clippy::too_many_arguments)]
fn run_dispatch(
    conn: &Connection,
    agents: &AgentsYaml,
    config_path: &Path,
    agent: &AgentEntry,
    store: &str,
    row_id: i64,
    display_id: &str,
    from: &str,
    to: &str,
    policy_ref: &str,
    policies_hash: &str,
    row_json: &Value,
) -> Result<i32> {
    if agent.is_builtin() {
        let kw = agent.command.trim_start_matches("builtin:");
        let ctx = crate::flow::builtins::DispatchCtx {
            conn,
            agents,
            config_path,
            policies_hash,
        };
        match crate::flow::builtins::dispatch_builtin(kw, row_json, &ctx) {
            Some(Ok(code)) => return Ok(code),
            Some(Err(e)) => {
                eprintln!(
                    "[daemon] builtin '{}' for {}/{} ({}->{}) failed: {}",
                    agent.command, store, display_id, from, to, e
                );
                return Ok(-1);
            }
            None => {
                eprintln!(
                    "[daemon] unknown builtin '{}' for {}/{} ({}->{}) policy_ref='{}'",
                    agent.command, store, display_id, from, to, policy_ref
                );
                return Ok(0);
            }
        }
    }
    use std::process::Command;
    let status = Command::new("sh")
        .arg("-c")
        .arg(&agent.command)
        .env("STORES_ROW_ID", row_id.to_string())
        .env("STORES_DISPLAY_ID", display_id)
        .env("STORES_TRANSITION_FROM", from)
        .env("STORES_TRANSITION_TO", to)
        .env("STORES_STORE", store)
        .env("STORES_POLICY_REF", policy_ref)
        .env("STORES_POLICIES_HASH", policies_hash)
        .status()
        .with_context(|| format!("spawning agent '{}'", agent.name))?;
    Ok(status.code().unwrap_or(-1))
}

/// Read a single row from `<store>` as a flat JSON object. JSON-typed columns
/// (TEXT-encoded) are best-effort parsed back into structured Values so
/// nested predicates work (`$linked_observations[0]`, etc).
fn read_row_as_json(conn: &Connection, store: &str, row_id: i64) -> Result<Value> {
    let sql = format!("SELECT * FROM {} WHERE id = ?1", quote_ident(store));
    let mut stmt = conn.prepare(&sql)?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(rusqlite::params![row_id])?;
    let row = rows
        .next()?
        .ok_or_else(|| anyhow!("row id={} not found in {}", row_id, store))?;
    let mut obj = serde_json::Map::new();
    for (i, name) in cols.iter().enumerate() {
        let v: rusqlite::types::Value = row.get(i)?;
        let jv = match v {
            rusqlite::types::Value::Null => Value::Null,
            rusqlite::types::Value::Integer(i) => Value::from(i),
            rusqlite::types::Value::Real(f) => {
                Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
            }
            rusqlite::types::Value::Text(s) => match serde_json::from_str::<Value>(&s) {
                Ok(parsed @ (Value::Object(_) | Value::Array(_))) => parsed,
                _ => Value::String(s),
            },
            rusqlite::types::Value::Blob(b) => {
                Value::String(String::from_utf8_lossy(&b).to_string())
            }
        };
        obj.insert(name.clone(), jv);
    }
    Ok(Value::Object(obj))
}

/// Public, just so a caller in lib.rs can resolve `.stores/config.yaml` for
/// tests without re-implementing path logic.
#[allow(dead_code)]
pub(crate) fn default_config_path() -> Result<PathBuf> {
    Ok(crate::paths::stores_dir()?.join("config.yaml"))
}

/// True when `pid > 0` and `kill(pid, 0)` succeeds or returns EPERM (foreign
/// live process). Signal 0 sends nothing; ESRCH means no such process.
pub(crate) fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Returns true if `pid` is in zombie state (exited, waiting to be reaped).
///
/// On Linux, `/proc/<pid>/stat` contains the process state as the character
/// after the closing ')' of the comm field. State 'Z' means zombie: the
/// process has already exited but `kill(pid, 0)` still returns 0 because the
/// kernel has not yet released the PID. For our purposes (daemon stop wait
/// loop and stale-pidfile detection) a zombie should be treated as "exited".
///
/// On non-Linux platforms (e.g. macOS) `/proc` is not available so this
/// always returns false. Callers fall back to `kill(pid, 0)` semantics; a
/// zombie may appear live in that case, which is an acceptable limitation.
#[cfg(target_os = "linux")]
pub(crate) fn pid_is_zombie(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let stat_path = format!("/proc/{pid}/stat");
    // /proc/<pid>/stat: "pid (comm) state ..."
    // comm can contain spaces and parentheses; find the LAST ')' to skip it.
    if let Ok(contents) = std::fs::read_to_string(&stat_path) {
        if let Some(after_comm) = contents.rfind(')') {
            let rest = contents[after_comm + 1..].trim_start();
            return rest.starts_with('Z');
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn pid_is_zombie(_pid: i32) -> bool {
    // /proc/<pid>/stat is not available on non-Linux platforms (e.g. macOS).
    // A zombie will appear live via kill(pid, 0); stop may time out in the
    // rare event the daemon enters zombie state during the wait window.
    false
}

/// True when a live drive subprocess's executable inode has been replaced on
/// disk — i.e. `/proc/<pid>/exe` resolves to a path ending with ` (deleted)`,
/// the kernel marker for an unlinked exe inode.
///
/// Also returns true if `read_link` fails with `NotFound` while `pid_is_alive`
/// still holds — the proc entry vanished under us (a race). Returns false for
/// `PermissionDenied` and all other errors so foreign-uid live processes are
/// never flipped. On non-Linux the function always returns false (parity with
/// `pid_is_zombie`).
#[cfg(target_os = "linux")]
pub(crate) fn drive_pid_exe_is_stale(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let exe_path = format!("/proc/{pid}/exe");
    match std::fs::read_link(&exe_path) {
        Ok(target) => target.to_string_lossy().ends_with(" (deleted)"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => pid_is_alive(pid),
        Err(_) => false,
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn drive_pid_exe_is_stale(_pid: i32) -> bool {
    false
}

/// Count tasks rows whose `drive_pid` is set to a still-running process.
/// Used by the daemon's `poll_once` cap-check (Task 4.5).
#[allow(dead_code)]
pub(crate) fn count_live_drive_pids(conn: &Connection) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT drive_pid FROM tasks WHERE drive_pid IS NOT NULL")?;
    let pids: Vec<i64> = stmt
        .query_map([], |r| r.get::<_, i64>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(pids.into_iter().filter(|p| pid_is_alive(*p as i32)).count())
}


/// Spawn `argv` as an orphaned grandchild detached from the daemon. Returns
/// the grandchild PID. Stdout/stderr go to `log_path` (created/appended).
/// `cwd` becomes the grandchild's working directory.
///
/// Uses double-fork + a pipe so the parent can read the grandchild PID and
/// reap the intermediate child without leaving a zombie. The grandchild is
/// reparented to PID 1 once the intermediate child exits.
pub(crate) fn spawn_detached_drive(argv: &[String], cwd: &Path, log_path: &Path) -> Result<i32> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;

    if argv.is_empty() {
        bail!("spawn_detached_drive: empty argv");
    }
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating log dir {}", parent.display()))?;
    }

    // Pipe for grandchild→parent PID communication.
    let mut fds: [libc::c_int; 2] = [-1, -1];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        bail!("pipe() failed");
    }
    let read_fd = fds[0];
    let write_fd = fds[1];

    let argv_owned: Vec<std::ffi::CString> = argv
        .iter()
        .map(|s| {
            std::ffi::CString::new(s.as_bytes())
                .unwrap_or_else(|_| std::ffi::CString::new("").unwrap())
        })
        .collect();
    let cwd_c = std::ffi::CString::new(cwd.as_os_str().as_bytes())
        .map_err(|_| anyhow!("cwd contains NUL"))?;
    let log_c = std::ffi::CString::new(log_path.as_os_str().as_bytes())
        .map_err(|_| anyhow!("log_path contains NUL"))?;

    unsafe {
        let pid1 = libc::fork();
        if pid1 < 0 {
            libc::close(read_fd);
            libc::close(write_fd);
            bail!("first fork failed");
        }
        if pid1 == 0 {
            // ---- intermediate child ----
            libc::close(read_fd);
            if libc::setsid() < 0 {
                libc::_exit(11);
            }
            let pid2 = libc::fork();
            if pid2 < 0 {
                libc::_exit(12);
            }
            if pid2 == 0 {
                // ---- grandchild ----
                libc::close(write_fd);
                // Open log file (create | append). Mode 0644.
                let log_fd = libc::open(
                    log_c.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
                    0o644,
                );
                if log_fd >= 0 {
                    libc::dup2(log_fd, libc::STDOUT_FILENO);
                    libc::dup2(log_fd, libc::STDERR_FILENO);
                    if log_fd > libc::STDERR_FILENO {
                        libc::close(log_fd);
                    }
                }
                // Close stdin (drive subprocess does not read it).
                libc::close(libc::STDIN_FILENO);

                if libc::chdir(cwd_c.as_ptr()) != 0 {
                    libc::_exit(13);
                }

                // Build argv ptr array (NULL-terminated).
                let mut argv_ptrs: Vec<*const libc::c_char> =
                    argv_owned.iter().map(|c| c.as_ptr()).collect();
                argv_ptrs.push(std::ptr::null());
                libc::execvp(argv_ptrs[0], argv_ptrs.as_ptr());
                // Only reached on exec failure.
                libc::_exit(127);
            }
            // ---- intermediate writes pid2 then exits ----
            let bytes = (pid2 as i32).to_le_bytes();
            let _ = libc::write(write_fd, bytes.as_ptr() as *const _, bytes.len());
            libc::close(write_fd);
            libc::_exit(0);
        }

        // ---- parent ----
        libc::close(write_fd);
        let mut buf = [0u8; 4];
        let n = libc::read(read_fd, buf.as_mut_ptr() as *mut _, buf.len());
        libc::close(read_fd);
        // Reap the intermediate child.
        let mut status: libc::c_int = 0;
        libc::waitpid(pid1, &mut status as *mut _, 0);
        if n != 4 {
            bail!(
                "spawn_detached_drive: short read from pid pipe ({} bytes)",
                n
            );
        }
        let pid2 = i32::from_le_bytes(buf);
        if pid2 <= 0 {
            bail!("spawn_detached_drive: invalid grandchild pid {}", pid2);
        }
        // Touch fd vars so the AsRawFd import isn't flagged unused.
        let _ = std::io::stdout().as_raw_fd();
        Ok(pid2)
    }
}

fn detach_process(log_file: &Option<String>) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let log_path = log_file
        .as_deref()
        .ok_or_else(|| anyhow!("--detach requires --log-file"))?;
    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            bail!("fork failed");
        }
        if pid > 0 {
            println!("{}", pid);
            std::process::exit(0);
        }
        if libc::setsid() < 0 {
            bail!("setsid failed");
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("opening log file {}", log_path))?;
        let fd = f.as_raw_fd();
        libc::dup2(fd, libc::STDOUT_FILENO);
        libc::dup2(fd, libc::STDERR_FILENO);
        libc::close(libc::STDIN_FILENO);
        std::mem::forget(f);
    }
    Ok(())
}

// AgentsYaml::default_empty helper lives next to AgentsYaml itself but we
// keep a tiny adapter here so the daemon's empty-config path is one call.
impl AgentsYaml {
    pub fn default_empty() -> Self {
        Self {
            agents: vec![],
            deployment_specialist: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::SUBSTRATE_DDL;
    use crate::flow::agents_yaml::TransitionEdge;
    use crate::flow::{AgentEntry, BackoffKind, RetryPolicy, Subscription};
    use std::collections::HashMap;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Clone)]
    struct MockIdentityProvider {
        identities: HashMap<PathBuf, BinaryIdentity>,
    }

    impl BinaryIdentityProvider for MockIdentityProvider {
        fn identity(&self, path: &Path) -> Result<BinaryIdentity> {
            self.identities
                .get(path)
                .copied()
                .ok_or_else(|| anyhow!("missing mock identity for {}", path.display()))
        }
    }

    fn mock_guard(
        startup: BinaryIdentity,
        launch_path: PathBuf,
        current: BinaryIdentity,
    ) -> DaemonExeGuard<MockIdentityProvider> {
        let mut identities = HashMap::new();
        identities.insert(launch_path.clone(), current);
        DaemonExeGuard::new(startup, launch_path, MockIdentityProvider { identities })
    }

    fn fresh_db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SUBSTRATE_DDL).unwrap();
        // Minimal `tasks` table the policy/dispatch tests rely on. Fields
        // mirror what the production schema would expose to predicates.
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_id TEXT UNIQUE NOT NULL,
                status TEXT NOT NULL,
                tier_hint TEXT,
                branch TEXT
            );",
        )
        .unwrap();
        c
    }

    fn empty_policies() -> PoliciesYaml {
        PoliciesYaml {
            hash: String::new(),
            policies: vec![],
        }
    }

    fn cfg_path() -> std::path::PathBuf {
        // Pointing at a non-existent file is fine: notify_with_path falls
        // through to the env var (also unset in tests) → stderr-only.
        std::path::PathBuf::from("/tmp/stores-test-nonexistent-config.yaml")
    }

    fn fresh_engine_runner_db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SUBSTRATE_DDL).unwrap();
        let tasks = crate::flow::builtins::load_tasks_schema().unwrap();
        let intake_yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "intake")
            .map(|(_, y)| *y)
            .unwrap();
        let observations_yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "observations")
            .map(|(_, y)| *y)
            .unwrap();
        let intake = crate::schema::Schema::from_yaml(intake_yaml).unwrap();
        let observations = crate::schema::Schema::from_yaml(observations_yaml).unwrap();
        c.execute_batch(&crate::codegen::ddl::ddl_for(&tasks))
            .unwrap();
        c.execute_batch(&crate::codegen::ddl::ddl_for(&intake))
            .unwrap();
        c.execute_batch(&crate::codegen::ddl::ddl_for(&observations))
            .unwrap();
        c
    }

    fn insert_history(
        conn: &Connection,
        store: &str,
        row_id: i64,
        display_id: &str,
        from: &str,
        to: &str,
    ) {
        conn.execute(
            "INSERT INTO transition_history \
             (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'submit', 'ai_autonomous', '2026-05-03T00:00:00Z')",
            rusqlite::params![store, row_id, display_id, from, to],
        )
        .unwrap();
    }

    fn insert_task_row(
        conn: &Connection,
        row_id: i64,
        display_id: &str,
        status: &str,
        tier: &str,
        branch: &str,
    ) {
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![row_id, display_id, status, tier, branch],
        )
        .unwrap();
    }

    fn noop_agent(name: &str, store: &str, from: &str, to: &str) -> AgentEntry {
        AgentEntry {
            name: name.to_string(),
            subscribes_to: vec![Subscription {
                store: store.to_string(),
                transition: TransitionEdge {
                    from: from.to_string(),
                    to: to.to_string(),
                },
                predicate: None,
            }],
            command: "/bin/true".to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy {
                max_attempts: 3,
                backoff: BackoffKind::Linear,
            },
            command_args: None,
        }
    }

    fn add_auto_drive_columns(conn: &Connection) {
        conn.execute_batch(
            "ALTER TABLE tasks ADD COLUMN workspace_path TEXT;
             ALTER TABLE tasks ADD COLUMN drive_pid INTEGER;
             ALTER TABLE tasks ADD COLUMN drive_started_at TEXT;
             ALTER TABLE tasks ADD COLUMN updated_at TEXT;",
        )
        .unwrap();
    }

    fn auto_drive_agent() -> AgentEntry {
        let mut agent = noop_agent("auto-drive", "tasks", "", "planning");
        agent.command = "builtin:auto-drive".to_string();
        agent.subscribes_to[0].predicate = Some(crate::flow::predicate::PredicateExpr::Neq {
            left: serde_json::json!("$workspace_path"),
            right: serde_json::json!(""),
        });
        agent
    }

    #[test]
    fn engine_runner_iteration_records_zero_row_heartbeat() {
        let conn = fresh_engine_runner_db();
        run_engine_runner_iteration(&conn, &AgentsYaml::default_empty(), &cfg_path(), "", 0)
            .unwrap();

        let row: (i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT saw_tasks, saw_intake, saw_observations, actionable, held, dispatched \
                 FROM engine_runner_heartbeats WHERE iteration=1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row, (0, 0, 0, 0, 0, 0));
    }

    #[test]
    fn argv_filter_removes_approve_token_forms_and_keeps_safe_args() {
        let filtered = filter_daemon_argv([
            "stores",
            "--approve-token",
            "plain-token",
            "agents",
            "run",
            "--approve-token=inline-token",
            "--poll-interval",
            "0.1",
        ]);
        assert_eq!(
            filtered,
            vec!["stores", "agents", "run", "--poll-interval", "0.1"]
        );
        let joined = filtered.join(" ");
        assert!(!joined.contains("--approve-token"));
        assert!(!joined.contains("plain-token"));
        assert!(!joined.contains("inline-token"));
    }

    #[test]
    fn argv_filter_removes_secret_flags_values_and_contains_forms() {
        let filtered = filter_daemon_argv([
            "stores",
            "--secret-token",
            "secret-value",
            "--safe",
            "ok",
            "--secret-key=inline",
            "prefix--secret-redacted",
        ]);
        assert_eq!(filtered, vec!["stores", "--safe", "ok"]);
        let joined = filtered.join(" ");
        assert!(!joined.contains("--secret-"));
        assert!(!joined.contains("secret-value"));
        assert!(!joined.contains("inline"));
    }

    // ----- argv scrubbing: additional cases per codex-revise -----

    /// Adjacent secret flags: `--approve-token --secret-key secret_value` must
    /// redact both flag/value pairs, not leak `secret_value` as a literal.
    #[test]
    fn argv_filter_adjacent_secret_flags_atomic() {
        let filtered = filter_daemon_argv([
            "stores",
            "--approve-token",
            "--secret-key",
            "secret_value",
            "safe-arg",
        ]);
        let joined = filtered.join(" ");
        assert!(!joined.contains("--approve-token"), "approve-token leaked");
        assert!(!joined.contains("--secret-key"), "secret-key leaked");
        assert!(!joined.contains("secret_value"), "secret_value leaked");
        assert!(joined.contains("safe-arg"), "safe-arg dropped incorrectly");
    }

    /// `--flag=value` form: single arg must be fully redacted.
    #[test]
    fn argv_filter_flag_equals_value_form() {
        let filtered = filter_daemon_argv([
            "stores",
            "--approve-token=tok123",
            "--my-token=abc",
            "--my-key=xyz",
            "--my-secret=sssh",
            "--safe",
            "ok",
        ]);
        let joined = filtered.join(" ");
        assert!(!joined.contains("tok123"), "tok123 leaked");
        assert!(!joined.contains("abc"), "abc leaked");
        assert!(!joined.contains("xyz"), "xyz leaked");
        assert!(!joined.contains("sssh"), "sssh leaked");
        assert_eq!(filtered, vec!["stores", "--safe", "ok"]);
    }

    /// Non-secret flags must NOT be redacted.
    #[test]
    fn argv_filter_non_secret_flags_preserved() {
        let filtered = filter_daemon_argv([
            "stores",
            "agents",
            "run",
            "--poll-interval",
            "500",
            "--log-file",
            "/tmp/daemon.log",
        ]);
        assert_eq!(
            filtered,
            vec![
                "stores",
                "agents",
                "run",
                "--poll-interval",
                "500",
                "--log-file",
                "/tmp/daemon.log"
            ]
        );
    }

    /// Mixed real-world argv: multiple secret and safe flags interleaved.
    #[test]
    fn argv_filter_mixed_real_world() {
        let filtered = filter_daemon_argv([
            "stores",
            "agents",
            "run",
            "--approve-token",
            "tkn-abc",
            "--poll-interval",
            "1000",
            "--secret-db-key=hunter2",
            "--log-file",
            "/var/log/stores.log",
            "--my-token",
            "bearer-xyz",
        ]);
        let joined = filtered.join(" ");
        assert!(!joined.contains("tkn-abc"), "token leaked");
        assert!(!joined.contains("hunter2"), "db-key leaked");
        assert!(!joined.contains("bearer-xyz"), "bearer leaked");
        assert!(
            joined.contains("--poll-interval"),
            "--poll-interval dropped"
        );
        assert!(joined.contains("1000"), "1000 dropped");
        assert!(joined.contains("--log-file"), "--log-file dropped");
    }

    // ----- display_id concurrency: insert-then-rowid -----

    /// Two inserts into daemon_starts must yield distinct display_ids derived
    /// from their respective autoincrement rowids — not from MAX(id)+1 which
    /// would race under concurrent writers.
    /// Uses the unique-pending-placeholder pattern from insert_daemon_startup:
    /// each insert uses a process+seq placeholder so no UNIQUE collision occurs.
    #[test]
    fn daemon_starts_display_id_derived_from_rowid() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();

        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        // Simulate the insert-then-update pattern from insert_daemon_startup.
        let insert_and_get_display_id = |conn: &Connection| -> String {
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let pending = format!("__pending_{}_{}", std::process::id(), seq);
            conn.execute(
                "INSERT INTO daemon_starts \
                 (display_id, status, created_at, updated_at, created_by, updated_by, \
                  pid, started_at, binary_path, binary_version, git_sha, argv, cwd) \
                 VALUES (?1, 'started', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', \
                         'daemon', 'daemon', 1, '2026-01-01T00:00:00Z', '/bin/stores', \
                         '0.1.0', 'deadbeef', '[]', '/tmp')",
                rusqlite::params![pending],
            )
            .unwrap();
            let rowid = conn.last_insert_rowid();
            let display_id = format!("D{rowid:03}");
            conn.execute(
                "UPDATE daemon_starts SET display_id = ?1 WHERE id = ?2",
                rusqlite::params![display_id, rowid],
            )
            .unwrap();
            display_id
        };

        let d1 = insert_and_get_display_id(&conn);
        let d2 = insert_and_get_display_id(&conn);

        assert_ne!(d1, d2, "display_ids must differ");
        // rowids are 1 and 2 → D001 and D002
        assert_eq!(d1, "D001");
        assert_eq!(d2, "D002");

        // Confirm stored values match derived display_ids.
        let stored: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT display_id FROM daemon_starts ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(stored, vec!["D001", "D002"]);
    }

    #[test]
    fn daemon_exe_identity_fresh_identity_stays_fresh() {
        let ident = BinaryIdentity { dev: 1, ino: 2 };
        let guard = mock_guard(ident, PathBuf::from("/tmp/stores"), ident);
        assert_eq!(guard.current_status().unwrap(), DaemonExeStatus::Fresh);
        assert_eq!(guard.check_stale().unwrap(), None);
    }

    #[test]
    fn daemon_exe_identity_detects_stale_launch_path_identity() {
        let guard = mock_guard(
            BinaryIdentity { dev: 1, ino: 2 },
            PathBuf::from("/tmp/stores"),
            BinaryIdentity { dev: 1, ino: 3 },
        );
        assert_eq!(
            guard.current_status().unwrap(),
            DaemonExeStatus::Stale {
                message: STALE_DAEMON_MESSAGE
            }
        );
        assert_eq!(guard.check_stale().unwrap(), Some(STALE_DAEMON_MESSAGE));
    }

    #[test]
    fn stale_auto_drive_guard_refuses_before_claim_or_spawn_side_effect() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Reset process-local dedup flag so test order doesn't matter.
        STALE_HALTED.store(false, Ordering::SeqCst);
        std::env::remove_var("STORES_DRIVE_CMD");
        let conn = fresh_db();
        add_auto_drive_columns(&conn);
        let tmp = tempfile::tempdir().unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path) \
             VALUES (91, 'T091', 'planning', 'T2', 'feat/stale', ?1)",
            rusqlite::params![tmp.path().to_string_lossy()],
        )
        .unwrap();
        insert_history(&conn, "tasks", 91, "T091", "", "planning");
        let agents = AgentsYaml {
            agents: vec![auto_drive_agent()],
            deployment_specialist: None,
        };
        let guard = mock_guard(
            BinaryIdentity { dev: 7, ino: 8 },
            PathBuf::from("/tmp/stores"),
            BinaryIdentity { dev: 7, ino: 9 },
        );

        // With the MAJOR 1 fix, poll_once_with_guard returns Err (not Ok(0))
        // when stale so that run_daemon's outer loop bails rather than
        // continuing to poll a stale binary.
        let result = poll_once_with_guard(
            &conn,
            &agents,
            &empty_policies(),
            &cfg_path(),
            "test-claimer",
            "epoch",
            Some(&guard),
        );
        assert!(
            result.is_err(),
            "stale binary must cause poll_once_with_guard to return Err"
        );
        assert_eq!(result.unwrap_err().to_string(), STALE_DAEMON_MESSAGE);
        let (claims, drive_pid): (i64, Option<i64>) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='auto-drive'), drive_pid \
                 FROM tasks WHERE display_id='T091'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(claims, 0, "stale guard must refuse before try_claim");
        assert!(drive_pid.is_none(), "stale guard must refuse before spawn");
    }

    #[test]
    fn fresh_auto_drive_guard_records_positive_drive_pid() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        STALE_HALTED.store(false, Ordering::SeqCst);
        let conn = fresh_db();
        add_auto_drive_columns(&conn);
        let tmp = tempfile::tempdir().unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path) \
             VALUES (92, 'T092', 'planning', 'T2', 'feat/fresh', ?1)",
            rusqlite::params![tmp.path().to_string_lossy()],
        )
        .unwrap();
        insert_history(&conn, "tasks", 92, "T092", "", "planning");
        let agents = AgentsYaml {
            agents: vec![auto_drive_agent()],
            deployment_specialist: None,
        };
        let ident = BinaryIdentity { dev: 7, ino: 8 };
        let guard = mock_guard(ident, PathBuf::from("/tmp/stores"), ident);
        std::env::set_var("STORES_DRIVE_CMD", "sleep 2 #");

        let n = poll_once_with_guard(
            &conn,
            &agents,
            &empty_policies(),
            &cfg_path(),
            "test-claimer",
            "epoch",
            Some(&guard),
        )
        .unwrap();
        std::env::remove_var("STORES_DRIVE_CMD");
        assert_eq!(n, 1);
        let drive_pid: i64 = conn
            .query_row(
                "SELECT drive_pid FROM tasks WHERE display_id='T092'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            drive_pid > 0,
            "fresh guard must allow spawn; got {drive_pid}"
        );
        unsafe {
            libc::kill(drive_pid as i32, libc::SIGTERM);
        }
    }

    #[test]
    fn daemon_stale_messages_keep_reexec_and_fail_loud_boundaries() {
        let guard = mock_guard(
            BinaryIdentity { dev: 1, ino: 2 },
            PathBuf::from("/tmp/stores"),
            BinaryIdentity { dev: 3, ino: 4 },
        );
        let message = guard.check_stale().unwrap().unwrap();
        assert_eq!(
            message,
            "daemon binary stale after cargo install; restart required"
        );
        assert_eq!(
            stale_reexec_attempt_line(guard.launch_path()),
            format!(
                "daemon binary stale; reexecing into /tmp/stores (was version {})",
                crate::version::build_identity()
            )
        );
    }

    /// AC4.2 test (b): a tasks row freshly transitioned to in_review is
    /// dispatched once to a registered noop subscriber within one poll
    /// iteration.
    #[test]
    fn poll_dispatches_matching_row_once() {
        let conn = fresh_db();
        insert_task_row(&conn, 42, "T042", "in_review", "T2", "feat/x");
        insert_history(&conn, "tasks", 42, "T042", "ready", "in_review");
        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };
        let policies = empty_policies();
        let cfg = cfg_path();

        let n = poll_once(&conn, &agents, &policies, &cfg, "test-claimer", "").unwrap();
        assert_eq!(n, 1, "first poll dispatches the matching row exactly once");

        // Lock recorded.
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 42 AND agent_name = 'noop'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);

        // Second poll on same db is a no-op (already claimed).
        let n2 = poll_once(&conn, &agents, &policies, &cfg, "test-claimer", "").unwrap();
        assert_eq!(
            n2, 0,
            "second poll does not re-dispatch an already-claimed row"
        );
    }

    /// AC4.3 test (c): two concurrent dispatch invocations against the same
    /// row result in exactly one row in dispatch_locks.
    #[test]
    fn concurrent_try_claim_yields_exactly_one_winner() {
        // Use a shared on-disk SQLite to allow two threads with their own
        // connections (in-memory DBs are not shared across handles).
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.sqlite");
        let setup = Connection::open(&db).unwrap();
        setup.execute_batch(SUBSTRATE_DDL).unwrap();
        drop(setup);

        let db1 = db.clone();
        let db2 = db.clone();
        let h1 = std::thread::spawn(move || {
            let c = Connection::open(&db1).unwrap();
            try_claim(
                &c,
                "tasks",
                7,
                "T007",
                "noop",
                1,
                "claimer-1",
                "epoch",
                "try_claim",
                None,
                None,
            )
            .unwrap()
        });
        let h2 = std::thread::spawn(move || {
            let c = Connection::open(&db2).unwrap();
            try_claim(
                &c,
                "tasks",
                7,
                "T007",
                "noop",
                1,
                "claimer-2",
                "epoch",
                "try_claim",
                None,
                None,
            )
            .unwrap()
        });

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();
        assert!(
            r1 ^ r2,
            "exactly one of the two concurrent claims must succeed; got r1={r1} r2={r2}"
        );

        let c = Connection::open(&db).unwrap();
        let cnt: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 7 AND agent_name = 'noop'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1, "exactly one dispatch_locks row exists post-race");
    }

    /// T050 P3 AC3.1: try_claim populates typed lifecycle columns.
    #[test]
    fn t050_try_claim_populates_typed_columns() {
        let conn = fresh_db();
        let args = json!({"display_id":"T050"});
        assert!(try_claim(
            &conn,
            "tasks",
            50,
            "T050",
            "auto-drive",
            1,
            "claimer",
            "epoch-050",
            "try_claim",
            Some("drive_pid_recorded_or_terminal"),
            Some(&args),
        )
        .unwrap());
        let (epoch, source, attempt, pcid, pcargs): (String, String, i64, String, String) = conn
            .query_row(
                "SELECT daemon_epoch, claim_source, attempt, postcondition_id, postcondition_args                  FROM dispatch_locks WHERE row_id=50 AND agent_name='auto-drive'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(epoch, "epoch-050");
        assert_eq!(source, "try_claim");
        assert_eq!(attempt, 0);
        assert_eq!(pcid, "drive_pid_recorded_or_terminal");
        assert!(pcargs.contains("T050"));
    }

    /// AC4.5: malformed agents.yaml refuses to parse; the error names the
    /// failing field. The daemon's `run_daemon` would surface this via
    /// context; we exercise the underlying loader here.
    #[test]
    fn malformed_agents_yaml_is_refused_with_field_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agents.yaml");
        // `command` missing on the only entry.
        std::fs::write(
            &path,
            "agents:\n  - name: a\n    subscribes_to:\n      - store: tasks\n        transition: { from: a, to: b }\n",
        )
        .unwrap();
        let err = crate::flow::agents_yaml::load_from_path(&path)
            .unwrap_err()
            .to_string();
        assert!(err.contains("command"), "expected field path; got: {err}");
    }

    // ---- Phase 5: policy integration tests (AC5.1 cases d/e/f/g/h) ----
    pub(super) mod policy {
        use super::*;
        use crate::flow::policies_yaml::PoliciesYaml;
        use crate::flow::{install_notifier, MockNotifier, NotifierBackend, NotifyEvent};
        use std::sync::Mutex;

        /// Helper: build a PoliciesYaml from inline YAML and seed agents with
        /// a single noop subscriber on tasks: ready→in_review.
        fn fixture(policies_yaml: &str) -> (Connection, AgentsYaml, PoliciesYaml) {
            let conn = fresh_db();
            let agents = AgentsYaml {
                agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
                deployment_specialist: None,
            };
            let policies = if policies_yaml.is_empty() {
                empty_policies()
            } else {
                PoliciesYaml::from_yaml(policies_yaml).unwrap()
            };
            (conn, agents, policies)
        }

        /// Capture-and-forward shim so the test can assert on events after
        /// the boxed backend is installed into the OnceLock.
        struct Shim {
            inner: &'static MockNotifier,
        }
        impl NotifierBackend for Shim {
            fn send(&self, url: &str, event: &NotifyEvent) -> Result<()> {
                self.inner.send(url, event)
            }
        }

        /// Install a fresh global mock notifier and return its handle.
        fn install_mock() -> &'static MockNotifier {
            let mock: &'static MockNotifier = Box::leak(Box::new(MockNotifier::new()));
            install_notifier(Box::new(Shim { inner: mock }));
            mock
        }

        /// All policy tests share the global notifier + STORES_NTFY_URL env.
        /// Use the process-wide notifier lock so cross-module tests that
        /// install their own mocks don't clobber each other's captures.
        fn lock() -> &'static Mutex<()> {
            crate::paths::test_notifier_lock()
        }

        /// AC5.1 case (d): integration — policy match drives daemon dispatch.
        /// An Allow policy with a matching predicate lets the row through;
        /// the same policy with a non-matching predicate falls through to
        /// default-allow.
        #[test]
        fn d_policy_match_drives_dispatch() {
            let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
            let yaml = r#"
policies:
  - id: allow-T2-fast-path
    transition: { store: tasks, from: ready, to: in_review }
    predicate: { op: "==", left: "$tier_hint", right: "T2" }
    action: allow
"#;
            let (conn, agents, policies) = fixture(yaml);
            insert_task_row(&conn, 11, "T011", "in_review", "T2", "feat/x");
            insert_history(&conn, "tasks", 11, "T011", "ready", "in_review");

            let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer", "").unwrap();
            assert_eq!(n, 1, "T2 row matches allow policy and is dispatched");
        }

        /// AC5.1 case (e): default-allow — no rule matches, row still flows.
        #[test]
        fn e_default_allow_when_no_rule_matches() {
            let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
            let (conn, agents, policies) = fixture("");
            insert_task_row(&conn, 21, "T021", "in_review", "T2", "feat/x");
            insert_history(&conn, "tasks", 21, "T021", "ready", "in_review");

            let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer", "").unwrap();
            assert_eq!(n, 1, "default-allow lets the row flow");
        }

        /// AC5.1 case (f) + AC5.2: NEVER overrides Allow → halt + ntfy fired.
        #[test]
        fn f_never_overrides_allow_and_skips_dispatch() {
            let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("STORES_NTFY_URL", "https://test.local");
            let mock = install_mock();
            let yaml = r#"
policies:
  - id: never-T3-fast-path
    transition: { store: tasks, from: ready, to: in_review }
    predicate: { op: "==", left: "$tier_hint", right: "T3" }
    action: never
  - id: allow-all
    transition: { store: tasks, from: ready, to: in_review }
    predicate: { op: "!=", left: "$tier_hint", right: "" }
    action: allow
"#;
            let (conn, agents, policies) = fixture(yaml);
            insert_task_row(&conn, 31, "T031", "in_review", "T3", "feat/x");
            insert_history(&conn, "tasks", 31, "T031", "ready", "in_review");

            let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer", "").unwrap();
            assert_eq!(n, 0, "NEVER halts dispatch (overrides Allow)");

            // No claim recorded.
            let cnt: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 31",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(cnt, 0, "halted row must not be claimed");

            // AC5.2: a single MockNotifier event with the halting policy id.
            let evs = mock.events();
            assert_eq!(evs.len(), 1, "exactly one ntfy event recorded");
            assert_eq!(evs[0].1.policy_id_or_actor_halt, "never-T3-fast-path");
            assert_eq!(evs[0].1.row_id, "T031");
            std::env::remove_var("STORES_NTFY_URL");
        }

        /// AC5.1 case (g) + AC5.3: when the dispatched subscriber writes a
        /// substrate transition, transition_history captures the policy_ref
        /// (matched id or 'default-allow') AND policies_hash. Manual writes
        /// (no env) record NULL.
        #[test]
        fn g_policy_ref_recording_on_auto_path_and_null_on_manual() {
            let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
            // Auto path: env vars set → write into transition_history.
            std::env::set_var("STORES_POLICY_REF", "allow-T1-fast-path");
            std::env::set_var("STORES_POLICIES_HASH", "deadbeef");
            let conn = fresh_db();
            // Need a real schema-driven write for this; use the same minimal
            // observations schema the transition.rs tests use.
            let schema_yaml = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_with_human
lifecycle:
  states: [open, triaged]
  transitions:
    - {from: open, to: triaged, verb: triage, actor: ai_with_human}
fields:
  - name: summary
    type: text
    required: true
"#;
            let schema = crate::schema::Schema::from_yaml(schema_yaml).unwrap();
            // Create the per-store observations table on the same conn.
            conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
                .unwrap();
            conn.execute(
                "INSERT INTO observations (display_id, status, summary) VALUES ('L001', 'open', 'x')",
                [],
            )
            .unwrap();
            let cmd = clap::Command::new("triage")
                .arg(clap::Arg::new("display_id").required(true).index(1))
                .arg(clap::Arg::new("summary").long("summary"));
            let m = cmd.get_matches_from(["triage", "L001"]);
            crate::handlers::transition::run(
                &schema,
                &conn,
                &m,
                crate::schema::actor::Actor::Human.into(),
                "triage",
            )
            .unwrap();

            let (pref, phash): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT policy_ref, policies_hash FROM transition_history \
                     WHERE store='observations' AND display_id='L001'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(pref.as_deref(), Some("allow-T1-fast-path"));
            assert_eq!(phash.as_deref(), Some("deadbeef"));

            // Manual path: clear envs → next write must record NULL.
            std::env::remove_var("STORES_POLICY_REF");
            std::env::remove_var("STORES_POLICIES_HASH");
            let schema2_yaml = r#"
name: tasks2
id_format: "T{:03d}"
default_actor: ai_with_human
lifecycle:
  states: [open, triaged]
  transitions:
    - {from: open, to: triaged, verb: triage, actor: ai_with_human}
fields:
  - name: summary
    type: text
    required: true
"#;
            let schema2 = crate::schema::Schema::from_yaml(schema2_yaml).unwrap();
            conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema2))
                .unwrap();
            conn.execute(
                "INSERT INTO tasks2 (display_id, status, summary) VALUES ('T001', 'open', 'x')",
                [],
            )
            .unwrap();
            let cmd2 = clap::Command::new("triage")
                .arg(clap::Arg::new("display_id").required(true).index(1))
                .arg(clap::Arg::new("summary").long("summary"));
            let m2 = cmd2.get_matches_from(["triage", "T001"]);
            crate::handlers::transition::run(
                &schema2,
                &conn,
                &m2,
                crate::schema::actor::Actor::Human.into(),
                "triage",
            )
            .unwrap();
            let (pref2, phash2): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT policy_ref, policies_hash FROM transition_history \
                     WHERE store='tasks2' AND display_id='T001'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert!(pref2.is_none(), "manual path: policy_ref must be NULL");
            assert!(phash2.is_none(), "manual path: policies_hash must be NULL");
        }

        /// AC5.1 case (h): ntfy mock — halt event body contains the row id
        /// and the halting policy id.
        #[test]
        fn h_ntfy_halt_event_body() {
            let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("STORES_NTFY_URL", "https://test.local");
            let mock = install_mock();
            let yaml = r#"
policies:
  - id: halt-on-empty-branch
    transition: { store: tasks, from: ready, to: in_review }
    predicate: { op: "==", left: "$branch", right: "" }
    action: halt
"#;
            let (conn, agents, policies) = fixture(yaml);
            insert_task_row(&conn, 99, "T099", "in_review", "T2", "");
            insert_history(&conn, "tasks", 99, "T099", "ready", "in_review");

            let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer", "").unwrap();
            assert_eq!(n, 0, "halt policy must skip dispatch");

            let evs = mock.events();
            assert_eq!(evs.len(), 1);
            assert_eq!(evs[0].1.row_id, "T099");
            assert_eq!(evs[0].1.policy_id_or_actor_halt, "halt-on-empty-branch");
            assert!(
                evs[0].1.transition_attempted.contains("ready"),
                "transition descriptor must mention from-state; got: {}",
                evs[0].1.transition_attempted
            );
            std::env::remove_var("STORES_NTFY_URL");
        }
    }

    /// T022 P2 / AC2.2: when a subscription's predicate evaluates false on
    /// the row, poll_once skips the claim and dispatch entirely — no
    /// dispatch_locks row, no ntfy event, no return-count bump.
    #[test]
    fn predicate_false_skips_claim() {
        let conn = fresh_db();
        // workspace_path column is what auto-drive will gate on; add it.
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN workspace_path TEXT")
            .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path) \
             VALUES (?1, ?2, 'planning', 'T2', 'feat/x', '')",
            rusqlite::params![55, "T055"],
        )
        .unwrap();
        insert_history(&conn, "tasks", 55, "T055", "", "planning");

        let mut agent = noop_agent("auto-drive", "tasks", "", "planning");
        agent.subscribes_to[0].predicate = Some(crate::flow::predicate::PredicateExpr::Neq {
            left: serde_json::json!("$workspace_path"),
            right: serde_json::json!(""),
        });
        let agents = AgentsYaml {
            agents: vec![agent],
            deployment_specialist: None,
        };
        let policies = empty_policies();

        let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer", "").unwrap();
        assert_eq!(n, 0, "predicate-false rows must not dispatch");

        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 55",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 0, "predicate-false rows must not be claimed");
    }

    /// T022 P2 / AC2.2: predicate-true → claim+dispatch fires exactly once.
    #[test]
    fn predicate_true_claims_and_dispatches() {
        let conn = fresh_db();
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN workspace_path TEXT")
            .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path) \
             VALUES (?1, ?2, 'planning', 'T2', 'feat/x', '/tmp/wt')",
            rusqlite::params![56, "T056"],
        )
        .unwrap();
        insert_history(&conn, "tasks", 56, "T056", "", "planning");

        let mut agent = noop_agent("auto-drive", "tasks", "", "planning");
        agent.subscribes_to[0].predicate = Some(crate::flow::predicate::PredicateExpr::Neq {
            left: serde_json::json!("$workspace_path"),
            right: serde_json::json!(""),
        });
        let agents = AgentsYaml {
            agents: vec![agent],
            deployment_specialist: None,
        };
        let policies = empty_policies();

        let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer", "").unwrap();
        assert_eq!(n, 1, "predicate-true row must dispatch once");

        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 56 AND agent_name='auto-drive'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);
    }

    /// T022 P4 / AC4.2: with `drive.max_parallel: 1` (default) and one drive
    /// already running, a second auto-drive dispatch is skipped pre-claim.
    /// `dispatch_locks` count for `auto-drive` must remain unchanged.
    #[test]
    fn auto_drive_cap_skips_when_full() {
        let conn = fresh_db();
        // Extend the minimal tasks table to carry workspace_path + drive_pid
        // (auto-drive's gating columns).
        conn.execute_batch(
            "ALTER TABLE tasks ADD COLUMN workspace_path TEXT;
             ALTER TABLE tasks ADD COLUMN drive_pid INTEGER;",
        )
        .unwrap();

        // Row already mid-drive: drive_pid = our own pid (alive).
        let our_pid = std::process::id() as i64;
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path, drive_pid) \
             VALUES (?1, ?2, 'executing', 'T2', 'feat/x', '/tmp/wt', ?3)",
            rusqlite::params![70, "T070", our_pid],
        )
        .unwrap();

        // Candidate row at planning awaiting auto-drive.
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path) \
             VALUES (?1, ?2, 'planning', 'T2', 'feat/y', '/tmp/wt2')",
            rusqlite::params![71, "T071"],
        )
        .unwrap();
        insert_history(&conn, "tasks", 71, "T071", "", "planning");

        let mut agent = noop_agent("auto-drive", "tasks", "", "planning");
        agent.command = "builtin:auto-drive".to_string();
        let agents = AgentsYaml {
            agents: vec![agent],
            deployment_specialist: None,
        };
        let policies = empty_policies();

        let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer", "").unwrap();
        assert_eq!(n, 0, "cap is full → no dispatch");

        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='auto-drive'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 0, "no claim must be taken when cap is full");
    }

    // ---- T026 P1: starting-line seeder tests ----

    /// Seeder inserts exactly one starting-line row per matching
    /// transition_history row across all subscriptions.
    #[test]
    fn seed_starting_line_inserts_one_per_history_row() {
        let conn = fresh_db();
        // Need an observations table for the second subscription's history rows.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_id TEXT UNIQUE NOT NULL,
                status TEXT NOT NULL
            );",
        )
        .unwrap();

        // 3 tasks ready→in_review, 2 observations confirmed→ready.
        insert_history(&conn, "tasks", 1, "T001", "ready", "in_review");
        insert_history(&conn, "tasks", 2, "T002", "ready", "in_review");
        insert_history(&conn, "tasks", 3, "T003", "ready", "in_review");
        insert_history(&conn, "observations", 1, "L001", "confirmed", "ready");
        insert_history(&conn, "observations", 2, "L002", "confirmed", "ready");

        let agents = AgentsYaml {
            agents: vec![
                noop_agent("task-watcher", "tasks", "ready", "in_review"),
                noop_agent("obs-watcher", "observations", "confirmed", "ready"),
            ],
            deployment_specialist: None,
        };

        let n = seed_starting_line(&conn, &agents, i64::MAX).unwrap();
        assert_eq!(n, 5, "should insert one row per matching history row");

        // Every newly-inserted row must carry the starting-line marker.
        let bad: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks \
                 WHERE NOT (claimed_by = 'starting-line-marker' \
                            AND last_status = 'skip-historical' \
                            AND finished_at IS NOT NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bad, 0, "every inserted row must be a starting-line marker");

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 5);
    }

    /// Re-running the seeder is a no-op: UNIQUE(store, row_id, agent_name)
    /// gives us idempotency via INSERT OR IGNORE.
    #[test]
    fn seed_starting_line_is_idempotent() {
        let conn = fresh_db();
        insert_history(&conn, "tasks", 1, "T001", "ready", "in_review");
        insert_history(&conn, "tasks", 2, "T002", "ready", "in_review");
        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };

        let n1 = seed_starting_line(&conn, &agents, i64::MAX).unwrap();
        assert_eq!(n1, 2);
        let count1: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();

        let n2 = seed_starting_line(&conn, &agents, i64::MAX).unwrap();
        assert_eq!(n2, 0, "second run inserts zero rows");
        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count1, count2, "dispatch_locks count unchanged");
    }

    /// Pre-existing real locks are NEVER overwritten by the seeder.
    #[test]
    fn seed_starting_line_never_overwrites_real_locks() {
        let conn = fresh_db();
        insert_history(&conn, "tasks", 7, "T007", "ready", "in_review");
        // Pre-insert a real claim for (tasks, row 7, 'noop').
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, \
              claimed_at, claimed_by, last_status, finished_at) \
             VALUES ('tasks', 7, 'T007', 'noop', 1, '2026-01-01T00:00:00Z', \
                     'daemon-1', 'ok', '2026-01-01T00:00:01Z')",
            [],
        )
        .unwrap();

        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };

        let n = seed_starting_line(&conn, &agents, i64::MAX).unwrap();
        assert_eq!(n, 0, "INSERT OR IGNORE skips conflicting row");

        let (claimed_by, last_status): (String, String) = conn
            .query_row(
                "SELECT claimed_by, last_status FROM dispatch_locks \
                 WHERE store='tasks' AND row_id=7 AND agent_name='noop'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(claimed_by, "daemon-1", "real lock untouched");
        assert_eq!(last_status, "ok", "real lock untouched");
    }

    /// Empty transition_history with subscribers configured → 0 rows inserted.
    #[test]
    fn seed_starting_line_no_history_no_op() {
        let conn = fresh_db();
        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };
        let n = seed_starting_line(&conn, &agents, i64::MAX).unwrap();
        assert_eq!(n, 0);
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 0);
    }

    /// L116 regression: the seeder MUST NOT claim transitions that fire
    /// between two daemon runs (e.g. user `confirm` verb between
    /// `agents run --once` calls). Realistic sequence:
    ///   1. Daemon run #1: agent has no locks → seeder seeds historical
    ///      transitions as skip-historical, then idle (no new transitions).
    ///   2. Between runs: user fires a verb, transition_history gets a NEW
    ///      row matching the agent's subscription (id > all previously-
    ///      seeded ids).
    ///   3. Daemon run #2: agent already has locks (from run #1) → seeder
    ///      MUST skip seeding entirely. The dispatcher's poll then wins
    ///      try_claim on the new transition.
    ///
    /// Pre-fix: the seeder ran on every daemon start regardless of prior
    /// state, claimed the new transition as skip-historical, and the
    /// dispatcher lost the UNIQUE(store, row_id, agent_name) race —
    /// silently swallowing the user's verb.
    #[test]
    fn seed_starting_line_skips_when_agent_already_has_locks() {
        let conn = fresh_db();
        // Run #1: pre-existing historical transition T001.
        insert_history(&conn, "tasks", 1, "T001", "ready", "in_review");
        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };
        let snap1 = snapshot_max_transition_id(&conn).unwrap();
        let n1 = seed_starting_line(&conn, &agents, snap1).unwrap();
        assert_eq!(n1, 1, "first run seeds the pre-existing historical row");

        // Between runs: a NEW transition fires.
        insert_history(&conn, "tasks", 2, "T002", "ready", "in_review");

        // Run #2: agent has locks from run #1; seeder must skip entirely.
        let snap2 = snapshot_max_transition_id(&conn).unwrap();
        let n2 = seed_starting_line(&conn, &agents, snap2).unwrap();
        assert_eq!(
            n2, 0,
            "subsequent run must skip seeding because agent already has locks (L116)"
        );

        // The new transition T002 must be UNCLAIMED so the dispatcher can win.
        let t_new_locks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE store='tasks' AND row_id=2 AND agent_name='noop'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            t_new_locks, 0,
            "post-startup transition must remain unclaimed by seeder (L116)"
        );
    }

    /// L116 corollary: a brand-new agent (never seen in dispatch_locks) DOES
    /// get its full starting-line seeded, even when other agents already
    /// have locks. Closes the L055 case (new subscriber added to running
    /// daemon) without reopening L116.
    #[test]
    fn seed_starting_line_seeds_new_agent_even_when_others_have_locks() {
        let conn = fresh_db();
        // Pre-existing transitions for two distinct subscriptions.
        insert_history(&conn, "tasks", 1, "T001", "ready", "in_review");
        insert_history(&conn, "tasks", 2, "T002", "ready", "in_review");

        // 'incumbent' has already run before — pretend it has prior locks.
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, \
              claimed_at, claimed_by, last_status, finished_at) \
             VALUES ('tasks', 1, 'T001', 'incumbent', 1, '2026-01-01T00:00:00Z', \
                     'daemon-old', 'ok', '2026-01-01T00:00:01Z')",
            [],
        )
        .unwrap();

        // Now a NEW agent 'newcomer' is added. seeder must seed for newcomer
        // but skip for incumbent.
        let agents = AgentsYaml {
            agents: vec![
                noop_agent("incumbent", "tasks", "ready", "in_review"),
                noop_agent("newcomer", "tasks", "ready", "in_review"),
            ],
            deployment_specialist: None,
        };
        let snap = snapshot_max_transition_id(&conn).unwrap();
        let n = seed_starting_line(&conn, &agents, snap).unwrap();
        assert_eq!(n, 2, "exactly 2 new rows for newcomer (T001 + T002)");

        let incumbent_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='incumbent'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(incumbent_count, 1, "incumbent untouched (no new seeds)");

        let newcomer_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='newcomer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(newcomer_count, 2, "newcomer seeded against full history");
    }

    /// snapshot_max_transition_id returns 0 on an empty table (cold start).
    #[test]
    fn snapshot_max_transition_id_returns_zero_when_empty() {
        let conn = fresh_db();
        let n = snapshot_max_transition_id(&conn).unwrap();
        assert_eq!(n, 0);
    }

    // ---- T041: retry-on-failure tests ----

    /// Helper: build an agent that runs `command` against tasks ready→in_review
    /// with a configurable retry policy.
    fn retry_agent(name: &str, command: &str, max_attempts: u32) -> AgentEntry {
        AgentEntry {
            name: name.to_string(),
            subscribes_to: vec![Subscription {
                store: "tasks".to_string(),
                transition: TransitionEdge {
                    from: "ready".to_string(),
                    to: "in_review".to_string(),
                },
                predicate: None,
            }],
            command: command.to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy {
                max_attempts,
                backoff: BackoffKind::Linear,
            },
            command_args: None,
        }
    }

    /// Force `finished_at` of the lock far into the past so the backoff
    /// window is considered elapsed regardless of BASE_BACKOFF_SECS.
    fn age_lock_finished_at(conn: &Connection, agent_name: &str) {
        conn.execute(
            "UPDATE dispatch_locks SET finished_at = '2000-01-01T00:00:00Z', next_retry_at = '2000-01-01T00:00:01Z' \
             WHERE agent_name = ?1",
            rusqlite::params![agent_name],
        )
        .unwrap();
    }

    /// AC1.2: an agent whose command fails on first attempt and succeeds on
    /// retry dispatches twice and lands in last_status='ok' with attempts=2.
    #[test]
    fn retry_then_succeed_dispatches_twice() {
        let conn = fresh_db();
        insert_task_row(&conn, 100, "T100", "in_review", "T2", "feat/x");
        insert_history(&conn, "tasks", 100, "T100", "ready", "in_review");

        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("sentinel");
        let cmd = format!(
            "if [ -f '{p}' ]; then exit 0; else touch '{p}'; exit 1; fi",
            p = sentinel.display()
        );
        let agents = AgentsYaml {
            agents: vec![retry_agent("flaky", &cmd, 3)],
            deployment_specialist: None,
        };
        let policies = empty_policies();
        let cfg = cfg_path();

        // Poll #1: first dispatch fails, attempts=1, status=exit=1.
        let n1 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n1, 1, "first poll dispatches once");

        let (attempts1, status1): (u32, String) = conn
            .query_row(
                "SELECT attempts, last_status FROM dispatch_locks WHERE agent_name='flaky'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempts1, 1);
        assert!(
            status1.starts_with("exit=") && status1 != "exit=0",
            "expected non-zero exit; got {status1}"
        );

        // Skip the backoff window.
        age_lock_finished_at(&conn, "flaky");

        // Poll #2: retry pass re-dispatches; sentinel exists → success.
        let n2 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n2, 1, "retry pass fires the second dispatch");

        let (attempts2, status2): (u32, String) = conn
            .query_row(
                "SELECT attempts, last_status FROM dispatch_locks WHERE agent_name='flaky'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempts2, 2);
        assert_eq!(status2, "ok");
    }

    /// AC1.3: retries cap at max_attempts. With max_attempts=2 and a command
    /// that always fails, exactly 2 dispatches fire; a third poll past the
    /// backoff does not re-fire.
    #[test]
    fn max_attempts_boundary_terminates() {
        let conn = fresh_db();
        insert_task_row(&conn, 200, "T200", "in_review", "T2", "feat/y");
        insert_history(&conn, "tasks", 200, "T200", "ready", "in_review");

        let agents = AgentsYaml {
            agents: vec![retry_agent("always-fail", "exit 1", 2)],
            deployment_specialist: None,
        };
        let policies = empty_policies();
        let cfg = cfg_path();

        let n1 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n1, 1);
        age_lock_finished_at(&conn, "always-fail");

        let n2 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n2, 1, "second (final) retry fires");

        let (attempts, status): (u32, String) = conn
            .query_row(
                "SELECT attempts, last_status FROM dispatch_locks WHERE agent_name='always-fail'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempts, 2, "exactly max_attempts dispatches");
        assert!(
            status.starts_with("exit=") && status != "exit=0",
            "terminal failure preserved; got {status}"
        );

        // Even past the backoff, no further retry — attempts >= max_attempts.
        age_lock_finished_at(&conn, "always-fail");
        let n3 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n3, 0, "no retry beyond max_attempts");

        let attempts_final: u32 = conn
            .query_row(
                "SELECT attempts FROM dispatch_locks WHERE agent_name='always-fail'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts_final, 2, "attempts must not bump past max");
    }

    /// AC1.4: a retry called within the backoff window must not fire; once
    /// the window has elapsed (simulated via finished_at backdate) it does.
    #[test]
    fn backoff_window_blocks_premature_retry() {
        let conn = fresh_db();
        insert_task_row(&conn, 300, "T300", "in_review", "T2", "feat/z");
        insert_history(&conn, "tasks", 300, "T300", "ready", "in_review");

        let agents = AgentsYaml {
            agents: vec![retry_agent("flaky2", "exit 1", 3)],
            deployment_specialist: None,
        };
        let policies = empty_policies();
        let cfg = cfg_path();

        let n1 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n1, 1);
        let attempts1: u32 = conn
            .query_row(
                "SELECT attempts FROM dispatch_locks WHERE agent_name='flaky2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts1, 1);

        // Immediate poll — backoff window NOT elapsed; no retry fires.
        let n2 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n2, 0, "premature retry must be skipped");
        let attempts_mid: u32 = conn
            .query_row(
                "SELECT attempts FROM dispatch_locks WHERE agent_name='flaky2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts_mid, 1, "attempts unchanged inside backoff");

        // Simulate elapsed backoff and re-poll.
        age_lock_finished_at(&conn, "flaky2");
        let n3 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n3, 1, "post-backoff retry fires");

        let attempts_after: u32 = conn
            .query_row(
                "SELECT attempts FROM dispatch_locks WHERE agent_name='flaky2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts_after, 2);
    }

    /// T050 P4 AC4.1 / Task 4.4: legacy_unknown rows are never retryable.
    #[test]
    fn t050_legacy_unknown_rows_are_not_retry_candidates() {
        let conn = fresh_db();
        insert_task_row(&conn, 390, "T390", "in_review", "T2", "feat/legacy");
        insert_history(&conn, "tasks", 390, "T390", "ready", "in_review");
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, \
             transition_id, claimed_by, claimed_at, finished_at, last_status, attempts, \
             attempt, terminal_reason, next_retry_at) \
             VALUES ('tasks', 390, 'T390', 'legacy-agent', 1, 'daemon-A', \
             '2000-01-01T00:00:00Z', '2000-01-01T00:00:01Z', 'skip-historical', 1, \
             1, 'legacy_unknown', NULL)",
            [],
        )
        .unwrap();

        assert!(
            find_retryable_locks(&conn, &retry_agent("legacy-agent", "exit 1", 3))
                .unwrap()
                .is_empty(),
            "legacy_unknown with NULL next_retry_at must not auto-retry"
        );
        assert!(
            find_retryable_locks(&conn, &retry_agent("other-agent", "exit 1", 3))
                .unwrap()
                .is_empty(),
            "legacy_unknown must not leak to any other agent"
        );
    }

    #[test]
    fn t100_migrates_old_terminal_reason_check_to_allow_rate_limit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE framework_migrations (id TEXT PRIMARY KEY, applied_at TEXT NOT NULL, note TEXT);
             CREATE TABLE dispatch_locks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                store TEXT NOT NULL,
                row_id INTEGER NOT NULL,
                display_id TEXT NOT NULL,
                agent_name TEXT NOT NULL,
                transition_id INTEGER,
                claimed_at TEXT NOT NULL,
                claimed_by TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 1,
                last_status TEXT,
                finished_at TEXT,
                daemon_epoch TEXT,
                claim_source TEXT CHECK(claim_source IN ('try_claim','retry_claim','manual','legacy')),
                attempt INTEGER,
                pid INTEGER,
                heartbeat_at TEXT,
                postcondition_id TEXT,
                postcondition_args TEXT,
                terminal_reason TEXT CHECK(terminal_reason IN ('ok','exit_nonzero','error','silent_zombie','timeout','halted','legacy_unknown')),
                next_retry_at TEXT,
                UNIQUE(store, row_id, agent_name)
             );
             INSERT INTO dispatch_locks
             (store,row_id,display_id,agent_name,transition_id,claimed_at,claimed_by,last_status,finished_at,attempts,attempt,terminal_reason,next_retry_at)
             VALUES ('tasks',809,'T809','auto-drive',1,'2026-01-01T00:00:00Z','daemon','exit=3','2026-01-01T00:00:00Z',1,1,'exit_nonzero',NULL);",
        )
        .unwrap();

        ensure_dispatch_locks_typed(&conn).unwrap();
        conn.execute(
            "UPDATE dispatch_locks SET terminal_reason='rate_limit', next_retry_at='2099-01-01T00:00:00Z' WHERE row_id=809",
            [],
        )
        .unwrap();
        let reason: String = conn
            .query_row(
                "SELECT terminal_reason FROM dispatch_locks WHERE row_id=809",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "rate_limit");
    }

    #[test]
    fn malformed_rate_limit_blocked_reason_is_not_cooldown() {
        assert_eq!(
            parse_rate_limit_blocked_reason("rate_limit:anthropic:not-an-iso8601-timestamp"),
            None
        );
        assert_eq!(
            parse_rate_limit_blocked_reason("rate_limit:anthropic:2026-01-01T00:00:00junk+"),
            None
        );
        assert_eq!(
            parse_rate_limit_blocked_reason("rate_limit:anthropic:2026-01-01T00:00:00Z"),
            Some("2026-01-01T00:00:00Z")
        );
    }

    #[test]
    fn rate_limit_blocked_reason_schedules_cooldown_without_flake_attempt_bump() {
        let conn = fresh_engine_runner_db();
        insert_task_row(&conn, 810, "T810", "blocked", "T2", "feat/rate-limit");
        conn.execute(
            "UPDATE tasks SET blocked_reason=?1 WHERE id=810",
            rusqlite::params!["rate_limit:anthropic:2099-01-01T00:00:00Z"],
        )
        .unwrap();
        insert_history(&conn, "tasks", 810, "T810", "executing", "blocked");
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store,row_id,display_id,agent_name,transition_id,claimed_at,claimed_by,last_status,finished_at,attempts,attempt,terminal_reason,next_retry_at) \
             VALUES ('tasks',810,'T810','auto-drive',1,'2026-01-01T00:00:00Z','daemon','running',NULL,1,1,NULL,NULL)",
            [],
        )
        .unwrap();
        let agent = noop_agent("auto-drive", "tasks", "executing", "blocked");
        let until = task_rate_limit_until(&conn, "tasks", 810).unwrap().unwrap();
        mark_claim_rate_limit_cooldown(&conn, "tasks", 810, "T810", &agent, &until, "exit=3")
            .unwrap();
        let row: (u32, u32, String, Option<String>) = conn
            .query_row(
                "SELECT attempts, attempt, terminal_reason, next_retry_at FROM dispatch_locks WHERE row_id=810",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, 1, "rate-limit cooldown must not bump attempts");
        assert_eq!(row.1, 1, "rate-limit cooldown must not bump attempt");
        assert_eq!(row.2, "rate_limit");
        assert_eq!(row.3.as_deref(), Some("2099-01-01T00:00:00Z"));
        assert!(find_retryable_locks(&conn, &agent).unwrap().is_empty());
    }

    #[test]
    fn elapsed_rate_limit_cooldown_is_single_cas_retry_candidate() {
        let conn = fresh_engine_runner_db();
        insert_task_row(&conn, 811, "T811", "blocked", "T2", "feat/rate-limit");
        insert_history(&conn, "tasks", 811, "T811", "executing", "blocked");
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store,row_id,display_id,agent_name,transition_id,claimed_at,claimed_by,last_status,finished_at,attempts,attempt,terminal_reason,next_retry_at) \
             VALUES ('tasks',811,'T811','auto-drive',1,'2026-01-01T00:00:00Z','daemon','exit=3','2026-01-01T00:00:00Z',9,9,'rate_limit','2000-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let agent = noop_agent("auto-drive", "tasks", "executing", "blocked");
        let candidates = find_retryable_locks(&conn, &agent).unwrap();
        assert_eq!(candidates.len(), 1, "elapsed rate-limit cooldown ignores flake max_attempts");
        assert!(claim_for_retry(&conn, &candidates[0], "auto-drive").unwrap());
        assert!(!claim_for_retry(&conn, &candidates[0], "auto-drive").unwrap());
    }

    // T085-style cascade guard: a future rate-limit timestamp produces zero retry
    // candidates on repeated polls, so no tight-loop dispatch can exceed 3 cycles.
    #[test]
    fn rate_limit_cascade_stays_parked_across_three_poll_cycles() {
        let conn = fresh_engine_runner_db();
        insert_task_row(&conn, 812, "T812", "blocked", "T2", "feat/rate-limit");
        insert_history(&conn, "tasks", 812, "T812", "executing", "blocked");
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store,row_id,display_id,agent_name,transition_id,claimed_at,claimed_by,last_status,finished_at,attempts,attempt,terminal_reason,next_retry_at) \
             VALUES ('tasks',812,'T812','auto-drive',1,'2026-01-01T00:00:00Z','daemon','exit=3','2026-01-01T00:00:00Z',1,1,'rate_limit','2099-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let agent = noop_agent("auto-drive", "tasks", "executing", "blocked");
        let mut dispatch_count = 0;
        for _ in 0..3 {
            let c = find_retryable_locks(&conn, &agent).unwrap();
            dispatch_count += c.len();
        }
        assert_eq!(dispatch_count, 0);
    }

    /// T050 P4 AC4.2: typed exit_nonzero + elapsed next_retry_at is retryable,
    /// and claim_for_retry's typed CAS admits exactly one retry claimant.
    #[test]
    fn t050_exit_nonzero_retryable_and_typed_cas_single_winner() {
        let conn = fresh_db();
        insert_task_row(&conn, 391, "T391", "in_review", "T2", "feat/retry");
        insert_history(&conn, "tasks", 391, "T391", "ready", "in_review");
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, \
             transition_id, claimed_by, claimed_at, finished_at, last_status, attempts, \
             attempt, terminal_reason, next_retry_at) \
             VALUES ('tasks', 391, 'T391', 'typed-cas', 1, 'daemon-A', \
             '2000-01-01T00:00:00Z', '2000-01-01T00:00:01Z', 'exit=1', 1, \
             1, 'exit_nonzero', '2000-01-01T00:00:02Z')",
            [],
        )
        .unwrap();

        let agent = retry_agent("typed-cas", "exit 1", 3);
        let candidates = find_retryable_locks(&conn, &agent).unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "elapsed typed failure must be retryable"
        );

        assert!(claim_for_retry(&conn, &candidates[0], "typed-cas").unwrap());
        assert!(
            !claim_for_retry(&conn, &candidates[0], "typed-cas").unwrap(),
            "second caller with stale attempt+terminal_reason snapshot must lose"
        );
    }

    /// T041 codex-revise (HIGH): two concurrent retry-claim attempts on
    /// the same dispatch_locks row must NOT both succeed. The atomic CAS
    /// in `claim_for_retry` (UPDATE...WHERE attempt + terminal_reason match
    /// the snapshot) ensures only the first caller flips last_status='retrying';
    /// the second sees affected_rows=0 and skips.
    #[test]
    fn claim_for_retry_is_atomic_cas() {
        let conn = fresh_db();
        insert_task_row(&conn, 400, "T400", "in_review", "T2", "feat/race");
        insert_history(&conn, "tasks", 400, "T400", "ready", "in_review");
        // Insert a dispatch_locks row in a "retryable" shape: attempts<max,
        // finished_at set, typed terminal failure, and next_retry_at elapsed.
        let now = crate::handlers::row::now_iso8601();
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, \
             transition_id, claimed_by, claimed_at, finished_at, last_status, attempts, \
             attempt, terminal_reason, next_retry_at) \
             VALUES ('tasks', 400, 'T400', 'flaky-race', 1, 'daemon-A', ?1, \
             '2000-01-01T00:00:00Z', 'exit=1', 1, 1, 'exit_nonzero', '2000-01-01T00:00:01Z')",
            rusqlite::params![now],
        )
        .unwrap();

        let agent = retry_agent("flaky-race", "exit 1", 3);
        assert_eq!(find_retryable_locks(&conn, &agent).unwrap().len(), 1);

        let candidate = RetryCandidate {
            store: "tasks".to_string(),
            row_id: 400,
            display_id: "T400".to_string(),
            transition_id: 1,
            attempts: 1,
            attempt_snapshot: 1,
            terminal_reason_snapshot: "exit_nonzero".to_string(),
            from_status: "ready".to_string(),
            to_status: "in_review".to_string(),
        };

        // First call wins.
        assert!(claim_for_retry(&conn, &candidate, "flaky-race").unwrap());
        let (status_after, terminal_reason, finished_at, next_retry_at): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT last_status, terminal_reason, finished_at, next_retry_at \
                 FROM dispatch_locks WHERE agent_name='flaky-race'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(status_after, "retrying");
        assert!(
            terminal_reason.is_none(),
            "retry claim leaves terminal state"
        );
        assert!(finished_at.is_none(), "retry claim clears finished_at");
        assert!(
            next_retry_at.is_none(),
            "retry claim consumes next_retry_at"
        );
        assert!(
            find_retryable_locks(&conn, &agent).unwrap().is_empty(),
            "claimed retry must not remain eligible for duplicate dispatch"
        );

        // Second call (same candidate, snapshot still 'exit=1') loses —
        // the CAS guard sees last_status='retrying' and returns false.
        assert!(!claim_for_retry(&conn, &candidate, "flaky-race").unwrap());
    }

    /// T041 codex-revise (MEDIUM): when a retry hits Decision::Halt, the
    /// notify must fire ONCE and the row's last_status must be parked at
    /// 'halted:<policy>' so subsequent polls' find_retryable_locks (which
    /// filters to error/exit statuses) does NOT re-include it. Closes the
    /// per-poll Halt-notification storm.
    #[test]
    fn halt_on_retry_parks_last_status_no_storm() {
        let conn = fresh_db();
        insert_task_row(&conn, 500, "T500", "in_review", "T2", "feat/halt");
        insert_history(&conn, "tasks", 500, "T500", "ready", "in_review");
        let now = crate::handlers::row::now_iso8601();
        // Lock in retryable shape; backoff window forced past via aged
        // finished_at (year 2000).
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, \
             transition_id, claimed_by, claimed_at, finished_at, last_status, attempts, attempt, terminal_reason, next_retry_at) \
             VALUES ('tasks', 500, 'T500', 'halted-agent', 1, 'daemon-A', ?1, '2000-01-01T00:00:00Z', 'exit=1', 1, 1, 'exit_nonzero', '2000-01-01T00:00:01Z')",
            rusqlite::params![now],
        )
        .unwrap();

        let agents = AgentsYaml {
            agents: vec![retry_agent("halted-agent", "exit 0", 5)],
            deployment_specialist: None,
        };
        // One halting policy on tasks: ready→in_review.
        let policies = PoliciesYaml {
            hash: String::new(),
            policies: vec![crate::flow::policies_yaml::PolicyEntry {
                id: "halt-all-tasks".to_string(),
                transition: crate::flow::policies_yaml::TransitionRef {
                    store: "tasks".to_string(),
                    from: "ready".to_string(),
                    to: "in_review".to_string(),
                },
                predicate: crate::flow::predicate::PredicateExpr::Eq {
                    left: serde_json::json!(1),
                    right: serde_json::json!(1),
                },
                action: crate::flow::policies_yaml::Action::Halt,
            }],
        };
        let cfg = cfg_path();

        // Poll #1: retry pass picks up the row, decides Halt, notifies once,
        // parks last_status='halted:halt-all-tasks'.
        let n1 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n1, 0, "halted retry must not count as a dispatch");
        let (status1, attempts1): (String, u32) = conn
            .query_row(
                "SELECT last_status, attempts FROM dispatch_locks WHERE agent_name='halted-agent'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status1, "halted:halt-all-tasks");
        assert_eq!(attempts1, 1, "halt does not consume an attempt");

        // Poll #2: row is no longer eligible (find_retryable_locks filters
        // to 'exit=*' / 'error:*'), so the retry pass skips it.
        let n2 = poll_once(&conn, &agents, &policies, &cfg, "test", "").unwrap();
        assert_eq!(n2, 0);
        let status2: String = conn
            .query_row(
                "SELECT last_status FROM dispatch_locks WHERE agent_name='halted-agent'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status2, "halted:halt-all-tasks",
            "no storm — state unchanged"
        );
    }

    /// T050 P3 AC3.2: ok subscriber is demoted when its named postcondition fails.
    #[test]
    fn t050_postcondition_failure_demotes_ok_to_error() {
        let conn = fresh_db();
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN drive_pid INTEGER;")
            .unwrap();
        insert_task_row(&conn, 850, "T850", "planning", "T2", "feat/x");
        let args = json!({"display_id":"T850"});
        try_claim(
            &conn,
            "tasks",
            850,
            "T850",
            "auto-drive",
            1,
            "claimer",
            "epoch",
            "try_claim",
            Some("drive_pid_recorded_or_terminal"),
            Some(&args),
        )
        .unwrap();
        let agent = retry_agent("auto-drive", "exit 0", 3);
        mark_claim_finished_typed(&conn, "tasks", 850, "T850", &agent, "ok", "ok").unwrap();
        let (reason, last): (String, String) = conn
            .query_row(
                "SELECT terminal_reason, last_status FROM dispatch_locks WHERE row_id=850",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(reason, "error");
        assert!(last.starts_with("error: check failed: "));
        let payload = last.strip_prefix("error: check failed: ").unwrap();
        let check: crate::flow::checks::CheckResult = serde_json::from_str(payload).unwrap();
        assert_eq!(check.check_id, "drive_pid_recorded_or_terminal");
        assert_eq!(check.outcome, crate::flow::checks::CheckOutcome::Fail);
        assert_eq!(
            check.args.get("display_id").and_then(|v| v.as_str()),
            Some("T850")
        );
        assert!(!check.observed_at.is_empty());
        assert!(check.reason.is_some());
        let th_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM transition_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(th_count, 0, "postcondition itself is read-only");
    }

    /// T050 P3 Task 3.6: next_retry_at is set only for retryable terminal reasons below max.
    #[test]
    fn t050_next_retry_at_by_terminal_reason() {
        let conn = fresh_db();
        let agent = retry_agent("typed-retry", "exit 1", 2);
        for (idx, reason, last, want_retry) in [
            (860, "exit_nonzero", "exit=1", true),
            (861, "error", "error: x", true),
            (862, "ok", "ok", false),
            (863, "legacy_unknown", "weird", false),
        ] {
            conn.execute(
                "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, transition_id, claimed_by, claimed_at, attempts, attempt)                  VALUES ('tasks', ?1, ?2, 'typed-retry', 1, 'daemon-A', '2000-01-01T00:00:00Z', 0, 0)",
                rusqlite::params![idx, format!("T{idx}")],
            ).unwrap();
            mark_claim_finished_typed(
                &conn,
                "tasks",
                idx,
                &format!("T{idx}"),
                &agent,
                reason,
                last,
            )
            .unwrap();
            let next: Option<String> = conn
                .query_row(
                    "SELECT next_retry_at FROM dispatch_locks WHERE row_id=?1",
                    rusqlite::params![idx],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(next.is_some(), want_retry, "reason={reason}");
        }

        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, transition_id, claimed_by, claimed_at, attempts, attempt)              VALUES ('tasks', 864, 'T864', 'typed-retry', 1, 'daemon-A', '2000-01-01T00:00:00Z', 1, 1)",
            [],
        ).unwrap();
        mark_claim_silent_zombie(
            &conn,
            "tasks",
            864,
            Some(&agent),
            "typed-retry",
            "silent_zombie_pid_dead",
        )
        .unwrap();
        let (reason, last, next): (String, String, Option<String>) = conn.query_row(
            "SELECT terminal_reason, last_status, next_retry_at FROM dispatch_locks WHERE row_id=864",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(reason, "silent_zombie");
        assert_eq!(last, "drive_failed:silent_zombie_pid_dead");
        assert!(next.is_none(), "attempt at max must not schedule retry");
    }

    #[test]
    fn t050_retry_finish_schedules_third_attempt_when_max_is_three() {
        let conn = fresh_db();
        let agent = retry_agent("typed-retry-three", "exit 1", 3);
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, transition_id, claimed_by, claimed_at, attempts, attempt) \
             VALUES ('tasks', 865, 'T865', 'typed-retry-three', 1, 'daemon-A', '2000-01-01T00:00:00Z', 1, 1)",
            [],
        )
        .unwrap();
        mark_claim_finished_typed(
            &conn,
            "tasks",
            865,
            "T865",
            &agent,
            "exit_nonzero",
            "exit=1",
        )
        .unwrap();
        let (attempts, next): (u32, Option<String>) = conn
            .query_row(
                "SELECT attempts, next_retry_at FROM dispatch_locks WHERE row_id=865",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempts, 2);
        assert!(
            next.is_some(),
            "second completed failure with max_attempts=3 must schedule third dispatch"
        );
    }

    /// T050 P3 AC3.4: mark_retry_halted writes terminal_reason='halted'.
    #[test]
    fn t050_mark_retry_halted_sets_terminal_reason() {
        let conn = fresh_db();
        insert_task_row(&conn, 851, "T851", "in_review", "T2", "feat/x");
        insert_history(&conn, "tasks", 851, "T851", "ready", "in_review");
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, transition_id, claimed_by, claimed_at, finished_at, last_status, attempts, attempt, terminal_reason, next_retry_at)              VALUES ('tasks', 851, 'T851', 'halted-agent', 1, 'daemon-A', '2000-01-01T00:00:00Z', '2000-01-01T00:00:00Z', 'exit=1', 1, 1, 'exit_nonzero', '2000-01-01T00:00:01Z')",
            [],
        ).unwrap();
        let c = RetryCandidate {
            store: "tasks".to_string(),
            row_id: 851,
            display_id: "T851".to_string(),
            transition_id: 1,
            attempts: 1,
            attempt_snapshot: 1,
            terminal_reason_snapshot: "exit_nonzero".to_string(),
            from_status: "ready".to_string(),
            to_status: "in_review".to_string(),
        };
        mark_retry_halted(&conn, &c, "halted-agent", "policy-x").unwrap();
        let reason: String = conn
            .query_row(
                "SELECT terminal_reason FROM dispatch_locks WHERE row_id=851",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "halted");
    }

    #[test]
    fn compute_backoff_secs_linear_and_exponential() {
        assert_eq!(
            compute_backoff_secs(BackoffKind::Linear, 1),
            BASE_BACKOFF_SECS
        );
        assert_eq!(
            compute_backoff_secs(BackoffKind::Linear, 3),
            BASE_BACKOFF_SECS * 3
        );
        assert_eq!(
            compute_backoff_secs(BackoffKind::Exponential, 1),
            BASE_BACKOFF_SECS
        );
        assert_eq!(
            compute_backoff_secs(BackoffKind::Exponential, 2),
            BASE_BACKOFF_SECS * 2
        );
        assert_eq!(
            compute_backoff_secs(BackoffKind::Exponential, 4),
            BASE_BACKOFF_SECS * 8
        );
    }

    #[test]
    fn parse_iso8601_to_epoch_roundtrips() {
        let s = crate::handlers::row::now_iso8601();
        let e = parse_iso8601_to_epoch(&s).unwrap();
        let now = unix_now_secs();
        assert!(
            (now - e).abs() < 5,
            "iso8601 round-trip within 5s; got s={s} e={e} now={now}"
        );
        // Known fixture.
        assert_eq!(parse_iso8601_to_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso8601_to_epoch("2000-01-01T00:00:00Z"),
            Some(946_684_800)
        );
    }

    /// SHUTDOWN flag is observed by sleep_interruptible.
    #[test]
    fn sleep_interruptible_exits_when_shutdown_set() {
        // Reset flag regardless of test order.
        SHUTDOWN.store(false, Ordering::SeqCst);
        let t = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            SHUTDOWN.store(true, Ordering::SeqCst);
        });
        let start = std::time::Instant::now();
        sleep_interruptible(5_000);
        let elapsed = start.elapsed();
        t.join().unwrap();
        SHUTDOWN.store(false, Ordering::SeqCst);
        assert!(
            elapsed < Duration::from_millis(500),
            "sleep_interruptible should return promptly when SHUTDOWN flips; elapsed={:?}",
            elapsed
        );
    }

    /// Cross-platform contract: drive_pid_exe_is_stale always returns false on
    /// non-Linux (no /proc). Gated on not-linux so it only runs off-platform.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn drive_pid_exe_is_stale_returns_false_off_linux() {
        assert!(
            !drive_pid_exe_is_stale(std::process::id() as i32),
            "drive_pid_exe_is_stale must always return false on non-Linux"
        );
    }

    #[cfg(target_os = "linux")]
    mod ticks_to_ns_tests {
        use super::super::ticks_to_ns;

        #[test]
        fn hz_zero_returns_zero() {
            assert_eq!(ticks_to_ns(12345, 0), 0);
        }

        #[test]
        fn hz_100_standard_linux() {
            // 12345 ticks * 10_000_000 ns/tick = 123_450_000_000 ns
            assert_eq!(ticks_to_ns(12345, 100), 123_450_000_000u64);
        }

        #[test]
        fn hz_300_precision() {
            // 300 ticks / 300 hz = 1 second = 1_000_000_000 ns exactly
            assert_eq!(ticks_to_ns(300, 300), 1_000_000_000u64);
        }

        #[test]
        fn hz_1_large_ticks_no_panic() {
            // ticks = u64::MAX / 1_000_000_000; result should not panic and saturates to u64::MAX
            let ticks = u64::MAX / 1_000_000_000;
            let result = ticks_to_ns(ticks, 1);
            // result = ticks * 1e9 / 1; the function must not panic (no overflow)
            let _ = result; // u64 is always <= u64::MAX by type
        }

        #[test]
        fn hz_gt_1_billion_no_zero() {
            // Verify high-hz platforms don't collapse to zero (old bug: 1e9/hz == 0 when hz > 1e9)
            // With hz=2_000_000_000, 2 ticks => 1 ns (floor), not 0
            let result = ticks_to_ns(2, 2_000_000_000);
            assert_eq!(result, 1u64);
        }
    }
}
