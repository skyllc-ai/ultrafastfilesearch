use std::sync::RwLock;

use once_cell::sync::Lazy;

use crate::config::{BLOCKING_THREADS, WORKER_THREADS};

// Create a Lazy static constant with RwLock
pub(crate) static CURRENT_WORKER_THREADS: Lazy<RwLock<usize>> =
    Lazy::new(|| RwLock::new(WORKER_THREADS));
pub(crate) static CURRENT_BLOCKING_THREADS: Lazy<RwLock<usize>> =
    Lazy::new(|| RwLock::new(BLOCKING_THREADS));
