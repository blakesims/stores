/// Mock runner for testing.
///
/// Holds a programmable queue of `RunnerOutput` values. Each `spawn` call pops
/// the next item from the front of the queue and returns it. When the queue is
/// exhausted, `spawn` returns an error.
///
/// The mock runner is always compiled (not feature-gated) so it is available in
/// all test configurations.
use anyhow::{bail, Result};
use std::cell::RefCell;

use super::{Runner, RunnerOutput};

/// A test-only runner backed by a pre-loaded response queue.
///
/// # Example
/// ```rust
/// use stores::runner::{RunnerOutput, mock::MockRunner, Runner};
///
/// let queue = vec![RunnerOutput {
///     stdout: "output".to_string(),
///     stderr: String::new(),
///     exit_code: 0,
///     final_message: Some(r#"{"role":"planner"}"#.to_string()),
///     structured_output: None,
///     session_id: None,
///     structured_output_source: None,
/// }];
/// let runner = MockRunner::new(queue);
/// let out = runner.spawn("planner", "sys", "brief", None, None).unwrap();
/// assert_eq!(out.exit_code, 0);
/// ```
pub struct MockRunner {
    queue: RefCell<Vec<RunnerOutput>>,
    workspace_paths_seen: RefCell<Vec<Option<String>>>,
}

impl MockRunner {
    /// Create a new `MockRunner` with the given response queue.
    ///
    /// Responses are returned in order (index 0 first). When the queue is
    /// exhausted, subsequent `spawn` calls return an error.
    pub fn new(queue: Vec<RunnerOutput>) -> Self {
        // Reverse so we can pop from the back efficiently (pop() returns last element).
        let mut q = queue;
        q.reverse();
        Self {
            queue: RefCell::new(q),
            workspace_paths_seen: RefCell::new(Vec::new()),
        }
    }

    /// Return the number of responses remaining in the queue.
    /// A value of 0 means all queued responses were consumed (the runner was fully drained).
    pub fn remaining_count(&self) -> usize {
        self.queue.borrow().len()
    }

    /// Return the `workspace_path` arguments recorded across all `spawn` calls, in order.
    pub fn workspace_paths_seen(&self) -> Vec<Option<String>> {
        self.workspace_paths_seen.borrow().clone()
    }
}

// SAFETY: MockRunner is used in single-threaded tests; RefCell is not Sync, but
// Runner only requires Send. Since MockRunner itself contains no raw pointers and
// RefCell<Vec<_>> can be moved between threads (just not shared), this is safe.
unsafe impl Send for MockRunner {}

impl Runner for MockRunner {
    fn name(&self) -> &str {
        "mock"
    }

    fn spawn(
        &self,
        role: &str,
        _system_prompt: &str,
        _brief: &str,
        _schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput> {
        self.workspace_paths_seen
            .borrow_mut()
            .push(workspace_path.map(|s| s.to_string()));
        let mut queue = self.queue.borrow_mut();
        match queue.pop() {
            Some(output) => Ok(output),
            None => bail!("mock runner: response queue exhausted (role={role})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_output(stdout: &str, exit_code: i32, final_message: Option<&str>) -> RunnerOutput {
        RunnerOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code,
            final_message: final_message.map(|s| s.to_string()),
            structured_output: None,
            session_id: None,
            structured_output_source: None,
        }
    }

    #[test]
    fn name_returns_mock() {
        let runner = MockRunner::new(vec![]);
        assert_eq!(runner.name(), "mock");
    }

    #[test]
    fn queued_responses_returned_in_order() {
        let first = make_output("first output", 0, Some(r#"{"role":"planner"}"#));
        let second = make_output("second output", 0, Some(r#"{"role":"executor"}"#));
        let runner = MockRunner::new(vec![first, second]);

        let out1 = runner
            .spawn("planner", "sys", "brief1", None, None)
            .unwrap();
        assert_eq!(out1.stdout, "first output");
        assert_eq!(out1.exit_code, 0);
        assert_eq!(out1.final_message.as_deref(), Some(r#"{"role":"planner"}"#));

        let out2 = runner
            .spawn("executor", "sys", "brief2", None, None)
            .unwrap();
        assert_eq!(out2.stdout, "second output");
        assert_eq!(
            out2.final_message.as_deref(),
            Some(r#"{"role":"executor"}"#)
        );
    }

    #[test]
    fn empty_queue_returns_error() {
        let runner = MockRunner::new(vec![]);
        let err = runner
            .spawn("planner", "sys", "brief", None, None)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("queue exhausted"),
            "expected 'queue exhausted' in error, got: {msg}"
        );
        assert!(
            msg.contains("planner"),
            "expected role name in error, got: {msg}"
        );
    }

    #[test]
    fn queue_exhaustion_after_consuming_all() {
        let runner = MockRunner::new(vec![make_output("only", 0, None)]);
        runner.spawn("r", "s", "b", None, None).unwrap(); // consumes the one item
        let err = runner.spawn("r", "s", "b", None, None).unwrap_err();
        assert!(err.to_string().contains("queue exhausted"));
    }

    #[test]
    fn non_zero_exit_code_returned_not_errored() {
        let runner = MockRunner::new(vec![make_output("fail output", 1, None)]);
        let out = runner
            .spawn("executor", "sys", "brief", None, None)
            .unwrap();
        assert_eq!(out.exit_code, 1);
        assert_eq!(out.stdout, "fail output");
    }

    /// AC1.6: MockRunner accepts a RunnerOutput with structured_output: Some(...)
    /// and spawn returns it verbatim. The schema arg is ignored.
    #[test]
    fn structured_output_round_trip() {
        let structured = serde_json::json!({
            "role": "planner",
            "phases": [],
            "decision_matrix": []
        });
        let output = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            final_message: None,
            structured_output: Some(structured.clone()),
            session_id: None,
            structured_output_source: None,
        };
        let runner = MockRunner::new(vec![output]);
        // schema arg is ignored by mock
        let out = runner
            .spawn(
                "planner",
                "sys",
                "brief",
                Some(r#"{"$schema":"ignored"}"#),
                None,
            )
            .unwrap();
        assert_eq!(out.structured_output.as_ref(), Some(&structured));
        assert_eq!(out.session_id, None);
    }
}
