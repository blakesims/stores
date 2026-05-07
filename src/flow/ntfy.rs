//! ntfy notifier: posts halt/transition events to the configured ntfy URL.
//!
//! Resolution: `.stores/config.yaml` `ntfy.url` → env `STORES_NTFY_URL` → none.
//! When neither is set, `notify` writes a single stderr line and returns
//! `Ok(())`. Network/curl failures degrade the same way (warning to stderr,
//! `Ok(())`).
//!
//! Backend dispatch goes through a [`NotifierBackend`] trait so tests can
//! inject a [`MockNotifier`]. The default backend shells out to `curl`
//! (matches `topology.rs`'s `dot` shell-out pattern; no new deps).

use anyhow::Result;
use serde::Serialize;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// Payload posted on every halt or actor-enforced halt.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NotifyEvent {
    pub row_id: String,
    /// Free-form transition descriptor, e.g. `"tasks: in_review→accepted"`.
    pub transition_attempted: String,
    /// Either the halting policy id (`"halt-on-empty-branch"`) or the literal
    /// string `"actor-enforced halt"` when the substrate's actor enforcement
    /// halted the transition (no policy matched).
    pub policy_id_or_actor_halt: String,
    pub summary: String,
}

/// Pluggable transport. Production wires [`CurlNotifier`]; tests inject
/// [`MockNotifier`].
pub trait NotifierBackend: Send + Sync {
    fn send(&self, url: &str, event: &NotifyEvent) -> Result<()>;
}

/// Default backend: shells out to `curl` to POST the event body to the ntfy
/// URL. Missing curl or non-zero exit → `Err`; the calling
/// [`notify_with_path`] swallows that into a stderr warning.
pub struct CurlNotifier;

impl NotifierBackend for CurlNotifier {
    fn send(&self, url: &str, event: &NotifyEvent) -> Result<()> {
        let body = format!(
            "{} | {} | {} | {}",
            event.row_id, event.transition_attempted, event.policy_id_or_actor_halt, event.summary,
        );
        let mut child = Command::new("curl")
            .args(["-fsS", "-X", "POST", "-d", &body, url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("curl spawn failed: {e}"))?;
        let status = child.wait()?;
        if !status.success() {
            anyhow::bail!("curl exited non-zero ({status})");
        }
        Ok(())
    }
}

/// Test backend: records every (url, event) it receives.
#[derive(Default)]
pub struct MockNotifier {
    pub sent: Mutex<Vec<(String, NotifyEvent)>>,
}

impl MockNotifier {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn events(&self) -> Vec<(String, NotifyEvent)> {
        self.sent.lock().unwrap().clone()
    }
}

impl NotifierBackend for MockNotifier {
    fn send(&self, url: &str, event: &NotifyEvent) -> Result<()> {
        self.sent
            .lock()
            .unwrap()
            .push((url.to_string(), event.clone()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Process-global notifier
// ---------------------------------------------------------------------------

static GLOBAL: OnceLock<Mutex<Box<dyn NotifierBackend>>> = OnceLock::new();

fn global() -> &'static Mutex<Box<dyn NotifierBackend>> {
    GLOBAL.get_or_init(|| Mutex::new(Box::new(CurlNotifier)))
}

/// Replace the process-global backend (daemon installs this at startup;
/// tests use it to capture).
pub fn install_notifier(backend: Box<dyn NotifierBackend>) {
    let cell = global();
    *cell.lock().unwrap() = backend;
}

/// Notify using the global backend, resolving the URL from `config_path` then
/// the env var. Missing URL → log to stderr and return `Ok(())`. Backend
/// failure → log to stderr and return `Ok(())` (notification is best-effort).
pub fn notify_with_path(config_path: &Path, event: NotifyEvent) -> Result<()> {
    let url = crate::flow::config::resolve_ntfy_url(config_path);
    dispatch(url.as_deref(), &event, |u, e| {
        let g = global().lock().unwrap();
        g.send(u, e)
    })
}

/// Variant that takes an explicit backend reference (tests / direct use).
pub fn notify_with_backend(
    config_path: &Path,
    event: NotifyEvent,
    backend: &dyn NotifierBackend,
) -> Result<()> {
    let url = crate::flow::config::resolve_ntfy_url(config_path);
    dispatch(url.as_deref(), &event, |u, e| backend.send(u, e))
}

fn dispatch<F: FnOnce(&str, &NotifyEvent) -> Result<()>>(
    url: Option<&str>,
    event: &NotifyEvent,
    send: F,
) -> Result<()> {
    match url {
        Some(u) => match send(u, event) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!(
                    "ntfy: send failed for row {} ({}): {}",
                    event.row_id, event.transition_attempted, e
                );
                Ok(())
            }
        },
        None => {
            eprintln!(
                "ntfy: not configured; halt event for row {} ({}): {}",
                event.row_id, event.transition_attempted, event.summary
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn env_lock() -> &'static Mutex<()> {
        crate::paths::test_notifier_lock()
    }

    fn ev() -> NotifyEvent {
        NotifyEvent {
            row_id: "T014".into(),
            transition_attempted: "tasks: in_review→accepted".into(),
            policy_id_or_actor_halt: "halt-on-empty-branch".into(),
            summary: "branch field empty; cannot accept-merge".into(),
        }
    }

    #[test]
    fn mock_captures_event_verbatim() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("STORES_NTFY_URL", "https://ntfy.test/topic");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml"); // absent
        let mock = MockNotifier::new();
        notify_with_backend(&path, ev(), &mock).unwrap();
        std::env::remove_var("STORES_NTFY_URL");

        let captured = mock.events();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, "https://ntfy.test/topic");
        assert_eq!(captured[0].1, ev());
    }

    #[test]
    fn env_url_used_when_no_config() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::env::set_var("STORES_NTFY_URL", "https://env-only");
        let mock = MockNotifier::new();
        notify_with_backend(&path, ev(), &mock).unwrap();
        std::env::remove_var("STORES_NTFY_URL");

        let captured = mock.events();
        assert_eq!(captured[0].0, "https://env-only");
    }

    #[test]
    fn no_url_anywhere_returns_ok_and_writes_stderr() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("STORES_NTFY_URL");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml"); // absent
        let mock = MockNotifier::new();
        // Best-effort: returns Ok, does NOT call backend.
        notify_with_backend(&path, ev(), &mock).unwrap();
        assert!(mock.events().is_empty());
    }

    #[test]
    fn config_yaml_url_used_when_present() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("STORES_NTFY_URL");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "ntfy:\n  url: https://from-config\n").unwrap();
        let mock = MockNotifier::new();
        notify_with_backend(&path, ev(), &mock).unwrap();
        let captured = mock.events();
        assert_eq!(captured[0].0, "https://from-config");
    }

    #[test]
    fn install_notifier_replaces_global() {
        // Smoke test: install a mock and round-trip via notify_with_path.
        // We use a unique config path so the env-fallback test above isn't
        // affected by leakage. Hold the process-wide notifier lock so we
        // don't race with other modules' tests that also install global
        // mocks (flow::builtins::tests, handlers::agents_run::tests).
        let _g = crate::paths::test_notifier_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "ntfy:\n  url: https://global-test\n").unwrap();

        // Install a mock backend — note the global is process-wide, so this
        // test must be the only one touching it. Other tests use
        // `notify_with_backend` which doesn't read the global.
        let mock = Box::leak(Box::new(MockNotifier::new()));
        install_notifier(Box::new(MockNotifierShim { inner: mock }));
        notify_with_path(&path, ev()).unwrap();

        let captured = mock.events();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, "https://global-test");
    }

    /// Forwards send() to a leaked MockNotifier so the test can read events
    /// after installing the boxed backend.
    struct MockNotifierShim {
        inner: &'static MockNotifier,
    }
    impl NotifierBackend for MockNotifierShim {
        fn send(&self, url: &str, event: &NotifyEvent) -> Result<()> {
            self.inner.send(url, event)
        }
    }
}
