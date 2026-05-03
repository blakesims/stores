//! .stores/policies.yaml — declarative policy layer.
//!
//! Default action is ALLOW (matches "everything flows between the gates").
//! NEVER policies are sacrosanct: if any NEVER predicate matches a transition,
//! the transition halts regardless of other policies.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::flow::predicate::{eval, PredicateExpr};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PoliciesYaml {
    /// SHA-256 (lowercase hex) of the canonical YAML bytes used to load this
    /// struct. Recorded on every automatic transition so historical decisions
    /// can be re-verified against the file's checksum.
    #[serde(skip)]
    pub hash: String,
    #[serde(default)]
    pub policies: Vec<PolicyEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PolicyEntry {
    pub id: String,
    pub transition: TransitionRef,
    pub predicate: PredicateExpr,
    pub action: Action,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TransitionRef {
    pub store: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Halt,
    /// Sacrosanct halt — cannot be overridden by other policies.
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Transition allowed. `policy_id` is the rule that decided, or
    /// `"default-allow"` when no policy matched.
    Allow {
        policy_id: String,
    },
    Halt {
        policy_id: String,
    },
}

impl PoliciesYaml {
    pub fn from_yaml(s: &str) -> Result<Self> {
        let mut parsed: Self =
            serde_yaml::from_str(s).map_err(|e| anyhow!("policies.yaml parse error: {}", e))?;
        parsed.hash = sha256_hex(s.as_bytes());
        parsed.validate()?;
        Ok(parsed)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for (i, p) in self.policies.iter().enumerate() {
            if p.id.is_empty() {
                bail!("policies[{}].id: empty string not permitted", i);
            }
            if !seen.insert(p.id.clone()) {
                bail!("policies[{}].id: duplicate policy id '{}'", i, p.id);
            }
            if p.transition.store.is_empty()
                || p.transition.from.is_empty()
                || p.transition.to.is_empty()
            {
                bail!(
                    "policies[{}].transition: store/from/to must all be non-empty",
                    i
                );
            }
        }
        Ok(())
    }
}

pub fn load_from_path(path: &std::path::Path) -> Result<PoliciesYaml> {
    let bytes =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    PoliciesYaml::from_yaml(&bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Decide whether an automatic transition is allowed.
///
/// Semantics:
///   1. NEVER first: if any NEVER policy matches the transition + row, halt
///      with that policy's id.
///   2. Halt next: if any Halt policy matches, halt with that policy's id.
///   3. Otherwise allow with the matching Allow policy id (or
///      `default-allow` if none matched).
pub fn decide(
    policies: &PoliciesYaml,
    store: &str,
    from: &str,
    to: &str,
    row: &Value,
) -> Result<Decision> {
    let candidates: Vec<&PolicyEntry> = policies
        .policies
        .iter()
        .filter(|p| {
            p.transition.store == store && p.transition.from == from && p.transition.to == to
        })
        .collect();

    // 1. NEVER first.
    for p in &candidates {
        if matches!(p.action, Action::Never) && eval(&p.predicate, row)? {
            return Ok(Decision::Halt {
                policy_id: p.id.clone(),
            });
        }
    }
    // 2. Halt next.
    for p in &candidates {
        if matches!(p.action, Action::Halt) && eval(&p.predicate, row)? {
            return Ok(Decision::Halt {
                policy_id: p.id.clone(),
            });
        }
    }
    // 3. Allow.
    for p in &candidates {
        if matches!(p.action, Action::Allow) && eval(&p.predicate, row)? {
            return Ok(Decision::Allow {
                policy_id: p.id.clone(),
            });
        }
    }
    Ok(Decision::Allow {
        policy_id: "default-allow".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SAMPLE: &str = r#"
policies:
  - id: never-skip-review
    transition: { store: tasks, from: planning, to: accepted }
    predicate:
      op: "=="
      left: "$tier_hint"
      right: "T3"
    action: never
  - id: allow-T1-fast-path
    transition: { store: tasks, from: planning, to: accepted }
    predicate:
      op: "=="
      left: "$tier_hint"
      right: "T1"
    action: allow
  - id: halt-on-empty-branch
    transition: { store: tasks, from: in_review, to: accepted }
    predicate:
      op: "=="
      left: "$branch"
      right: ""
    action: halt
"#;

    fn row(tier: &str) -> Value {
        json!({ "tier_hint": tier, "branch": "feat/x" })
    }

    #[test]
    fn parses_well_formed() {
        let p = PoliciesYaml::from_yaml(SAMPLE).unwrap();
        assert_eq!(p.policies.len(), 3);
        assert!(!p.hash.is_empty());
    }

    #[test]
    fn duplicate_id_rejected() {
        let yaml = r#"
policies:
  - id: dup
    transition: { store: tasks, from: a, to: b }
    predicate: { op: "==", left: "x", right: "x" }
    action: allow
  - id: dup
    transition: { store: tasks, from: a, to: b }
    predicate: { op: "==", left: "x", right: "x" }
    action: allow
"#;
        let err = PoliciesYaml::from_yaml(yaml).unwrap_err().to_string();
        assert!(err.contains("duplicate"));
    }

    #[test]
    fn missing_required_field_reports_path() {
        let yaml = r#"
policies:
  - id: x
    transition: { store: tasks, from: a, to: b }
    action: allow
"#;
        let err = PoliciesYaml::from_yaml(yaml).unwrap_err().to_string();
        assert!(err.contains("predicate"), "got: {err}");
    }

    #[test]
    fn never_overrides_allow() {
        let p = PoliciesYaml::from_yaml(SAMPLE).unwrap();
        // Both NEVER (matches T3) and Allow (matches T1) live on the same
        // transition. With a T3 row, NEVER must win.
        let r = json!({ "tier_hint": "T3" });
        let d = decide(&p, "tasks", "planning", "accepted", &r).unwrap();
        assert_eq!(
            d,
            Decision::Halt {
                policy_id: "never-skip-review".into()
            }
        );
    }

    #[test]
    fn allow_path_when_no_never_matches() {
        let p = PoliciesYaml::from_yaml(SAMPLE).unwrap();
        let d = decide(&p, "tasks", "planning", "accepted", &row("T1")).unwrap();
        assert_eq!(
            d,
            Decision::Allow {
                policy_id: "allow-T1-fast-path".into()
            }
        );
    }

    #[test]
    fn default_allow_when_nothing_matches() {
        let p = PoliciesYaml::from_yaml(SAMPLE).unwrap();
        // Tier hint "T2" matches no rule on this transition.
        let d = decide(&p, "tasks", "planning", "accepted", &row("T2")).unwrap();
        assert_eq!(
            d,
            Decision::Allow {
                policy_id: "default-allow".into()
            }
        );
    }

    #[test]
    fn halt_policy_fires() {
        let p = PoliciesYaml::from_yaml(SAMPLE).unwrap();
        let r = json!({ "branch": "" });
        let d = decide(&p, "tasks", "in_review", "accepted", &r).unwrap();
        assert_eq!(
            d,
            Decision::Halt {
                policy_id: "halt-on-empty-branch".into()
            }
        );
    }

    #[test]
    fn hash_is_stable_and_changes_on_edit() {
        let a = PoliciesYaml::from_yaml(SAMPLE).unwrap();
        let b = PoliciesYaml::from_yaml(SAMPLE).unwrap();
        assert_eq!(a.hash, b.hash);
        let mutated = SAMPLE.replace("never-skip-review", "never-skip-review-2");
        let c = PoliciesYaml::from_yaml(&mutated).unwrap();
        assert_ne!(a.hash, c.hash);
    }
}
