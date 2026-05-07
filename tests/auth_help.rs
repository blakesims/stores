//! T074: `stores auth show --help` must render from the repository root and
//! advertise the explicit age identity override.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn auth_show_help_lists_identity_from_repo_root() {
    let bin = env!("CARGO_BIN_EXE_stores");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(bin)
        .current_dir(root)
        .args(["auth", "show", "--help"])
        .output()
        .expect("failed to invoke stores binary");

    assert!(
        output.status.success(),
        "stores auth show --help exited non-zero: {:?}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("help output must be UTF-8");
    assert!(
        stdout.contains("--identity"),
        "expected `--identity` in auth show help output, got:\n{stdout}"
    );
}
