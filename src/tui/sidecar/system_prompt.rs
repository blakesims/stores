//! Side-car system prompt assembler.
//!
//! Embeds `agents/sidecar/system-prompt.md` via `include_str!` and exposes
//! `system_prompt_for(scope)` which appends a one-line scope header so the
//! receiving Claude Code session knows which brief variant applies.

use super::SidecarScope;

const BASE: &str = include_str!("../../../agents/sidecar/system-prompt.md");

/// Concatenate the base prompt with the scope's variant header. The base
/// already contains every variant block; the header is a clarifying first
/// line that names the active scope.
pub fn system_prompt_for(scope: &SidecarScope) -> String {
    let header = match scope {
        SidecarScope::PerRow { display_id, .. } => {
            format!("# ACTIVE SCOPE: per-row review/triage ({display_id})\n\n")
        }
        SidecarScope::General => {
            "# ACTIVE SCOPE: orchestrator-mode general\n\n".to_string()
        }
        SidecarScope::ObsDraft => "# ACTIVE SCOPE: obs-drafting\n\n".to_string(),
    };
    format!("{header}{BASE}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_per_row_contains_required_substrings() {
        let s = system_prompt_for(&SidecarScope::PerRow {
            display_id: "T999".to_string(),
            fresh: false,
        });
        assert!(s.contains("ACTIVE SCOPE: per-row"), "scope header missing");
        assert!(s.contains("T999"), "display id missing from header");
        assert!(s.contains("stores tasks accept"), "verb vocabulary missing");
        assert!(s.contains("propose"), "discipline keyword 'propose' missing");
        assert!(s.contains("await"), "discipline keyword 'await' missing");
        assert!(s.contains("--approve-token"), "token flag missing");
    }

    #[test]
    fn snapshot_general_contains_required_substrings() {
        let s = system_prompt_for(&SidecarScope::General);
        assert!(s.contains("ACTIVE SCOPE: orchestrator-mode general"));
        assert!(s.contains("stores tasks accept"));
        assert!(s.contains("propose"));
        assert!(s.contains("await"));
        assert!(s.contains("--approve-token"));
    }

    #[test]
    fn snapshot_obs_draft_contains_required_substrings() {
        let s = system_prompt_for(&SidecarScope::ObsDraft);
        assert!(s.contains("ACTIVE SCOPE: obs-drafting"));
        assert!(s.contains("stores tasks accept"));
        assert!(s.contains("propose"));
        assert!(s.contains("await"));
        assert!(s.contains("--approve-token"));
    }
}
