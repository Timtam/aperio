//! Decode JSON-encoded arguments inside an FFI fn.
//!
//! The host serialises the trait method's typed arguments to
//! JSON and passes the bytes via a `*const u8 + len` pair. The
//! plugin's FFI fn calls [`decode_args`] right at the top to
//! turn that back into a typed Rust value before driving the
//! actual `async fn`.
//!
//! Failures here are programmer errors in the host (the JSON
//! shape doesn't match what the plugin expected), not user-
//! facing problems — so we surface them as
//! `PLUGIN_CALL_ERR_INVALID` with the serde error message
//! attached. The host's shim wrapper folds that into
//! `cal_core::Error::InvalidInput`, which gives the user a
//! clear-enough "communication broke between Aperio and the
//! plugin" message.

use plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INVALID};
use serde::de::DeserializeOwned;

use crate::response::error_response;

/// Decode the FFI args pointer pair into a typed value. Returns
/// either the parsed `T` or a ready-to-return error
/// [`PluginCallResult`] the FFI fn can yield directly.
///
/// Empty inputs (NULL pointer or zero length) decode as the
/// JSON `null` literal — that's the host's convention for
/// void-arg methods, which avoids the plugin having to special-
/// case "no args present" vs "args are JSON null".
///
/// # Safety
///
/// `ptr` must point at `len` valid bytes the host owns for the
/// duration of the call. The host contract in
/// [`plugin_core::ffi`] guarantees this for every vtable
/// invocation — the bytes are valid until the FFI fn returns.
pub unsafe fn decode_args<T: DeserializeOwned>(
    ptr: *const u8,
    len: usize,
) -> Result<T, PluginCallResult> {
    let bytes: &[u8] = if ptr.is_null() || len == 0 {
        b"null"
    } else {
        // SAFETY: caller (host) guarantees the pointer is valid
        // for `len` bytes for the duration of the call.
        std::slice::from_raw_parts(ptr, len)
    };
    serde_json::from_slice::<T>(bytes).map_err(|err| {
        error_response(
            PLUGIN_CALL_ERR_INVALID,
            &format!("decode args: {err}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct ListArgs {
        list_id: String,
    }

    #[test]
    fn decodes_typed_json() {
        let json = br#"{"list_id":"cal-1"}"#;
        let parsed: ListArgs =
            unsafe { decode_args(json.as_ptr(), json.len()).expect("parses") };
        assert_eq!(
            parsed,
            ListArgs {
                list_id: "cal-1".into()
            }
        );
    }

    #[test]
    fn empty_pointer_decodes_as_null() {
        let parsed: Option<u32> =
            unsafe { decode_args(std::ptr::null(), 0).expect("parses") };
        assert!(parsed.is_none());
    }

    #[test]
    fn empty_len_decodes_as_null() {
        let bytes = b"";
        let parsed: Option<u32> =
            unsafe { decode_args(bytes.as_ptr(), 0).expect("parses") };
        assert!(parsed.is_none());
    }

    #[test]
    fn malformed_json_yields_invalid_status() {
        let bad = br"{not json";
        let err: PluginCallResult =
            unsafe { decode_args::<ListArgs>(bad.as_ptr(), bad.len()).unwrap_err() };
        assert_eq!(err.status, PLUGIN_CALL_ERR_INVALID);
        let mut p = err.payload;
        unsafe { p.free_in_place() };
    }
}
