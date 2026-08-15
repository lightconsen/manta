//! Map the running platform to a release tarball target name.
//!
//! Mirrors the mapping in `scripts/install.sh` so the same asset naming is
//! used everywhere. Returns `None` for platforms without published binaries.

/// Map an OS/architecture pair to a release asset target
/// (e.g. `linux-amd64`, `macos-arm64`).
///
/// Pure function over string inputs so the mapping is unit-testable without
/// depending on the build target.
pub fn target_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("linux-amd64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("macos-amd64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        _ => None,
    }
}

/// Resolve the asset target for the currently running binary.
pub fn asset_target() -> Option<&'static str> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_for_supported_platforms() {
        assert_eq!(target_for("linux", "x86_64"), Some("linux-amd64"));
        assert_eq!(target_for("linux", "aarch64"), Some("linux-arm64"));
        assert_eq!(target_for("macos", "x86_64"), Some("macos-amd64"));
        assert_eq!(target_for("macos", "aarch64"), Some("macos-arm64"));
    }

    #[test]
    fn target_for_unsupported_platforms() {
        assert_eq!(target_for("linux", "i686"), None);
        assert_eq!(target_for("windows", "x86_64"), None);
        assert_eq!(target_for("freebsd", "aarch64"), None);
    }

    #[test]
    fn current_platform_resolves_or_is_known_unsupported() {
        // On a supported platform this must resolve; the assertion guards
        // against a build target regression in the mapping.
        match std::env::consts::OS {
            "linux" | "macos" => assert!(asset_target().is_some()),
            _ => assert!(asset_target().is_none()),
        }
    }
}
