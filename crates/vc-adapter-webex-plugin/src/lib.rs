//! Cisco WebEx videoconference adapter packaged as a plugin
//! (DESIGN.md §11 + §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "client_id": "…",
//!   "client_secret": "…",
//!   "refresh_token": "…",
//!   "site_url": "example.webex.com",
//!   "use_personal_room": false,
//!   "send_webex_emails": false
//! }
//! ```
//!
//! The host merges the two CREDENTIALS — `client_secret` and `refresh_token` —
//! in from the keychain at open time; only the rest is persisted in the account
//! row. That split matters: the account row is appended to the sync event log
//! unencrypted whenever end-to-end encryption is off, so a secret living there
//! would travel to the user's own sync target in the clear.
//!
//! Unlike Teams (Microsoft Graph) and Meet (Google Calendar), Webex does not
//! piggy-back on any of Aperio's calendar adapters — it runs its own OAuth flow
//! against the Webex Meetings REST API, so the refresh token lives in a
//! Webex-specific keychain slot and the host runs a separate sign-in for each
//! Webex account.

use std::os::raw::{c_char, c_void};

use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::VcVtable;
use plugin_sdk::{decode_args, open_instance_with, PluginInstance};
use serde::Deserialize;
use vc_adapter_webex::{oauth, WebexAccountConfig, WebexAdapter};
use vc_core::{MeetingId, NewMeeting, VcAdapter};

plugin_sdk::vc_dispatch_helpers!(WebexAdapter);

/// Bridges the adapter's credential reports onto the plugin host channel.
///
/// The adapter is a plain library and knows nothing about FFI; this wrapper is
/// where the two meet.
struct HostChannelSink;

impl vc_adapter_webex::api::CredentialSink for HostChannelSink {
    fn credential_rotated(&self, scope: &str, slot: &str, value: &str, expires_at: Option<&str>) {
        let status =
            plugin_sdk::host_channel::report_credential_rotated(scope, slot, value, expires_at);
        // Never log the value. The status alone says whether the host took
        // durable responsibility, which is the only thing worth knowing here.
        if status != plugin_sdk::plugin_core::abi::HOST_CHANNEL_ACCEPTED {
            tracing::warn!(
                slot,
                status,
                "the host did not accept a rotated Webex credential; it stays in memory                  until this instance closes"
            );
        }
    }
}

#[derive(Debug, Deserialize)]
struct InitConfig {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    /// The capability token the host splices in so a report can name this
    /// account. Absent on a host that predates the channel.
    #[serde(default, rename = "__aperio_host_token")]
    host_token: Option<String>,
    #[serde(default)]
    site_url: Option<String>,
    #[serde(default)]
    use_personal_room: bool,
    #[serde(default)]
    send_webex_emails: bool,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.client_id.trim().is_empty()
            || cfg.client_secret.trim().is_empty()
            || cfg.refresh_token.trim().is_empty()
        {
            return Err("client_id, client_secret and refresh_token must not be empty".to_string());
        }
        Ok(WebexAdapter::new(
            WebexAccountConfig {
                client_id: cfg.client_id,
                site_url: cfg.site_url.filter(|s| !s.trim().is_empty()),
                use_personal_room: cfg.use_personal_room,
                send_webex_emails: cfg.send_webex_emails,
            },
            cfg.client_secret,
            cfg.refresh_token,
        )
        .with_credential_sink(cfg.host_token, Box::new(HostChannelSink)))
    })
}

/// # Safety
/// FFI export.
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<WebexAdapter>::drop_handle(handle);
}

unsafe extern "C" fn ffi_test_connection(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch_unit(h, |p| async move { p.test_connection().await })
}

unsafe extern "C" fn ffi_create_meeting(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let spec: NewMeeting = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.create_meeting(spec).await })
}

unsafe extern "C" fn ffi_get_meeting(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let id: MeetingId = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.get_meeting(&id).await })
}

unsafe extern "C" fn ffi_delete_meeting(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let id: MeetingId = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move { p.delete_meeting(&id).await })
}

pub static VC_VTABLE: VcVtable = VcVtable {
    test_connection: Some(ffi_test_connection),
    create_meeting: Some(ffi_create_meeting),
    get_meeting: Some(ffi_get_meeting),
    delete_meeting: Some(ffi_delete_meeting),
    ..VcVtable::empty()
};

// ---------------------------------------------------------------------------
// Interactive sign-in
// ---------------------------------------------------------------------------

/// Arguments for `aperio_plugin_interactive_auth`, matching the shape the
/// Google and Microsoft plugins already use so the host's OAuth plumbing does
/// not need a Webex-specific branch.
///
/// Three phases:
///
///  - absent / `"full"` — the desktop loopback dance: bind 127.0.0.1:8080, open
///    the browser, capture the redirect, exchange. One call, blocking until the
///    user is done.
///  - `"authorize"` — mobile: return the consent URL plus the PKCE verifier and
///    the CSRF state; the host opens it in a native auth session.
///  - `"exchange"` — mobile: swap the returned code for tokens.
///
/// The adapter keeps no state between the two mobile phases, so the host holds
/// the verifier and the state and replays them.
#[derive(Debug, Deserialize)]
struct InteractiveAuthArgs {
    client_id: String,
    /// Webex requires a client secret at the token endpoint even under PKCE —
    /// measured against the live endpoint, not assumed. Absent only in the
    /// `authorize` phase, which does no I/O.
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    phase: Option<String>,
    /// `authorize` + `exchange`: the redirect the host will actually receive
    /// on. Must be byte-identical across the two phases and registered on the
    /// integration, or Webex rejects the exchange with an error that names
    /// neither value.
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    pkce_verifier: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    returned_state: Option<String>,
}

/// The OAuth CSRF check: the state that came back on the redirect must equal
/// the one issued at authorize.
///
/// Fails CLOSED — an absent or empty value on either side is a rejection, not a
/// skip. This guards token minting and account creation, which is precisely
/// where a permissive check would be worth attacking.
fn verify_oauth_state(issued: Option<&str>, returned: Option<&str>) -> Result<(), String> {
    let issued = issued.unwrap_or_default().trim();
    let returned = returned.unwrap_or_default().trim();
    if issued.is_empty() || returned.is_empty() || issued != returned {
        return Err("OAuth state mismatch (possible CSRF) — aborting".to_string());
    }
    Ok(())
}

/// The client secret, rejected when blank.
///
/// Its own function because the two phases that need it fail for the same
/// reason and should say so identically — and because "present but whitespace"
/// is the shape a half-filled form produces, which would otherwise reach Webex
/// and come back as an opaque `invalid_client`.
fn require_secret(secret: Option<&str>, phase: &str) -> Result<String, String> {
    let secret = secret.unwrap_or_default().trim();
    if secret.is_empty() {
        return Err(format!(
            "client_secret is required in the {phase} phase — Webex issues no token without one, \
             even under PKCE"
        ));
    }
    Ok(secret.to_string())
}

async fn plugin_interactive_auth(args_json: String) -> Result<Vec<u8>, String> {
    let args: InteractiveAuthArgs = serde_json::from_str(&args_json)
        .map_err(|e| format!("malformed interactive_auth args: {e}"))?;
    let client_id = args.client_id.trim().to_string();
    if client_id.is_empty() {
        return Err("client_id must not be empty".to_string());
    }
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("could not build an HTTP client: {e}"))?;

    match args.phase.as_deref() {
        Some("authorize") => {
            let redirect_uri = args
                .redirect_uri
                .ok_or_else(|| "redirect_uri is required in the authorize phase".to_string())?;
            let authz = oauth::authorize(&client_id, &redirect_uri, oauth::WEBEX_AUTH_URL)
                .map_err(|e| format!("Webex authorize: {e}"))?;
            serde_json::to_vec(&authz).map_err(|e| format!("serialise authorize response: {e}"))
        }
        Some("exchange") => {
            let secret = require_secret(args.client_secret.as_deref(), "exchange")?;
            let code = args
                .code
                .ok_or_else(|| "code is required in the exchange phase".to_string())?;
            let verifier = args
                .pkce_verifier
                .ok_or_else(|| "pkce_verifier is required in the exchange phase".to_string())?;
            let redirect_uri = args
                .redirect_uri
                .ok_or_else(|| "redirect_uri is required in the exchange phase".to_string())?;
            verify_oauth_state(args.state.as_deref(), args.returned_state.as_deref())?;
            let tokens = oauth::exchange_code(
                &http,
                oauth::WEBEX_TOKEN_URL,
                &client_id,
                Some(secret.as_str()),
                code.trim(),
                verifier.trim(),
                &redirect_uri,
            )
            .await
            .map_err(|e| format!("Webex exchange: {e}"))?;
            serde_json::to_vec(&tokens).map_err(|e| format!("serialise TokenSet: {e}"))
        }
        None | Some("full") => {
            let secret = require_secret(args.client_secret.as_deref(), "full")?;
            let tokens = oauth::run_loopback(
                &client_id,
                Some(secret.as_str()),
                oauth::WEBEX_AUTH_URL,
                oauth::WEBEX_TOKEN_URL,
                &http,
            )
            .await
            .map_err(|e| format!("Webex OAuth: {e}"))?;
            serde_json::to_vec(&tokens).map_err(|e| format!("serialise TokenSet: {e}"))
        }
        Some(other) => Err(format!("unknown interactive_auth phase: {other}")),
    }
}

plugin_sdk::declare_interactive_auth! {
    handler: plugin_interactive_auth,
}

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.vc-adapter-webex",
    name: "Aperio Cisco WebEx",
    version: "0.1.0",
    plugin_type: "videoconference-adapter",
    vtable: VC_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
