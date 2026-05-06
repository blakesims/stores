//! `builtin:investigator` — observations: open → needs_investigation handler.
//!
//! Spawns a subagent (via `STORES_INVESTIGATOR_CMD`, a shell command), reads
//! a pull-shaped evidence envelope from stdout, validates that the envelope
//! contains the required fields and NO forbidden contract-shaped fields, and
//! persists the evidence into the observation row's `investigation_note` and
//! `notes` JSON columns.
//!
//! Pull-shape doctrine (T038): the investigator outputs evidence for the
//! human to prune, NOT a draft contract. Envelopes carrying any of
//! `draft_contract`, `intent_contract`, `done_when`, `scope_in`, `scope_out`,
//! `acceptance`, `objective` are rejected fail-loud.
//!
//! Status guard: re-reads the row before persisting; if `status` is no
//! longer `needs_investigation` (e.g. a human already advanced it), the
//! persist is a no-op. Prevents clobbering work-in-progress when the
//! subagent has been running while the row moved on.
//!
//! Idempotency: writing investigation_note is unconditional on each run. The
//! subscriber-edge guard (open → needs_investigation only fires once) is
//! the upstream idempotency primitive.

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::process::Command;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::handlers::row::now_iso8601;

/// Forbidden top-level fields — presence of any rejects the envelope.
const FORBIDDEN_FIELDS: &[&str] = &[
    "draft_contract",
    "intent_contract",
    "done_when",
    "scope_in",
    "scope_out",
    "acceptance",
    "objective",
];

const REQUIRED_FIELDS: &[&str] = &[
    "evidence",
    "duplicate_candidates",
    "confidence",
    "proposed_tier",
    "grill_question",
];

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row
        .get("display_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if display_id.is_empty() {
        eprintln!("[investigator] observation row missing display_id; skipping");
        return Ok(1);
    }

    // Spawn the subagent (or test shim) and capture stdout.
    let envelope_text = match invoke_subagent(display_id) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[investigator] {}: subagent invocation failed: {:#}", display_id, e);
            return Ok(1);
        }
    };

    let envelope: Value = match serde_json::from_str(&envelope_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[investigator] {}: envelope is not valid JSON: {}",
                display_id, e
            );
            return Ok(1);
        }
    };

    if let Err(e) = validate_pull_envelope(&envelope) {
        eprintln!("[investigator] {}: envelope rejected: {:#}", display_id, e);
        return Ok(1);
    }

    // Status guard: re-read the row; abort if no longer needs_investigation.
    let current_status: Option<String> = ctx
        .conn
        .query_row(
            "SELECT status FROM observations WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .ok();
    match current_status.as_deref() {
        Some("needs_investigation") => {}
        Some(other) => {
            eprintln!(
                "[investigator] {}: status is '{}' (expected 'needs_investigation'); persist is a no-op",
                display_id, other
            );
            return Ok(0);
        }
        None => {
            eprintln!("[investigator] {}: row not found; aborting", display_id);
            return Ok(1);
        }
    }

    if let Err(e) = persist(ctx.conn, display_id, &envelope) {
        eprintln!("[investigator] {}: persist failed: {:#}", display_id, e);
        return Ok(1);
    }

    eprintln!("[investigator] {}: evidence persisted", display_id);
    Ok(0)
}

/// Spawn the configured subagent command (or test shim) and return its
/// stdout. Resolution order: `STORES_INVESTIGATOR_CMD` env var (test/prod
/// override) — fail-loud if unset.
fn invoke_subagent(display_id: &str) -> Result<String> {
    let cmd_str = std::env::var("STORES_INVESTIGATOR_CMD").map_err(|_| {
        anyhow!(
            "STORES_INVESTIGATOR_CMD not set; cannot invoke investigator subagent"
        )
    })?;
    let output = Command::new("sh")
        .arg("-c")
        .arg(&cmd_str)
        .env("STORES_DISPLAY_ID", display_id)
        .env("STORES_STORE", "observations")
        .output()
        .with_context(|| format!("spawning STORES_INVESTIGATOR_CMD: {cmd_str}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "subagent exited non-zero: {} (stderr: {})",
            output.status,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Validate the pull-shaped envelope: required fields present, forbidden
/// fields absent, type-checks on the simple scalar fields. Does NOT perform
/// full JSON-Schema validation — the four mechanical checks below are what
/// the schema gate enforces.
fn validate_pull_envelope(envelope: &Value) -> Result<()> {
    let obj = envelope
        .as_object()
        .ok_or_else(|| anyhow!("envelope must be a JSON object"))?;

    for forbidden in FORBIDDEN_FIELDS {
        if obj.contains_key(*forbidden) {
            return Err(anyhow!(
                "forbidden field '{}' present (pull-shape doctrine: investigator must not draft a contract)",
                forbidden
            ));
        }
    }

    for required in REQUIRED_FIELDS {
        if !obj.contains_key(*required) {
            return Err(anyhow!("missing required field '{}'", required));
        }
    }

    if !obj["evidence"].is_array() {
        return Err(anyhow!("'evidence' must be an array"));
    }
    if !obj["duplicate_candidates"].is_array() {
        return Err(anyhow!("'duplicate_candidates' must be an array"));
    }
    let confidence = obj["confidence"]
        .as_str()
        .ok_or_else(|| anyhow!("'confidence' must be a string"))?;
    if !["low", "medium", "high"].contains(&confidence) {
        return Err(anyhow!(
            "'confidence' must be one of low|medium|high; got '{}'",
            confidence
        ));
    }
    let tier = obj["proposed_tier"]
        .as_str()
        .ok_or_else(|| anyhow!("'proposed_tier' must be a string"))?;
    if !["T0", "T1", "T2", "T3"].contains(&tier) {
        return Err(anyhow!(
            "'proposed_tier' must be one of T0|T1|T2|T3; got '{}'",
            tier
        ));
    }
    let grill = obj["grill_question"]
        .as_str()
        .ok_or_else(|| anyhow!("'grill_question' must be a string"))?;
    if grill.chars().count() > 200 {
        return Err(anyhow!("'grill_question' exceeds 200 chars"));
    }

    Ok(())
}

/// Persist the validated envelope. Writes:
///   * `investigation_note` — compact JSON-stringified envelope
///   * `notes` — merged JSON object containing duplicate_candidates,
///     confidence, proposed_tier, grill_question (preserves any pre-existing
///     keys outside this set)
/// No transition is fired; the human reviews evidence next.
fn persist(conn: &Connection, display_id: &str, envelope: &Value) -> Result<()> {
    let envelope_str =
        serde_json::to_string(envelope).context("serialize investigator envelope")?;

    // Merge into existing notes JSON (if any).
    let existing_notes: Option<String> = conn
        .query_row(
            "SELECT notes FROM observations WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let mut notes_obj = match existing_notes {
        Some(s) if !s.is_empty() => serde_json::from_str::<Value>(&s)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default(),
        _ => serde_json::Map::new(),
    };
    for key in ["duplicate_candidates", "confidence", "proposed_tier", "grill_question"] {
        if let Some(v) = envelope.get(key) {
            notes_obj.insert(key.to_string(), v.clone());
        }
    }
    let notes_str = serde_json::to_string(&Value::Object(notes_obj))
        .context("serialize merged notes")?;

    let now = now_iso8601();
    conn.execute(
        "UPDATE observations \
         SET investigation_note = ?1, notes = ?2, updated_at = ?3, updated_by = 'ai_autonomous' \
         WHERE display_id = ?4",
        rusqlite::params![envelope_str, notes_str, now, display_id],
    )
    .context("UPDATE observations (investigator persist)")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::flow::AgentsYaml;
    use crate::schema::Schema;
    use rusqlite::Connection;
    use std::sync::Mutex;

    /// STORES_INVESTIGATOR_CMD is process-global; serialize tests that touch it.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        for name in ["observations"] {
            let yaml = BUNDLED_STORE_SCHEMAS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, y)| *y)
                .unwrap();
            let schema = Schema::from_yaml(yaml).unwrap();
            conn.execute_batch(&ddl_for(&schema)).unwrap();
        }
        conn
    }

    fn insert_obs(conn: &Connection, display_id: &str, status: &str) {
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, captured_week, \
              created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'test obs', 'dev', 'normal', ?3, 'w-test', ?3, ?3, 'ai_autonomous', 'ai_autonomous')",
            rusqlite::params![display_id, status, "2026-05-06T00:00:00Z"],
        )
        .unwrap();
    }

    fn obs_row_json(conn: &Connection, display_id: &str) -> Value {
        let mut stmt = conn
            .prepare("SELECT * FROM observations WHERE display_id = ?1")
            .unwrap();
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt.query(rusqlite::params![display_id]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i).unwrap();
            let jv = match v {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(n) => Value::from(n),
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

    fn ctx_for<'a>(
        conn: &'a Connection,
        agents: &'a AgentsYaml,
        cfg: &'a std::path::Path,
    ) -> DispatchCtx<'a> {
        DispatchCtx {
            conn,
            agents,
            config_path: cfg,
            policies_hash: "",
        }
    }

    /// Set STORES_INVESTIGATOR_CMD to a `printf` that emits the given JSON
    /// blob on stdout and exits 0. RAII unset on drop.
    struct CmdShim;
    impl CmdShim {
        fn install(json: &str) -> Self {
            // Use printf with %s to safely embed quotes/braces.
            let escaped = json.replace('\'', r"'\''");
            let cmd = format!("printf '%s' '{}'", escaped);
            std::env::set_var("STORES_INVESTIGATOR_CMD", cmd);
            CmdShim
        }
    }
    impl Drop for CmdShim {
        fn drop(&mut self) {
            std::env::remove_var("STORES_INVESTIGATOR_CMD");
        }
    }

    fn valid_envelope() -> Value {
        serde_json::json!({
            "evidence": [
                "git log shows the panic was added in 7703608",
                {"file": "src/foo.rs", "line": 142, "snippet": "panic!(\"x\")"}
            ],
            "duplicate_candidates": [
                {"l_id": "L042", "similarity_reason": "same module, same panic"}
            ],
            "confidence": "high",
            "proposed_tier": "T2",
            "grill_question": "Is the panic intentional for the bounded path?"
        })
    }

    #[test]
    fn investigator_pull_envelope_round_trip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db();
        insert_obs(&conn, "L001", "needs_investigation");
        let env = valid_envelope();
        let _shim = CmdShim::install(&serde_json::to_string(&env).unwrap());

        let row = obs_row_json(&conn, "L001");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);

        let res = run(&row, &ctx).unwrap();
        assert_eq!(res, 0, "valid envelope must return Ok(0)");

        let (note, notes_str): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT investigation_note, notes FROM observations WHERE display_id='L001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let note = note.expect("investigation_note must be set");
        let parsed: Value = serde_json::from_str(&note).unwrap();
        // Round-trip: every field present verbatim.
        assert_eq!(parsed.get("confidence").and_then(|v| v.as_str()), Some("high"));
        assert_eq!(parsed.get("proposed_tier").and_then(|v| v.as_str()), Some("T2"));
        assert_eq!(
            parsed.get("grill_question").and_then(|v| v.as_str()),
            Some("Is the panic intentional for the bounded path?")
        );
        let evidence = parsed.get("evidence").and_then(|v| v.as_array()).unwrap();
        assert_eq!(evidence.len(), 2);
        let dups = parsed
            .get("duplicate_candidates")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(dups.len(), 1);

        // notes column also has the four merged keys.
        let notes_str = notes_str.expect("notes must be populated");
        let notes: Value = serde_json::from_str(&notes_str).unwrap();
        for key in ["duplicate_candidates", "confidence", "proposed_tier", "grill_question"] {
            assert!(notes.get(key).is_some(), "notes must contain '{}'", key);
        }
    }

    #[test]
    fn investigator_rejects_push_shaped_payload() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db();
        insert_obs(&conn, "L002", "needs_investigation");

        let mut env = valid_envelope();
        env.as_object_mut().unwrap().insert(
            "draft_contract".to_string(),
            serde_json::json!({"objective": "do thing"}),
        );
        let _shim = CmdShim::install(&serde_json::to_string(&env).unwrap());

        let row = obs_row_json(&conn, "L002");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);

        let res = run(&row, &ctx).unwrap();
        assert_eq!(res, 1, "push-shaped envelope must be rejected (non-zero)");

        let note: Option<String> = conn
            .query_row(
                "SELECT investigation_note FROM observations WHERE display_id='L002'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(note.is_none(), "rejected envelope must not persist");
    }

    #[test]
    fn investigator_status_guard_protects_against_clobber() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db();
        // Row is at status=open (NOT needs_investigation) — simulates a row
        // that moved on between subscriber claim and subagent completion.
        insert_obs(&conn, "L003", "open");
        let env = valid_envelope();
        let _shim = CmdShim::install(&serde_json::to_string(&env).unwrap());

        let row = obs_row_json(&conn, "L003");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);

        let res = run(&row, &ctx).unwrap();
        assert_eq!(res, 0, "status-guard skip is Ok(0) (not a hard failure)");

        let note: Option<String> = conn
            .query_row(
                "SELECT investigation_note FROM observations WHERE display_id='L003'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            note.is_none(),
            "investigation_note must remain NULL when status guard fires"
        );

        // No transition_history row tagged investigator was written.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE store='observations' AND display_id='L003'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn investigator_subscriber_fires_on_needs_investigation() {
        // End-to-end: parse the bundled tests/fixtures/agents.yaml and assert
        // that an investigator agent is registered with the right subscriber
        // shape (store=observations, transition open→needs_investigation,
        // command=builtin:investigator).
        let agents_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agents.yaml");
        let agents = crate::flow::agents_yaml::load_from_path(&agents_path)
            .expect("tests/fixtures/agents.yaml must parse");

        let inv = agents
            .agents
            .iter()
            .find(|a| a.name == "investigator")
            .expect("investigator agent must be registered");
        assert_eq!(inv.command, "builtin:investigator");

        let sub = inv
            .subscribes_to
            .iter()
            .find(|s| s.store == "observations")
            .expect("investigator must subscribe to observations");
        assert_eq!(sub.transition.from, "open");
        assert_eq!(sub.transition.to, "needs_investigation");

        // dispatch_builtin returns Some for "investigator" keyword.
        let conn = fresh_db();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);
        let row = serde_json::json!({"display_id": ""});
        let res = crate::flow::builtins::dispatch_builtin("investigator", &row, &ctx);
        assert!(res.is_some(), "'investigator' keyword must resolve in dispatch_builtin");
    }
}
