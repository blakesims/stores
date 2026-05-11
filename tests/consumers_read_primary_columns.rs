use std::fs;

#[test]
fn consumers_read_primary_columns() {
    let checks = [
        ("src/tui/data.rs", "apply_task_to_flow(&t.status"),
        ("src/tui/render.rs", "match t.status.as_str()"),
        ("src/tui/render.rs", "t.status == \"planning\""),
        ("src/tui/app.rs", "is_terminal_task_status(&t.status)"),
        ("src/cli/watch.rs", "task_status_rank(&t.status)"),
    ];
    let mut offenders = Vec::new();
    for (path, needle) in checks {
        let text = strip_cfg_test_blocks(&fs::read_to_string(path).unwrap());
        if text.contains(needle) {
            offenders.push(format!("{path}: {needle}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "status semantic branches remain:\n{}",
        offenders.join("\n")
    );
}

fn strip_cfg_test_blocks(text: &str) -> String {
    let mut out = Vec::new();
    let mut skip_next = false;
    for line in text.lines() {
        if line.trim() == "#[cfg(test)]" {
            skip_next = true;
            continue;
        }
        if skip_next {
            if line.starts_with("fn ")
                || line.starts_with("mod tests")
                || line.starts_with("struct ")
            {
                skip_next = false;
                continue;
            }
            skip_next = false;
        }
        out.push(line);
    }
    out.join("\n")
}
