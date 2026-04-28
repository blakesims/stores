/// Schema-Aligned Parser (SAP) — extracts a JSON envelope from prose text.
///
/// When the claude CLI's `--json-schema` structured-output path doesn't fire
/// (e.g. a multi-turn tool-using agent ends in prose rather than the structured-
/// output tool), `result.structured_output` is `null`. SAP recovers the envelope
/// by scanning the human-readable `result.result` text for a JSON object that
/// matches the expected schema.
///
/// # Algorithm
///
/// 1. Strip markdown code fences (` ```json ... ``` ` and bare ` ``` ... ``` `).
/// 2. Walk the cleaned text for balanced `{...}` candidate objects.
/// 3. Return the first candidate that:
///    - Parses as valid JSON, AND
///    - When `schema.is_some()`, also passes `jsonschema` validation against the
///      supplied schema.
/// 4. Return `None` if no candidate matches.
///
/// # Industry context
///
/// This is analogous to BAML's "Schema-Aligned Parsing" and the extraction layer
/// in Pydantic AI / Instructor. See T004 Plan Revisions for rationale.
use serde_json::Value;

/// Extract a JSON envelope from prose text with optional schema validation.
///
/// # Parameters
/// - `text`: the raw prose from `result.result` (may contain markdown fences,
///   surrounding commentary, etc.).
/// - `schema`: optional JSON Schema value to validate candidates against. When
///   `None`, the first parseable JSON object is returned (no validation).
///
/// # Returns
/// The first candidate JSON object that parses (and validates, when schema is
/// supplied). `None` if no candidate matches.
pub fn extract_envelope_from_text(text: &str, schema: Option<&Value>) -> Option<Value> {
    let cleaned = strip_markdown_fences(text);
    let candidates = find_json_candidates(&cleaned);

    for candidate in candidates {
        let Ok(value) = serde_json::from_str::<Value>(&candidate) else {
            continue;
        };
        // Must be an object (not array, number, string, etc.)
        if !value.is_object() {
            continue;
        }
        if let Some(schema_val) = schema {
            if validate_against_schema(&value, schema_val) {
                return Some(value);
            }
            // Candidate didn't pass schema — try next.
            continue;
        }
        // No schema — return first parseable object.
        return Some(value);
    }

    None
}

/// Strip markdown code fences from text.
///
/// Handles:
/// - ` ```json\n...\n``` ` (fenced with language tag)
/// - ` ```\n...\n``` ` (bare fence)
///
/// The content between the outermost fence pair is extracted and the fences
/// themselves are removed. Nested fences are not supported (not needed for
/// single-envelope use case).
fn strip_markdown_fences(text: &str) -> String {
    // Find the first opening fence (```json or ```)
    // and the matching closing fence (```).
    let mut result = text.to_string();

    // Repeatedly strip fence pairs until none remain.
    loop {
        let fence_start = result.find("```");
        if fence_start.is_none() {
            break;
        }
        let start = fence_start.unwrap();
        // Find end of the opening fence line.
        let after_fence = &result[start + 3..];
        let newline_pos = after_fence.find('\n');
        if newline_pos.is_none() {
            break;
        }
        let content_start = start + 3 + newline_pos.unwrap() + 1;
        // Find the closing ``` on its own line.
        let content = &result[content_start..];
        // Look for ``` at line start (may have surrounding whitespace).
        let close_pos = find_closing_fence(content);
        if let Some(cp) = close_pos {
            let inner = &content[..cp];
            // Replace the full fence block with just the inner content.
            let before = &result[..start];
            let after_close = content_start + cp;
            // skip the closing ``` line
            let close_line_end = result[after_close..]
                .find('\n')
                .map(|p| after_close + p + 1)
                .unwrap_or(result.len());
            result = format!("{}{}{}", before, inner, &result[close_line_end..]);
        } else {
            break;
        }
    }

    result
}

/// Find the position of a closing ``` fence in `text`.
///
/// Scans for ``` at the start of a line (possibly preceded by whitespace).
/// Returns the byte offset of the ``` in `text`, or `None` if not found.
fn find_closing_fence(text: &str) -> Option<usize> {
    let mut pos = 0;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            return Some(pos);
        }
        pos += line.len() + 1; // +1 for the newline
    }
    None
}

/// Walk `text` and extract all balanced `{...}` substrings as candidates.
///
/// Uses a simple depth counter — does not handle JSON strings containing `{`
/// or `}`, but this is sufficient for the planner/executor envelope shapes
/// which don't embed raw curly braces in string values at top level.
fn find_json_candidates(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut candidates = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Track depth to find the matching close brace.
            let start = i;
            let mut depth = 0;
            let mut in_string = false;
            let mut escape_next = false;
            let mut j = i;

            while j < bytes.len() {
                let b = bytes[j];
                if escape_next {
                    escape_next = false;
                } else if in_string {
                    if b == b'\\' {
                        escape_next = true;
                    } else if b == b'"' {
                        in_string = false;
                    }
                } else {
                    match b {
                        b'"' => in_string = true,
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                candidates.push(text[start..=j].to_string());
                                i = j;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
        }
        i += 1;
    }

    candidates
}

/// Validate a JSON value against a schema using the `jsonschema` crate.
///
/// Returns `true` if valid, `false` if validation fails or the schema cannot
/// be compiled.
///
/// When the `runner-claude-code` feature is disabled (and thus `jsonschema` is
/// not available as a runtime dep), this function always returns `false` so
/// schema-gated candidates are skipped. The SAP tests that exercise schema
/// validation only run under `runner-claude-code`.
#[cfg(feature = "runner-claude-code")]
fn validate_against_schema(value: &Value, schema: &Value) -> bool {
    let compiled = match jsonschema::JSONSchema::compile(schema) {
        Ok(v) => v,
        Err(_) => return false,
    };
    compiled.is_valid(value)
}

#[cfg(not(feature = "runner-claude-code"))]
fn validate_against_schema(_value: &Value, _schema: &Value) -> bool {
    // jsonschema runtime dep is only available under runner-claude-code.
    // Without it, schema validation is unavailable; skip this candidate.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC1.10(a): plain JSON object is returned as-is.
    #[test]
    fn sap_parses_plain_json() {
        let input = r#"{"role":"planner","phases":[]}"#;
        let result = extract_envelope_from_text(input, None);
        assert!(result.is_some(), "expected Some for plain JSON");
        let v = result.unwrap();
        assert_eq!(v["role"].as_str(), Some("planner"));
    }

    /// AC1.10(b): JSON inside markdown fences is extracted.
    #[test]
    fn sap_strips_markdown_fences() {
        let input = "## Header\n\n```json\n{\"role\":\"planner\",\"phases\":[]}\n```\n\n## Trailer";
        let result = extract_envelope_from_text(input, None);
        assert!(result.is_some(), "expected Some after stripping fences");
        let v = result.unwrap();
        assert_eq!(v["role"].as_str(), Some("planner"));
    }

    /// AC1.10(c): JSON embedded in surrounding prose is extracted.
    #[test]
    fn sap_handles_prose_with_json() {
        let input = r#"Some commentary text. {"role":"planner","phases":[]}. More text."#;
        let result = extract_envelope_from_text(input, None);
        assert!(result.is_some(), "expected Some for prose-wrapped JSON");
        let v = result.unwrap();
        assert_eq!(v["role"].as_str(), Some("planner"));
    }

    /// AC1.10(d): first candidate fails JSON parse; second is valid; second is returned.
    #[test]
    fn sap_skips_invalid_first_candidate() {
        // First { block is malformed JSON; second is valid.
        let input = "{ invalid json here } and then {\"role\":\"planner\",\"phases\":[]}";
        let result = extract_envelope_from_text(input, None);
        assert!(result.is_some(), "expected Some when second candidate is valid");
        let v = result.unwrap();
        assert_eq!(v["role"].as_str(), Some("planner"));
    }

    /// AC1.10(e): two valid JSON objects; only second matches schema; second returned.
    #[test]
    fn sap_validates_against_schema() {
        let schema: Value = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "required": ["role", "phases"],
            "properties": {
                "role": {"const": "planner"},
                "phases": {"type": "array"}
            },
            "additionalProperties": false
        });

        // First candidate is valid JSON but doesn't match schema (wrong role).
        // Second candidate matches schema.
        let input = r#"{"role":"not-planner"} and then {"role":"planner","phases":[]}"#;
        let result = extract_envelope_from_text(input, Some(&schema));
        assert!(result.is_some(), "expected Some when second candidate passes schema");
        let v = result.unwrap();
        assert_eq!(v["role"].as_str(), Some("planner"));
    }

    /// AC1.10(f): no JSON in input; returns None.
    #[test]
    fn sap_returns_none_on_no_match() {
        let input = "Just prose, no JSON here.";
        let result = extract_envelope_from_text(input, None);
        assert!(result.is_none(), "expected None for prose-only input");
    }

    /// AC1.10(g): realistic haiku transcript recovery.
    /// Reads the result.result field from the staged fixture, passes it through
    /// extract_envelope_from_text with the planner schema, and asserts recovery.
    #[test]
    fn sap_recovers_planner_envelope_from_haiku_transcript() {
        use std::io::BufRead;

        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/agent_outputs/planner-haiku-multiturn.jsonl"
        );
        let file = std::fs::File::open(fixture_path).expect("fixture file");
        let reader = std::io::BufReader::new(file);

        // Find the last result event.
        let mut last_result_text: Option<String> = None;
        for line in reader.lines() {
            let line = line.expect("read line");
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&trimmed) else {
                continue;
            };
            if v.get("type").and_then(|t| t.as_str()) == Some("result") {
                last_result_text = v
                    .get("result")
                    .and_then(|r| r.as_str())
                    .map(|s| s.to_string());
            }
        }

        let text = last_result_text.expect("fixture must have a result event with result text");

        // Load planner schema.
        let schema_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/agents/schemas/planner.schema.json"
        );
        let schema_text = std::fs::read_to_string(schema_path).expect("planner schema");
        let schema: Value = serde_json::from_str(&schema_text).expect("parse schema");

        let result = extract_envelope_from_text(&text, Some(&schema));
        assert!(
            result.is_some(),
            "SAP must recover planner envelope from haiku transcript"
        );
        let v = result.unwrap();
        assert_eq!(
            v["role"].as_str(),
            Some("planner"),
            "recovered envelope must have role=planner"
        );
    }
}
