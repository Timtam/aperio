//! Build [`PluginCallResult`]s that the host can decode.
//!
//! The host side of the FFI bridge expects:
//!   - On success: status = [`PLUGIN_CALL_OK`] + payload = JSON
//!     bytes of the response value (or empty for void methods).
//!   - On error: status = one of the `PLUGIN_CALL_ERR_*` codes
//!     + payload = UTF-8 error message bytes.
//!
//! Plugin authors construct one of these per FFI fn call. The
//! response builders all use the same memory ownership trick:
//! the Vec<u8> is converted to a Box<[u8]>, the box is leaked,
//! and a [`free_boxed_slice`] function pointer is attached to
//! the [`PluginBytes`] so the host can hand the buffer back to
//! the plugin's allocator when it's done with it.

use plugin_core::ffi::{
    PluginBytes, PluginCallResult, PLUGIN_CALL_ERR_INTERNAL, PLUGIN_CALL_OK,
};
use serde::Serialize;
use std::os::raw::c_int;

/// Plugin-side free fn used by [`bytes_to_response`] for every
/// payload allocation. Reconstructs the `Box<[u8]>` that
/// [`bytes_to_response`] leaked, lets it drop, returning the
/// memory to the plugin's allocator. The host calls this through
/// [`PluginBytes::free`] after decoding the JSON.
///
/// # Safety
///
/// `data` must be the exact pointer returned by an
/// [`bytes_to_response`] in this same shared library, and `len`
/// must be the length passed alongside it. Calling with anything
/// else is undefined behaviour. The host only ever round-trips
/// our own bytes back to us, so the contract holds.
pub unsafe extern "C" fn free_boxed_slice(data: *mut u8, len: usize) {
    if data.is_null() || len == 0 {
        return;
    }
    // SAFETY: matches the layout bytes_to_response produced via
    // Box::into_raw on the Box<[u8]>. We rebuild the fat pointer
    // with the same length so the allocator's bookkeeping
    // matches the original allocation.
    let _ = unsafe {
        Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len))
    };
}

/// Wrap raw bytes + status into a [`PluginCallResult`]. Used by
/// both the OK + error response builders so the leak/free
/// dance lives in exactly one place. Empty `bytes` becomes the
/// empty-payload sentinel.
pub fn bytes_to_response(status: c_int, bytes: Vec<u8>) -> PluginCallResult {
    if bytes.is_empty() {
        return PluginCallResult {
            status,
            payload: PluginBytes::empty(),
        };
    }
    let mut boxed = bytes.into_boxed_slice();
    let data = boxed.as_mut_ptr();
    let len = boxed.len();
    std::mem::forget(boxed);
    PluginCallResult {
        status,
        payload: PluginBytes {
            data,
            len,
            free: Some(free_boxed_slice),
        },
    }
}

/// Build a successful [`PluginCallResult`] from a `Serialize`-able
/// value. Serialisation failure is genuinely unexpected (the
/// trait methods only ever return types we control), but if it
/// happens we surface an `Internal` error rather than panicking
/// across the FFI boundary.
pub fn ok_response<T: Serialize>(value: &T) -> PluginCallResult {
    match serde_json::to_vec(value) {
        Ok(bytes) => bytes_to_response(PLUGIN_CALL_OK, bytes),
        Err(err) => error_response(
            PLUGIN_CALL_ERR_INTERNAL,
            &format!("encode response: {err}"),
        ),
    }
}

/// Shortcut for `Result<()>`-shaped trait methods. Same as
/// `ok_response(&())` but skips the serde step entirely.
pub fn ok_empty_response() -> PluginCallResult {
    PluginCallResult::ok_empty()
}

/// Error response with the given status + UTF-8 message. The
/// host's shim wrappers feed the message into the matching
/// `cal_core::Error` / `sync_core::SyncError` variant.
pub fn error_response(status: c_int, msg: &str) -> PluginCallResult {
    bytes_to_response(status, msg.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_core::ffi::PLUGIN_CALL_ERR_AUTH;

    #[test]
    fn ok_empty_has_empty_payload() {
        let r = ok_empty_response();
        assert_eq!(r.status, PLUGIN_CALL_OK);
        assert!(r.payload.is_empty());
    }

    #[test]
    fn ok_response_serialises_value() {
        let r = ok_response(&vec![1u32, 2, 3]);
        assert_eq!(r.status, PLUGIN_CALL_OK);
        // SAFETY: payload was allocated by us via the free
        // function attached below.
        let slice = unsafe { r.payload.as_slice() };
        assert_eq!(slice, b"[1,2,3]");
        // Round-trip free so we don't leak in the test process.
        let mut payload = r.payload;
        unsafe { payload.free_in_place() };
    }

    #[test]
    fn error_response_carries_utf8_message() {
        let r = error_response(PLUGIN_CALL_ERR_AUTH, "creds rejected");
        assert_eq!(r.status, PLUGIN_CALL_ERR_AUTH);
        let slice = unsafe { r.payload.as_slice() };
        assert_eq!(slice, b"creds rejected");
        let mut payload = r.payload;
        unsafe { payload.free_in_place() };
    }

    #[test]
    fn bytes_to_response_empty_input_yields_empty_sentinel() {
        let r = bytes_to_response(PLUGIN_CALL_OK, Vec::new());
        assert!(r.payload.is_empty());
    }
}
