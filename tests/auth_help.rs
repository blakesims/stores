//! T078: `stores auth show --help` renders from the repository root and no
//! longer advertises age/SOPS identity plumbing.

use std::path::{Path, PathBuf};
use std::process::Command;

fn run_ok(bin: &str, cwd: &Path, args: &[&str], token_dir: Option<&Path>) {
    let mut cmd = Command::new(bin);
    cmd.current_dir(cwd).args(args);
    if let Some(dir) = token_dir {
        cmd.env("STORES_TOKEN_DIR", dir);
    }
    let output = cmd.output().expect("failed to invoke stores binary");
    assert!(
        output.status.success(),
        "stores {:?} exited non-zero: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

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
    let removed_identity_flag = ["--", "identity"].concat();
    assert!(
        !stdout.contains(&removed_identity_flag)
            && !stdout.contains(" age")
            && !stdout.contains("SOPS")
            && !stdout.contains("decrypt"),
        "age/SOPS identity plumbing must not appear in auth show help, got:\n{stdout}"
    );
}

#[test]
fn ratify_amend_rejects_non_human_even_with_valid_token() {
    let bin = env!("CARGO_BIN_EXE_stores");
    let tmp = tempfile::tempdir().expect("tempdir");
    let token_dir = tmp.path().join("tokens");
    std::fs::create_dir_all(&token_dir).expect("token dir");
    std::fs::write(
        token_dir.join("approve.token.hash"),
        "397a2a9c5bf5e2ccec38c2596b682bb1bd05fe6e4ecea6c10cf42755ff225403\n",
    )
    .expect("token hash");

    run_ok(bin, tmp.path(), &["setup"], Some(&token_dir));
    run_ok(
        bin,
        tmp.path(),
        &[
            "architecture-reviews",
            "add",
            "--kind",
            "amend",
            "--summary",
            "amend token test",
            "--cascade-decisions",
            r#"[{"target":"docs/heart-and-architect.md","decision":"update"}]"#,
            "--invoker",
            "ai_with_human",
        ],
        Some(&token_dir),
    );
    run_ok(
        bin,
        tmp.path(),
        &[
            "architecture-reviews",
            "claim-review",
            "A001",
            "--invoker",
            "ai_with_human",
        ],
        Some(&token_dir),
    );
    run_ok(
        bin,
        tmp.path(),
        &[
            "architecture-reviews",
            "issue-verdict",
            "A001",
            "--kind",
            "amend",
            "--verdict",
            "propose_doctrine_update",
            "--rationale",
            "x",
            "--cascade-decisions",
            r#"[{"target":"docs/heart-and-architect.md","decision":"update"}]"#,
            "--invoker",
            "ai_with_human",
        ],
        Some(&token_dir),
    );

    for actor in ["ai_with_human", "ai_autonomous"] {
        let output = Command::new(bin)
            .current_dir(tmp.path())
            .env("STORES_TOKEN_DIR", &token_dir)
            .args([
                "architecture-reviews",
                "ratify-amend",
                "A001",
                "--invoker",
                actor,
                "--approve-token",
                "valid-token",
            ])
            .output()
            .expect("ratify-amend invocation");
        assert!(
            !output.status.success(),
            "ratify-amend unexpectedly accepted {actor} with valid token"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("requires invoker actor human"),
            "ratify-amend {actor} stderr did not cite human-only rule:\n{stderr}"
        );
    }

    run_ok(
        bin,
        tmp.path(),
        &[
            "architecture-reviews",
            "ratify-amend",
            "A001",
            "--invoker",
            "human",
            "--approve-token",
            "valid-token",
        ],
        Some(&token_dir),
    );
}
