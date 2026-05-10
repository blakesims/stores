//! T138 P5 — end-to-end integration-lane regression suite.
//!
//! Drives the substrate's generic integration lane (`builtin:integrate`)
//! through the actual daemon dispatch path (`poll_once_with_guard`), proving:
//!
//!   * Capacity-1 serialization across simultaneous candidates.
//!   * Pre-rebase stale_base BLOCKING (Review 2 closure).
//!   * Post-refresh ER head-freshness re-check.
//!   * Typed integration_blocked routing for every failure outcome
//!     (rebase_conflict, stale_base, stale_external_review,
//!     pre_land_check_failed, merge_failure, push_failure).
//!   * Subscriber boundary at `(integrating → integrated)`: with subscriber
//!     wired the row advances; without it the row parks at `integrated`.
//!   * Single-record integration_attempts provenance with attempt_no
//!     monotonicity across retries.
//!
//! All provenance assertions use SQLite JSON functions (`json_array_length`
//! / `json_extract`) on the `tasks.integration_attempts` list_record column;
//! there is NO separate `integration_attempts` SQL table.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::{Subscription, TransitionEdge};
use stores::flow::policies_yaml::PoliciesYaml;
use stores::flow::{AgentEntry, AgentsYaml, RetryPolicy};
use stores::handlers::agents_run::{poll_once_with_guard, FsBinaryIdentityProvider};
use stores::handlers::framework_migrate::ensure_integration_singleton_index;
use stores::schema::Schema;

// ─── shared fixture helpers ───────────────────────────────────────────────

fn cwd_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    let mut full: Vec<&str> = vec!["-C", repo.to_str().unwrap()];
    full.extend_from_slice(args);
    Command::new("git").args(&full).output().unwrap()
}

fn rev_parse(repo: &Path, rev: &str) -> String {
    let out = git(repo, &["rev-parse", rev]);
    assert!(
        out.status.success(),
        "git rev-parse {} in {} failed: {}",
        rev,
        repo.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Initialize a tempdir as a single-worktree git repo with two commits on
/// `main` (init + a placeholder), returning the canonical repo path.
fn init_two_commit_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    assert!(git(&repo, &["init", "-b", "main"]).status.success());
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), "init\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "init"]);
    std::fs::write(repo.join("doc.md"), "doc\n").unwrap();
    git(&repo, &["add", "doc.md"]);
    git(&repo, &["commit", "-m", "doc"]);
    (tmp, repo)
}

/// Create branch `branch` off current main, add a non-conflicting `<file>`
/// commit, and return to main. The caller is responsible for choosing
/// unique file names so multiple branches can land in sequence.
fn add_branch_with_unique_change(repo: &Path, branch: &str, file: &str, contents: &str) {
    git(repo, &["checkout", "-b", branch]);
    std::fs::write(repo.join(file), contents).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-m", &format!("{} commit", branch)]);
    git(repo, &["checkout", "main"]);
}

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    for name in ["tasks", "external_reviews", "observations"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let s = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&s)).unwrap();
    }
    ensure_integration_singleton_index(&conn).unwrap();
    conn
}

/// Seed an `integration_queued` row plus the `(accepted, integration_queued)`
/// transition_history entry that the integrate subscription matches. The
/// daemon poll path will pick this up and dispatch `builtin:integrate`.
fn seed_queued_task(conn: &Connection, display_id: &str, branch: &str, workspace_path: &str) {
    let now = "2026-05-09T00:00:00Z";
    let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, activation, blocked_reason, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'integration_queued', 'test', 't', ?2, ?3, ?4, 'active', '', ?5, ?5, 'framework', 'framework')",
        rusqlite::params![display_id, branch, workspace_path, contract, now],
    )
    .unwrap();
    let row_id: i64 = conn
        .query_row(
            "SELECT id FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES ('tasks', ?1, ?2, 'accepted', 'integration_queued', 'enqueue-integration', 'framework', ?3)",
        rusqlite::params![row_id, display_id, now],
    )
    .unwrap();
}

fn insert_passed_er(
    conn: &Connection,
    er_id: &str,
    task_id: &str,
    attempt: i64,
    base_sha: &str,
    head_sha: &str,
) {
    let now = "2026-05-09T00:00:00Z";
    conn.execute(
        "INSERT INTO external_reviews \
         (display_id, status, task_id, attempt, adapter, base_sha, head_sha, verdict, \
          created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'passed', ?2, ?3, 'external_review', ?4, ?5, 'PASS', ?6, ?6, 'framework', 'framework')",
        rusqlite::params![er_id, task_id, attempt, base_sha, head_sha, now],
    )
    .unwrap();
}

/// Provenance helpers — both queries hit the `tasks.integration_attempts`
/// JSON array column directly (not a separate SQL table).
fn attempts_count(conn: &Connection, display_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(json_array_length(integration_attempts), 0) \
         FROM tasks WHERE display_id=?1",
        rusqlite::params![display_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn attempts_field(
    conn: &Connection,
    display_id: &str,
    idx: &str,
    field: &str,
) -> Option<String> {
    let path = if idx == "last" {
        format!("$[#-1].{}", field)
    } else {
        format!("$[{}].{}", idx, field)
    };
    conn.query_row(
        &format!(
            "SELECT json_extract(integration_attempts, '{}') FROM tasks WHERE display_id=?1",
            path
        ),
        rusqlite::params![display_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .unwrap()
}

fn attempts_field_int(
    conn: &Connection,
    display_id: &str,
    idx: &str,
    field: &str,
) -> Option<i64> {
    let path = if idx == "last" {
        format!("$[#-1].{}", field)
    } else {
        format!("$[{}].{}", idx, field)
    };
    conn.query_row(
        &format!(
            "SELECT json_extract(integration_attempts, '{}') FROM tasks WHERE display_id=?1",
            path
        ),
        rusqlite::params![display_id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .unwrap()
}

fn task_status(conn: &Connection, display_id: &str) -> String {
    conn.query_row(
        "SELECT status FROM tasks WHERE display_id=?1",
        rusqlite::params![display_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn task_blocked_reason(conn: &Connection, display_id: &str) -> String {
    let v: Option<String> = conn
        .query_row(
            "SELECT integration_blocked_reason FROM tasks WHERE display_id=?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    v.unwrap_or_default()
}

fn er_status(conn: &Connection, er_id: &str) -> String {
    conn.query_row(
        "SELECT status FROM external_reviews WHERE display_id=?1",
        rusqlite::params![er_id],
        |r| r.get(0),
    )
    .unwrap()
}

/// Build an AgentsYaml that wires only the integrate subscriber. The lane
/// ends at `integrated` because no post-integrated agent is registered.
fn integrate_only_agents(pre_land_check: &str, allow_push: bool, push_remote: &str) -> AgentsYaml {
    let mut args = serde_yaml::Mapping::new();
    args.insert(
        serde_yaml::Value::String("pre_land_check".into()),
        serde_yaml::Value::String(pre_land_check.into()),
    );
    args.insert(
        serde_yaml::Value::String("allow_push".into()),
        serde_yaml::Value::Bool(allow_push),
    );
    args.insert(
        serde_yaml::Value::String("push_remote".into()),
        serde_yaml::Value::String(push_remote.into()),
    );
    AgentsYaml {
        agents: vec![AgentEntry {
            name: "integrate".to_string(),
            subscribes_to: vec![Subscription {
                store: "tasks".to_string(),
                transition: TransitionEdge {
                    from: "accepted".to_string(),
                    to: "integration_queued".to_string(),
                },
                predicate: None,
            }],
            command: "builtin:integrate".to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy::default(),
            command_args: Some(args),
        }],
        deployment_specialist: None,
    }
}

fn empty_policies() -> PoliciesYaml {
    PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    }
}

fn cfg_path() -> PathBuf {
    PathBuf::from("/tmp/stores-test-no-config-integration-lane-e2e.yaml")
}

/// Drive `poll_once_with_guard` repeatedly until `predicate` holds or
/// `max_iters` elapse. Returns the iteration count at success. After every
/// poll we run `between_polls(conn)` so callers can sample invariants such
/// as the singleton constraint on `status='integrating'`. AC5.4 / AC5.8
/// require this helper to drive the daemon dispatch path mechanically — we
/// call `poll_once_with_guard::<FsBinaryIdentityProvider>` with `None` for
/// the exe guard, matching the daemon's invocation shape.
fn drive_daemon_until<F, B>(
    conn: &Connection,
    agents: &AgentsYaml,
    max_iters: usize,
    mut between_polls: B,
    mut predicate: F,
) -> usize
where
    F: FnMut(&Connection) -> bool,
    B: FnMut(&Connection),
{
    let policies = empty_policies();
    let cfg = cfg_path();
    for i in 0..max_iters {
        between_polls(conn);
        let _ = poll_once_with_guard::<FsBinaryIdentityProvider>(
            conn,
            agents,
            &policies,
            &cfg,
            "test-claimer",
            "epoch",
            None,
        )
        .unwrap();
        between_polls(conn);
        if predicate(conn) {
            return i + 1;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("drive_daemon_until: predicate not satisfied within {max_iters} poll iterations");
}

// ─── Task 5.2 — serialized two-candidate landing via daemon poll ─────────

#[test]
fn serialized_two_candidate_landing_via_daemon_poll() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    add_branch_with_unique_change(&repo, "feat/A", "a.txt", "a\n");
    add_branch_with_unique_change(&repo, "feat/B", "b.txt", "b\n");

    seed_queued_task(&conn, "T100", "feat/A", repo.to_str().unwrap());
    seed_queued_task(&conn, "T101", "feat/B", repo.to_str().unwrap());

    let agents = integrate_only_agents("true", false, "origin");

    use std::cell::Cell;
    let max_observed = Cell::new(0i64);
    let _iters = drive_daemon_until(
        &conn,
        &agents,
        20,
        |c| {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM tasks WHERE status='integrating'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if n > max_observed.get() {
                max_observed.set(n);
            }
        },
        |c| task_status(c, "T100") == "integrated" && task_status(c, "T101") == "integrated",
    );

    assert!(
        max_observed.get() <= 1,
        "AC5.4: COUNT(*) WHERE status='integrating' must never exceed 1 between polls; saw {}",
        max_observed.get()
    );

    // T100 (A) was seeded first → its (accepted, integration_queued)
    // transition_history row has a lower id → poll_once_with_guard
    // dispatches it first (the inner SELECT orders by id ASC). The singleton
    // index on status='integrating' then forces T101 (B) to wait until A
    // releases. Therefore B's base_main_sha must equal A's landed_main_sha.
    let a_landed = attempts_field(&conn, "T100", "last", "landed_main_sha").unwrap();
    let b_base = attempts_field(&conn, "T101", "last", "base_main_sha").unwrap();
    assert_eq!(
        b_base, a_landed,
        "AC5.4: B (T101) base_main_sha must equal A (T100) landed_main_sha — \
         B must be validated against the main head produced by A"
    );
}

// ─── Task 5.3 — stale ER head supersedes and blocks ──────────────────────

#[test]
fn stale_external_review_supersede_and_block() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    add_branch_with_unique_change(&repo, "feat/X", "x.txt", "x\n");

    // Capture the candidate's pre-advance HEAD and current main SHA.
    let er_head = rev_parse(&repo, "feat/X");
    let main_sha = rev_parse(&repo, "main");

    // Advance candidate so it diverges from the ER's recorded head.
    git(&repo, &["checkout", "feat/X"]);
    std::fs::write(repo.join("x2.txt"), "more\n").unwrap();
    git(&repo, &["add", "x2.txt"]);
    git(&repo, &["commit", "-m", "more"]);
    git(&repo, &["checkout", "main"]);

    seed_queued_task(&conn, "T200", "feat/X", repo.to_str().unwrap());
    insert_passed_er(&conn, "ER200", "T200", 1, &main_sha, &er_head);

    let agents = integrate_only_agents("true", false, "origin");
    drive_daemon_until(&conn, &agents, 5, |_| {}, |c| {
        task_status(c, "T200") == "integration_blocked"
    });

    assert_eq!(er_status(&conn, "ER200"), "superseded");
    assert_eq!(task_status(&conn, "T200"), "integration_blocked");
    assert_eq!(
        attempts_field(&conn, "T200", "last", "outcome").as_deref(),
        Some("stale_external_review")
    );
}

// ─── Task 5.4 — rebase conflict ──────────────────────────────────────────

#[test]
fn rebase_conflict_routes_to_integration_blocked() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    // Both branches touch the same file → rebase will conflict.
    git(&repo, &["checkout", "-b", "feat/conflict"]);
    std::fs::write(repo.join("doc.md"), "branch-side\n").unwrap();
    git(&repo, &["add", "doc.md"]);
    git(&repo, &["commit", "-m", "branch"]);
    git(&repo, &["checkout", "main"]);
    std::fs::write(repo.join("doc.md"), "main-side\n").unwrap();
    git(&repo, &["add", "doc.md"]);
    git(&repo, &["commit", "-m", "main change"]);
    // Worktree must be on the candidate branch for rebase to run.
    git(&repo, &["checkout", "feat/conflict"]);

    let pre_main = rev_parse(&repo, "main");
    seed_queued_task(&conn, "T300", "feat/conflict", repo.to_str().unwrap());

    let agents = integrate_only_agents("true", false, "origin");
    drive_daemon_until(&conn, &agents, 5, |_| {}, |c| {
        task_status(c, "T300") == "integration_blocked"
    });

    assert_eq!(
        attempts_field(&conn, "T300", "last", "outcome").as_deref(),
        Some("rebase_conflict")
    );
    assert_eq!(attempts_count(&conn, "T300"), 1);
    assert_eq!(rev_parse(&repo, "main"), pre_main, "main HEAD must be unchanged");
}

// ─── Task 5.5 — pre_land_check failure ───────────────────────────────────

#[test]
fn pre_land_check_fail_routes_to_integration_blocked() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    add_branch_with_unique_change(&repo, "feat/pl", "pl.txt", "pl\n");

    let pre_main = rev_parse(&repo, "main");
    let pre_branch = rev_parse(&repo, "feat/pl");
    seed_queued_task(&conn, "T400", "feat/pl", repo.to_str().unwrap());

    let agents = integrate_only_agents("false", false, "origin");
    drive_daemon_until(&conn, &agents, 5, |_| {}, |c| {
        task_status(c, "T400") == "integration_blocked"
    });

    assert_eq!(
        attempts_field(&conn, "T400", "last", "outcome").as_deref(),
        Some("pre_land_check_failed")
    );
    assert_eq!(rev_parse(&repo, "main"), pre_main, "main HEAD must be unchanged");
    // Branch left intact for diagnosis (rebase may have moved it but the
    // commit is still reachable; here no main advance happened so it's
    // unchanged).
    assert_eq!(rev_parse(&repo, "feat/pl"), pre_branch);
}

// ─── Task 5.6 — merge failure (commit injected on main between steps) ────

#[test]
fn merge_failure_routes_to_integration_blocked() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    // Branch modifies an existing tracked file so a later main-side
    // modification of the same file forces the post-rebase merge to conflict.
    git(&repo, &["checkout", "-b", "feat/mf"]);
    std::fs::write(repo.join("doc.md"), "branch-flavor\n").unwrap();
    git(&repo, &["add", "doc.md"]);
    git(&repo, &["commit", "-m", "branch flavor"]);
    git(&repo, &["checkout", "main"]);

    let pre_main = rev_parse(&repo, "main");
    seed_queued_task(&conn, "T500", "feat/mf", repo.to_str().unwrap());

    // pre_land_check command synthesizes a divergent commit on main while
    // running in the candidate worktree. The main-side commit modifies the
    // same file the candidate touches, so the integrate builtin's
    // subsequent `git merge --no-ff feat/mf` fails with a content conflict.
    let conflict_script = format!(
        "set -e; cd {repo}; \
         git checkout main; \
         echo main-flavor > doc.md; \
         git add doc.md; \
         git commit -m 'inject conflict'; \
         git checkout feat/mf",
        repo = repo.to_str().unwrap()
    );

    let agents = integrate_only_agents(&conflict_script, false, "origin");
    drive_daemon_until(&conn, &agents, 5, |_| {}, |c| {
        task_status(c, "T500") == "integration_blocked"
    });

    assert_eq!(
        attempts_field(&conn, "T500", "last", "outcome").as_deref(),
        Some("merge_failure")
    );
    // Main is allowed to have moved one commit ahead (the injected blocker)
    // but must NOT include the candidate's branch tip.
    let post_main = rev_parse(&repo, "main");
    assert_ne!(post_main, pre_main, "test setup: pre_land_check should advance main by one commit");
    let merge_base = String::from_utf8_lossy(
        &git(&repo, &["merge-base", "main", "feat/mf"]).stdout,
    )
    .trim()
    .to_string();
    assert_ne!(
        merge_base, post_main,
        "candidate must NOT have been merged: merge-base must not equal main HEAD"
    );
}

// ─── Task 5.7 — push failure ─────────────────────────────────────────────

#[test]
fn push_failure_routes_to_integration_blocked() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    add_branch_with_unique_change(&repo, "feat/pf", "pf.txt", "pf\n");

    let pre_main = rev_parse(&repo, "main");
    seed_queued_task(&conn, "T600", "feat/pf", repo.to_str().unwrap());

    // push_remote points at a path that is not a git repo — `git push` exits
    // non-zero. The integrate builtin rolls main back to base_main_sha.
    let bad_remote = "/nonexistent-bare-path-for-T138-P5";
    let agents = integrate_only_agents("true", true, bad_remote);
    drive_daemon_until(&conn, &agents, 5, |_| {}, |c| {
        task_status(c, "T600") == "integration_blocked"
    });

    assert_eq!(
        attempts_field(&conn, "T600", "last", "outcome").as_deref(),
        Some("push_failure")
    );
    let base_main = attempts_field(&conn, "T600", "last", "base_main_sha")
        .expect("integration_attempts[last].base_main_sha must be recorded for push_failure");
    assert_eq!(
        base_main, pre_main,
        "sanity: recorded base_main_sha must equal pre-test main HEAD"
    );
    let post_main = rev_parse(&repo, "main");
    assert_eq!(
        post_main, base_main,
        "AC5.6: local main HEAD must equal base_main_sha after push_failure rollback"
    );
}

// ─── Task 5.8 — stale_base via orphaned reviewed base ────────────────────

#[test]
fn stale_base_routes_to_integration_blocked_without_advancing_main() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    add_branch_with_unique_change(&repo, "feat/sb", "sb.txt", "sb\n");

    // Capture the original "main" SHA — the ER row will reference this as
    // base_sha — and the candidate's pre-test HEAD.
    let orphaned_base = rev_parse(&repo, "main");
    let pre_branch = rev_parse(&repo, "feat/sb");

    // Force-rewrite main to an orphaned history so `orphaned_base` is no
    // longer reachable from main.
    git(&repo, &["checkout", "--orphan", "fresh"]);
    git(&repo, &["rm", "-rf", "."]);
    std::fs::write(repo.join("fresh.txt"), "fresh\n").unwrap();
    git(&repo, &["add", "fresh.txt"]);
    git(&repo, &["commit", "-m", "fresh init"]);
    git(&repo, &["branch", "-M", "fresh", "main"]);
    let pre_main = rev_parse(&repo, "main");
    assert_ne!(pre_main, orphaned_base);

    seed_queued_task(&conn, "T700", "feat/sb", repo.to_str().unwrap());
    insert_passed_er(&conn, "ER700", "T700", 1, &orphaned_base, &pre_branch);

    let agents = integrate_only_agents("true", false, "origin");
    drive_daemon_until(&conn, &agents, 5, |_| {}, |c| {
        task_status(c, "T700") == "integration_blocked"
    });

    // (i) tasks.status = integration_blocked
    assert_eq!(task_status(&conn, "T700"), "integration_blocked");
    // (ii) outcome = stale_base
    assert_eq!(
        attempts_field(&conn, "T700", "last", "outcome").as_deref(),
        Some("stale_base")
    );
    // (iii) reviewed_base_sha = orphaned base
    assert_eq!(
        attempts_field(&conn, "T700", "last", "reviewed_base_sha").as_deref(),
        Some(orphaned_base.as_str())
    );
    // (iv) integration_blocked_reason contains 'stale_base'
    assert!(
        task_blocked_reason(&conn, "T700").contains("stale_base"),
        "integration_blocked_reason must contain 'stale_base'; got: {:?}",
        task_blocked_reason(&conn, "T700")
    );
    // (v) external_reviews.status = superseded
    assert_eq!(er_status(&conn, "ER700"), "superseded");
    // (vi) main HEAD unchanged
    assert_eq!(rev_parse(&repo, "main"), pre_main, "main HEAD must be unchanged");
    // (vii) candidate branch HEAD unchanged
    assert_eq!(
        rev_parse(&repo, "feat/sb"),
        pre_branch,
        "candidate branch HEAD must be unchanged"
    );
}

// ─── Task 5.9 — post-integrated handoff is repo-specific ────────────────

/// Variant B: NO post-integrated subscriber wired. The substrate's lane
/// fires `mark_integrated`, the row sits at `integrated`, and no further
/// repo-specific verb fires.
#[test]
fn post_integrated_handoff_repo_specific_variant_b_no_subscriber() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    add_branch_with_unique_change(&repo, "feat/ib", "ib.txt", "ib\n");

    seed_queued_task(&conn, "T800", "feat/ib", repo.to_str().unwrap());
    let agents = integrate_only_agents("true", false, "origin");

    drive_daemon_until(&conn, &agents, 10, |_| {}, |c| {
        task_status(c, "T800") == "integrated"
    });

    // Drive a few more poll iterations — without a subscriber the row must
    // not advance past `integrated`.
    let policies = empty_policies();
    let cfg = cfg_path();
    for _ in 0..5 {
        let _ = poll_once_with_guard::<FsBinaryIdentityProvider>(
            &conn,
            &agents,
            &policies,
            &cfg,
            "test-claimer",
            "epoch",
            None,
        )
        .unwrap();
    }
    assert_eq!(task_status(&conn, "T800"), "integrated");

    // No mark_cargo_installed transition_history row may exist for T800 —
    // proves the substrate fired no repo-specific verb.
    let n_cargo: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='tasks' AND display_id='T800' AND verb='mark_cargo_installed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        n_cargo, 0,
        "no mark_cargo_installed audit row may be present without a subscriber"
    );
}

/// Variant A: cargo-install IS subscribed to (integrating → integrated).
/// After the lane fires `mark_integrated` the cargo-install builtin runs
/// and the row advances to `cargo_installed`.
#[test]
fn post_integrated_handoff_repo_specific_variant_a_cargo_install_subscribed() {
    use std::path::PathBuf;
    let _g = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());

    // Build a workspace whose Cargo.toml names the stores crate so
    // cargo-install accepts it. Mirrors the pattern in
    // tests/flow_chain_isolation.rs.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cargo-install-noop");
    copy_dir(&fixture, &repo);
    assert!(git(&repo, &["init", "-b", "main"]).status.success());
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    add_branch_with_unique_change(&repo, "feat/ca", "ca.txt", "ca\n");

    let cargo_home = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let private_bin = cargo_home.path().join("private-daemon/bin/stores");
    std::env::set_var("CARGO_HOME", cargo_home.path());
    std::env::set_var("CARGO_TARGET_DIR", target_dir.path());
    std::env::set_var("STORES_DAEMON_BIN_PATH", &private_bin);
    std::env::set_var("STORES_BIN", env!("CARGO_BIN_EXE_stores"));

    let conn = fresh_db();
    seed_queued_task(&conn, "T801", "feat/ca", repo.to_str().unwrap());

    // Integrate + cargo-install agents.
    let mut integrate_args = serde_yaml::Mapping::new();
    integrate_args.insert(
        serde_yaml::Value::String("pre_land_check".into()),
        serde_yaml::Value::String("true".into()),
    );
    let mut cargo_args = serde_yaml::Mapping::new();
    let features = vec![serde_yaml::Value::String("runner-claude-code".into())];
    cargo_args.insert(
        serde_yaml::Value::String("features".into()),
        serde_yaml::Value::Sequence(features),
    );
    let agents = AgentsYaml {
        agents: vec![
            AgentEntry {
                name: "integrate".into(),
                subscribes_to: vec![Subscription {
                    store: "tasks".into(),
                    transition: TransitionEdge {
                        from: "accepted".into(),
                        to: "integration_queued".into(),
                    },
                    predicate: None,
                }],
                command: "builtin:integrate".into(),
                claim_window_secs: 300,
                retry_policy: RetryPolicy::default(),
                command_args: Some(integrate_args),
            },
            AgentEntry {
                name: "cargo-install".into(),
                subscribes_to: vec![Subscription {
                    store: "tasks".into(),
                    transition: TransitionEdge {
                        from: "integrating".into(),
                        to: "integrated".into(),
                    },
                    predicate: None,
                }],
                command: "builtin:cargo-install".into(),
                claim_window_secs: 600,
                retry_policy: RetryPolicy {
                    max_attempts: 1,
                    backoff: stores::flow::BackoffKind::Linear,
                },
                command_args: Some(cargo_args),
            },
        ],
        deployment_specialist: None,
    };

    // cargo-install resolves main_repo via workspace_path → must succeed.
    drive_daemon_until(&conn, &agents, 30, |_| {}, |c| {
        task_status(c, "T801") == "cargo_installed"
    });

    assert_eq!(task_status(&conn, "T801"), "cargo_installed");
    let n_cargo: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='tasks' AND display_id='T801' AND verb='mark_cargo_installed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_cargo, 1, "exactly one mark_cargo_installed audit row");

    std::env::remove_var("CARGO_HOME");
    std::env::remove_var("CARGO_TARGET_DIR");
    std::env::remove_var("STORES_DAEMON_BIN_PATH");
    std::env::remove_var("STORES_BIN");
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

// ─── Task 5.10 — integration_attempts provenance (happy path + retry) ────

#[test]
fn integration_attempts_provenance_happy_path_and_retry() {
    let conn = fresh_db();
    let (_tmp, repo) = init_two_commit_repo();
    add_branch_with_unique_change(&repo, "feat/prov-hp", "prov_hp.txt", "hp\n");
    add_branch_with_unique_change(&repo, "feat/prov-rt", "prov_rt.txt", "rt\n");

    // ── Part 1 (AC5.7 first half): single happy-path attempt → length 1.
    // T900 integrates cleanly; the provenance ledger has exactly one entry
    // with attempt_no=1 and outcome='integrated'.
    seed_queued_task(&conn, "T900", "feat/prov-hp", repo.to_str().unwrap());
    let agents_pass = integrate_only_agents("true", false, "origin");
    drive_daemon_until(&conn, &agents_pass, 5, |_| {}, |c| {
        task_status(c, "T900") == "integrated"
    });
    assert_eq!(
        attempts_count(&conn, "T900"),
        1,
        "AC5.7: after the first happy-path attempt, length must be 1"
    );
    assert_eq!(attempts_field_int(&conn, "T900", "0", "attempt_no"), Some(1));
    assert_eq!(
        attempts_field(&conn, "T900", "0", "outcome").as_deref(),
        Some("integrated"),
        "AC5.7: first happy-path attempt must record outcome='integrated'"
    );
    for f in [
        "base_main_sha",
        "candidate_head_before",
        "candidate_head_after",
        "landed_main_sha",
    ] {
        let v = attempts_field(&conn, "T900", "0", f).unwrap_or_default();
        assert!(
            !v.is_empty() && v.len() >= 7,
            "AC5.7: $[0].{} must be populated on the happy-path attempt; got: {:?}",
            f,
            v
        );
    }

    // ── Part 2 (AC5.7 second half): retry semantics — failure-then-success
    // appends a fresh entry with attempt_no=2, preserving the prior failure.
    // T901 first hits pre_land_check → integration_blocked, then we
    // synthesize the retry edge and re-poll with a passing check.
    seed_queued_task(&conn, "T901", "feat/prov-rt", repo.to_str().unwrap());
    let agents_fail = integrate_only_agents("false", false, "origin");
    drive_daemon_until(&conn, &agents_fail, 5, |_| {}, |c| {
        task_status(c, "T901") == "integration_blocked"
    });
    assert_eq!(attempts_count(&conn, "T901"), 1);
    assert_eq!(attempts_field_int(&conn, "T901", "0", "attempt_no"), Some(1));
    assert_eq!(
        attempts_field(&conn, "T901", "0", "outcome").as_deref(),
        Some("pre_land_check_failed")
    );

    // Synthesize a retry: schema declares retry-integration as ai_with_human
    // (out of reach for the framework helper). Force back to
    // integration_queued and append the recovery transition_history row so
    // the integrate subscription on (integration_blocked, integration_queued)
    // can re-dispatch.
    conn.execute(
        "UPDATE tasks SET status='integration_queued' WHERE display_id='T901'",
        [],
    )
    .unwrap();
    let row_id: i64 = conn
        .query_row(
            "SELECT id FROM tasks WHERE display_id='T901'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES ('tasks', ?1, 'T901', 'integration_blocked', 'integration_queued', \
                 'retry-integration', 'ai_with_human', '2026-05-09T01:00:00Z')",
        rusqlite::params![row_id],
    )
    .unwrap();
    // Drop the existing dispatch_locks row so the integrate agent can claim
    // again under the UNIQUE(store, row_id, agent_name) constraint. The
    // production retry path reuses the lock via claim_for_retry; for this
    // direct-driven test we clear the lock so try_claim succeeds afresh.
    conn.execute(
        "DELETE FROM dispatch_locks WHERE display_id='T901' AND agent_name='integrate'",
        [],
    )
    .unwrap();

    // Re-wire the integrate agent to subscribe to the recovery edge with a
    // passing pre_land_check.
    let mut args = serde_yaml::Mapping::new();
    args.insert(
        serde_yaml::Value::String("pre_land_check".into()),
        serde_yaml::Value::String("true".into()),
    );
    let agents_retry = AgentsYaml {
        agents: vec![AgentEntry {
            name: "integrate".into(),
            subscribes_to: vec![Subscription {
                store: "tasks".into(),
                transition: TransitionEdge {
                    from: "integration_blocked".into(),
                    to: "integration_queued".into(),
                },
                predicate: None,
            }],
            command: "builtin:integrate".into(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy::default(),
            command_args: Some(args),
        }],
        deployment_specialist: None,
    };

    drive_daemon_until(&conn, &agents_retry, 10, |_| {}, |c| {
        task_status(c, "T901") == "integrated"
    });

    // AC5.7: post-retry → length 2 with attempt_no values exactly {1, 2}.
    assert_eq!(
        attempts_count(&conn, "T901"),
        2,
        "AC5.7: after retry, length must be 2"
    );
    assert_eq!(attempts_field_int(&conn, "T901", "0", "attempt_no"), Some(1));
    assert_eq!(attempts_field_int(&conn, "T901", "1", "attempt_no"), Some(2));
    // Prior failure preserved at $[0].
    assert_eq!(
        attempts_field(&conn, "T901", "0", "outcome").as_deref(),
        Some("pre_land_check_failed"),
        "AC5.7: prior failure outcome must be preserved at $[0]"
    );
    // New success appended at $[1].
    assert_eq!(
        attempts_field(&conn, "T901", "1", "outcome").as_deref(),
        Some("integrated")
    );
    for f in [
        "base_main_sha",
        "candidate_head_before",
        "candidate_head_after",
        "landed_main_sha",
    ] {
        let v = attempts_field(&conn, "T901", "1", f).unwrap_or_default();
        assert!(
            !v.is_empty() && v.len() >= 7,
            "AC5.7: $[1].{} must be populated on the retry-success entry; got: {:?}",
            f,
            v
        );
    }
}
