use std::process::Command;

fn git_stdout(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn emit_git_rerun_inputs() {
    // HEAD usually contains only `ref: refs/heads/<branch>`; the branch ref is
    // what advances on same-branch commits, so watch both plus packed-refs.
    if let Some(head_path) = git_stdout(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }
    if let Some(branch_ref) = git_stdout(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(branch_path) = git_stdout(&["rev-parse", "--git-path", &branch_ref]) {
            println!("cargo:rerun-if-changed={branch_path}");
        }
    }
    if let Some(packed_refs_path) = git_stdout(&["rev-parse", "--git-path", "packed-refs"]) {
        println!("cargo:rerun-if-changed={packed_refs_path}");
    }
}

fn main() {
    let sha = git_stdout(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=VERGEN_GIT_SHA={sha}");
    emit_git_rerun_inputs();
}
