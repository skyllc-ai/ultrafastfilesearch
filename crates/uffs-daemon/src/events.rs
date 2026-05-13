// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Daemon event broadcasting — push notifications to connected clients.
//!
//! The daemon emits `DaemonEvent`s at lifecycle milestones (drive loaded,
//! ready, refresh, shutdown) and periodic stats heartbeats. Events are
//! serialized as JSON-RPC 2.0 notifications (no `id` field) and pushed to
//! all connected clients via `tokio::sync::broadcast`.
//!
//! Clients that don't read fast enough simply miss events (broadcast
//! channel lag) — this is fire-and-forget, never blocks the daemon.

use serde::Serialize;
use tokio::sync::broadcast;

/// Broadcast channel capacity — how many events can be buffered before
/// slow receivers start lagging.  16 is enough for startup (7 drives +
/// ready) plus headroom for stats heartbeats and refreshes.
const EVENT_CHANNEL_CAPACITY: usize = 64;

/// A daemon event pushed to all connected clients.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum DaemonEvent {
    /// Emitted when the daemon process starts (before any drives load).
    DaemonStarting {
        /// Daemon process ID.
        pid: u32,
        /// Daemon version string.
        version: String,
    },
    /// A single drive finished loading.
    DriveLoaded {
        /// Drive letter (e.g. 'C').
        drive: char,
        /// Number of records in the drive index.
        records: usize,
        /// MFT parse time in milliseconds.
        mft_ms: u128,
        /// Compact index build time in milliseconds.
        compact_ms: u128,
        /// Trigram index build time in milliseconds.
        trigram_ms: u128,
        /// How many drives have been loaded so far.
        drives_loaded: usize,
        /// Total drives expected.
        drives_total: usize,
    },
    /// All drives are loaded — daemon is ready to serve queries.
    DaemonReady {
        /// Number of loaded drives.
        drives: usize,
        /// Total records across all drives.
        total_records: usize,
        /// Startup duration in milliseconds.
        startup_ms: u128,
    },
    /// A drive refresh has started.
    RefreshStarted {
        /// Drives being refreshed.
        drives: Vec<char>,
    },
    /// A single drive finished refreshing.
    DriveRefreshed {
        /// Drive letter.
        drive: char,
        /// Updated record count.
        records: usize,
        /// MFT parse time in milliseconds.
        mft_ms: u128,
        /// Compact index build time in milliseconds.
        compact_ms: u128,
        /// Trigram index build time in milliseconds.
        trigram_ms: u128,
    },
    /// All drives finished refreshing — back to ready.
    RefreshComplete {
        /// Number of drives refreshed.
        drives_refreshed: usize,
    },
    /// Periodic stats heartbeat (emitted every 30 seconds).
    StatsHeartbeat {
        /// Total queries served since daemon start.
        total_queries: u64,
        /// Daemon uptime in seconds.
        uptime_secs: u64,
        /// Total records across all drives.
        total_records: usize,
        /// Active client connections.
        connections: usize,
    },
    /// A client connected or disconnected.
    ConnectionChanged {
        /// Current active connection count.
        active: usize,
    },
    /// Daemon is about to shut down.
    ShuttingDown {
        /// Reason for shutdown.
        reason: String,
    },
}

/// Create a new event broadcast channel.
///
/// Returns `(sender, receiver)`.  The sender is stored in `IndexManager`
/// (internal) and `LifecycleHandle` (internal); each client connection
/// subscribes via `sender.subscribe()`.
#[must_use]
pub(crate) fn event_channel() -> (EventSender, EventReceiver) {
    let (tx, rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
    (EventSender(tx), rx)
}

/// Wrapper around `broadcast::Sender<DaemonEvent>` with convenience methods.
#[derive(Clone, Debug)]
pub(crate) struct EventSender(broadcast::Sender<DaemonEvent>);

/// Alias for the receiver half.
pub(crate) type EventReceiver = broadcast::Receiver<DaemonEvent>;

impl EventSender {
    /// Emit an event to all connected clients. Never blocks or errors —
    /// if no receivers exist or the channel is full, the event is silently
    /// dropped.
    pub(crate) fn emit(&self, event: DaemonEvent) {
        // Log at trace level so daemon logs capture every event too.
        tracing::trace!(?event, "📣 event emitted");
        let _ignore = self.0.send(event);
    }

    /// Subscribe to the event stream (one subscription per client connection).
    #[must_use]
    pub(crate) fn subscribe(&self) -> EventReceiver {
        self.0.subscribe()
    }
}

/// Serialize a [`DaemonEvent`] as a JSON-RPC 2.0 notification line.
///
/// Returns a newline-terminated JSON string ready to write to the socket:
/// ```json
/// {"jsonrpc":"2.0","method":"daemon.drive_loaded","params":{...}}\n
/// ```
#[must_use]
pub(crate) fn event_to_json_line(event: &DaemonEvent) -> Option<String> {
    let method = match event {
        DaemonEvent::DaemonStarting { .. } => "daemon.starting",
        DaemonEvent::DriveLoaded { .. } => "daemon.drive_loaded",
        DaemonEvent::DaemonReady { .. } => "daemon.ready",
        DaemonEvent::RefreshStarted { .. } => "daemon.refresh_started",
        DaemonEvent::DriveRefreshed { .. } => "daemon.drive_refreshed",
        DaemonEvent::RefreshComplete { .. } => "daemon.refresh_complete",
        DaemonEvent::StatsHeartbeat { .. } => "daemon.stats",
        DaemonEvent::ConnectionChanged { .. } => "daemon.connection_changed",
        DaemonEvent::ShuttingDown { .. } => "daemon.shutting_down",
    };
    let params = serde_json::to_value(event).ok()?;
    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&notification).ok()?;
    line.push('\n');
    Some(line)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    clippy::default_numeric_fallback,
    reason = "JSON Value indexing is safe (returns null), integer literals are self-evident in tests"
)]
mod tests {
    use super::*;

    #[test]
    fn event_channel_emits_and_receives() {
        let (tx, mut rx) = event_channel();
        tx.emit(DaemonEvent::DaemonStarting {
            pid: 1234,
            version: "0.4.53".to_owned(),
        });
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, DaemonEvent::DaemonStarting {
            pid: 1234,
            ..
        }));
    }

    #[test]
    fn event_channel_fan_out_to_multiple_subscribers() {
        let (tx, _rx) = event_channel();
        let mut sub1 = tx.subscribe();
        let mut sub2 = tx.subscribe();

        tx.emit(DaemonEvent::ConnectionChanged { active: 3 });

        let e1 = sub1.try_recv().unwrap();
        let e2 = sub2.try_recv().unwrap();
        assert!(matches!(e1, DaemonEvent::ConnectionChanged { active: 3 }));
        assert!(matches!(e2, DaemonEvent::ConnectionChanged { active: 3 }));
    }

    #[test]
    fn event_channel_no_receivers_does_not_panic() {
        let (tx, rx) = event_channel();
        drop(rx);
        // Should not panic — silently drops.
        tx.emit(DaemonEvent::ShuttingDown {
            reason: "test".to_owned(),
        });
    }

    #[test]
    fn event_to_json_line_produces_valid_jsonrpc_notification() {
        let event = DaemonEvent::DriveLoaded {
            drive: 'C',
            records: 3_400_000,
            mft_ms: 2100,
            compact_ms: 850,
            trigram_ms: 320,
            drives_loaded: 1,
            drives_total: 7,
        };
        let line = event_to_json_line(&event).unwrap();

        // Ends with newline
        assert!(line.ends_with('\n'));

        // Valid JSON
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["method"], "daemon.drive_loaded");
        assert!(parsed.get("id").is_none(), "notifications must not have id");
        assert_eq!(parsed["params"]["drive"], "C");
        assert_eq!(parsed["params"]["records"], 3_400_000);
        assert_eq!(parsed["params"]["drives_loaded"], 1);
        assert_eq!(parsed["params"]["drives_total"], 7);
    }

    #[test]
    fn event_to_json_line_daemon_ready() {
        let event = DaemonEvent::DaemonReady {
            drives: 7,
            total_records: 25_800_000,
            startup_ms: 12_439,
        };
        let line = event_to_json_line(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["method"], "daemon.ready");
        assert_eq!(parsed["params"]["drives"], 7);
        assert_eq!(parsed["params"]["total_records"], 25_800_000);
        assert_eq!(parsed["params"]["startup_ms"], 12_439);
    }

    #[test]
    fn event_to_json_line_stats_heartbeat() {
        let event = DaemonEvent::StatsHeartbeat {
            total_queries: 42,
            uptime_secs: 300,
            total_records: 25_800_000,
            connections: 2,
        };
        let line = event_to_json_line(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["method"], "daemon.stats");
        assert_eq!(parsed["params"]["total_queries"], 42);
        assert_eq!(parsed["params"]["connections"], 2);
    }

    #[test]
    fn event_to_json_line_shutting_down() {
        let event = DaemonEvent::ShuttingDown {
            reason: "idle timeout (300s, tier 0)".to_owned(),
        };
        let line = event_to_json_line(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["method"], "daemon.shutting_down");
        assert_eq!(parsed["params"]["reason"], "idle timeout (300s, tier 0)");
    }

    #[test]
    fn event_to_json_line_all_event_types_serialize() {
        // Verify every variant produces valid JSON — no panics, no None.
        let events = vec![
            DaemonEvent::DaemonStarting {
                pid: 1,
                version: "test".to_owned(),
            },
            DaemonEvent::DriveLoaded {
                drive: 'D',
                records: 100,
                mft_ms: 10,
                compact_ms: 5,
                trigram_ms: 3,
                drives_loaded: 1,
                drives_total: 1,
            },
            DaemonEvent::DaemonReady {
                drives: 1,
                total_records: 100,
                startup_ms: 50,
            },
            DaemonEvent::RefreshStarted {
                drives: vec!['C', 'D'],
            },
            DaemonEvent::DriveRefreshed {
                drive: 'C',
                records: 200,
                mft_ms: 20,
                compact_ms: 10,
                trigram_ms: 5,
            },
            DaemonEvent::RefreshComplete {
                drives_refreshed: 2,
            },
            DaemonEvent::StatsHeartbeat {
                total_queries: 0,
                uptime_secs: 0,
                total_records: 0,
                connections: 0,
            },
            DaemonEvent::ConnectionChanged { active: 1 },
            DaemonEvent::ShuttingDown {
                reason: "test".to_owned(),
            },
        ];

        for event in &events {
            let line = event_to_json_line(event);
            assert!(line.is_some(), "Failed to serialize: {event:?}");
            let json: serde_json::Value = serde_json::from_str(line.unwrap().trim()).unwrap();
            assert_eq!(json["jsonrpc"], "2.0");
            assert!(json.get("method").is_some());
            assert!(json.get("id").is_none());
        }
    }

    /// Simulates the daemon's `notification_loop`: events arrive via broadcast
    /// and are forwarded as JSON-RPC notification lines to the client's socket.
    #[tokio::test]
    async fn notification_loop_delivers_events_to_client() {
        let (tx, _rx) = event_channel();
        let mut sub = tx.subscribe();

        // Simulate: client socket is a duplex (we read from one end)
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(16);

        // Spawn notification forwarder (mirrors IpcServer::notification_loop)
        let task = tokio::spawn(async move {
            loop {
                match sub.recv().await {
                    Ok(event) => {
                        if let Some(json_line) = event_to_json_line(&event)
                            && out_tx.send(json_line).await.is_err()
                        {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
        });

        // Emit two events
        tx.emit(DaemonEvent::DriveLoaded {
            drive: 'C',
            records: 100,
            mft_ms: 10,
            compact_ms: 5,
            trigram_ms: 3,
            drives_loaded: 1,
            drives_total: 2,
        });
        tx.emit(DaemonEvent::DaemonReady {
            drives: 2,
            total_records: 200,
            startup_ms: 50,
        });

        // Read them from the channel
        let line1 = tokio::time::timeout(core::time::Duration::from_secs(1), out_rx.recv())
            .await
            .unwrap()
            .unwrap();

        let line2 = tokio::time::timeout(core::time::Duration::from_secs(1), out_rx.recv())
            .await
            .unwrap()
            .unwrap();

        let parsed1: serde_json::Value = serde_json::from_str(line1.trim()).unwrap();
        let parsed2: serde_json::Value = serde_json::from_str(line2.trim()).unwrap();

        assert_eq!(parsed1["method"], "daemon.drive_loaded");
        assert_eq!(parsed2["method"], "daemon.ready");

        task.abort();
    }

    /// Verify that the client's notification routing works: when a
    /// response stream contains interleaved notifications, they are
    /// correctly separated by checking for "method" + no "id".
    #[test]
    fn client_notification_detection_matches_daemon_format() {
        // All daemon events should be detected as notifications
        let event = DaemonEvent::StatsHeartbeat {
            total_queries: 10,
            uptime_secs: 60,
            total_records: 1000,
            connections: 1,
        };
        let line = event_to_json_line(&event).unwrap();
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();

        // Client detection logic: has "method" + no "id" → notification
        assert!(value.get("method").is_some(), "must have method");
        assert!(value.get("id").is_none(), "must not have id");

        // A normal RPC response would have "id" — verify it's distinguishable
        let rpc_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"status": "ready"},
        });
        assert!(rpc_response.get("id").is_some(), "response must have id");
    }
}
