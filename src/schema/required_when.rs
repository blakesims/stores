use anyhow::{bail, Result};

/// Returns true only when `keyword` appears as a standalone word (surrounded by
/// non-alphanumeric/non-underscore boundaries) in `s`. This prevents false
/// positives on enum literals like 'NORTH' (contains "OR") or 'BAND' (contains "AND").
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

/// Minimal condition AST. Only `dotted.path == 'literal'` is supported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub lhs_path: Vec<String>,
    pub rhs_literal: String,
}

/// Parse a `required_when` string.
///
/// Accepted form: `dotted.path == 'literal'`
/// - Single-quoted RHS only.
/// - Whitespace around `==` is trimmed.
/// - Rejects `!=`, `&&`, `||`, double-quoted RHS, and anything else.
pub fn parse(input: &str) -> Result<Expr> {
    let s = input.trim();

    // Reject obvious unsupported tokens early for clear messages.
    if s.contains("!=") {
        bail!(
            "unsupported token '!=' in required_when expression; only '==' is supported: {:?}",
            input
        );
    }
    if s.contains("&&") {
        bail!(
            "unsupported token '&&' in required_when expression; compound expressions are not supported: {:?}",
            input
        );
    }
    if s.contains("||") {
        bail!(
            "unsupported token '||' in required_when expression; compound expressions are not supported: {:?}",
            input
        );
    }
    // Only reject OR/AND when they appear as standalone keywords (word boundaries on both sides).
    // Naive s.contains("OR") would false-positive on enum values like 'NORTH'.
    if contains_keyword(s, "OR") || s.contains(" or ") {
        bail!(
            "unsupported token 'OR' in required_when expression; compound expressions are not supported: {:?}",
            input
        );
    }
    if contains_keyword(s, "AND") || s.contains(" and ") {
        bail!(
            "unsupported token 'AND' in required_when expression; compound expressions are not supported: {:?}",
            input
        );
    }

    // Split on '=='
    let parts: Vec<&str> = s.splitn(2, "==").collect();
    if parts.len() != 2 {
        bail!(
            "required_when expression must contain '=='; got: {:?}",
            input
        );
    }

    let lhs = parts[0].trim();
    let rhs = parts[1].trim();

    // Validate LHS: only dotted identifier path
    if !lhs
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        bail!(
            "required_when LHS must be a dotted identifier path (e.g. 'triage.verdict'); got: {:?}",
            lhs
        );
    }
    if lhs.is_empty() {
        bail!("required_when LHS is empty in: {:?}", input);
    }
    // Must not start or end with '.'
    if lhs.starts_with('.') || lhs.ends_with('.') {
        bail!("required_when LHS has leading/trailing '.': {:?}", lhs);
    }

    // Validate RHS: must be single-quoted
    if rhs.starts_with('"') || rhs.ends_with('"') {
        bail!(
            "required_when RHS must use single quotes, not double quotes: {:?}",
            rhs
        );
    }
    if !rhs.starts_with('\'') || !rhs.ends_with('\'') {
        bail!(
            "required_when RHS must be a single-quoted literal (e.g. \\'T3\\'); got: {:?}",
            rhs
        );
    }

    let literal = rhs[1..rhs.len() - 1].to_string();

    let lhs_path: Vec<String> = lhs.split('.').map(|s| s.to_string()).collect();

    Ok(Expr { lhs_path, rhs_literal: literal })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let e = parse("triage.verdict == 'T3'").unwrap();
        assert_eq!(e.lhs_path, vec!["triage", "verdict"]);
        assert_eq!(e.rhs_literal, "T3");
    }

    #[test]
    fn parse_with_extra_whitespace() {
        let e = parse("  triage.verdict  ==  'T3'  ").unwrap();
        assert_eq!(e.lhs_path, vec!["triage", "verdict"]);
        assert_eq!(e.rhs_literal, "T3");
    }

    #[test]
    fn parse_single_segment() {
        let e = parse("status == 'active'").unwrap();
        assert_eq!(e.lhs_path, vec!["status"]);
        assert_eq!(e.rhs_literal, "active");
    }

    #[test]
    fn reject_not_equal() {
        let err = parse("a != b").unwrap_err();
        assert!(err.to_string().contains("'!='"));
    }

    #[test]
    fn reject_or_keyword() {
        let err = parse("a == 'x' OR b == 'y'").unwrap_err();
        assert!(err.to_string().contains("OR"));
    }

    /// M1 regression: enum literal containing "OR" as a substring (e.g. 'NORTH')
    /// must NOT be rejected as a compound-expression keyword.
    #[test]
    fn parse_accepts_quoted_or_in_literal() {
        let e = parse("region == 'NORTH'").unwrap();
        assert_eq!(e.lhs_path, vec!["region"]);
        assert_eq!(e.rhs_literal, "NORTH");
    }

    /// M1 regression: 'BAND' contains "AND" but must parse cleanly.
    #[test]
    fn parse_accepts_quoted_and_in_literal() {
        let e = parse("type == 'BAND'").unwrap();
        assert_eq!(e.lhs_path, vec!["type"]);
        assert_eq!(e.rhs_literal, "BAND");
    }

    #[test]
    fn reject_and_symbol() {
        let err = parse("a == 'x' && b == 'y'").unwrap_err();
        assert!(err.to_string().contains("'&&'"));
    }

    #[test]
    fn reject_double_quotes_rhs() {
        let err = parse("a == \"x\"").unwrap_err();
        assert!(err.to_string().contains("double quotes"));
    }

    #[test]
    fn reject_unquoted_rhs() {
        let err = parse("a == x").unwrap_err();
        assert!(err.to_string().contains("single-quoted"));
    }
}
