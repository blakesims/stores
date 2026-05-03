use anyhow::{bail, Result};

/// Validate an id_format template.
///
/// Rules:
/// - Must contain exactly one `{:0Nd}` placeholder where N is one or more digits.
/// - Anything else in the string is treated as a literal prefix/suffix.
pub fn validate(template: &str) -> Result<()> {
    // Count occurrences of a valid placeholder.
    // Pattern: {:0<digits>d}
    let count = count_placeholders(template);
    if count == 0 {
        bail!(
            "id_format '{template}' must contain exactly one '{{:0Nd}}' placeholder (e.g. '{{:03d}}')"
        );
    }
    if count > 1 {
        bail!(
            "id_format '{template}' contains {count} placeholders; exactly one is required"
        );
    }

    // Reject any remaining `{` or `}` that are not part of a valid placeholder.
    // A valid placeholder has been counted; we just check there are no unmatched braces.
    // Simple approach: after removing the one valid placeholder, no braces should remain.
    let stripped = strip_first_placeholder(template);
    if stripped.contains('{') || stripped.contains('}') {
        bail!(
            "id_format '{template}' contains unexpected braces outside of the '{{:0Nd}}' placeholder"
        );
    }

    Ok(())
}

/// Count `{:0<digits>d}` placeholders in the template.
fn count_placeholders(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Try to match {:0<digits>d}
            if let Some(end) = s[i..].find('}') {
                let inner = &s[i + 1..i + end];
                if is_valid_placeholder_inner(inner) {
                    count += 1;
                    i += end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    count
}

fn is_valid_placeholder_inner(inner: &str) -> bool {
    // Must match `:0<digits>d`
    if !inner.starts_with(":0") {
        return false;
    }
    let rest = &inner[2..]; // after ":0"
    if rest.is_empty() {
        return false;
    }
    // rest must be digits followed by 'd'
    if !rest.ends_with('d') {
        return false;
    }
    let digits = &rest[..rest.len() - 1];
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Remove the first valid placeholder from the template string.
fn strip_first_placeholder(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = s[i..].find('}') {
                let inner = &s[i + 1..i + end];
                if is_valid_placeholder_inner(inner) {
                    let mut result = s[..i].to_string();
                    result.push_str(&s[i + end + 1..]);
                    return result;
                }
            }
        }
        i += 1;
    }
    s.to_string()
}

/// Render a display_id from an `id_format` template and a primary key.
///
/// Example: `render("L{:03d}", 1)` → `"L001"`.
pub fn render(template: &str, pk: i64) -> String {
    // Find the placeholder position and width specifier.
    let start = template.find('{').expect("template must have placeholder");
    let end = template.find('}').expect("template must have closing brace");
    // inner is ":0Nd"
    let inner = &template[start + 1..end];
    // inner starts with ":0" — width follows
    let width_str = &inner[2..inner.len() - 1]; // strip ":0" and "d"
    let width: usize = width_str.parse().unwrap_or(1);
    let prefix = &template[..start];
    let suffix = &template[end + 1..];
    format!("{prefix}{pk:0>width$}{suffix}", pk = pk, width = width)
}

/// Parse a display_id string against an `id_format` template, returning the
/// numeric portion as `i64`.
///
/// The supplied id must:
/// - exactly match the template's literal prefix (case-sensitive)
/// - exactly match the template's literal suffix (case-sensitive)
/// - contain only ASCII digits in the placeholder slot
/// - have a digit count >= the template's minimum width (zero-pad width).
///   Longer numeric portions are allowed (so `T1000` parses against `T{:03d}`).
///
/// Example: `parse("T{:03d}", "T013")` → `Ok(13)`.
/// Example: `parse("T{:03d}", "Tabc")` → `Err(...)`.
pub fn parse(template: &str, supplied: &str) -> Result<i64> {
    let start = template
        .find('{')
        .ok_or_else(|| anyhow::anyhow!("id_format '{template}' missing placeholder"))?;
    let end = template
        .find('}')
        .ok_or_else(|| anyhow::anyhow!("id_format '{template}' missing closing brace"))?;
    let inner = &template[start + 1..end];
    if !is_valid_placeholder_inner(inner) {
        bail!("id_format '{template}' has invalid placeholder");
    }
    let width_str = &inner[2..inner.len() - 1];
    let min_width: usize = width_str.parse().unwrap_or(1);
    let prefix = &template[..start];
    let suffix = &template[end + 1..];

    if !supplied.starts_with(prefix) || !supplied.ends_with(suffix) {
        bail!(
            "id '{supplied}' does not match id_format '{template}' \
             (expected prefix '{prefix}', suffix '{suffix}')"
        );
    }
    if supplied.len() < prefix.len() + suffix.len() {
        bail!("id '{supplied}' too short for id_format '{template}'");
    }
    let digits = &supplied[prefix.len()..supplied.len() - suffix.len()];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!(
            "id '{supplied}' does not match id_format '{template}' \
             (numeric portion must be ASCII digits)"
        );
    }
    if digits.len() < min_width {
        bail!(
            "id '{supplied}' does not match id_format '{template}' \
             (numeric portion must be at least {min_width} digits, got {})",
            digits.len()
        );
    }
    digits
        .parse::<i64>()
        .map_err(|e| anyhow::anyhow!("id '{supplied}' numeric portion overflows i64: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_formats() {
        validate("L{:03d}").unwrap();
        validate("I{:04d}").unwrap();
        validate("{:01d}").unwrap();
        validate("PREFIX-{:010d}-SUFFIX").unwrap();
    }

    #[test]
    fn missing_placeholder_errors() {
        let err = validate("NOPLACEHOLDER").unwrap_err();
        assert!(err.to_string().contains("exactly one"));
    }

    #[test]
    fn too_many_placeholders_errors() {
        let err = validate("{:03d}{:02d}").unwrap_err();
        assert!(err.to_string().contains("2 placeholders"));
    }

    #[test]
    fn invalid_placeholder_form() {
        // {:3d} without the leading zero is not accepted
        let err = validate("X{:3d}").unwrap_err();
        assert!(err.to_string().contains("exactly one") || err.to_string().contains("{:0Nd}"));
    }

    #[test]
    fn l003_format() {
        validate("L{:03d}").unwrap();
    }

    // ---- L001: parse() round-trip and rejection cases ----

    #[test]
    fn parse_accepts_padded_id() {
        assert_eq!(parse("T{:03d}", "T013").unwrap(), 13);
        assert_eq!(parse("T{:03d}", "T001").unwrap(), 1);
    }

    #[test]
    fn parse_accepts_id_wider_than_min_width() {
        // Template requires at least 3 digits, but T1000 is fine (4 digits).
        assert_eq!(parse("T{:03d}", "T1000").unwrap(), 1000);
    }

    #[test]
    fn parse_rejects_non_digit_body() {
        let err = parse("T{:03d}", "Tabc").unwrap_err();
        assert!(err.to_string().contains("ASCII digits"));
    }

    #[test]
    fn parse_rejects_wrong_prefix() {
        let err = parse("T{:03d}", "X013").unwrap_err();
        assert!(err.to_string().contains("prefix"));
    }

    #[test]
    fn parse_rejects_too_few_digits() {
        let err = parse("T{:03d}", "T13").unwrap_err();
        assert!(err.to_string().contains("at least 3 digits"));
    }

    #[test]
    fn parse_round_trips_with_render() {
        let rendered = render("T{:03d}", 42);
        assert_eq!(rendered, "T042");
        assert_eq!(parse("T{:03d}", &rendered).unwrap(), 42);
    }
}
