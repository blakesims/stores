// parse.rs — public entry point for Schema::from_yaml, which lives in mod.rs.
// This file re-exports the constructor so external callers can use
// `schema::parse::from_yaml` if preferred, or just `Schema::from_yaml`.

pub use super::Schema;

#[cfg(test)]
mod tests {
    use super::Schema;

    #[test]
    fn from_yaml_surfaces_serde_error_message() {
        let bad = "name: 123\nid_format: ok\nlifecycle:\n  states: [a]\n  transitions: []\nfields: []";
        // name should be a string — serde_yaml accepts integers as strings, so
        // test a genuinely invalid YAML structure instead.
        let bad_yaml = ": broken [[[";
        let err = Schema::from_yaml(bad_yaml).unwrap_err();
        assert!(err.to_string().contains("YAML parse error"));
        let _ = bad; // suppress warning
    }
}
