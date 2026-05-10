use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use uuid::Uuid;

use crate::schema::actor::Actor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencingToken(pub String);

#[derive(Debug, Clone)]
pub struct AcquireParams<'a> {
    pub resource_id: &'a str,
    pub owner_display_id: &'a str,
    pub owner_kind: &'a str,
    pub ttl_secs: Option<u64>,
    pub claim_source: Option<&'a str>,
    pub invoker: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredLock {
    pub resource_id: String,
    pub owner_display_id: String,
    pub fencing_token: String,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("ResourceLockBusy: resource_id={resource_id} current_owner={current_owner}")]
pub struct ResourceLockBusy {
    pub resource_id: String,
    pub current_owner: String,
}

fn now() -> DateTime<Utc> {
    std::time::SystemTime::now().into()
}

fn fmt(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn parse(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn audit(
    tx: &rusqlite::Transaction<'_>,
    resource_id: &str,
    from: &str,
    to: &str,
    verb: &str,
    invoker: Actor,
) -> Result<()> {
    crate::db::insert_transition_history_with_note(
        tx,
        "resource_locks",
        0,
        resource_id,
        from,
        to,
        verb,
        &invoker.to_string(),
        None,
        None,
        None,
    )
}

pub fn acquire(conn: &rusqlite::Connection, p: &AcquireParams<'_>) -> Result<FencingToken> {
    if !matches!(p.owner_kind, "task" | "job") {
        bail!("owner_kind must be task or job");
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let existing: Option<(String, String, Option<String>)> = tx.query_row(
        "SELECT owner_display_id, fencing_token, expires_at FROM resource_locks WHERE resource_id=?1",
        params![p.resource_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;
    if let Some((owner, tok, exp)) = existing {
        let expired = exp
            .as_deref()
            .and_then(parse)
            .map(|d| d < now())
            .unwrap_or(false);
        if !expired {
            return Err(ResourceLockBusy {
                resource_id: p.resource_id.to_string(),
                current_owner: owner,
            }
            .into());
        }
        let deleted = tx.execute(
            "DELETE FROM resource_locks WHERE resource_id=?1 AND fencing_token=?2",
            params![p.resource_id, tok],
        )?;
        if deleted != 1 {
            let current_owner: Option<String> = tx
                .query_row(
                    "SELECT owner_display_id FROM resource_locks WHERE resource_id=?1",
                    params![p.resource_id],
                    |r| r.get(0),
                )
                .optional()?;
            return Err(ResourceLockBusy {
                resource_id: p.resource_id.to_string(),
                current_owner: current_owner.unwrap_or(owner),
            }
            .into());
        }
        audit(
            &tx,
            p.resource_id,
            "locked",
            "unlocked",
            "recover_stale",
            p.invoker,
        )?;
    }
    let token = Uuid::new_v4().to_string();
    let acquired_at = fmt(now());
    let expires_at = p.ttl_secs.map(|s| fmt(now() + Duration::seconds(s as i64)));
    if let Err(e) = tx.execute(
        "INSERT INTO resource_locks (resource_id, owner_kind, owner_display_id, fencing_token, acquired_at, heartbeat_at, expires_at, daemon_epoch, claim_source) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, NULL, ?7)",
        params![p.resource_id, p.owner_kind, p.owner_display_id, token, acquired_at, expires_at, p.claim_source],
    ) {
        let current_owner: Option<String> = tx
            .query_row(
                "SELECT owner_display_id FROM resource_locks WHERE resource_id=?1",
                params![p.resource_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(current_owner) = current_owner {
            return Err(ResourceLockBusy {
                resource_id: p.resource_id.to_string(),
                current_owner,
            }
            .into());
        }
        return Err(e.into());
    }
    audit(
        &tx,
        p.resource_id,
        "unlocked",
        "locked",
        "acquire",
        p.invoker,
    )?;
    tx.commit()?;
    Ok(FencingToken(token))
}

pub fn release(
    conn: &rusqlite::Connection,
    resource_id: &str,
    fencing_token: &str,
    invoker: Actor,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute(
        "DELETE FROM resource_locks WHERE resource_id=?1 AND fencing_token=?2",
        params![resource_id, fencing_token],
    )?;
    if n != 1 {
        bail!("ResourceLockReleaseMismatch: resource_id={resource_id}");
    }
    audit(&tx, resource_id, "locked", "unlocked", "release", invoker)?;
    tx.commit()?;
    Ok(())
}

pub fn check_ownership(
    conn: &rusqlite::Connection,
    resource_id: &str,
    owner_display_id: &str,
) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM resource_locks WHERE resource_id=?1 AND owner_display_id=?2",
        params![resource_id, owner_display_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn check_ownership_token(
    conn: &rusqlite::Connection,
    resource_id: &str,
    owner_display_id: &str,
    fencing_token: &str,
) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM resource_locks WHERE resource_id=?1 AND owner_display_id=?2 AND fencing_token=?3",
        params![resource_id, owner_display_id, fencing_token],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn recover_stale(
    conn: &rusqlite::Connection,
    now: DateTime<Utc>,
    invoker: Actor,
) -> Result<Vec<RecoveredLock>> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let cutoff = fmt(now);
    let rows: Vec<RecoveredLock> = {
        let mut stmt = tx.prepare("SELECT resource_id, owner_display_id, fencing_token FROM resource_locks WHERE expires_at IS NOT NULL AND expires_at < ?1 ORDER BY resource_id")?;
        let iter = stmt.query_map(params![cutoff], |r| {
            Ok(RecoveredLock {
                resource_id: r.get(0)?,
                owner_display_id: r.get(1)?,
                fencing_token: r.get(2)?,
            })
        })?;
        iter.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut recovered = Vec::new();
    for row in &rows {
        let deleted = tx.execute(
            "DELETE FROM resource_locks WHERE resource_id=?1 AND fencing_token=?2",
            params![row.resource_id, row.fencing_token],
        )?;
        if deleted == 1 {
            audit(
                &tx,
                &row.resource_id,
                "locked",
                "unlocked",
                "recover_stale",
                invoker,
            )?;
            recovered.push(row.clone());
        }
    }
    let rows = recovered;
    tx.commit()?;
    Ok(rows)
}

pub fn list(conn: &rusqlite::Connection) -> Result<Vec<(String, String, String, String)>> {
    let mut stmt = conn.prepare("SELECT resource_id, owner_kind, owner_display_id, fencing_token FROM resource_locks ORDER BY resource_id")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stores_test::*;

    mod stores_test {
        use rusqlite::Connection;
        pub fn conn() -> Connection {
            let c = Connection::open_in_memory().unwrap();
            c.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL).unwrap();
            c
        }
        pub fn hist(c: &Connection) -> Vec<(String, String)> {
            let mut s=c.prepare("SELECT verb, invoker FROM transition_history WHERE store='resource_locks' ORDER BY id").unwrap();
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        }
    }
    fn params<'a>(r: &'a str, ttl: Option<u64>, invoker: Actor) -> AcquireParams<'a> {
        AcquireParams {
            resource_id: r,
            owner_display_id: "T1",
            owner_kind: "task",
            ttl_secs: ttl,
            claim_source: None,
            invoker,
        }
    }

    #[test]
    fn acquire_free_returns_token() {
        let c = conn();
        assert!(!acquire(&c, &params("r", None, Actor::Framework))
            .unwrap()
            .0
            .is_empty());
    }
    #[test]
    fn second_acquire_busy() {
        let c = conn();
        acquire(&c, &params("r", None, Actor::Framework)).unwrap();
        assert!(acquire(&c, &params("r", None, Actor::Framework))
            .unwrap_err()
            .to_string()
            .contains("ResourceLockBusy"));
    }
    #[test]
    fn expired_acquire_recovers_and_audits() {
        let c = conn();
        acquire(&c, &params("r", Some(0), Actor::Human)).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        acquire(&c, &params("r", None, Actor::Human)).unwrap();
        assert!(hist(&c).contains(&("recover_stale".into(), "human".into())));
    }
    #[test]
    fn release_correct_token_audits() {
        let c = conn();
        let t = acquire(&c, &params("r", None, Actor::Framework)).unwrap();
        release(&c, "r", &t.0, Actor::Framework).unwrap();
        assert_eq!(
            hist(&c).last().unwrap(),
            &("release".into(), "framework".into())
        );
    }
    #[test]
    fn release_wrong_token_no_audit() {
        let c = conn();
        acquire(&c, &params("r", None, Actor::Framework)).unwrap();
        assert!(release(&c, "r", "bad", Actor::Framework).is_err());
        assert_eq!(hist(&c).len(), 1);
    }
    #[test]
    fn recover_stale_deletes_only_expired() {
        let c = conn();
        acquire(&c, &params("old", Some(0), Actor::Framework)).unwrap();
        acquire(&c, &params("new", Some(60), Actor::Framework)).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let v = recover_stale(&c, now(), Actor::Human).unwrap();
        assert_eq!(v.len(), 1);
        assert!(check_ownership(&c, "new", "T1").unwrap());
    }
    #[test]
    fn check_ownership_token_rejects_same_owner_token_rotation() {
        let c = conn();
        let t = acquire(&c, &params("r", None, Actor::Framework)).unwrap();
        assert!(check_ownership_token(&c, "r", "T1", &t.0).unwrap());
        c.execute(
            "UPDATE resource_locks SET fencing_token='rotated' WHERE resource_id='r'",
            [],
        )
        .unwrap();
        assert!(check_ownership(&c, "r", "T1").unwrap());
        assert!(!check_ownership_token(&c, "r", "T1", &t.0).unwrap());
    }
    #[test]
    fn successful_mutations_write_expected_verbs_invokers() {
        let c = conn();
        let t = acquire(&c, &params("r", None, Actor::Human)).unwrap();
        release(&c, "r", &t.0, Actor::Human).unwrap();
        assert_eq!(
            hist(&c),
            vec![
                ("acquire".into(), "human".into()),
                ("release".into(), "human".into())
            ]
        );
    }
}
