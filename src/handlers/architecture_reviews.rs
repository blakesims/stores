use anyhow::{bail, Context, Result};
use clap::ArgMatches;
use rusqlite::{Connection, Transaction};
use serde_json::Value;

use crate::schema::{
    actor::{Actor, InvokerCtx},
    lifecycle::select_transition,
    Schema,
};
use crate::validate::{self, EntryMap, Op};

use super::row::{build_entry_map, now_iso8601, read_row};
use super::transition::{execute_transition_write, read_policy_env};

pub fn run_issue_verdict(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("architecture_reviews issue-verdict: begin tx")?;
    issue_verdict_in_tx(&tx, schema, matches, invoker)?;
    tx.commit()
        .context("architecture_reviews issue-verdict: commit tx")?;
    Ok(())
}

pub fn run_ratify_amend(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("architecture_reviews ratify-amend: begin tx")?;
    ratify_amend_in_tx(&tx, schema, matches, invoker)?;
    tx.commit()
        .context("architecture_reviews ratify-amend: commit tx")?;
    Ok(())
}

pub fn run_supersede(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("architecture_reviews supersede: begin tx")?;
    supersede_in_tx(&tx, schema, matches, invoker)?;
    tx.commit()
        .context("architecture_reviews supersede: commit tx")?;
    Ok(())
}

pub fn run_withdraw(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("architecture_reviews withdraw: begin tx")?;
    withdraw_in_tx(&tx, schema, matches, invoker)?;
    tx.commit()
        .context("architecture_reviews withdraw: commit tx")?;
    Ok(())
}

fn issue_verdict_in_tx(
    tx: &Transaction,
    schema: &Schema,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    require_actor(invoker, Actor::AiWithHuman, "issue-verdict")?;
    let display_id = display_id(matches);
    let (row_id, existing) = read_row(schema, tx, display_id)?;
    let current_status = status_of(&existing);
    if current_status != "in_review" {
        bail!("issue-verdict requires status=in_review; {display_id} is status={current_status}");
    }

    let mut diff = cli_diff(schema, matches)?;
    let produced_task_id = matches
        .try_get_one::<String>("produced-task-id")
        .ok()
        .flatten()
        .cloned();
    let kind = required_str(&diff, "kind", "issue-verdict requires --kind")?.to_string();
    let verdict = required_str(&diff, "verdict", "issue-verdict requires --verdict")?.to_string();
    if !diff.contains_key("rationale") {
        bail!("issue-verdict requires --rationale");
    }

    match kind.as_str() {
        "interpret" => {
            if verdict == "propose_doctrine_update" {
                bail!("interpret architecture_reviews cannot use verdict=propose_doctrine_update");
            }
            diff.insert(
                "verdict_issued_at".to_string(),
                Value::String(now_iso8601()),
            );
        }
        "amend" => {
            if verdict != "propose_doctrine_update" {
                bail!("amend architecture_reviews require verdict=propose_doctrine_update");
            }
            validate_cascade_decisions(diff.get("cascade_decisions"))?;
        }
        other => bail!("unknown architecture review kind '{other}'; expected interpret or amend"),
    }

    let mut merged = existing.clone();
    merge_diff(&mut merged, &diff);
    if verdict == "merge_with_cluster" {
        let has_source = merged
            .get("source_observation")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .is_some();
        let has_linked = merged
            .get("linked_observation_ids")
            .and_then(|v| v.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);
        if !has_source && !has_linked {
            required_str(
                &merged,
                "source_observation",
                "merge_with_cluster requires --source-observation or linked_observation_ids",
            )?;
        }
        required_str(
            &merged,
            "merge_target_id",
            "merge_with_cluster requires --merge-target-id",
        )?;
    }
    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        "issue-verdict",
        None,
        &merged,
    )?;
    super::transition::inject_upstream_primary_tuple(
        schema,
        transition,
        "issue-verdict",
        current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;
    validate::validate(
        schema,
        &merged,
        Op::Transition(
            "issue-verdict".to_string(),
            super::transition::strip_framework_overlay_from_validation_diff(schema, &diff),
        ),
        invoker,
    )
    .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    if let Some(task_id) = produced_task_id {
        if verdict == "create_primitive_task" {
            diff.insert(
                "produced_task_id".to_string(),
                Value::String(task_id.clone()),
            );
            merged.insert("produced_task_id".to_string(), Value::String(task_id));
        }
    }

    let (pref, phash) = read_policy_env();
    execute_transition_write(
        tx,
        schema,
        row_id,
        display_id,
        current_status,
        &transition.to,
        "issue-verdict",
        &diff,
        &merged,
        invoker.actor,
        pref.as_deref(),
        phash.as_deref(),
        None,
    )?;

    if let Some(supersedes) = merged
        .get("supersedes")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        mark_superseded_if_present(tx, schema, supersedes, invoker.actor, Some(display_id))?;
    }

    let outcome = merged
        .get("outcome")
        .and_then(|v| v.as_str())
        .or_else(|| merged.get("verdict").and_then(|v| v.as_str()))
        .unwrap_or("");
    let ruling_outcome = super::observation_arch_gate::RulingOutcome::parse(outcome)?;
    super::observation_arch_gate::apply_verdict_effects(
        tx,
        display_id,
        &merged,
        invoker.actor,
        ruling_outcome,
    )?;

    println!(
        "Transitioned {display_id}: {} → {}",
        transition.from, transition.to
    );
    Ok(())
}

fn ratify_amend_in_tx(
    tx: &Transaction,
    schema: &Schema,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    if invoker.actor != Actor::Human {
        bail!("ratify-amend requires invoker actor human; ai_autonomous and ai_with_human are rejected even with a valid approval token");
    }
    if !invoker.token_valid {
        bail!("ratify-amend requires a valid tier-A --approve-token");
    }

    let display_id = display_id(matches);
    let (row_id, existing) = read_row(schema, tx, display_id)?;
    let current_status = status_of(&existing);
    if current_status != "awaiting_human_ratification" {
        bail!("ratify-amend requires status=awaiting_human_ratification; {display_id} is status={current_status}");
    }
    if existing.get("kind").and_then(|v| v.as_str()) != Some("amend") {
        bail!("ratify-amend requires kind=amend");
    }

    let now = now_iso8601();
    let mut diff = EntryMap::new();
    diff.insert("ratified_at".to_string(), Value::String(now.clone()));
    diff.insert(
        "ratified_by".to_string(),
        Value::String("human".to_string()),
    );
    diff.insert("verdict_issued_at".to_string(), Value::String(now));

    let mut merged = existing.clone();
    merge_diff(&mut merged, &diff);
    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        "ratify-amend",
        None,
        &merged,
    )?;
    super::transition::inject_upstream_primary_tuple(
        schema,
        transition,
        "ratify-amend",
        current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;
    validate::validate(
        schema,
        &merged,
        Op::Transition(
            "ratify-amend".to_string(),
            super::transition::strip_framework_overlay_from_validation_diff(schema, &diff),
        ),
        invoker,
    )
    .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    let (pref, phash) = read_policy_env();
    execute_transition_write(
        tx,
        schema,
        row_id,
        display_id,
        current_status,
        &transition.to,
        "ratify-amend",
        &diff,
        &merged,
        invoker.actor,
        pref.as_deref(),
        phash.as_deref(),
        None,
    )?;
    let outcome = merged
        .get("outcome")
        .and_then(|v| v.as_str())
        .or_else(|| merged.get("verdict").and_then(|v| v.as_str()))
        .unwrap_or("");
    let ruling_outcome = super::observation_arch_gate::RulingOutcome::parse(outcome)?;
    super::observation_arch_gate::apply_verdict_effects(
        tx,
        display_id,
        &merged,
        invoker.actor,
        ruling_outcome,
    )?;
    println!(
        "Transitioned {display_id}: {} → {}",
        transition.from, transition.to
    );
    Ok(())
}

fn supersede_in_tx(
    tx: &Transaction,
    schema: &Schema,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    require_actor(invoker, Actor::AiWithHuman, "supersede")?;
    let display_id = display_id(matches);
    mark_superseded_if_present(tx, schema, display_id, invoker.actor, None)?;
    if let Ok((_, row)) = read_row(schema, tx, display_id) {
        super::observation_arch_gate::apply_verdict_effects(
            tx,
            display_id,
            &row,
            invoker.actor,
            super::observation_arch_gate::RulingOutcome::Superseded,
        )?;
    }
    println!("Superseded {display_id}");
    Ok(())
}

fn withdraw_in_tx(
    tx: &Transaction,
    schema: &Schema,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    require_actor(invoker, Actor::AiWithHuman, "withdraw")?;
    let display_id = display_id(matches);
    super::transition::run_in_tx(tx, schema, matches, invoker, "withdraw")?;
    let (_, row) = read_row(schema, tx, display_id)?;
    super::observation_arch_gate::apply_verdict_effects(
        tx,
        display_id,
        &row,
        invoker.actor,
        super::observation_arch_gate::RulingOutcome::Withdrawn,
    )
}

fn mark_superseded_if_present(
    tx: &Transaction,
    schema: &Schema,
    display_id: &str,
    actor: Actor,
    superseded_by: Option<&str>,
) -> Result<()> {
    let (row_id, existing) = match read_row(schema, tx, display_id) {
        Ok(row) => row,
        Err(_) => return Ok(()),
    };
    let current_status = status_of(&existing);
    if current_status == "superseded" {
        return Ok(());
    }
    let mut diff = EntryMap::new();
    let mut merged = existing.clone();
    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        "supersede",
        None,
        &merged,
    )?;
    super::transition::inject_upstream_primary_tuple(
        schema,
        transition,
        "supersede",
        current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;
    let invoker = InvokerCtx {
        actor,
        token_valid: false,
    };
    validate::validate(
        schema,
        &merged,
        Op::Transition(
            "supersede".to_string(),
            super::transition::strip_framework_overlay_from_validation_diff(schema, &diff),
        ),
        invoker,
    )
    .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;
    let (pref, phash) = read_policy_env();
    execute_transition_write(
        tx,
        schema,
        row_id,
        display_id,
        current_status,
        &transition.to,
        "supersede",
        &diff,
        &merged,
        actor,
        pref.as_deref(),
        phash.as_deref(),
        None,
    )?;
    if let Some(successor) = superseded_by {
        tx.execute(
            "UPDATE architecture_reviews SET superseded_by_id=?1 WHERE display_id=?2",
            rusqlite::params![successor, display_id],
        )?;
    }
    Ok(())
}

fn validate_cascade_decisions(value: Option<&Value>) -> Result<()> {
    let arr = match value {
        Some(Value::Array(arr)) => arr,
        Some(Value::String(_)) => bail!("cascade_decisions must be well-formed JSON array"),
        _ => bail!("cascade_decisions is required for amend architecture_reviews"),
    };
    if arr.is_empty() {
        bail!("cascade_decisions must contain at least one decision");
    }
    let allowed = ["keep", "update", "supersede", "withdraw", "create_followup"];
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("cascade_decisions entries must be objects"))?;
        let target = obj.get("target").and_then(|v| v.as_str()).unwrap_or("");
        if target.is_empty() {
            bail!("cascade_decisions entries require target");
        }
        let decision = obj.get("decision").and_then(|v| v.as_str()).unwrap_or("");
        if !allowed.contains(&decision) {
            bail!("cascade_decisions entries require known decision action");
        }
    }
    Ok(())
}

fn cli_diff(schema: &Schema, matches: &ArgMatches) -> Result<EntryMap> {
    build_entry_map(schema, |cli_name| {
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
    })
}

fn merge_diff(merged: &mut EntryMap, diff: &EntryMap) {
    for (k, v) in diff {
        merged.insert(k.clone(), v.clone());
    }
}

fn display_id(matches: &ArgMatches) -> &str {
    matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("")
}

fn status_of(entry: &EntryMap) -> &str {
    // ADR 0002 compatibility-only T148 task 6.1: schema transition selection
    // still consumes the legacy architecture_reviews.status state-machine label.
    entry.get("status").and_then(|v| v.as_str()).unwrap_or("")
}

fn required_str<'a>(entry: &'a EntryMap, field: &str, msg: &str) -> Result<&'a str> {
    entry
        .get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!(msg.to_string()))
}

fn require_actor(invoker: InvokerCtx, expected: Actor, verb: &str) -> Result<()> {
    if invoker.actor != expected {
        bail!(
            "{verb} requires invoker actor {expected}; got {}",
            invoker.actor
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, ArgAction, Command};
    use rusqlite::Connection;

    fn schema() -> Schema {
        Schema::from_yaml(include_str!(
            "../../stores/architecture_reviews/schema.yaml"
        ))
        .unwrap()
    }

    fn setup() -> (Schema, Connection) {
        let schema = schema();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();
        (schema, conn)
    }

    fn insert_row(conn: &Connection, id: &str, status: &str, kind: &str) {
        conn.execute(
            "INSERT INTO architecture_reviews (display_id,status,created_at,updated_at,created_by,updated_by,kind,summary) VALUES (?1,?2,'now','now','ai_with_human','ai_with_human',?3,'summary')",
            rusqlite::params![id, status, kind],
        ).unwrap();
    }

    fn issue_cmd() -> Command {
        Command::new("issue-verdict")
            .arg(Arg::new("display_id").required(true))
            .arg(Arg::new("kind").long("kind"))
            .arg(Arg::new("verdict").long("verdict"))
            .arg(Arg::new("rationale").long("rationale"))
            .arg(
                Arg::new("cascade-decisions")
                    .long("cascade-decisions")
                    .action(ArgAction::Append),
            )
            .arg(Arg::new("supersedes").long("supersedes"))
            .arg(Arg::new("source-observation").long("source-observation"))
            .arg(Arg::new("merge-target-id").long("merge-target-id"))
    }

    fn ratify_cmd() -> Command {
        Command::new("ratify-amend").arg(Arg::new("display_id").required(true))
    }

    #[test]
    fn interpret_issue_goes_to_verdict_issued() {
        let (schema, conn) = setup();
        insert_row(&conn, "A001", "in_review", "interpret");
        let m = issue_cmd().get_matches_from([
            "issue-verdict",
            "A001",
            "--kind",
            "interpret",
            "--verdict",
            "allow_local_fix",
            "--rationale",
            "x",
        ]);
        run_issue_verdict(&schema, &conn, &m, Actor::AiWithHuman.into()).unwrap();
        let (_, row) = read_row(&schema, &conn, "A001").unwrap();
        assert_eq!(
            row.get("status").and_then(|v| v.as_str()),
            Some("verdict_issued")
        );
        assert!(row
            .get("verdict_issued_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .ends_with('Z'));
    }

    #[test]
    fn amend_issue_requires_valid_cascade_decisions() {
        let (schema, conn) = setup();
        insert_row(&conn, "A001", "in_review", "amend");
        let missing = issue_cmd().get_matches_from([
            "issue-verdict",
            "A001",
            "--kind",
            "amend",
            "--verdict",
            "propose_doctrine_update",
            "--rationale",
            "x",
        ]);
        let err =
            run_issue_verdict(&schema, &conn, &missing, Actor::AiWithHuman.into()).unwrap_err();
        assert!(err.to_string().contains("cascade_decisions"));

        let bad = issue_cmd().get_matches_from([
            "issue-verdict",
            "A001",
            "--kind",
            "amend",
            "--verdict",
            "propose_doctrine_update",
            "--rationale",
            "x",
            "--cascade-decisions",
            "[{\"target\":\"x\",\"decision\":\"bogus\"}]",
        ]);
        let err = run_issue_verdict(&schema, &conn, &bad, Actor::AiWithHuman.into()).unwrap_err();
        assert!(err.to_string().contains("cascade_decisions"));
    }

    #[test]
    fn amend_issue_awaits_human_ratification() {
        let (schema, conn) = setup();
        insert_row(&conn, "A001", "in_review", "amend");
        let m = issue_cmd().get_matches_from([
            "issue-verdict",
            "A001",
            "--kind",
            "amend",
            "--verdict",
            "propose_doctrine_update",
            "--rationale",
            "x",
            "--cascade-decisions",
            "[{\"target\":\"doc\",\"decision\":\"update\"}]",
        ]);
        run_issue_verdict(&schema, &conn, &m, Actor::AiWithHuman.into()).unwrap();
        let (_, row) = read_row(&schema, &conn, "A001").unwrap();
        assert_eq!(
            row.get("status").and_then(|v| v.as_str()),
            Some("awaiting_human_ratification")
        );
        assert_eq!(
            row.get("ratified_at")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            ""
        );
    }

    #[test]
    fn ratify_rejects_non_human_even_with_token() {
        let (schema, conn) = setup();
        insert_row(&conn, "A001", "awaiting_human_ratification", "amend");
        let m = ratify_cmd().get_matches_from(["ratify-amend", "A001"]);
        let err = run_ratify_amend(
            &schema,
            &conn,
            &m,
            InvokerCtx {
                actor: Actor::AiWithHuman,
                token_valid: true,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires invoker actor human"));
        let err = run_ratify_amend(
            &schema,
            &conn,
            &m,
            InvokerCtx {
                actor: Actor::AiAutonomous,
                token_valid: true,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("ai_autonomous"));
    }

    #[test]
    fn ratify_human_with_token_finalizes() {
        let (schema, conn) = setup();
        insert_row(&conn, "A001", "awaiting_human_ratification", "amend");
        conn.execute("UPDATE architecture_reviews SET cascade_decisions='[{\"target\":\"doc\",\"decision\":\"update\"}]', verdict='propose_doctrine_update' WHERE display_id='A001'", []).unwrap();
        let m = ratify_cmd().get_matches_from(["ratify-amend", "A001"]);
        run_ratify_amend(
            &schema,
            &conn,
            &m,
            InvokerCtx {
                actor: Actor::Human,
                token_valid: true,
            },
        )
        .unwrap();
        let (_, row) = read_row(&schema, &conn, "A001").unwrap();
        assert_eq!(
            row.get("status").and_then(|v| v.as_str()),
            Some("verdict_issued")
        );
        assert!(row
            .get("ratified_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .ends_with('Z'));
    }

    #[test]
    fn merge_with_cluster_issue_resolves_source_observation() {
        let (schema, conn) = setup();
        let obs_schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&obs_schema))
            .unwrap();
        conn.execute("INSERT INTO observations (display_id,status,created_at,updated_at,created_by,updated_by,summary,source,priority,captured_at,captured_week,pending_architecture_review) VALUES ('L001','confirmed','now','now','human','human','s','dev','normal','2026-05-07','w19-d4',1)", []).unwrap();
        insert_row(&conn, "A001", "in_review", "interpret");
        conn.execute(
            "UPDATE architecture_reviews SET source_observation='L001' WHERE display_id='A001'",
            [],
        )
        .unwrap();
        let m = issue_cmd().get_matches_from([
            "issue-verdict",
            "A001",
            "--kind",
            "interpret",
            "--verdict",
            "merge_with_cluster",
            "--rationale",
            "x",
            "--merge-target-id",
            "L999",
        ]);
        run_issue_verdict(&schema, &conn, &m, Actor::AiWithHuman.into()).unwrap();
        let row: (String, i64, String, String, String) = conn.query_row(
            "SELECT status,pending_architecture_review,resolved_by,merge_target_id,resolution_kind FROM observations WHERE display_id='L001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).unwrap();
        assert_eq!(
            row,
            (
                "resolved".into(),
                0,
                "A001".into(),
                "L999".into(),
                "merged_with_cluster".into()
            )
        );
    }

    #[test]
    fn supersedes_marks_prior_terminal_and_keeps_new_status() {
        let (schema, conn) = setup();
        insert_row(&conn, "A001", "verdict_issued", "interpret");
        insert_row(&conn, "A002", "in_review", "interpret");
        let m = issue_cmd().get_matches_from([
            "issue-verdict",
            "A002",
            "--kind",
            "interpret",
            "--verdict",
            "allow_local_fix",
            "--rationale",
            "x",
            "--supersedes",
            "A001",
        ]);
        run_issue_verdict(&schema, &conn, &m, Actor::AiWithHuman.into()).unwrap();
        let (_, old) = read_row(&schema, &conn, "A001").unwrap();
        let (_, new) = read_row(&schema, &conn, "A002").unwrap();
        assert_eq!(
            old.get("status").and_then(|v| v.as_str()),
            Some("superseded")
        );
        assert_eq!(
            old.get("superseded_by_id").and_then(|v| v.as_str()),
            Some("A002")
        );
        assert_eq!(
            new.get("status").and_then(|v| v.as_str()),
            Some("verdict_issued")
        );
    }
}
