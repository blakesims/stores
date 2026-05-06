use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::schema::{
    actor::{Actor, InvokerCtx},
    FieldType, Schema,
};
use crate::validate::{self, Op};

use super::row::{build_entry_map, now_iso8601, read_row};
use super::transition::read_policy_env;

/// POLICY_RANK: maps approval_policy to a numeric rank for direction detection.
/// Lower rank = more permissive. Relaxation = new_rank < old_rank.
fn policy_rank(policy: &str) -> Option<u8> {
    match policy {
        "auto" => Some(0),
        "human" => Some(1),
        "architecture" => Some(2),
        _ => None,
    }
}

/// Convert serde_json::Value to rusqlite::types::Value appropriate for the field type.
fn json_to_sql(ty: &FieldType, val: &Value) -> rusqlite::types::Value {
    match ty {
        FieldType::List(_)
        | FieldType::ListRecord(_)
        | FieldType::ListFk { .. }
        | FieldType::Json => {
            let s = serde_json::to_string(val).unwrap_or_else(|_| "null".to_string());
            rusqlite::types::Value::Text(s)
        }
        FieldType::Bool => {
            let i = match val {
                Value::Bool(b) => {
                    if *b {
                        1
                    } else {
                        0
                    }
                }
                Value::Number(n) => n.as_i64().unwrap_or(0),
                _ => 0,
            };
            rusqlite::types::Value::Integer(i)
        }
        FieldType::Integer => {
            let i = match val {
                Value::Number(n) => n.as_i64().unwrap_or(0),
                _ => 0,
            };
            rusqlite::types::Value::Integer(i)
        }
        _ => match val {
            Value::String(s) => rusqlite::types::Value::Text(s.clone()),
            Value::Null => rusqlite::types::Value::Null,
            other => rusqlite::types::Value::Text(other.to_string()),
        },
    }
}

/// override-risk: tier-B handler (requires at least ai_with_human).
///
/// Accepts --risk-class, --risk-flags, --cluster-key, --reason (required).
/// Validates enum membership via Op::Update. Writes the changed fields and
/// inserts a transition_history row with verb='override_risk', from_status
/// and to_status both set to the current status (no lifecycle change),
/// actor_note=<reason>.
pub fn run_override_risk(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");
    let reason = matches
        .get_one::<String>("reason")
        .ok_or_else(|| anyhow::anyhow!("--reason is required for override-risk"))?;

    // AC3.6: reject ai_autonomous on every override path
    if invoker.actor == Actor::AiAutonomous {
        anyhow::bail!(
            "override-risk requires at least --invoker ai_with_human; \
             invoker is ai_autonomous (auto-detected from $CLAUDECODE). \
             To proceed: pass --invoker ai_with_human (or --invoker human)."
        );
    }

    let tx = conn
        .unchecked_transaction()
        .context("override-risk: begin tx")?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Build diff from CLI args (build_entry_map skips unregistered args safely)
    let diff_all = build_entry_map(schema, |cli_name| {
        match matches.try_get_many::<String>(cli_name) {
            Ok(Some(vals)) => {
                let collected: Vec<String> = vals.cloned().collect();
                if collected.is_empty() {
                    None
                } else {
                    Some(collected)
                }
            }
            _ => None,
        }
    })?;

    // Keep only risk metadata fields
    const RISK_FIELDS: &[&str] = &["risk_class", "risk_flags", "cluster_key"];
    let diff: crate::validate::EntryMap = diff_all
        .into_iter()
        .filter(|(k, _)| RISK_FIELDS.contains(&k.as_str()))
        .collect();

    if diff.is_empty() {
        anyhow::bail!(
            "override-risk: at least one of --risk-class, --risk-flags, --cluster-key \
             must be supplied"
        );
    }

    let mut merged = existing.clone();
    for (k, v) in &diff {
        merged.insert(k.clone(), v.clone());
    }

    // Validate enum membership + field-level actor check via Op::Update
    validate::validate(schema, &merged, Op::Update(diff.clone()), invoker)
        .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    // Build UPDATE SQL for only the changed fields
    let now = now_iso8601();
    let invoker_str = invoker.actor.to_string();
    let qtable = quote_ident(&schema.name);

    let mut set_parts: Vec<String> = vec![
        "updated_at = ?1".to_string(),
        "updated_by = ?2".to_string(),
    ];
    let mut sql_values: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(now),
        rusqlite::types::Value::Text(invoker_str.clone()),
    ];
    let mut param_idx = 3usize;

    for field in &schema.fields {
        if let Some(new_val) = diff.get(&field.name) {
            set_parts.push(format!("{} = ?{param_idx}", field.name));
            param_idx += 1;
            sql_values.push(json_to_sql(&field.ty, new_val));
        }
    }

    let where_idx = param_idx;
    sql_values.push(rusqlite::types::Value::Integer(row_id));

    let set_clause = set_parts.join(", ");
    tx.execute(
        &format!("UPDATE {qtable} SET {set_clause} WHERE id = ?{where_idx}"),
        rusqlite::params_from_iter(sql_values.iter()),
    )
    .context("override-risk: update row")?;

    // transition_history: verb='override_risk', no lifecycle change, actor_note=reason
    let (pref, phash) = read_policy_env();
    crate::db::insert_transition_history_with_note(
        &tx,
        &schema.name,
        row_id,
        display_id,
        current_status,
        current_status,
        "override_risk",
        &invoker_str,
        pref.as_deref(),
        phash.as_deref(),
        Some(reason),
    )?;

    tx.commit().context("override-risk: commit tx")?;
    println!(
        "override-risk {display_id}: risk metadata updated (status={current_status} unchanged)"
    );
    Ok(())
}

/// override-policy: direction-aware handler.
///
/// POLICY_RANK: auto=0, human=1, architecture=2.
/// - ai_autonomous always rejected (AC3.6).
/// - Relaxation (new_rank < old_rank): requires effective human —
///   Actor::Human OR (Actor::AiWithHuman + token_valid). (AC3.2)
/// - Escalation (new_rank >= old_rank): requires ai_with_human or higher. (AC3.3)
///
/// Writes approval_policy and inserts a transition_history row with
/// verb='override_policy_relax' or 'override_policy_escalate', actor_note=<reason>.
pub fn run_override_policy(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");
    let reason = matches
        .get_one::<String>("reason")
        .ok_or_else(|| anyhow::anyhow!("--reason is required for override-policy"))?;
    let new_policy = matches
        .get_one::<String>("approval-policy")
        .ok_or_else(|| anyhow::anyhow!("--approval-policy is required for override-policy"))?
        .clone();

    // AC3.6: reject ai_autonomous on every override path
    if invoker.actor == Actor::AiAutonomous {
        anyhow::bail!(
            "override-policy requires at least --invoker ai_with_human; \
             invoker is ai_autonomous (auto-detected from $CLAUDECODE). \
             To proceed: pass --invoker ai_with_human (or --invoker human)."
        );
    }

    let tx = conn
        .unchecked_transaction()
        .context("override-policy: begin tx")?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Read existing approval_policy (default: "human" per schema)
    let old_policy = existing
        .get("approval_policy")
        .and_then(|v| v.as_str())
        .unwrap_or("human")
        .to_string();

    let old_rank = policy_rank(&old_policy).ok_or_else(|| {
        anyhow::anyhow!("existing approval_policy '{old_policy}' is not a recognized value")
    })?;
    let new_rank = policy_rank(&new_policy).ok_or_else(|| {
        anyhow::anyhow!(
            "--approval-policy '{new_policy}' is not a recognized value \
             (expected: auto, human, architecture)"
        )
    })?;

    let is_relaxation = new_rank < old_rank;
    let verb = if is_relaxation {
        "override_policy_relax"
    } else {
        "override_policy_escalate"
    };

    // Direction-aware actor gate
    if is_relaxation {
        // AC3.2: relaxation requires effective human (tier-A semantics)
        let effective_human = invoker.actor == Actor::Human
            || (invoker.actor == Actor::AiWithHuman && invoker.token_valid);
        if !effective_human {
            anyhow::bail!(
                "override-policy relaxation ({old_policy} → {new_policy}) requires effective \
                 human assent; invoker is '{invoker}' without a valid approve-token. \
                 Pass --invoker human, or --invoker ai_with_human --approve-token <T>."
            );
        }
    }
    // AC3.3: escalation only requires ai_with_human; human and ai_with_human both pass.
    // ai_autonomous is already rejected above.

    // Build diff with just approval_policy
    let mut diff: crate::validate::EntryMap = std::collections::BTreeMap::new();
    diff.insert(
        "approval_policy".to_string(),
        Value::String(new_policy.clone()),
    );

    let mut merged = existing.clone();
    merged.insert("approval_policy".to_string(), Value::String(new_policy.clone()));

    // Validate enum membership + field-level actor (actor: ai_with_human on approval_policy)
    validate::validate(schema, &merged, Op::Update(diff.clone()), invoker)
        .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    // Write to DB
    let now = now_iso8601();
    let invoker_str = invoker.actor.to_string();
    let qtable = quote_ident(&schema.name);
    tx.execute(
        &format!(
            "UPDATE {qtable} SET approval_policy = ?1, updated_at = ?2, updated_by = ?3 \
             WHERE id = ?4"
        ),
        rusqlite::params![new_policy.as_str(), &now, &invoker_str, row_id],
    )
    .context("override-policy: update row")?;

    // transition_history: verb=override_policy_relax|escalate, no lifecycle change
    let (pref, phash) = read_policy_env();
    crate::db::insert_transition_history_with_note(
        &tx,
        &schema.name,
        row_id,
        display_id,
        current_status,
        current_status,
        verb,
        &invoker_str,
        pref.as_deref(),
        phash.as_deref(),
        Some(reason),
    )?;

    tx.commit().context("override-policy: commit tx")?;

    let direction = if is_relaxation { "relaxed" } else { "escalated" };
    println!(
        "override-policy {display_id}: approval_policy {direction} \
         {old_policy} → {new_policy} (status={current_status} unchanged)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::schema::{actor::Actor, Schema};

    fn obs_schema() -> Schema {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "observations")
            .map(|(_, y)| *y)
            .expect("bundled observations schema");
        Schema::from_yaml(yaml).expect("parse observations schema")
    }

    fn setup() -> (Schema, Connection) {
        let schema = obs_schema();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        (schema, conn)
    }

    fn insert_obs(conn: &Connection, display_id: &str) {
        let now = "2026-05-06T00:00:00Z";
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, captured_week, \
              created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'open', 'test', 'dev', 'normal', ?2, 'w18', ?2, ?2, 'human', 'human')",
            rusqlite::params![display_id, now],
        )
        .unwrap();
    }

    fn audit_rows(conn: &Connection, display_id: &str) -> Vec<(String, Option<String>, String)> {
        let mut s = conn
            .prepare(
                "SELECT verb, actor_note, invoker FROM transition_history \
                 WHERE display_id = ?1 ORDER BY id ASC",
            )
            .unwrap();
        s.query_map(rusqlite::params![display_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    fn build_override_risk_matches(args: &[&str]) -> clap::ArgMatches {
        use clap::{Arg, ArgAction, Command};
        let cmd = Command::new("override-risk")
            .arg(Arg::new("display_id").required(true).index(1))
            .arg(Arg::new("reason").long("reason").required(true))
            .arg(Arg::new("risk-class").long("risk-class").required(false))
            .arg(
                Arg::new("risk-flags")
                    .long("risk-flags")
                    .action(ArgAction::Append)
                    .required(false),
            )
            .arg(Arg::new("cluster-key").long("cluster-key").required(false));
        cmd.get_matches_from(args)
    }

    fn build_override_policy_matches(args: &[&str]) -> clap::ArgMatches {
        use clap::{Arg, Command};
        let cmd = Command::new("override-policy")
            .arg(Arg::new("display_id").required(true).index(1))
            .arg(Arg::new("reason").long("reason").required(true))
            .arg(
                Arg::new("approval-policy")
                    .long("approval-policy")
                    .required(true),
            );
        cmd.get_matches_from(args)
    }

    // ---- override-risk tests ----

    #[test]
    fn override_risk_ai_autonomous_rejected() {
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");
        let m = build_override_risk_matches(
            &["override-risk", "L001", "--reason", "test", "--risk-class", "low"],
        );
        let err = run_override_risk(&schema, &conn, &m, Actor::AiAutonomous.into())
            .expect_err("ai_autonomous must be rejected");
        assert!(
            err.to_string().contains("ai_autonomous"),
            "error must cite ai_autonomous; got: {err}"
        );
    }

    #[test]
    fn override_risk_ai_with_human_accepted_and_history_written() {
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");
        let m = build_override_risk_matches(
            &["override-risk", "L001", "--reason", "downgrade risk", "--risk-class", "low"],
        );
        run_override_risk(&schema, &conn, &m, Actor::AiWithHuman.into())
            .expect("ai_with_human must be accepted");

        // Verify the field was updated
        let rc: String = conn
            .query_row(
                "SELECT risk_class FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rc, "low");

        // Verify transition_history row
        let rows = audit_rows(&conn, "L001");
        assert_eq!(rows.len(), 1, "one audit row expected; got: {:?}", rows);
        assert_eq!(rows[0].0, "override_risk");
        assert_eq!(rows[0].1.as_deref(), Some("downgrade risk"));
    }

    #[test]
    fn override_risk_bogus_risk_class_rejected() {
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");
        let m = build_override_risk_matches(
            &["override-risk", "L001", "--reason", "test", "--risk-class", "bogus"],
        );
        let err = run_override_risk(&schema, &conn, &m, Actor::AiWithHuman.into())
            .expect_err("bogus risk_class must be rejected");
        assert!(
            err.to_string().contains("validation failed") || err.to_string().contains("bogus"),
            "expected validation error; got: {err}"
        );
    }

    #[test]
    fn override_risk_missing_reason_rejected_at_cli() {
        // --reason required at parse time; clap exits non-zero
        use clap::{Arg, Command};
        let cmd = Command::new("override-risk")
            .arg(Arg::new("display_id").required(true).index(1))
            .arg(Arg::new("reason").long("reason").required(true))
            .arg(Arg::new("risk-class").long("risk-class").required(false));
        let err = cmd
            .try_get_matches_from(&["override-risk", "L001", "--risk-class", "low"])
            .expect_err("missing --reason must fail parse");
        let msg = err.to_string();
        assert!(
            msg.contains("--reason") || msg.contains("required"),
            "error must cite --reason; got: {msg}"
        );
    }

    // ---- override-policy tests ----

    #[test]
    fn override_policy_ai_autonomous_rejected() {
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");
        let m = build_override_policy_matches(&[
            "override-policy",
            "L001",
            "--reason",
            "test",
            "--approval-policy",
            "auto",
        ]);
        let err = run_override_policy(&schema, &conn, &m, Actor::AiAutonomous.into())
            .expect_err("ai_autonomous must be rejected");
        assert!(
            err.to_string().contains("ai_autonomous"),
            "error must cite ai_autonomous; got: {err}"
        );
    }

    #[test]
    fn override_policy_relaxation_ai_with_human_no_token_rejected() {
        // human→auto is relaxation; ai_with_human without token must be rejected
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");
        // Default policy is 'human'; request 'auto' = relaxation
        let m = build_override_policy_matches(&[
            "override-policy",
            "L001",
            "--reason",
            "relax policy",
            "--approval-policy",
            "auto",
        ]);
        let err = run_override_policy(&schema, &conn, &m, Actor::AiWithHuman.into())
            .expect_err("relaxation without token must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("relaxation") || msg.contains("effective human") || msg.contains("approve-token"),
            "error must describe relaxation requirement; got: {msg}"
        );
    }

    #[test]
    fn override_policy_relaxation_ai_with_human_with_token_accepted() {
        // human→auto relaxation with token: must be accepted
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");
        let m = build_override_policy_matches(&[
            "override-policy",
            "L001",
            "--reason",
            "need auto policy",
            "--approval-policy",
            "auto",
        ]);
        let invoker = InvokerCtx {
            actor: Actor::AiWithHuman,
            token_valid: true,
        };
        run_override_policy(&schema, &conn, &m, invoker)
            .expect("ai_with_human + token must be accepted for relaxation");

        let policy: String = conn
            .query_row(
                "SELECT approval_policy FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(policy, "auto");

        let rows = audit_rows(&conn, "L001");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "override_policy_relax");
        assert_eq!(rows[0].1.as_deref(), Some("need auto policy"));
    }

    #[test]
    fn override_policy_escalation_ai_with_human_no_token_accepted() {
        // auto→human is escalation; ai_with_human without token must be accepted
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");
        // Seed row with approval_policy='auto' (below default)
        conn.execute(
            "UPDATE observations SET approval_policy='auto' WHERE display_id='L001'",
            [],
        )
        .unwrap();
        let m = build_override_policy_matches(&[
            "override-policy",
            "L001",
            "--reason",
            "raise bar",
            "--approval-policy",
            "human",
        ]);
        run_override_policy(&schema, &conn, &m, Actor::AiWithHuman.into())
            .expect("escalation without token must be accepted");

        let policy: String = conn
            .query_row(
                "SELECT approval_policy FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(policy, "human");

        let rows = audit_rows(&conn, "L001");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "override_policy_escalate");
        assert_eq!(rows[0].1.as_deref(), Some("raise bar"));
    }

    #[test]
    fn override_policy_actor_note_equals_reason() {
        // AC3.4: every successful override write records reason as actor_note
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");
        conn.execute(
            "UPDATE observations SET approval_policy='auto' WHERE display_id='L001'",
            [],
        )
        .unwrap();
        let m = build_override_policy_matches(&[
            "override-policy",
            "L001",
            "--reason",
            "my specific reason",
            "--approval-policy",
            "architecture",
        ]);
        run_override_policy(&schema, &conn, &m, Actor::Human.into()).unwrap();

        let rows = audit_rows(&conn, "L001");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].1.as_deref(),
            Some("my specific reason"),
            "actor_note must equal --reason argument"
        );
    }

    #[test]
    fn override_policy_missing_reason_rejected_at_cli() {
        use clap::{Arg, Command};
        let cmd = Command::new("override-policy")
            .arg(Arg::new("display_id").required(true).index(1))
            .arg(Arg::new("reason").long("reason").required(true))
            .arg(Arg::new("approval-policy").long("approval-policy").required(true));
        let err = cmd
            .try_get_matches_from(&["override-policy", "L001", "--approval-policy", "auto"])
            .expect_err("missing --reason must fail parse");
        let msg = err.to_string();
        assert!(
            msg.contains("--reason") || msg.contains("required"),
            "error must cite --reason; got: {msg}"
        );
    }

    #[test]
    fn override_policy_all_relaxation_paths_require_token_or_human() {
        // AC3.2: architecture→human, human→auto, architecture→auto, human→auto
        let (schema, conn) = setup();
        insert_obs(&conn, "L001");

        let relaxation_cases: &[(&str, &str)] = &[
            ("architecture", "human"),
            ("human", "auto"),
            ("architecture", "auto"),
        ];

        for (i, (from, to)) in relaxation_cases.iter().enumerate() {
            let display_id = format!("L{:03}", i + 1);
            if i > 0 {
                insert_obs(&conn, &display_id);
            }
            conn.execute(
                "UPDATE observations SET approval_policy=?1 WHERE display_id=?2",
                rusqlite::params![from, &display_id],
            )
            .unwrap();

            let m = build_override_policy_matches(&[
                "override-policy",
                &display_id,
                "--reason",
                "test relaxation",
                "--approval-policy",
                to,
            ]);

            // Bare ai_with_human (no token) must fail
            let err =
                run_override_policy(&schema, &conn, &m, Actor::AiWithHuman.into()).expect_err(
                    &format!("{from}→{to} relaxation without token must be rejected"),
                );
            assert!(
                err.to_string().contains("relaxation")
                    || err.to_string().contains("approve-token")
                    || err.to_string().contains("effective human"),
                "expected relaxation gate error for {from}→{to}; got: {err}"
            );

            // ai_with_human + valid token must succeed
            let m2 = build_override_policy_matches(&[
                "override-policy",
                &display_id,
                "--reason",
                "approved relax",
                "--approval-policy",
                to,
            ]);
            let invoker = InvokerCtx {
                actor: Actor::AiWithHuman,
                token_valid: true,
            };
            run_override_policy(&schema, &conn, &m2, invoker)
                .unwrap_or_else(|e| panic!("{from}→{to} with token must succeed; got: {e}"));
        }
    }

    #[test]
    fn override_policy_all_escalation_paths_accept_ai_with_human_without_token() {
        // AC3.3: auto→human, human→architecture, auto→architecture
        let (schema, conn) = setup();

        let escalation_cases: &[(&str, &str)] = &[
            ("auto", "human"),
            ("human", "architecture"),
            ("auto", "architecture"),
        ];

        for (i, (from, to)) in escalation_cases.iter().enumerate() {
            let display_id = format!("L{:03}", i + 1);
            insert_obs(&conn, &display_id);
            conn.execute(
                "UPDATE observations SET approval_policy=?1 WHERE display_id=?2",
                rusqlite::params![from, &display_id],
            )
            .unwrap();

            let m = build_override_policy_matches(&[
                "override-policy",
                &display_id,
                "--reason",
                "escalate policy",
                "--approval-policy",
                to,
            ]);
            run_override_policy(&schema, &conn, &m, Actor::AiWithHuman.into())
                .unwrap_or_else(|e| {
                    panic!("{from}→{to} escalation with ai_with_human must succeed; got: {e}")
                });

            let policy: String = conn
                .query_row(
                    "SELECT approval_policy FROM observations WHERE display_id=?1",
                    rusqlite::params![&display_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(policy, *to);
        }
    }
}
