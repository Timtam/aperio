//! Shared call infrastructure for every FFI shim.
//!
//! - [`encode_args`]: serialise typed args to JSON bytes.
//! - [`call_method`]: dispatch one vtable call through
//!   `spawn_blocking`, return the raw payload + status as a
//!   [`CallOutcome`].
//! - [`decode_payload`]: deserialise bytes into the typed
//!   response.
//!
//! The shims themselves handle the typed-trait-method side
//! (turning a [`CallOutcome`] into a `cal_core::Error` variant
//! etc.) so this module stays type-agnostic + reusable by all 4
//! shim families.

use std::os::raw::c_void;

use serde::Serialize;
use tracing::warn;

use crate::ffi::PLUGIN_CALL_OK;
use crate::vtables::VtableMethodFn;

/// Internal representation of a finished FFI call. The shim
/// converts this into its typed error before handing it back
/// to the host.
pub(crate) struct CallOutcome {
    pub status: i32,
    /// JSON bytes — response on `status == 0`, UTF-8 error
    /// message otherwise. May be empty in either case.
    pub bytes: Vec<u8>,
}

impl CallOutcome {
    /// True iff the call succeeded.
    pub fn is_ok(&self) -> bool {
        self.status == PLUGIN_CALL_OK
    }

    /// Best-effort UTF-8 view of the bytes — used to surface the
    /// plugin's error message in the typed error variant.
    pub fn message(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Serialise `args` to JSON bytes. The shim feeds the resulting
/// `Vec<u8>` straight into [`call_method`].
///
/// `()` and unit-shaped args end up as a 4-byte `"null"` payload,
/// which every JSON-aware plugin trivially handles.
pub(crate) fn encode_args<T: Serialize>(args: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(args).map_err(|e| format!("encode args: {e}"))
}

/// Run one vtable call. Wraps the FFI invocation in
/// `tokio::task::spawn_blocking` so a blocking plugin can't tie
/// up the async reactor; the caller still awaits the resulting
/// future like any other async method.
///
/// `instance_addr` is the opaque per-account handle the host got
/// from the plugin's `open_instance` hook, passed as `usize` so
/// it crosses the await boundary cleanly (raw pointers are not
/// `Send`). May be `0` for process-global plugins. `method` is
/// the `Option<VtableMethodFn>` slot from the relevant vtable
/// struct — a `None` slot returns a synthetic
/// `PLUGIN_CALL_ERR_UNSUPPORTED` outcome so the shim's status-
/// to-error mapping does the right thing without bespoke
/// branching at each call site.
pub(crate) async fn call_method(
    method: Option<VtableMethodFn>,
    instance_addr: usize,
    args: Vec<u8>,
) -> CallOutcome {
    let Some(method) = method else {
        return CallOutcome {
            status: crate::ffi::PLUGIN_CALL_ERR_UNSUPPORTED,
            bytes: b"method not implemented by plugin".to_vec(),
        };
    };

    // Move the args into the blocking closure so the lifetime
    // story is trivially correct: the plugin sees a pointer to
    // bytes owned by the closure, and the closure outlives the
    // call. We extract status + payload bytes + free the plugin
    // buffer INSIDE the closure so what crosses the await
    // boundary is a plain (i32, Vec<u8>) — `PluginCallResult`
    // itself isn't `Send` because of its raw-pointer field.
    let join: Result<(i32, Vec<u8>), _> = tokio::task::spawn_blocking(move || {
        let instance = instance_addr as *mut c_void;
        // SAFETY: vtable method pointer is valid for the lifetime
        // of the LoadedPlugin we got it from; the caller keeps an
        // Arc<LoadedInstance> alive across this await (see the
        // FfiCalendarAdapter struct field for the canonical
        // pattern), which in turn keeps the LoadedPlugin Arc + the
        // library alive. args is a Vec<u8> we own; the pointer is
        // valid for the duration of the synchronous call.
        let result = unsafe { method(instance, args.as_ptr(), args.len()) };
        // SAFETY: result.payload is owned by the plugin; we copy
        // the bytes BEFORE returning the buffer to the plugin's
        // allocator. After free_in_place runs the original
        // pointer is invalidated, which is fine because we've
        // already taken a Vec<u8> copy.
        let bytes = unsafe { result.payload.as_slice().to_vec() };
        let status = result.status;
        let mut payload = result.payload;
        unsafe { payload.free_in_place() };
        (status, bytes)
    })
    .await;

    match join {
        Ok((status, bytes)) => CallOutcome { status, bytes },
        Err(join_err) => {
            warn!(?join_err, "plugin call panicked or was cancelled");
            CallOutcome {
                status: crate::ffi::PLUGIN_CALL_ERR_INTERNAL,
                bytes: format!("plugin task: {join_err}").into_bytes(),
            }
        }
    }
}

/// Synchronous variant of [`call_method`] for trait methods that
/// don't go through async (e.g. `CalendarFeature::calendar_color`).
/// Same instance-handle threading, no `spawn_blocking` —
/// implementations are expected to answer from in-memory state
/// without IO.
pub(crate) fn call_method_sync(
    method: Option<VtableMethodFn>,
    instance_addr: usize,
    args: Vec<u8>,
) -> CallOutcome {
    let Some(method) = method else {
        return CallOutcome {
            status: crate::ffi::PLUGIN_CALL_ERR_UNSUPPORTED,
            bytes: b"method not implemented by plugin".to_vec(),
        };
    };
    // SAFETY: same contract as call_method's blocking branch —
    // args is a Vec<u8> we own; pointer is valid for the
    // duration of the call.
    let instance = instance_addr as *mut c_void;
    let result = unsafe { method(instance, args.as_ptr(), args.len()) };
    let bytes = unsafe { result.payload.as_slice().to_vec() };
    let status = result.status;
    let mut payload = result.payload;
    unsafe { payload.free_in_place() };
    CallOutcome { status, bytes }
}

/// Decode the OK payload into a typed value. Empty payload is
/// interpreted as JSON `null` so void-returning methods can keep
/// their FFI shape trivial.
pub(crate) fn decode_payload<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    if bytes.is_empty() {
        // serde_json's null handles all the Option, (), and Vec
        // shapes our shims need to deserialise.
        return serde_json::from_slice::<T>(b"null")
            .map_err(|e| format!("decode empty payload: {e}"));
    }
    serde_json::from_slice(bytes).map_err(|e| format!("decode payload: {e}"))
}
