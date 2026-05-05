//! 4-position sort cycle for the rows pane.
//!
//! Cycle order: `UpdatedDesc → UpdatedAsc → Priority → DisplayId → UpdatedDesc`.
//! Pressing `,` four times returns to the original key.

use super::data::Row;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    UpdatedDesc,
    UpdatedAsc,
    Priority,
    DisplayId,
}

impl Sort {
    pub fn cycle(self) -> Self {
        match self {
            Sort::UpdatedDesc => Sort::UpdatedAsc,
            Sort::UpdatedAsc => Sort::Priority,
            Sort::Priority => Sort::DisplayId,
            Sort::DisplayId => Sort::UpdatedDesc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::UpdatedDesc => "updated↓",
            Sort::UpdatedAsc => "updated↑",
            Sort::Priority => "priority",
            Sort::DisplayId => "id",
        }
    }
}

/// Sort an index list (into `rows`) in-place per the active key.
pub fn sort_indices(sort: Sort, rows: &[Row], indices: &mut Vec<usize>) {
    match sort {
        Sort::UpdatedDesc => indices.sort_by(|&a, &b| updated(&rows[b]).cmp(updated(&rows[a]))),
        Sort::UpdatedAsc => indices.sort_by(|&a, &b| updated(&rows[a]).cmp(updated(&rows[b]))),
        Sort::Priority => indices.sort_by(|&a, &b| {
            priority_rank(&rows[a])
                .cmp(&priority_rank(&rows[b]))
                .then_with(|| updated(&rows[b]).cmp(updated(&rows[a])))
        }),
        Sort::DisplayId => indices.sort_by(|&a, &b| display_id(&rows[a]).cmp(display_id(&rows[b]))),
    }
}

fn updated(row: &Row) -> &str {
    match row {
        Row::Task(t) => &t.updated_at,
        Row::Obs(o) => &o.updated_at,
    }
}

fn display_id(row: &Row) -> &str {
    row.display_id()
}

fn priority_rank(row: &Row) -> u8 {
    let p = match row {
        Row::Obs(o) => o.priority.as_str(),
        // Tasks don't carry an explicit priority field in the loaded row;
        // treat them as `normal` so they cluster between high and low obs.
        Row::Task(_) => "normal",
    };
    match p {
        "high" => 0,
        "normal" | "" => 1,
        "low" => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::data::{ObsRow, Row, TaskRow};

    fn task(id: &str, updated: &str) -> Row {
        Row::Task(TaskRow {
            display_id: id.to_string(),
            status: "executing".to_string(),
            title: "t".to_string(),
            claimed_by: None,
            updated_at: updated.to_string(),
        })
    }

    fn obs(id: &str, priority: &str, updated: &str) -> Row {
        Row::Obs(ObsRow {
            display_id: id.to_string(),
            status: "open".to_string(),
            priority: priority.to_string(),
            summary: "s".to_string(),
            updated_at: updated.to_string(),
            contract_state: None,
        })
    }

    #[test]
    fn cycle_returns_after_four_presses() {
        let s = Sort::default();
        let cycled = s.cycle().cycle().cycle().cycle();
        assert_eq!(s, cycled);
    }

    #[test]
    fn cycle_visits_all_four_positions() {
        let mut s = Sort::UpdatedDesc;
        let mut seen = vec![s];
        for _ in 0..3 {
            s = s.cycle();
            seen.push(s);
        }
        assert_eq!(seen.len(), 4);
        // All four are distinct.
        let mut sorted = seen.clone();
        sorted.sort_by_key(|s| *s as u8);
        sorted.dedup();
        assert_eq!(sorted.len(), 4);
    }

    #[test]
    fn cycle_reorders_rows() {
        let rows = vec![
            task("T002", "2026-05-03"),
            task("T001", "2026-05-05"),
            task("T003", "2026-05-04"),
        ];
        let mut idx: Vec<usize> = (0..rows.len()).collect();

        sort_indices(Sort::UpdatedDesc, &rows, &mut idx);
        assert_eq!(idx, vec![1, 2, 0]); // T001(5), T003(4), T002(3)

        sort_indices(Sort::UpdatedAsc, &rows, &mut idx);
        assert_eq!(idx, vec![0, 2, 1]); // T002(3), T003(4), T001(5)

        sort_indices(Sort::DisplayId, &rows, &mut idx);
        assert_eq!(idx, vec![1, 0, 2]); // T001, T002, T003
    }

    #[test]
    fn priority_sort_high_before_low() {
        let rows = vec![
            obs("L1", "low", "2026-05-01"),
            obs("L2", "high", "2026-05-01"),
            obs("L3", "normal", "2026-05-01"),
        ];
        let mut idx: Vec<usize> = (0..rows.len()).collect();
        sort_indices(Sort::Priority, &rows, &mut idx);
        assert_eq!(idx, vec![1, 2, 0]); // high, normal, low
    }
}
