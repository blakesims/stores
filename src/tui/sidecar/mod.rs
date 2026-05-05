//! Side-car spawn primitives (T028 P4).
//!
//! Defines the scope/session newtypes shared by the priming pipeline,
//! system-prompt assembler, and on-disk session store. The actual spawn
//! logic (terminal hand-off via PTY) lives in P5.

pub mod obs_draft;
pub mod session;
pub mod spawn;
pub mod system_prompt;

/// Which side-car flavor was launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarScope {
    /// Per-row review/triage. `fresh: true` skips resume-by-mtime.
    PerRow { display_id: String, fresh: bool },
    /// Orchestrator-mode general thinking partner.
    General,
    /// Drafting a new observation against operator friction.
    ObsDraft,
}

impl SidecarScope {
    /// Stable filesystem-safe key used in `.stores/runs/sidecar/{key}-{id}.jsonl`
    /// resumability lookups.
    pub fn scope_key(&self) -> String {
        match self {
            SidecarScope::PerRow { display_id, .. } => format!("per-row-{display_id}"),
            SidecarScope::General => "general".to_string(),
            SidecarScope::ObsDraft => "obs-draft".to_string(),
        }
    }

    /// Short tag used inside snapshot filenames + system-prompt headers.
    pub fn tag(&self) -> &'static str {
        match self {
            SidecarScope::PerRow { .. } => "per-row",
            SidecarScope::General => "general",
            SidecarScope::ObsDraft => "obs-draft",
        }
    }
}

/// UUID v4 string identifying one side-car session's JSONL transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
