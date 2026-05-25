//! Helpers for the plugin's `open_instance` FFI fn.
//!
//! The C-ABI signature is:
//!
//! ```ignore
//! unsafe extern "C" fn plugin_open_instance(
//!     config_json: *const c_char,
//! ) -> OpenInstanceResult
//! ```
//!
//! Almost every plugin's open hook follows the same skeleton:
//! parse `config_json` as UTF-8, deserialise it into the per-
//! plugin `InitConfig`, build the adapter, and box it through
//! [`crate::PluginInstance`]. [`open_instance_with`] wraps that
//! boilerplate so the plugin author writes a single closure
//! that takes `&str` and returns `Result<T, String>` — the
//! error half flows back through `OpenInstanceResult.error` as
//! a UTF-8 detail the host renders verbatim.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

use plugin_core::abi::{OpenInstanceResult, PLUGIN_ERR_INIT, PLUGIN_ERR_INVALID_CONFIG, PLUGIN_OK};
use plugin_core::ffi::PluginBytes;

use crate::instance::PluginInstance;

/// Build an [`OpenInstanceResult`] from a closure that takes the
/// raw config JSON and returns the adapter value (or an error
/// string surfaced to the user).
///
/// The closure receives the JSON as a borrowed `&str`. Empty /
/// NULL configs come through as `""` so the closure can treat
/// them uniformly with a serde_json fall-back default.
///
/// On success: wraps the adapter in a fresh
/// [`PluginInstance<T>`] (which spins up the plugin's own tokio
/// runtime), leaks it as `*mut c_void`, and returns it in
/// `instance` with `status = PLUGIN_OK`.
///
/// On error: returns a NULL handle with the matching
/// `PLUGIN_ERR_*` status and a UTF-8 error message in `error`
/// (host frees + surfaces).
///
/// # Safety
///
/// `config_json` must be NUL-terminated UTF-8 as the C ABI
/// requires (the host's [`plugin_core::manager::PluginManager::open_instance`]
/// guarantees this).
pub unsafe fn open_instance_with<T, F>(config_json: *const c_char, build: F) -> OpenInstanceResult
where
    F: FnOnce(&str) -> Result<T, String>,
{
    let json_str: &str = if config_json.is_null() {
        ""
    } else {
        match CStr::from_ptr(config_json).to_str() {
            Ok(s) => s,
            Err(_) => {
                return error_result(PLUGIN_ERR_INVALID_CONFIG, "config_json is not valid UTF-8")
            }
        }
    };
    let adapter = match build(json_str) {
        Ok(a) => a,
        Err(msg) => return error_result(PLUGIN_ERR_INVALID_CONFIG, &msg),
    };
    let instance = match PluginInstance::new(adapter) {
        Ok(i) => i,
        Err(e) => return error_result(PLUGIN_ERR_INIT, &format!("{e}")),
    };
    OpenInstanceResult {
        instance: instance.into_raw_handle(),
        status: PLUGIN_OK,
        error: PluginBytes::empty(),
    }
}

/// Build a NULL-handle [`OpenInstanceResult`] with the given
/// status + UTF-8 error message. Used internally by
/// [`open_instance_with`]; exposed `pub` so plugin authors with
/// bespoke open paths can produce the same shape.
pub fn error_result(status: c_int, msg: &str) -> OpenInstanceResult {
    let bytes = msg.as_bytes().to_vec();
    let mut boxed = bytes.into_boxed_slice();
    let data = boxed.as_mut_ptr();
    let len = boxed.len();
    std::mem::forget(boxed);
    unsafe extern "C" fn free_boxed(data: *mut u8, len: usize) {
        if !data.is_null() {
            let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len));
        }
    }
    OpenInstanceResult {
        instance: std::ptr::null_mut(),
        status,
        error: PluginBytes {
            data,
            len,
            free: Some(free_boxed),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn open_with_ok_closure_yields_non_null_handle() {
        let cfg = CString::new(r#"{"name":"alice"}"#).unwrap();
        let result = unsafe {
            open_instance_with::<u32, _>(cfg.as_ptr(), |json| {
                assert_eq!(json, r#"{"name":"alice"}"#);
                Ok(42)
            })
        };
        assert_eq!(result.status, PLUGIN_OK);
        assert!(!result.instance.is_null());
        // Borrow back + verify the value crossed the boxing
        // boundary intact.
        let inst =
            unsafe { PluginInstance::<u32>::from_handle(result.instance) }.expect("non-null");
        assert_eq!(*inst.plugin(), 42);
        // Drop to keep miri happy.
        unsafe { PluginInstance::<u32>::drop_handle(result.instance) };
    }

    #[test]
    fn open_with_err_closure_yields_null_handle_and_message() {
        let cfg = CString::new(r#"{"bad":true}"#).unwrap();
        let mut result = unsafe {
            open_instance_with::<u32, _>(cfg.as_ptr(), |_| {
                Err("config missing required field".to_string())
            })
        };
        assert_eq!(result.status, PLUGIN_ERR_INVALID_CONFIG);
        assert!(result.instance.is_null());
        // Error bytes round-trip as the original message.
        // SAFETY: error came from error_result above; not freed.
        let bytes = unsafe { result.error.as_slice() };
        assert_eq!(
            std::str::from_utf8(bytes).unwrap(),
            "config missing required field"
        );
        unsafe { result.error.free_in_place() };
    }

    #[test]
    fn null_config_pointer_passes_empty_string_to_closure() {
        let result = unsafe {
            open_instance_with::<u32, _>(std::ptr::null(), |json| {
                assert_eq!(json, "");
                Ok(7)
            })
        };
        assert_eq!(result.status, PLUGIN_OK);
        assert!(!result.instance.is_null());
        unsafe { PluginInstance::<u32>::drop_handle(result.instance) };
    }
}
