/// Build-time package version surfaced in daemon stale-executable logs.
pub const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Optional build metadata supplied by the build environment.
pub const BUILD_METADATA: Option<&str> = option_env!("STORES_BUILD_METADATA");

/// Optional git SHA supplied by the build environment.
pub const BUILD_GIT_SHA: Option<&str> = option_env!("STORES_GIT_SHA");

/// Human-readable build identity. The stale-exec operator log currently uses
/// the package version so the line remains stable and grep-friendly.
pub fn build_identity() -> &'static str {
    let _ = BUILD_METADATA;
    let _ = BUILD_GIT_SHA;
    BUILD_VERSION
}
