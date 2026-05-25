//! Project plugin-side errors onto host-side status codes.
//!
//! When a trait method returns `cal_core::Error` or
//! `sync_core::SyncError`, the FFI fn turns it into a
//! [`PluginCallResult`] with the right `PLUGIN_CALL_ERR_*`
//! status + a UTF-8 message. The host's shim wrappers
//! ([`plugin_core::shim`]) reverse the mapping so the rest of
//! the host code sees its native error variant for every
//! variant the plugin emitted.
//!
//! Keeping this in one place (rather than per-trait-method)
//! means there's exactly one mapping table; if the
//! `PLUGIN_CALL_ERR_*` set grows the change lands here + on
//! the host side + nowhere else.

use cal_core::error::Error as CalError;
use plugin_core::ffi::{
    PLUGIN_CALL_ERR_AUTH, PLUGIN_CALL_ERR_CONFLICT, PLUGIN_CALL_ERR_FORBIDDEN,
    PLUGIN_CALL_ERR_INTERNAL, PLUGIN_CALL_ERR_INVALID, PLUGIN_CALL_ERR_IO,
    PLUGIN_CALL_ERR_NETWORK, PLUGIN_CALL_ERR_NOT_FOUND, PLUGIN_CALL_ERR_PROTOCOL,
    PLUGIN_CALL_ERR_UNSUPPORTED,
};
use plugin_core::PluginCallResult;
use sync_core::error::SyncError;
use vc_core::VcError;

use crate::response::error_response;

/// Map a `cal_core::Error` into a [`PluginCallResult`] with the
/// matching `PLUGIN_CALL_ERR_*` status. Used by every
/// CalendarFeature / TasksFeature / ContactsFeature FFI fn the
/// SDK generates.
pub fn cal_error_to_response(err: CalError) -> PluginCallResult {
    let (status, msg) = match err {
        CalError::Authentication(m) => (PLUGIN_CALL_ERR_AUTH, m),
        CalError::Forbidden(m) => (PLUGIN_CALL_ERR_FORBIDDEN, m),
        CalError::NotFound(m) => (PLUGIN_CALL_ERR_NOT_FOUND, m),
        CalError::Conflict(m) => (PLUGIN_CALL_ERR_CONFLICT, m),
        CalError::Network(m) => (PLUGIN_CALL_ERR_NETWORK, m),
        CalError::Protocol(m) => (PLUGIN_CALL_ERR_PROTOCOL, m),
        CalError::InvalidInput(m) => (PLUGIN_CALL_ERR_INVALID, m),
        CalError::Unsupported(m) => (PLUGIN_CALL_ERR_UNSUPPORTED, m),
        CalError::Internal(m) => (PLUGIN_CALL_ERR_INTERNAL, m),
    };
    error_response(status, &msg)
}

/// Map a `sync_core::SyncError` into a [`PluginCallResult`].
/// `EncryptionRequired`, `SchemaTooOld` and `StaleDevice` are
/// surfaced as Protocol with the formatted message — they're
/// orchestrator-side concerns the plugin can't trigger on its
/// own (the encrypting wrapper lives above the SyncAdapter
/// trait), but mapping them here means a forwarded error from
/// any unusual code path still survives the round-trip.
pub fn sync_error_to_response(err: SyncError) -> PluginCallResult {
    let (status, msg) = match err {
        SyncError::Auth(m) => (PLUGIN_CALL_ERR_AUTH, m),
        SyncError::Network(m) => (PLUGIN_CALL_ERR_NETWORK, m),
        SyncError::NotFound(m) => (PLUGIN_CALL_ERR_NOT_FOUND, m),
        SyncError::Protocol(m) => (PLUGIN_CALL_ERR_PROTOCOL, m),
        SyncError::Io(m) => (PLUGIN_CALL_ERR_IO, m),
        SyncError::Internal(m) => (PLUGIN_CALL_ERR_INTERNAL, m),
        SyncError::EncryptionRequired => (
            PLUGIN_CALL_ERR_PROTOCOL,
            "encryption required but no key is configured".to_string(),
        ),
        SyncError::SchemaTooOld { required, running } => (
            PLUGIN_CALL_ERR_PROTOCOL,
            format!(
                "schema too old: dataset needs {required}; running {running}"
            ),
        ),
        SyncError::StaleDevice { snapshot_at } => (
            PLUGIN_CALL_ERR_PROTOCOL,
            format!("stale device; snapshot at {snapshot_at}"),
        ),
    };
    error_response(status, &msg)
}

/// Map a `vc_core::VcError` into a [`PluginCallResult`]. One-to-
/// one mapping — the VC trait error enum was modelled on the
/// same variant set, so each maps cleanly to a status code.
pub fn vc_error_to_response(err: VcError) -> PluginCallResult {
    let (status, msg) = match err {
        VcError::Authentication(m) => (PLUGIN_CALL_ERR_AUTH, m),
        VcError::Forbidden(m) => (PLUGIN_CALL_ERR_FORBIDDEN, m),
        VcError::NotFound(m) => (PLUGIN_CALL_ERR_NOT_FOUND, m),
        VcError::Network(m) => (PLUGIN_CALL_ERR_NETWORK, m),
        VcError::Protocol(m) => (PLUGIN_CALL_ERR_PROTOCOL, m),
        VcError::InvalidInput(m) => (PLUGIN_CALL_ERR_INVALID, m),
        VcError::Unsupported(m) => (PLUGIN_CALL_ERR_UNSUPPORTED, m),
        VcError::Internal(m) => (PLUGIN_CALL_ERR_INTERNAL, m),
    };
    error_response(status, &msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_core::ffi::PLUGIN_CALL_ERR_AUTH;

    #[test]
    fn cal_authentication_maps_to_auth_status() {
        let r = cal_error_to_response(CalError::Authentication(
            "bad creds".to_string(),
        ));
        assert_eq!(r.status, PLUGIN_CALL_ERR_AUTH);
        let slice = unsafe { r.payload.as_slice() };
        assert_eq!(slice, b"bad creds");
        let mut p = r.payload;
        unsafe { p.free_in_place() };
    }

    #[test]
    fn cal_not_found_maps_to_not_found_status() {
        let r = cal_error_to_response(CalError::NotFound("x".into()));
        assert_eq!(r.status, PLUGIN_CALL_ERR_NOT_FOUND);
        let mut p = r.payload;
        unsafe { p.free_in_place() };
    }

    #[test]
    fn sync_auth_maps_to_auth_status() {
        let r = sync_error_to_response(SyncError::Auth("nope".into()));
        assert_eq!(r.status, PLUGIN_CALL_ERR_AUTH);
        let mut p = r.payload;
        unsafe { p.free_in_place() };
    }

    #[test]
    fn vc_authentication_maps_to_auth_status() {
        let r = vc_error_to_response(VcError::Authentication(
            "expired token".to_string(),
        ));
        assert_eq!(r.status, PLUGIN_CALL_ERR_AUTH);
        let mut p = r.payload;
        unsafe { p.free_in_place() };
    }

    #[test]
    fn vc_unsupported_maps_to_unsupported_status() {
        let r = vc_error_to_response(VcError::Unsupported(
            "no recording on free tier".to_string(),
        ));
        assert_eq!(r.status, PLUGIN_CALL_ERR_UNSUPPORTED);
        let mut p = r.payload;
        unsafe { p.free_in_place() };
    }

    #[test]
    fn sync_schema_too_old_maps_to_protocol_with_versions() {
        let r = sync_error_to_response(SyncError::SchemaTooOld {
            required: "2.0.0".into(),
            running: "1.0.0".into(),
        });
        assert_eq!(r.status, PLUGIN_CALL_ERR_PROTOCOL);
        let slice = unsafe { r.payload.as_slice() };
        let msg = std::str::from_utf8(slice).expect("utf8");
        assert!(msg.contains("2.0.0"));
        assert!(msg.contains("1.0.0"));
        let mut p = r.payload;
        unsafe { p.free_in_place() };
    }
}
