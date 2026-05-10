#[test]
fn transition_history_primary_tuple_columns_declared() {
    let ddl = include_str!("../src/codegen/ddl.rs");
    for col in [
        "lifecycle_from",
        "active_step_from",
        "integration_step_from",
        "lifecycle_to",
        "active_step_to",
        "integration_step_to",
    ] {
        assert!(ddl.contains(col), "missing transition_history column {col}");
    }
}

#[test]
fn task_status_reads_primary_lifecycle_columns() {
    let status_rs = include_str!("../src/handlers/status.rs");
    assert!(status_rs.contains("task_projection_exprs"));
    assert!(status_rs.contains("lifecycle_expr"));
    assert!(status_rs.contains("active_step_expr"));
    assert!(status_rs.contains("integration_step_expr"));
}
