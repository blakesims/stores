use anyhow::Result;
use std::collections::BTreeMap;

use crate::schema::{actor::Actor, Schema};

/// In-memory entry map: nested structure mirroring the schema.
/// Leaf values are serde_json::Value scalars; Record values are nested
/// serde_json::Value::Object.
pub type EntryMap = BTreeMap<String, serde_json::Value>;

/// Stub validator — Phase 5 fills the body.
pub fn validate(
    _schema: &Schema,
    _entry: &EntryMap,
    _invoker: Actor,
) -> Result<()> {
    Ok(())
}
