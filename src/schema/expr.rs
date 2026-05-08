/// General-purpose guard expression AST for the workflow engine (Task 1.3).
///
/// Supports the locked subset (D6):
///   - Equality:  `path == 'literal'`  or  `path == integer`
///   - Inequality: `path != 'literal'` or  `path != integer`
///   - Length comparisons: `path.length < N`, `path.length <= N`, `path.length > N`,
///     `path.length >= N`, `path.length == N`
///
/// NOT supported: `&&`, `||`, AND, OR.  The single-quote rule and double-quote
/// rejection from `required_when.rs` carry over verbatim.
///
/// This module lives **alongside** `required_when.rs`; it does NOT rename or
/// replace it.  The narrower equality-only parser in `required_when.rs` is kept
/// for backwards compatibility.
///
/// `required_when.rs` re-exports `expr::Expr` so call-sites that used
/// `RequiredWhenExpr` keep working.
use anyhow::{bail, Result};

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

/// Left-hand side of a guard expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lhs {
    /// A dotted path into the entry, e.g. `["current_phase"]` or
    /// `["plan", "phases"]`.
    Path(Vec<String>),
    /// The `.length` of the value at the dotted path, e.g.
    /// `plan.phases.length` → `PathLength(["plan", "phases"])`.
    PathLength(Vec<String>),
}

/// Comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Right-hand side of a guard expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rhs {
    /// A single-quoted string literal, e.g. `'PASS'`.
    Literal(String),
    /// An unquoted integer, e.g. `4`.
    Integer(i64),
    /// A dotted path into the entry (same semantics as `Lhs::Path`), e.g.
    /// `current_phase` or `plan.current`.  Allows path == path comparisons.
    Path(Vec<String>),
    /// The `.length` of a dotted-path value (same semantics as `Lhs::PathLength`),
    /// e.g. `plan.phases.length`.  Used in `current_phase < plan.phases.length`.
    PathLength(Vec<String>),
}

/// A single (non-compound) guard predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub lhs: Lhs,
    pub op: Op,
    pub rhs: Rhs,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Returns true only when `keyword` appears as a standalone word (surrounded by
/// non-alphanumeric/non-underscore boundaries) in `s`.
fn contains_keyword(s: &str, keyword: &str) -> bool {
    let kw = keyword.as_bytes();
    let haystack = s.as_bytes();
    if haystack.len() < kw.len() {
        return false;
    }
    let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    for i in 0..=(haystack.len() - kw.len()) {
        if haystack[i..i + kw.len()] == *kw {
            let before_ok = i == 0 || !is_word_char(haystack[i - 1]);
            let after_ok = i + kw.len() == haystack.len() || !is_word_char(haystack[i + kw.len()]);
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

/// Parse a `guard:` string into an `Expr`.
///
/// Accepted forms:
/// - `dotted.path == 'literal'`
/// - `dotted.path != integer`
/// - `dotted.path.length <= integer`
/// - `dotted.path < other.path.length`   ← RHS may be a path or path.length
/// - `dotted.path == other.path`         ← RHS may be a bare path (path == path)
/// - (etc. for all six Op variants and both Lhs/Rhs combinations)
///
/// Rejections (for clear error messages):
/// - Double-quoted RHS
/// - `&&`, `||`, AND, OR keyword compounds
pub fn parse_guard(input: &str) -> Result<Expr> {
    let s = input.trim();

    // Reject compound-expression tokens early.
    if s.contains("&&") {
        bail!(
            "unsupported token '&&' in guard expression; compound expressions are not supported: {:?}",
            input
        );
    }
    if s.contains("||") {
        bail!(
            "unsupported token '||' in guard expression; compound expressions are not supported: {:?}",
            input
        );
    }
    if contains_keyword(s, "OR") || s.contains(" or ") {
        bail!(
            "unsupported token 'OR' in guard expression; compound expressions are not supported: {:?}",
            input
        );
    }
    if contains_keyword(s, "AND") || s.contains(" and ") {
        bail!(
            "unsupported token 'AND' in guard expression; compound expressions are not supported: {:?}",
            input
        );
    }

    // Identify the operator.  Order matters: check two-char ops before one-char.
    let (lhs_raw, op, rhs_raw) = extract_op(s)?;

    let lhs_raw = lhs_raw.trim();
    let rhs_raw = rhs_raw.trim();

    // Parse LHS — detect `.length` suffix.
    let lhs = parse_lhs(lhs_raw)?;

    // Parse RHS — single-quoted literal OR bare integer.
    let rhs = parse_rhs(rhs_raw, input)?;

    // Length operators require a numeric (integer or path-length) RHS, not a string literal.
    if matches!(lhs, Lhs::PathLength(_)) && matches!(rhs, Rhs::Literal(_)) {
        bail!(
            "length comparison requires an integer RHS, got a string literal: {:?}",
            input
        );
    }

    Ok(Expr { lhs, op, rhs })
}

fn extract_op(s: &str) -> Result<(&str, Op, &str)> {
    // Try two-character operators first to avoid mis-parsing `<=` as `<` followed by `=`.
    let two_char: &[(&str, Op)] = &[
        ("<=", Op::Le),
        (">=", Op::Ge),
        ("!=", Op::Neq),
        ("==", Op::Eq),
    ];
    for (tok, op) in two_char {
        if let Some(pos) = s.find(tok) {
            return Ok((&s[..pos], *op, &s[pos + tok.len()..]));
        }
    }
    // Single-character operators
    let one_char: &[(&str, Op)] = &[("<", Op::Lt), (">", Op::Gt)];
    for (tok, op) in one_char {
        if let Some(pos) = s.find(tok) {
            return Ok((&s[..pos], *op, &s[pos + tok.len()..]));
        }
    }
    bail!(
        "guard expression must contain a comparison operator (==, !=, <, <=, >, >=): {:?}",
        s
    );
}

fn parse_lhs(raw: &str) -> Result<Lhs> {
    // Detect `.length` suffix
    let (path_str, is_length) = if let Some(stripped) = raw.strip_suffix(".length") {
        (stripped, true)
    } else {
        (raw, false)
    };

    // Validate: dotted identifier characters only
    if !path_str
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        bail!(
            "guard LHS must be a dotted identifier path (e.g. 'plan.phases.length'); got: {:?}",
            raw
        );
    }
    if path_str.is_empty() {
        bail!("guard LHS is empty");
    }
    if path_str.starts_with('.') || path_str.ends_with('.') {
        bail!("guard LHS has leading/trailing '.': {:?}", path_str);
    }

    let parts: Vec<String> = path_str.split('.').map(|s| s.to_string()).collect();
    if is_length {
        Ok(Lhs::PathLength(parts))
    } else {
        Ok(Lhs::Path(parts))
    }
}

fn parse_rhs(raw: &str, original: &str) -> Result<Rhs> {
    if raw.starts_with('"') || raw.ends_with('"') {
        bail!(
            "guard RHS must use single quotes, not double quotes: {:?}",
            original
        );
    }
    if raw.starts_with('\'') {
        if !raw.ends_with('\'') || raw.len() < 2 {
            bail!(
                "guard RHS single-quoted literal is not properly closed: {:?}",
                original
            );
        }
        let literal = raw[1..raw.len() - 1].to_string();
        return Ok(Rhs::Literal(literal));
    }
    // Attempt to parse as integer
    if let Ok(n) = raw.parse::<i64>() {
        return Ok(Rhs::Integer(n));
    }
    // Attempt to parse as a dotted-identifier path (possibly ending in .length).
    // This mirrors parse_lhs so that `current_phase < plan.phases.length` works.
    let (path_str, is_length) = if let Some(stripped) = raw.strip_suffix(".length") {
        (stripped, true)
    } else {
        (raw, false)
    };
    if !path_str.is_empty()
        && path_str
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !path_str.starts_with('.')
        && !path_str.ends_with('.')
    {
        let parts: Vec<String> = path_str.split('.').map(|s| s.to_string()).collect();
        if is_length {
            return Ok(Rhs::PathLength(parts));
        } else {
            return Ok(Rhs::Path(parts));
        }
    }
    bail!(
        "guard RHS must be a single-quoted literal, an integer, or a dotted path (optionally ending in .length); got: {:?}",
        raw
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- AC1.3 ----

    #[test]
    fn parse_length_lt() {
        let e = parse_guard("phases.length < 4").unwrap();
        assert_eq!(e.lhs, Lhs::PathLength(vec!["phases".to_string()]));
        assert_eq!(e.op, Op::Lt);
        assert_eq!(e.rhs, Rhs::Integer(4));
    }

    #[test]
    fn parse_current_cycle_le() {
        let e = parse_guard("current_cycle <= 4").unwrap();
        assert_eq!(e.lhs, Lhs::Path(vec!["current_cycle".to_string()]));
        assert_eq!(e.op, Op::Le);
        assert_eq!(e.rhs, Rhs::Integer(4));
    }

    #[test]
    fn parse_eq_literal() {
        let e = parse_guard("status == 'PASS'").unwrap();
        assert_eq!(e.lhs, Lhs::Path(vec!["status".to_string()]));
        assert_eq!(e.op, Op::Eq);
        assert_eq!(e.rhs, Rhs::Literal("PASS".to_string()));
    }

    #[test]
    fn parse_neq_literal() {
        let e = parse_guard("status != 'FAIL'").unwrap();
        assert_eq!(e.op, Op::Neq);
        assert_eq!(e.rhs, Rhs::Literal("FAIL".to_string()));
    }

    #[test]
    fn parse_deep_path_length() {
        let e = parse_guard("plan.phases.length >= 2").unwrap();
        assert_eq!(
            e.lhs,
            Lhs::PathLength(vec!["plan".to_string(), "phases".to_string()])
        );
        assert_eq!(e.op, Op::Ge);
        assert_eq!(e.rhs, Rhs::Integer(2));
    }

    #[test]
    fn reject_or_keyword() {
        let err = parse_guard("a == 'x' OR b == 'y'").unwrap_err();
        assert!(err.to_string().contains("OR"), "err: {err}");
    }

    #[test]
    fn reject_and_symbol() {
        let err = parse_guard("a == 'x' && b == 'y'").unwrap_err();
        assert!(err.to_string().contains("'&&'"), "err: {err}");
    }

    #[test]
    fn reject_or_symbol() {
        let err = parse_guard("a == 'x' || b == 'y'").unwrap_err();
        assert!(err.to_string().contains("'||'"), "err: {err}");
    }

    #[test]
    fn reject_double_quotes() {
        let err = parse_guard("status == \"PASS\"").unwrap_err();
        assert!(err.to_string().contains("double quotes"), "err: {err}");
    }

    #[test]
    fn reject_length_with_string_rhs() {
        let err = parse_guard("phases.length < 'foo'").unwrap_err();
        assert!(
            err.to_string()
                .contains("length comparison requires an integer"),
            "err: {err}"
        );
    }

    #[test]
    fn parse_gt() {
        let e = parse_guard("current_phase > 3").unwrap();
        assert_eq!(e.op, Op::Gt);
        assert_eq!(e.rhs, Rhs::Integer(3));
    }

    #[test]
    fn parse_no_op_errors() {
        let err = parse_guard("current_phase").unwrap_err();
        assert!(
            err.to_string().contains("comparison operator"),
            "err: {err}"
        );
    }

    /// Guard parser must accept enum literals containing "OR"/"AND" as substrings.
    #[test]
    fn parse_accepts_quoted_or_in_literal() {
        let e = parse_guard("region == 'NORTH'").unwrap();
        assert_eq!(e.rhs, Rhs::Literal("NORTH".to_string()));
    }

    #[test]
    fn parse_accepts_quoted_and_in_literal() {
        let e = parse_guard("status == 'BAND'").unwrap();
        assert_eq!(e.rhs, Rhs::Literal("BAND".to_string()));
    }

    // ---- AC1.4: path-vs-path-length RHS (C1 fix) ----

    #[test]
    fn parse_current_phase_lt_plan_phases_length() {
        // The exact form required by AC1.4 and the Phase 7 tasks schema.
        let e = parse_guard("current_phase < plan.phases.length").unwrap();
        assert_eq!(e.lhs, Lhs::Path(vec!["current_phase".to_string()]));
        assert_eq!(e.op, Op::Lt);
        assert_eq!(
            e.rhs,
            Rhs::PathLength(vec!["plan".to_string(), "phases".to_string()])
        );
    }

    #[test]
    fn parse_current_phase_ge_plan_phases_length() {
        let e = parse_guard("current_phase >= plan.phases.length").unwrap();
        assert_eq!(e.lhs, Lhs::Path(vec!["current_phase".to_string()]));
        assert_eq!(e.op, Op::Ge);
        assert_eq!(
            e.rhs,
            Rhs::PathLength(vec!["plan".to_string(), "phases".to_string()])
        );
    }

    #[test]
    fn parse_path_eq_path() {
        // path == path form
        let e = parse_guard("a == b").unwrap();
        assert_eq!(e.lhs, Lhs::Path(vec!["a".to_string()]));
        assert_eq!(e.op, Op::Eq);
        assert_eq!(e.rhs, Rhs::Path(vec!["b".to_string()]));
    }
}
