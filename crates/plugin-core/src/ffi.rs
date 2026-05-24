//! Common FFI primitives shared by every vtable (DESIGN.md §20.3).
//!
//! Every async trait method on the cal-core / sync-core feature
//! traits crosses the plugin boundary as a synchronous function
//! call that takes JSON-encoded arguments and returns a
//! [`PluginCallResult`] — a status code plus a JSON-encoded
//! payload (either the response or an error message).
//!
//! ## Why JSON
//!
//! Per the project decision recorded at P1 plan time: JSON keeps
//! the bridge language-agnostic (the canonical promise of §20.3 —
//! plugins MAY be written in Rust, C, C++, Zig, Go, Swift, …),
//! and Aperio's workload (a few hundred adapter calls per sync
//! round) makes serialisation overhead utterly irrelevant compared
//! to the network round-trips the calls actually drive.
//!
//! ## Memory ownership
//!
//! Arguments flow host → plugin: the host hands the plugin a
//! `*const u8 + usize` pair that's valid for the duration of the
//! call. The plugin copies whatever it needs before returning.
//!
//! Results flow plugin → host: the plugin returns a [`PluginBytes`]
//! whose `data` pointer was allocated by the plugin's allocator.
//! It carries its own [`PluginBytes::free`] function pointer so
//! the host can return the bytes to the plugin's allocator after
//! decoding — no assumption about cross-DLL allocator compatibility.
//!
//! ## Threading
//!
//! Vtable method invocations MAY happen concurrently from
//! different host tasks. Plugin implementations MUST be
//! thread-safe (the same way every Adapter impl already is in
//! the in-process world). The host wraps every FFI call in
//! `tokio::task::spawn_blocking` so a blocking plugin can't stall
//! the runtime.

use std::os::raw::c_int;

/// Status codes returned in the `status` field of
/// [`PluginCallResult`]. Mapped one-to-one onto `cal_core::Error`
/// / `sync_core::SyncError` by the shim wrappers — so a plugin
/// that wants to surface "auth failed" just sets
/// `status = PLUGIN_CALL_ERR_AUTH` and the host's
/// `FfiCalendarAdapter` translates that back into
/// `Error::Authentication(<payload>)` for the rest of the app.
///
/// Values stay in sync with the C header — adding a new code
/// requires the header bump.
pub const PLUGIN_CALL_OK: c_int = 0;
/// Method isn't implemented on this adapter (cal_core's
/// `Error::Unsupported` / sync_core has no direct mapping; treat
/// as `Internal` with the payload's message).
pub const PLUGIN_CALL_ERR_UNSUPPORTED: c_int = 1;
/// Argument JSON couldn't be decoded by the plugin — usually a
/// programmer error in the host. Surfaces as
/// `cal_core::Error::InvalidInput` so a buggy host gets a clear
/// pointer at the offending call site.
pub const PLUGIN_CALL_ERR_INVALID: c_int = 2;
/// Authentication failed (`cal_core::Error::Authentication`,
/// `sync_core::SyncError::Auth`).
pub const PLUGIN_CALL_ERR_AUTH: c_int = 3;
/// Network-level failure (`cal_core::Error::Network`,
/// `sync_core::SyncError::Network`).
pub const PLUGIN_CALL_ERR_NETWORK: c_int = 4;
/// Resource not found (`cal_core::Error::NotFound`,
/// `sync_core::SyncError::NotFound`).
pub const PLUGIN_CALL_ERR_NOT_FOUND: c_int = 5;
/// Protocol / parse error (`cal_core::Error::Protocol`,
/// `sync_core::SyncError::Protocol`).
pub const PLUGIN_CALL_ERR_PROTOCOL: c_int = 6;
/// IO-level failure (`sync_core::SyncError::Io`; mapped to
/// `cal_core::Error::Internal` for calendar plugins).
pub const PLUGIN_CALL_ERR_IO: c_int = 7;
/// Conflict / precondition failed (`cal_core::Error::Conflict`).
pub const PLUGIN_CALL_ERR_CONFLICT: c_int = 8;
/// Access denied (`cal_core::Error::Forbidden`).
pub const PLUGIN_CALL_ERR_FORBIDDEN: c_int = 9;
/// Catch-all internal failure. Payload carries the message.
pub const PLUGIN_CALL_ERR_INTERNAL: c_int = 10;

/// Plugin-owned byte buffer crossing the FFI boundary.
///
/// `data` is allocated by the plugin's allocator. The host MUST
/// call `free(data, len)` once it's done with the bytes —
/// typically immediately after decoding the JSON payload. The
/// double-pointer pattern (data + free function pointer) avoids
/// any assumption that the host and plugin share an allocator.
///
/// A `PluginBytes` with `data == NULL` and `len == 0` is the
/// "no payload" sentinel — used for void-returning methods that
/// only need to signal status.
#[repr(C)]
#[derive(Debug)]
pub struct PluginBytes {
    pub data: *mut u8,
    pub len: usize,
    /// Releases `data` back to the plugin's allocator. MAY be
    /// `None` when `data == NULL` — i.e. a status-only response
    /// with no payload doesn't need a free function.
    pub free: Option<unsafe extern "C" fn(data: *mut u8, len: usize)>,
}

impl PluginBytes {
    /// Empty sentinel — no payload, no free function needed.
    pub const fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            free: None,
        }
    }

    /// True iff this is the empty sentinel.
    pub fn is_empty(&self) -> bool {
        self.data.is_null() || self.len == 0
    }

    /// Borrow the bytes as a slice WITHOUT taking ownership. The
    /// returned slice is valid until [`Self::free_in_place`] is
    /// called. Returns an empty slice for the [`Self::empty`]
    /// sentinel.
    ///
    /// # Safety
    ///
    /// The pointer must have been produced by a plugin's vtable
    /// call and not yet freed. In practice the shim wrappers
    /// always go `as_slice` → decode → free in a tight sequence
    /// inside one stack frame.
    pub unsafe fn as_slice(&self) -> &[u8] {
        if self.is_empty() {
            &[]
        } else {
            std::slice::from_raw_parts(self.data, self.len)
        }
    }

    /// Release the buffer via the plugin-supplied free function.
    /// Safe to call multiple times — the no-op branch handles
    /// already-released / empty buffers.
    ///
    /// # Safety
    ///
    /// Caller must not have given the bytes to another consumer
    /// that may try to read them after this call.
    pub unsafe fn free_in_place(&mut self) {
        if let (false, Some(free)) = (self.is_empty(), self.free) {
            free(self.data, self.len);
        }
        self.data = std::ptr::null_mut();
        self.len = 0;
        self.free = None;
    }
}

/// Standard return type for every vtable method.
///
/// `status` is one of the `PLUGIN_CALL_*` constants. On
/// [`PLUGIN_CALL_OK`] the payload is the JSON-encoded response
/// (an empty buffer for `()` returns). On any non-zero status the
/// payload is a UTF-8 error message; the shim wrappers wrap it
/// into the matching variant of the host-side error type.
#[repr(C)]
#[derive(Debug)]
pub struct PluginCallResult {
    pub status: c_int,
    pub payload: PluginBytes,
}

impl PluginCallResult {
    /// Convenience for the no-payload OK case.
    pub const fn ok_empty() -> Self {
        Self {
            status: PLUGIN_CALL_OK,
            payload: PluginBytes::empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bytes_have_null_data() {
        let b = PluginBytes::empty();
        assert!(b.is_empty());
        assert!(b.data.is_null());
        assert_eq!(b.len, 0);
        assert!(b.free.is_none());
    }

    #[test]
    fn empty_bytes_slice_is_empty() {
        let b = PluginBytes::empty();
        let s = unsafe { b.as_slice() };
        assert!(s.is_empty());
    }

    #[test]
    fn free_in_place_is_idempotent_on_empty() {
        let mut b = PluginBytes::empty();
        unsafe { b.free_in_place() };
        unsafe { b.free_in_place() };
        assert!(b.is_empty());
    }

    #[test]
    fn status_codes_are_distinct() {
        let codes = [
            PLUGIN_CALL_OK,
            PLUGIN_CALL_ERR_UNSUPPORTED,
            PLUGIN_CALL_ERR_INVALID,
            PLUGIN_CALL_ERR_AUTH,
            PLUGIN_CALL_ERR_NETWORK,
            PLUGIN_CALL_ERR_NOT_FOUND,
            PLUGIN_CALL_ERR_PROTOCOL,
            PLUGIN_CALL_ERR_IO,
            PLUGIN_CALL_ERR_CONFLICT,
            PLUGIN_CALL_ERR_FORBIDDEN,
            PLUGIN_CALL_ERR_INTERNAL,
        ];
        for (i, a) in codes.iter().enumerate() {
            for b in &codes[i + 1..] {
                assert_ne!(a, b, "duplicate status codes");
            }
        }
    }

    #[test]
    fn ok_empty_helper_matches_shape() {
        let r = PluginCallResult::ok_empty();
        assert_eq!(r.status, PLUGIN_CALL_OK);
        assert!(r.payload.is_empty());
    }
}
