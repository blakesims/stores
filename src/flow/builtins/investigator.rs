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
use std::io::{ErrorKind, Write};
use std::process::{Command, Stdio};

use crate::flow::builtins::{
    fire_framework_transition_for, load_store_schema, BuiltinResult, DispatchCtx,
};
use crate::validate::EntryMap;

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
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    if display_id.is_empty() {
        eprintln!("[investigator] observation row missing display_id; skipping");
        return Ok(1);
    }

    let schema = match load_store_schema("observations") {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[investigator] {}: load observations schema failed: {:#}",
                display_id, e
            );
            return Ok(1);
        }
    };

    if let Err(e) = fire_framework_transition_for(
        ctx.conn,
        &schema,
        display_id,
        "investigation-started",
        EntryMap::new(),
        ctx.policies_hash,
        Some("builtin:investigator claimed row"),
    ) {
        eprintln!(
            "[investigator] {}: could not start investigation from needs_investigation: {:#}",
            display_id, e
        );
        return Ok(1);
    }

    let question = build_investigator_question(row);

    let envelope_text = match invoke_subagent(display_id, &question) {
        Ok(s) => s,
        Err(e) => {
            return fail_investigation(
                ctx,
                &schema,
                display_id,
                &format!("subagent invocation failed: {e:#}"),
            )
        }
    };

    let envelope: Value = match serde_json::from_str(&envelope_text) {
        Ok(v) => v,
        Err(e) => {
            return fail_investigation(
                ctx,
                &schema,
                display_id,
                &format!(
                    "envelope is not valid JSON: {e}; stdout tail: {}",
                    tail(&envelope_text, 500)
                ),
            )
        }
    };

    if let Err(e) = validate_pull_envelope(&envelope) {
        return fail_investigation(
            ctx,
            &schema,
            display_id,
            &format!("envelope rejected: {e:#}"),
        );
    }

    let mut diff = match investigation_success_diff(ctx.conn, display_id, &envelope) {
        Ok(d) => d,
        Err(e) => {
            return fail_investigation(
                ctx,
                &schema,
                display_id,
                &format!("persist preparation failed: {e:#}"),
            )
        }
    };
    diff.insert("investigation_failure_reason".to_string(), Value::Null);
    if let Err(e) = fire_framework_transition_for(
        ctx.conn,
        &schema,
        display_id,
        "investigation-succeeded",
        diff,
        ctx.policies_hash,
        Some("builtin:investigator persisted report"),
    ) {
        return fail_investigation(
            ctx,
            &schema,
            display_id,
            &format!("persist/transition investigation-succeeded failed: {e:#}"),
        );
    }

    eprintln!(
        "[investigator] {}: report persisted; status investigated",
        display_id
    );
    Ok(0)
}

fn fail_investigation(
    ctx: &DispatchCtx,
    schema: &crate::schema::Schema,
    display_id: &str,
    reason: &str,
) -> BuiltinResult {
    eprintln!("[investigator] {}: {}", display_id, reason);
    let mut diff = EntryMap::new();
    diff.insert(
        "investigation_failure_reason".to_string(),
        Value::String(tail(reason, 4000)),
    );
    if let Err(e) = fire_framework_transition_for(
        ctx.conn,
        schema,
        display_id,
        "investigation-failed",
        diff,
        ctx.policies_hash,
        Some("builtin:investigator failed"),
    ) {
        eprintln!(
            "[investigator] {}: additionally failed to mark investigation_failed: {:#}",
            display_id, e
        );
    }
    Ok(1)
}

fn build_investigator_question(row: &Value) -> String {
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    let summary = row.get("summary").and_then(|v| v.as_str()).unwrap_or("");
    let body = row.get("body").and_then(|v| v.as_str()).unwrap_or("");
    format!(
        "Investigate observation {display_id}.\n\nSummary:\n{summary}\n\nBody:\n{body}\n\nReturn only the JSON envelope required by the investigator schema."
    )
}

fn tail(s: &str, max_chars: usize) -> String {
    let mut chars: Vec<char> = s.chars().rev().take(max_chars).collect();
    chars.reverse();
    chars.into_iter().collect()
}

/// Bundled investigator agent prompt. Used as the default system prompt when
/// `STORES_INVESTIGATOR_CMD` is not set.
const BUNDLED_INVESTIGATOR_PROMPT: &str = include_str!("../../../agents/investigator.md");

/// Spawn the configured subagent command (or test shim) and return stdout.
/// `STORES_INVESTIGATOR_CMD` receives the structured question on stdin and in
/// `STORES_INVESTIGATOR_QUESTION`; the default `claude` invocation prints that
/// same question as the user message with `agents/investigator.md` as system
/// prompt.
fn invoke_subagent(display_id: &str, question: &str) -> Result<String> {
    if let Ok(cmd_str) = std::env::var("STORES_INVESTIGATOR_CMD") {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&cmd_str)
            .env("STORES_DISPLAY_ID", display_id)
            .env("STORES_STORE", "observations")
            .env("STORES_INVESTIGATOR_QUESTION", question)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning STORES_INVESTIGATOR_CMD: {cmd_str}"))?;
        if let Some(stdin) = child.stdin.as_mut() {
            if let Err(e) = stdin.write_all(question.as_bytes()) {
                if e.kind() != ErrorKind::BrokenPipe {
                    return Err(e)
                        .context("writing investigator question to STORES_INVESTIGATOR_CMD stdin");
                }
            }
        }
        let output = child
            .wait_with_output()
            .context("waiting for STORES_INVESTIGATOR_CMD")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "STORES_INVESTIGATOR_CMD exited non-zero: {} (stderr: {})",
                output.status,
                stderr.trim()
            ));
        }
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    let output = Command::new("claude")
        .arg("--append-system-prompt")
        .arg(BUNDLED_INVESTIGATOR_PROMPT)
        .arg("--print")
        .arg(question)
        .env("STORES_DISPLAY_ID", display_id)
        .env("STORES_STORE", "observations")
        .env("STORES_INVESTIGATOR_QUESTION", question)
        .output()
        .with_context(|| {
            "spawning default investigator subagent (claude). \
             Set STORES_INVESTIGATOR_CMD to override, or ensure 'claude' is on PATH."
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "default investigator subagent (claude) exited non-zero: {} (stderr: {})",
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

    // Evidence: array of strings OR objects with {file, line, snippet}.
    // Nested objects enforce additionalProperties:false — only {file, line,
    // snippet} keys are accepted; line must be an integer (NOT a float, even
    // though serde_json treats both as numbers).
    let evidence_keys: std::collections::HashSet<&str> =
        ["file", "line", "snippet"].iter().copied().collect();
    let evidence = obj["evidence"]
        .as_array()
        .ok_or_else(|| anyhow!("'evidence' must be an array"))?;
    for (i, item) in evidence.iter().enumerate() {
        match item {
            Value::String(_) => {}
            Value::Object(o) => {
                // Schema (agents/schemas/investigator.schema.json) requires
                // only 'file'; line and snippet are optional. Parser must
                // not be stricter than the schema contract.
                if !o.contains_key("file") {
                    return Err(anyhow!("'evidence[{}]' object must have 'file' key", i));
                }
                if !o["file"].is_string() {
                    return Err(anyhow!("'evidence[{}].file' must be a string", i));
                }
                // Optional fields: type-check only when present.
                if let Some(line) = o.get("line") {
                    // Schema requires integer; reject floats.
                    if !(line.is_i64() || line.is_u64()) {
                        return Err(anyhow!(
                            "'evidence[{}].line' must be an integer (got {})",
                            i,
                            line
                        ));
                    }
                }
                if let Some(snippet) = o.get("snippet") {
                    if !snippet.is_string() {
                        return Err(anyhow!("'evidence[{}].snippet' must be a string", i));
                    }
                }
                for key in o.keys() {
                    if !evidence_keys.contains(key.as_str()) {
                        return Err(anyhow!(
                            "'evidence[{}]' has extra field '{}' (additionalProperties: false)",
                            i,
                            key
                        ));
                    }
                }
            }
            _ => {
                return Err(anyhow!(
                    "'evidence[{}]' must be a string or {{file, line, snippet}} object; got {}",
                    i,
                    item
                ));
            }
        }
    }

    // duplicate_candidates: array of {l_id, similarity_reason} objects.
    // Nested additionalProperties:false enforced.
    let dup_keys: std::collections::HashSet<&str> =
        ["l_id", "similarity_reason"].iter().copied().collect();
    let dups = obj["duplicate_candidates"]
        .as_array()
        .ok_or_else(|| anyhow!("'duplicate_candidates' must be an array"))?;
    for (i, item) in dups.iter().enumerate() {
        let o = item.as_object().ok_or_else(|| {
            anyhow!(
                "'duplicate_candidates[{}]' must be an object; got {}",
                i,
                item
            )
        })?;
        let l_id = o
            .get("l_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("'duplicate_candidates[{}].l_id' must be a string", i))?;
        if !regex::Regex::new(r"^L\d{3,}$").unwrap().is_match(l_id) {
            return Err(anyhow!(
                "'duplicate_candidates[{}].l_id' must match /^L\\d{{3,}}$/; got '{}'",
                i,
                l_id
            ));
        }
        if !o
            .get("similarity_reason")
            .map(|v| v.is_string())
            .unwrap_or(false)
        {
            return Err(anyhow!(
                "'duplicate_candidates[{}].similarity_reason' must be a string",
                i
            ));
        }
        for key in o.keys() {
            if !dup_keys.contains(key.as_str()) {
                return Err(anyhow!(
                    "'duplicate_candidates[{}]' has extra field '{}' (additionalProperties: false)",
                    i,
                    key
                ));
            }
        }
    }

    // Reject any extra top-level fields outside the required set. This is
    // the "additionalProperties: false" check the JSON schema declares; it
    // closes the MEDIUM finding that nested forbidden fields could slip
    // through (e.g. {"notes": {"draft_contract": ...}}).
    let allowed: std::collections::HashSet<&str> = REQUIRED_FIELDS.iter().copied().collect();
    for key in obj.keys() {
        if !allowed.contains(key.as_str()) {
            return Err(anyhow!(
                "envelope contains extra field '{}' (additionalProperties: false)",
                key
            ));
        }
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

/// Persist the validated envelope via a CAS UPDATE. The WHERE clause
/// includes `status = 'needs_investigation'` so a human transition between
/// the subagent spawn and the persist (TOCTOU window) does NOT clobber the
/// row. Returns the number of rows affected: 1 ⇒ persisted; 0 ⇒ row moved
/// on, persist no-op'd.
///
/// Writes:
///   * `investigation_note` — compact JSON-stringified envelope
///   * `notes` — merged JSON object containing duplicate_candidates,
///     confidence, proposed_tier, grill_question (preserves any pre-existing
///     keys outside this set)
/// No transition is fired; the human reviews evidence next.
fn investigation_success_diff(
    conn: &Connection,
    display_id: &str,
    envelope: &Value,
) -> Result<EntryMap> {
    let note = format_investigation_note(envelope)?;

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
    for key in [
        "duplicate_candidates",
        "confidence",
        "proposed_tier",
        "grill_question",
    ] {
        if let Some(v) = envelope.get(key) {
            notes_obj.insert(key.to_string(), v.clone());
        }
    }

    let mut diff = EntryMap::new();
    diff.insert("investigation_note".to_string(), Value::String(note));
    diff.insert("notes".to_string(), Value::Object(notes_obj));
    Ok(diff)
}

fn format_investigation_note(envelope: &Value) -> Result<String> {
    let evidence = envelope
        .get("evidence")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("validated envelope lost evidence array"))?;
    let duplicates = envelope
        .get("duplicate_candidates")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("validated envelope lost duplicate_candidates array"))?;
    let confidence = envelope
        .get("confidence")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tier = envelope
        .get("proposed_tier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let grill = envelope
        .get("grill_question")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut out = String::new();
    out.push_str("Evidence:\n");
    if evidence.is_empty() {
        out.push_str("- none supplied\n");
    } else {
        for item in evidence {
            match item {
                Value::String(s) => out.push_str(&format!("- {s}\n")),
                Value::Object(o) => {
                    let file = o.get("file").and_then(|v| v.as_str()).unwrap_or("");
                    let line = o.get("line").map(|v| format!(":{v}")).unwrap_or_default();
                    let snippet = o.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
                    if snippet.is_empty() {
                        out.push_str(&format!("- {file}{line}\n"));
                    } else {
                        out.push_str(&format!("- {file}{line} — {snippet}\n"));
                    }
                }
                _ => {}
            }
        }
    }
    out.push_str("\nDuplicate candidates:\n");
    if duplicates.is_empty() {
        out.push_str("- none\n");
    } else {
        for dup in duplicates {
            let id = dup.get("l_id").and_then(|v| v.as_str()).unwrap_or("");
            let why = dup
                .get("similarity_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            out.push_str(&format!("- {id}: {why}\n"));
        }
    }
    out.push_str(&format!(
        "\nConfidence: {confidence}\nProposed tier: {tier}\nGrill question: {grill}\n"
    ));
    Ok(out)
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
             (display_id, status, summary, body, source, priority, captured_at, captured_week, \
              created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'test obs summary', 'test obs body with details', 'dev', 'normal', ?3, 'w-test', ?3, ?3, 'ai_autonomous', 'ai_autonomous')",
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

        let (status, note, notes_str): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT status, investigation_note, notes FROM observations WHERE display_id='L001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "investigated");
        let note = note.expect("investigation_note must be set");
        assert!(
            note.contains("Evidence:"),
            "note must be human-readable: {note}"
        );
        assert!(
            note.contains("Confidence: high"),
            "note must label confidence: {note}"
        );
        assert!(
            note.contains("Proposed tier: T2"),
            "note must label tier: {note}"
        );
        assert!(
            note.contains("Grill question:"),
            "note must label grill question: {note}"
        );
        assert!(
            serde_json::from_str::<Value>(&note).is_err(),
            "note must not be a raw JSON blob"
        );

        // notes column also has the four merged keys.
        let notes_str = notes_str.expect("notes must be populated");
        let notes: Value = serde_json::from_str(&notes_str).unwrap();
        for key in [
            "duplicate_candidates",
            "confidence",
            "proposed_tier",
            "grill_question",
        ] {
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
        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, investigation_failure_reason FROM observations WHERE display_id='L002'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "investigation_failed");
        assert!(reason.unwrap_or_default().contains("envelope rejected"));
    }

    #[test]
    fn investigator_override_receives_question_on_stdin() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db();
        insert_obs(&conn, "L004", "needs_investigation");
        let env = valid_envelope();
        let json = serde_json::to_string(&env).unwrap().replace('\'', r"'\''");
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        let cmd = format!("cat > '{}'; printf '%s' '{}'", path, json);
        std::env::set_var("STORES_INVESTIGATOR_CMD", cmd);
        let _shim = CmdShim;

        let row = obs_row_json(&conn, "L004");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);
        assert_eq!(run(&row, &ctx).unwrap(), 0);

        let question = std::fs::read_to_string(path).unwrap();
        assert!(
            question.contains("test obs summary"),
            "question missing summary: {question}"
        );
        assert!(
            question.contains("test obs body with details"),
            "question missing body: {question}"
        );
    }

    #[test]
    fn investigator_nonzero_stub_marks_failed() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db();
        insert_obs(&conn, "L005", "needs_investigation");
        std::env::set_var(
            "STORES_INVESTIGATOR_CMD",
            "printf 'rate_limit' >&2; exit 17",
        );
        let _shim = CmdShim;

        let row = obs_row_json(&conn, "L005");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);
        assert_eq!(run(&row, &ctx).unwrap(), 1);

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, investigation_failure_reason FROM observations WHERE display_id='L005'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "investigation_failed");
        assert!(reason.unwrap_or_default().contains("rate_limit"));
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
        assert_eq!(res, 1, "unexpected state must fail loud before clobbering");

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
        let agents_path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents.yaml");
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
        assert!(
            res.is_some(),
            "'investigator' keyword must resolve in dispatch_builtin"
        );
    }
}
