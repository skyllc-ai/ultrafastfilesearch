//! Time measurement utilities for benchmarking and profiling.
//!
//! Provides functions for measuring execution time of both synchronous
//! and asynchronous operations.

// Infrastructure utilities - defined for benchmarking and profiling use
#![allow(dead_code)]

use std::future::Future;
use std::time::{Duration, Instant};

pub(crate) fn measure_time_normal<F, R>(func: F) -> (R, Duration)
where
    F: FnOnce() -> R,
{
    let start = Instant::now();
    let result = func();
    let duration = start.elapsed();
    (result, duration)
}

pub(crate) async fn measure_time_tokio<F, Fut, R>(func: F) -> (R, Duration)
where
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = R> + Send,
{
    let start = Instant::now();
    let result = func().await;
    let duration = start.elapsed();
    (result, duration)
}

pub(crate) fn measure_time_normal_bench<F, R>(func: F) -> Duration
where
    F: Fn() -> Result<R, Box<dyn std::error::Error>>,
{
    let start = Instant::now();
    let _ = func();
    let duration = start.elapsed();
    duration
}

pub(crate) async fn measure_time_tokio_bench<F, Fut>(func: F) -> Duration
where
    F: FnOnce() -> Fut,
    Fut: Future,
{
    let start = Instant::now();
    let _ = func().await;
    start.elapsed()
}

pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = (total_seconds / 3600) % 24;
    let days = total_seconds / 86400;

    let milliseconds = duration.subsec_millis();
    let microseconds = duration.subsec_micros() % 1_000;
    let nanoseconds = duration.subsec_nanos() % 1_000;

    if days > 0 {
        format!("{:>2}d {:>2}h {:>2}m {:>2}s", days, hours, minutes, seconds)
    } else if hours > 0 {
        format!("{:>2}h {:>2}m {:>2}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{:>3} m  {:>3} s ", minutes, seconds)
    } else if seconds > 0 {
        format!("{:>3} s  {:>3} ms", seconds, milliseconds)
    } else if milliseconds > 0 {
        format!("{:>3} ms {:>3} μs", milliseconds, microseconds)
    } else if microseconds > 0 {
        format!("{:>3} μs {:>3} ns", microseconds, nanoseconds)
    } else {
        format!("{:>3} ns", nanoseconds)
    }
}
