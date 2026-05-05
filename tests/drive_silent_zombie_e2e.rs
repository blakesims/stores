//! T030 Phase 1 / Task 1.3: end-to-end harness for the L062 silent-zombie
//! shape. Builds against the real `stores` binary via
//! `env!("CARGO_BIN_EXE_stores")`, sets up a tmp `.stores` DB, inserts a
//! task at planning with a known-dead `drive_pid`, pre-closes the
//! dispatch_lock to simulate `mark_claim_finished` having already fired,
//! then drives the daemon for a single iteration (or one watchdog sweep)
//! and asserts the row transitioned to `blocked`.
//!
//! Marked `#[ignore]` for Phase 1: the production code does not yet
//! detect this shape, so the test would fail. Phase 5 will un-ignore.

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore]
fn silent_zombie_lock_already_closed_e2e() {
    let bin: PathBuf = PathBuf::from(env!("CARGO_BIN_EXE_stores"));
    assert!(
        bin.exists(),
        "CARGO_BIN_EXE_stores must point at a built binary: {}",
        bin.display()
    );

    let tmp = tempfile::tempdir().expect("tmpdir");
    let workspace = tmp.path();
    let stores_dir = workspace.join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();

    // 1. Initialize the substrate DB in the workspace via `stores migrate`
    //    (or equivalent setup verb) — left as a TODO for Phase 5; the
    //    skeleton's job is to compile and document the shape.

    // 2. Insert a task row at status='planning' with drive_pid set to a
    //    PID overwhelmingly likely to be dead, plus a closed dispatch_lock
    //    (finished_at SET).

    // 3. Run the daemon for a bounded number of iterations:
    //    let output = Command::new(&bin)
    //        .args(["agents", "run", "--max-iters", "3", "--poll-interval-ms", "100"])
    //        .current_dir(workspace)
    //        .output()
    //        .expect("invoke daemon");
    //    assert!(output.status.success());
    let _ = Command::new(&bin); // keep `bin` referenced so this compiles.

    // 4. Read tasks.status for the inserted row and assert it == 'blocked'
    //    with blocked_reason == 'drive_failed'.
    //    (Phase 5 fills the body once the watchdog detects this shape.)
}
