//! Tiny predicate language used by the policy layer.
//!
//! Operators: `==`, `!=`, `in`, `not in`, `matches` (regex).
//!
//! Operands carry a discriminating prefix:
//!   - `$path.to.field` — resolved against the row (a `serde_json::Value`).
//!   - `helper:<name>`  — a derived helper. Supported: `linked_observation_count`,
//!     `branch`, `status`.
//!   - any other JSON literal (string, number, bool, list, null).

use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "op")]
pub enum PredicateExpr {
    #[serde(rename = "==")]
    Eq { left: Value, right: Value },
    #[serde(rename = "!=")]
    Neq { left: Value, right: Value },
    #[serde(rename = "in")]
    In { left: Value, right: Value },
    #[serde(rename = "not in")]
    NotIn { left: Value, right: Value },
    #[serde(rename = "matches")]
    Matches { left: Value, right: Value },
    /// T140 P2: compose predicates with logical AND so a single subscription
    /// can require multiple row-state preconditions (e.g. workspace_path set
    /// AND activation flipped to 'active'). All inner predicates must
    /// evaluate true for the AllOf to evaluate true; an empty list evaluates
    /// true (vacuously).
    #[serde(rename = "all_of")]
    AllOf { all: Vec<PredicateExpr> },
}

pub fn eval(expr: &PredicateExpr, row: &Value) -> Result<bool> {
    match expr {
        PredicateExpr::Eq { left, right } => {
            Ok(value_equal(&resolve(left, row), &resolve(right, row)))
        }
        PredicateExpr::Neq { left, right } => {
            Ok(!value_equal(&resolve(left, row), &resolve(right, row)))
        }
        PredicateExpr::In { left, right } => {
            let l = resolve(left, row);
            let r = resolve(right, row);
            let arr = r
                .as_array()
                .ok_or_else(|| anyhow!("'in' rhs must be a list, got {}", r))?;
            Ok(arr.iter().any(|x| value_equal(x, &l)))
        }
        PredicateExpr::NotIn { left, right } => {
            let l = resolve(left, row);
            let r = resolve(right, row);
            let arr = r
                .as_array()
                .ok_or_else(|| anyhow!("'not in' rhs must be a list, got {}", r))?;
            Ok(!arr.iter().any(|x| value_equal(x, &l)))
        }
        PredicateExpr::Matches { left, right } => {
            let l = resolve(left, row);
            let r = resolve(right, row);
            let s = l
                .as_str()
                .ok_or_else(|| anyhow!("'matches' lhs must resolve to a string, got {}", l))?;
            let pat = r
                .as_str()
                .ok_or_else(|| anyhow!("'matches' rhs must be a string regex, got {}", r))?;
            let re = Regex::new(pat).map_err(|e| anyhow!("invalid regex '{}': {}", pat, e))?;
            Ok(re.is_match(s))
        }
        PredicateExpr::AllOf { all } => {
            for inner in all {
                if !eval(inner, row)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

fn value_equal(a: &Value, b: &Value) -> bool {
    // Coerce numeric equality across i64/u64/f64.
    if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
        return x == y;
    }
    a == b
}

fn resolve(operand: &Value, row: &Value) -> Value {
    if let Some(s) = operand.as_str() {
        if let Some(path) = s.strip_prefix('$') {
            return resolve_path(path, row);
        }
        if let Some(name) = s.strip_prefix("helper:") {
            return resolve_helper(name, row);
        }
    }
    operand.clone()
}

fn resolve_path(path: &str, row: &Value) -> Value {
    let mut cur = row;
    for seg in path.split('.') {
        match cur.get(seg) {
            Some(v) => cur = v,
            None => return Value::Null,
        }
    }
    cur.clone()
}

fn resolve_helper(name: &str, row: &Value) -> Value {
    match name {
        "linked_observation_count" => {
            let v = row.get("linked_observations");
            let n = v.and_then(|x| x.as_array()).map(|a| a.len()).unwrap_or(0);
            Value::Number(serde_json::Number::from(n as u64))
        }
        "branch" => row.get("branch").cloned().unwrap_or(Value::Null),
        "status" => row.get("status").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// Convenience: parse a predicate from a YAML fragment.
pub fn from_yaml(s: &str) -> Result<PredicateExpr> {
    serde_yaml::from_str(s).map_err(|e| anyhow!("predicate parse error: {}", e))
}

#[allow(dead_code)]
pub(crate) fn ensure_valid(expr: &PredicateExpr) -> Result<()> {
    match expr {
        PredicateExpr::Matches { right, .. } => {
            if let Some(s) = right.as_str() {
                Regex::new(s).map_err(|e| anyhow!("invalid regex '{}': {}", s, e))?;
            } else {
                bail!("matches rhs must be a string");
            }
        }
        PredicateExpr::AllOf { all } => {
            for inner in all {
                ensure_valid(inner)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row() -> Value {
        json!({
            "tier_hint": "T3",
            "branch": "feat/x",
            "status": "in_review",
            "linked_observations": ["L001", "L002"],
            "tasks": { "tier_hint": "T3" }
        })
    }

    #[test]
    fn eq_path_vs_literal() {
        let e = PredicateExpr::Eq {
            left: json!("$tier_hint"),
            right: json!("T3"),
        };
        assert!(eval(&e, &row()).unwrap());
    }

    #[test]
    fn eq_nested_path() {
        let e = PredicateExpr::Eq {
            left: json!("$tasks.tier_hint"),
            right: json!("T3"),
        };
        assert!(eval(&e, &row()).unwrap());
    }

    #[test]
    fn neq_works() {
        let e = PredicateExpr::Neq {
            left: json!("$status"),
            right: json!("accepted"),
        };
        assert!(eval(&e, &row()).unwrap());
    }

    #[test]
    fn in_list_literal() {
        let e = PredicateExpr::In {
            left: json!("$tier_hint"),
            right: json!(["T2", "T3"]),
        };
        assert!(eval(&e, &row()).unwrap());
    }

    #[test]
    fn not_in_list_literal() {
        let e = PredicateExpr::NotIn {
            left: json!("$tier_hint"),
            right: json!(["T1", "T2"]),
        };
        assert!(eval(&e, &row()).unwrap());
    }

    #[test]
    fn matches_regex() {
        let e = PredicateExpr::Matches {
            left: json!("$branch"),
            right: json!("^feat/"),
        };
        assert!(eval(&e, &row()).unwrap());
    }

    #[test]
    fn helper_linked_observation_count() {
        let e = PredicateExpr::Eq {
            left: json!("helper:linked_observation_count"),
            right: json!(2),
        };
        assert!(eval(&e, &row()).unwrap());
    }

    #[test]
    fn missing_path_resolves_null() {
        let e = PredicateExpr::Eq {
            left: json!("$nope.nope"),
            right: json!(null),
        };
        assert!(eval(&e, &row()).unwrap());
    }

    #[test]
    fn invalid_regex_errors() {
        let e = PredicateExpr::Matches {
            left: json!("$branch"),
            right: json!("("),
        };
        assert!(eval(&e, &row()).is_err());
    }

    #[test]
    fn yaml_round_trip() {
        let y = "op: \"==\"\nleft: \"$status\"\nright: \"in_review\"\n";
        let e = from_yaml(y).unwrap();
        assert!(eval(&e, &row()).unwrap());
    }

    /// T140 P2 / Task 2.2: confirm `$activation` path lookup resolves the
    /// top-level `activation` field on the row JSON. The substrate's
    /// activation predicate (added to `auto-drive` and `integrate`
    /// subscribers in this phase) depends on this path-lookup behavior.
    #[test]
    fn activation_path_lookup_eval() {
        let active = json!({"activation": "active", "status": "planning"});
        let inactive = json!({"activation": "inactive", "status": "planning"});
        let missing = json!({"status": "planning"});

        let pred = PredicateExpr::Eq {
            left: json!("$activation"),
            right: json!("active"),
        };
        assert!(eval(&pred, &active).unwrap(), "active row must match");
        assert!(
            !eval(&pred, &inactive).unwrap(),
            "inactive row must not match"
        );
        assert!(
            !eval(&pred, &missing).unwrap(),
            "missing column must not match (fail-closed)"
        );
    }

    /// T140 P2: AllOf composes multiple predicates with logical AND. Empty
    /// list is vacuously true; first false short-circuits.
    #[test]
    fn all_of_composes_with_and() {
        let r = json!({"activation": "active", "workspace_path": "/tmp/wt"});

        let both_true = PredicateExpr::AllOf {
            all: vec![
                PredicateExpr::Neq {
                    left: json!("$workspace_path"),
                    right: json!(""),
                },
                PredicateExpr::Eq {
                    left: json!("$activation"),
                    right: json!("active"),
                },
            ],
        };
        assert!(eval(&both_true, &r).unwrap());

        let one_false = PredicateExpr::AllOf {
            all: vec![
                PredicateExpr::Neq {
                    left: json!("$workspace_path"),
                    right: json!(""),
                },
                PredicateExpr::Eq {
                    left: json!("$activation"),
                    right: json!("inactive"),
                },
            ],
        };
        assert!(!eval(&one_false, &r).unwrap());

        let empty = PredicateExpr::AllOf { all: vec![] };
        assert!(eval(&empty, &r).unwrap(), "empty AllOf is vacuously true");
    }

    /// AllOf round-trips through YAML so agents.yaml can express it.
    #[test]
    fn all_of_yaml_round_trip() {
        let y = "op: all_of\nall:\n  - op: \"!=\"\n    left: \"$workspace_path\"\n    right: \"\"\n  - op: \"==\"\n    left: \"$activation\"\n    right: \"active\"\n";
        let e = from_yaml(y).unwrap();
        let r = json!({"workspace_path": "/tmp/wt", "activation": "active"});
        assert!(eval(&e, &r).unwrap());
    }
}
