//! `stores topology` — static schematic of the substrate.
//!
//! Phase 1: CLI scaffolding + actor styling table only. Emitters land in Phase 2.

use anyhow::Result;
use std::collections::HashMap;

use crate::manifest::Manifest;
use crate::schema::Schema;
use crate::schema::actor::Actor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Auto,
    Dot,
    Mermaid,
}

#[derive(Debug, Clone)]
pub struct Opts {
    pub format: Format,
    pub store_filter: Option<String>,
    pub no_icons: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorStyle {
    pub dot_color: &'static str,
    pub icon: &'static str,
    pub text_code: &'static str,
    pub label_prefix: String,
}

/// Resolve an actor (or absence thereof) to a renderable style.
///
/// `color_disabled` is true when `NO_COLOR` is set or the caller is the mermaid
/// emitter (which has no per-edge color); in that case `dot_color` is "" so the
/// caller can skip emitting a `color=` attribute.
///
/// `no_icons` strips the Nerd Font glyph; `label_prefix` becomes the text code
/// (`A` / `H+` / `H!` / `F`) on its own. With icons enabled, the prefix is
/// `<icon> <text_code>`.
pub fn actor_style(actor: Option<Actor>, no_icons: bool, color_disabled: bool) -> ActorStyle {
    let (dot_color, icon, text_code) = match actor {
        Some(Actor::AiAutonomous) => ("green", "\u{f544}", "A"),
        Some(Actor::AiWithHuman) => ("gold", "\u{f2b5}", "H+"),
        Some(Actor::Human) => ("red", "\u{f007}", "H!"),
        Some(Actor::Framework) | None => ("gray", "\u{f013}", "F"),
    };

    let label_prefix = if no_icons {
        text_code.to_string()
    } else {
        format!("{icon} {text_code}")
    };

    ActorStyle {
        dot_color: if color_disabled { "" } else { dot_color },
        icon,
        text_code,
        label_prefix,
    }
}

/// Entry point for the `topology` subcommand. Phase 1: no-op.
pub fn run(
    _manifest: &Manifest,
    _schemas: &HashMap<String, Schema>,
    _opts: Opts,
) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_style_ai_autonomous_color_on_icons_on() {
        let s = actor_style(Some(Actor::AiAutonomous), false, false);
        assert_eq!(s.dot_color, "green");
        assert_eq!(s.text_code, "A");
        assert_eq!(s.icon, "\u{f544}");
        assert_eq!(s.label_prefix, format!("{} A", "\u{f544}"));
    }

    #[test]
    fn actor_style_ai_autonomous_color_off_icons_on() {
        let s = actor_style(Some(Actor::AiAutonomous), false, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.text_code, "A");
        assert_eq!(s.label_prefix, format!("{} A", "\u{f544}"));
    }

    #[test]
    fn actor_style_ai_autonomous_color_on_icons_off() {
        let s = actor_style(Some(Actor::AiAutonomous), true, false);
        assert_eq!(s.dot_color, "green");
        assert_eq!(s.label_prefix, "A");
    }

    #[test]
    fn actor_style_ai_autonomous_color_off_icons_off() {
        let s = actor_style(Some(Actor::AiAutonomous), true, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.label_prefix, "A");
    }

    #[test]
    fn actor_style_ai_with_human_all_modes() {
        let s = actor_style(Some(Actor::AiWithHuman), false, false);
        assert_eq!(s.dot_color, "gold");
        assert_eq!(s.text_code, "H+");
        assert_eq!(s.label_prefix, format!("{} H+", "\u{f2b5}"));

        let s = actor_style(Some(Actor::AiWithHuman), true, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.label_prefix, "H+");
    }

    #[test]
    fn actor_style_human_all_modes() {
        let s = actor_style(Some(Actor::Human), false, false);
        assert_eq!(s.dot_color, "red");
        assert_eq!(s.text_code, "H!");
        assert_eq!(s.label_prefix, format!("{} H!", "\u{f007}"));

        let s = actor_style(Some(Actor::Human), true, false);
        assert_eq!(s.dot_color, "red");
        assert_eq!(s.label_prefix, "H!");

        let s = actor_style(Some(Actor::Human), false, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.label_prefix, format!("{} H!", "\u{f007}"));
    }

    #[test]
    fn actor_style_framework_all_modes() {
        let s = actor_style(Some(Actor::Framework), false, false);
        assert_eq!(s.dot_color, "gray");
        assert_eq!(s.text_code, "F");
        assert_eq!(s.label_prefix, format!("{} F", "\u{f013}"));

        let s = actor_style(Some(Actor::Framework), true, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.label_prefix, "F");
    }

    #[test]
    fn actor_style_none_treated_as_framework() {
        let s = actor_style(None, false, false);
        assert_eq!(s.dot_color, "gray");
        assert_eq!(s.text_code, "F");
    }
}
