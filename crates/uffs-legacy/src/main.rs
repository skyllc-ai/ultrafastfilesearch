//! UFFS Legacy Binary
//!
//! **⚠️ DEPRECATED: Use `uffs-cli` instead.**
//!
//! This binary is kept for reference and testing legacy functionality.

use std::error::Error;
use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::io::Result as IoResult;
use std::iter::once;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
#[cfg(windows)]
use std::os::windows::prelude::OsStrExt;
use std::path::Path;
use std::pin::Pin;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use colored::Colorize;
use log::error;
use once_cell::sync::Lazy;
use tokio::runtime::Builder;
use tokio::time::Instant;
use tracing::info;
use uffs_legacy::config::constants::{BLOCKING_THREADS, MAX_TEMP_FILES_HDD_BATCH, WORKER_THREADS};
use uffs_legacy::modules::disk::drive_info::{get_drive_info, print_drive_info_table};
#[cfg(windows)]
use uffs_legacy::modules::disk::wim_defrag_analysis::query_defrag_analysis;
#[cfg(windows)]
use uffs_legacy::modules::disk::wim_disk_quota::query_disk_quota;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_disk_drive::query_disk_drives;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_disk_partition::query_disk_partitions;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_encryptable_volume::query_encryptable_volumes;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_logical_disk::query_logical_disk;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_mount_point::query_mount_point;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_msft_disk::query_msft_disks;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_msft_partition::query_msft_partition;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_perf_disk_physical_disk::query_perf_disk_physical_disk;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_physical_media::query_physical_media;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_quota_setting::query_quota_setting;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_shadow_copy::query_shadow_copy;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_volume::query_volumes;
#[cfg(windows)]
use uffs_legacy::modules::disk::wmi_volume_quota::query_volume_quota;
#[cfg(windows)]
use uffs_legacy::modules::entities::disk::ColumnLengths;
use uffs_legacy::modules::errors::errors_impl::UFFSError;
use uffs_legacy::modules::logging::logger::init_logger;
use uffs_legacy::modules::utils::time_utils::format_duration;
use uffs_legacy::modules::utils::tree_printer_utils::print_directory_tree;

/// Windows-specific WMI query functions
#[cfg(windows)]
fn run_windows_wmi_queries() {
    use std::process::exit;

    // Slow
    match query_disk_quota() {
        Ok(results) => {
            for disk_quota in results {
                println!("Disk Quota: \n\n{}", disk_quota);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    println!("ULTRA-FAST-FILE START");

    match query_defrag_analysis() {
        Ok(results) => {
            for defrag_analysis in results {
                println!("defrag_analysis: \n\n{}", defrag_analysis);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_volume_quota() {
        Ok(results) => {
            for quota in results {
                println!("Volume Quota: \n\n{}", quota);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_mount_point() {
        Ok(results) => {
            for mount_point in results {
                println!("Mount Point: \n\n{}", mount_point);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    // SLOW
    match query_quota_setting() {
        Ok(results) => {
            for quota_setting in results {
                println!("Quota Setting: \n\n{}", quota_setting);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_shadow_copy() {
        Ok(results) => {
            for shadow_copy in results {
                println!("Shadow Copy: \n\n{}", shadow_copy);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_perf_disk_physical_disk() {
        Ok(results) => {
            for perf_disk_physical_disk in results {
                println!("perf_disk_physical_disk: \n\n{}", perf_disk_physical_disk);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_msft_partition() {
        Ok(results) => {
            for msft_partition in results {
                println!("msft_partition: \n\n{}", msft_partition);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_volumes() {
        Ok(results) => {
            for volumes in results {
                println!("volumes: \n\n{}", volumes);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    // Slow
    match query_logical_disk() {
        Ok(results) => {
            for logical_disk in results {
                println!("logical_disk: \n\n{}", logical_disk);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_disk_partitions() {
        Ok(results) => {
            for disk_partitions in results {
                println!("disk_partitions: \n\n{}", disk_partitions);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_disk_drives() {
        Ok(results) => {
            for disk_drives in results {
                println!("disk_drives: \n\n{}", disk_drives);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_msft_disks() {
        Ok(results) => {
            for msft_disks in results {
                println!("msft_disks: \n\n{}", msft_disks);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    // little Slow
    match query_physical_media() {
        Ok(results) => {
            for physical_media in results {
                println!("physical_media: \n\n{}", physical_media);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    match query_encryptable_volumes() {
        Ok(results) => {
            for encryptable_volumes in results {
                println!("encryptable_volumes: \n\n{}", encryptable_volumes);
            }
        }
        Err(e) => eprintln!("Error: {:?}", e),
    }

    // Early exit after WMI queries for testing
    if true {
        exit(0)
    };
}

fn main() -> anyhow::Result<(), UFFSError> {
    println!("{}", "UFFS - Ultra Fast File Search".green().bold());
    println!("{}", "High-Performance File Search Tool".cyan());
    println!();

    // Windows-specific WMI queries
    #[cfg(windows)]
    {
        run_windows_wmi_queries();
    }

    #[cfg(not(windows))]
    {
        println!(
            "{}",
            "Running on non-Windows platform - WMI queries skipped".yellow()
        );
    }

    let start = Instant::now();

    // Get the drive info
    let mut drives = get_drive_info()?;

    let total_duration = start.elapsed();

    // Print the table
    print_drive_info_table(&mut drives, total_duration);

    // initialize_app();
    //
    // run_app();

    info!("Application finished.");

    Ok(())
}

// fn find_best_configuration(configurations: Vec<(usize, usize)>) -> (usize,
// usize) {     let mut best_duration = Duration::MAX;
//     let mut best_config = (0, 0);
//
//     for (worker_threads, blocking_threads) in configurations {
//         let duration = run_with_configuration(worker_threads,
// blocking_threads);         println!(
//             "Configuration with {} worker threads and {} blocking threads
// took {:?}",             worker_threads,
//             blocking_threads,
//             format_duration(duration)
//         );
//
//         if duration < best_duration {
//             best_duration = duration;
//             best_config = (worker_threads, blocking_threads);
//         }
//     }
//
//     best_config
// }
//
// fn run_with_configuration(worker_threads: usize, blocking_threads: usize) ->
// Duration {     let mut time_used = Default::default();
//     // Configure Tokio runtime with optimized settings for high-performance
// system     let runtime = Builder::new_multi_thread()
//         .worker_threads(worker_threads)
//         .max_blocking_threads(blocking_threads)
//         .enable_all()
//         .build()
//         .expect("Failed to create Tokio runtime");
//
//     // Run the async function using the configured runtime
//     runtime.block_on(async {
//         let start = Instant::now();
//
//         let separator1 = "=".repeat(50).green().to_string();
//         let separator2 = "-".repeat(50).red().to_string();
//
//         println!("{}", separator1);
//         println!("\nReadDirectories4\n");
//         println!("{}", separator2);
//
//         let directory_reader = Arc::new(ReadDirectories4);
//
//         process_all_disks(directory_reader).await;
//
//         time_used = Instant::now() - start;
//     });
//
//     info!("Application finished.");
//
//     time_used
// }

// fn main_opti(WORKER_THREADS: usize, BLOCKING_THREADS: usize) -> Result<(),
// Box<dyn Error + Send + Sync>> {     // Configure Tokio runtime with optimized
// settings for high-performance system     let runtime =
// Builder::new_multi_thread()         .WORKER_THREADS(WORKER_THREADS)
//         .max_blocking_threads(BLOCKING_THREADS)
//         .enable_all()
//         .build()
//         .expect("Failed to create Tokio runtime");
//
//     runtime.block_on(async {
//         let separator1 = "=".repeat(50).green().to_string();
//         let separator2 = "-".repeat(50).red().to_string();
//
//         println!("{}", separator1);
//         println!("\nReadDirectories4\n");
//         println!("{}", separator2);
//
//         let directory_reader = Arc::new(ReadDirectories4);
//
//         process_all_disks(directory_reader).await;
//
//     });
//
//     info!("Application finished.");
//
//     Ok(())
// }

// fn main_opti(WORKER_THREADS: usize, BLOCKING_THREADS: usize) ->
// std::io::Result<()> {     // Initialize the logging
//     let _guard = init_logger();
//
//     info!("Application started...");
//
//     // // Hardcoded search path (ensure it ends with \ and *)
//     // let search_path = "C:\\Users\\rnio\\*"; // Replace with an actual
// directory path     // let search_path_wide: Vec<u16> =
// OsString::from(search_path).encode_wide().chain(once(0)).collect();     //
//     // // Initialize find data structure
//     // let mut find_data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
//     //
//     // // Start the search
//     // let handle = unsafe { FindFirstFileW(search_path_wide.as_ptr(), &mut
// find_data) };     //
//     // if handle == INVALID_HANDLE_VALUE {
//     //     let error = unsafe { GetLastError() };
//     //     println!("Error: {}", error);
//     //     return     Ok(())
//     //     ;
//     // }
//     //
//     // loop {
//     //     // Convert file name to Vec<u16>
//     //     let file_name: Vec<u16> =
// find_data.cFileName.iter().take_while(|&&c| c != 0).cloned().collect();
// // // println!("file_name VEC:     \t{:?}", &file_name);     //
//     //     // Skip "." and ".." entries
//     //     if file_name != [b'.' as u16] && file_name != [b'.' as u16, b'.'
// as u16] {     //         if (find_data.dwFileAttributes &
// FILE_ATTRIBUTE_DIRECTORY) != 0 {     //             // It's a directory
//     //             println!("Directory: {}", vec_u16_to_string(&file_name));
//     //         } else {
//     //             // It's a file
//     //             println!("File: {}", vec_u16_to_string(&file_name));
//     //         }
//     //     }
//     //     // else{
//     //     //     println!("file_name:     \t{}",
// vec_u16_to_string(&file_name));     //     // }
//     //
//     //     // Get the next file or directory entry
//     //     if unsafe { FindNextFileW(handle, &mut find_data) } == 0 {
//     //         let error = unsafe { GetLastError() };
//     //         if error == 18 { // ERROR_NO_MORE_FILES
//     //             println!("No more files.");
//     //             break;
//     //         } else {
//     //             println!("Error: {}", error);
//     //             unsafe { FindClose(handle) };
//     //             return     Ok(())
//     //             ;
//     //         }
//     //     }
//     // }
//     //
//     // // Close the handle after finishing the search
//     // unsafe { FindClose(handle) };
//     //
//     // println!("Done");
//     /////////////////////////////////////////////////////////////////////////
// ///////////     //     // Initialize the input and output data structures
//     //     let start_path = "D:\\\\WOW Flight\\"; // Replace with an actual
// directory path     //     let start_path_wide = vec_u16_from_str(start_path);
//     //     let mut num_files = 0;
//     //     let mut num_dirs = 0;
//     //     let mut new_dirs_paths: Vec<Vec<u16>> = Vec::new();
//     //
//     //     // Call the function
//     //     match count_disk_entries_all_at_once(
//     //         &start_path_wide,
//     //         &mut num_files,
//     //         &mut num_dirs,
//     //         &mut new_dirs_paths,
//     //     ) {
//     //         Ok(()) => {
//     //             println!("Number of files: {}", num_files);
//     //             println!("Number of directories: {}", num_dirs);
//     //             println!("Number of new directories paths: {}\n\n",
// new_dirs_paths.len());     //             println!("The new directories :
// {:?}", new_dirs_paths);     //         }
//     //         Err(e) => {
//     //             println!("An error occurred: {}", e);
//     //         }
//     //     }
//     /////////////////////////////////////////////////////////////////////////
// ///////////
//
//     // Configure Tokio runtime with optimized settings for high-performance
// system     let runtime = Builder::new_multi_thread()
//         .WORKER_THREADS(WORKER_THREADS)
//         .max_blocking_threads(BLOCKING_THREADS)
//         .enable_all()
//         .build()
//         .expect("Failed to create Tokio runtime");
//
//     runtime.block_on(async {
//         // let hdd_path = Path::new("D:\\temp_test");
//         // let ssd_path = Path::new("C:\\temp_test");
//         //
//         // let (test_ssd, duration_ssd) =
//         //     measure_time_normal(||
// create_temp_dir_with_files_ssd(hdd_path).unwrap());         //
//         // let number = count_files_in_dir(&test_ssd.path()).await.unwrap();
//         // println!("Number of files: \t{}",number);
//         // println!("HDD temp dir: {:?}", test_ssd.path());
//         // println!("HDD creation took: {:?}",
// format_duration(duration_ssd));         //
//         // let search_path = test_ssd.path().join("*");
//         // println!("search_path: {:?}", search_path);
//         // let (result, duration_ssd) =
//         //     measure_time_normal(||
// read_directory_all_at_once(search_path.as_os_str()).unwrap());         //
// println!("Files:\t{}",result.len());         // println!("Reading HDD temp
// dir: {:?}", test_ssd.path());         // println!("Reading HDD creation took:
// {:?}", format_duration(duration_ssd));         //
//         // let (test_ssd, duration_ssd) =
//         //     measure_time_normal(||
// create_temp_dir_with_files_ssd(ssd_path).unwrap());         // let number =
// count_files_in_dir(&test_ssd.path()).await.unwrap();         //
// println!("Number of files: \t{}",number);         // println!("SSD temp dir:
// {:?}", test_ssd.path());         // println!("SSD creation took: {:?}",
// format_duration(duration_ssd));
//
//         // Get current disk information
//         // Read from file or re-create in case not done / too old
//         // let (current_drive_info, duration_init_drive) =
// measure_time_tokio(|| init_drives()).await;         //
//         // println!(
//         //     "Initialization time: {:?}",
//         //     format_duration(duration_init_drive)
//         // );
//         //
//         // // let current_drive_info = init_drives().await;
//         //
//         // println!("{:?}", current_drive_info);
//
//         // Process with ReadDirectories1
//         // let directory_reader = select_algorithm(&mut new_drive_info);
//         let separator1 = format!(
//             "{}",
//             "=".repeat(50
//             )
//                 .green()
//         );
//         let separator2 = format!(
//             "{}",
//             "-".repeat(50
//             )
//                 .red()
//         );
//         println!("{}", separator1);
//
//         println!("\nReadDirectories4\n");
//
//         println!("{}", separator2);
//
//         let directory_reader = Arc::new(ReadDirectories4);
//         process_all_disks(directory_reader).await;
//
//         // let path = get_path().expect("Error getting path to process");
//         // process_one_path(current_drive_info).await;
//
//         // process_test_disk(current_drive_info).await;
//         // process_all_disks(current_drive_info).await;
//     });
//
//     info!("Application finished.");
//
//     Ok(())
// }
