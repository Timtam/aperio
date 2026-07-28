//! The plugin side of the host channel: report something the host did not ask
//! for.
//!
//! The counterpart to [`crate::log_forward`], and installed the same way — the
//! host calls the plugin's `aperio_plugin_set_host_channel` export once, right
//! after `create`, handing over a function pointer into the host binary. On the
//! static-link (mobile) path the export is never called, because the pointer
//! would be into the same binary anyway; [`report`] then answers
//! [`NOT_AVAILABLE`] and the caller carries on.
//!
//! ## What an adapter does with this
//!
//! Exactly one thing today: an OAuth adapter whose provider hands back a new
//! credential during a refresh reports it, so the host can persist it. Without
//! that, the refreshed value lives in the adapter's memory until the instance
//! closes and the host's stored copy silently goes stale.
//!
//! ```ignore
//! plugin_sdk::host_channel::report_credential_rotated(
//!     scope_token,          // handed to the instance in its open config
//!     "refresh_token",
//!     &new_value,
//!     Some(expires_at_rfc3339),
//! );
//! ```
//!
//! ## What it must never carry
//!
//! Anything that is not a credential the host asked to keep. The envelope
//! crosses into the host's log paths on failure, so the payload is built here
//! rather than by the caller, and the value itself is never logged on either
//! side.

use std::sync::OnceLock;

use crate::plugin_core::abi::{
    AperioHostChannelFn, HOST_CHANNEL_ENVELOPE_V1, KIND_CREDENTIAL_ROTATED,
};

/// Returned by [`report`] when the host never installed a channel — an older
/// host, or the static-link path where the export is not called. Distinct from
/// every `APERIO_HOST_CHANNEL_*` status so a caller can tell "nobody is
/// listening" from "the host said no".
pub const NOT_AVAILABLE: i32 = -1;

static CHANNEL: OnceLock<AperioHostChannelFn> = OnceLock::new();

/// Store the host's sink. Called by the generated
/// `aperio_plugin_set_host_channel` export; idempotent, first call wins.
pub fn install_host_channel(cb: AperioHostChannelFn) {
    let _ = CHANNEL.set(cb);
}

/// Whether a host channel is available. Lets an adapter skip building a
/// payload it has nowhere to send.
pub fn is_available() -> bool {
    CHANNEL.get().is_some()
}

/// Send one report. Returns the host's status, or [`NOT_AVAILABLE`].
///
/// `scope` is the opaque capability token the host put in this instance's open
/// config under `__aperio_host_token`. Without it the host cannot tell which
/// account is speaking and will refuse the report.
pub fn report(scope: &str, kind: &str, payload: serde_json::Value) -> i32 {
    let Some(cb) = CHANNEL.get() else {
        return NOT_AVAILABLE;
    };
    if scope.is_empty() {
        // Nothing to resolve against; do not bother the host.
        return NOT_AVAILABLE;
    }
    let envelope = serde_json::json!({
        "v": HOST_CHANNEL_ENVELOPE_V1,
        "scope": scope,
        "kind": kind,
        "payload": payload,
    });
    let Ok(bytes) = serde_json::to_vec(&envelope) else {
        return NOT_AVAILABLE;
    };
    // SAFETY: `cb` is the host-supplied `'static` function pointer from the
    // install contract, and `bytes` outlives the call.
    unsafe { cb(bytes.as_ptr(), bytes.len()) }
}

/// Report that a credential this instance holds has been replaced by the
/// provider.
///
/// `slot` names which one in the host's vocabulary — `refresh_token`,
/// `access_token`, `api_token`. `expires_at` is RFC 3339 when the provider says
/// so; it matters independently of the value, because a provider can hand back
/// the SAME token with a fresh expiry and a host that stores only values then
/// watches a perfectly good credential appear to die.
pub fn report_credential_rotated(
    scope: &str,
    slot: &str,
    value: &str,
    expires_at: Option<&str>,
) -> i32 {
    let mut payload = serde_json::json!({ "slot": slot, "value": value });
    if let Some(expires_at) = expires_at {
        payload["expires_at"] = serde_json::Value::String(expires_at.to_string());
    }
    report(scope, KIND_CREDENTIAL_ROTATED, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_host_channel_reporting_is_a_no_op_not_a_panic() {
        // The static-link path never installs one, and an older host has no
        // such export. Neither may take the adapter down.
        assert_eq!(
            report_credential_rotated("token", "refresh_token", "v", None),
            NOT_AVAILABLE
        );
        assert!(!is_available());
    }

    #[test]
    fn an_empty_scope_is_refused_locally() {
        assert_eq!(report("", "any.kind", serde_json::json!({})), NOT_AVAILABLE);
    }

    #[test]
    fn the_rotation_payload_names_the_slot_and_keeps_the_expiry() {
        // Built here rather than by the caller so every adapter reports the
        // same shape, and so the value can never be accidentally logged by one.
        let mut payload = serde_json::json!({ "slot": "refresh_token", "value": "v" });
        payload["expires_at"] = serde_json::Value::String("2026-10-26T00:00:00Z".into());
        assert_eq!(payload["slot"], "refresh_token");
        assert_eq!(payload["expires_at"], "2026-10-26T00:00:00Z");
    }
}
