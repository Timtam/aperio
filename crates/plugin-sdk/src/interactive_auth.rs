//! Helpers for the plugin's optional `interactive_auth` FFI
//! entry point.
//!
//! The C-ABI signature is:
//!
//! ```ignore
//! unsafe extern "C" fn aperio_plugin_interactive_auth(
//!     args_ptr: *const u8,
//!     args_len: usize,
//! ) -> PluginCallResult
//! ```
//!
//! Plugins that need an OAuth dance (or any other interactive
//! setup step the user has to drive through) export this
//! alongside the lifecycle exports. The host's
//! [`plugin_core::PluginManager::interactive_auth`] does a
//! libloading symbol lookup at plugin-load time + caches the
//! result; plugins that don't export the symbol just have the
//! capability marked as unavailable.
//!
//! [`interactive_auth_with`] is the helper for the OAuth-style
//! case: it spins up a one-shot tokio runtime, hands the
//! supplied args JSON to the plugin author's async closure, +
//! marshals the resulting credential blob (on success) or
//! error string (on failure) back across the FFI boundary.
//! `declare_interactive_auth!` emits the `#[no_mangle]` wrapper
//! that calls into [`interactive_auth_with`] from the
//! aperio_plugin_interactive_auth symbol the host looks up.

use std::future::Future;

use plugin_core::ffi::{
    PluginCallResult, PLUGIN_CALL_ERR_AUTH, PLUGIN_CALL_ERR_INTERNAL, PLUGIN_CALL_ERR_INVALID,
    PLUGIN_CALL_OK,
};

use crate::response::{bytes_to_response, error_response};
use crate::runtime::PluginRuntime;

/// Drive an OAuth-style interactive flow on a fresh one-shot
/// tokio runtime + marshal the result into the
/// [`PluginCallResult`] the host expects.
///
/// `handler` takes the args JSON as `&str` and returns the
/// credential blob the host should store opaquely (typically a
/// JSON-encoded TokenSet). An [`Err`] flows back as
/// [`PLUGIN_CALL_ERR_AUTH`] with the error string as payload —
/// most OAuth failures fall under that category (revoked
/// consent, browser-closed timeout, …). Plugins that need to
/// distinguish other status codes can build the
/// [`PluginCallResult`] manually via [`error_response`].
///
/// The runtime gets dropped at the end of the call —
/// shutdown_background means we don't block the caller; the
/// browser-tab listener is teardown-safe.
///
/// # Safety
///
/// `args_ptr` + `args_len` must describe a buffer of JSON bytes
/// the host owns for the duration of the call (which the
/// PluginManager's spawn_blocking wrapper guarantees).
pub unsafe fn interactive_auth_with<F, Fut>(
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
    // Copy into an owned String before handing to the handler.
    // Passing `&str` here would force the async closure's
    // returned future to borrow from a stack-local — Rust's
    // lifetime checker can't see through the FnOnce + Future
    // combination, so the cleanest API is just to give the
    // handler ownership.
    let json_str = match std::str::from_utf8(args_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            return error_response(
                PLUGIN_CALL_ERR_INVALID,
                "interactive_auth args are not valid UTF-8",
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
        Err(msg) => error_response(PLUGIN_CALL_ERR_AUTH, &msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip via the typed-closure shim: bytes in, bytes
    /// out, status OK.
    #[test]
    fn ok_closure_yields_ok_payload() {
        let args = br#"{"client_id":"alice"}"#;
        let mut result = unsafe {
            interactive_auth_with(args.as_ptr(), args.len(), |json| async move {
                assert_eq!(json, r#"{"client_id":"alice"}"#);
                Ok(b"refresh-token-bytes".to_vec())
            })
        };
        assert_eq!(result.status, plugin_core::ffi::PLUGIN_CALL_OK);
        let bytes = unsafe { result.payload.as_slice() };
        assert_eq!(bytes, b"refresh-token-bytes");
        unsafe { result.payload.free_in_place() };
    }

    /// `Err` from the closure surfaces as PLUGIN_CALL_ERR_AUTH
    /// with the error message as the payload — that's what the
    /// host's `InteractiveAuthError::Plugin` reads back.
    #[test]
    fn err_closure_yields_auth_error_with_message() {
        let args = b"";
        let mut result = unsafe {
            interactive_auth_with(args.as_ptr(), args.len(), |_| async move {
                Err::<Vec<u8>, _>("user closed the consent screen".to_string())
            })
        };
        assert_eq!(result.status, PLUGIN_CALL_ERR_AUTH);
        let bytes = unsafe { result.payload.as_slice() };
        assert_eq!(
            std::str::from_utf8(bytes).unwrap(),
            "user closed the consent screen",
        );
        unsafe { result.payload.free_in_place() };
    }

    /// Non-UTF-8 args surface as PLUGIN_CALL_ERR_INVALID before
    /// the user's closure runs.
    #[test]
    fn non_utf8_args_short_circuit_to_invalid() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        CALLED.store(false, Ordering::SeqCst);
        // 0xFF is invalid UTF-8 — `from_utf8` rejects it.
        let bad = &[0xFFu8];
        let mut result = unsafe {
            interactive_auth_with(bad.as_ptr(), bad.len(), |_| async move {
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
