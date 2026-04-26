use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Human,
    AiAutonomous,
    AiWithHuman,
}

impl Actor {
    /// Read actor from environment: if $CLAUDECODE is set (non-empty), we are AI-autonomous.
    pub fn from_env() -> Actor {
        match std::env::var("CLAUDECODE") {
            Ok(v) if !v.is_empty() => Actor::AiAutonomous,
            _ => Actor::Human,
        }
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::Human => write!(f, "human"),
            Actor::AiAutonomous => write!(f, "ai_autonomous"),
            Actor::AiWithHuman => write!(f, "ai_with_human"),
        }
    }
}

/// Deserialize actor from YAML strings like "human", "ai_autonomous", "ai_with_human".
impl<'de> serde::Deserialize<'de> for Actor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "human" => Ok(Actor::Human),
            "ai_autonomous" => Ok(Actor::AiAutonomous),
            "ai_with_human" => Ok(Actor::AiWithHuman),
            other => Err(serde::de::Error::custom(format!(
                "unknown actor '{other}'; expected one of: human, ai_autonomous, ai_with_human"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_known_actors() {
        let a: Actor = serde_yaml::from_str("human").unwrap();
        assert_eq!(a, Actor::Human);
        let b: Actor = serde_yaml::from_str("ai_autonomous").unwrap();
        assert_eq!(b, Actor::AiAutonomous);
        let c: Actor = serde_yaml::from_str("ai_with_human").unwrap();
        assert_eq!(c, Actor::AiWithHuman);
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
    }
}
