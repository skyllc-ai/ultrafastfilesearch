// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Human rendering of the `uffs --uninstall` analysis (task U-12, partial: the
//! resolution table; the artifact inventory + plan land in later milestones).

use super::resolve_order::{ResolutionState, StemResolution};

/// Print the discovered-binary resolution table: for each stem, every copy in
/// OS search order, with the one a bare command runs flagged ACTIVE.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_resolution_table(stems: &[StemResolution]) {
    if stems.is_empty() {
        println!("No UFFS binaries found in any install root or on PATH.");
        return;
    }
    println!("Discovered UFFS binaries (the copy a bare command runs is ACTIVE):\n");
    for stem in stems {
        println!("{}:", stem.stem);
        for copy in &stem.copies {
            let state = match copy.state {
                ResolutionState::Active => "ACTIVE",
                ResolutionState::Shadowed if copy.on_search_path => "shadowed",
                ResolutionState::Shadowed => "off-path",
            };
            let version = copy.version.as_deref().unwrap_or("-");
            println!(
                "  {state:<8}  {version:<9}  {channel:<9}  {scope:<7}  {dir}",
                channel = copy.channel.label(),
                scope = copy.scope.label(),
                dir = copy.dir.display(),
            );
        }
    }
}
