// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! MCP tool implementations.
//!
//! Each tool module contains a handler function that takes parsed arguments
//! and an [`UffsClient`](uffs_client::connect::UffsClient), returning an
//! [`rmcp::model::CallToolResult`].

// Phase 3: parent `tools` is `pub(crate)`; siblings stay private to it.
/// `uffs_aggregate` — server-side aggregation summaries.
pub(crate) mod aggregate;
/// `uffs_drives` — list indexed NTFS drives.
pub(crate) mod drives;
/// `uffs_facet_values` — search within facet values for a field.
pub(crate) mod facet_values;
/// `uffs_info` — file/directory detail lookup by path.
pub(crate) mod info;
/// `uffs_search` — file search across all indexed drives.
pub(crate) mod search;
/// `uffs_status` — daemon health and loading progress.
pub(crate) mod status;
