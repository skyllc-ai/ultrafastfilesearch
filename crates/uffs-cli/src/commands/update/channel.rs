// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Per-root install-channel classification — pure, path-based, and
//! testable (Phase A.3 of the self-update design).
//!
//! Each discovered install *root* is classified independently: a `WinGet`
//! root and a hand-placed root can coexist in one update run, so the
//! update strategy is chosen per root, not globally.

use std::path::Path;

use super::model::{Channel, Scope};

/// Classify the install [`Channel`] and [`Scope`] of a directory purely
/// from its path.
///
/// Heuristics (case-insensitive):
/// - `…\WinGet\Packages\…` → [`Channel::WinGet`] (scope from the path root).
/// - a `…\target\debug\…` or `…\target\release\…` segment →
///   [`Channel::DevBuild`].
/// - anything else → [`Channel::Unmanaged`].
pub(crate) fn classify(dir: &Path) -> (Channel, Scope) {
    let lower = dir.to_string_lossy().to_ascii_lowercase();
    if lower.contains("\\winget\\packages\\") || lower.contains("/winget/packages/") {
        return (Channel::WinGet, winget_scope(&lower));
    }
    if is_dev_build(&lower) {
        return (Channel::DevBuild, Scope::Unknown);
    }
    (Channel::Unmanaged, Scope::Unknown)
}

/// Infer `WinGet` install scope from a lower-cased path.
///
/// Machine-wide installs live under `Program Files`; per-user installs
/// under the local app-data tree.
fn winget_scope(lower: &str) -> Scope {
    if lower.contains("program files") {
        Scope::Machine
    } else if lower.contains("\\appdata\\local\\") || lower.contains("/appdata/local/") {
        Scope::User
    } else {
        Scope::Unknown
    }
}

/// Return `true` when the lower-cased path contains a cargo
/// `target/{debug,release}` segment — either as the final segment
/// (`…\target\release`) or with children (`…/target/debug/deps`).
///
/// Separators are normalised to `/` first; the marker must be followed
/// by end-of-string or `/` so `target/release-notes` does not match.
fn is_dev_build(lower: &str) -> bool {
    let norm = lower.replace('\\', "/");
    ["/target/debug", "/target/release"].iter().any(|marker| {
        norm.find(marker).is_some_and(|pos| {
            norm.get(pos + marker.len()..)
                .is_some_and(|after| after.is_empty() || after.starts_with('/'))
        })
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Channel, Scope, classify};

    #[test]
    fn winget_user_scope() {
        let path = Path::new(
            r"C:\Users\me\AppData\Local\Microsoft\WinGet\Packages\Sky.UFFS_abc\uffs-windows-x64",
        );
        assert_eq!(classify(path), (Channel::WinGet, Scope::User));
    }

    #[test]
    fn winget_machine_scope() {
        let path = Path::new(r"C:\Program Files\WinGet\Packages\Sky.UFFS_abc");
        assert_eq!(classify(path), (Channel::WinGet, Scope::Machine));
    }

    #[test]
    fn unmanaged_plain_dir() {
        assert_eq!(
            classify(Path::new(r"C:\uffs-test")),
            (Channel::Unmanaged, Scope::Unknown)
        );
    }

    #[test]
    fn dev_build_target_tree() {
        assert_eq!(
            classify(Path::new(r"D:\src\uffs\target\release")),
            (Channel::DevBuild, Scope::Unknown)
        );
        assert_eq!(
            classify(Path::new("/home/me/uffs/target/debug")),
            (Channel::DevBuild, Scope::Unknown)
        );
    }

    #[test]
    fn forward_slash_winget() {
        let path = Path::new("/c/Users/me/AppData/Local/Microsoft/WinGet/Packages/Sky.UFFS_x");
        assert_eq!(classify(path), (Channel::WinGet, Scope::User));
    }
}
