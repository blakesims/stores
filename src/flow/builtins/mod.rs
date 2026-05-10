//! Built-in subscribers shipped with the autonomous flow engine.
//!
//! T138 P3: `integrate` is the generic integration-lane subscriber on
//! `(accepted → integration_queued)` and
//! `(integration_blocked → integration_queued)`. It refreshes the
//! candidate against current main, runs the configured pre-land check,
//! fast-merges into main, and fires `mark_integrated` (or
//! `mark_integration_blocked` with a typed reason). Repo-specific
//! post-`integrated` subscribers (e.g. `cargo-install` and
//! `schema-migrate` in this repo) hang off the `integrated` state.
//!
//! `accept-merge` is the pre-T138 pre-integration-lane subscriber. It is
//! no longer dispatched (see `dispatch_builtin` and the deprecation note
//! at the top of `accept_merge.rs`); the helpers in that module are
//! re-used by `integrate`.
//!
//! `user-escalation` handles `deploy_blocked` rows by filing a substrate
//! observation that points back at the blocked task.

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::flow::AgentsYaml;
use crate::handlers::row::read_row;
use crate::handlers::transition::{execute_transition_write, inject_tasks_overlay_into_diff};
use crate::schema::actor::Actor;
use crate::schema::lifecycle::select_transition;
use crate::schema::Schema;
use crate::validate::{self, EntryMap, Op};

pub mod accept_merge;
pub mod activate_queued;
pub mod auto_drive;
pub mod auto_promote;
pub mod auto_resolve_observation;
pub mod auto_scaffold;
pub mod cargo_install;
pub mod external_review;
pub mod gatekeeper_router;
pub mod gatekeeper_router_drain;
pub mod gatekeeper_stub;
pub mod integrate;
pub mod investigator;
pub mod release_to_integration;
pub mod schema_migrate;
pub mod user_escalation;

/// Context handed to a builtin at dispatch time. Lives only for the duration
/// of the builtin call.
pub struct DispatchCtx<'a> {
    pub conn: &'a Connection,
    pub agents: &'a AgentsYaml,
    pub config_path: &'a Path,
    pub policies_hash: &'a str,
}

/// Result of a builtin run. `Ok(0)` means clean success; non-zero is a soft
/// failure surfaced via `last_status`. `Err` is a hard error (logged but
/// non-fatal to the daemon loop).
pub type BuiltinResult = Result<i32>;

/// Dispatch a builtin keyword like `"builtin:integrate"`. Returns
/// `Ok(None)` for unknown keywords so the caller can fall back to the
/// shell-command path or log "unknown builtin".
///
/// T138 P3 AC3.5: the `accept-merge` keyword is intentionally NOT
/// registered here — the integrate lane (`builtin:integrate`) supersedes
/// it. `dispatch_builtin("accept-merge", …)` returns `None`. The
/// `accept_merge::run` function is still callable from legacy
/// non-subscriber callers (see `accept_merge.rs`'s deprecation note).
pub fn dispatch_builtin(keyword: &str, row: &Value, ctx: &DispatchCtx) -> Option<BuiltinResult> {
    match keyword {
        "activate-queued" => Some(activate_queued::run(row, ctx)),
        "auto-drive" => Some(auto_drive::run(row, ctx)),
        "auto-promote" => Some(auto_promote::run(row, ctx)),
        "auto-resolve-observation" => Some(auto_resolve_observation::run(row, ctx)),
        "auto-scaffold" => Some(auto_scaffold::run(row, ctx)),
        "cargo-install" => Some(cargo_install::run(row, ctx)),
        "external-review" => Some(external_review::run(row, ctx).map(|outcome| match outcome {
            external_review::DispatchOutcome::Dispatched => 0,
            external_review::DispatchOutcome::CapHeld => 0,
            external_review::DispatchOutcome::RaceLost => 0,
        })),
        "gatekeeper-stub" => Some(gatekeeper_stub::run(row, ctx)),
        "integrate" => Some(integrate::run(row, ctx)),
        "investigator" => Some(investigator::run(row, ctx)),
        "release-to-integration" => Some(release_to_integration::run(row, ctx)),
        "schema-migrate" => Some(schema_migrate::run(row, ctx)),
        "user-escalation" => Some(user_escalation::run(row, ctx)),
        _ => None,
    }
}

/// Map a built-in subscriber keyword to the postcondition_id it owns. The
/// framework reads this at registration time to stamp `dispatch_locks
/// .postcondition_id` so the row's terminal verification has a named
/// predicate to call (T050 P2). Unknown keywords return `None`.
pub fn postcondition_for_builtin(keyword: &str) -> Option<&'static str> {
    match keyword {
        "activate-queued" => Some("queued_task_released_or_blocked"),
        "auto-promote" => Some("task_exists_for_linked_observation"),
        "auto-scaffold" => Some("task_workspace_exists"),
        "auto-drive" => Some("drive_pid_recorded_or_terminal"),
        "cargo-install" => Some("cargo_installed_state"),
        "integrate" => Some("integrated_state"),
        "release-to-integration" => Some("release_to_integration_state"),
        "schema-migrate" => Some("schema_migrated_state"),
        _ => None,
    }
}

/// Resolve the main-repo working tree from `workspace_path`. Uses
/// `git rev-parse --git-common-dir` and strips the trailing `.git` segment.
/// Returns `None` if the path is not a git repo or git fails.
pub(crate) fn resolve_main_repo(workspace_path: &str) -> Option<PathBuf> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["-C", workspace_path, "rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let common = if PathBuf::from(&raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        PathBuf::from(workspace_path).join(raw)
    };
    let canon = common.canonicalize().ok()?;
    // .git → parent is the main working tree
    canon.parent().map(|p| p.to_path_buf())
}

/// Refresh a `tasks` row by display_id and return it as a JSON object.
/// Returns `None` if the row is missing or the query fails.
pub(crate) fn refresh_task_row(conn: &Connection, display_id: &str) -> Option<Value> {
    let mut stmt = conn
        .prepare("SELECT * FROM tasks WHERE display_id = ?1")
        .ok()?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(rusqlite::params![display_id]).ok()?;
    let row = rows.next().ok()??;
    let mut obj = serde_json::Map::new();
    for (i, name) in cols.iter().enumerate() {
        let v: rusqlite::types::Value = row.get(i).ok()?;
        let jv = match v {
            rusqlite::types::Value::Null => Value::Null,
            rusqlite::types::Value::Integer(i) => Value::from(i),
            rusqlite::types::Value::Real(f) => {
                Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
            }
            rusqlite::types::Value::Text(s) => Value::String(s),
            rusqlite::types::Value::Blob(b) => {
                Value::String(String::from_utf8_lossy(&b).to_string())
            }
        };
        obj.insert(name.clone(), jv);
    }
    Some(Value::Object(obj))
}

pub(crate) fn load_tasks_schema() -> Result<Schema> {
    load_store_schema("tasks")
}

/// Load a bundled store schema by name.
pub(crate) fn load_store_schema(name: &str) -> Result<Schema> {
    let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, y)| *y)
        .ok_or_else(|| anyhow!("{} bundled schema not found", name))?;
    Schema::from_yaml(yaml).with_context(|| format!("parsing bundled {} schema", name))
}

/// Fire a framework-actor transition on a `tasks` row in-process. Optional
/// `diff_extra` is merged into the write (e.g. `blocked_reason` for
/// `mark_deploy_blocked`). The transition is selected by `verb` from the
/// row's current state; the function fails if no such transition exists.
pub(crate) fn fire_framework_transition(
    conn: &Connection,
    display_id: &str,
    verb: &str,
    diff_extra: EntryMap,
    policies_hash: &str,
) -> Result<()> {
    let schema = load_tasks_schema()?;
    fire_framework_transition_for(
        conn,
        &schema,
        display_id,
        verb,
        diff_extra,
        policies_hash,
        None,
    )
}

/// Generic framework-actor transition firing for any store schema. Identical
/// semantics to `fire_framework_transition` but routed at a caller-supplied
/// schema (e.g. `observations` for the auto-resolve-observation subscriber,
/// T037 P1). All gates — declared transition lookup, validators, actor —
/// fire just as they do for tasks.
pub fn fire_framework_transition_for(
    conn: &Connection,
    schema: &Schema,
    display_id: &str,
    verb: &str,
    diff_extra: EntryMap,
    policies_hash: &str,
    actor_note: Option<&str>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut diff = diff_extra;
    let mut merged = existing.clone();
    for (k, v) in &diff {
        merged.insert(k.clone(), v.clone());
    }

    if schema.name == "tasks" && verb == "release-to-integration" {
        let policy = merged
            .get("human_acceptance_policy")
            .and_then(Value::as_str)
            .unwrap_or("optional");
        let decided = merged.get("acceptance_decided_by").and_then(Value::as_str);
        if policy == "required" && decided != Some("human") {
            anyhow::bail!("human acceptance required but not recorded");
        }
        if policy == "delegated_by_policy" && !matches!(decided, Some("human" | "policy_delegate")) {
            anyhow::bail!("delegated acceptance requires acceptance_decided_by");
        }
    }

    let transition = select_transition(
        &schema.lifecycle.transitions,
        &current_status,
        verb,
        None,
        &merged,
    )?;

    validate::validate(
        schema,
        &merged,
        Op::Transition(verb.to_string(), diff.clone()),
        Actor::Framework.into(),
    )
    .map_err(|errs| {
        anyhow!(
            "{} validation failed:\n{}",
            verb,
            validate::pretty_print(&errs)
        )
    })?;

    inject_tasks_overlay_into_diff(
        schema,
        verb,
        &current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;

    let phash_opt = if policies_hash.is_empty() {
        None
    } else {
        Some(policies_hash)
    };
    execute_transition_write(
        &tx,
        schema,
        row_id,
        display_id,
        &current_status,
        &transition.to,
        verb,
        &diff,
        &merged,
        Actor::Framework,
        None,
        phash_opt,
        actor_note,
    )?;

    tx.commit()?;
    Ok(())
}

/// Convenience: fire `mark_deploy_blocked` with `blocked_reason` populated.
pub(crate) fn fire_mark_deploy_blocked(
    conn: &Connection,
    display_id: &str,
    blocked_reason: &str,
    policies_hash: &str,
) -> Result<()> {
    fire_mark_deploy_blocked_with_note(conn, display_id, blocked_reason, policies_hash, None)
}

/// Variant of `fire_mark_deploy_blocked` that records `actor_note` on the
/// emitted `transition_history` row. Used by the framework subscriber-runner
/// (T046) to encode the failed agent + exit code on the audit row.
pub(crate) fn fire_mark_deploy_blocked_with_note(
    conn: &Connection,
    display_id: &str,
    blocked_reason: &str,
    policies_hash: &str,
    actor_note: Option<&str>,
) -> Result<()> {
    let mut diff: EntryMap = std::collections::BTreeMap::new();
    diff.insert(
        "blocked_reason".to_string(),
        Value::String(blocked_reason.to_string()),
    );
    let schema = load_tasks_schema()?;
    fire_framework_transition_for(
        conn,
        &schema,
        display_id,
        "mark_deploy_blocked",
        diff,
        policies_hash,
        actor_note,
    )
}

/// Convenience: fire `mark_drive_failed` with `blocked_reason` populated.
/// Used by the auto-drive subscriber when the drive subprocess exits non-zero
/// or the wrap envelope never lands. Mirrors `fire_mark_deploy_blocked`.
///
/// `detection_reason`, when `Some`, suffix-tags the stored `blocked_reason`
/// as `<base>:<detection_reason>` (e.g. `drive_failed:silent_zombie_pid_dead`),
/// making silent-zombie watchdog flips mechanically distinguishable from
/// generic drive failures in `tasks.blocked_reason` and downstream audits.
pub(crate) fn fire_mark_drive_failed(
    conn: &Connection,
    display_id: &str,
    blocked_reason: &str,
    policies_hash: &str,
    detection_reason: Option<&str>,
) -> Result<()> {
    let mut diff: EntryMap = std::collections::BTreeMap::new();
    let value = match detection_reason {
        Some(suffix) if !suffix.is_empty() => format!("{}:{}", blocked_reason, suffix),
        _ => blocked_reason.to_string(),
    };
    diff.insert("blocked_reason".to_string(), Value::String(value));
    fire_framework_transition(conn, display_id, "mark_drive_failed", diff, policies_hash)
}

/// Dispatch the row to the configured `deployment_specialist` (default
/// `builtin:user-escalation`). Used after a builtin flips a row to
/// `deploy_blocked`. The `caller` tag is used in error logs.
pub(crate) fn dispatch_to_specialist(
    row: &Value,
    ctx: &DispatchCtx,
    display_id: &str,
    caller: &str,
) {
    let spec_name = ctx
        .agents
        .deployment_specialist
        .as_deref()
        .unwrap_or("builtin:user-escalation");

    let refreshed = refresh_task_row(ctx.conn, display_id).unwrap_or_else(|| row.clone());

    if let Some(kw) = spec_name.strip_prefix("builtin:") {
        if let Some(res) = dispatch_builtin(kw, &refreshed, ctx) {
            if let Err(e) = res {
                eprintln!("[{}] specialist '{}' failed: {}", caller, spec_name, e);
            }
        } else {
            eprintln!("[{}] unknown builtin specialist: {}", caller, spec_name);
        }
        return;
    }

    if let Some(agent) = ctx.agents.agents.iter().find(|a| a.name == spec_name) {
        if let Some(kw) = agent.command.strip_prefix("builtin:") {
            if let Some(Err(e)) = dispatch_builtin(kw, &refreshed, ctx) {
                eprintln!("[{}] specialist '{}' failed: {}", caller, agent.name, e);
            }
        } else {
            let _ = Command::new("sh")
                .arg("-c")
                .arg(&agent.command)
                .env("STORES_DISPLAY_ID", display_id)
                .env("STORES_STORE", "tasks")
                .status();
        }
    } else {
        eprintln!(
            "[{}] deployment_specialist '{}' not found in agents.yaml",
            caller, spec_name
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::flow::agents_yaml::TransitionEdge;
    use crate::flow::{
        install_notifier, AgentEntry, BackoffKind, MockNotifier, NotifierBackend, NotifyEvent,
        RetryPolicy, Subscription,
    };
    use crate::schema::Schema;
    use rusqlite::Connection;
    use std::process::Command;
    use std::sync::Mutex;

    /// All builtin tests share the global notifier + STORES_NTFY_URL env;
    /// serialize them via the process-wide notifier lock so cross-module
    /// tests (e.g. `handlers::agents_run::tests::policy`) that also install
    /// mocks don't clobber each other's captured events.
    pub(crate) fn lock() -> &'static Mutex<()> {
        crate::paths::test_notifier_lock()
    }

    struct Shim {
        inner: &'static MockNotifier,
    }
    impl NotifierBackend for Shim {
        fn send(&self, url: &str, event: &NotifyEvent) -> Result<()> {
            self.inner.send(url, event)
        }
    }

    fn install_mock() -> &'static MockNotifier {
        let mock: &'static MockNotifier = Box::leak(Box::new(MockNotifier::new()));
        install_notifier(Box::new(Shim { inner: mock }));
        mock
    }

    fn fresh_db_with_tasks() -> (Connection, Schema, Schema) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();

        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .unwrap();
        let tasks = Schema::from_yaml(tasks_yaml).unwrap();
        conn.execute_batch(&ddl_for(&tasks)).unwrap();

        let obs_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "observations")
            .map(|(_, y)| *y)
            .unwrap();
        let obs = Schema::from_yaml(obs_yaml).unwrap();
        conn.execute_batch(&ddl_for(&obs)).unwrap();

        (conn, tasks, obs)
    }

    fn git(repo: &Path, args: &[&str]) -> std::process::Output {
        let mut full: Vec<&str> = vec!["-C", repo.to_str().unwrap()];
        full.extend_from_slice(args);
        Command::new("git").args(&full).output().unwrap()
    }

    /// Init a temp repo with a main branch holding `file.txt = "main\n"`.
    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let g = git(&repo, &["init", "-b", "main"]);
        assert!(g.status.success(), "git init failed: {:?}", g);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("file.txt"), "main\n").unwrap();
        git(&repo, &["add", "file.txt"]);
        git(&repo, &["commit", "-m", "init"]);
        (tmp, repo)
    }

    fn insert_accepted_task(
        conn: &Connection,
        display_id: &str,
        branch: &str,
        workspace_path: &str,
    ) -> i64 {
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'accepted', 'test', 't', ?2, ?3, ?4, ?5, ?5, 'framework', 'framework')",
            rusqlite::params![display_id, branch, workspace_path, contract, now],
        ).unwrap();
        conn.last_insert_rowid()
    }

    /// T138 P3: post-Phase-1 the cargo-install subscriber expects the row to
    /// be at `integrated` (not `accepted`); this helper seeds rows directly
    /// into that state so the cargo-install tests reflect the new source.
    fn insert_integrated_task(
        conn: &Connection,
        display_id: &str,
        branch: &str,
        workspace_path: &str,
    ) -> i64 {
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'integrated', 'test', 't', ?2, ?3, ?4, ?5, ?5, 'framework', 'framework')",
            rusqlite::params![display_id, branch, workspace_path, contract, now],
        ).unwrap();
        conn.last_insert_rowid()
    }

    fn task_row_json(conn: &Connection, display_id: &str) -> Value {
        let mut stmt = conn
            .prepare("SELECT * FROM tasks WHERE display_id = ?1")
            .unwrap();
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt.query(rusqlite::params![display_id]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i).unwrap();
            let jv = match v {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(i) => Value::from(i),
                rusqlite::types::Value::Real(f) => {
                    Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
                }
                rusqlite::types::Value::Text(s) => Value::String(s),
                rusqlite::types::Value::Blob(b) => {
                    Value::String(String::from_utf8_lossy(&b).to_string())
                }
            };
            obj.insert(name.clone(), jv);
        }
        Value::Object(obj)
    }

    fn empty_agents_yaml() -> AgentsYaml {
        AgentsYaml::default_empty()
    }

    fn cfg_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp/stores-test-no-config.yaml")
    }

    /// AC6.2 / test (i): clean merge keeps row at `accepted` and produces a
    /// merge commit on main.
    #[test]
    fn i_accept_merge_clean() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let (_tmp, repo) = init_repo();
        // Branch with a non-conflicting addition.
        git(&repo, &["checkout", "-b", "feat/x"]);
        std::fs::write(repo.join("feat.txt"), "feat\n").unwrap();
        git(&repo, &["add", "feat.txt"]);
        git(&repo, &["commit", "-m", "feat"]);
        git(&repo, &["checkout", "main"]);

        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_accepted_task(&conn, "T100", "feat/x", repo.to_str().unwrap());
        let row = task_row_json(&conn, "T100");
        let agents = empty_agents_yaml();
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };

        let res = accept_merge::run(&row, &ctx).unwrap();
        assert_eq!(res, 0);

        // Row still accepted.
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T100'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "accepted");

        // HEAD on main is a merge commit (two parents).
        let log = git(&repo, &["log", "--oneline", "--merges", "-n", "1"]);
        let s = String::from_utf8_lossy(&log.stdout);
        assert!(
            !s.trim().is_empty(),
            "expected a merge commit on main; git log --merges output: {s:?}"
        );
    }

    // T138 P3: removed `j_accept_merge_conflict_and_ntfy` and
    // `k_deploy_blocked_transition_mechanics`. Both exercised the
    // accept-merge → deploy_blocked routing on `accepted` rows. Phase 1
    // dropped the `(accepted, deploy_blocked)` schema edge (the integration
    // lane now owns merge/refresh; cargo_installed→deploy_blocked is the
    // only mark_deploy_blocked source). The integrate-lane equivalents are
    // covered by `flow::builtins::integrate` tests; the cargo-install
    // failure surface is covered by
    // `j_cargo_install_failure_surfaces_error_and_does_not_promote`.

    /// T046 (T138 P3 update): a subscriber that exits non-zero on the
    /// (integrated → cargo_installed) edge must be routed to deploy_blocked by
    /// the framework subscriber-runner, with the exit code recorded in
    /// transition_history.actor_note. Pre-T138 this scenario tested the
    /// `accept-merge` failure on (in_review → accepted); post-Phase-1 the
    /// schema's only `mark_deploy_blocked` source is `cargo_installed`, fired
    /// when schema-migrate fails on the `(cargo_installed → schema_migrated)`
    /// edge. The route_failure logic itself is unchanged — only the source
    /// state and subscriber name shift.
    #[test]
    fn t046_subscriber_nonzero_exit_routes_to_deploy_blocked() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _t, _o) = fresh_db_with_tasks();
        // Row sits at `cargo_installed` (post integrated→cargo_installed), as
        // it would be when schema-migrate claimed the dispatch and is about to run.
        insert_cargo_installed_task(&conn, "T046", "/tmp/no-such-workspace");

        // Stub the subscriber-runner's failure path: row is at `cargo_installed`,
        // dispatch returned exit=11. The framework must fire mark_deploy_blocked
        // and stamp actor_note with the exit code. The schema-migrate
        // subscription edge in the post-T138 agents.yaml is
        // (integrated → cargo_installed) — i.e. subscription.to = "cargo_installed",
        // matching current_status so the route gate fires.
        crate::handlers::agents_run::route_failure_to_deploy_blocked(
            &conn,
            "tasks",
            "T046",
            "schema-migrate",
            "exit=11",
            "feedface",
            "integrated",
        );

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T046'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            status, "integrated",
            "non-zero subscriber exit must keep generic status integrated"
        );
        let post_integration_step: String = conn.query_row("SELECT post_integration_step FROM tasks WHERE display_id='T046'", [], |r| r.get(0)).unwrap();
        assert_eq!(post_integration_step, "deploy_blocked");
        let reason = reason.unwrap_or_default();
        assert!(
            reason.contains("schema-migrate") && reason.contains("exit=11"),
            "blocked_reason must cite agent + exit code; got: {reason}"
        );

        let (verb, invoker, note, phash): (String, String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT verb, invoker, actor_note, policies_hash FROM transition_history \
                 WHERE store='tasks' AND display_id='T046' AND verb='mark_deploy_blocked'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(verb, "mark_deploy_blocked");
        assert_eq!(invoker, "framework");
        let note = note.unwrap_or_default();
        assert!(
            note.contains("agent=schema-migrate") && note.contains("exit=11"),
            "actor_note must record agent + exit code; got: {note}"
        );
        assert_eq!(phash.as_deref(), Some("feedface"));
    }

    /// T046: zero-exit must NOT trigger any transition. Row stays at accepted.
    #[test]
    fn t046_subscriber_zero_exit_no_transition() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_accepted_task(&conn, "T046b", "feat/x", "/tmp/no-such");

        // Even though the helper is only invoked from the failure path, guard
        // against accidental wiring by asserting the routing function itself
        // is a no-op when the row's current state has no mark_deploy_blocked
        // transition declared. We simulate that by feeding an unrelated state
        // through a non-tasks store name (the function early-returns).
        crate::handlers::agents_run::route_failure_to_deploy_blocked(
            &conn,
            "observations",
            "T046b",
            "some-agent",
            "exit=1",
            "",
            "accepted",
        );
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T046b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "accepted", "non-tasks-store path must no-op");
    }

    /// T046 codex-revise: subscription_to MUST match current_status before the
    /// framework routes failure to deploy_blocked. Closes the prior cycle's
    /// HIGH finding that the routing was over-broad — any tasks-store
    /// subscriber failing while a row sat at `accepted` would have triggered
    /// mark_deploy_blocked, even subscribers whose own transition.to was a
    /// different state (e.g. a hypothetical subscriber on tasks: ready→executing
    /// that happened to fail-and-fire while a different row was at accepted).
    /// The tightened gate pins routing to the subscriber that LANDED the row
    /// in the from-state of the deploy_blocked edge.
    #[test]
    fn t046_subscription_to_mismatch_does_not_route() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_accepted_task(&conn, "T046c", "feat/x", "/tmp/no-such");

        // Subscriber claims to have fired on `ready → executing` (subscription_to
        // = "executing"), but the row is currently at `accepted`. The gate
        // must short-circuit BEFORE firing mark_deploy_blocked, leaving the
        // row at accepted and writing no transition_history row for
        // mark_deploy_blocked.
        crate::handlers::agents_run::route_failure_to_deploy_blocked(
            &conn,
            "tasks",
            "T046c",
            "some-other-subscriber",
            "exit=7",
            "",
            "executing", // subscription_to ≠ current_status (accepted)
        );
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T046c'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "accepted",
            "subscription_to mismatch must NOT route the row to deploy_blocked"
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE display_id='T046c' AND verb='mark_deploy_blocked'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "no mark_deploy_blocked transition_history row may be written when subscription_to mismatches"
        );
    }

    // T138 P3: removed `accept_merge_test_helper_fire` (the only caller was
    // `k_deploy_blocked_transition_mechanics`, which itself was removed when
    // the (accepted, deploy_blocked) schema edge was retired in Phase 1).

    /// Copy a tests/fixtures cargo project into a fresh temp dir and `git
    /// init` the result so `resolve_main_repo` finds it. Returns the
    /// tempdir handle (drop deletes) + the resolved repo path.
    fn init_cargo_repo(fixture: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        copy_dir(&src, &repo);

        let g = git(&repo, &["init", "-b", "main"]);
        assert!(g.status.success(), "git init failed: {:?}", g);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        (tmp, repo)
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

    /// AC2.2 / test (i): cargo-install succeeds on a clean fixture, fires
    /// `mark_cargo_installed` (framework actor), and the row advances from
    /// `integrated` to `cargo_installed` (T138 P3: source state is
    /// `integrated`, not `accepted`).
    #[test]
    fn i_cargo_install_clean_chains_to_mark_cargo_installed() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let (_tmp, repo) = init_cargo_repo("cargo-install-noop");

        let cargo_home = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        std::env::set_var("CARGO_HOME", cargo_home.path());
        std::env::set_var("CARGO_TARGET_DIR", target_dir.path());
        let private_bin = cargo_home.path().join("private-daemon/bin/stores");
        std::env::set_var("STORES_DAEMON_BIN_PATH", &private_bin);

        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_integrated_task(&conn, "T400", "feat/x", repo.to_str().unwrap());
        let row = task_row_json(&conn, "T400");
        let agents = empty_agents_yaml();
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };

        let res = cargo_install::run(&row, &ctx).unwrap();
        assert_eq!(res, 0);

        let (status, post_integration_step): (String, String) = conn
            .query_row(
                "SELECT status, post_integration_step FROM tasks WHERE display_id='T400'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "integrated");
        assert_eq!(post_integration_step, "cargo_installed");

        let (verb, invoker): (String, String) = conn
            .query_row(
                "SELECT verb, invoker FROM transition_history \
                 WHERE store='tasks' AND display_id='T400' AND verb='mark_cargo_installed'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(verb, "mark_cargo_installed");
        assert_eq!(invoker, "framework");
        assert!(
            private_bin.exists(),
            "cargo-install must promote to private daemon binary path"
        );
        crate::handlers::agents_run::validate_stores_binary_candidate(&private_bin)
            .expect("promoted private daemon binary must validate");

        std::env::remove_var("CARGO_HOME");
        std::env::remove_var("CARGO_TARGET_DIR");
        std::env::remove_var("STORES_DAEMON_BIN_PATH");
    }

    /// AC2.3 / test (j): cargo-install fails on a fixture with a deliberate
    /// compile error. T138 codex-revise: cargo-install now runs from
    /// `integrated`, and build/candidate failures must route to deploy_blocked
    /// rather than leaving the row stranded at integrated.
    #[test]
    fn j_cargo_install_failure_routes_to_deploy_blocked_and_does_not_promote() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("STORES_NTFY_URL", "https://test.local");
        let _mock = install_mock();

        let (_tmp, repo) = init_cargo_repo("cargo-install-broken");
        let cargo_home = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        std::env::set_var("CARGO_HOME", cargo_home.path());
        std::env::set_var("CARGO_TARGET_DIR", target_dir.path());
        let private_bin = cargo_home.path().join("private-daemon/bin/stores");
        std::env::set_var("STORES_DAEMON_BIN_PATH", &private_bin);
        std::fs::create_dir_all(private_bin.parent().unwrap()).unwrap();
        std::fs::write(&private_bin, "existing-private-binary\n").unwrap();
        let existing_private = std::fs::read(&private_bin).unwrap();

        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_integrated_task(&conn, "T401", "feat/y", repo.to_str().unwrap());
        let row = task_row_json(&conn, "T401");
        let agents = AgentsYaml {
            agents: vec![],
            deployment_specialist: None,
        };
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "cafebabe",
        };

        let res = cargo_install::run(&row, &ctx);
        assert!(
            res.is_ok(),
            "cargo-install build failure from `integrated` must route to deploy_blocked; got {res:?}"
        );

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T401'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "integrated",
            "row must keep generic status integrated on cargo-install failure"
        );
        let post_integration_step: String = conn.query_row("SELECT post_integration_step FROM tasks WHERE display_id='T401'", [], |r| r.get(0)).unwrap();
        assert_eq!(post_integration_step, "deploy_blocked");

        assert_eq!(
            std::fs::read(&private_bin).unwrap(),
            existing_private,
            "cargo/candidate failure must not promote over existing private binary"
        );

        std::env::remove_var("STORES_NTFY_URL");
        std::env::remove_var("CARGO_HOME");
        std::env::remove_var("CARGO_TARGET_DIR");
        std::env::remove_var("STORES_DAEMON_BIN_PATH");
    }

    #[test]
    fn cargo_install_bad_candidate_does_not_promote_and_blocks() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"stores\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\
             [[bin]]\nname = \"stores\"\npath = \"src/main.rs\"\n\
             [features]\ndefault = []\nrunner-claude-code = []\n",
        )
        .unwrap();
        std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
        assert!(git(&repo, &["init", "-b", "main"]).status.success());
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);

        let cargo_home = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        let private_bin = cargo_home.path().join("private-daemon/bin/stores");
        std::fs::create_dir_all(private_bin.parent().unwrap()).unwrap();
        std::fs::write(&private_bin, "existing-private-binary\n").unwrap();
        let existing_private = std::fs::read(&private_bin).unwrap();
        std::env::set_var("CARGO_HOME", cargo_home.path());
        std::env::set_var("CARGO_TARGET_DIR", target_dir.path());
        std::env::set_var("STORES_DAEMON_BIN_PATH", &private_bin);

        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_integrated_task(&conn, "T402", "feat/z", repo.to_str().unwrap());
        let row = task_row_json(&conn, "T402");
        let agents = AgentsYaml {
            agents: vec![],
            deployment_specialist: None,
        };
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };

        // T138 codex-revise: cargo-install on `integrated` routes candidate
        // validation failures to deploy_blocked without promoting over the
        // existing private binary.
        let res = cargo_install::run(&row, &ctx);
        assert!(
            res.is_ok(),
            "cargo-install with an invalid candidate from `integrated` must route to deploy_blocked; got {res:?}"
        );
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T402'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "integrated",
            "row must keep generic status integrated when candidate validation fails"
        );
        let post_integration_step: String = conn.query_row("SELECT post_integration_step FROM tasks WHERE display_id='T402'", [], |r| r.get(0)).unwrap();
        assert_eq!(post_integration_step, "deploy_blocked");
        assert_eq!(std::fs::read(&private_bin).unwrap(), existing_private);

        std::env::remove_var("CARGO_HOME");
        std::env::remove_var("CARGO_TARGET_DIR");
        std::env::remove_var("STORES_DAEMON_BIN_PATH");
    }

    /// AC6.4 / test (l): user-escalation files exactly one observation row
    /// whose body cites the blocked task's display_id.
    #[test]
    fn l_user_escalation_files_observation() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _t, _o) = fresh_db_with_tasks();

        // Insert a deploy_blocked task directly.
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, blocked_reason, contract, created_at, updated_at, created_by, updated_by) \
             VALUES ('T300', 'deploy_blocked', 't', 't', 'feat/blocked', 'conflict on a.rs, b.rs', ?1, ?2, ?2, 'framework', 'framework')",
            rusqlite::params![contract, now],
        ).unwrap();
        let row = task_row_json(&conn, "T300");

        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 0);

        let agents = AgentsYaml::default_empty();
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };

        user_escalation::run(&row, &ctx).unwrap();

        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 1, "exactly one observation must be filed");

        let (display_id, body, task_id, status): (String, String, String, String) = conn
            .query_row(
                "SELECT display_id, body, task_id, status FROM observations LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert!(
            display_id.starts_with("L"),
            "minted L-id; got: {display_id}"
        );
        assert_eq!(status, "open");
        assert_eq!(
            task_id, "T300",
            "task_id soft-FK must point at blocked task"
        );
        assert!(
            body.contains("T300"),
            "observation body must cite blocked task display_id; got: {body}"
        );
    }

    // -----------------------------------------------------------------
    // Phase 3: builtin:schema-migrate
    //
    // The success-path no-op and applies-new-columns scenarios are covered
    // end-to-end in `tests/schema_migrate_post_accept_e2e.rs` (T031 P2).
    // Subprocess-driven migration cannot be unit-tested here because
    // `env!("CARGO_BIN_EXE_stores")` is only set for integration test
    // crates — lib unit tests have no path to the freshly-built binary.
    // The failure path (subprocess error → deploy_blocked) remains testable
    // here via a deliberately-bad manifest, since the failure surfaces
    // before the subprocess succeeds.
    // -----------------------------------------------------------------

    /// Insert a task already in `cargo_installed` status (the precondition
    /// for the schema-migrate subscriber).
    fn insert_cargo_installed_task(
        conn: &Connection,
        display_id: &str,
        workspace_path: &str,
    ) -> i64 {
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, lifecycle, active_step, integration_step, post_integration_step, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'integrated', 'test', 't', 'feat/x', ?2, ?3, 'done', 'none', 'none', 'cargo_installed', ?4, ?4, 'framework', 'framework')",
            rusqlite::params![display_id, workspace_path, contract, now],
        ).unwrap();
        conn.last_insert_rowid()
    }

    /// AC3.4 / test (e): migrate failure — manifest references a missing
    /// bundled store; `apply_at` errors; row→deploy_blocked with the migrate
    /// error captured in `blocked_reason` and an ntfy event captured.
    #[test]
    fn e_schema_migrate_failure_blocks() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let mock = install_mock();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let stores_bin = root.join("stores-migrate-fails.sh");
        std::fs::write(
            &stores_bin,
            "#!/usr/bin/env bash\necho 'schema path bundled:does-not-exist not found' >&2\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stores_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let prev_stores_bin_outer = std::env::var_os("STORES_BIN");
        std::env::set_var("STORES_BIN", &stores_bin);
        let stores_dir = root.join(".stores");
        std::fs::create_dir_all(&stores_dir).unwrap();
        let fake_stores = tmp.path().join("stores-fail.sh");
        std::fs::write(
            &fake_stores,
            "#!/usr/bin/env bash\necho 'bundled store does-not-exist not found' >&2\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_stores).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_stores, perms).unwrap();
        }
        let prev_stores_bin = std::env::var("STORES_BIN").ok();
        std::env::set_var("STORES_BIN", &fake_stores);
        // Use a config file (immune to STORES_NTFY_URL env races across modules).
        let cfg_file = tmp.path().join("config.yaml");
        std::fs::write(&cfg_file, "ntfy:\n  url: https://test.local\n").unwrap();
        let stores_bin = tmp.path().join("stores-migrate-fails.sh");
        std::fs::write(
            &stores_bin,
            "#!/bin/sh\necho 'bundled store does-not-exist not found' >&2\nexit 1\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&stores_bin).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
            std::fs::set_permissions(&stores_bin, perms).unwrap();
        }
        std::env::set_var("STORES_BIN", stores_bin.to_string_lossy().to_string());

        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_cargo_installed_task(&conn, "T502", root.to_str().unwrap());
        let row = task_row_json(&conn, "T502");
        let agents = AgentsYaml {
            agents: vec![],
            deployment_specialist: None,
        };
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg_file,
            policies_hash: "deadbeef",
        };

        schema_migrate::run(&row, &ctx).unwrap();
        match prev_stores_bin_outer {
            Some(value) => std::env::set_var("STORES_BIN", value),
            None => std::env::remove_var("STORES_BIN"),
        }

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T502'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "integrated");
        let post_integration_step: String = conn.query_row("SELECT post_integration_step FROM tasks WHERE display_id='T502'", [], |r| r.get(0)).unwrap();
        assert_eq!(post_integration_step, "deploy_blocked");
        let reason = reason.unwrap_or_default();
        assert!(
            reason.contains("schema-migrate failed"),
            "blocked_reason must carry migrate error; got: {reason}"
        );
        assert!(
            reason.contains("does-not-exist"),
            "blocked_reason must reference the failing store; got: {reason}"
        );

        let evs = mock.events();
        assert!(
            evs.iter()
                .any(|(_, e)| e.row_id == "T502"
                    && e.transition_attempted.contains("deploy_blocked")),
            "expected deploy_blocked ntfy event; got: {:?}",
            evs
        );
        match prev_stores_bin {
            Some(v) => std::env::set_var("STORES_BIN", v),
            None => std::env::remove_var("STORES_BIN"),
        }

        let phash: Option<String> = conn
            .query_row(
                "SELECT policies_hash FROM transition_history \
                 WHERE store='tasks' AND display_id='T502' AND verb='mark_deploy_blocked'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(phash.as_deref(), Some("deadbeef"));
    }

    // -----------------------------------------------------------------
    // Phase 1 (T022): mark_drive_failed transition mechanics
    // -----------------------------------------------------------------

    /// Insert a task at an arbitrary `status` with `workspace_path` set, so
    /// fire_mark_drive_failed can drive the transition. The contract field is
    /// minimally populated to satisfy required-field checks.
    fn insert_task_at_status(conn: &Connection, display_id: &str, status: &str) -> i64 {
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'test', 't', 'feat/x', '/tmp/no-such', ?3, ?4, ?4, 'framework', 'framework')",
            rusqlite::params![display_id, status, contract, now],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// AC1.2: bare mark_drive_failed transition mechanics — call the helper on
    /// a planning row and verify status / blocked_reason / transition_history.
    #[test]
    fn m_drive_failed_transition_mechanics() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_task_at_status(&conn, "T600", "planning");

        fire_mark_drive_failed(&conn, "T600", "drive_failed", "", None).unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T600'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "blocked");
        assert_eq!(reason.as_deref(), Some("drive_failed"));

        let (verb, invoker): (String, String) = conn
            .query_row(
                "SELECT verb, invoker FROM transition_history \
                 WHERE store='tasks' AND display_id='T600'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(verb, "mark_drive_failed");
        assert_eq!(invoker, "framework");
    }

    /// AC1.3: mark_drive_failed succeeds from each of plan_review, ready,
    /// executing, code_review (parameterised over source state).
    #[test]
    fn n_drive_failed_from_each_source_state() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let cases = [
            ("T610", "plan_review"),
            ("T611", "ready"),
            ("T612", "executing"),
            ("T613", "code_review"),
        ];
        let (conn, _t, _o) = fresh_db_with_tasks();
        for (id, src) in &cases {
            insert_task_at_status(&conn, id, src);
            fire_mark_drive_failed(&conn, id, "drive_failed", "", None)
                .unwrap_or_else(|e| panic!("fire_mark_drive_failed from {src} failed: {e}"));
            let status: String = conn
                .query_row(
                    "SELECT status FROM tasks WHERE display_id=?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(status, "blocked", "src state {src} did not flip to blocked");
        }
    }

    // T138 P3: removed `m_accept_merge_noop_when_branch_already_merged_and_workspace_gone`.
    // It exercised accept-merge's no-op short-circuit firing
    // `mark_cargo_installed` on an `accepted → cargo_installed` direct edge.
    // Phase 1 retired that edge; the integration lane now mediates the
    // candidate-already-on-main case via the integrate builtin's
    // pre-merge ancestry check, which fires `mark_integrated` instead.

    /// T061 / L145 codex-revise: when branch is already merged AND cargo-install
    /// is a peer subscriber on the same accepted-entry edge (retry-deploy chain),
    /// accept-merge must NOT fire mark_cargo_installed — it leaves that to
    /// cargo-install so the install actually re-runs before the row advances.
    /// accept-merge returns Ok(0) as a no-op; the row stays in `accepted`.
    ///
    /// This test covers the MAJOR finding from the T061 codex review:
    /// "retry-deploy on an already-merged branch can skip the cargo-install retry."
    #[test]
    fn m_accept_merge_noop_no_mark_cargo_installed_when_cargo_install_peer_present() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let _cwd_g = crate::paths::test_cwd_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (tmp, repo) = init_repo();
        // Pre-merge the branch into main so is_branch_merged_into_main returns true.
        git(&repo, &["checkout", "-b", "feat/already-merged-retry"]);
        std::fs::write(repo.join("retry.txt"), "retry\n").unwrap();
        git(&repo, &["add", "retry.txt"]);
        git(&repo, &["commit", "-m", "retry change"]);
        git(&repo, &["checkout", "main"]);
        let m = git(
            &repo,
            &["merge", "--no-ff", "--no-edit", "feat/already-merged-retry"],
        );
        assert!(m.status.success(), "pre-merge into main failed: {:?}", m);

        // Set daemon cwd to the live main repo.
        let old_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&repo).expect("set_current_dir failed");

        // Stale workspace path (cleaned worktree after merge — mimics retry-deploy scenario).
        let gone = tmp.path().join("worktrees/T998-gone-retry");
        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_accepted_task(
            &conn,
            "T998",
            "feat/already-merged-retry",
            gone.to_str().unwrap(),
        );
        let row = task_row_json(&conn, "T998");

        // Build an AgentsYaml that includes cargo-install subscribed to
        // deploy_blocked→accepted (the retry-deploy chain). This is the
        // peer-subscriber condition that should suppress mark_cargo_installed.
        let agents = AgentsYaml {
            agents: vec![AgentEntry {
                name: "cargo-install".to_string(),
                subscribes_to: vec![Subscription {
                    store: "tasks".to_string(),
                    transition: TransitionEdge {
                        from: "deploy_blocked".to_string(),
                        to: "accepted".to_string(),
                    },
                    integration_step: None,
                    predicate: None,
                }],
                command: "builtin:cargo-install".to_string(),
                claim_window_secs: 600,
                retry_policy: RetryPolicy {
                    max_attempts: 1,
                    backoff: BackoffKind::Linear,
                },
                command_args: None,
            }],
            deployment_specialist: None,
        };
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };

        let res = accept_merge::run(&row, &ctx);

        // Restore cwd before any panic from assertions below.
        std::env::set_current_dir(&old_cwd).expect("restore cwd failed");

        assert_eq!(res.unwrap(), 0);

        // Row must still be in `accepted` — mark_cargo_installed was NOT fired.
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T998'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "accepted",
            "accept-merge must not advance to cargo_installed when cargo-install peer is present"
        );

        // No mark_cargo_installed in transition_history.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE store='tasks' AND display_id='T998' AND verb='mark_cargo_installed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "mark_cargo_installed must not appear in transition_history when cargo-install peer is present"
        );
    }

    /// T050 P2 AC2.3: postcondition_for_builtin maps each builtin keyword to
    /// the documented postcondition_id; unknown keywords return None.
    #[test]
    fn postcondition_for_builtin_mapping() {
        assert_eq!(
            postcondition_for_builtin("auto-promote"),
            Some("task_exists_for_linked_observation")
        );
        assert_eq!(
            postcondition_for_builtin("auto-scaffold"),
            Some("task_workspace_exists")
        );
        assert_eq!(
            postcondition_for_builtin("auto-drive"),
            Some("drive_pid_recorded_or_terminal")
        );
        assert_eq!(
            postcondition_for_builtin("cargo-install"),
            Some("cargo_installed_state")
        );
        assert_eq!(
            postcondition_for_builtin("schema-migrate"),
            Some("schema_migrated_state")
        );
        assert_eq!(postcondition_for_builtin("unknown-keyword"), None);
        assert_eq!(postcondition_for_builtin(""), None);

        // Each mapped postcondition_id resolves via the registry.
        for kw in [
            "auto-promote",
            "auto-scaffold",
            "auto-drive",
            "cargo-install",
            "schema-migrate",
        ] {
            let id = postcondition_for_builtin(kw).unwrap();
            assert!(
                crate::flow::postconditions::lookup(id).is_some(),
                "lookup({}) returned None for keyword {}",
                id,
                kw
            );
        }
    }

    /// AC1.4: a row at `in_review` rejects mark_drive_failed (no transition
    /// declared from in_review via that verb).
    #[test]
    fn o_drive_failed_rejected_from_in_review() {
        let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _t, _o) = fresh_db_with_tasks();
        insert_task_at_status(&conn, "T620", "in_review");
        let err = fire_mark_drive_failed(&conn, "T620", "drive_failed", "", None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("mark_drive_failed") || msg.contains("no transition"),
            "expected transition-rejection error; got: {msg}"
        );
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T620'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "in_review", "row must remain at in_review");
    }

    /// T140 P2 / Task 2.6 / AC2.2: every `pub mod X;` declaration in this
    /// file must be classified in docs/subscriber-classes.md with exactly
    /// one of the five class labels. Adding a new builtin without updating
    /// the doc is a fail-loud error.
    #[test]
    fn subscriber_class_taxonomy_complete() {
        const BUILTINS_MOD_RS: &str = include_str!("mod.rs");
        const SUBSCRIBER_CLASSES_MD: &str = include_str!("../../../docs/subscriber-classes.md");
        const ALLOWED_CLASSES: &[&str] = &[
            "work_starting",
            "safety_reconcile",
            "ceremony_post_accept",
            "observation_lifecycle",
            "deprecated_internal",
        ];

        // Parse `pub mod X;` declarations.
        let mut declared: Vec<String> = Vec::new();
        for line in BUILTINS_MOD_RS.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("pub mod ") {
                if let Some(name) = rest.split(';').next() {
                    let name = name.trim();
                    if !name.is_empty() {
                        declared.push(name.to_string());
                    }
                }
            }
        }
        assert!(
            !declared.is_empty(),
            "expected at least one `pub mod X;` in src/flow/builtins/mod.rs"
        );

        // Parse the markdown table: rows starting with '|' whose first cell
        // is a backticked module name.
        use std::collections::HashMap;
        let mut doc: HashMap<String, (usize, Vec<String>)> = HashMap::new();
        for raw in SUBSCRIBER_CLASSES_MD.lines() {
            let line = raw.trim();
            if !line.starts_with('|') {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            if lower.contains("module") && lower.contains("class") {
                continue;
            }
            if line
                .chars()
                .all(|c| c == '|' || c == '-' || c == ' ' || c == ':')
            {
                continue;
            }
            let cells: Vec<&str> = line
                .trim_matches('|')
                .split('|')
                .map(|s| s.trim())
                .collect();
            if cells.len() < 2 {
                continue;
            }
            let mod_name = cells[0].trim_matches('`').trim();
            if mod_name.is_empty() {
                continue;
            }
            let class_cell = cells[1];

            let classes: Vec<String> = ALLOWED_CLASSES
                .iter()
                .filter(|c| {
                    let c = **c;
                    class_cell
                        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                        .any(|tok| tok == c)
                })
                .map(|s| (*s).to_string())
                .collect();

            let entry = doc.entry(mod_name.to_string()).or_insert((0, Vec::new()));
            entry.0 += 1;
            for c in classes {
                if !entry.1.contains(&c) {
                    entry.1.push(c);
                }
            }
        }

        let mut errors: Vec<String> = Vec::new();
        for m in &declared {
            match doc.get(m) {
                None => errors.push(format!("missing in doc: `{m}`")),
                Some((count, _)) if *count > 1 => {
                    errors.push(format!("duplicate row in doc: `{m}` x{count}"))
                }
                Some((_, classes)) if classes.is_empty() => {
                    errors.push(format!("no class label for `{m}`"))
                }
                Some((_, classes)) if classes.len() > 1 => {
                    errors.push(format!("multiple class labels for `{m}`: {classes:?}"))
                }
                _ => {}
            }
        }
        for k in doc.keys() {
            if !declared.contains(k) {
                errors.push(format!("stale doc row `{k}`: no matching pub mod"));
            }
        }

        assert!(
            errors.is_empty(),
            "subscriber-class taxonomy drift:\n  - {}",
            errors.join("\n  - ")
        );

        // ---- T140 P6 / Task 6.2 (a): every dispatch_builtin keyword is
        // listed in the doc, classified as something other than
        // `deprecated_internal`, and its rationale records the dispatch fact
        // so the dispatched-vs-undispatched note matches the class.
        //
        // Keywords are extracted from the `dispatch_builtin` function body
        // by parsing the source file: each match arm has the shape
        //     "<keyword>" => Some(<module>::run( ... ))
        // and the test asserts the doc row keyed by the corresponding
        // module name (kebab-case → snake_case) carries:
        //   - rationale containing "Registered in `dispatch_builtin`"
        //   - class != "deprecated_internal"
        //
        // Accordingly, every `pub mod` whose doc row says "Not registered"
        // must NOT appear in dispatch_builtin. Failure messages name the
        // offending module(s) so future drift fails loud at exact identity.

        let keywords = parse_dispatch_builtin_keywords(BUILTINS_MOD_RS);
        assert!(
            !keywords.is_empty(),
            "expected `dispatch_builtin` to register at least one keyword"
        );

        // Re-parse the markdown table, this time keeping rationale text
        // alongside class.
        let mut doc_rows: HashMap<String, (String, String)> = HashMap::new();
        for raw in SUBSCRIBER_CLASSES_MD.lines() {
            let line = raw.trim();
            if !line.starts_with('|') {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            if lower.contains("module") && lower.contains("class") {
                continue;
            }
            if line
                .chars()
                .all(|c| c == '|' || c == '-' || c == ' ' || c == ':')
            {
                continue;
            }
            let cells: Vec<&str> = line
                .trim_matches('|')
                .split('|')
                .map(|s| s.trim())
                .collect();
            if cells.len() < 3 {
                continue;
            }
            let mod_name = cells[0].trim_matches('`').trim().to_string();
            if mod_name.is_empty() {
                continue;
            }
            doc_rows.insert(mod_name, (cells[1].to_string(), cells[2].to_string()));
        }

        let mut dispatch_errors: Vec<String> = Vec::new();
        for kw in &keywords {
            let module = kw.replace('-', "_");
            match doc_rows.get(&module) {
                None => dispatch_errors.push(format!(
                    "dispatch_builtin keyword `{kw}` (module `{module}`) is missing from docs/subscriber-classes.md"
                )),
                Some((class_cell, rationale)) => {
                    if class_cell.contains("deprecated_internal") {
                        dispatch_errors.push(format!(
                            "dispatch_builtin keyword `{kw}` is classified `deprecated_internal` — \
                             dispatched modules must not carry the deprecated_internal class"
                        ));
                    }
                    let rl = rationale.to_ascii_lowercase();
                    if !rl.contains("registered in `dispatch_builtin`")
                        || rl.contains("not registered in `dispatch_builtin`")
                    {
                        dispatch_errors.push(format!(
                            "dispatch_builtin keyword `{kw}` rationale must say \"Registered in `dispatch_builtin`\"; \
                             got: {rationale:?}"
                        ));
                    }
                }
            }
        }

        // ---- T140 P6 / Task 6.2 (b): accept-merge is the only entry
        // classified as `deprecated_internal`.
        let mut deprecated_modules: Vec<String> = Vec::new();
        for (module, (class_cell, _)) in &doc_rows {
            if class_cell.contains("deprecated_internal") {
                deprecated_modules.push(module.clone());
            }
        }
        deprecated_modules.sort();
        if deprecated_modules != vec!["accept_merge".to_string()] {
            dispatch_errors.push(format!(
                "expected `accept_merge` to be the unique deprecated_internal entry; \
                 got: {deprecated_modules:?}"
            ));
        }

        // Also assert: every undispatched module's rationale says so. This
        // is the dual of the dispatched-side check above.
        for (module, (_class, rationale)) in &doc_rows {
            let kw = module.replace('_', "-");
            let rl = rationale.to_ascii_lowercase();
            let rationale_says_registered = rl.contains("registered in `dispatch_builtin`")
                && !rl.contains("not registered in `dispatch_builtin`");
            let actually_dispatched = keywords.iter().any(|k| k == &kw);
            if rationale_says_registered != actually_dispatched {
                dispatch_errors.push(format!(
                    "module `{module}`: rationale registered={rationale_says_registered} but dispatch_builtin actually_dispatched={actually_dispatched}; \
                     rationale={rationale:?}"
                ));
            }
        }

        assert!(
            dispatch_errors.is_empty(),
            "T140 P6 / Task 6.2 dispatched-vs-undispatched drift:\n  - {}",
            dispatch_errors.join("\n  - ")
        );
    }

    /// Parse the `dispatch_builtin` function body for `"<keyword>" =>` arms.
    /// Returns the keywords in source order.
    fn parse_dispatch_builtin_keywords(source: &str) -> Vec<String> {
        let start = source
            .find("pub fn dispatch_builtin(")
            .expect("dispatch_builtin source present");
        let body = &source[start..];
        let body_open = body.find('{').expect("dispatch_builtin body open brace");
        let body = &body[body_open..];
        // Walk the body to find a balanced closing brace.
        let mut depth = 0i32;
        let mut end = 0usize;
        for (i, ch) in body.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &body[..end];

        let mut keywords = Vec::new();
        for line in body.lines() {
            let trimmed = line.trim();
            // Skip the catch-all `_ => None,` arm.
            if !trimmed.contains("=>") || trimmed.starts_with("_ ") || trimmed.starts_with("_=>") {
                continue;
            }
            // Find the first quoted token before "=>".
            if let Some(arrow) = trimmed.find("=>") {
                let lhs = &trimmed[..arrow];
                if let (Some(s), Some(e)) = (lhs.find('"'), lhs.rfind('"')) {
                    if e > s {
                        let kw = &lhs[s + 1..e];
                        if !kw.is_empty() {
                            keywords.push(kw.to_string());
                        }
                    }
                }
            }
        }
        keywords
    }

    /// T138 P3 AC3.5: `dispatch_builtin("accept-merge", …)` must return
    /// `None` — the keyword is no longer registered now that the integrate
    /// lane owns the merge step. The accept-merge module's helpers are
    /// still callable via `pub(crate)` for the integrate builtin's reuse.
    #[test]
    fn dispatch_builtin_accept_merge_keyword_unregistered() {
        let (conn, _t, _o) = fresh_db_with_tasks();
        let agents = empty_agents_yaml();
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };
        let row = serde_json::json!({"display_id": "T_dispatch_test"});
        assert!(
            dispatch_builtin("accept-merge", &row, &ctx).is_none(),
            "T138 P3 AC3.5: dispatch_builtin(\"accept-merge\", …) must return None"
        );
        // Sanity: the integrate keyword IS registered (post-T138 successor).
        assert!(
            dispatch_builtin("integrate", &row, &ctx).is_some(),
            "dispatch_builtin(\"integrate\", …) must remain registered"
        );
    }
}
