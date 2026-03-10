//! Windows-only command handlers for the `uffs_mft` binary.

mod bench;
mod benchmark_index;
mod benchmark_mft;
mod bitmap_diag;
mod incremental;
mod info;
mod read;
mod save;
mod shared;

pub(crate) use self::bench::{cmd_bench, cmd_bench_all};
pub(crate) use self::benchmark_index::{
    cmd_benchmark_index, cmd_benchmark_index_lean, cmd_benchmark_multi_volume, cmd_benchmark_tree,
};
pub(crate) use self::benchmark_mft::cmd_benchmark_mft;
pub(crate) use self::bitmap_diag::cmd_bitmap_diag;
pub(crate) use self::incremental::{
    cmd_cache_clear, cmd_cache_get, cmd_cache_status, cmd_index_all, cmd_index_load,
    cmd_index_save, cmd_index_update, cmd_usn_info, cmd_usn_read,
};
pub(crate) use self::info::{cmd_drives, cmd_info};
pub(crate) use self::read::cmd_read;
pub(crate) use self::save::cmd_save;
