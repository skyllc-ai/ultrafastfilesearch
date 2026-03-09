//! Optional conversion from `MftIndex` into Polars data frames.

use super::*;

// Polars DataFrame Conversion (optional, on-demand)
// ============================================================================

impl MftIndex {
    /// Convert the lean index to a Polars `DataFrame`.
    ///
    /// This is an **optional** conversion for when you need:
    /// - Complex SQL-like queries
    /// - Analytics and aggregations
    /// - Export to Parquet/CSV
    ///
    /// For simple searches, use the lean index directly (faster).
    ///
    /// # Cross-Platform
    ///
    /// This method is cross-platform and works on all platforms.
    ///
    /// # Output Format
    ///
    /// Outputs **one row per FRS** (File Record Segment) - this is the true
    /// `MftIndex` representation. Hard links and ADS are NOT expanded.
    ///
    /// For search results with expansion across hard links and ADS, use
    /// `IndexQuery::collect()`.
    ///
    /// # Tree Metrics
    ///
    /// The `DataFrame` includes tree metrics (descendants, treesize,
    /// `tree_allocated`) that are pre-computed in the `MftIndex` via
    /// `compute_tree_metrics()`.
    ///
    /// # Errors
    ///
    /// Returns an error if `DataFrame` construction fails.
    #[expect(clippy::cast_possible_truncation, reason = "index counts fit in usize")]
    #[expect(
        clippy::too_many_lines,
        reason = "DataFrame construction has many columns"
    )]
    pub fn to_dataframe(&self) -> crate::Result<uffs_polars::DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};
        let n = self.records.len();
        // Pre-allocate all column vectors (35 columns in v5)
        let (mut frs, mut seq, mut lsn, mut parent, mut name, mut ns) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (mut size, mut alloc) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let (mut si_c, mut si_m, mut si_a, mut si_mft) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (mut usn, mut sec_id, mut own_id): (Vec<u64>, Vec<u32>, Vec<u32>) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (mut fn_c, mut fn_m, mut fn_a, mut fn_mft) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (
            mut dir,
            mut ro,
            mut hid,
            mut sys,
            mut arc,
            mut cmp,
            mut enc,
            mut spr,
            mut rp,
            mut off,
            mut nix,
            mut tmp,
        ) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (mut flags, mut lnk, mut str, mut path): (Vec<u16>, Vec<u16>, Vec<u16>, Vec<String>) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        // Tree metrics columns
        let (mut descendants, mut treesize, mut tree_allocated): (Vec<u32>, Vec<u64>, Vec<u64>) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        // File type (extension) column - first-class citizen like name, size, etc.
        let mut file_type: Vec<String> = Vec::with_capacity(n);
        let (mut reparse_tag, mut is_resident): (Vec<u32>, Vec<bool>) =
            (Vec::with_capacity(n), Vec::with_capacity(n));
        // P3 forensic columns - only allocate if forensic mode is enabled
        let (mut is_deleted, mut is_corrupt, mut is_extension, mut base_frs_col) =
            if self.forensic_mode {
                (
                    Vec::with_capacity(n),
                    Vec::with_capacity(n),
                    Vec::with_capacity(n),
                    Vec::with_capacity(n),
                )
            } else {
                (Vec::new(), Vec::new(), Vec::new(), Vec::new())
            };
        // Extract data from records
        for rec in &self.records {
            frs.push(rec.frs);
            seq.push(rec.sequence_number);
            lsn.push(rec.lsn);
            parent.push(rec.first_name.parent_frs);
            name.push(self.record_name(rec).to_owned());
            ns.push(rec.namespace);
            size.push(rec.first_stream.size.length);
            alloc.push(rec.first_stream.size.allocated);
            si_c.push(rec.stdinfo.created);
            si_m.push(rec.stdinfo.modified);
            si_a.push(rec.stdinfo.accessed);
            si_mft.push(rec.stdinfo.mft_changed);
            usn.push(rec.stdinfo.usn);
            sec_id.push(rec.stdinfo.security_id);
            own_id.push(rec.stdinfo.owner_id);
            fn_c.push(rec.fn_created);
            fn_m.push(rec.fn_modified);
            fn_a.push(rec.fn_accessed);
            fn_mft.push(rec.fn_mft_changed);
            let si = &rec.stdinfo;
            dir.push(si.is_directory());
            ro.push(si.is_readonly());
            hid.push(si.is_hidden());
            sys.push(si.is_system());
            arc.push(si.is_archive());
            cmp.push(si.is_compressed());
            enc.push(si.is_encrypted());
            spr.push(si.is_sparse());
            rp.push(si.is_reparse());
            off.push(si.is_offline());
            nix.push(si.is_not_indexed());
            tmp.push(si.is_temporary());
            flags.push(si.to_attributes() as u16);
            lnk.push(rec.name_count);
            str.push(rec.stream_count);
            reparse_tag.push(rec.reparse_tag);
            is_resident.push(rec.first_stream.is_resident());
            // File type (extension) - lookup from ExtensionTable using extension_id
            let ext_id = rec.first_name.name.extension_id();
            let ext_str = self.extensions.get_extension(ext_id).unwrap_or("");
            file_type.push(ext_str.to_owned());
            // P3 forensic fields - only populate if forensic mode is enabled
            if self.forensic_mode {
                is_deleted.push(rec.is_deleted());
                is_corrupt.push(rec.is_corrupt());
                is_extension.push(rec.is_extension());
                base_frs_col.push(rec.base_frs);
            }
            // Tree metrics (pre-computed via compute_tree_metrics())
            // Use the tree_metrics() method as the single source of truth (Fix #3)
            let (desc, ts, ta) = rec.tree_metrics();
            descendants.push(desc);
            treesize.push(ts);
            tree_allocated.push(ta);
            path.push(self.build_path(rec.frs));
        }
        // Build DataFrame
        let dt = DataType::Datetime(TimeUnit::Microseconds, None);
        // Base columns (37 without forensic, 41 with forensic)
        let mut cols = vec![
            Series::new("frs".into(), frs).into_column(),
            Series::new("sequence_number".into(), seq).into_column(),
            Series::new("lsn".into(), lsn).into_column(),
            Series::new("parent_frs".into(), parent).into_column(),
            Series::new("name".into(), name).into_column(),
            Series::new("type".into(), file_type).into_column(),
            Series::new("namespace".into(), ns).into_column(),
            Series::new("size".into(), size).into_column(),
            Series::new("allocated_size".into(), alloc).into_column(),
            Series::new("si_created".into(), si_c)
                .cast(&dt)?
                .into_column(),
            Series::new("si_modified".into(), si_m)
                .cast(&dt)?
                .into_column(),
            Series::new("si_accessed".into(), si_a)
                .cast(&dt)?
                .into_column(),
            Series::new("si_mft_changed".into(), si_mft)
                .cast(&dt)?
                .into_column(),
            Series::new("usn".into(), usn).into_column(),
            Series::new("security_id".into(), sec_id).into_column(),
            Series::new("owner_id".into(), own_id).into_column(),
            Series::new("fn_created".into(), fn_c)
                .cast(&dt)?
                .into_column(),
            Series::new("fn_modified".into(), fn_m)
                .cast(&dt)?
                .into_column(),
            Series::new("fn_accessed".into(), fn_a)
                .cast(&dt)?
                .into_column(),
            Series::new("fn_mft_changed".into(), fn_mft)
                .cast(&dt)?
                .into_column(),
            Series::new("is_directory".into(), dir).into_column(),
            Series::new("is_readonly".into(), ro).into_column(),
            Series::new("is_hidden".into(), hid).into_column(),
            Series::new("is_system".into(), sys).into_column(),
            Series::new("is_archive".into(), arc).into_column(),
            Series::new("is_compressed".into(), cmp).into_column(),
            Series::new("is_encrypted".into(), enc).into_column(),
            Series::new("is_sparse".into(), spr).into_column(),
            Series::new("is_reparse".into(), rp).into_column(),
            Series::new("is_offline".into(), off).into_column(),
            Series::new("is_not_indexed".into(), nix).into_column(),
            Series::new("is_temporary".into(), tmp).into_column(),
            Series::new("reparse_tag".into(), reparse_tag).into_column(),
            Series::new("is_resident".into(), is_resident).into_column(),
        ];
        // P3 forensic columns - only include when forensic_mode is enabled
        if self.forensic_mode {
            cols.push(Series::new("is_deleted".into(), is_deleted).into_column());
            cols.push(Series::new("is_corrupt".into(), is_corrupt).into_column());
            cols.push(Series::new("is_extension".into(), is_extension).into_column());
            cols.push(Series::new("base_frs".into(), base_frs_col).into_column());
        }
        // Remaining columns (always included)
        cols.push(Series::new("flags".into(), flags).into_column());
        cols.push(Series::new("link_count".into(), lnk).into_column());
        cols.push(Series::new("stream_count".into(), str).into_column());
        // Tree metrics (pre-computed via compute_tree_metrics())
        cols.push(Series::new("descendants".into(), descendants).into_column());
        cols.push(Series::new("treesize".into(), treesize).into_column());
        cols.push(Series::new("tree_allocated".into(), tree_allocated).into_column());
        cols.push(Series::new("path".into(), path).into_column());

        uffs_polars::DataFrame::new_infer_height(cols).map_err(crate::MftError::from)
    }
}
