//! AC4.2: `stores topology --help` advertises the three documented flags.

use std::process::Command;

#[test]
fn topology_help_lists_documented_flags() {
    let bin = env!("CARGO_BIN_EXE_stores");
    let tmp = tempfile::tempdir().expect("tmpdir");
    let output = Command::new(bin)
        .current_dir(tmp.path())
        .args(["topology", "--help"])
        .output()
        .expect("failed to invoke stores binary");

    assert!(
        output.status.success(),
        "stores topology --help exited non-zero: {:?}",
        output.status
    );

    let stdout = String::from_utf8(output.stdout).expect("help output must be UTF-8");

    for flag in ["--format", "--store", "--no-icons"] {
        assert!(
            stdout.contains(flag),
            "expected `{flag}` in topology --help output, got:\n{stdout}"
        );
    }
}
