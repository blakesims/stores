//! Autonomous flow engine: agent registry, policy layer, predicate evaluator.
//!
//! Phase 2 scaffolding: pure config-file parsing and predicate evaluation.
//! No daemon yet.

pub mod agents_yaml;
pub mod builtins;
pub mod config;
pub mod ntfy;
pub mod policies_yaml;
pub mod predicate;

pub use agents_yaml::{AgentEntry, AgentsYaml, BackoffKind, RetryPolicy, Subscription};
pub use config::{NtfyCfg, StoresConfig};
pub use ntfy::{
    install_notifier, notify_with_backend, notify_with_path, CurlNotifier, MockNotifier,
    NotifierBackend, NotifyEvent,
};
pub use policies_yaml::{decide, Action, Decision, PoliciesYaml, PolicyEntry, TransitionRef};
pub use predicate::{eval, PredicateExpr};
