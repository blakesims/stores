/// Build-time package version surfaced in daemon stale-executable logs.
pub const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Optional build metadata supplied by the build environment.
pub const BUILD_METADATA: Option<&str> = option_env!("STORES_BUILD_METADATA");

/// Optional git SHA supplied by the build environment.
pub const BUILD_GIT_SHA: Option<&str> = match option_env!("STORES_GIT_SHA") {
    Some(sha) => Some(sha),
    None => option_env!("VERGEN_GIT_SHA"),
};

/// Optional build timestamp supplied by build.rs or the build environment.
pub const BUILD_TIMESTAMP: Option<&str> = option_env!("STORES_BUILD_TIMESTAMP");

fn known(value: Option<&str>) -> &str {
    match value {
        Some("") | None => "unknown",
        Some(value) => value,
    }
}

/// Rich build identity for operator diagnostics.
pub fn build_identity_diagnostics() -> String {
    let mut parts = vec![
        format!("version={}", BUILD_VERSION),
        format!("git_sha={}", known(BUILD_GIT_SHA)),
        format!("build_timestamp={}", known(BUILD_TIMESTAMP)),
    ];
    if let Some(metadata) = BUILD_METADATA.filter(|m| !m.is_empty()) {
        parts.push(format!("metadata={metadata}"));
    }
    parts.join(" ")
}
