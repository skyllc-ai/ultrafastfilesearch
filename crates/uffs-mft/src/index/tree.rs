//! Tree-metric orchestration and self-healing entry points for `MftIndex`.

use super::*;

impl MftIndex {
    /// Compute tree metrics (descendants, treesize, `tree_allocated`) for all
    /// records.
    ///
    /// This method uses a bottom-up "leaf-peeling" algorithm (Kahn-style
    /// topological sort) to compute directory tree metrics in a single O(n)
    /// pass without recursion.
    ///
    /// The algorithm processes nodes in post-order (children before parents):
    /// 1. Build `parent_idx` and `pending_children` arrays
    /// 2. Initialize base metrics for each node (its own size)
    /// 3. Push all leaf nodes (`pending_children` == 0) to stack
    /// 4. Pop nodes from stack, accumulate into parent, decrement parent's
    ///    pending count
    /// 5. When parent's pending count reaches 0, push parent to stack
    ///
    /// This should be called once after the index is fully built (after parsing
    /// or merging).
    ///
    /// # Performance
    ///
    /// - Time: O(n) - each node processed exactly once
    /// - Space: O(n) - two temporary arrays (`parent_idx`, `pending_children`)
    /// - No recursion - guaranteed stack safety
    /// - Excellent cache locality - array-based, not `HashMap`
    /// - 2-3x faster than recursive memoization
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut index = MftIndex::new('C');
    /// // ... parse MFT records ...
    /// index.compute_tree_metrics(); // Compute tree metrics for all directories
    /// ```
    pub fn compute_tree_metrics(&mut self) {
        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics ENTER");
        self.compute_tree_metrics_impl(false);
        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics EXIT");
    }

    /// Compute tree metrics with optional debug output.
    ///
    /// When `debug` is true, prints detailed information about hardlink
    /// handling to stdout for debugging purposes.
    pub fn compute_tree_metrics_debug(&mut self) {
        self.compute_tree_metrics_impl(true);
    }
    /// Tree metrics algorithm.
    ///
    /// This uses the current tree algorithm with recursive DFS traversal
    /// starting from root (FRS 5) and the delta formula for proportional
    /// hardlink share calculation.
    ///
    /// # Self-Healing for LIVE Scans
    ///
    /// LIVE scans can occasionally produce incomplete child lists due to
    /// ordering/timing issues. If the first tree pass leaves any directories
    /// with `descendants == 0`, this method rebuilds the child lists from
    /// `FILE_NAME` parent references and reruns tree metrics.
    fn compute_tree_metrics_impl(&mut self, debug: bool) {
        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics_impl ENTER (first pass)");

        // Rebuild directory children from the name graph when requested.
        // This removes parse-order artifacts from live mode and stabilizes
        // name_index mapping.
        //
        // Gate this behind an env var so the production fast path is preserved.
        // Set UFFS_REBUILD_CHILDREN_ALWAYS=1 for validation runs.
        if std::env::var_os("UFFS_REBUILD_CHILDREN_ALWAYS").is_some() {
            tracing::debug!(
                "[TRIP] compute_tree_metrics_impl -> rebuilding children from names (forced via env)"
            );
            self.rebuild_children_from_names();
        }

        // Skip the orphan sweep when requested via env.
        // Set UFFS_SKIP_ORPHANS=1 to exclude orphans without resolved paths
        // from tree aggregation so only records reachable from ROOT through
        // visible FILE_NAME edges are included.
        let skip_orphans = std::env::var_os("UFFS_SKIP_ORPHANS").is_some();
        if skip_orphans {
            tracing::debug!(
                "[TRIP] compute_tree_metrics_impl -> skip_orphans=true (validation mode via env)"
            );
        }

        // First pass: compute tree metrics
        crate::tree_metrics::compute_tree_metrics(self, debug, skip_orphans);
        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics_impl -> first pass done");

        // Detect "unstamped directory" condition (LIVE scan symptom).
        // Directories should have descendants >= 1 (at least themselves).
        let bad_dir_count = self
            .records
            .iter()
            .filter(|rec| rec.stdinfo.is_directory() && rec.descendants == 0)
            .count();

        // Also check root specifically for treesize=0 (belt-and-suspenders).
        // Root should always have treesize > 0 if there are any files on the volume.
        let root_looks_bad = self
            .frs_to_idx_opt(5)
            .and_then(|root_idx| self.records.get(root_idx))
            .is_some_and(|root| {
                root.stdinfo.is_directory() && (root.descendants == 0 || root.treesize == 0)
            });

        tracing::debug!(
            bad_dir_count,
            root_looks_bad,
            "[TRIP] MftIndex::compute_tree_metrics_impl -> self-heal check"
        );

        if bad_dir_count != 0 || root_looks_bad {
            tracing::debug!("[TRIP] MftIndex::compute_tree_metrics_impl -> SELF-HEAL TRIGGERED");
            tracing::warn!(
                bad_dir_count,
                root_looks_bad,
                "[tree] unstamped directories or root after first pass; \
                 rebuilding child lists from names and rerunning"
            );

            // Rebuild child lists from FILE_NAME parent references
            tracing::debug!(
                "[TRIP] MftIndex::compute_tree_metrics_impl -> calling rebuild_children_from_names"
            );
            self.rebuild_children_from_names();

            // Second pass: recompute tree metrics with fixed child lists
            tracing::debug!(
                "[TRIP] MftIndex::compute_tree_metrics_impl -> second pass (after self-heal)"
            );
            crate::tree_metrics::compute_tree_metrics(self, debug, skip_orphans);
        }

        // Post-tree diagnostic: log which directories STILL have descendants==0
        // after all passes (including self-heal). This runs in RELEASE builds
        // to help diagnose LIVE scan issues.
        // Interpretation:
        // - If bad_dirs is non-empty here → Failure mode A/C (not stamped)
        // - If bad_dirs is empty here but CSV shows bad rows → Failure mode B (reset
        //   after compute)
        let final_bad_dirs: Vec<_> = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, rec)| rec.stdinfo.is_directory() && rec.descendants == 0)
            .map(|(idx, rec)| {
                (
                    idx,
                    rec.frs,
                    rec.first_child,
                    rec.name_count,
                    rec.total_stream_count,
                    rec.stdinfo.is_reparse(),
                )
            })
            .collect();

        if !final_bad_dirs.is_empty() {
            tracing::warn!(
                bad_dir_count = final_bad_dirs.len(),
                "[tree] FINAL: directories with descendants==0 after all tree metrics passes"
            );
            // Log first 10 bad directories for debugging
            for (idx, frs, first_child, name_count, stream_count, is_reparse) in
                final_bad_dirs.iter().take(10)
            {
                tracing::warn!(
                    idx,
                    frs,
                    first_child,
                    name_count,
                    stream_count,
                    is_reparse,
                    "[tree] FINAL: bad directory details"
                );
            }
        }

        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics_impl EXIT");
    }
}
