//! `FfiVcAdapter` — `vc_core::VcAdapter` impl that dispatches
//! across the FFI boundary into a loaded plugin's
//! [`crate::vtables::VcVtable`].
//!
//! Same shape as [`super::sync::FfiSyncAdapter`] — single
//! vtable (no multi-capability wrapper because videoconference
//! plugins are single-capability by design) + spawn_blocking
//! dispatch + status-to-error mapping targeting
//! `vc_core::VcError`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use tracing::warn;
use vc_core::{Meeting, MeetingId, NewMeeting, VcAdapter, VcError, VcResult};

use crate::ffi::*;
use crate::manager::{InFlightGuard, LoadedInstance};
use crate::vtables::VcVtable;

use super::call::{call_method, decode_payload, encode_args, CallOutcome};

pub struct FfiVcAdapter {
    _instance: Arc<LoadedInstance>,
    handle_addr: usize,
    vtable: VtableSnapshot,
    /// In-flight counter handle shared with the
    /// [`crate::manager::LoadedPlugin`]. Every FFI-dispatching
    /// trait method brackets its body with an [`InFlightGuard`]
    /// derived from this Arc so the host's unload path can
    /// observe a deterministic "is anything in flight" gate.
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Clone, Copy)]
struct VtableSnapshot {
    test_connection: Option<crate::vtables::VtableMethodFn>,
    create_meeting: Option<crate::vtables::VtableMethodFn>,
    get_meeting: Option<crate::vtables::VtableMethodFn>,
    delete_meeting: Option<crate::vtables::VtableMethodFn>,
    resolve_meeting: Option<crate::vtables::VtableMethodFn>,
    list_meetings: Option<crate::vtables::VtableMethodFn>,
}

impl FfiVcAdapter {
    /// Wrap a loaded vc-adapter plugin instance so it can be
    /// handed to the host as `Arc<dyn VcAdapter>`. Returns
    /// `None` if the vtable pointer is NULL or the minimum-
    /// surface check fails (need at least create_meeting +
    /// delete_meeting to be useful — see
    /// [`VcVtable::has_minimum_surface`]).
    pub fn new(instance: Arc<LoadedInstance>) -> Option<Self> {
        let plugin = instance.plugin().clone();
        let raw = plugin.vtable_ptr();
        if raw.is_null() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "vc plugin has NULL vtable; refusing to wrap",
            );
            return None;
        }
        // SAFETY: manifest declares plugin_type =
        // videoconference-adapter, so the vtable pointer is a
        // *const VcVtable per the ABI contract.
        let vtable_ref: &VcVtable = unsafe { &*(raw as *const VcVtable) };
        // Read `vtable_version` BEFORE trusting the rest of the layout. It is at
        // offset 0 in every vtable in every revision, so it is the one field
        // that is safe to read before the layout is known — and until this
        // check existed, a plugin built against a shorter revision passed the
        // loader on `abi_version` alone and the host read past the end of its
        // struct. See `vtables::vtable_layout_ok`.
        if !crate::vtables::vtable_layout_ok(vtable_ref.vtable_version) {
            {
                warn!(
                    plugin_id = %plugin.manifest.id,
                    vtable_version = vtable_ref.vtable_version,
                    host_abi = crate::ABI_VERSION,
                    "vc plugin's vtable declares an unknown layout revision; refusing to wrap",
                );
                return None;
            }
        }
        if !vtable_ref.has_minimum_surface() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "vc plugin's vtable lacks create_meeting/delete_meeting; refusing to wrap",
            );
            return None;
        }
        let snapshot = VtableSnapshot {
            test_connection: vtable_ref.test_connection,
            create_meeting: vtable_ref.create_meeting,
            get_meeting: vtable_ref.get_meeting,
            delete_meeting: vtable_ref.delete_meeting,
            resolve_meeting: vtable_ref.resolve_meeting,
            list_meetings: vtable_ref.list_meetings,
        };
        let handle_addr = instance.handle() as usize;
        let in_flight = Arc::clone(plugin.in_flight_handle());
        Some(Self {
            _instance: instance,
            handle_addr,
            vtable: snapshot,
            in_flight,
        })
    }
}

/// Plugin status → `vc_core::VcError`. Same mapping
/// philosophy as the sync shim: each status code lands in the
/// closest `VcError` variant, with a fallback to `Internal`
/// for anything unknown.
fn status_to_vc_error(outcome: CallOutcome) -> VcError {
    let msg = outcome.message();
    match outcome.status {
        PLUGIN_CALL_ERR_UNSUPPORTED => {
            VcError::Unsupported(format!("plugin missing method: {msg}"))
        }
        PLUGIN_CALL_ERR_INVALID => VcError::InvalidInput(format!("plugin rejected args: {msg}")),
        PLUGIN_CALL_ERR_AUTH => VcError::Authentication(msg),
        PLUGIN_CALL_ERR_FORBIDDEN => VcError::Forbidden(msg),
        PLUGIN_CALL_ERR_NETWORK => VcError::Network(msg),
        PLUGIN_CALL_ERR_NOT_FOUND => VcError::NotFound(msg),
        PLUGIN_CALL_ERR_PROTOCOL => VcError::Protocol(msg),
        PLUGIN_CALL_ERR_CONFLICT => VcError::Protocol(format!("conflict: {msg}")),
        PLUGIN_CALL_ERR_IO => VcError::Network(format!("io: {msg}")),
        PLUGIN_CALL_ERR_INTERNAL => VcError::Internal(msg),
        other => VcError::Internal(format!("plugin status {other}: {msg}")),
    }
}

async fn call_then_decode<T, A>(
    method: Option<crate::vtables::VtableMethodFn>,
    instance_addr: usize,
    args: &A,
) -> VcResult<T>
where
    T: serde::de::DeserializeOwned,
    A: Serialize,
{
    let bytes = encode_args(args).map_err(|e| VcError::Internal(format!("encode args: {e}")))?;
    let outcome = call_method(method, instance_addr, bytes).await;
    if outcome.is_ok() {
        decode_payload(&outcome.bytes)
            .map_err(|e| VcError::Protocol(format!("decode plugin response: {e}")))
    } else {
        Err(status_to_vc_error(outcome))
    }
}

async fn call_for_unit<A: Serialize>(
    method: Option<crate::vtables::VtableMethodFn>,
    instance_addr: usize,
    args: &A,
) -> VcResult<()> {
    let bytes = encode_args(args).map_err(|e| VcError::Internal(format!("encode args: {e}")))?;
    let outcome = call_method(method, instance_addr, bytes).await;
    if outcome.is_ok() {
        Ok(())
    } else {
        Err(status_to_vc_error(outcome))
    }
}

#[async_trait]
impl VcAdapter for FfiVcAdapter {
    async fn test_connection(&self) -> VcResult<()> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_for_unit(self.vtable.test_connection, self.handle_addr, &()).await
    }

    async fn create_meeting(&self, spec: NewMeeting) -> VcResult<Meeting> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.create_meeting, self.handle_addr, &spec).await
    }

    async fn get_meeting(&self, id: &MeetingId) -> VcResult<Option<Meeting>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.get_meeting, self.handle_addr, id).await
    }

    async fn delete_meeting(&self, id: &MeetingId) -> VcResult<()> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_for_unit(self.vtable.delete_meeting, self.handle_addr, id).await
    }

    fn can_list_meetings(&self) -> bool {
        self.vtable.list_meetings.is_some()
    }

    async fn resolve_meeting(&self, join_url: &str) -> VcResult<Option<Meeting>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(
            self.vtable.resolve_meeting,
            self.handle_addr,
            &serde_json::json!({ "join_url": join_url }),
        )
        .await
    }

    async fn list_meetings(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> VcResult<Vec<Meeting>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(
            self.vtable.list_meetings,
            self.handle_addr,
            &serde_json::json!({ "start": start, "end": end }),
        )
        .await
    }
}
