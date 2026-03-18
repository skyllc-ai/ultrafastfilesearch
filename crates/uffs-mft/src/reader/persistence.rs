//! Persistence helpers for parquet and raw MFT save/load paths.
//!
//! Exception: IOCP capture mode adds ~270 LOC of Windows-specific overlapped
//! I/O code for capturing chunks in IOCP completion order. Splitting this would
//! fragment the `MftReader` persistence API.

use std::path::Path;

#[cfg(windows)]
use tracing::{debug, info};
use uffs_polars::{DataFrame, ParquetReader, ParquetWriter, SerReader};

use super::MftReader;
use crate::error::{MftError, Result};

impl MftReader {
    /// Save a `DataFrame` to Parquet format.
    ///
    /// Parquet provides excellent compression and fast loading times.
    ///
    /// # Arguments
    ///
    /// * `df` - The `DataFrame` to save
    /// * `path` - Output file path
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn save_parquet<P: AsRef<Path>>(df: &mut DataFrame, path: P) -> Result<()> {
        let file = std::fs::File::create(path.as_ref())?;
        ParquetWriter::new(file)
            .finish(df)
            .map_err(|err| MftError::Parquet(err.to_string()))?;
        Ok(())
    }

    /// Load a `DataFrame` from Parquet format.
    ///
    /// # Arguments
    ///
    /// * `path` - Input file path
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is invalid.
    pub fn load_parquet<P: AsRef<Path>>(path: P) -> Result<DataFrame> {
        let file = std::fs::File::open(path.as_ref())?;
        let df = ParquetReader::new(file)
            .finish()
            .map_err(|err| MftError::Parquet(err.to_string()))?;
        Ok(df)
    }

    /// Read the entire MFT as raw bytes.
    ///
    /// This reads all MFT records as contiguous raw bytes, handling fragmented
    /// MFTs by reassembling extents in order. The result can be saved with
    /// [`save_raw_mft`](crate::raw::save_raw_mft) for offline analysis.
    ///
    /// # Returns
    ///
    /// A tuple of (raw bytes, record size).
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub fn read_raw(&self) -> Result<(Vec<u8>, u32)> {
        self.read_raw_internal()
    }

    /// Read raw MFT (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub const fn read_raw(&self) -> Result<(Vec<u8>, u32)> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal raw MFT reading implementation.
    ///
    /// Uses the shared `ParallelMftReader` infrastructure for proper chunk
    /// handling, sector alignment, and dynamic buffer sizing.
    #[cfg(windows)]
    fn read_raw_internal(&self) -> Result<(Vec<u8>, u32)> {
        use crate::io::{MftExtentMap, ParallelMftReader, generate_read_chunks};
        use crate::platform::detect_drive_type;

        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();

        let extents = self.handle.get_mft_extents().unwrap_or_else(|_| {
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);
        let total_records = extent_map.total_records();
        let total_size = total_records as usize * record_size as usize;
        let mut output = vec![0u8; total_size];

        let drive_type = detect_drive_type(self.volume);
        let parallel_reader =
            ParallelMftReader::new_optimized(extent_map.clone(), None, drive_type);
        let chunks = generate_read_chunks(&extent_map, None, parallel_reader.chunk_size);
        let handle = self.handle.raw_handle();

        for chunk in chunks {
            let data = parallel_reader.read_chunk(handle, &chunk, record_size)?;
            let output_offset = chunk.start_frs as usize * record_size as usize;
            let copy_size = data.len().min(total_size - output_offset);
            output[output_offset..output_offset + copy_size].copy_from_slice(&data[..copy_size]);
        }

        Ok((output, record_size))
    }

    /// Read raw MFT and save to file using streaming I/O.
    ///
    /// This method uses streaming I/O to avoid buffering the entire MFT in
    /// memory. Each chunk is read from disk and immediately written to the
    /// output file, enabling efficient saves of large MFTs (10+ GB).
    ///
    /// # Errors
    ///
    /// Returns an error if raw MFT reading or writing the output file fails.
    #[cfg(windows)]
    pub fn save_raw_to_file<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        self.save_raw_streaming(path, options)
    }

    /// Internal streaming save implementation.
    #[cfg(windows)]
    #[expect(
        unsafe_code,
        reason = "FFI: windows ReadFile, SetFilePointerEx for raw MFT streaming"
    )]
    fn save_raw_streaming<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        use std::thread;

        use crossbeam_channel::{Receiver, Sender, bounded};
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

        use crate::io::{AlignedBuffer, MftExtentMap, SECTOR_SIZE, generate_read_chunks};
        use crate::platform::detect_drive_type;
        use crate::raw::StreamingRawMftWriter;

        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();
        let extents = self.handle.get_mft_extents().unwrap_or_else(|_| {
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });
        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);

        let drive_type = detect_drive_type(self.volume);
        let chunk_size = match drive_type {
            crate::platform::DriveType::Nvme => 8 * 1024 * 1024,
            crate::platform::DriveType::Ssd => 8 * 1024 * 1024,
            crate::platform::DriveType::Hdd | crate::platform::DriveType::Unknown => {
                4 * 1024 * 1024
            }
        };

        let chunks = generate_read_chunks(&extent_map, None, chunk_size);
        let total_chunks = chunks.len();
        info!(
            "Streaming save: {} chunks, {} MB each, drive type: {:?}",
            total_chunks,
            chunk_size / (1024 * 1024),
            drive_type
        );

        let mut writer = StreamingRawMftWriter::new(path, record_size, options)?;
        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = bounded(2);
        let handle_ptr = self.handle.raw_handle().0 as usize;
        let record_size_copy = record_size;

        let reader_handle = thread::spawn(move || -> Result<()> {
            let handle = HANDLE(handle_ptr as *mut std::ffi::c_void);
            let mut buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);

            for chunk in chunks {
                let read_size = chunk.record_count * u64::from(record_size_copy);
                let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
                let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
                    / SECTOR_SIZE)
                    * SECTOR_SIZE;

                if buffer.len() < aligned_size {
                    buffer = AlignedBuffer::new(aligned_size);
                }

                let mut new_pos: i64 = 0;
                // SAFETY: `handle` is a live raw MFT handle and `new_pos` is valid
                // writable storage for the duration of this seek.
                let seek_result = unsafe {
                    SetFilePointerEx(
                        handle,
                        aligned_offset as i64,
                        Some(&mut new_pos),
                        FILE_BEGIN,
                    )
                };
                if seek_result.is_err() {
                    return Err(MftError::Io(std::io::Error::last_os_error()));
                }

                let mut bytes_read: u32 = 0;
                // SAFETY: `handle` is live, the aligned buffer slice covers
                // `aligned_size` writable bytes, and `bytes_read` is a valid
                // out-parameter for the call.
                let read_result = unsafe {
                    ReadFile(
                        handle,
                        Some(&mut buffer.as_mut_slice()[..aligned_size]),
                        Some(&mut bytes_read),
                        None,
                    )
                };
                if read_result.is_err() {
                    return Err(MftError::Io(std::io::Error::last_os_error()));
                }

                let actual_size = read_size as usize;
                let data =
                    buffer.as_slice()[offset_adjustment..offset_adjustment + actual_size].to_vec();

                if tx.send(data).is_err() {
                    break;
                }
            }

            Ok(())
        });

        let mut chunks_written = 0;
        for data in rx {
            writer.write_chunk(&data)?;
            chunks_written += 1;

            if chunks_written % 100 == 0 {
                debug!(
                    "Streaming save progress: {}/{} chunks",
                    chunks_written, total_chunks
                );
            }
        }

        match reader_handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(_) => {
                return Err(MftError::Io(std::io::Error::other(
                    "Reader thread panicked",
                )));
            }
        }

        let header = writer.finish()?;
        info!(
            "Streaming save complete: {} records, {} bytes",
            header.record_count, header.original_size
        );
        Ok(header)
    }

    /// Save raw MFT to file (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub fn save_raw_to_file<P: AsRef<Path>>(
        &self,
        _path: P,
        _options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        Err(MftError::PlatformNotSupported)
    }

    /// Load raw MFT from file and parse to `DataFrame`.
    ///
    /// # Errors
    ///
    /// Returns an error if the raw file cannot be loaded or parsed.
    pub fn load_raw_to_dataframe<P: AsRef<Path>>(path: P) -> Result<DataFrame> {
        Self::load_raw_to_dataframe_with_options(path, &crate::raw::LoadRawOptions::default())
    }

    /// Load raw MFT from file and convert to `DataFrame` with custom options.
    ///
    /// # Errors
    ///
    /// Returns an error if the raw file cannot be loaded, fixed up, parsed, or
    /// converted into a `DataFrame`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "record_count is u64 but MFT sizes are bounded by disk size, always fits in usize"
    )]
    pub fn load_raw_to_dataframe_with_options<P: AsRef<Path>>(
        path: P,
        options: &crate::raw::LoadRawOptions,
    ) -> Result<DataFrame> {
        use crate::parse::{MftRecordMerger, apply_fixup, parse_record_full};

        let raw = crate::raw::load_raw_mft(path, options)?;
        let mut merger = MftRecordMerger::with_capacity(raw.header.record_count as usize);

        for (frs, record_data) in raw.iter_records() {
            let mut record_buf = record_data.to_vec();
            if !apply_fixup(&mut record_buf) {
                continue;
            }
            merger.add_result(parse_record_full(&record_buf, frs));
        }

        let parsed_columns = merger.merge_into_columns(true);
        Self::build_dataframe_from_columns(parsed_columns)
    }

    /// Load raw MFT from file and build `MftIndex`.
    ///
    /// # Errors
    ///
    /// Returns an error if the raw file cannot be loaded or parsed into an
    /// index.
    pub fn load_raw_to_index<P: AsRef<Path>>(path: P) -> Result<crate::index::MftIndex> {
        Self::load_raw_to_index_with_options(path, &crate::raw::LoadRawOptions::default())
    }

    /// Load raw MFT from file and build `MftIndex` with custom options.
    ///
    /// Auto-detects file format:
    /// - **UFFS-IOCP**: IOCP capture format (replays Windows IOCP completion
    ///   order)
    /// - **UFFS-MFT**: Standard compressed format
    /// - **Raw NTFS**: Uncompressed MFT bytes (FILE magic)
    ///
    /// # Errors
    ///
    /// Returns an error if the raw file cannot be loaded or if record parsing
    /// or index construction fails.
    #[expect(
        clippy::too_many_lines,
        reason = "parsing logic with forensic/sequential/parallel branches is inherently complex"
    )]
    pub fn load_raw_to_index_with_options<P: AsRef<Path>>(
        path: P,
        options: &crate::raw::LoadRawOptions,
    ) -> Result<crate::index::MftIndex> {
        use std::time::Instant;

        use tracing::info;

        use crate::index::MftIndex;
        use crate::parse::{
            MftRecordMerger, ParseOptions, ParseResult, apply_fixup, parse_record_forensic,
            parse_record_full,
        };

        let path_ref = path.as_ref();

        // Check for IOCP capture format first
        if crate::raw_iocp::is_iocp_capture(path_ref)? {
            info!(
                path = %path_ref.display(),
                "📼 Detected IOCP capture format - replaying Windows IOCP order"
            );
            return Self::load_iocp_capture_to_index(path_ref, options);
        }

        let mut raw = crate::raw::load_raw_mft(path_ref, options)?;
        let capacity = usize::try_from(raw.header.record_count).unwrap_or(0);
        let total_records_in_file = capacity;
        let parse_options = if options.forensic {
            ParseOptions::FORENSIC
        } else {
            ParseOptions::DEFAULT
        };

        if options.forensic {
            let mut parsed_records = Vec::with_capacity(capacity);
            let mut records_examined: u64 = 0;
            let mut fixup_success: u64 = 0;
            let mut fixup_failed: u64 = 0;
            let mut base_records: u64 = 0;

            for (frs, record_data) in raw.iter_records() {
                records_examined += 1;
                let mut record_buf = record_data.to_vec();
                let fixup_ok = apply_fixup(&mut record_buf);
                if fixup_ok {
                    fixup_success += 1;
                } else {
                    fixup_failed += 1;
                }
                let result = parse_record_forensic(&record_buf, frs, &parse_options, !fixup_ok);
                if let ParseResult::Base(parsed) = result {
                    base_records += 1;
                    parsed_records.push(parsed);
                }
            }

            info!("📊 OFFLINE PATH PARSE DIAGNOSTICS (Forensic Mode)");
            info!(
                total_records_in_file,
                records_examined,
                fixup_success,
                fixup_failed,
                base_records_parsed = base_records,
                final_record_count = parsed_records.len(),
                "Offline parse pipeline summary"
            );

            Ok(MftIndex::from_parsed_records(
                raw.header.volume_letter,
                parsed_records,
            ))
        } else {
            let record_size = raw.header.record_size as usize;
            let single_thread = std::env::var("UFFS_SINGLE_THREAD").is_ok();
            let parse_start = Instant::now();

            if single_thread {
                let mut merger = MftRecordMerger::with_capacity(capacity);
                let mut fixup_success: u64 = 0;
                let mut fixup_failed: u64 = 0;
                let mut base_records: u64 = 0;
                let mut extension_records: u64 = 0;
                let mut skip_records: u64 = 0;

                for (frs, record_data) in raw.iter_records() {
                    let mut record_buf = record_data.to_vec();
                    if !apply_fixup(&mut record_buf) {
                        fixup_failed += 1;
                        continue;
                    }
                    fixup_success += 1;
                    let result = parse_record_full(&record_buf, frs);
                    match &result {
                        ParseResult::Base(_) => base_records += 1,
                        ParseResult::Extension(_) => extension_records += 1,
                        ParseResult::Skip => skip_records += 1,
                    }
                    merger.add_result(result);
                }

                let parsed_records = merger.merge();
                info!(
                    total_records_in_file,
                    parse_ms = parse_start.elapsed().as_millis(),
                    fixup_success,
                    fixup_failed,
                    base_records,
                    extension_records,
                    skip_records,
                    final_merged_count = parsed_records.len(),
                    "Offline parse complete (sequential)"
                );

                Ok(MftIndex::from_parsed_records(
                    raw.header.volume_letter,
                    parsed_records,
                ))
            } else {
                use rayon::prelude::*;

                let records_per_chunk = 4096_usize;
                let bytes_per_chunk = records_per_chunk * record_size;
                let buffer_slice = raw.data.as_mut_slice();

                let results: Vec<(Vec<ParseResult>, u64, u64, u64, u64, u64)> = buffer_slice
                    .par_chunks_mut(bytes_per_chunk)
                    .enumerate()
                    .map(|(chunk_idx, chunk)| {
                        let mut results = Vec::new();
                        let mut fixup_ok = 0_u64;
                        let mut fixup_fail = 0_u64;
                        let mut bases = 0_u64;
                        let mut extensions = 0_u64;
                        let mut skips = 0_u64;

                        let start_frs = chunk_idx * records_per_chunk;
                        let records_in_chunk = chunk.len() / record_size;

                        for i in 0..records_in_chunk {
                            let offset = i * record_size;
                            let Some(record_slice) = chunk.get_mut(offset..offset + record_size)
                            else {
                                continue;
                            };

                            if !apply_fixup(record_slice) {
                                fixup_fail += 1;
                                continue;
                            }
                            fixup_ok += 1;

                            let frs = (start_frs + i) as u64;
                            let result = parse_record_full(record_slice, frs);
                            match &result {
                                ParseResult::Base(_) => bases += 1,
                                ParseResult::Extension(_) => extensions += 1,
                                ParseResult::Skip => skips += 1,
                            }
                            if !matches!(result, ParseResult::Skip) {
                                results.push(result);
                            }
                        }

                        (results, fixup_ok, fixup_fail, bases, extensions, skips)
                    })
                    .collect();

                let mut merger = MftRecordMerger::with_capacity(capacity);
                let mut fixup_success: u64 = 0;
                let mut fixup_failed: u64 = 0;
                let mut base_records: u64 = 0;
                let mut extension_records: u64 = 0;
                let mut skip_records: u64 = 0;

                for (chunk_results, ok, fail, bases, exts, skips) in results {
                    fixup_success += ok;
                    fixup_failed += fail;
                    base_records += bases;
                    extension_records += exts;
                    skip_records += skips;
                    for result in chunk_results {
                        merger.add_result(result);
                    }
                }

                let parsed_records = merger.merge();
                info!(
                    total_records_in_file,
                    parse_ms = parse_start.elapsed().as_millis(),
                    fixup_success,
                    fixup_failed,
                    base_records,
                    extension_records,
                    skip_records,
                    final_merged_count = parsed_records.len(),
                    threads = rayon::current_num_threads(),
                    "Offline parse complete (parallel)"
                );

                Ok(MftIndex::from_parsed_records(
                    raw.header.volume_letter,
                    parsed_records,
                ))
            }
        }
    }

    /// Load IOCP capture and build `MftIndex` by replaying chunks in captured
    /// order.
    ///
    /// This processes MFT chunks in the exact order Windows IOCP delivered
    /// them, enabling 100% accurate reproduction of LIVE parsing behavior
    /// on any platform.
    ///
    /// # Errors
    ///
    /// Returns an error if the capture file cannot be loaded or parsing fails.
    /// Load IOCP capture and build `MftIndex` using parallel parsing.
    ///
    /// This mirrors the Windows LIVE pipeline exactly:
    /// 1. Load chunks in IOCP completion order
    /// 2. Parse each chunk in parallel using rayon (non-deterministic thread
    ///    order)
    /// 3. Merge all parse results through `MftRecordMerger`
    ///
    /// The parallel parsing is critical for reproducing Windows LIVE behavior,
    /// as it introduces the same non-deterministic ordering that can expose
    /// merger edge cases.
    fn load_iocp_capture_to_index(
        path: &Path,
        options: &crate::raw::LoadRawOptions,
    ) -> Result<crate::index::MftIndex> {
        use std::time::Instant;

        use rayon::prelude::*;
        use tracing::info;

        use crate::index::MftIndex;
        use crate::parse::{MftRecordMerger, ParseResult, apply_fixup, parse_record_full};
        use crate::raw_iocp::load_iocp_capture;

        let load_start = Instant::now();
        let capture = load_iocp_capture(path)?;
        let header = &capture.header;
        let record_size = header.record_size as usize;
        let volume = options.volume_letter.unwrap_or(header.volume_letter);

        info!(
            path = %path.display(),
            chunks = header.chunk_count,
            total_records = header.total_records,
            volume = %volume,
            concurrency = header.concurrency,
            compressed = header.is_compressed(),
            "📼 Loading IOCP capture (parallel replay)"
        );

        let parse_start = Instant::now();
        // MFT record count always fits in memory on 64-bit systems (typical max ~5M
        // records)
        #[expect(
            clippy::cast_possible_truncation,
            reason = "MFT record count fits in usize on 64-bit (target platform)"
        )]
        let capacity = header.total_records as usize;

        // Collect all chunks with their data for parallel processing
        // This mimics the Windows LIVE pipelined reader's behavior
        let chunks_data: Vec<_> = capture.iter_chunks().collect();

        // Parse all chunks in parallel using rayon - this replicates the
        // non-deterministic ordering of Windows LIVE parallel parsing
        let parse_results: Vec<ParseResult> = chunks_data
            .par_iter()
            .flat_map(|(chunk, data)| {
                let num_records = data.len() / record_size;
                let mut results = Vec::with_capacity(num_records);

                for i in 0..num_records {
                    let offset = i * record_size;
                    if let Some(record_slice) = data.get(offset..offset + record_size) {
                        let mut record_buf = record_slice.to_vec();
                        if apply_fixup(&mut record_buf) {
                            let frs = chunk.start_frs + i as u64;
                            results.push(parse_record_full(&record_buf, frs));
                        }
                    }
                }
                results
            })
            .collect();

        let records_parsed = parse_results.len();
        info!(
            parse_results = records_parsed,
            "✅ Parallel parsing complete"
        );

        // Merge results using MftRecordMerger (single-threaded, as in LIVE)
        let mut merger = MftRecordMerger::with_capacity(capacity);
        for result in parse_results {
            merger.add_result(result);
        }
        let parsed_records = merger.merge();

        let parse_ms = parse_start.elapsed().as_millis();

        info!(
            load_ms = load_start.elapsed().as_millis(),
            parse_ms,
            chunks_processed = header.chunk_count,
            records_parsed,
            final_merged_count = parsed_records.len(),
            "✅ IOCP capture parallel replay complete"
        );

        let mut index = MftIndex::from_parsed_records(volume, parsed_records);
        index.compute_tree_metrics();

        Ok(index)
    }

    /// Save MFT using IOCP capture mode.
    ///
    /// This reads MFT using IOCP and saves chunks in the order they complete,
    /// capturing the non-deterministic I/O ordering for realistic testing.
    ///
    /// # Errors
    ///
    /// Returns an error if raw MFT reading or writing the output file fails.
    #[cfg(windows)]
    pub fn save_iocp_capture<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw_iocp::IocpCaptureOptions,
    ) -> Result<crate::raw_iocp::IocpCaptureHeader> {
        self.save_iocp_internal(path, options)
    }

    /// Internal IOCP capture implementation.
    #[cfg(windows)]
    #[expect(
        unsafe_code,
        reason = "FFI: ReadFile, GetQueuedCompletionStatus for overlapped IOCP capture"
    )]
    fn save_iocp_internal<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw_iocp::IocpCaptureOptions,
    ) -> Result<crate::raw_iocp::IocpCaptureHeader> {
        use std::collections::VecDeque;
        use std::pin::Pin;

        use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, GetLastError, HANDLE};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        use crate::io::{
            AlignedBuffer, IoCompletionPort, MftExtentMap, OverlappedRead, SECTOR_SIZE,
            generate_read_chunks,
        };
        use crate::platform::detect_drive_type;
        use crate::raw_iocp::IocpCaptureWriter;

        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();
        let concurrency = options.concurrency as usize;

        let extents = self.handle.get_mft_extents().unwrap_or_else(|_| {
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);
        let drive_type = detect_drive_type(self.volume);
        let chunk_size = drive_type.optimal_chunk_size();
        let chunks = generate_read_chunks(&extent_map, None, chunk_size);
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            let writer = IocpCaptureWriter::new(record_size, options);
            return writer.write_to_file(path);
        }

        info!(
            chunks = num_chunks,
            chunk_size_mb = chunk_size / (1024 * 1024),
            concurrency,
            "🎯 Starting IOCP capture with {} concurrent reads",
            concurrency
        );

        // Create capture writer
        let mut writer = IocpCaptureWriter::new(record_size, options);

        // IOCP requires FILE_FLAG_OVERLAPPED, so we need a separate handle
        let handle: HANDLE = self.handle.open_overlapped_handle()?;

        // Create IOCP and associate overlapped handle
        let iocp = IoCompletionPort::new(0)?;
        if let Err(e) = iocp.associate(handle, 0) {
            // SAFETY: handle was successfully opened by open_overlapped_handle
            unsafe { CloseHandle(handle) }.ok();
            return Err(e);
        }

        // Pre-allocate buffer pool and in-flight operations
        let max_chunk_size = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .max()
            .unwrap_or(chunk_size as u64) as usize;

        // Don't sort chunks - we want to capture IOCP completion order
        let mut pending_chunks: VecDeque<crate::io::ReadChunk> = chunks.into_iter().collect();
        let mut in_flight: Vec<Option<Pin<Box<OverlappedRead>>>> =
            (0..concurrency).map(|_| None).collect();

        // Issue initial reads
        for (slot_idx, slot) in in_flight.iter_mut().enumerate() {
            if let Some(chunk) = pending_chunks.pop_front() {
                let buffer = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
                let mut op = Box::pin(OverlappedRead::new(buffer, chunk, record_size, slot_idx));

                let aligned_offset =
                    (op.chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                op.set_offset(aligned_offset);

                let read_size = op.chunk.record_count * u64::from(record_size);
                let offset_adjustment = (op.chunk.disk_offset - aligned_offset) as usize;
                let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
                    / SECTOR_SIZE)
                    * SECTOR_SIZE;

                let overlapped_ptr = unsafe { op.as_mut().get_unchecked_mut().as_overlapped_ptr() };
                let read_result = unsafe {
                    ReadFile(
                        handle,
                        Some(
                            &mut op.as_mut().get_unchecked_mut().buffer.as_mut_slice()
                                [..aligned_size],
                        ),
                        None,
                        Some(overlapped_ptr),
                    )
                };

                if read_result.is_err() {
                    let err = unsafe { GetLastError() };
                    if err != ERROR_IO_PENDING {
                        // SAFETY: handle was opened by open_overlapped_handle
                        unsafe { CloseHandle(handle) }.ok();
                        return Err(MftError::Io(std::io::Error::from_raw_os_error(
                            err.0 as i32,
                        )));
                    }
                }
                *slot = Some(op);
            }
        }

        // Process completions in the order they arrive
        let mut completed_chunks = 0;
        while completed_chunks < num_chunks {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            let status = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    u32::MAX,
                )
            };

            if status.is_err() && overlapped_ptr.is_null() {
                // SAFETY: handle was opened by open_overlapped_handle
                unsafe { CloseHandle(handle) }.ok();
                return Err(MftError::Io(std::io::Error::last_os_error()));
            }

            // Find which slot completed by matching the overlapped pointer
            let mut completed_slot_idx: Option<usize> = None;
            for (idx, slot) in in_flight.iter_mut().enumerate() {
                if let Some(op) = slot {
                    let op_ptr = op.as_overlapped_ptr();
                    if op_ptr == overlapped_ptr {
                        completed_slot_idx = Some(idx);
                        break;
                    }
                }
            }

            if let Some(slot_idx) = completed_slot_idx {
                if let Some(op) = in_flight[slot_idx].take() {
                    // Extract chunk data in completion order
                    let chunk = &op.chunk;
                    let read_size = chunk.record_count * u64::from(record_size);
                    let aligned_offset =
                        (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                    let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;

                    let data = op.buffer.as_slice()
                        [offset_adjustment..offset_adjustment + read_size as usize]
                        .to_vec();

                    // Record in completion order
                    writer.record_chunk(chunk.start_frs, data);
                    completed_chunks += 1;

                    debug!(
                        "Captured chunk {} of {} (FRS {}..{})",
                        completed_chunks,
                        num_chunks,
                        chunk.start_frs,
                        chunk.start_frs + chunk.record_count
                    );

                    // Issue next read if available
                    if let Some(next_chunk) = pending_chunks.pop_front() {
                        let buffer = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
                        let mut new_op = Box::pin(OverlappedRead::new(
                            buffer,
                            next_chunk,
                            record_size,
                            slot_idx,
                        ));

                        let aligned_offset =
                            (new_op.chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                        new_op.set_offset(aligned_offset);

                        let read_size = new_op.chunk.record_count * u64::from(record_size);
                        let offset_adjustment =
                            (new_op.chunk.disk_offset - aligned_offset) as usize;
                        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE
                            - 1)
                            / SECTOR_SIZE)
                            * SECTOR_SIZE;

                        let overlapped_ptr =
                            unsafe { new_op.as_mut().get_unchecked_mut().as_overlapped_ptr() };
                        let read_result = unsafe {
                            ReadFile(
                                handle,
                                Some(
                                    &mut new_op.as_mut().get_unchecked_mut().buffer.as_mut_slice()
                                        [..aligned_size],
                                ),
                                None,
                                Some(overlapped_ptr),
                            )
                        };

                        if read_result.is_err() {
                            let err = unsafe { GetLastError() };
                            if err != ERROR_IO_PENDING {
                                // SAFETY: handle was opened by open_overlapped_handle
                                unsafe { CloseHandle(handle) }.ok();
                                return Err(MftError::Io(std::io::Error::from_raw_os_error(
                                    err.0 as i32,
                                )));
                            }
                        }
                        in_flight[slot_idx] = Some(new_op);
                    }
                }
            }
        }

        info!(
            "IOCP capture complete: {} chunks in completion order",
            writer.chunk_count()
        );

        // Close the overlapped handle before writing the file
        // SAFETY: handle was opened by open_overlapped_handle and is no longer needed
        unsafe { CloseHandle(handle) }.ok();

        writer.write_to_file(path)
    }

    /// Save IOCP capture (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub fn save_iocp_capture<P: AsRef<Path>>(
        &self,
        _path: P,
        _options: &crate::raw_iocp::IocpCaptureOptions,
    ) -> Result<crate::raw_iocp::IocpCaptureHeader> {
        Err(MftError::PlatformNotSupported)
    }

    /// Load raw MFT from file and build `MftIndex` using direct-to-index
    /// parser.
    ///
    /// This is a single-pass implementation that parses records directly into
    /// the index without creating intermediate `ParsedRecord` allocations. It
    /// uses the modernized `parse_record_to_index()` from Wave 1.
    ///
    /// # Errors
    ///
    /// Returns an error if the raw file cannot be loaded or if record parsing
    /// or index construction fails.
    pub fn load_raw_to_index_direct<P: AsRef<Path>>(
        path: P,
        options: &crate::raw::LoadRawOptions,
    ) -> Result<crate::index::MftIndex> {
        use std::time::Instant;

        use tracing::info;

        use crate::index::MftIndex;
        use crate::parse::{apply_fixup, parse_record_to_index};

        let parse_start = Instant::now();

        // Load raw MFT data
        let mut raw = crate::raw::load_raw_mft(path, options)?;
        let capacity = usize::try_from(raw.header.record_count).unwrap_or(0);
        let total_records_in_file = capacity;
        let record_size = raw.header.record_size as usize;

        // Create index with pre-allocated capacity
        let mut index = MftIndex::with_capacity(raw.header.volume_letter, capacity);

        // Parse records directly into index
        let mut fixup_success: u64 = 0;
        let mut fixup_failed: u64 = 0;
        let mut records_added: u64 = 0;

        let buffer_slice = raw.data.as_mut_slice();
        for (frs, chunk) in buffer_slice.chunks_exact_mut(record_size).enumerate() {
            // Apply fixup in place
            if !apply_fixup(chunk) {
                fixup_failed += 1;
                continue;
            }
            fixup_success += 1;

            // Parse record directly into index
            // parse_record_to_index handles both base and extension records internally
            let added = parse_record_to_index(chunk, frs as u64, &mut index);
            if added {
                records_added += 1;
            }
        }

        // Sort directory children for deterministic output
        // CRITICAL for OFFLINE path: ensures consistent ordering across runs
        index.sort_directory_children();

        // Compute tree metrics
        index.compute_tree_metrics();

        let parse_time = parse_start.elapsed();

        info!(
            total_records_in_file,
            parse_ms = parse_time.as_millis(),
            fixup_success,
            fixup_failed,
            records_added,
            final_index_size = index.len(),
            "Direct-to-index parse complete"
        );

        Ok(index)
    }
}
