//! Integration test for the `--apply` transaction-rollback guarantee
//! (T017 contract clause (7)(e); see also `tests/migrate_e2e.sh` (e)).
//!
//! Strategy:
//!   1. Spin up a temp `.stores/` and install the bundled `observations`
//!      store via the public `install::run` path.
//!   2. Mutate the live DB to a state where `compute_plan` returns two
//!      additive ALTERs whose batch will fail mid-way:
//!         * drop columns `body` and `source`
//!         * pre-add `SOURCE` (uppercase) — case-collides with the
//!           lowercase `source` ALTER but is invisible to compute_plan's
//!           case-sensitive HashMap lookup.
//!   3. Call `migrate::run_migrate(true)` and assert:
//!      (i)  it returns `Err` (transaction rolled back).
//!      (ii) the post-failure DB state matches the pre-call state — in
//!           particular, the `body` column was NOT partially applied.

use rusqlite::Connection;
use std::path::PathBuf;

use stores::handlers::migrate;

/// Capture the columns of `observations` as a sorted `(name, type)` vector
/// suitable for direct equality assertion.
fn columns(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn.prepare("PRAGMA table_info(\"observations\")").unwrap();
    let mut rows: Vec<(String, String)> = stmt
        .query_map([], |row| {
            let name: String = row.get(1)?;
            let ty: String = row.get(2)?;
            Ok((name, ty))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows.sort();
    rows
}

#[test]
fn run_migrate_apply_rolls_back_on_partial_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workdir: PathBuf = tmp.path().to_path_buf();

    let old_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(&workdir).expect("cd tmp");

    // Always restore cwd, even on assertion panic.
    let _restore = scopeguard(|| {
        let _ = std::env::set_current_dir(&old_cwd);
    });

    // 1. Init + install bundled observations store via the public path.
    stores::cli::init::run().expect("stores init");
    stores::install::run(&PathBuf::from("observations")).expect("install observations");

    let db_file = workdir.join(".stores/db.sqlite");
    let conn = Connection::open(&db_file).expect("open db");

    // 2. Engineer the partial-failure fixture.
    conn.execute_batch(
        "ALTER TABLE \"observations\" DROP COLUMN body; \
         ALTER TABLE \"observations\" DROP COLUMN source; \
         ALTER TABLE \"observations\" ADD COLUMN SOURCE TEXT;",
    )
    .expect("fixture setup");

    let pre_cols = columns(&conn);
    assert!(
        !pre_cols.iter().any(|(n, _)| n == "body"),
        "fixture broken: body still present pre-call"
    );
    assert!(
        !pre_cols.iter().any(|(n, _)| n == "source"),
        "fixture broken: source still present pre-call"
    );
    assert!(
        pre_cols.iter().any(|(n, _)| n == "SOURCE"),
        "fixture broken: uppercase SOURCE not pre-added"
    );

    // Drop our handle so run_migrate can reopen the DB without lock contention.
    drop(conn);

    // 3. Attempt --apply — must return Err (partial-failure rollback).
    let result = migrate::run_migrate(true);
    assert!(
        result.is_err(),
        "run_migrate(apply=true) must Err on partial failure; got: {result:?}"
    );

    // 4. Reopen and assert state is unchanged.
    let conn2 = Connection::open(&db_file).expect("reopen db");
    let post_cols = columns(&conn2);
    assert_eq!(
        pre_cols, post_cols,
        "PRAGMA table_info changed across failed --apply; \
         transaction did not roll back cleanly"
    );
    assert!(
        !post_cols.iter().any(|(n, _)| n == "body"),
        "rollback failed: body column was partially applied"
    );
}

/// Minimal RAII guard so we restore cwd even if assertions panic.
fn scopeguard<F: FnOnce()>(f: F) -> impl Drop {
    struct Guard<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for Guard<F> {
        fn drop(&mut self) {
            if let Some(f) = self.0.take() {
                f();
            }
        }
    }
    Guard(Some(f))
}
