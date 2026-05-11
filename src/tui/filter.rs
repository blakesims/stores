//! Filter palette overlay and predicate state.
//!
//! Pressing `f` opens the palette; pressing `1`/`2`/`3` opens a preset.
//! `Esc` cancels (drafted predicate is discarded), `Enter` applies.

use super::data::{Row, Section};

/// Filterable fields. Predicates are AND-combined; an unset field is wildcard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterPredicate {
    pub state: Option<String>,
    pub priority: Option<String>,
    pub tier: Option<String>,
    pub since: Option<String>,
}

impl FilterPredicate {
    pub fn is_empty(&self) -> bool {
        self.state.is_none()
            && self.priority.is_none()
            && self.tier.is_none()
            && self.since.is_none()
    }

    /// Returns true if the row, classified into `section`, matches.
    pub fn matches(&self, row: &Row, section: Section) -> bool {
        if let Some(s) = &self.state {
            if !state_matches(s, row, section) {
                return false;
            }
        }
        if let Some(p) = &self.priority {
            match row {
                Row::Obs(o) => {
                    if &o.priority != p {
                        return false;
                    }
                }
                Row::CollapsedObs(c) => {
                    if &c.representative.priority != p {
                        return false;
                    }
                }
                Row::Task(_) | Row::Review(_) => {
                    // Tasks/reviews are unfiltered by priority (no field loaded).
                }
                Row::Intake(i) => {
                    if i.priority.as_deref() != Some(p.as_str()) {
                        return false;
                    }
                }
            }
        }
        // tier / since: predicate is recorded but data is best-effort
        // (intent_contract.tier_hint and updated_at cutoffs are not yet
        // surfaced through the Row struct; applying them is a no-op here).
        let _ = (&self.tier, &self.since);
        true
    }
}

fn state_matches(state: &str, row: &Row, section: Section) -> bool {
    match state {
        // Saved-view aliases: route through section classification so the
        // filter palette can be opened with a preset and the same predicate
        // works for keypresses 1/2/3.
        "ratifiable" | "ratify_u1" => section == Section::ObsRatifiable,
        "accept_u3" => section == Section::TasksAcceptU3,
        "ai_review" | "held_ai_review" => section == Section::TasksHeldAiReview,
        "silent_zombie" | "held_zombie" => section == Section::TasksHeldZombie,
        "actionable_current_work" | "in_flight" => section == Section::TasksActionableCurrentWork,
        "deploy_recovery" | "deploy_blocked" => section == Section::TasksDeployRecovery,
        "blocked_needs_action" => section == Section::TasksBlockedNeedsAction,
        "needs_triage" => section == Section::TasksNeedsTriage,
        "recently_terminal" => section == Section::TasksRecentlyTerminal,
        // Otherwise match the raw status field. ADR 0002 compatibility-only T148 task 6.1:
        // upstream rows retain this legacy filter only for explicit status queries.
        other => match row {
            Row::Task(t) => t.status == other,
            Row::Obs(o) => o.status == other,
            Row::CollapsedObs(c) => c.representative.status == other,
            Row::Review(r) => r.status == other || other == "external_review",
            Row::Intake(i) => i.status == other,
        },
    }
}

/// Saved-view presets bound to keys `1`/`2`/`3`.
pub fn preset(key: char) -> Option<FilterPredicate> {
    match key {
        '1' => Some(FilterPredicate {
            state: Some("ratifiable".to_string()),
            ..Default::default()
        }),
        '2' => Some(FilterPredicate {
            state: Some("actionable_current_work".to_string()),
            ..Default::default()
        }),
        '3' => Some(FilterPredicate {
            state: Some("deploy_recovery".to_string()),
            ..Default::default()
        }),
        _ => None,
    }
}

/// Palette state during the `f`-mode interaction.
#[derive(Debug, Clone, Default)]
pub struct FilterPalette {
    /// Buffered keystroke text (e.g. "state=in_flight").
    pub buffer: String,
    /// Predicate that will be committed on Enter.
    pub draft: FilterPredicate,
}

impl FilterPalette {
    pub fn open() -> Self {
        Self::default()
    }

    /// Append a character into the textual buffer and re-parse.
    pub fn push(&mut self, c: char) {
        self.buffer.push(c);
        self.reparse();
    }

    pub fn backspace(&mut self) {
        self.buffer.pop();
        self.reparse();
    }

    fn reparse(&mut self) {
        let mut p = FilterPredicate::default();
        for part in self.buffer.split([' ', ',']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((k, v)) = part.split_once('=') {
                let v = v.trim().to_string();
                match k.trim() {
                    "state" => p.state = Some(v),
                    "priority" => p.priority = Some(v),
                    "tier" => p.tier = Some(v),
                    "since" => p.since = Some(v),
                    _ => {}
                }
            }
        }
        self.draft = p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::data::{ObsRow, TaskRow};

    fn task(status: &str) -> Row {
        Row::Task(TaskRow {
            display_id: "T001".to_string(),
            status: status.to_string(),
            title: "t".to_string(),
            claimed_by: None,
            updated_at: String::new(),
            ..Default::default()
        })
    }

    fn obs(priority: &str) -> Row {
        Row::Obs(ObsRow {
            display_id: "L001".to_string(),
            status: "open".to_string(),
            priority: priority.to_string(),
            summary: "s".to_string(),
            updated_at: String::new(),
            contract_state: None,
            ..Default::default()
        })
    }

    #[test]
    fn palette_round_trip() {
        // Open palette, type "state=in_flight", parse, commit, then Esc-cancel
        // would restore the original predicate (we model that at the App layer).
        let mut p = FilterPalette::open();
        for c in "state=in_flight".chars() {
            p.push(c);
        }
        assert_eq!(p.draft.state.as_deref(), Some("in_flight"));
        // Backspace clears the parsed value.
        for _ in 0..("in_flight".len()) {
            p.backspace();
        }
        assert_eq!(p.draft.state.as_deref(), Some(""));
    }

    #[test]
    fn predicate_state_in_flight() {
        let p = FilterPredicate {
            state: Some("in_flight".to_string()),
            ..Default::default()
        };
        assert!(p.matches(&task("executing"), Section::TasksActionableCurrentWork));
        assert!(!p.matches(&task("blocked"), Section::TasksBlockedNeedsAction));
    }

    #[test]
    fn predicate_priority_filters_obs_only() {
        let p = FilterPredicate {
            priority: Some("high".to_string()),
            ..Default::default()
        };
        assert!(p.matches(&obs("high"), Section::ObsOpenNoContract));
        assert!(!p.matches(&obs("low"), Section::ObsOpenNoContract));
    }

    #[test]
    fn presets_for_saved_views() {
        assert_eq!(preset('1').unwrap().state.as_deref(), Some("ratifiable"));
        assert_eq!(
            preset('2').unwrap().state.as_deref(),
            Some("actionable_current_work")
        );
        assert_eq!(
            preset('3').unwrap().state.as_deref(),
            Some("deploy_recovery")
        );
        assert!(preset('4').is_none());
    }
}
