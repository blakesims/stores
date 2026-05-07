use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Human,
    AiAutonomous,
    AiWithHuman,
    /// Engine-only actor. Set programmatically by the workflow engine; never
    /// resolved from `--invoker` or from the environment.
    Framework,
}

impl Actor {
    /// Detect whether this process is running inside an AI runtime.
    ///
    /// Non-empty `STORES_AI_RUNTIME` and non-empty `CLAUDECODE` are AI-runtime
    /// signals; empty or unset values are ignored.
    pub fn ai_runtime_detected() -> bool {
        ["STORES_AI_RUNTIME", "CLAUDECODE"]
            .iter()
            .any(|name| std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false))
    }

    /// Read actor from environment: AI-runtime signals default to AI-autonomous.
    /// Never returns `Framework` — that is set only programmatically by the engine.
    pub fn from_env() -> Actor {
        if Self::ai_runtime_detected() {
            Actor::AiAutonomous
        } else {
            Actor::Human
        }
    }
}

/// Resolved invoker plus the validated approve-token bit.
///
/// `token_valid` is set to `true` only when the CLI was passed
/// `--approve-token <T>` and `<T>` matched the on-disk hash via constant-time
/// compare. Phase 3 will consume this bit to relax `actor: human` checks for
/// `ai_with_human` writes; Phase 2 only plumbs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvokerCtx {
    pub actor: Actor,
    pub token_valid: bool,
}

impl InvokerCtx {
    /// Construct a context with no approve-token (the common case).
    pub fn bare(actor: Actor) -> Self {
        Self {
            actor,
            token_valid: false,
        }
    }
}

impl From<Actor> for InvokerCtx {
    fn from(actor: Actor) -> Self {
        Self::bare(actor)
    }
}

impl fmt::Display for InvokerCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Display matches the underlying actor so log/audit strings stay stable.
        write!(f, "{}", self.actor)
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::Human => write!(f, "human"),
            Actor::AiAutonomous => write!(f, "ai_autonomous"),
            Actor::AiWithHuman => write!(f, "ai_with_human"),
            Actor::Framework => write!(f, "framework"),
        }
    }
}

/// Deserialize actor from YAML strings like "human", "ai_autonomous", "ai_with_human",
/// "framework".
impl<'de> serde::Deserialize<'de> for Actor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "human" => Ok(Actor::Human),
            "ai_autonomous" => Ok(Actor::AiAutonomous),
            "ai_with_human" => Ok(Actor::AiWithHuman),
            "framework" => Ok(Actor::Framework),
            other => Err(serde::de::Error::custom(format!(
                "unknown actor '{other}'; expected one of: human, ai_autonomous, ai_with_human, framework"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_support::ENV_LOCK;

    fn with_ai_env(stores_ai_runtime: Option<&str>, claudecode: Option<&str>, f: impl FnOnce()) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_stores = std::env::var("STORES_AI_RUNTIME").ok();
        let prev_claude = std::env::var("CLAUDECODE").ok();
        match stores_ai_runtime {
            Some(v) => std::env::set_var("STORES_AI_RUNTIME", v),
            None => std::env::remove_var("STORES_AI_RUNTIME"),
        }
        match claudecode {
            Some(v) => std::env::set_var("CLAUDECODE", v),
            None => std::env::remove_var("CLAUDECODE"),
        }

        f();

        match prev_stores {
            Some(v) => std::env::set_var("STORES_AI_RUNTIME", v),
            None => std::env::remove_var("STORES_AI_RUNTIME"),
        }
        match prev_claude {
            Some(v) => std::env::set_var("CLAUDECODE", v),
            None => std::env::remove_var("CLAUDECODE"),
        }
    }

    #[test]
    fn deserialize_known_actors() {
        let a: Actor = serde_yaml::from_str("human").unwrap();
        assert_eq!(a, Actor::Human);
        let b: Actor = serde_yaml::from_str("ai_autonomous").unwrap();
        assert_eq!(b, Actor::AiAutonomous);
        let c: Actor = serde_yaml::from_str("ai_with_human").unwrap();
        assert_eq!(c, Actor::AiWithHuman);
        let d: Actor = serde_yaml::from_str("framework").unwrap();
        assert_eq!(d, Actor::Framework);
    }

    #[test]
    fn deserialize_unknown_actor_errors() {
        let err = serde_yaml::from_str::<Actor>("robot").unwrap_err();
        assert!(err.to_string().contains("unknown actor 'robot'"));
    }

    #[test]
    fn display_roundtrips() {
        assert_eq!(Actor::Human.to_string(), "human");
        assert_eq!(Actor::AiAutonomous.to_string(), "ai_autonomous");
        assert_eq!(Actor::AiWithHuman.to_string(), "ai_with_human");
        assert_eq!(Actor::Framework.to_string(), "framework");
    }

    #[test]
    fn from_env_never_returns_framework() {
        with_ai_env(None, None, || {
            // Even with AI-runtime signals unset, from_env returns Human (not Framework).
            // Framework is only set programmatically.
            let actor = Actor::from_env();
            assert_ne!(actor, Actor::Framework);
        });
    }

    #[test]
    fn stores_ai_runtime_detected_when_non_empty() {
        with_ai_env(Some("claude-code"), None, || {
            assert!(Actor::ai_runtime_detected());
            assert_eq!(Actor::from_env(), Actor::AiAutonomous);
        });
    }

    #[test]
    fn claudecode_detected_when_non_empty() {
        with_ai_env(None, Some("1"), || {
            assert!(Actor::ai_runtime_detected());
            assert_eq!(Actor::from_env(), Actor::AiAutonomous);
        });
    }

    #[test]
    fn empty_or_unset_ai_runtime_values_are_not_detected() {
        with_ai_env(None, None, || {
            assert!(!Actor::ai_runtime_detected());
            assert_eq!(Actor::from_env(), Actor::Human);
        });
        with_ai_env(Some(""), Some(""), || {
            assert!(!Actor::ai_runtime_detected());
            assert_eq!(Actor::from_env(), Actor::Human);
        });
    }
}
