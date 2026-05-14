// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs_status` tool — daemon health and loading progress.

use rmcp::model::{CallToolResult, Content};
use uffs_client::connect::UffsClient;

use crate::error::BridgeError;
use crate::schemas::StatusOutput;

/// Execute the status tool (no arguments).
///
/// # Errors
///
/// Returns [`BridgeError`] if the daemon call fails.
pub(crate) async fn run(client: &mut UffsClient) -> Result<CallToolResult, BridgeError> {
    let response = client
        .status()
        .await
        .map_err(|err| BridgeError::Daemon(format!("Failed to get status: {err}")))?;

    let status_str = serde_json::to_string_pretty(&response.status)?;

    let text = format!(
        "Daemon Status: {status_str}\nUptime: {}s\nConnections: {}\nPID: {}\n",
        response.uptime_secs, response.connections, response.pid
    );

    let structured = StatusOutput {
        status: serde_json::to_value(&response.status)?,
        uptime_secs: response.uptime_secs,
        connections: response.connections,
        pid: response.pid,
    };

    let mut result = CallToolResult::success(vec![Content::text(text)]);
    result.structured_content = Some(serde_json::to_value(structured)?);
    Ok(result)
}
