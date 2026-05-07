pub mod agents;
pub mod auth;
pub mod dispatch;
pub mod dynamic;
pub mod init;
pub mod metrics;
pub mod setup;
pub mod skills;
pub mod topology;
pub mod watch;

/// Shared test-only synchronization for env-var mutation across modules.
/// Both `auth::tests` and `dispatch::tests` mutate `STORES_TOKEN_DIR` and
/// must serialize against each other — within-module Mutexes are not enough
/// when `cargo test` runs modules in parallel threads.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());
}
