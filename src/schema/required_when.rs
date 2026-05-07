use anyhow::{bail, Result};

/// Returns true only when `keyword` appears as a standalone word outside quoted
/// literals. This prevents false positives on enum literals like 'NORTH'.
fn find_keyword_outside_quotes(s: &str, keyword: &str) -> Option<usize> {
    let kw = keyword.as_bytes();
    let haystack = s.as_bytes();
    if haystack.len() < kw.len() {
        return None;
    }
    let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut in_quote = false;
    for i in 0..=(haystack.len() - kw.len()) {
        if haystack[i] == b'\'' {
            in_quote = !in_quote;
        }
        if in_quote {
            continue;
        }
        if haystack[i..i + kw.len()] == *kw {
            let before_ok = i == 0 || !is_word_char(haystack[i - 1]);
            let after_ok = i + kw.len() == haystack.len() || !is_word_char(haystack[i + kw.len()]);
            if before_ok && after_ok {
                return Some(i);
            }
        }
    }
    None
}

fn contains_keyword_outside_quotes(s: &str, keyword: &str) -> bool {
    find_keyword_outside_quotes(s, keyword).is_some()
}

/// Minimal condition AST. Supports `dotted.path == 'literal'` and
/// `dotted.path IN ['literal1', 'literal2', ...]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub lhs_path: Vec<String>,
    /// Back-compat accessor for single-literal callers. For `IN`, this is the
    /// first accepted literal; use `rhs_literals` for evaluation.
    pub rhs_literal: String,
    pub rhs_literals: Vec<String>,
}

impl Expr {
    pub fn matches_literal(&self, value: &str) -> bool {
        self.rhs_literals.iter().any(|lit| lit == value)
    }

    pub fn condition_string(&self) -> String {
        if self.rhs_literals.len() == 1 {
            format!("{} == '{}'", self.lhs_path.join("."), self.rhs_literal)
        } else {
            let literals = self
                .rhs_literals
                .iter()
                .map(|lit| format!("'{lit}'"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} IN [{}]", self.lhs_path.join("."), literals)
        }
    }
}

/// Parse a `required_when` string.
///
/// Accepted forms:
/// - `dotted.path == 'literal'`
/// - `dotted.path IN ['literal1', 'literal2', ...]`
pub fn parse(input: &str) -> Result<Expr> {
    let s = input.trim();

    if s.contains("!=") {
        bail!(
            "unsupported token '!=' in required_when expression; only '==' and 'IN [...]' are supported: {:?}",
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
    if contains_keyword_outside_quotes(s, "OR") || s.contains(" or ") {
        bail!(
            "unsupported token 'OR' in required_when expression; compound expressions are not supported: {:?}",
            input
        );
    }
    if contains_keyword_outside_quotes(s, "AND") || s.contains(" and ") {
        bail!(
            "unsupported token 'AND' in required_when expression; compound expressions are not supported: {:?}",
            input
        );
    }

    if let Some(idx) = find_keyword_outside_quotes(s, "IN") {
        return parse_in(input, s, idx);
    }
    parse_eq(input, s)
}

fn parse_eq(input: &str, s: &str) -> Result<Expr> {
    let parts: Vec<&str> = s.splitn(2, "==").collect();
    if parts.len() != 2 {
        bail!(
            "required_when expression must contain '==' or 'IN [...]'; got: {:?}",
            input
        );
    }

    let lhs_path = parse_lhs(parts[0].trim())?;
    let literal = parse_single_quoted_literal(parts[1].trim())?;
    Ok(Expr {
        lhs_path,
        rhs_literal: literal.clone(),
        rhs_literals: vec![literal],
    })
}

fn parse_in(input: &str, s: &str, idx: usize) -> Result<Expr> {
    let lhs_path = parse_lhs(s[..idx].trim())?;
    let rhs = s[idx + "IN".len()..].trim();
    if !rhs.starts_with('[') || !rhs.ends_with(']') {
        bail!(
            "required_when IN RHS must be a bracketed list (e.g. ['T2', 'T3']); got: {:?}",
            rhs
        );
    }
    let inner = rhs[1..rhs.len() - 1].trim();
    if inner.is_empty() {
        bail!(
            "required_when IN list must contain at least one literal: {:?}",
            input
        );
    }

    let mut literals = Vec::new();
    for part in inner.split(',') {
        let literal = parse_single_quoted_literal(part.trim())?;
        if literal.is_empty() {
            bail!(
                "required_when IN list contains an empty literal: {:?}",
                input
            );
        }
        literals.push(literal);
    }
    Ok(Expr {
        lhs_path,
        rhs_literal: literals[0].clone(),
        rhs_literals: literals,
    })
}

fn parse_lhs(lhs: &str) -> Result<Vec<String>> {
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
        bail!("required_when LHS is empty");
    }
    if lhs.starts_with('.') || lhs.ends_with('.') {
        bail!("required_when LHS has leading/trailing '.': {:?}", lhs);
    }
    if lhs.split('.').any(str::is_empty) {
        bail!(
            "required_when LHS contains an empty path segment: {:?}",
            lhs
        );
    }
    Ok(lhs.split('.').map(|s| s.to_string()).collect())
}

fn parse_single_quoted_literal(rhs: &str) -> Result<String> {
    if rhs.starts_with('"') || rhs.ends_with('"') {
        bail!(
            "required_when RHS must use single quotes, not double quotes: {:?}",
            rhs
        );
    }
    if !rhs.starts_with('\'') || !rhs.ends_with('\'') || rhs.len() < 2 {
        bail!(
            "required_when RHS must be a single-quoted literal (e.g. \'T3\'); got: {:?}",
            rhs
        );
    }
    Ok(rhs[1..rhs.len() - 1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let e = parse("triage.verdict == 'T3'").unwrap();
        assert_eq!(e.lhs_path, vec!["triage", "verdict"]);
        assert_eq!(e.rhs_literal, "T3");
        assert_eq!(e.rhs_literals, vec!["T3"]);
        assert_eq!(e.condition_string(), "triage.verdict == 'T3'");
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
    fn parse_in_two_values() {
        let e = parse("decision IN ['normal_observation', 'arch_review_candidate']").unwrap();
        assert_eq!(e.lhs_path, vec!["decision"]);
        assert_eq!(e.rhs_literal, "normal_observation");
        assert_eq!(
            e.rhs_literals,
            vec!["normal_observation", "arch_review_candidate"]
        );
        assert_eq!(
            e.condition_string(),
            "decision IN ['normal_observation', 'arch_review_candidate']"
        );
        assert!(e.matches_literal("arch_review_candidate"));
        assert!(!e.matches_literal("routed"));
    }

    #[test]
    fn parse_in_three_values_with_dotted_path() {
        let e = parse("route.decision IN ['duplicate', 'needs_info', 'reject_noise']").unwrap();
        assert_eq!(e.lhs_path, vec!["route", "decision"]);
        assert!(e.matches_literal("needs_info"));
        assert!(!e.matches_literal("fast_track"));
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

    #[test]
    fn parse_accepts_quoted_or_in_literal() {
        let e = parse("region == 'NORTH'").unwrap();
        assert_eq!(e.rhs_literal, "NORTH");
    }

    #[test]
    fn parse_accepts_quoted_and_in_literal() {
        let e = parse("type == 'BAND'").unwrap();
        assert_eq!(e.rhs_literal, "BAND");
    }

    #[test]
    fn parse_accepts_quoted_in_literal() {
        let e = parse("word == 'IN'").unwrap();
        assert_eq!(e.rhs_literal, "IN");
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

    #[test]
    fn reject_in_without_brackets() {
        let err = parse("decision IN 'normal_observation'").unwrap_err();
        assert!(err.to_string().contains("bracketed list"));
    }

    #[test]
    fn reject_in_unquoted_member() {
        let err = parse("decision IN ['normal_observation', arch_review_candidate]").unwrap_err();
        assert!(err.to_string().contains("single-quoted"));
    }
}
