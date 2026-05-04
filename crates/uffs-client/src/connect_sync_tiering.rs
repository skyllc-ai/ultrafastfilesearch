// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Memory-tiering RPC helpers for [`crate::connect_sync::UffsClientSync`].
//!
//! Split off [`crate::connect_sync`] so the tiering RPC cluster
//! (`hibernate`, `preload`, `forget`, `status_drives`) lives next
//! to the [`crate::protocol::response_tiering`] wire types it
//! consumes — same sibling-module pattern as the daemon-state
//! cluster (`response_status` ↔ inline `connect_sync` helpers).
//!
//! Phase 8-B … 8-E — paired with the daemon-side handlers in
//! `crates/uffs-daemon/src/handler.rs` (`handle_hibernate`,
//! `handle_preload`, `handle_forget`, `handle_status_drives`).
//! Every helper does the typed-envelope dance: serialise the
//! params struct, fire the JSON-RPC, deserialise the response
//! into the matching wire type.

use crate::connect_sync::UffsClientSync;
use crate::error::ClientError;
use crate::protocol::response::{
    ForgetParams, ForgetResponse, HibernateParams, HibernateResponse, PreloadParams,
    PreloadResponse, StatusDrivesParams, StatusDrivesResponse,
};

impl UffsClientSync {
    /// Demote every loaded shard (or the caller-supplied subset) to
    /// `Cold` via the daemon's `hibernate` RPC.
    ///
    /// Empty `params.drives` ⇒ "every loaded drive" (the daemon's
    /// authoritative view of "every loaded drive" is what gets
    /// hibernated, not the client's).
    ///
    /// # Errors
    ///
    /// Returns `ClientError` on I/O, protocol, or timeout failure.
    /// The daemon does not currently fail this RPC under normal
    /// operation; per-drive demote failures (which today never
    /// surface — `demote_letter_with_reason` only `None`s on
    /// already-Cold shards, which the operator audit reports under
    /// `already_cold`) would land in a future per-drive `errors`
    /// field.
    pub fn hibernate(
        &mut self,
        params: &HibernateParams,
    ) -> Result<HibernateResponse, ClientError> {
        let payload =
            serde_json::to_value(params).map_err(|err| ClientError::Protocol(err.to_string()))?;
        let result = self.send_request("hibernate", Some(payload))?;
        serde_json::from_value(result).map_err(|err| ClientError::Protocol(err.to_string()))
    }

    /// Promote one or more drives to `Hot` and pin the tier for
    /// `params.pin_minutes` minutes (or
    /// [`crate::protocol::response::DEFAULT_PRELOAD_PIN_MINUTES`]
    /// when omitted) via the daemon's `preload` RPC.
    ///
    /// Empty `params.drives` is rejected by the daemon with
    /// `ERR_INVALID_PARAMS` — surface as `ClientError::Protocol` so
    /// CLI scripts that miswire the request fail loudly.
    ///
    /// # Errors
    ///
    /// Returns `ClientError` on I/O, protocol, or timeout failure.
    /// Per-drive failures (drive not loaded, body load failure,
    /// transient state) land in [`PreloadResponse::errors`] rather
    /// than in `ClientError`.
    pub fn preload(&mut self, params: &PreloadParams) -> Result<PreloadResponse, ClientError> {
        let payload =
            serde_json::to_value(params).map_err(|err| ClientError::Protocol(err.to_string()))?;
        let result = self.send_request("preload", Some(payload))?;
        serde_json::from_value(result).map_err(|err| ClientError::Protocol(err.to_string()))
    }

    /// Evict drive(s) from the registry and unlink their on-disk
    /// cache files via the daemon's `forget` RPC.
    ///
    /// Without `params.force`, the daemon refuses non-`Cold` drives
    /// with [`crate::protocol::ERR_DRIVE_BUSY`] (`-4`); the helper
    /// surfaces that as [`ClientError::Protocol`] with the daemon's
    /// listing of the busy drives.  With `params.force` the daemon
    /// auto-hibernates each non-`Cold` drive first.
    ///
    /// # Errors
    ///
    /// Returns `ClientError` on I/O, protocol, or `ERR_DRIVE_BUSY`
    /// failure.  Per-drive `permission denied`-style errors land in
    /// [`ForgetResponse::errors`] without failing the call.
    pub fn forget(&mut self, params: &ForgetParams) -> Result<ForgetResponse, ClientError> {
        let payload =
            serde_json::to_value(params).map_err(|err| ClientError::Protocol(err.to_string()))?;
        let result = self.send_request("forget", Some(payload))?;
        serde_json::from_value(result).map_err(|err| ClientError::Protocol(err.to_string()))
    }

    /// Per-drive tier + telemetry table via the daemon's
    /// `status_drives` RPC.
    ///
    /// Operator-facing companion to [`Self::status_raw`] — widens
    /// the tier marker into a structured row per drive
    /// (`resident_bytes`, `query_rate_per_min`, `pin_until_unix_ms`,
    /// …) so a CLI can render a table without cross-referencing
    /// tracing logs.  Sends an empty params object (the wire type is
    /// a unit struct that round-trips as `{}`).
    ///
    /// # Errors
    ///
    /// Returns `ClientError` on I/O, protocol, or timeout failure.
    /// An empty registry produces an `Ok` with `drives = []` rather
    /// than an error.
    pub fn status_drives(&mut self) -> Result<StatusDrivesResponse, ClientError> {
        let payload = serde_json::to_value(StatusDrivesParams)
            .map_err(|err| ClientError::Protocol(err.to_string()))?;
        let result = self.send_request("status_drives", Some(payload))?;
        serde_json::from_value(result).map_err(|err| ClientError::Protocol(err.to_string()))
    }
}
