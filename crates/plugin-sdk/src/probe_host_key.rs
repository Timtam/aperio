//! Helpers for the plugin's optional `probe_host_key` FFI entry
//! point.
//!
//! The C-ABI signature is:
//!
//! ```ignore
//! unsafe extern "C" fn aperio_plugin_probe_host_key(
//!     args_ptr: *const u8,
//!     args_len: usize,
//! ) -> PluginCallResult
//! ```
//!
//! Plugins wrapping a TOFU-style transport (SFTP today) export
//! this alongside the lifecycle exports so the host's trust
//! dialog can read the server's presented fingerprint without
//! committing a pin or even authenticating. The host's
//! [`plugin_core::PluginManager::probe_host_key`] does a
//! libloading symbol lookup at plugin-load time + caches the
//! result; plugins that don't export the symbol just have the
//! capability marked as unavailable.
//!
//! [`probe_host_key_with`] is the helper: it spins up a one-shot
//! tokio runtime, hands the supplied args JSON to the plugin
//! author's async closure, + marshals the resulting fingerprint
//! blob (on success) or error string (on failure) back across
//! the FFI boundary. `declare_probe_host_key!` emits the
//! `#[no_mangle]` wrapper that calls into [`probe_host_key_with`]
//! from the aperio_plugin_probe_host_key symbol the host looks
//! up.

use std::future::Future;

use plugin_core::ffi::{
    PluginCallResult, PLUGIN_CALL_ERR_INTERNAL, PLUGIN_CALL_ERR_INVALID, PLUGIN_CALL_ERR_NETWORK,
    PLUGIN_CALL_OK,
};

use crate::response::{bytes_to_response, error_response};
use crate::runtime::PluginRuntime;

/// Drive a host-key probe on a fresh one-shot tokio runtime +
/// marshal the result into the [`PluginCallResult`] the host
/// expects.
///
/// `handler` takes the args JSON as `String` and returns the
/// fingerprint blob the host should consume (typically a JSON-
/// encoded struct like `{"fingerprint": "SHA256:..."}`). An
/// [`Err`] flows back as [`PLUGIN_CALL_ERR_NETWORK`] with the
/// error string as payload — most probe failures are connection
/// problems (dead host, TLS handshake, …), which is what the
/// trust-dialog UX renders as "couldn't reach the server".
/// Plugins that need to distinguish other status codes can build
/// the [`PluginCallResult`] manually via [`error_response`].
///
/// The runtime gets dropped at the end of the call —
/// shutdown_background means we don't block the caller.
///
/// # Safety
///
/// `args_ptr` + `args_len` must describe a buffer of JSON bytes
/// the host owns for the duration of the call (which the
/// PluginManager's spawn_blocking wrapper guarantees).
pub unsafe fn probe_host_key_with<F, Fut>(
    args_ptr: *const u8,
    args_len: usize,
    handler: F,
) -> PluginCallResult
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let args_bytes: &[u8] = if args_ptr.is_null() || args_len == 0 {
        &[]
    } else {
        // SAFETY: host contract — pointer is valid for `args_len`
        // bytes for the duration of the call.
        std::slice::from_raw_parts(args_ptr, args_len)
    };
    let json_str = match std::str::from_utf8(args_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            return error_response(
                PLUGIN_CALL_ERR_INVALID,
                "probe_host_key args are not valid UTF-8",
            )
        }
    };
    let runtime = match PluginRuntime::new() {
        Ok(r) => r,
        Err(err) => {
            return error_response(PLUGIN_CALL_ERR_INTERNAL, &format!("build runtime: {err}"))
        }
    };
    match runtime.block_on(handler(json_str)) {
        Ok(blob) => bytes_to_response(PLUGIN_CALL_OK, blob),
        Err(msg) => error_response(PLUGIN_CALL_ERR_NETWORK, &msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip via the typed-closure shim: bytes in, bytes
    /// out, status OK.
    #[test]
    fn ok_closure_yields_ok_payload() {
        let args = br#"{"host":"nas","port":22}"#;
        let mut result = unsafe {
            probe_host_key_with(args.as_ptr(), args.len(), |json| async move {
                assert_eq!(json, r#"{"host":"nas","port":22}"#);
                Ok(br#"{"fingerprint":"SHA256:abc"}"#.to_vec())
            })
        };
        assert_eq!(result.status, plugin_core::ffi::PLUGIN_CALL_OK);
        let bytes = unsafe { result.payload.as_slice() };
        assert_eq!(bytes, br#"{"fingerprint":"SHA256:abc"}"#);
        unsafe { result.payload.free_in_place() };
    }

    /// `Err` from the closure surfaces as
    /// PLUGIN_CALL_ERR_NETWORK with the error message as the
    /// payload — that's what the host's
    /// `ProbeHostKeyError::Plugin` reads back.
    #[test]
    fn err_closure_yields_network_error_with_message() {
        let args = b"";
        let mut result = unsafe {
            probe_host_key_with(args.as_ptr(), args.len(), |_| async move {
                Err::<Vec<u8>, _>("connection refused".to_string())
            })
        };
        assert_eq!(result.status, PLUGIN_CALL_ERR_NETWORK);
        let bytes = unsafe { result.payload.as_slice() };
        assert_eq!(std::str::from_utf8(bytes).unwrap(), "connection refused",);
        unsafe { result.payload.free_in_place() };
    }

    /// Non-UTF-8 args surface as PLUGIN_CALL_ERR_INVALID before
    /// the user's closure runs.
    #[test]
    fn non_utf8_args_short_circuit_to_invalid() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        CALLED.store(false, Ordering::SeqCst);
        let bad = &[0xFFu8];
        let mut result = unsafe {
            probe_host_key_with(bad.as_ptr(), bad.len(), |_| async move {
                CALLED.store(true, Ordering::SeqCst);
                Ok(Vec::new())
            })
        };
        assert!(
            !CALLED.load(Ordering::SeqCst),
            "closure must not run on non-UTF-8 args",
        );
        assert_eq!(result.status, PLUGIN_CALL_ERR_INVALID);
        unsafe { result.payload.free_in_place() };
    }
}
