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
    PLUGIN_CALL_ERR_INTERNAL, PLUGIN_CALL_ERR_INVALID, PLUGIN_CALL_ERR_IO, PLUGIN_CALL_ERR_NETWORK,
    PLUGIN_CALL_ERR_NOT_FOUND, PLUGIN_CALL_ERR_PROTOCOL, PLUGIN_CALL_ERR_UNSUPPORTED,
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
        // No plugin holds an OS permission today; if one ever does, it
        // crosses as a refusal the host already reads (the token in front),
        // so the plugin ABI needs no new status.
        CalError::AccessNotGranted(m) => (
            PLUGIN_CALL_ERR_FORBIDDEN,
            cal_core::WriteRefusal::AccessNotGranted.message(&m),
        ),
    };
    error_response(status, &msg)
}

/// A plugin's interactive-auth error text, from the core error its adapter
/// maps to. A message that starts with a refusal or a leftover token stays as
/// it is, token first, so the host and the shells still read it: a busy token
/// endpoint says the provider is busy, not "sign in again" (decision 270).
/// Anything else reads `prefix: error`.
pub fn auth_error(prefix: &str, err: &CalError) -> String {
    let message = err.message();
    if cal_core::WriteRefusal::parse(message).is_some()
        || cal_core::WriteLeftover::parse(message).is_some()
    {
        message.to_string()
    } else {
        format!("{prefix}: {err}")
    }
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
        // Not AUTH, which is what the transport uses for rejected credentials.
        // By the time decryption runs the backend has already accepted us; the
        // bytes are here and the passphrase does not open them.
        SyncError::DecryptionFailed(m) => {
            (PLUGIN_CALL_ERR_PROTOCOL, format!("decryption failed: {m}"))
        }
        SyncError::SchemaTooOld { required, running } => (
            PLUGIN_CALL_ERR_PROTOCOL,
            format!("schema too old: dataset needs {required}; running {running}"),
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
        let r = cal_error_to_response(CalError::Authentication("bad creds".to_string()));
        assert_eq!(r.status, PLUGIN_CALL_ERR_AUTH);
        let slice = unsafe { r.payload.as_slice() };
        assert_eq!(slice, b"bad creds");
        let mut p = r.payload;
        unsafe { p.free_in_place() };
    }

    /// A busy provider is a refusal and a save made in part a network error
    /// (decisions 270, 277): both cross with their status and their message
    /// byte for byte, token first.
    #[test]
    fn a_busy_refusal_crosses_as_forbidden_and_a_partly_saved_as_network() {
        use plugin_core::ffi::{PLUGIN_CALL_ERR_FORBIDDEN, PLUGIN_CALL_ERR_NETWORK};
        for (err, status, message) in [
            (
                CalError::provider_busy(cal_core::BusyDetail::TooManyRequests),
                PLUGIN_CALL_ERR_FORBIDDEN,
                "provider-busy: http-429",
            ),
            (
                CalError::partly_saved("members"),
                PLUGIN_CALL_ERR_NETWORK,
                "partly-saved: members",
            ),
        ] {
            let r = cal_error_to_response(err);
            assert_eq!(r.status, status);
            let slice = unsafe { r.payload.as_slice() };
            assert_eq!(slice, message.as_bytes());
            let mut p = r.payload;
            unsafe { p.free_in_place() };
        }
    }

    /// An interactive-auth error keeps a token first; anything else is
    /// named by its phase.
    #[test]
    fn auth_error_keeps_a_token_first() {
        let busy = CalError::provider_busy(cal_core::BusyDetail::RateLimit);
        assert_eq!(
            auth_error("Google exchange", &busy),
            "provider-busy: rate-limit"
        );
        let refused = CalError::Authentication("invalid_grant".into());
        assert_eq!(
            auth_error("Google exchange", &refused),
            "Google exchange: authentication failed: invalid_grant"
        );
    }

    /// A new series taken back is a refusal and one that may stand a network
    /// error (decisions 203, 210): both cross the plugin boundary with their
    /// status and their message byte for byte, token first, so the host reads
    /// the same outcome the adapter wrote.
    #[test]
    fn a_series_taken_back_or_left_standing_keeps_its_status_and_token() {
        use plugin_core::ffi::{PLUGIN_CALL_ERR_FORBIDDEN, PLUGIN_CALL_ERR_NETWORK};
        for (err, status, message) in [
            (
                CalError::Forbidden("deletions-not-kept: invited after-control".into()),
                PLUGIN_CALL_ERR_FORBIDDEN,
                "deletions-not-kept: invited after-control",
            ),
            (
                CalError::Network("created-may-stand: create-answer-lost".into()),
                PLUGIN_CALL_ERR_NETWORK,
                "created-may-stand: create-answer-lost",
            ),
        ] {
            let r = cal_error_to_response(err);
            assert_eq!(r.status, status);
            let slice = unsafe { r.payload.as_slice() };
            assert_eq!(slice, message.as_bytes());
            let mut p = r.payload;
            unsafe { p.free_in_place() };
        }
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
        let r = vc_error_to_response(VcError::Authentication("expired token".to_string()));
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
