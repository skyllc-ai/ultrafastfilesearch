// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Memory-tiering RPC helpers for [`crate::connect_sync::UffsClientSync`].
//!
//! Split off [`crate::connect_sync`] so the tiering RPC cluster
//! (`hibernate`, `preload`, plus `forget` / `status_drives` in later
//! sub-phases 8-D / 8-E) lives next to the
//! [`crate::protocol::response_tiering`] wire types it consumes —
//! same sibling-module pattern as the daemon-state cluster
//! (`response_status` ↔ inline `connect_sync` helpers).
//!
//! Phase 8-B / 8-C — paired with the daemon-side handlers in
//! `crates/uffs-daemon/src/handler.rs` (`handle_hibernate`,
//! `handle_preload`).  Both helpers do the typed envelope dance:
//! serialise the params struct, fire the JSON-RPC, deserialise the
//! response into the matching wire type.

use crate::connect_sync::UffsClientSync;
use crate::error::ClientError;
use crate::protocol::response::{
    HibernateParams, HibernateResponse, PreloadParams, PreloadResponse,
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
}
