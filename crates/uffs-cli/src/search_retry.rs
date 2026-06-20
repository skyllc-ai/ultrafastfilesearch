// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Client-side resilience for a search that races a parked-drive re-warm.
//!
//! When the daemon has idled long enough to **park** drives (bodies evicted,
//! only the bloom + path trie resident), the first search re-warms them. On
//! Windows that re-warm reads MFT extents over the broker's overlapped handle,
//! and a pending read can be cancelled with `ERROR_OPERATION_ABORTED` (os error
//! 995) when the worker thread that issued it is reaped. The daemon's own read
//! primitive ([`uffs-mft`'s `read_handle_at`]) already retries that transient;
//! this is the client-side belt-and-suspenders so a user never sees a raw I/O
//! error for a recoverable warm-up hiccup — and gets a "warming" note instead.

// Each helper is used exactly once on the search path; this is cohesion, not a
// smell — the `single_call_fn` restriction lint is relaxed for the module.
#![expect(
    clippy::single_call_fn,
    reason = "search-retry helpers, each called once on the search hot path"
)]

use uffs_client::connect_sync::UffsClientSync;
use uffs_client::error::ClientError;

/// Max client-side retries when a search aborts mid-index-warm.
const WARM_RETRY_MAX: u32 = 5;
/// Backoff between warm-retry attempts.
const WARM_RETRY_BACKOFF: core::time::Duration = core::time::Duration::from_millis(400);

/// Run `search_cli_raw`, transparently retrying the one transient a search can
/// hit while the daemon re-warms parked drives: Windows
/// `ERROR_OPERATION_ABORTED` (os error 995).
///
/// Bounded + back-off so a genuine, persistent failure still fails fast.
pub(crate) fn search_cli_with_warm_retry(
    client: &mut UffsClientSync,
    args: &[String],
) -> Result<serde_json::Value, ClientError> {
    let mut attempt = 0_u32;
    loop {
        match client.search_cli_raw(args) {
            Ok(response) => return Ok(response),
            Err(err) if attempt < WARM_RETRY_MAX && is_index_warming_abort(&err) => {
                attempt += 1;
                report_index_warming(attempt);
                std::thread::sleep(WARM_RETRY_BACKOFF);
            }
            Err(err) => return Err(err),
        }
    }
}

/// `true` if `err` is the transient `ERROR_OPERATION_ABORTED` (os error 995) a
/// search hits when it races a parked-drive re-warm. Matched on the stable OS
/// error code in the rendered message — the daemon's typed I/O error is
/// flattened to a string across the JSON-RPC boundary, so the code is the only
/// portable signal left.
fn is_index_warming_abort(err: &ClientError) -> bool {
    let message = err.to_string();
    message.contains("os error 995") || message.contains("operation has been aborted")
}

/// Tell the user (on stderr, so it never pollutes stdout/CSV) that the index is
/// warming and the search is being retried.
#[expect(clippy::print_stderr, reason = "user-facing progress note, off stdout")]
fn report_index_warming(attempt: u32) {
    eprintln!("UFFS index is warming up — retrying search ({attempt}/{WARM_RETRY_MAX})…");
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::default_numeric_fallback,
        reason = "test module — relaxed linting"
    )]

    use uffs_client::error::ClientError;

    use super::is_index_warming_abort;

    /// Only a transient `ERROR_OPERATION_ABORTED` (os error 995) — what a
    /// search hits while racing a parked-drive re-warm — should be retried;
    /// real errors must surface immediately.
    #[test]
    fn warming_abort_matches_only_995() {
        assert!(is_index_warming_abort(&ClientError::Io(
            "The I/O operation has been aborted because of either a thread exit \
             or an application request. (os error 995)"
                .to_owned()
        )));
        assert!(is_index_warming_abort(&ClientError::DaemonError {
            code: -32000,
            message: "I/O error: ... (os error 995)".to_owned(),
        }));
        // Real failures are NOT retried.
        assert!(!is_index_warming_abort(&ClientError::Io(
            "permission denied (os error 5)".to_owned()
        )));
        assert!(!is_index_warming_abort(&ClientError::ConnectionClosed));
    }
}
