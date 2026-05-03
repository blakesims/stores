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
    // Compile any regex eagerly.
    if let PredicateExpr::Matches { right, .. } = expr {
        if let Some(s) = right.as_str() {
            Regex::new(s).map_err(|e| anyhow!("invalid regex '{}': {}", s, e))?;
        } else {
            bail!("matches rhs must be a string");
        }
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
}
