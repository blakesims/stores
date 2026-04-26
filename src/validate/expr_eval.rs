/// Evaluate a `schema::expr::Expr` guard against an `EntryMap`.
///
/// Rules:
/// - `Path` lhs: look up the value at the dotted path in the entry.
///   - `Rhs::Literal`: compare as strings (value must be a JSON string).
///   - `Rhs::Integer`: compare numerically (value must be a JSON integer).
/// - `PathLength` lhs: resolve the length of the value at the path.
///   - Array → `array.len()` as `i64`.
///   - String → `str.chars().count()` as `i64`.
///   - Object (record) → `object.len()` as `i64`.
///   - Any other shape / missing path → predicate is `false` (never panics).
/// - Missing / null paths always return `false`.

use crate::schema::expr::{Expr, Lhs, Op, Rhs};
use crate::validate::EntryMap;
use serde_json::Value;

/// Evaluate `expr` against `entry`.  Returns `false` for missing or
/// type-mismatched paths rather than panicking.
pub fn eval(expr: &Expr, entry: &EntryMap) -> bool {
    match &expr.lhs {
        Lhs::Path(path) => eval_path(path, &expr.op, &expr.rhs, entry),
        Lhs::PathLength(path) => eval_length(path, &expr.op, &expr.rhs, entry),
    }
}

/// Resolve an `Rhs` value to an `i64` for numeric comparison.
/// Returns `None` when the RHS is a path that is missing/wrong-type.
fn rhs_as_i64(rhs: &Rhs, entry: &EntryMap) -> Option<i64> {
    match rhs {
        Rhs::Integer(n) => Some(*n),
        Rhs::PathLength(path) => {
            let val = lookup_path(path, entry)?;
            Some(match val {
                Value::Array(arr) => arr.len() as i64,
                Value::String(s) => s.chars().count() as i64,
                Value::Object(map) => map.len() as i64,
                _ => return None,
            })
        }
        // Path and Literal are not numeric — callers handle them separately
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Walk a dotted path into a nested entry/value, returning the leaf value.
fn lookup_path<'a>(path: &[String], entry: &'a EntryMap) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let first = entry.get(&path[0])?;
    if path.len() == 1 {
        return Some(first);
    }
    // Recurse into nested Value::Object
    lookup_in_value(&path[1..], first)
}

fn lookup_in_value<'a>(path: &[String], val: &'a Value) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(val);
    }
    match val {
        Value::Object(map) => {
            let child = map.get(&path[0])?;
            lookup_in_value(&path[1..], child)
        }
        // Arrays: index by parsed usize (numeric path segment)
        Value::Array(arr) => {
            let idx: usize = path[0].parse().ok()?;
            let child = arr.get(idx)?;
            lookup_in_value(&path[1..], child)
        }
        _ => None,
    }
}

fn eval_path(path: &[String], op: &Op, rhs: &Rhs, entry: &EntryMap) -> bool {
    let val = match lookup_path(path, entry) {
        Some(v) => v,
        None => return false,
    };

    match rhs {
        Rhs::Literal(lit) => {
            let s = match val {
                Value::String(s) => s.as_str(),
                _ => return false,
            };
            match op {
                Op::Eq => s == lit,
                Op::Neq => s != lit,
                // Lexicographic comparison for string paths (unusual but consistent)
                Op::Lt => s < lit.as_str(),
                Op::Le => s <= lit.as_str(),
                Op::Gt => s > lit.as_str(),
                Op::Ge => s >= lit.as_str(),
            }
        }
        Rhs::Integer(n) => {
            let i = match val {
                Value::Number(num) => match num.as_i64() {
                    Some(i) => i,
                    None => return false,
                },
                _ => return false,
            };
            compare_i64(i, *n, op)
        }
        // RHS is a path or path.length — resolve the LHS as i64 first, then compare.
        // Handles `current_phase < plan.phases.length` and `a == b`.
        Rhs::PathLength(_) => {
            // LHS must be numeric (integer) for a path < path.length comparison.
            let lhs_i = match val {
                Value::Number(num) => match num.as_i64() {
                    Some(i) => i,
                    None => return false,
                },
                _ => return false,
            };
            let rhs_i = match rhs_as_i64(rhs, entry) {
                Some(n) => n,
                None => return false,
            };
            compare_i64(lhs_i, rhs_i, op)
        }
        Rhs::Path(rhs_path) => {
            // Both sides resolved from the entry.  Try numeric comparison first
            // (both are integers), then string comparison.
            let rhs_val = match lookup_path(rhs_path, entry) {
                Some(v) => v,
                None => return false,
            };
            match (val, rhs_val) {
                (Value::Number(l), Value::Number(r)) => {
                    match (l.as_i64(), r.as_i64()) {
                        (Some(li), Some(ri)) => compare_i64(li, ri, op),
                        _ => false,
                    }
                }
                (Value::String(l), Value::String(r)) => {
                    let (ls, rs) = (l.as_str(), r.as_str());
                    match op {
                        Op::Eq => ls == rs,
                        Op::Neq => ls != rs,
                        Op::Lt => ls < rs,
                        Op::Le => ls <= rs,
                        Op::Gt => ls > rs,
                        Op::Ge => ls >= rs,
                    }
                }
                _ => false,
            }
        }
    }
}

fn eval_length(path: &[String], op: &Op, rhs: &Rhs, entry: &EntryMap) -> bool {
    let val = match lookup_path(path, entry) {
        Some(v) => v,
        None => return false,
    };

    let len: i64 = match val {
        Value::Array(arr) => arr.len() as i64,
        Value::String(s) => s.chars().count() as i64,
        Value::Object(map) => map.len() as i64,
        _ => return false,
    };

    let n = match rhs {
        Rhs::Integer(n) => *n,
        Rhs::Literal(_) => return false, // parse_guard already rejects this
        Rhs::PathLength(_) => match rhs_as_i64(rhs, entry) {
            Some(n) => n,
            None => return false,
        },
        // path.length compared to a bare path would be unusual; resolve as integer if possible
        Rhs::Path(rhs_path) => {
            let rhs_val = match lookup_path(rhs_path, entry) {
                Some(v) => v,
                None => return false,
            };
            match rhs_val {
                Value::Number(num) => match num.as_i64() {
                    Some(i) => i,
                    None => return false,
                },
                _ => return false,
            }
        }
    };

    compare_i64(len, n, op)
}

fn compare_i64(lhs: i64, rhs: i64, op: &Op) -> bool {
    match op {
        Op::Eq => lhs == rhs,
        Op::Neq => lhs != rhs,
        Op::Lt => lhs < rhs,
        Op::Le => lhs <= rhs,
        Op::Gt => lhs > rhs,
        Op::Ge => lhs >= rhs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::expr::{Expr, Lhs, Op, Rhs};
    use crate::schema::expr::parse_guard;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn entry_from_json(val: Value) -> EntryMap {
        match val {
            Value::Object(map) => map.into_iter().collect(),
            _ => panic!("expected JSON object"),
        }
    }

    // ---- AC1.4: marquee tests ----

    #[test]
    fn current_cycle_le_4_with_value_4_is_true() {
        let expr = parse_guard("current_cycle <= 4").unwrap();
        let entry = entry_from_json(json!({ "current_cycle": 4 }));
        assert!(eval(&expr, &entry), "4 <= 4 should be true");
    }

    #[test]
    fn current_cycle_le_4_with_value_5_is_false() {
        let expr = parse_guard("current_cycle <= 4").unwrap();
        let entry = entry_from_json(json!({ "current_cycle": 5 }));
        assert!(!eval(&expr, &entry), "5 <= 4 should be false");
    }

    /// Renamed from `current_phase_lt_plan_phases_length_depth3` (was misleading — tested
    /// `plan.phases.length > 1`, not the AC1.4-required form).  Kept as a regression.
    #[test]
    fn plan_phases_length_gt_constant_depth3() {
        // plan.phases is a list with 2 elements; 2 > 1 → true
        let expr = parse_guard("plan.phases.length > 1").unwrap();
        let entry = entry_from_json(json!({
            "plan": {
                "phases": [
                    { "name": "Phase 1" },
                    { "name": "Phase 2" }
                ]
            }
        }));
        assert!(eval(&expr, &entry), "length 2 > 1 should be true");
    }

    /// Renamed from `current_phase_lt_plan_phases_length_false` (was misleading).
    #[test]
    fn plan_phases_length_gt_constant_false() {
        let expr = parse_guard("plan.phases.length > 3").unwrap();
        let entry = entry_from_json(json!({
            "plan": { "phases": [{"name": "p1"}, {"name": "p2"}] }
        }));
        assert!(!eval(&expr, &entry), "length 2 > 3 should be false");
    }

    // ---- AC1.4: path-vs-path-length form (C1 fix) ----

    #[test]
    fn current_phase_lt_plan_phases_length_true() {
        // The exact AC1.4-required form: current_phase < plan.phases.length
        // Entry: current_phase=1, plan.phases has 2 elements → 1 < 2 → true
        let expr = parse_guard("current_phase < plan.phases.length").unwrap();
        let entry = entry_from_json(json!({
            "current_phase": 1,
            "plan": {
                "phases": [
                    { "name": "Phase 1" },
                    { "name": "Phase 2" }
                ]
            }
        }));
        assert!(eval(&expr, &entry), "1 < 2 (length) should be true");
    }

    #[test]
    fn current_phase_lt_plan_phases_length_false_equal() {
        // current_phase=2, plan.phases has 2 elements → 2 < 2 → false
        let expr = parse_guard("current_phase < plan.phases.length").unwrap();
        let entry = entry_from_json(json!({
            "current_phase": 2,
            "plan": {
                "phases": [
                    { "name": "Phase 1" },
                    { "name": "Phase 2" }
                ]
            }
        }));
        assert!(!eval(&expr, &entry), "2 < 2 should be false");
    }

    #[test]
    fn current_phase_lt_plan_phases_length_missing_phase_returns_false() {
        // Missing current_phase → false (not a crash)
        let expr = parse_guard("current_phase < plan.phases.length").unwrap();
        let entry = entry_from_json(json!({
            "plan": { "phases": [{"name": "p1"}, {"name": "p2"}] }
        }));
        assert!(!eval(&expr, &entry), "missing current_phase should return false");
    }

    // ---- path == path form ----

    #[test]
    fn path_eq_path_true() {
        let expr = parse_guard("a == b").unwrap();
        let entry = entry_from_json(json!({ "a": 1, "b": 1 }));
        assert!(eval(&expr, &entry), "a==b when both are 1 should be true");
    }

    #[test]
    fn path_eq_path_false() {
        let expr = parse_guard("a == b").unwrap();
        let entry = entry_from_json(json!({ "a": 1, "b": 2 }));
        assert!(!eval(&expr, &entry), "a==b when a=1 b=2 should be false");
    }

    // ---- All operators ----

    #[test]
    fn eq_integer() {
        let e = Expr { lhs: Lhs::Path(vec!["n".into()]), op: Op::Eq, rhs: Rhs::Integer(3) };
        let entry = entry_from_json(json!({ "n": 3 }));
        assert!(eval(&e, &entry));
        let entry2 = entry_from_json(json!({ "n": 4 }));
        assert!(!eval(&e, &entry2));
    }

    #[test]
    fn neq_integer() {
        let e = Expr { lhs: Lhs::Path(vec!["n".into()]), op: Op::Neq, rhs: Rhs::Integer(3) };
        let entry = entry_from_json(json!({ "n": 5 }));
        assert!(eval(&e, &entry));
        let entry2 = entry_from_json(json!({ "n": 3 }));
        assert!(!eval(&e, &entry2));
    }

    #[test]
    fn lt_integer() {
        let e = Expr { lhs: Lhs::Path(vec!["n".into()]), op: Op::Lt, rhs: Rhs::Integer(5) };
        let entry = entry_from_json(json!({ "n": 4 }));
        assert!(eval(&e, &entry));
        let entry2 = entry_from_json(json!({ "n": 5 }));
        assert!(!eval(&e, &entry2));
    }

    #[test]
    fn gt_integer() {
        let e = Expr { lhs: Lhs::Path(vec!["n".into()]), op: Op::Gt, rhs: Rhs::Integer(3) };
        let entry = entry_from_json(json!({ "n": 4 }));
        assert!(eval(&e, &entry));
        let entry2 = entry_from_json(json!({ "n": 3 }));
        assert!(!eval(&e, &entry2));
    }

    #[test]
    fn ge_integer() {
        let e = Expr { lhs: Lhs::Path(vec!["n".into()]), op: Op::Ge, rhs: Rhs::Integer(3) };
        let entry = entry_from_json(json!({ "n": 3 }));
        assert!(eval(&e, &entry));
        let entry2 = entry_from_json(json!({ "n": 2 }));
        assert!(!eval(&e, &entry2));
    }

    #[test]
    fn eq_literal() {
        let e = Expr {
            lhs: Lhs::Path(vec!["status".into()]),
            op: Op::Eq,
            rhs: Rhs::Literal("PASS".to_string()),
        };
        let entry = entry_from_json(json!({ "status": "PASS" }));
        assert!(eval(&e, &entry));
        let entry2 = entry_from_json(json!({ "status": "FAIL" }));
        assert!(!eval(&e, &entry2));
    }

    #[test]
    fn neq_literal() {
        let e = Expr {
            lhs: Lhs::Path(vec!["status".into()]),
            op: Op::Neq,
            rhs: Rhs::Literal("FAIL".to_string()),
        };
        let entry = entry_from_json(json!({ "status": "PASS" }));
        assert!(eval(&e, &entry));
    }

    #[test]
    fn length_eq() {
        let e = Expr {
            lhs: Lhs::PathLength(vec!["tags".into()]),
            op: Op::Eq,
            rhs: Rhs::Integer(3),
        };
        let entry = entry_from_json(json!({ "tags": ["a", "b", "c"] }));
        assert!(eval(&e, &entry));
        let entry2 = entry_from_json(json!({ "tags": ["a", "b"] }));
        assert!(!eval(&e, &entry2));
    }

    #[test]
    fn length_le() {
        let e = Expr {
            lhs: Lhs::PathLength(vec!["items".into()]),
            op: Op::Le,
            rhs: Rhs::Integer(4),
        };
        let entry = entry_from_json(json!({ "items": [1, 2, 3] }));
        assert!(eval(&e, &entry));
    }

    #[test]
    fn length_gt() {
        let e = Expr {
            lhs: Lhs::PathLength(vec!["items".into()]),
            op: Op::Gt,
            rhs: Rhs::Integer(2),
        };
        let entry = entry_from_json(json!({ "items": [1, 2, 3] }));
        assert!(eval(&e, &entry));
    }

    // ---- Missing / null paths → false ----

    #[test]
    fn missing_path_returns_false() {
        let e = Expr {
            lhs: Lhs::Path(vec!["nonexistent".into()]),
            op: Op::Eq,
            rhs: Rhs::Integer(1),
        };
        let entry: EntryMap = BTreeMap::new();
        assert!(!eval(&e, &entry), "missing path should return false");
    }

    #[test]
    fn null_path_returns_false() {
        let e = Expr {
            lhs: Lhs::Path(vec!["field".into()]),
            op: Op::Eq,
            rhs: Rhs::Integer(1),
        };
        let entry = entry_from_json(json!({ "field": null }));
        assert!(!eval(&e, &entry));
    }

    #[test]
    fn missing_length_path_returns_false() {
        let e = Expr {
            lhs: Lhs::PathLength(vec!["missing".into()]),
            op: Op::Gt,
            rhs: Rhs::Integer(0),
        };
        let entry: EntryMap = BTreeMap::new();
        assert!(!eval(&e, &entry), "missing length path should return false");
    }

    #[test]
    fn type_mismatch_returns_false() {
        // integer field, but we look up a string value
        let e = Expr {
            lhs: Lhs::Path(vec!["n".into()]),
            op: Op::Eq,
            rhs: Rhs::Integer(1),
        };
        let entry = entry_from_json(json!({ "n": "not-a-number" }));
        assert!(!eval(&e, &entry));
    }

    // ---- Nested path (depth 2 & 3) ----

    #[test]
    fn nested_path_depth2() {
        let e = parse_guard("triage.verdict == 'T3'").unwrap();
        let entry = entry_from_json(json!({
            "triage": { "verdict": "T3" }
        }));
        assert!(eval(&e, &entry));
        let entry2 = entry_from_json(json!({
            "triage": { "verdict": "T1" }
        }));
        assert!(!eval(&e, &entry2));
    }

    #[test]
    fn nested_path_depth3_integer() {
        // cycles[0].phase == 2
        let e = Expr {
            lhs: Lhs::Path(vec!["cycles".into(), "0".into(), "phase".into()]),
            op: Op::Eq,
            rhs: Rhs::Integer(2),
        };
        let entry = entry_from_json(json!({
            "cycles": [{ "phase": 2, "executor": { "summary": "done" } }]
        }));
        assert!(eval(&e, &entry));
    }

    #[test]
    fn string_length() {
        let e = Expr {
            lhs: Lhs::PathLength(vec!["name".into()]),
            op: Op::Ge,
            rhs: Rhs::Integer(3),
        };
        let entry = entry_from_json(json!({ "name": "foo" }));
        assert!(eval(&e, &entry)); // "foo".len() == 3 >= 3
    }
}
