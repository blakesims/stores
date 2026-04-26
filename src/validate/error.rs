/// Kinds of validation rule that can be violated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleKind {
    Required,
    RequiredWhen { expr: String },
    Enum,
    Pattern { pattern: String },
    Actor,
}

/// A single validation violation.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Dot-joined path to the offending field, e.g. `["contract", "done_when"]`.
    pub field_path: Vec<String>,
    pub rule: RuleKind,
    pub message: String,
}

impl ValidationError {
    pub fn field_dot(&self) -> String {
        self.field_path.join(".")
    }
}

/// Pretty-print a slice of errors as a bullet list sorted by field path.
pub fn pretty_print(errors: &[ValidationError]) -> String {
    let mut sorted = errors.to_vec();
    sorted.sort_by(|a, b| a.field_path.cmp(&b.field_path));
    sorted
        .iter()
        .map(|e| format!("- {}: {}", e.field_dot(), e.message))
        .collect::<Vec<_>>()
        .join("\n")
}
