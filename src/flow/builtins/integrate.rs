//! `builtin:integrate` — generic integration lane.
//!
//! Owns the full integration step for a `tasks` row sitting at
//! `integration_queued`:
//!
//!   1. Atomically claim the singleton `integrating` slot (Phase 1's
//!      partial UNIQUE index `idx_tasks_integration_singleton` enforces
//!      capacity-1; concurrent attempts surface as a UNIQUE
//!      ConstraintViolation, which we treat as capacity-busy and return
//!      `Ok(0)`).
//!   2. Append a single in-progress entry to `tasks.integration_attempts`.
//!   3. Pre-rebase stale_base check against the latest passed
//!      external_review row.
//!   4. Refresh candidate (rebase or merge_main).
//!   5. Re-check external_review HEAD freshness against post-refresh head.
//!   6. Run the configured pre_land_check command.
//!   7. Fast-merge candidate into main; optional push.
//!   8. Update the in-progress entry in place; fire `mark_integrated` or
//!      `mark_integration_blocked` with a typed reason.
//!
//! No hardcoded post-land verbs — repo-specific subscribers hang off the
//! `integrated` state.
//!
//! Single-record provenance: only ONE entry is written per attempt, all
//! subsequent steps UPDATE the same `$[#-1]` element via SQLite `json_set`.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::flow::builtins::{
    fire_framework_transition_for, load_store_schema, load_tasks_schema, resolve_main_repo,
    BuiltinResult, DispatchCtx,
};
use crate::handlers::row::now_iso8601;

/// Configuration read from the `integrate` agent's `command_args` (or
/// defaulted when absent). The `pre_land_check` field is required when
/// integration runs end-to-end; a missing value short-circuits to
/// `pre_land_check_failed` so the gap surfaces as typed integration
/// provenance rather than an `Err`.
struct IntegrateCfg {
    pre_land_check: Option<String>,
    pre_land_check_timeout_secs: u64,
    refresh_strategy: String, // "rebase" | "merge_main"
    refresh_timeout_secs: u64,
    main_branch: String,
    allow_push: bool,
    push_remote: String,
}

impl IntegrateCfg {
    fn from_ctx(ctx: &DispatchCtx) -> Self {
        let mut cfg = Self {
            pre_land_check: None,
            pre_land_check_timeout_secs: 600,
            refresh_strategy: "rebase".to_string(),
            refresh_timeout_secs: 300,
            main_branch: "main".to_string(),
            allow_push: false,
            push_remote: "origin".to_string(),
        };
        let Some(entry) = ctx.agents.agents.iter().find(|a| a.name == "integrate") else {
            return cfg;
        };
        let Some(args) = entry.command_args.as_ref() else {
            return cfg;
        };
        if let Some(v) = args.get(serde_yaml::Value::String("pre_land_check".into())) {
            if let Some(s) = v.as_str() {
                if !s.trim().is_empty() {
                    cfg.pre_land_check = Some(s.to_string());
                }
            }
        }
        if let Some(v) = args.get(serde_yaml::Value::String(
            "pre_land_check_timeout_secs".into(),
        )) {
            if let Some(n) = v.as_u64() {
                cfg.pre_land_check_timeout_secs = n;
            }
        }
        if let Some(v) = args.get(serde_yaml::Value::String("refresh_strategy".into())) {
            if let Some(s) = v.as_str() {
                if s == "rebase" || s == "merge_main" {
                    cfg.refresh_strategy = s.to_string();
                }
            }
        }
        if let Some(v) = args.get(serde_yaml::Value::String("refresh_timeout_secs".into())) {
            if let Some(n) = v.as_u64() {
                cfg.refresh_timeout_secs = n;
            }
        }
        if let Some(v) = args.get(serde_yaml::Value::String("main_branch".into())) {
            if let Some(s) = v.as_str() {
                if !s.trim().is_empty() {
                    cfg.main_branch = s.to_string();
                }
            }
        }
        if let Some(v) = args.get(serde_yaml::Value::String("allow_push".into())) {
            if let Some(b) = v.as_bool() {
                cfg.allow_push = b;
            }
        }
        if let Some(v) = args.get(serde_yaml::Value::String("push_remote".into())) {
            if let Some(s) = v.as_str() {
                if !s.trim().is_empty() {
                    cfg.push_remote = s.to_string();
                }
            }
        }
        cfg
    }
}

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row
        .get("display_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let branch = row.get("branch").and_then(|v| v.as_str()).unwrap_or("");
    let workspace_path = row
        .get("workspace_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if display_id.is_empty() {
        eprintln!("[integrate] missing display_id; nothing to do");
        return Ok(0);
    }

    let cfg = IntegrateCfg::from_ctx(ctx);
    let tasks_schema = load_tasks_schema()?;

    // 1. Atomic capacity claim. Phase 1's partial UNIQUE index on
    //    tasks(status) WHERE status='integrating' enforces capacity-1: the
    //    second concurrent UPDATE flipping status to 'integrating' fails
    //    with SQLITE_CONSTRAINT_UNIQUE. Treat the constraint violation as
    //    capacity-busy and return Ok(0) without writing a per-attempt
    //    integration_attempts entry.
    if let Err(e) = fire_framework_transition_for(
        ctx.conn,
        &tasks_schema,
        display_id,
        "start-integration",
        BTreeMap::new(),
        ctx.policies_hash,
        None,
    ) {
        if is_unique_constraint_violation(&e) {
            eprintln!("[integrate] capacity-busy display_id={}", display_id);
            return Ok(0);
        }
        return Err(e.context(format!(
            "firing start-integration for {} (capacity claim)",
            display_id
        )));
    }

    // From here on we hold the integrating slot. Every exit path must
    // finalize the in-progress integration_attempts entry and either fire
    // mark_integrated or mark_integration_blocked so the slot is released.
    if branch.is_empty() {
        let summary = "branch field empty on tasks row; cannot integrate".to_string();
        block_with_outcome(
            ctx,
            display_id,
            "merge_failure",
            &summary,
            None, // no in-progress entry yet
        )?;
        return Ok(0);
    }
    if workspace_path.is_empty() {
        let summary = "workspace_path empty; cannot resolve main repo".to_string();
        block_with_outcome(ctx, display_id, "merge_failure", &summary, None)?;
        return Ok(0);
    }

    let main_repo = match resolve_main_repo(workspace_path) {
        Some(p) => p,
        None => {
            let summary = format!(
                "could not resolve main repo from workspace_path '{}'",
                workspace_path
            );
            block_with_outcome(ctx, display_id, "merge_failure", &summary, None)?;
            return Ok(0);
        }
    };

    let workspace_buf = PathBuf::from(workspace_path);

    // Capture pre-flight SHAs.
    let base_main_sha = match git_rev_parse(&main_repo, &cfg.main_branch) {
        Ok(s) => s,
        Err(e) => {
            let summary = format!("git rev-parse {} failed: {}", cfg.main_branch, e);
            block_with_outcome(ctx, display_id, "merge_failure", &summary, None)?;
            return Ok(0);
        }
    };
    let candidate_head_before = match git_rev_parse(&workspace_buf, branch) {
        Ok(s) => s,
        Err(e) => {
            let summary = format!("git rev-parse {} failed: {}", branch, e);
            block_with_outcome(ctx, display_id, "merge_failure", &summary, None)?;
            return Ok(0);
        }
    };

    // 2. Append ONE in-progress entry. attempt_no = current array length + 1.
    let attempt_no = current_attempt_count(ctx.conn, display_id)? + 1;
    let in_progress = json!({
        "attempt_no": attempt_no,
        "started_at": now_iso8601(),
        "base_main_sha": base_main_sha,
        "candidate_head_before": candidate_head_before,
        "candidate_head_after": "",
        "landed_main_sha": "",
        "refresh_strategy": cfg.refresh_strategy,
        "pre_land_check_summary": "",
        "reviewed_base_sha": "",
        "outcome": Value::Null,
        "completed_at": "",
    });
    append_attempt_entry(ctx.conn, display_id, &in_progress)?;

    // 3. PRE-REBASE stale_base check. Skip silently when no passed ER row
    //    exists OR its base_sha is null (T1 lanes / repos without ER).
    if let Some(er) = latest_passed_er_row(ctx.conn, display_id)? {
        if let Some(er_base) = er.base_sha.filter(|s| !s.is_empty()) {
            if !is_ancestor(&main_repo, &er_base, &cfg.main_branch) {
                let summary = format!(
                    "reviewed base {} no longer reachable from current main {}; \
                     force-push or history rewrite suspected — fresh external review required",
                    short_sha(&er_base),
                    short_sha(&base_main_sha)
                );
                update_last_attempt(
                    ctx.conn,
                    display_id,
                    &[
                        ("completed_at", Value::String(now_iso8601())),
                        ("outcome", Value::String("stale_base".to_string())),
                        ("reviewed_base_sha", Value::String(er_base)),
                        ("pre_land_check_summary", Value::String(summary.clone())),
                    ],
                )?;
                supersede_external_review(ctx, &er.display_id)?;
                fire_mark_integration_blocked(
                    ctx,
                    display_id,
                    &format!("stale_base: {}", summary),
                )?;
                return Ok(0);
            }
        }
    }

    // 4. Ensure the workspace is checked out to the candidate branch BEFORE
    //    running refresh or pre_land_check. Otherwise a workspace_path that
    //    happens to be on `main` (or any other branch) would silently
    //    rebase/validate the wrong tree immediately before landing.
    if let Err(msg) = ensure_branch_checked_out(&workspace_buf, branch) {
        update_last_attempt(
            ctx.conn,
            display_id,
            &[
                ("completed_at", Value::String(now_iso8601())),
                ("outcome", Value::String("merge_failure".to_string())),
                ("pre_land_check_summary", Value::String(msg.clone())),
            ],
        )?;
        fire_mark_integration_blocked(ctx, display_id, &format!("merge_failure: {}", msg))?;
        return Ok(0);
    }

    // 5. Refresh candidate. The workspace_path is now guaranteed on the
    //    candidate branch; rebase / merge runs there.
    let refresh_result = run_refresh(&workspace_buf, &cfg);
    match refresh_result {
        RefreshOutcome::Ok => {}
        RefreshOutcome::Conflict(msg) => {
            crate::flow::builtins::accept_merge::abort_rebase(&workspace_buf);
            crate::flow::builtins::accept_merge::abort_merge(&workspace_buf);
            update_last_attempt(
                ctx.conn,
                display_id,
                &[
                    ("completed_at", Value::String(now_iso8601())),
                    ("outcome", Value::String("rebase_conflict".to_string())),
                    ("pre_land_check_summary", Value::String(msg.clone())),
                ],
            )?;
            fire_mark_integration_blocked(
                ctx,
                display_id,
                &format!("rebase_conflict: {}", msg),
            )?;
            return Ok(0);
        }
        RefreshOutcome::StaleBase(msg) => {
            crate::flow::builtins::accept_merge::abort_rebase(&workspace_buf);
            crate::flow::builtins::accept_merge::abort_merge(&workspace_buf);
            update_last_attempt(
                ctx.conn,
                display_id,
                &[
                    ("completed_at", Value::String(now_iso8601())),
                    ("outcome", Value::String("stale_base".to_string())),
                    ("pre_land_check_summary", Value::String(msg.clone())),
                ],
            )?;
            fire_mark_integration_blocked(ctx, display_id, &format!("stale_base: {}", msg))?;
            return Ok(0);
        }
    }

    // 5. Capture candidate_head_after.
    let candidate_head_after = match git_rev_parse(&workspace_buf, branch) {
        Ok(s) => s,
        Err(e) => {
            let summary = format!(
                "git rev-parse {} (post-refresh) failed: {}",
                branch, e
            );
            update_last_attempt(
                ctx.conn,
                display_id,
                &[
                    ("completed_at", Value::String(now_iso8601())),
                    ("outcome", Value::String("merge_failure".to_string())),
                    ("pre_land_check_summary", Value::String(summary.clone())),
                ],
            )?;
            fire_mark_integration_blocked(
                ctx,
                display_id,
                &format!("merge_failure: {}", summary),
            )?;
            return Ok(0);
        }
    };
    update_last_attempt(
        ctx.conn,
        display_id,
        &[(
            "candidate_head_after",
            Value::String(candidate_head_after.clone()),
        )],
    )?;

    // 6. ER head-freshness re-check (T2/T3). Skip when no passed ER row.
    if let Some(er) = latest_passed_er_row(ctx.conn, display_id)? {
        let er_head = er.head_sha.clone().unwrap_or_default();
        if !er_head.is_empty() && er_head != candidate_head_after {
            let summary = format!(
                "ER {} reviewed head {} but candidate is now {}; superseded",
                er.display_id,
                short_sha(&er_head),
                short_sha(&candidate_head_after)
            );
            update_last_attempt(
                ctx.conn,
                display_id,
                &[
                    ("completed_at", Value::String(now_iso8601())),
                    (
                        "outcome",
                        Value::String("stale_external_review".to_string()),
                    ),
                    ("pre_land_check_summary", Value::String(summary.clone())),
                ],
            )?;
            supersede_external_review(ctx, &er.display_id)?;
            fire_mark_integration_blocked(
                ctx,
                display_id,
                &format!("stale_external_review: {}", summary),
            )?;
            return Ok(0);
        }
    }

    // 7. Run pre_land_check.
    let pre_land_summary = match cfg.pre_land_check.as_deref() {
        Some(cmd) if !cmd.trim().is_empty() => {
            match run_pre_land(cmd, &workspace_buf, cfg.pre_land_check_timeout_secs) {
                Ok(()) => "ok".to_string(),
                Err(msg) => {
                    update_last_attempt(
                        ctx.conn,
                        display_id,
                        &[
                            ("completed_at", Value::String(now_iso8601())),
                            (
                                "outcome",
                                Value::String("pre_land_check_failed".to_string()),
                            ),
                            ("pre_land_check_summary", Value::String(msg.clone())),
                        ],
                    )?;
                    fire_mark_integration_blocked(
                        ctx,
                        display_id,
                        &format!("pre_land_check_failed: {}", msg),
                    )?;
                    return Ok(0);
                }
            }
        }
        _ => {
            // Missing pre_land_check is a configuration error. Surface it as
            // typed pre_land_check_failed provenance so the human can see
            // exactly why the lane refused to land the candidate.
            let msg = "pre_land_check command not configured on integrate agent's command_args"
                .to_string();
            update_last_attempt(
                ctx.conn,
                display_id,
                &[
                    ("completed_at", Value::String(now_iso8601())),
                    (
                        "outcome",
                        Value::String("pre_land_check_failed".to_string()),
                    ),
                    ("pre_land_check_summary", Value::String(msg.clone())),
                ],
            )?;
            fire_mark_integration_blocked(
                ctx,
                display_id,
                &format!("pre_land_check_failed: {}", msg),
            )?;
            return Ok(0);
        }
    };

    // 8. Fast-merge candidate into main. The merge runs from the main-repo
    //    checkout (resolve_main_repo()), not from the candidate worktree.
    let merge = Command::new("git")
        .args([
            "-C",
            main_repo.to_str().unwrap_or("."),
            "checkout",
            &cfg.main_branch,
        ])
        .output();
    if let Ok(out) = merge {
        if !out.status.success() {
            let summary = format!(
                "git checkout {} failed: {}",
                cfg.main_branch,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            update_last_attempt(
                ctx.conn,
                display_id,
                &[
                    ("completed_at", Value::String(now_iso8601())),
                    ("outcome", Value::String("merge_failure".to_string())),
                    ("pre_land_check_summary", Value::String(summary.clone())),
                ],
            )?;
            fire_mark_integration_blocked(
                ctx,
                display_id,
                &format!("merge_failure: {}", summary),
            )?;
            return Ok(0);
        }
    }
    let merge_out = Command::new("git")
        .args([
            "-C",
            main_repo.to_str().unwrap_or("."),
            "merge",
            "--no-ff",
            "--no-edit",
            branch,
        ])
        .output()
        .with_context(|| format!("spawning git merge for {}", display_id))?;
    if !merge_out.status.success() {
        let stderr = String::from_utf8_lossy(&merge_out.stderr).to_string();
        crate::flow::builtins::accept_merge::abort_merge(&main_repo);
        let summary = format!(
            "git merge --no-ff {} into {} failed: {}",
            branch,
            cfg.main_branch,
            stderr.lines().next().unwrap_or("merge failed").trim()
        );
        update_last_attempt(
            ctx.conn,
            display_id,
            &[
                ("completed_at", Value::String(now_iso8601())),
                ("outcome", Value::String("merge_failure".to_string())),
                ("pre_land_check_summary", Value::String(summary.clone())),
            ],
        )?;
        fire_mark_integration_blocked(ctx, display_id, &format!("merge_failure: {}", summary))?;
        return Ok(0);
    }

    // 9. Optional push.
    if cfg.allow_push {
        let push = Command::new("git")
            .args([
                "-C",
                main_repo.to_str().unwrap_or("."),
                "push",
                &cfg.push_remote,
                &cfg.main_branch,
            ])
            .output()
            .with_context(|| format!("spawning git push for {}", display_id))?;
        if !push.status.success() {
            let stderr = String::from_utf8_lossy(&push.stderr).to_string();
            // Best-effort rollback: only when the new HEAD is exactly
            // base_main_sha + the new merge commit (HEAD^ == base_main_sha).
            let head_parent = git_rev_parse(&main_repo, "HEAD^").ok();
            if head_parent.as_deref() == Some(&base_main_sha) {
                let _ = Command::new("git")
                    .args([
                        "-C",
                        main_repo.to_str().unwrap_or("."),
                        "reset",
                        "--hard",
                        &base_main_sha,
                    ])
                    .output();
            }
            let summary = format!(
                "git push {} {} failed: {}",
                cfg.push_remote,
                cfg.main_branch,
                stderr.lines().next().unwrap_or("push failed").trim()
            );
            update_last_attempt(
                ctx.conn,
                display_id,
                &[
                    ("completed_at", Value::String(now_iso8601())),
                    ("outcome", Value::String("push_failure".to_string())),
                    ("pre_land_check_summary", Value::String(summary.clone())),
                ],
            )?;
            fire_mark_integration_blocked(
                ctx,
                display_id,
                &format!("push_failure: {}", summary),
            )?;
            return Ok(0);
        }
    }

    // 10. Capture landed_main_sha; finalize the in-progress entry; fire
    //     mark_integrated.
    let landed_main_sha = git_rev_parse(&main_repo, &cfg.main_branch)
        .with_context(|| format!("git rev-parse {} post-merge", cfg.main_branch))?;
    update_last_attempt(
        ctx.conn,
        display_id,
        &[
            ("completed_at", Value::String(now_iso8601())),
            ("outcome", Value::String("integrated".to_string())),
            ("landed_main_sha", Value::String(landed_main_sha)),
            ("pre_land_check_summary", Value::String(pre_land_summary)),
        ],
    )?;
    fire_framework_transition_for(
        ctx.conn,
        &tasks_schema,
        display_id,
        "mark_integrated",
        BTreeMap::new(),
        ctx.policies_hash,
        None,
    )
    .with_context(|| format!("firing mark_integrated for {}", display_id))?;
    Ok(0)
}

// ───────────────────────── helpers ─────────────────────────

fn is_unique_constraint_violation(e: &anyhow::Error) -> bool {
    e.chain().any(|err| {
        err.downcast_ref::<rusqlite::Error>()
            .map(|sql_err| {
                matches!(
                    sql_err,
                    rusqlite::Error::SqliteFailure(f, _)
                        if f.code == rusqlite::ErrorCode::ConstraintViolation
                )
            })
            .unwrap_or(false)
    })
}

fn short_sha(s: &str) -> String {
    s.chars().take(7).collect()
}

fn git_rev_parse(repo: &Path, rev: &str) -> Result<String> {
    let out = Command::new("git")
        .args(["-C", repo.to_str().unwrap_or("."), "rev-parse", rev])
        .output()
        .with_context(|| format!("spawning git rev-parse {} in {}", rev, repo.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "git rev-parse {} in {} failed: {}",
            rev,
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap_or("."),
            "merge-base",
            "--is-ancestor",
            ancestor,
            descendant,
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn current_attempt_count(conn: &rusqlite::Connection, display_id: &str) -> Result<i64> {
    let n: i64 = conn
        .query_row(
            "SELECT COALESCE(json_array_length(integration_attempts), 0) FROM tasks \
             WHERE display_id=?1",
            params![display_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(n)
}

fn append_attempt_entry(
    conn: &rusqlite::Connection,
    display_id: &str,
    entry: &Value,
) -> Result<()> {
    let entry_json = serde_json::to_string(entry)?;
    conn.execute(
        "UPDATE tasks SET integration_attempts = \
           json_insert(COALESCE(integration_attempts, json('[]')), '$[#]', json(?1)) \
         WHERE display_id=?2",
        params![entry_json, display_id],
    )?;
    Ok(())
}

fn update_last_attempt(
    conn: &rusqlite::Connection,
    display_id: &str,
    fields: &[(&str, Value)],
) -> Result<()> {
    if fields.is_empty() {
        return Ok(());
    }
    // SQLite's `json_set(json, path1, val1, path2, val2, ...)` updates each
    // path in turn. Build the call dynamically.
    let mut sql = String::from("UPDATE tasks SET integration_attempts = json_set(integration_attempts");
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::with_capacity(fields.len() * 2 + 1);
    for (i, (k, v)) in fields.iter().enumerate() {
        let path_idx = 2 + i * 2;
        let val_idx = path_idx + 1;
        sql.push_str(&format!(", ?{path_idx}, ?{val_idx}"));
        params_vec.push(rusqlite::types::Value::Text(format!("$[#-1].{}", k)));
        match v {
            Value::Null => params_vec.push(rusqlite::types::Value::Null),
            Value::Bool(b) => {
                params_vec.push(rusqlite::types::Value::Integer(if *b { 1 } else { 0 }))
            }
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    params_vec.push(rusqlite::types::Value::Integer(i));
                } else {
                    params_vec.push(rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)));
                }
            }
            Value::String(s) => params_vec.push(rusqlite::types::Value::Text(s.clone())),
            other => params_vec.push(rusqlite::types::Value::Text(other.to_string())),
        }
    }
    sql.push_str(") WHERE display_id=?1");
    let mut all_params: Vec<rusqlite::types::Value> = Vec::with_capacity(params_vec.len() + 1);
    all_params.push(rusqlite::types::Value::Text(display_id.to_string()));
    all_params.extend(params_vec);
    conn.execute(&sql, rusqlite::params_from_iter(all_params.iter()))?;
    Ok(())
}

struct PassedEr {
    display_id: String,
    base_sha: Option<String>,
    head_sha: Option<String>,
}

fn latest_passed_er_row(
    conn: &rusqlite::Connection,
    task_id: &str,
) -> Result<Option<PassedEr>> {
    let row = conn
        .query_row(
            "SELECT display_id, base_sha, head_sha FROM external_reviews \
             WHERE task_id=?1 AND status='passed' AND verdict='PASS' \
             ORDER BY attempt DESC, id DESC LIMIT 1",
            params![task_id],
            |r| {
                Ok(PassedEr {
                    display_id: r.get(0)?,
                    base_sha: r.get(1)?,
                    head_sha: r.get(2)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

fn supersede_external_review(ctx: &DispatchCtx, er_display_id: &str) -> Result<()> {
    let schema = load_store_schema("external_reviews")?;
    fire_framework_transition_for(
        ctx.conn,
        &schema,
        er_display_id,
        "supersede",
        BTreeMap::new(),
        ctx.policies_hash,
        Some("integrate"),
    )
    .with_context(|| format!("supersede external_review {}", er_display_id))
}

fn fire_mark_integration_blocked(
    ctx: &DispatchCtx,
    display_id: &str,
    reason: &str,
) -> Result<()> {
    let mut diff: BTreeMap<String, Value> = BTreeMap::new();
    diff.insert(
        "integration_blocked_reason".to_string(),
        Value::String(reason.to_string()),
    );
    let schema = load_tasks_schema()?;
    fire_framework_transition_for(
        ctx.conn,
        &schema,
        display_id,
        "mark_integration_blocked",
        diff,
        ctx.policies_hash,
        None,
    )
    .with_context(|| format!("firing mark_integration_blocked for {}", display_id))
}

/// Block before any in-progress entry was written. Used for the early
/// branch/workspace_path/main-repo-resolution failure modes that happen
/// after the capacity claim but before we know enough to write a useful
/// integration_attempts entry. The slot is released by mark_integration_blocked.
fn block_with_outcome(
    ctx: &DispatchCtx,
    display_id: &str,
    outcome: &str,
    summary: &str,
    _entry_present: Option<()>,
) -> Result<()> {
    fire_mark_integration_blocked(ctx, display_id, &format!("{}: {}", outcome, summary))
}

enum RefreshOutcome {
    Ok,
    Conflict(String),
    StaleBase(String),
}

fn run_refresh(workspace: &Path, cfg: &IntegrateCfg) -> RefreshOutcome {
    let args: Vec<String> = if cfg.refresh_strategy == "merge_main" {
        vec![
            "merge".into(),
            cfg.main_branch.clone(),
            "--no-ff".into(),
            "--no-edit".into(),
        ]
    } else {
        vec!["rebase".into(), cfg.main_branch.clone()]
    };
    let mut full: Vec<String> = vec!["-C".into(), workspace.to_string_lossy().into_owned()];
    full.extend(args.iter().cloned());
    let mut child = match Command::new("git")
        .args(&full)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return RefreshOutcome::Conflict(format!(
                "spawning git {} failed: {}",
                args.join(" "),
                e
            ));
        }
    };

    let deadline = Instant::now() + Duration::from_secs(cfg.refresh_timeout_secs.max(1));
    let (success, stderr) = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stderr_buf = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut stderr_buf);
                }
                break (status.success(), stderr_buf);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    crate::flow::builtins::accept_merge::abort_rebase(workspace);
                    crate::flow::builtins::accept_merge::abort_merge(workspace);
                    return RefreshOutcome::Conflict(format!(
                        "{} {} timeout after {}s",
                        cfg.refresh_strategy, cfg.main_branch, cfg.refresh_timeout_secs
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return RefreshOutcome::Conflict(format!(
                    "wait error during {} {}: {}",
                    cfg.refresh_strategy, cfg.main_branch, e
                ));
            }
        }
    };

    if success {
        return RefreshOutcome::Ok;
    }
    let stderr_first = stderr.lines().next().unwrap_or("").trim().to_string();
    // Defense-in-depth stale_base classification (Task 2.6 fallback).
    let lower = stderr.to_lowercase();
    if lower.contains("invalid upstream")
        || lower.contains("no such ref")
        || lower.contains("unknown revision")
    {
        return RefreshOutcome::StaleBase(format!(
            "{} {}: {}",
            cfg.refresh_strategy,
            cfg.main_branch,
            if stderr_first.is_empty() {
                "stale base"
            } else {
                stderr_first.as_str()
            }
        ));
    }
    let conflict_files = crate::flow::builtins::accept_merge::list_conflict_files(workspace);
    let files = if conflict_files.is_empty() {
        "<no conflict files reported>".to_string()
    } else {
        conflict_files.join(", ")
    };
    RefreshOutcome::Conflict(format!(
        "{}: {} ({})",
        cfg.refresh_strategy,
        files,
        if stderr_first.is_empty() {
            "refresh failed"
        } else {
            stderr_first.as_str()
        }
    ))
}

/// Ensure `workspace` has `branch` checked out. If a different branch is
/// active, run `git checkout <branch>`. Returns `Err` with a human-readable
/// reason on failure (dirty tree blocking checkout, missing branch, etc.).
fn ensure_branch_checked_out(workspace: &Path, branch: &str) -> std::result::Result<(), String> {
    let cur = Command::new("git")
        .args([
            "-C",
            workspace.to_str().unwrap_or("."),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output();
    if let Ok(o) = cur {
        if o.status.success() {
            let name = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if name == branch {
                return Ok(());
            }
        }
    }
    let co = Command::new("git")
        .args(["-C", workspace.to_str().unwrap_or("."), "checkout", branch])
        .output()
        .map_err(|e| format!("spawning git checkout {} failed: {}", branch, e))?;
    if !co.status.success() {
        let stderr = String::from_utf8_lossy(&co.stderr).to_string();
        return Err(format!(
            "git checkout {} in {} failed: {}",
            branch,
            workspace.display(),
            stderr.lines().next().unwrap_or("checkout failed").trim()
        ));
    }
    Ok(())
}

fn run_pre_land(cmd: &str, workspace: &Path, timeout_secs: u64) -> std::result::Result<(), String> {
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(workspace)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("pre_land_check spawn failed: {}", e)),
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(1));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = child;
                let mut stderr_buf = String::new();
                if let Some(mut e) = out.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut stderr_buf);
                }
                if status.success() {
                    return Ok(());
                }
                let tail: Vec<&str> = stderr_buf.lines().collect();
                let start = tail.len().saturating_sub(20);
                let summary = format!(
                    "pre_land_check exit={}: {}",
                    status.code().unwrap_or(-1),
                    tail[start..].join("\n").trim()
                );
                return Err(summary);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "pre_land_check timeout after {}s",
                        timeout_secs
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(format!("pre_land_check wait error: {}", e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::flow::agents_yaml::{Subscription, TransitionEdge};
    use crate::flow::{AgentEntry, AgentsYaml, RetryPolicy};
    use crate::handlers::framework_migrate::ensure_integration_singleton_index;
    use crate::schema::Schema;
    use rusqlite::Connection;
    use std::path::PathBuf;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        for store in ["tasks", "external_reviews"] {
            let yaml = BUNDLED_STORE_SCHEMAS
                .iter()
                .find(|(n, _)| *n == store)
                .map(|(_, y)| *y)
                .unwrap();
            let schema = Schema::from_yaml(yaml).unwrap();
            conn.execute_batch(&ddl_for(&schema)).unwrap();
        }
        ensure_integration_singleton_index(&conn).unwrap();
        conn
    }

    fn git(repo: &Path, args: &[&str]) -> std::process::Output {
        let mut full: Vec<&str> = vec!["-C", repo.to_str().unwrap()];
        full.extend_from_slice(args);
        Command::new("git").args(&full).output().unwrap()
    }

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

    fn add_branch_with_change(repo: &Path, branch: &str, file: &str, contents: &str) {
        git(repo, &["checkout", "-b", branch]);
        std::fs::write(repo.join(file), contents).unwrap();
        git(repo, &["add", file]);
        git(repo, &["commit", "-m", &format!("{} commit", branch)]);
        git(repo, &["checkout", "main"]);
    }

    fn insert_queued_task(
        conn: &Connection,
        display_id: &str,
        branch: &str,
        workspace_path: &str,
    ) {
        let now = "2026-05-09T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'integration_queued', 'test', 't', ?2, ?3, ?4, ?5, ?5, 'framework', 'framework')",
            rusqlite::params![display_id, branch, workspace_path, contract, now],
        )
        .unwrap();
    }

    fn force_status(conn: &Connection, display_id: &str, status: &str) {
        conn.execute(
            "UPDATE tasks SET status=?1 WHERE display_id=?2",
            rusqlite::params![status, display_id],
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
                rusqlite::types::Value::Real(f) => Value::from(
                    serde_json::Number::from_f64(f).unwrap_or(0.into()),
                ),
                rusqlite::types::Value::Text(s) => Value::String(s),
                rusqlite::types::Value::Blob(b) => {
                    Value::String(String::from_utf8_lossy(&b).to_string())
                }
            };
            obj.insert(name.clone(), jv);
        }
        Value::Object(obj)
    }

    #[test]
    fn pre_land_check_large_stdout_does_not_deadlock() {
        let tmp = tempfile::tempdir().unwrap();
        let result = run_pre_land(
            "python3 - <<'PY'\nprint('x' * 200000)\nPY",
            tmp.path(),
            5,
        );
        assert!(
            result.is_ok(),
            "large stdout from a successful pre_land_check must not fill a pipe and timeout: {result:?}"
        );
    }

    fn integrate_agents_yaml(pre_land_check: &str) -> AgentsYaml {
        let mut args = serde_yaml::Mapping::new();
        args.insert(
            serde_yaml::Value::String("pre_land_check".into()),
            serde_yaml::Value::String(pre_land_check.into()),
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

    fn cfg_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp/stores-test-no-config.yaml")
    }

    fn json_extract_str(conn: &Connection, display_id: &str, path: &str) -> Option<String> {
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

    fn json_array_len(conn: &Connection, display_id: &str) -> i64 {
        conn.query_row(
            "SELECT COALESCE(json_array_length(integration_attempts), 0) FROM tasks \
             WHERE display_id=?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// (a) Capacity-busy short-circuit. Another row already holds
    /// `status='integrating'`; the second integrate.run must return Ok(0)
    /// without writing a new integration_attempts entry on the loser row.
    #[test]
    fn a_capacity_busy_short_circuits() {
        let conn = fresh_db();
        let (_tmp, repo) = init_repo();
        // Row A holds the integrating slot.
        insert_queued_task(&conn, "T100", "feat/a", repo.to_str().unwrap());
        force_status(&conn, "T100", "integrating");
        // Row B is queued.
        insert_queued_task(&conn, "T101", "feat/b", repo.to_str().unwrap());
        let pre_count = json_array_len(&conn, "T101");

        let row = task_row_json(&conn, "T101");
        let agents = integrate_agents_yaml("true");
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };
        let res = run(&row, &ctx).unwrap();
        assert_eq!(res, 0);
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T101'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "integration_queued");
        let post_count = json_array_len(&conn, "T101");
        assert_eq!(
            pre_count, post_count,
            "loser row must not gain an integration_attempts entry"
        );
    }

    /// (b) Clean rebase + ff merge → integrated; provenance entry shows
    /// outcome='integrated', attempt_no=1, all SHAs populated.
    #[test]
    fn b_clean_integration_records_provenance() {
        let conn = fresh_db();
        let (_tmp, repo) = init_repo();
        add_branch_with_change(&repo, "feat/x", "feat.txt", "feat\n");
        insert_queued_task(&conn, "T200", "feat/x", repo.to_str().unwrap());

        let row = task_row_json(&conn, "T200");
        let agents = integrate_agents_yaml("true");
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "feed",
        };
        run(&row, &ctx).unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T200'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "integrated");
        assert_eq!(json_array_len(&conn, "T200"), 1);
        assert_eq!(
            json_extract_str(&conn, "T200", "$[0].outcome").as_deref(),
            Some("integrated")
        );
        let attempt_no: i64 = conn
            .query_row(
                "SELECT json_extract(integration_attempts, '$[0].attempt_no') FROM tasks WHERE display_id='T200'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempt_no, 1);
        for path in [
            "$[#-1].base_main_sha",
            "$[#-1].candidate_head_before",
            "$[#-1].candidate_head_after",
            "$[#-1].landed_main_sha",
        ] {
            let s = json_extract_str(&conn, "T200", path).unwrap_or_default();
            assert!(
                !s.is_empty() && s.len() >= 7,
                "{} must be populated; got: {:?}",
                path,
                s
            );
        }
    }

    /// (c) Rebase-conflict path. Same single entry; outcome='rebase_conflict'.
    #[test]
    fn c_rebase_conflict_records_outcome() {
        let conn = fresh_db();
        let (_tmp, repo) = init_repo();
        // Both main and branch touch file.txt → rebase will conflict.
        git(&repo, &["checkout", "-b", "feat/conflict"]);
        std::fs::write(repo.join("file.txt"), "branch-side\n").unwrap();
        git(&repo, &["add", "file.txt"]);
        git(&repo, &["commit", "-m", "branch"]);
        git(&repo, &["checkout", "main"]);
        std::fs::write(repo.join("file.txt"), "main-side\n").unwrap();
        git(&repo, &["add", "file.txt"]);
        git(&repo, &["commit", "-m", "main change"]);
        // Worktree must currently be on the branch for rebase to run.
        git(&repo, &["checkout", "feat/conflict"]);
        insert_queued_task(&conn, "T300", "feat/conflict", repo.to_str().unwrap());

        let row = task_row_json(&conn, "T300");
        let agents = integrate_agents_yaml("true");
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };
        run(&row, &ctx).unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T300'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "integration_blocked");
        assert_eq!(json_array_len(&conn, "T300"), 1);
        assert_eq!(
            json_extract_str(&conn, "T300", "$[#-1].outcome").as_deref(),
            Some("rebase_conflict")
        );
        let reason: Option<String> = conn
            .query_row(
                "SELECT integration_blocked_reason FROM tasks WHERE display_id='T300'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            reason.unwrap_or_default().contains("rebase_conflict"),
            "blocked_reason must cite rebase_conflict"
        );
    }

    /// (d) pre_land_check fail path.
    #[test]
    fn d_pre_land_check_failure() {
        let conn = fresh_db();
        let (_tmp, repo) = init_repo();
        add_branch_with_change(&repo, "feat/y", "feat.txt", "feat\n");
        insert_queued_task(&conn, "T400", "feat/y", repo.to_str().unwrap());
        let row = task_row_json(&conn, "T400");
        // pre_land_check that always fails.
        let agents = integrate_agents_yaml("false");
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };
        run(&row, &ctx).unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T400'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "integration_blocked");
        assert_eq!(
            json_extract_str(&conn, "T400", "$[#-1].outcome").as_deref(),
            Some("pre_land_check_failed")
        );
    }

    /// (e) ER stale-PASS HEAD path: latest passed ER references an old head;
    /// post-rebase head differs → outcome='stale_external_review'; ER row
    /// transitions to 'superseded'.
    #[test]
    fn e_stale_external_review_head() {
        let conn = fresh_db();
        let (_tmp, repo) = init_repo();
        add_branch_with_change(&repo, "feat/e", "feat.txt", "feat\n");
        // Capture current branch HEAD then advance the branch by another commit.
        let er_head = String::from_utf8_lossy(
            &git(&repo, &["rev-parse", "feat/e"]).stdout,
        )
        .trim()
        .to_string();
        // Advance candidate so it diverges from the ER's recorded head.
        git(&repo, &["checkout", "feat/e"]);
        std::fs::write(repo.join("feat2.txt"), "more\n").unwrap();
        git(&repo, &["add", "feat2.txt"]);
        git(&repo, &["commit", "-m", "more"]);
        git(&repo, &["checkout", "main"]);
        let main_sha = String::from_utf8_lossy(
            &git(&repo, &["rev-parse", "main"]).stdout,
        )
        .trim()
        .to_string();

        insert_queued_task(&conn, "T500", "feat/e", repo.to_str().unwrap());
        // ER's base_sha equals main_sha (so stale_base check passes), but
        // head_sha is stale.
        insert_passed_er(&conn, "ER001", "T500", 1, &main_sha, &er_head);

        let row = task_row_json(&conn, "T500");
        let agents = integrate_agents_yaml("true");
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };
        run(&row, &ctx).unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T500'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "integration_blocked");
        assert_eq!(
            json_extract_str(&conn, "T500", "$[#-1].outcome").as_deref(),
            Some("stale_external_review")
        );
        let er_status: String = conn
            .query_row(
                "SELECT status FROM external_reviews WHERE display_id='ER001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(er_status, "superseded");
    }

    /// (f) Retry replay: after one integration_blocked, retry → integrated.
    /// Asserts json_array_length=2, attempt_no progression, last outcome.
    #[test]
    fn f_retry_replay_appends_second_entry() {
        let conn = fresh_db();
        let (_tmp, repo) = init_repo();
        add_branch_with_change(&repo, "feat/f", "feat.txt", "feat\n");
        insert_queued_task(&conn, "T600", "feat/f", repo.to_str().unwrap());

        // First attempt: pre_land_check fails → integration_blocked.
        let row = task_row_json(&conn, "T600");
        let agents = integrate_agents_yaml("false");
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };
        run(&row, &ctx).unwrap();
        assert_eq!(json_array_len(&conn, "T600"), 1);

        // retry-integration: integration_blocked → integration_queued.
        let tasks_schema = load_tasks_schema().unwrap();
        fire_framework_transition_for(
            &conn,
            &tasks_schema,
            "T600",
            "retry-integration",
            BTreeMap::new(),
            "",
            None,
        )
        // retry-integration is ai_with_human, but the framework helper uses
        // Actor::Framework. The schema declares retry-integration with
        // actor: ai_with_human, so framework-actor won't match. Bypass via
        // direct state forcing for this unit test (production path goes
        // through CLI verb with proper invoker).
        .ok();
        force_status(&conn, "T600", "integration_queued");

        // Second attempt: pre_land_check succeeds → integrated.
        let agents2 = integrate_agents_yaml("true");
        let row2 = task_row_json(&conn, "T600");
        let ctx2 = DispatchCtx {
            conn: &conn,
            agents: &agents2,
            config_path: &cfg,
            policies_hash: "",
        };
        run(&row2, &ctx2).unwrap();

        assert_eq!(json_array_len(&conn, "T600"), 2);
        let a0: i64 = conn
            .query_row(
                "SELECT json_extract(integration_attempts, '$[0].attempt_no') FROM tasks WHERE display_id='T600'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let a1: i64 = conn
            .query_row(
                "SELECT json_extract(integration_attempts, '$[1].attempt_no') FROM tasks WHERE display_id='T600'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a0, 1);
        assert_eq!(a1, 2);
        assert_eq!(
            json_extract_str(&conn, "T600", "$[1].outcome").as_deref(),
            Some("integrated")
        );
    }

    /// Set up a main repo plus N candidate worktrees inside a parent
    /// tempdir. Returns (parent_tmpdir, main_repo, [worktree_paths]).
    fn init_repo_with_worktrees(branches: &[&str]) -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("main");
        std::fs::create_dir(&repo).unwrap();
        let g = git(&repo, &["init", "-b", "main"]);
        assert!(g.status.success(), "git init failed: {:?}", g);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("file.txt"), "main\n").unwrap();
        git(&repo, &["add", "file.txt"]);
        git(&repo, &["commit", "-m", "init"]);
        let mut worktrees = Vec::new();
        for (i, br) in branches.iter().enumerate() {
            // Create the branch from main with a unique non-conflicting change.
            let fname = format!("cg{}.txt", i + 1);
            git(&repo, &["checkout", "-b", br]);
            std::fs::write(repo.join(&fname), format!("{}\n", br)).unwrap();
            git(&repo, &["add", &fname]);
            git(&repo, &["commit", "-m", &format!("{} commit", br)]);
            git(&repo, &["checkout", "main"]);
            // Add a worktree dedicated to that branch.
            let wt = parent.path().join(format!("wt{}", i + 1));
            let g = git(&repo, &["worktree", "add", wt.to_str().unwrap(), br]);
            assert!(
                g.status.success(),
                "git worktree add failed for {}: {}",
                br,
                String::from_utf8_lossy(&g.stderr)
            );
            worktrees.push(wt);
        }
        (parent, repo, worktrees)
    }

    fn fresh_file_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.busy_timeout(Duration::from_secs(10)).unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        for store in ["tasks", "external_reviews"] {
            let yaml = BUNDLED_STORE_SCHEMAS
                .iter()
                .find(|(n, _)| *n == store)
                .map(|(_, y)| *y)
                .unwrap();
            let schema = Schema::from_yaml(yaml).unwrap();
            conn.execute_batch(&ddl_for(&schema)).unwrap();
        }
        ensure_integration_singleton_index(&conn).unwrap();
        conn
    }

    /// (g) Concurrency: spawn ≥3 worker threads that race for the singleton
    /// integrating slot via builtin:integrate.run. Sample
    /// `COUNT(*) WHERE status='integrating'` on every iteration of every
    /// worker and assert it never exceeds 1. After all candidates land,
    /// verify each successor's `base_main_sha` equals its predecessor's
    /// `landed_main_sha`. Uses a file-backed SQLite DB + worktrees + a
    /// pre_land_check that holds the slot for ~200ms so workers actually
    /// contend.
    #[test]
    fn g_concurrent_integration_serializes_correctly() {
        use std::sync::atomic::{AtomicI64, Ordering};
        use std::sync::Arc;

        let branches = ["feat/cg1", "feat/cg2", "feat/cg3"];
        let (parent, _repo, worktrees) = init_repo_with_worktrees(&branches);
        let db_path = parent.path().join("test.db");

        // Bootstrap the DB on the main thread, then drop the connection
        // before workers open their own.
        {
            let conn = fresh_file_db(&db_path);
            for (i, br) in branches.iter().enumerate() {
                let tid = format!("T9{:02}", i);
                insert_queued_task(&conn, &tid, br, worktrees[i].to_str().unwrap());
            }
        }

        let max_concurrent = Arc::new(AtomicI64::new(0));
        let db_path_arc: Arc<PathBuf> = Arc::new(db_path.clone());

        // pre_land_check: extend the integrating window so other workers
        // observe contention. ~200ms is plenty for 3 threads on a fast box.
        let cmd = "sleep 0.2 && true";

        let handles: Vec<_> = (0..branches.len())
            .map(|i| {
                let dbp = db_path_arc.clone();
                let mc = max_concurrent.clone();
                let tid = format!("T9{:02}", i);
                std::thread::spawn(move || {
                    // Bounded retry loop; each call either wins the slot
                    // and integrates, or short-circuits as capacity-busy.
                    for _attempt in 0..200 {
                        let conn = Connection::open(&*dbp).unwrap();
                        conn.busy_timeout(Duration::from_secs(10)).unwrap();
                        let status: String = conn
                            .query_row(
                                "SELECT status FROM tasks WHERE display_id=?1",
                                rusqlite::params![tid],
                                |r| r.get(0),
                            )
                            .unwrap();
                        if status == "integrated" || status == "integration_blocked" {
                            return;
                        }
                        // Sample concurrent-integrator count BEFORE entering
                        // run() so we observe the actual claim window.
                        let count: i64 = conn
                            .query_row(
                                "SELECT COUNT(*) FROM tasks WHERE status='integrating'",
                                [],
                                |r| r.get(0),
                            )
                            .unwrap();
                        let prev = mc.load(Ordering::Relaxed);
                        if count > prev {
                            mc.store(count, Ordering::Relaxed);
                        }
                        let row = task_row_json(&conn, &tid);
                        let agents = integrate_agents_yaml(cmd);
                        let cfg = cfg_path();
                        let ctx = DispatchCtx {
                            conn: &conn,
                            agents: &agents,
                            config_path: &cfg,
                            policies_hash: "",
                        };
                        let _ = run(&row, &ctx);
                        std::thread::sleep(Duration::from_millis(20));
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert!(
            max_concurrent.load(Ordering::Relaxed) <= 1,
            "max concurrent integrating rows must be ≤ 1; saw {}",
            max_concurrent.load(Ordering::Relaxed)
        );

        let conn = Connection::open(&db_path).unwrap();
        // All three integrated.
        for i in 0..branches.len() {
            let tid = format!("T9{:02}", i);
            let status: String = conn
                .query_row(
                    "SELECT status FROM tasks WHERE display_id=?1",
                    rusqlite::params![tid],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(status, "integrated", "{} not integrated", tid);
        }

        // SHA chain: collect (base, landed) for each row and verify they
        // form a single chain where each successor's base equals the
        // predecessor's landed. `now_iso8601` is only second-precision, so
        // sorting by `started_at` would be unreliable under concurrency;
        // we determine integration order from the SHA graph instead.
        let mut entries: Vec<(String, String, String)> = Vec::new();
        for i in 0..branches.len() {
            let tid = format!("T9{:02}", i);
            let base =
                json_extract_str(&conn, &tid, "$[#-1].base_main_sha").unwrap_or_default();
            let landed =
                json_extract_str(&conn, &tid, "$[#-1].landed_main_sha").unwrap_or_default();
            assert!(!base.is_empty() && !landed.is_empty(), "{} SHAs empty", tid);
            entries.push((tid, base, landed));
        }
        // Find the head of the chain: the entry whose base is NOT any
        // other entry's landed.
        let landed_set: std::collections::HashSet<String> =
            entries.iter().map(|e| e.2.clone()).collect();
        let mut current = entries
            .iter()
            .find(|e| !landed_set.contains(&e.1))
            .expect("at least one entry must have a base outside the landed set")
            .clone();
        let mut visited = vec![current.clone()];
        // base→entry index for follow-up.
        let mut by_base: std::collections::HashMap<String, (String, String, String)> =
            std::collections::HashMap::new();
        for e in &entries {
            by_base.insert(e.1.clone(), e.clone());
        }
        // Walk forward: successor.base must equal current.landed.
        while visited.len() < entries.len() {
            let next = by_base
                .get(&current.2)
                .cloned()
                .unwrap_or_else(|| panic!(
                    "no successor whose base equals predecessor landed {}",
                    current.2
                ));
            assert_eq!(
                next.1, current.2,
                "{} base_main_sha must equal predecessor {} landed_main_sha",
                next.0, current.0
            );
            visited.push(next.clone());
            current = next;
        }
        assert_eq!(visited.len(), entries.len(), "chain must cover all entries");
    }

    /// (i) Refresh and pre_land_check must run with HEAD on the candidate
    /// branch. Set up workspace_path on `main` (the failure surface from
    /// review 2), then have pre_land_check write `git rev-parse
    /// --abbrev-ref HEAD` to a probe file. Assert the probe says the
    /// candidate branch, not `main`.
    #[test]
    fn i_pre_land_runs_on_candidate_branch_checkout() {
        let conn = fresh_db();
        let (_tmp, repo) = init_repo();
        add_branch_with_change(&repo, "feat/i", "feat.txt", "feat\n");
        // Sanity: add_branch_with_change leaves the worktree on main.
        let starting = String::from_utf8_lossy(
            &git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).stdout,
        )
        .trim()
        .to_string();
        assert_eq!(starting, "main");

        insert_queued_task(&conn, "T1000", "feat/i", repo.to_str().unwrap());

        let probe = repo.join("probe.head");
        // Use absolute path so cwd-changes don't affect it.
        let cmd = format!(
            "git rev-parse --abbrev-ref HEAD > {}",
            probe.to_str().unwrap()
        );
        let agents = integrate_agents_yaml(&cmd);
        let cfg = cfg_path();
        let row = task_row_json(&conn, "T1000");
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };
        run(&row, &ctx).unwrap();

        let observed = std::fs::read_to_string(&probe).unwrap_or_default();
        assert_eq!(
            observed.trim(),
            "feat/i",
            "pre_land_check must run with HEAD on the candidate branch, saw {:?}",
            observed
        );

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T1000'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "integrated");
    }

    /// (h) Stale_base BLOCKING path: ER row's base_sha is orphaned (not
    /// reachable from current main). Lane must block before any rebase
    /// runs; main HEAD and candidate branch HEAD must be untouched; ER
    /// must transition to 'superseded'.
    #[test]
    fn h_stale_base_blocks_without_advancing_main() {
        let conn = fresh_db();
        let (_tmp, repo) = init_repo();
        // Build a branch from current main.
        add_branch_with_change(&repo, "feat/h", "feat.txt", "feat\n");
        // Capture the original "main" SHA (which the ER row will reference
        // as base_sha) and the candidate's pre-test HEAD.
        let orphaned_base = String::from_utf8_lossy(
            &git(&repo, &["rev-parse", "main"]).stdout,
        )
        .trim()
        .to_string();
        let candidate_head_before = String::from_utf8_lossy(
            &git(&repo, &["rev-parse", "feat/h"]).stdout,
        )
        .trim()
        .to_string();
        // Force-rewrite main: orphan the original SHA.
        git(&repo, &["checkout", "--orphan", "fresh"]);
        git(&repo, &["rm", "-rf", "."]);
        std::fs::write(repo.join("fresh.txt"), "fresh\n").unwrap();
        git(&repo, &["add", "fresh.txt"]);
        git(&repo, &["commit", "-m", "fresh init"]);
        git(&repo, &["branch", "-M", "fresh", "main"]);
        let new_main = String::from_utf8_lossy(
            &git(&repo, &["rev-parse", "main"]).stdout,
        )
        .trim()
        .to_string();
        assert_ne!(orphaned_base, new_main);

        insert_queued_task(&conn, "T800", "feat/h", repo.to_str().unwrap());
        insert_passed_er(&conn, "ER800", "T800", 1, &orphaned_base, &candidate_head_before);

        let row = task_row_json(&conn, "T800");
        let agents = integrate_agents_yaml("true");
        let cfg = cfg_path();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &cfg,
            policies_hash: "",
        };
        run(&row, &ctx).unwrap();

        // (i) tasks status → integration_blocked
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T800'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "integration_blocked");
        // (ii) outcome = stale_base
        assert_eq!(
            json_extract_str(&conn, "T800", "$[#-1].outcome").as_deref(),
            Some("stale_base")
        );
        // (iii) reviewed_base_sha = orphaned_base
        assert_eq!(
            json_extract_str(&conn, "T800", "$[#-1].reviewed_base_sha"),
            Some(orphaned_base.clone())
        );
        // (iv) integration_blocked_reason contains 'stale_base'
        let reason: Option<String> = conn
            .query_row(
                "SELECT integration_blocked_reason FROM tasks WHERE display_id='T800'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            reason.unwrap_or_default().contains("stale_base"),
            "integration_blocked_reason must contain 'stale_base'"
        );
        // (v) main HEAD unchanged
        let post_main = String::from_utf8_lossy(
            &git(&repo, &["rev-parse", "main"]).stdout,
        )
        .trim()
        .to_string();
        assert_eq!(post_main, new_main, "main HEAD must be unchanged");
        // (vi) candidate branch HEAD unchanged
        let post_branch = String::from_utf8_lossy(
            &git(&repo, &["rev-parse", "feat/h"]).stdout,
        )
        .trim()
        .to_string();
        assert_eq!(
            post_branch, candidate_head_before,
            "candidate branch HEAD must be unchanged"
        );
        // (vii) ER row → superseded
        let er_status: String = conn
            .query_row(
                "SELECT status FROM external_reviews WHERE display_id='ER800'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(er_status, "superseded");
    }
}
