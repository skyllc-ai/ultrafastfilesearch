// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! MCP bridge error types.
//!
//! Translates between `uffs-client` errors and MCP protocol errors.

use rmcp::ErrorData as McpError;

/// Errors that can occur in the MCP ↔ daemon bridge.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The daemon client returned an error.
    #[error("daemon error: {0}")]
    Daemon(String),

    /// A required tool parameter was missing.
    #[error("missing required parameter: {0}")]
    MissingParam(&'static str),

    /// A tool parameter had an invalid value.
    #[error("invalid parameter '{name}': {reason}")]
    InvalidParam {
        /// Parameter name.
        name: &'static str,
        /// Why it was rejected.
        reason: String,
    },

    /// JSON serialization/deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl BridgeError {
    /// Returns `true` if this error likely indicates a broken daemon
    /// connection (e.g. daemon crashed, pipe broken) rather than a
    /// user-facing validation error.
    #[must_use]
    pub(crate) fn is_daemon_connection_error(&self) -> bool {
        match self {
            Self::Daemon(msg) => {
                let lower = msg.to_lowercase();
                lower.contains("broken pipe")
                    || lower.contains("connection refused")
                    || lower.contains("connection reset")
                    || lower.contains("eof")
                    || lower.contains("not connected")
                    || lower.contains("connection failed")
                    || lower.contains("timed out")
            }
            Self::MissingParam(_) | Self::InvalidParam { .. } | Self::Serialization(_) => false,
        }
    }
}

impl From<BridgeError> for McpError {
    fn from(err: BridgeError) -> Self {
        match err {
            BridgeError::MissingParam(_) | BridgeError::InvalidParam { .. } => {
                Self::invalid_params(err.to_string(), None)
            }
            BridgeError::Daemon(_) | BridgeError::Serialization(_) => {
                Self::internal_error(err.to_string(), None)
            }
        }
    }
}
