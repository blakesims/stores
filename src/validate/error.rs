/// Kinds of validation rule that can be violated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleKind {
    Required,
    RequiredWhen {
        expr: String,
    },
    Enum,
    /// An element of a List(Text) field is not in the declared list_enum set.
    ListEnum,
    Pattern {
        pattern: String,
    },
    Actor,
    /// The field requires a specific JSON shape but received an unparseable or
    /// incorrect-type value.  `expected` names the required form (e.g. "JSON array"
    /// for list_record/list_fk, "valid JSON" for Json fields).
    InvalidJson {
        expected: String,
    },
    /// An array field exceeded its declared `max_items` bound, or a string field
    /// exceeded its declared `max_length` bound.  `limit` is the bound; `actual`
    /// is what was supplied.
    BoundsExceeded {
        limit: usize,
        actual: usize,
    },
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
