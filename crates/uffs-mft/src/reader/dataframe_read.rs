//! DataFrame-oriented read entrypoints and the primary MFT read pipeline.

#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use tracing::{debug, info, warn};
use uffs_polars::DataFrame;

#[cfg(windows)]
use super::read_mode::dataframe_effective_mode;
use super::{MftProgress, MftReader};
#[cfg(windows)]
use super::MftReadMode;
use crate::error::{MftError, Result};

impl MftReader {

    /// Read the entire MFT and return as a `DataFrame`.
    ///
    /// This method reads all MFT records and constructs a Polars `DataFrame`
    /// with the standard schema (frs, `parent_frs`, name, size, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub fn read_all(&self) -> Result<DataFrame> {
        self.read_mft_internal(None::<fn(MftProgress)>)
    }

    /// Read the entire MFT (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub const fn read_all(&self) -> Result<DataFrame> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT with progress callback.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called periodically with progress updates
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        self.read_mft_internal(Some(callback))
    }

    /// Read MFT with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub fn read_with_progress<F>(&self, _callback: F) -> Result<DataFrame>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal MFT reading implementation.
    ///
    /// This implementation uses the high-performance parallel reader with:
    /// 1. Extent-aware reading for fragmented MFTs
    /// 2. Bitmap-based cluster skipping (matching the historical baseline)
    /// 3. Parallel record processing using Rayon
    /// 4. Large batch I/O (4-8 MB) for reduced syscall overhead
    /// 5. Drive-type aware tuning (SSD vs HDD)
    #[cfg(windows)]
    fn read_mft_internal<F>(&self, callback: Option<F>) -> Result<DataFrame>
    where
        F: Fn(MftProgress),
    {
        use crate::io::{MftExtentMap, ParallelMftReader};
        use crate::platform::detect_drive_type;

        info!(volume = %self.volume, "Starting MFT read");

        let start_time = Instant::now();
        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();

        // Detect drive type for optimal I/O tuning
        let drive_type = detect_drive_type(self.volume);
        info!(
            volume = %self.volume,
            drive_type = ?drive_type,
            chunk_size_mb = drive_type.optimal_chunk_size() / (1024 * 1024),
            "🚀 Drive type detected for I/O optimization"
        );

        debug!(
            record_size,
            bytes_per_cluster = volume_data.bytes_per_cluster,
            mft_valid_data_length = volume_data.mft_valid_data_length,
            "Volume data retrieved"
        );

        // Get MFT extents for fragmented MFT support
        let extents = self.handle.get_mft_extents().unwrap_or_else(|e| {
            warn!(error = ?e, "Failed to get MFT extents, using fallback");
            // Fallback to single contiguous extent
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        info!(num_extents = extents.len(), "MFT extents retrieved");

        // Create extent map
        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);

        let total_records = extent_map.total_records();
        info!(total_records, "Total MFT records to read");

        // Try to get the MFT bitmap for optimization (if enabled)
        let bitmap = if self.use_bitmap {
            let bm = self.handle.get_mft_bitmap().ok();
            if let Some(ref b) = bm {
                let in_use = b.count_in_use();
                info!(
                    in_use_records = in_use,
                    skip_percentage = 100.0 - (in_use as f64 / total_records as f64 * 100.0),
                    "MFT bitmap loaded - will skip unused records"
                );
            } else {
                debug!("No MFT bitmap available - reading all records");
            }
            bm
        } else {
            info!("Bitmap optimization DISABLED (--no-bitmap) - reading ALL records");
            None
        };

        // Report initial progress
        if let Some(ref cb) = callback {
            cb(MftProgress {
                records_read: 0,
                total_records: Some(total_records),
                bytes_read: 0,
                elapsed: start_time.elapsed(),
            });
        }

        // M2 9.1-9.3: Select reader based on mode
        // Historical benchmark insight: "read all then parse" is faster than pipelining
        // even on HDD because: no context switching, CPU cache stays hot, no
        // channel overhead, OS can optimize continuous sequential reads better.
        // For read_all() (returns Vec<ParsedRecord>), use SlidingIocp for IOCP-based
        // I/O.
        let effective_mode = dataframe_effective_mode(self.mode, drive_type);

        info!(
            mode = %effective_mode,
            "🚀 Using read mode"
        );

        let handle = self.handle.raw_handle();
        let total_bytes = total_records * u64::from(record_size);

        // Read using the selected mode
        let parsed_records = match effective_mode {
            MftReadMode::Parallel | MftReadMode::Auto => {
                // Parallel mode: read all chunks then parse in parallel (best for SSD)
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_parallel_with_progress(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    parallel_reader
                        .read_all_parallel_with_progress::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::Streaming => {
                // Streaming mode: sequential reads with immediate parsing (lower memory)
                let mut streaming_reader =
                    crate::io::StreamingMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    streaming_reader.read_all_streaming(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    streaming_reader.read_all_streaming::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::Prefetch => {
                // Prefetch mode: double-buffered reads for I/O overlap (good for HDD)
                let prefetch_reader =
                    crate::io::PrefetchMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    prefetch_reader.read_all_prefetch(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    prefetch_reader.read_all_prefetch::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::Pipelined => {
                // Pipelined mode: true I/O+CPU overlap with separate threads (best for HDD)
                let pipelined_reader =
                    crate::io::PipelinedMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    pipelined_reader.read_all_pipelined(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    pipelined_reader.read_all_pipelined::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::PipelinedParallel => {
                // Pipelined parallel mode: I/O overlap + multi-core parsing (best for HDD)
                let pipelined_reader =
                    crate::io::PipelinedMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    pipelined_reader.read_all_pipelined_parallel(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    pipelined_reader
                        .read_all_pipelined_parallel::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::IocpParallel => {
                // IOCP parallel mode: Multiple overlapped reads in flight (best for HDD)
                // IOCP requires FILE_FLAG_OVERLAPPED, so we open a separate handle
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let iocp_reader = crate::io::IocpMftReader::new(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    iocp_reader.read_all_iocp(
                        overlapped_handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )
                } else {
                    iocp_reader.read_all_iocp::<fn(u64, u64)>(overlapped_handle, true, None)
                };

                // Close the overlapped handle
                // SAFETY: overlapped_handle is a valid handle opened by open_overlapped_handle
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::Bulk => {
                // Bulk mode: C++ style "read all, then parse"
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_bulk(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    parallel_reader.read_all_bulk::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::BulkIocp => {
                // Bulk IOCP mode: True C++ style - queues ALL reads to IOCP at once
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_bulk_iocp(
                        overlapped_handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )
                } else {
                    parallel_reader.read_all_bulk_iocp::<fn(u64, u64)>(
                        overlapped_handle,
                        true,
                        None,
                    )
                };

                // Close the overlapped handle
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::SlidingIocp => {
                // Sliding window IOCP mode: C++ style with 2 reads in flight
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_sliding_window_iocp(
                        overlapped_handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )
                } else {
                    parallel_reader.read_all_sliding_window_iocp::<fn(u64, u64)>(
                        overlapped_handle,
                        true,
                        None,
                    )
                };

                // Close the overlapped handle
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::SlidingIocpInline => {
                // SlidingIocpInline is designed for direct index building.
                // For read_mft_internal (which returns Vec<ParsedRecord>), fall back to
                // SlidingIocp.
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_sliding_window_iocp(
                        overlapped_handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )
                } else {
                    parallel_reader.read_all_sliding_window_iocp::<fn(u64, u64)>(
                        overlapped_handle,
                        true,
                        None,
                    )
                };

                // Close the overlapped handle
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
        };

        // Add placeholder records for missing parent directories.
        // This matches the legacy output behavior where `at()` creates placeholder
        // records for any referenced FRS that hasn't been seen yet.
        // Can be disabled with `with_add_placeholders(false)` for ~15% speedup.
        let mut parsed_records = parsed_records;
        if self.add_placeholders {
            let placeholders_added =
                crate::io::add_missing_parent_placeholders_to_vec(&mut parsed_records);
            if placeholders_added > 0 {
                debug!(
                    placeholders_added,
                    "Added placeholder records for path resolution"
                );
            }
        }

        let read_elapsed = start_time.elapsed();
        let records_parsed_count = parsed_records.len();
        let throughput_mb_s = if read_elapsed.as_secs_f64() > 0.0 {
            (total_bytes as f64 / (1024.0 * 1024.0)) / read_elapsed.as_secs_f64()
        } else {
            0.0
        };
        let records_per_sec = if read_elapsed.as_secs_f64() > 0.0 {
            records_parsed_count as f64 / read_elapsed.as_secs_f64()
        } else {
            0.0
        };

        info!(
            records_parsed = records_parsed_count,
            total_records,
            elapsed_ms = read_elapsed.as_millis(),
            throughput_mb_s = format!("{:.1}", throughput_mb_s),
            records_per_sec = format!("{:.0}", records_per_sec),
            "✅ Parallel read complete"
        );

        // Report final progress
        if let Some(ref cb) = callback {
            cb(MftProgress {
                records_read: total_records,
                total_records: Some(total_records),
                bytes_read: total_bytes,
                elapsed: start_time.elapsed(),
            });
        }

        // M1 8.3 OPTIMIZATION: Fuse stats computation with DataFrame building
        // This eliminates one full pass over all records (was ~5-10% of DF build time)
        //
        // With expand_links=true (default), we expand hard links to separate rows.
        // Stats are computed per unique FRS (before expansion).
        let expand_links = self.expand_links;
        let base_capacity = parsed_records.len();
        // If expanding links, estimate ~20% more rows for hard links
        let capacity = if expand_links {
            (base_capacity as f64 * 1.2) as usize
        } else {
            base_capacity
        };
        let mut stats = MftStats::default();

        // Pre-allocate all column vectors
        let mut frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut created_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut name_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_archive_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_offline_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_not_indexed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_temporary_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_integrity_stream_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_no_scrub_data_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_pinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_unpinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_virtual_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut flags_vec: Vec<u32> = Vec::with_capacity(capacity);

        // Single pass: build columns AND compute stats simultaneously
        for parsed in parsed_records {
            let name_count = parsed.name_count();
            let stream_count = parsed.stream_count();

            // Accumulate stats inline (no separate loop!)
            // Stats are per unique FRS, not per expanded link
            if parsed.is_directory {
                stats.dir_count += 1;
            } else {
                stats.file_count += 1;
                stats.total_file_size = stats.total_file_size.saturating_add(parsed.size);
                stats.total_allocated_size = stats
                    .total_allocated_size
                    .saturating_add(parsed.allocated_size);
            }
            stats.hidden_count += u64::from(parsed.std_info.is_hidden);
            stats.system_count += u64::from(parsed.std_info.is_system);
            stats.compressed_count += u64::from(parsed.std_info.is_compressed);
            stats.encrypted_count += u64::from(parsed.std_info.is_encrypted);
            stats.sparse_count += u64::from(parsed.std_info.is_sparse);
            stats.reparse_count += u64::from(parsed.std_info.is_reparse);
            stats.multi_stream_count += u64::from(stream_count > 1);
            stats.multi_name_count += u64::from(name_count > 1);

            // Build column vectors - expand (names × streams) if enabled
            if expand_links {
                // Get names to iterate over
                let names: Vec<_> = if parsed.names.is_empty() {
                    vec![crate::ntfs::NameInfo {
                        name: parsed.name.clone(),
                        parent_frs: parsed.parent_frs,
                        namespace: 3,
                        fn_created: parsed.fn_created,
                        fn_modified: parsed.fn_modified,
                        fn_accessed: parsed.fn_accessed,
                        fn_mft_changed: parsed.fn_mft_changed,
                        source_frs: parsed.frs,
                    }]
                } else {
                    parsed.names.clone()
                };

                // Get streams to iterate over
                let streams: Vec<_> = if parsed.streams.is_empty() {
                    vec![StreamInfo {
                        name: String::new(),
                        size: parsed.size,
                        allocated_size: parsed.allocated_size,
                        is_sparse: false,
                        is_compressed: false,
                        is_resident: false,
                    }]
                } else {
                    parsed.streams.clone()
                };

                // Expand: one row per (name × stream) combination
                for name_info in &names {
                    for stream_info in &streams {
                        frs_vec.push(parsed.frs);
                        parent_frs_vec.push(name_info.parent_frs);
                        name_vec.push(name_info.name.clone());
                        // Use stream-specific size for ADS
                        let (size, alloc) = if stream_info.name.is_empty() {
                            (parsed.size, parsed.allocated_size)
                        } else {
                            (stream_info.size, stream_info.allocated_size)
                        };
                        size_vec.push(size);
                        allocated_size_vec.push(alloc);
                        created_vec.push(parsed.std_info.created);
                        modified_vec.push(parsed.std_info.modified);
                        accessed_vec.push(parsed.std_info.accessed);
                        mft_changed_vec.push(parsed.std_info.mft_changed);
                        is_directory_vec.push(parsed.is_directory);
                        name_count_vec.push(1);
                        stream_count_vec.push(1);
                        stream_name_vec.push(stream_info.name.clone());
                        is_readonly_vec.push(parsed.std_info.is_readonly);
                        is_hidden_vec.push(parsed.std_info.is_hidden);
                        is_system_vec.push(parsed.std_info.is_system);
                        is_archive_vec.push(parsed.std_info.is_archive);
                        is_compressed_vec.push(parsed.std_info.is_compressed);
                        is_encrypted_vec.push(parsed.std_info.is_encrypted);
                        is_sparse_vec.push(parsed.std_info.is_sparse);
                        is_reparse_vec.push(parsed.std_info.is_reparse);
                        is_offline_vec.push(parsed.std_info.is_offline);
                        is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
                        is_temporary_vec.push(parsed.std_info.is_temporary);
                        is_integrity_stream_vec.push(parsed.std_info.is_integrity_stream);
                        is_no_scrub_data_vec.push(parsed.std_info.is_no_scrub_data);
                        is_pinned_vec.push(parsed.std_info.is_pinned);
                        is_unpinned_vec.push(parsed.std_info.is_unpinned);
                        is_virtual_vec.push(parsed.std_info.is_virtual);
                        flags_vec.push(parsed.std_info.to_raw_flags());
                    }
                }
            } else {
                // No expansion: one row per FRS (use primary name)
                frs_vec.push(parsed.frs);
                parent_frs_vec.push(parsed.parent_frs);
                name_vec.push(parsed.name);
                size_vec.push(parsed.size);
                allocated_size_vec.push(parsed.allocated_size);
                created_vec.push(parsed.std_info.created);
                modified_vec.push(parsed.std_info.modified);
                accessed_vec.push(parsed.std_info.accessed);
                mft_changed_vec.push(parsed.std_info.mft_changed);
                is_directory_vec.push(parsed.is_directory);
                name_count_vec.push(name_count);
                stream_count_vec.push(stream_count);
                stream_name_vec.push(String::new()); // Default stream
                is_readonly_vec.push(parsed.std_info.is_readonly);
                is_hidden_vec.push(parsed.std_info.is_hidden);
                is_system_vec.push(parsed.std_info.is_system);
                is_archive_vec.push(parsed.std_info.is_archive);
                is_compressed_vec.push(parsed.std_info.is_compressed);
                is_encrypted_vec.push(parsed.std_info.is_encrypted);
                is_sparse_vec.push(parsed.std_info.is_sparse);
                is_reparse_vec.push(parsed.std_info.is_reparse);
                is_offline_vec.push(parsed.std_info.is_offline);
                is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
                is_temporary_vec.push(parsed.std_info.is_temporary);
                is_integrity_stream_vec.push(parsed.std_info.is_integrity_stream);
                is_no_scrub_data_vec.push(parsed.std_info.is_no_scrub_data);
                is_pinned_vec.push(parsed.std_info.is_pinned);
                is_unpinned_vec.push(parsed.std_info.is_unpinned);
                is_virtual_vec.push(parsed.std_info.is_virtual);
                flags_vec.push(parsed.std_info.to_raw_flags());
            }
        }

        // Log stats (computed during the loop above)
        stats.log_summary();

        // Build DataFrame with full schema
        Self::build_dataframe_full(
            frs_vec,
            parent_frs_vec,
            name_vec,
            size_vec,
            allocated_size_vec,
            created_vec,
            modified_vec,
            accessed_vec,
            mft_changed_vec,
            is_directory_vec,
            name_count_vec,
            stream_count_vec,
            stream_name_vec,
            is_readonly_vec,
            is_hidden_vec,
            is_system_vec,
            is_archive_vec,
            is_compressed_vec,
            is_encrypted_vec,
            is_sparse_vec,
            is_reparse_vec,
            is_offline_vec,
            is_not_indexed_vec,
            is_temporary_vec,
            is_integrity_stream_vec,
            is_no_scrub_data_vec,
            is_pinned_vec,
            is_unpinned_vec,
            is_virtual_vec,
            flags_vec,
        )
    }
}

