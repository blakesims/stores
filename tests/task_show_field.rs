use serde_json::Value;
use std::process::{Command, Output};

const TITLE: &str = "Structured read fixture";
const DONE_WHEN: &str = "orchestrator reads fields without grepping markdown";

fn stores(repo: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stores"))
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run stores {args:?}: {e}"))
}

fn assert_success(output: Output, args: &[&str]) -> Output {
    assert!(
        output.status.success(),
        "stores {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn fixture_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    assert_success(stores(tmp.path(), &["init"]), &["init"]);
    assert_success(stores(tmp.path(), &["install", "tasks"]), &["install", "tasks"]);
    let add_args = [
        "tasks",
        "add",
        "--title",
        TITLE,
        "--slug",
        "structured-read-fixture",
        "--capability",
        "test",
        "--done-when",
        DONE_WHEN,
        "--scope-in",
        "show field selector",
        "--scope-out",
        "write verbs",
        "--invoker",
        "ai_with_human",
    ];
    let add = assert_success(stores(tmp.path(), &add_args), &add_args);
    assert_eq!(String::from_utf8(add.stdout).unwrap(), "T001\n");
    tmp
}

#[test]
fn task_show_field_reads_scalars_objects_json_and_reports_missing_fields() {
    let tmp = fixture_repo();

    let title_args = ["tasks", "show", "T001", "--field", "title"];
    let title = assert_success(stores(tmp.path(), &title_args), &title_args);
    assert_eq!(String::from_utf8(title.stdout).unwrap(), format!("{TITLE}\n"));

    let done_when_args = ["tasks", "show", "T001", "--field", "contract.done_when"];
    let done_when = assert_success(stores(tmp.path(), &done_when_args), &done_when_args);
    assert_eq!(
        String::from_utf8(done_when.stdout).unwrap(),
        format!("{DONE_WHEN}\n")
    );

    let contract_args = ["tasks", "show", "T001", "--field", "contract", "--json"];
    let contract = assert_success(stores(tmp.path(), &contract_args), &contract_args);
    let contract_json: Value = serde_json::from_slice(&contract.stdout).unwrap();
    assert_eq!(contract_json["done_when"], DONE_WHEN);

    let row_args = ["tasks", "show", "T001", "--json"];
    let row = assert_success(stores(tmp.path(), &row_args), &row_args);
    let row_json: Value = serde_json::from_slice(&row.stdout).unwrap();
    assert_eq!(row_json["display_id"], "T001");
    assert_eq!(row_json["contract"]["done_when"], DONE_WHEN);

    let missing_args = ["tasks", "show", "T001", "--field", "no_such_field"];
    let missing = stores(tmp.path(), &missing_args);
    assert!(
        !missing.status.success(),
        "stores {missing_args:?} unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("no_such_field"),
        "stderr did not contain missing field name:\n{}",
        String::from_utf8_lossy(&missing.stderr)
    );
}
