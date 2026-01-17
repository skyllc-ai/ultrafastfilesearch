use std::path::Path;
use crate::config::worker_threads::CURRENT_BLOCKING_THREADS;
use crate::config::worker_threads::CURRENT_WORKER_THREADS;

use std::sync::{Arc, RwLock};
use std::time::Instant;
use async_std::path::PathBuf;
use chrono::Local;
use crate::config::{BLOCKING_THREADS, LOG_DATE_FORMAT, MAX_DIRS_ALL, MAX_FILES_ALL, WORKER_THREADS};
use crate::modules::logging::init_logger;
use crate::modules::process::run_directory_processing;
use crate::modules::runtime::build_runtime;
use crate::modules::utils::{format_duration, get_number_of_cpu_cores};
use tracing::{debug, error, info};
use walkdir::DirEntry;

pub fn initialize_app() {
    let _guard = init_logger();

    info!("Application started...");

}

pub fn set_threads_count() -> (usize, usize) {
    let cpu_cores = get_number_of_cpu_cores();
    let worker_threads = cpu_cores - (cpu_cores as f64 * 0.1) as usize;
    let blocking_threads = worker_threads * 2;

    (worker_threads, blocking_threads)
}

pub fn run_app() {

    use walkdir::WalkDir;

    let root_path = PathBuf::from("c:\\");

    let mut files: Vec<DirEntry> = Vec::with_capacity(MAX_FILES_ALL);
    let mut dirs: Vec<DirEntry> = Vec::with_capacity(MAX_DIRS_ALL);
    let mut timestamp = Local::now().format(LOG_DATE_FORMAT).to_string();

    let start = Instant::now();
    use std::io::{self, Write};

    for entry in WalkDir::new(root_path.clone()).into_iter().filter_map(|e| e.ok()) {
        let file_type = entry.file_type();
        let path = entry.path().to_owned();
        if file_type.is_dir() {
            dirs.push(entry); // Add directories to the dirs vector
        } else if file_type.is_file() {
            // Simulate printing to null by writing to std::io::sink()
            // let mut null_output = std::io::sink();
            // writeln!(null_output, "{:?}", path).unwrap(); // Redirects to "null" instead of printing

            // println!("{:?}",path);
            files.push(entry); // Add files to the files vector
        }    
    }

    let num_files = files.len();

    let num_dirs = dirs.len();

    let duration = start.elapsed();
    let formatted_duration = format_duration(duration);
    timestamp = Local::now().format(LOG_DATE_FORMAT).to_string();

    let components: Vec<_> = root_path.components().collect();

    println!(
        "DONE: {:<18} at {}. FILES: {:>10} DIRS: {:>10} Running TIME: {:<8}",
        root_path.display(),
        timestamp,
        num_files,
        num_dirs,
        formatted_duration,
    ); 
    
    let (worker_threads, blocking_threads) = set_threads_count();
    debug!(
        "Running with {} worker threads and {} blocking threads",
        worker_threads, blocking_threads
    );

    debug!("Setting up runtime:");

    // // Configure Tokio runtime with optimized settings for high-performance system
    let runtime = build_runtime(worker_threads, blocking_threads);

    debug!(
        "Running with {} worker threads and {} blocking threads",
        worker_threads, blocking_threads
    );

    // Run the async function using the configured runtime
    runtime.block_on(async {
        let time_used = run_directory_processing().await;
        info!("Time used: {:?}", format_duration(time_used));
    });

    info!("Application finished.");
}
