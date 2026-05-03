//! Autonomous flow engine: agent registry, policy layer, predicate evaluator.
//!
//! Phase 2 scaffolding: pure config-file parsing and predicate evaluation.
//! No daemon yet.

pub mod agents_yaml;
pub mod policies_yaml;
pub mod predicate;

pub use agents_yaml::{AgentEntry, AgentsYaml, BackoffKind, RetryPolicy, Subscription};
pub use policies_yaml::{decide, Action, Decision, PoliciesYaml, PolicyEntry, TransitionRef};
pub use predicate::{eval, PredicateExpr};
