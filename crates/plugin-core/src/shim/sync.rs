//! `FfiSyncAdapter` — `sync_core::SyncAdapter` impl that
//! dispatches across the FFI boundary into a loaded plugin's
//! [`crate::vtables::SyncVtable`].
//!
//! Same shape as [`super::calendar::FfiCalendarAdapter`], but the
//! status-to-error mapping targets `sync_core::SyncError`
//! instead of `cal_core::Error`. The sync trait's argument types
//! (LogFile, Snapshot, MetaJson, LogFileName, DeviceCursor) all
//! already derive `Serialize` / `Deserialize` in sync-core so the
//! JSON bridge works without bespoke wire shapes.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use sync_core::error::{SyncError, SyncResult};
use sync_core::log::{LogFile, LogFileName};
use sync_core::meta::MetaJson;
use sync_core::snapshot::Snapshot;
use sync_core::{DeviceCursor, SyncAdapter};
use tracing::warn;

use crate::ffi::*;
use crate::manager::LoadedInstance;
use crate::vtables::SyncVtable;

use super::call::{call_method, decode_payload, encode_args, CallOutcome};

pub struct FfiSyncAdapter {
    _instance: Arc<LoadedInstance>,
    handle_addr: usize,
    vtable: VtableSnapshot,
}

#[derive(Clone, Copy)]
struct VtableSnapshot {
    test_connection: Option<crate::vtables::VtableMethodFn>,
    fetch_meta: Option<crate::vtables::VtableMethodFn>,
    push_meta: Option<crate::vtables::VtableMethodFn>,
    fetch_new_logs: Option<crate::vtables::VtableMethodFn>,
    push_log: Option<crate::vtables::VtableMethodFn>,
    fetch_snapshot: Option<crate::vtables::VtableMethodFn>,
    push_snapshot: Option<crate::vtables::VtableMethodFn>,
    delete_log: Option<crate::vtables::VtableMethodFn>,
    push_sound_asset: Option<crate::vtables::VtableMethodFn>,
    fetch_sound_asset: Option<crate::vtables::VtableMethodFn>,
}

impl FfiSyncAdapter {
    /// Wrap a loaded sync-adapter plugin instance so it can be
    /// handed to the orchestrator as `Arc<dyn SyncAdapter>`.
    /// Returns `None` if the vtable pointer is NULL or the
    /// minimum-surface check fails (the sync trait needs at
    /// least fetch_meta / push_meta / fetch_new_logs / push_log
    /// to do anything useful).
    pub fn new(instance: Arc<LoadedInstance>) -> Option<Self> {
        let plugin = instance.plugin().clone();
        let raw = plugin.vtable_ptr();
        if raw.is_null() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "sync plugin has NULL vtable; refusing to wrap",
            );
            return None;
        }
        // SAFETY: the manifest declares plugin_type = sync-adapter,
        // so the vtable pointer is a *const SyncVtable per the
        // ABI contract.
        let vtable_ref: &SyncVtable = unsafe { &*(raw as *const SyncVtable) };
        if !vtable_ref.has_minimum_surface() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "sync plugin's vtable lacks fetch_meta/push_meta/fetch_new_logs/push_log; refusing to wrap",
            );
            return None;
        }
        let snapshot = VtableSnapshot {
            test_connection: vtable_ref.test_connection,
            fetch_meta: vtable_ref.fetch_meta,
            push_meta: vtable_ref.push_meta,
            fetch_new_logs: vtable_ref.fetch_new_logs,
            push_log: vtable_ref.push_log,
            fetch_snapshot: vtable_ref.fetch_snapshot,
            push_snapshot: vtable_ref.push_snapshot,
            delete_log: vtable_ref.delete_log,
            push_sound_asset: vtable_ref.push_sound_asset,
            fetch_sound_asset: vtable_ref.fetch_sound_asset,
        };
        let handle_addr = instance.handle() as usize;
        Some(Self {
            _instance: instance,
            handle_addr,
            vtable: snapshot,
        })
    }
}

/// Plugin status → `sync_core::SyncError`. SyncError doesn't
/// have direct counterparts for `Conflict` / `Forbidden` /
/// `Invalid` / `Unsupported` (the sync trait's methods don't
/// raise those at the type level), so they fold into the
/// closest match — `Protocol` for malformed args / unsupported
/// methods, `Auth` for forbidden, `Internal` for everything
/// else.
fn status_to_sync_error(outcome: CallOutcome) -> SyncError {
    let msg = outcome.message();
    match outcome.status {
        PLUGIN_CALL_ERR_UNSUPPORTED => {
            SyncError::Protocol(format!("plugin missing method: {msg}"))
        }
        PLUGIN_CALL_ERR_INVALID => {
            SyncError::Protocol(format!("plugin rejected args: {msg}"))
        }
        PLUGIN_CALL_ERR_AUTH | PLUGIN_CALL_ERR_FORBIDDEN => SyncError::Auth(msg),
        PLUGIN_CALL_ERR_NETWORK => SyncError::Network(msg),
        PLUGIN_CALL_ERR_NOT_FOUND => SyncError::NotFound(msg),
        PLUGIN_CALL_ERR_PROTOCOL => SyncError::Protocol(msg),
        PLUGIN_CALL_ERR_IO => SyncError::Io(msg),
        PLUGIN_CALL_ERR_CONFLICT => {
            SyncError::Protocol(format!("conflict: {msg}"))
        }
        PLUGIN_CALL_ERR_INTERNAL => SyncError::Internal(msg),
        other => SyncError::Internal(format!("plugin status {other}: {msg}")),
    }
}

async fn call_then_decode<T, A>(
    method: Option<crate::vtables::VtableMethodFn>,
    instance_addr: usize,
    args: &A,
) -> SyncResult<T>
where
    T: serde::de::DeserializeOwned,
    A: Serialize,
{
    let bytes = encode_args(args).map_err(|e| SyncError::Internal(format!(
        "encode args: {e}"
    )))?;
    let outcome = call_method(method, instance_addr, bytes).await;
    if outcome.is_ok() {
        decode_payload(&outcome.bytes).map_err(|e| SyncError::Protocol(format!(
            "decode plugin response: {e}"
        )))
    } else {
        Err(status_to_sync_error(outcome))
    }
}

async fn call_for_unit<A: Serialize>(
    method: Option<crate::vtables::VtableMethodFn>,
    instance_addr: usize,
    args: &A,
) -> SyncResult<()> {
    let bytes = encode_args(args).map_err(|e| SyncError::Internal(format!(
        "encode args: {e}"
    )))?;
    let outcome = call_method(method, instance_addr, bytes).await;
    if outcome.is_ok() {
        Ok(())
    } else {
        Err(status_to_sync_error(outcome))
    }
}

#[derive(Serialize)]
struct PushSoundAssetArgs<'a> {
    hash: &'a str,
    extension: &'a str,
    bytes_base64: String,
    _phantom: std::marker::PhantomData<&'a ()>,
}

#[derive(Serialize)]
struct FetchSoundAssetArgs<'a> {
    hash: &'a str,
    extension: &'a str,
}

#[async_trait]
impl SyncAdapter for FfiSyncAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        call_for_unit(self.vtable.test_connection, self.handle_addr, &()).await
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        call_then_decode(self.vtable.fetch_meta, self.handle_addr, &()).await
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        call_for_unit(self.vtable.push_meta, self.handle_addr, meta).await
    }

    async fn fetch_new_logs(
        &self,
        since: &DeviceCursor,
    ) -> SyncResult<Vec<LogFile>> {
        call_then_decode(self.vtable.fetch_new_logs, self.handle_addr, since).await
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        call_for_unit(self.vtable.push_log, self.handle_addr, log).await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        call_then_decode(self.vtable.fetch_snapshot, self.handle_addr, &()).await
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        call_for_unit(self.vtable.push_snapshot, self.handle_addr, snapshot).await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        call_for_unit(self.vtable.delete_log, self.handle_addr, name).await
    }

    async fn push_sound_asset(
        &self,
        hash: &str,
        extension: &str,
        bytes: &[u8],
    ) -> SyncResult<()> {
        use base64::Engine as _;
        let args = PushSoundAssetArgs {
            hash,
            extension,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            _phantom: std::marker::PhantomData,
        };
        call_for_unit(self.vtable.push_sound_asset, self.handle_addr, &args).await
    }

    async fn fetch_sound_asset(
        &self,
        hash: &str,
        extension: &str,
    ) -> SyncResult<Option<Vec<u8>>> {
        use base64::Engine as _;
        let args = FetchSoundAssetArgs { hash, extension };
        let maybe_b64: Option<String> =
            call_then_decode(self.vtable.fetch_sound_asset, self.handle_addr, &args).await?;
        match maybe_b64 {
            None => Ok(None),
            Some(s) => base64::engine::general_purpose::STANDARD
                .decode(s)
                .map(Some)
                .map_err(|e| SyncError::Protocol(format!(
                    "plugin returned bad base64 for sound asset: {e}"
                ))),
        }
    }
}
