//! Lean-index read entrypoints and the core index read pipeline.

#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use tracing::{debug, info, trace, warn};

#[cfg(windows)]
use super::read_mode::index_effective_mode;
use super::{MftProgress, MftReader};
#[cfg(windows)]
use super::MftReadMode;
use crate::error::{MftError, Result};
#[cfg(windows)]
use crate::platform::VolumeHandle;

impl MftReader {

    /// Read the entire MFT into a lean `MftIndex` (fast path).
    ///
    /// This method builds a compact `MftIndex` structure instead of a Polars
    /// DataFrame. It's significantly faster because it avoids the DataFrame
    /// building overhead (~15-20s on large drives).
    ///
    /// Use this when you need fast indexing and searching. Convert to DataFrame
    /// later with `MftIndex::to_dataframe()` if you need Polars analytics.
    ///
    /// # Note
    ///
    /// This function uses `spawn_blocking` internally to run MFT reading on a
    /// dedicated blocking thread. This avoids potential nested tokio runtime
    /// issues that can occur when dependencies (like polars) try to create
    /// their own runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub async fn read_all_index(&self) -> Result<crate::index::MftIndex> {
        tracing::debug!(volume = %self.volume, "[TRIP] reader::read_all_index ENTER");
        trace!(volume = %self.volume, "read_all_index: ENTER");
        // Capture configuration to recreate reader in blocking thread
        let volume = self.volume;
        let mode = self.mode;
        let merge_extensions = self.merge_extensions;
        let use_bitmap = self.use_bitmap;
        let expand_links = self.expand_links;

        let add_placeholders = self.add_placeholders;
        let concurrency = self.concurrency;
        let io_size = self.io_size;
        let parallel_parse = self.parallel_parse;
        let parse_workers = self.parse_workers;
        let forensic = self.forensic;

        let result = tokio::task::spawn_blocking(move || {
            trace!(volume = %volume, "read_all_index: INSIDE spawn_blocking");
            // Create a new reader in the blocking thread
            let handle = VolumeHandle::open(volume)?;
            let reader = MftReader {
                volume,
                handle,
                mode,
                merge_extensions,
                use_bitmap,
                expand_links,
                add_placeholders,
                concurrency,
                io_size,
                parallel_parse,
                parse_workers,
                forensic,
            };
            let idx = reader.read_mft_index_internal(None::<fn(MftProgress)>);
            trace!(volume = %volume, "read_all_index: read_mft_index_internal done");
            idx
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?;
        trace!(volume = %volume, "read_all_index: EXIT");
        tracing::debug!(volume = %volume, "[TRIP] reader::read_all_index EXIT");
        result
    }

    /// Synchronous version of `read_all_index` for use in blocking contexts.
    ///
    /// This is the same as `read_all_index` but without the async wrapper,
    /// for use with `spawn_blocking` or other blocking contexts.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub fn read_all_index_sync(&self) -> Result<crate::index::MftIndex> {
        self.read_mft_index_internal(None::<fn(MftProgress)>)
    }

    /// Synchronous version of `read_all_index` (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub const fn read_all_index_sync(&self) -> Result<crate::index::MftIndex> {
        Err(MftError::PlatformNotSupported)
    }

    /// Synchronous version of `read_index_with_progress` for use in blocking
    /// contexts.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub fn read_index_with_progress_sync<F>(&self, callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        self.read_mft_index_internal(Some(callback))
    }

    /// Synchronous version of `read_index_with_progress` (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub fn read_index_with_progress_sync<F>(&self, _callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT into lean index with progress callback.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called periodically with progress updates
    ///
    /// # Note
    ///
    /// This function uses `spawn_blocking` internally to run MFT reading on a
    /// dedicated blocking thread. This avoids potential nested tokio runtime
    /// issues.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub async fn read_index_with_progress<F>(&self, callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        // Capture configuration to recreate reader in blocking thread
        let volume = self.volume;
        let mode = self.mode;
        let merge_extensions = self.merge_extensions;
        let use_bitmap = self.use_bitmap;
        let expand_links = self.expand_links;
        let add_placeholders = self.add_placeholders;
        let concurrency = self.concurrency;
        let io_size = self.io_size;
        let parallel_parse = self.parallel_parse;
        let parse_workers = self.parse_workers;
        let forensic = self.forensic;

        tokio::task::spawn_blocking(move || {
            // Create a new reader in the blocking thread
            let handle = VolumeHandle::open(volume)?;
            let reader = MftReader {
                volume,
                handle,
                mode,
                merge_extensions,
                use_bitmap,
                expand_links,
                add_placeholders,
                concurrency,
                io_size,
                parallel_parse,
                parse_workers,
                forensic,
            };
            reader.read_mft_index_internal(Some(callback))
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?
    }

    /// Read MFT into lean index with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_index_with_progress<F>(&self, _callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal implementation for building lean `MftIndex`.
    ///
    /// This is the fast path that avoids DataFrame building overhead.
    /// Uses the same I/O and parsing as `read_mft_internal`, but builds
    /// a compact `MftIndex` instead of a Polars DataFrame.
    #[cfg(windows)]
    #[expect(
        clippy::too_many_lines,
        reason = "sequential I/O pipeline with mode-specific branches cannot be meaningfully split"
    )]
    fn read_mft_index_internal<F>(&self, callback: Option<F>) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress),
    {
        use crate::index::MftIndex;
        use crate::io::{MftExtentMap, ParallelMftReader};
        use crate::platform::detect_drive_type;

        tracing::debug!(volume = %self.volume, "[TRIP] reader::read_mft_index_internal ENTER");
        info!(volume = %self.volume, "Starting MFT read (lean index)");

        let start_time = Instant::now();
        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();

        // Detect drive type for optimal I/O tuning
        let drive_type = detect_drive_type(self.volume);
        info!(
            volume = %self.volume,
            drive_type = ?drive_type,
            "🚀 Drive type detected for I/O optimization (lean index)"
        );

        // Get MFT extents for fragmented MFT support
        let extents = self.handle.get_mft_extents().unwrap_or_else(|e| {
            warn!(error = ?e, "Failed to get MFT extents, using fallback");
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

        // Try to get the MFT bitmap for optimization
        let bitmap = if self.use_bitmap {
            let bm = self.handle.get_mft_bitmap().ok();
            if let Some(ref b) = bm {
                let in_use = b.count_in_use();
                info!(
                    in_use_records = in_use,
                    skip_percentage = 100.0 - (in_use as f64 / total_records as f64 * 100.0),
                    "MFT bitmap loaded - will skip unused records"
                );
            }
            bm
        } else {
            info!("Bitmap optimization DISABLED - reading ALL records");
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

        // Select reader based on mode
        // Historical benchmark insight: "read all then parse" is faster than pipelining
        // even on HDD because: no context switching, CPU cache stays hot, no
        // channel overhead, OS can optimize continuous sequential reads better.
        // For lean index (MftIndex), use SlidingIocpInline for NVMe/SSD - this uses
        // IOCP with multiple reads in flight and inline parsing, matching C++
        // performance.
        let effective_mode = index_effective_mode(self.mode, drive_type);

        info!(mode = %effective_mode, "🚀 Using read mode (lean index)");

        let handle = self.handle.raw_handle();
        let total_bytes = total_records * u64::from(record_size);

        // Read using the selected mode (same as read_mft_internal)
        let parsed_records = match effective_mode {
            MftReadMode::Parallel | MftReadMode::Auto => {
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
            MftReadMode::Pipelined => {
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
                // IOCP parallel mode: Multiple overlapped reads in flight
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
                // Sliding window IOCP with inline parsing and direct index building
                // This mode returns MftIndex directly, skipping the intermediate
                // Vec<ParsedRecord>
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = parallel_reader.read_all_sliding_window_iocp_to_index::<fn(u64, u64)>(
                    overlapped_handle,
                    self.volume,
                    self.concurrency,
                    self.io_size,
                    None,
                );

                // Close the overlapped handle
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                let index = result?;

                // Report final progress
                if let Some(ref cb) = callback {
                    cb(MftProgress {
                        records_read: total_records,
                        total_records: Some(total_records),
                        bytes_read: total_bytes,
                        elapsed: start_time.elapsed(),
                    });
                }

                // Return early - we already have the index, no need for placeholder/build
                // phases
                return Ok(index);
            }
            MftReadMode::Streaming | MftReadMode::Prefetch => {
                // Fallback to parallel for streaming/prefetch modes in lean index
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);
                parallel_reader
                    .read_all_parallel_with_progress::<fn(u64, u64)>(handle, true, None)?
            }
        };

        // Add placeholder records for missing parent directories.
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

        tracing::debug!(
            volume = %self.volume,
            records_parsed = records_parsed_count,
            "[TRIP] reader::read_mft_index_internal -> I/O+parse done, calling MftIndex::from_parsed_records"
        );
        info!(
            records_parsed = records_parsed_count,
            elapsed_ms = read_elapsed.as_millis(),
            throughput_mb_s = format!("{:.1}", throughput_mb_s),
            "✅ MFT read complete, building lean index"
        );

        // Build lean MftIndex (fast path - no DataFrame overhead)
        let index_start = Instant::now();
        let index = MftIndex::from_parsed_records(self.volume, parsed_records);
        let index_elapsed = index_start.elapsed();

        tracing::debug!(
            volume = %self.volume,
            records = index.records.len(),
            index_build_ms = index_elapsed.as_millis(),
            "[TRIP] reader::read_mft_index_internal -> MftIndex::from_parsed_records done"
        );
        info!(
            records = index.records.len(),
            names_buffer_kb = index.names.len() / 1024,
            index_build_ms = index_elapsed.as_millis(),
            "✅ Lean index built"
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

        tracing::debug!(volume = %self.volume, "[TRIP] reader::read_mft_index_internal EXIT");
        Ok(index)
    }
}

