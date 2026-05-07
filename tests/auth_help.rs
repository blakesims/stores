//! T078: `stores auth show --help` renders from the repository root and no
//! longer advertises age/SOPS identity plumbing.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn auth_show_help_is_plaintext_from_repo_root() {
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
        stdout.contains("Print the plaintext approval token"),
        "expected plaintext show help output, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("--identity")
            && !stdout.contains(" age")
            && !stdout.contains("SOPS")
            && !stdout.contains("decrypt"),
        "age/SOPS identity plumbing must not appear in auth show help, got:\n{stdout}"
    );
}
