//! Incremental search bar: case-insensitive substring match against
//! `display_id` and `title_or_summary`. `n`/`N` cycle hits.

use super::data::Row;

#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub active: bool,
    pub query: String,
    /// Indices into the visible-row list (post-filter), most recently
    /// recomputed when the query changes.
    pub hits: Vec<usize>,
    pub hit_idx: usize,
}

impl SearchState {
    pub fn open() -> Self {
        Self {
            active: true,
            ..Default::default()
        }
    }

    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.hits.clear();
        self.hit_idx = 0;
    }

    pub fn push(&mut self, c: char) {
        self.query.push(c);
    }

    pub fn backspace(&mut self) {
        self.query.pop();
    }

    /// Recompute hits over a flat row slice. `flat[i]` is the i-th row in
    /// the rendered visible list. Returns the chosen hit index (in `flat`)
    /// or `None` if empty.
    pub fn recompute(&mut self, flat: &[&Row]) -> Option<usize> {
        let q = self.query.to_lowercase();
        self.hits.clear();
        if q.is_empty() {
            self.hit_idx = 0;
            return None;
        }
        for (i, row) in flat.iter().enumerate() {
            let collapsed_id_hit = match row {
                Row::CollapsedObs(c) => c
                    .display_ids
                    .iter()
                    .any(|id| id.to_lowercase().contains(&q)),
                _ => false,
            };
            if row.display_id().to_lowercase().contains(&q)
                || row.title_or_summary().to_lowercase().contains(&q)
                || collapsed_id_hit
            {
                self.hits.push(i);
            }
        }
        self.hit_idx = 0;
        self.hits.first().copied()
    }

    /// Move to the next hit (`n`); returns the new flat index, if any.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<usize> {
        if self.hits.is_empty() {
            return None;
        }
        self.hit_idx = (self.hit_idx + 1) % self.hits.len();
        Some(self.hits[self.hit_idx])
    }

    /// Move to the previous hit (`N`); returns the new flat index, if any.
    pub fn prev(&mut self) -> Option<usize> {
        if self.hits.is_empty() {
            return None;
        }
        if self.hit_idx == 0 {
            self.hit_idx = self.hits.len() - 1;
        } else {
            self.hit_idx -= 1;
        }
        Some(self.hits[self.hit_idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::data::{CollapsedObsRow, ObsRow, Section, TaskRow};

    fn task(id: &str, title: &str) -> Row {
        Row::Task(TaskRow {
            display_id: id.to_string(),
            status: "executing".to_string(),
            title: title.to_string(),
            claimed_by: None,
            updated_at: String::new(),
            ..Default::default()
        })
    }

    fn obs(id: &str, summary: &str) -> Row {
        Row::Obs(ObsRow {
            display_id: id.to_string(),
            status: "open".to_string(),
            priority: "normal".to_string(),
            summary: summary.to_string(),
            updated_at: String::new(),
            contract_state: None,
            ..Default::default()
        })
    }

    #[test]
    fn search_t01_matches_first_task() {
        let rows = [
            task("T010", "tenth"),
            task("T002", "second"),
            task("T011", "eleventh"),
            task("T012", "twelfth"),
        ];
        let flat: Vec<&Row> = rows.iter().collect();
        let mut s = SearchState::open();
        for c in "T01".chars() {
            s.push(c);
        }
        let hit = s.recompute(&flat);
        // T010, T011, T012 all contain "T01" as a substring; T002 does not.
        assert_eq!(hit, Some(0));
        assert_eq!(s.hits, vec![0, 2, 3]);
        assert_eq!(s.next(), Some(2));
        assert_eq!(s.next(), Some(3));
        assert_eq!(s.next(), Some(0)); // wraps
        assert_eq!(s.prev(), Some(3));
    }

    #[test]
    fn search_summary_fragment_case_insensitive() {
        let rows = [
            obs("L001", "Daemon liveness wedged"),
            obs("L002", "Approval token broken"),
        ];
        let flat: Vec<&Row> = rows.iter().collect();
        let mut s = SearchState::open();
        for c in "approval".chars() {
            s.push(c);
        }
        let hit = s.recompute(&flat);
        assert_eq!(hit, Some(1));
        assert_eq!(s.hits, vec![1]);
    }

    #[test]
    fn search_matches_hidden_collapsed_observation_ids() {
        let row = Row::CollapsedObs(CollapsedObsRow {
            section: Section::ObsOther,
            summary: "cluster".to_string(),
            count: 2,
            primary_display_id: "L001".to_string(),
            display_ids: vec!["L001".to_string(), "L999".to_string()],
            representative: ObsRow {
                display_id: "L001".to_string(),
                status: "open".to_string(),
                priority: "normal".to_string(),
                summary: "cluster".to_string(),
                ..Default::default()
            },
        });
        let rows = [row];
        let flat: Vec<&Row> = rows.iter().collect();
        let mut s = SearchState::open();
        for c in "L999".chars() {
            s.push(c);
        }
        assert_eq!(s.recompute(&flat), Some(0));
    }

    #[test]
    fn empty_query_returns_no_hits() {
        let rows = [task("T001", "x")];
        let flat: Vec<&Row> = rows.iter().collect();
        let mut s = SearchState::open();
        assert_eq!(s.recompute(&flat), None);
        assert!(s.hits.is_empty());
    }
}
