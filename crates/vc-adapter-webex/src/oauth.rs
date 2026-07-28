//! OAuth 2.0 for Cisco Webex.
//!
//! Modelled on `cal_adapter_google::auth`, with four differences that come
//! straight from Webex's own behaviour and are the reason this is not a copy:
//!
//! **The redirect URI must match EXACTLY, so the loopback port is fixed.**
//! Google accepts any `127.0.0.1` port (RFC 8252 §7.3), which lets its adapter
//! bind port 0 and use whatever the OS hands out. Webex compares the
//! `redirect_uri` against the registered list verbatim, path included — the URL
//! its own portal generates is `http://127.0.0.1:8080/oauth/webex`. So the
//! listener binds 8080, and "port already in use" is a real, reportable
//! condition rather than something the OS papers over. There is deliberately no
//! fallback to the `localhost` form, even though it is registered too — see
//! [`bind_loopback`] for why a fallback there would be worse than failing.
//!
//! **The refresh token's CLOCK restarts on every use.** Measured against a live
//! account on 2026-07-28: the access token lasts 14 days, the refresh token 90,
//! and a refresh moved the refresh expiry 90 days out again while returning the
//! SAME token value. So the design assumption going in — that the value
//! rotates — did not hold in that observation, but the consequence is
//! unchanged and is the reason this is called out: **the caller must persist
//! whatever comes back, value and expiry both.** Storing only the value would
//! let an account look dead 90 days after first sign-in while the credential is
//! still perfectly good; ignoring a value that HAS changed would throw away the
//! only one that works. One observation is not a guarantee that Webex never
//! rotates, so the code treats a returned token as authoritative either way.
//!
//! **A client secret is required even under PKCE — verified, not assumed.** The
//! `webex-auth` example ran the identical flow twice against a live account on
//! 2026-07-28: without the secret Webex answered `client_secret cannot be null
//! or empty`, with it the exchange succeeded. There is no public-client mode.
//! The secret stays optional in these signatures anyway, because the failure it
//! produces is worth naming precisely and because a future Webex may change its
//! mind; [`exchange_code`] reports which posture was refused.
//!
//! **`spark:kms` is not needed.** Webex's portal shows it in the convenience
//! authorization URL it generates, which is where the assumption that it is
//! mandatory came from. It is not: the live run requested exactly the three
//! meeting scopes below and was granted exactly those three, and both the
//! exchange and the refresh worked. Should a Meetings API call ever come back
//! demanding it, add it here — but do not add it on spec, and never add
//! `spark:all`, which grants far more than a calendar app has any business
//! holding and is an automatic App Hub rejection.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, warn};
use vc_core::{VcError, VcResult};

pub const WEBEX_AUTH_URL: &str = "https://webexapis.com/v1/authorize";
pub const WEBEX_TOKEN_URL: &str = "https://webexapis.com/v1/access_token";

/// Create, change and delete meetings, plus their invitees.
pub const SCOPE_SCHEDULES_WRITE: &str = "meeting:schedules_write";
/// Read meetings and their join details.
pub const SCOPE_SCHEDULES_READ: &str = "meeting:schedules_read";
/// Read the user's Webex sites and Personal Meeting Room. Also the cheapest
/// credential probe: it answers "does this account have a Webex site at all",
/// which is the thing most likely to be wrong.
pub const SCOPE_PREFERENCES_READ: &str = "meeting:preferences_read";

/// What Aperio asks for, space-separated as Webex expects. Webex adds
/// `spark:kms` itself; requesting it explicitly changes nothing, so it is left
/// out to keep the consent screen honest about what Aperio wants.
pub const SCOPES: &str = "meeting:schedules_write meeting:schedules_read meeting:preferences_read";

/// The loopback port Aperio's Webex integration is registered for. Fixed, not
/// ephemeral — see the module docs.
pub const REDIRECT_PORT: u16 = 8080;
/// The registered path. Webex matches the whole URI, so this is not decoration.
pub const REDIRECT_PATH: &str = "/oauth/webex";

/// The desktop redirect URI, and the value that must be sent unchanged at both
/// the authorize and the exchange step.
pub fn loopback_redirect_uri() -> String {
    format!("http://127.0.0.1:{REDIRECT_PORT}{REDIRECT_PATH}")
}

/// The mobile redirect. Webex accepted this custom scheme at registration, so
/// the mobile flow can use a native auth session exactly as the Google adapter
/// does, rather than needing a loopback server on a phone.
pub const MOBILE_REDIRECT_URI: &str = "aperio://oauth-callback";

/// Ceiling on the consent dance. Webex expires unused authorization codes on a
/// similar scale, and waiting longer means holding port 8080 hostage.
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
/// How long ONE connection may take to say what it wants. Short, because the
/// only connection that matters is a browser redirect that arrives complete.
/// Clamped by [`AUTH_TIMEOUT`], so a stalled probe costs seconds out of the
/// overall budget rather than the whole of it.
const PER_CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
/// Byte ceiling on one request's line plus headers.
const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// Tokens from Webex's `/access_token` endpoint.
///
/// Both fields must be persisted after every refresh. The observed behaviour is
/// that the refresh token's VALUE stays the same while its 90-day expiry moves
/// forward, so a host that stores only the value watches a perfectly good
/// credential appear to die — and one that assumes the value never changes
/// would be wrong the first time Webex decides otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// When the ACCESS token stops working (~14 days out).
    pub expires_at: DateTime<Utc>,
    /// When the REFRESH token stops working (~90 days out, reset on every use).
    /// Tracked so the host can tell "needs a refresh" apart from "needs the
    /// user to sign in again", which are very different messages.
    pub refresh_expires_at: Option<DateTime<Utc>>,
    pub scope: Option<String>,
}

impl TokenSet {
    /// True when the access token is spent (or close enough that a request
    /// started now would likely land after it lapsed).
    pub fn access_expired(&self, now: DateTime<Utc>, skew: Duration) -> bool {
        self.expires_at
            <= now + chrono::Duration::from_std(skew).unwrap_or_else(|_| chrono::Duration::zero())
    }
}

/// The pure **authorize** phase: build the consent URL plus the PKCE verifier
/// and CSRF state. No I/O — the caller opens the URL and captures the redirect,
/// via [`run_loopback`] on the desktop or a native auth session on mobile, then
/// replays `code` + `state` + `pkce_verifier` into [`exchange_code`].
///
/// `redirect_uri` must be one of the values registered on the integration and
/// must be byte-identical at exchange time.
pub fn authorize(client_id: &str, redirect_uri: &str, auth_url: &str) -> VcResult<Authorization> {
    if client_id.trim().is_empty() {
        return Err(VcError::InvalidInput("client_id must not be empty".into()));
    }
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();
    let mut url = url::Url::parse(auth_url)
        .map_err(|e| VcError::InvalidInput(format!("invalid auth_url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPES)
        .append_pair("state", &state)
        // PKCE even though Webex also wants the secret: it costs one hash and
        // it stops a stolen authorization code from being redeemed elsewhere.
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(Authorization {
        authorize_url: url.to_string(),
        pkce_verifier: verifier,
        state,
        redirect_uri: redirect_uri.to_string(),
    })
}

/// Output of [`authorize`]. The adapter keeps no cross-phase state, so the
/// caller holds these opaquely between the two steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Authorization {
    pub authorize_url: String,
    pub pkce_verifier: String,
    pub state: String,
    /// Echoed back so the caller cannot accidentally exchange against a
    /// different redirect than it authorized with — Webex rejects that, and the
    /// resulting error names neither value.
    pub redirect_uri: String,
}

/// The full desktop dance: bind the loopback listener, open the browser, wait
/// for the redirect, exchange the code.
///
/// Binding comes FIRST, before the browser opens: if port 8080 is taken there
/// is no point sending the user to a consent screen whose redirect will land
/// nowhere.
pub async fn run_loopback(
    client_id: &str,
    client_secret: Option<&str>,
    auth_url: &str,
    token_url: &str,
    http: &reqwest::Client,
) -> VcResult<TokenSet> {
    let (listener, authz) = begin_loopback(client_id, auth_url).await?;
    open_consent_screen(&authz);
    let code = capture_loopback_code(listener, &authz).await?;
    exchange_code(
        http,
        token_url,
        client_id,
        client_secret,
        &code,
        &authz.pkce_verifier,
        &authz.redirect_uri,
    )
    .await
}

/// Bind the loopback listener and build the consent URL that matches it.
///
/// Binding comes FIRST on purpose: if port 8080 is taken there is no point
/// sending the user to a consent screen whose redirect will land nowhere. Split
/// out from [`run_loopback`] so a caller can drive the phases itself — the
/// `webex-auth` example does, to try two token postures against one flow.
pub async fn begin_loopback(
    client_id: &str,
    auth_url: &str,
) -> VcResult<(TcpListener, Authorization)> {
    let (listener, redirect_uri) = bind_loopback().await?;
    let authz = authorize(client_id, &redirect_uri, auth_url)?;
    Ok((listener, authz))
}

/// Open the user's browser at the consent screen. Best effort: on a machine
/// with no default browser the caller has already printed the URL, so a failure
/// is logged rather than fatal.
pub fn open_consent_screen(authz: &Authorization) {
    debug!(url = %authz.authorize_url, "opening the Webex consent screen");
    if let Err(e) = open::that(authz.authorize_url.as_str()) {
        warn!(
            ?e,
            "could not launch a browser; the URL must be opened by hand"
        );
    }
}

/// Wait for the redirect and return the authorization code, having verified
/// that the `state` is the one we issued.
pub async fn capture_loopback_code(
    listener: TcpListener,
    authz: &Authorization,
) -> VcResult<String> {
    let (code, returned_state) = wait_for_redirect(listener).await?;
    if returned_state != authz.state {
        return Err(VcError::Authentication(
            "the redirect carried a different state than we issued; \
             the sign-in was not the one this app started"
                .into(),
        ));
    }
    Ok(code)
}

/// Bind the registered loopback address, returning the listener and the exact
/// redirect URI that goes with it.
///
/// **Only `127.0.0.1`.** `http://localhost:8080/oauth/webex` is registered too,
/// but falling back to it would be worse than failing: `localhost` is a NAME,
/// and the only situation in which the IPv4 bind fails is one where something
/// else already answers on 127.0.0.1:8080. Binding `::1` under the name
/// `localhost` would then succeed while the browser — which resolves the same
/// name by its own rules, IPv4-first on most systems — hands the authorization
/// code to that other process instead. A confusing success is worse than an
/// honest failure, so the fallback is deliberately absent.
///
/// The address is built as an explicit `SocketAddr` rather than a `(&str, u16)`
/// tuple so no name resolution happens at all.
async fn bind_loopback() -> VcResult<(TcpListener, String)> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, REDIRECT_PORT));
    match TcpListener::bind(addr).await {
        Ok(listener) => Ok((listener, loopback_redirect_uri())),
        Err(e) => {
            debug!(%addr, ?e, "loopback bind failed");
            Err(VcError::Network(format!(
                "Port {REDIRECT_PORT} on 127.0.0.1 is not available ({e}). Webex only \
                 accepts the exact redirect address its integration is registered for, so \
                 this port cannot be swapped for a free one — close whatever is listening \
                 on it and start the sign-in again."
            )))
        }
    }
}

/// Exchange an authorization code for tokens.
///
/// `client_secret` is optional so the public-client question can be settled
/// empirically (see the module docs and the `webex-auth` example). When the
/// server refuses, the error says whether a secret was sent, because "invalid
/// client" with and without one mean completely different things.
pub async fn exchange_code(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> VcResult<TokenSet> {
    let form = code_exchange_form(client_id, client_secret, code, verifier, redirect_uri);
    post_token_form(http, token_url, &form, client_secret.is_some()).await
}

/// The exact form fields of a code exchange. Split out so a test can assert
/// that `client_secret` is ABSENT in the public-client posture — asserting a
/// missing field through a serialised HTTP body needs a negative lookahead,
/// which the regex engine behind the mock server does not support.
fn code_exchange_form<'a>(
    client_id: &'a str,
    client_secret: Option<&'a str>,
    code: &'a str,
    verifier: &'a str,
    redirect_uri: &'a str,
) -> Vec<(&'a str, &'a str)> {
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    form
}

/// Trade the refresh token for a fresh access token.
///
/// The returned [`TokenSet`] must be persisted whole — see the type's
/// documentation. The refresh token usually comes back unchanged in value with
/// a fresh 90-day expiry; when the response omits it entirely, the previous one
/// is still valid and the caller keeps it.
pub async fn refresh(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> VcResult<TokenSet> {
    let form = refresh_form(client_id, client_secret, refresh_token);
    post_token_form(http, token_url, &form, client_secret.is_some()).await
}

/// The exact form fields of a refresh. See [`code_exchange_form`].
fn refresh_form<'a>(
    client_id: &'a str,
    client_secret: Option<&'a str>,
    refresh_token: &'a str,
) -> Vec<(&'a str, &'a str)> {
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    form
}

async fn post_token_form(
    http: &reqwest::Client,
    token_url: &str,
    form: &[(&str, &str)],
    sent_secret: bool,
) -> VcResult<TokenSet> {
    let now = Utc::now();
    let body = serde_urlencoded::to_string(form)
        .map_err(|e| VcError::Protocol(format!("encode token request: {e}")))?;
    let response = http
        .post(token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| VcError::Network(e.to_string()))?;
    parse_token_response(response, now, sent_secret).await
}

/// Webex's token response. `expires_in` and `refresh_token_expires_in` are
/// seconds.
#[derive(Debug, Deserialize)]
struct RawTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    refresh_token_expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

async fn parse_token_response(
    response: reqwest::Response,
    now: DateTime<Utc>,
    sent_secret: bool,
) -> VcResult<TokenSet> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| VcError::Network(e.to_string()))?;

    if !status.is_success() {
        // Naming whether a secret was sent turns two indistinguishable
        // failures into two different diagnoses — and it is exactly the
        // question the public-client experiment asks.
        let posture = if sent_secret {
            "with a client secret"
        } else {
            "as a public client, with PKCE and no client secret"
        };
        let detail = summarise_error(&text);
        return Err(match status.as_u16() {
            400 | 401 => VcError::Authentication(format!(
                "Webex refused the token request ({posture}): {detail}"
            )),
            403 => VcError::Forbidden(format!("Webex refused the token request: {detail}")),
            429 => VcError::Network(format!(
                "Webex is rate-limiting the token endpoint: {detail}"
            )),
            // The server having a bad day, not a response we failed to
            // understand. `Network` is what the UI treats as transient and
            // offers a retry for; `Protocol` reads as "this adapter is broken"
            // and is not retried.
            408 | 500..=599 => VcError::Network(format!(
                "the Webex token endpoint is unavailable ({status}): {detail}"
            )),
            // Anything left — a 404, a 415, an unfollowed redirect — means the
            // endpoint is not the one this adapter thinks it is talking to.
            other => VcError::Protocol(format!("Webex token endpoint returned {other}: {detail}")),
        });
    }

    let raw: RawTokenResponse = serde_json::from_str(&text)
        .map_err(|e| VcError::Protocol(format!("token response was not the expected JSON: {e}")))?;
    if raw.access_token.trim().is_empty() {
        return Err(VcError::Protocol(
            "Webex returned an empty access token".into(),
        ));
    }
    Ok(TokenSet {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token.filter(|t| !t.trim().is_empty()),
        // A missing `expires_in` is treated as already expired rather than as
        // forever: the next call then refreshes, which is recoverable, whereas
        // assuming a long life strands the account until the user notices.
        // `chrono::Duration::seconds` PANICS outside roughly ±i64::MAX/1000, and
        // this crate compiles into a dlopen'd plugin whose release profile
        // aborts on panic. A server answering with a nonsense lifetime must not
        // take the app down, so both clocks go through the fallible
        // constructor: a garbage value reads as "expired", which costs one
        // refresh instead of a process.
        expires_at: now + checked_seconds(raw.expires_in.unwrap_or(0)),
        refresh_expires_at: raw
            .refresh_token_expires_in
            .map(|s| now + checked_seconds(s)),
        scope: raw.scope,
    })
}

/// Seconds as a `chrono::Duration`, or zero when the value is absurd. Negative
/// and out-of-range lifetimes both collapse to "already expired".
fn checked_seconds(secs: i64) -> chrono::Duration {
    if secs <= 0 {
        return chrono::Duration::zero();
    }
    chrono::Duration::try_seconds(secs).unwrap_or_else(chrono::Duration::zero)
}

/// Pull something human out of an error body without ever echoing a token.
/// Webex answers with `{"message":…,"errors":[{"description":…}],"trackingId":…}`
/// on most failures and with an OAuth `{"error":…,"error_description":…}` on
/// others. The tracking id is kept: it is the only handle Cisco support takes.
fn summarise_error(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        let trimmed = body.trim();
        return if trimmed.is_empty() {
            "no response body".to_string()
        } else {
            trimmed.chars().take(300).collect()
        };
    };
    let code = json.get("error").and_then(|v| v.as_str());
    let message = json
        .get("error_description")
        .and_then(|v| v.as_str())
        .or_else(|| json.get("message").and_then(|v| v.as_str()))
        .or_else(|| {
            json.get("errors")
                .and_then(|e| e.get(0))
                .and_then(|e| e.get("description"))
                .and_then(|v| v.as_str())
        });
    // Keep BOTH. The OAuth `error` code is the machine-readable half and the
    // one that separates `invalid_client` (wrong credentials) from
    // `invalid_grant` (spent or mismatched code) — the exact question anyone
    // debugging this asks. The description is the prose written to explain it.
    let head = match (code, message) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        (Some(c), None) => c.to_string(),
        (None, Some(m)) => m.to_string(),
        (None, None) => "no message".to_string(),
    };
    match json.get("trackingId").and_then(|v| v.as_str()) {
        Some(id) => format!("{head} (Webex tracking id {id})"),
        None => head,
    }
}

// ── PKCE + state ─────────────────────────────────────────────────────────

/// RFC 7636 §4.1 + §4.2: a 43-character verifier and its S256 challenge.
fn generate_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// ── Loopback listener ────────────────────────────────────────────────────

/// Wait for the redirect, returning its `code` and `state`.
///
/// The listener has to survive company on its port. Anything that is not the
/// registered path — a browser preload, a favicon fetch, a security agent's
/// probe — is answered with 404 and the loop re-arms, because there is exactly
/// one connection that matters and it must not be spent on noise.
///
/// Three rules make that robust rather than merely intended:
///
/// * **Every read is inside a deadline.** A peer that completes the handshake
///   and then says nothing would otherwise park the flow in `read_line` with no
///   timeout at all — the overall ceiling only ever wrapped `accept`. Each
///   connection gets a short budget of its own, clamped by the absolute
///   ceiling, so a silent probe costs a few seconds and the real callback still
///   lands inside the window.
/// * **A broken connection is noise, not a failure.** A reset, a TLS handshake
///   sent to an HTTP port, a request line that is not UTF-8 — none of these
///   come from Webex, so none of them may end the sign-in the user is in the
///   middle of. They are logged and the loop re-arms.
/// * **The headers are bounded.** A peer that streams headers forever is cut
///   off by both the byte budget and the per-connection deadline.
async fn wait_for_redirect(listener: TcpListener) -> VcResult<(String, String)> {
    let deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;
    loop {
        let accept = tokio::time::timeout_at(deadline, listener.accept()).await;
        let (socket, peer) = match accept {
            Err(_) => {
                return Err(VcError::Authentication(
                    "The Webex sign-in was not completed in time.".into(),
                ))
            }
            // The listener itself failing is terminal — there is nothing left
            // to accept on.
            Ok(Err(e)) => return Err(VcError::Network(e.to_string())),
            Ok(Ok(pair)) => pair,
        };

        let conn_deadline = std::cmp::min(
            deadline,
            tokio::time::Instant::now() + PER_CONNECTION_TIMEOUT,
        );
        let mut reader = BufReader::new(socket);
        let request_line =
            match tokio::time::timeout_at(conn_deadline, read_request(&mut reader)).await {
                // Silent peer: drop it and keep waiting for the real callback.
                Err(_) => {
                    debug!(%peer, "a connection sent nothing in time; ignoring it");
                    continue;
                }
                // Reset, TLS to an HTTP port, non-UTF-8 — not Webex, so not fatal.
                Ok(Err(e)) => {
                    debug!(%peer, ?e, "a connection could not be read; ignoring it");
                    continue;
                }
                Ok(Ok(line)) => line,
            };

        let target = request_line.split_whitespace().nth(1).unwrap_or("/");
        // A request line we cannot parse is more noise, not a protocol error on
        // Webex's part.
        let Ok(pseudo) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
            debug!(%peer, "a request target could not be parsed; ignoring it");
            respond(reader, 404, NOT_FOUND_BODY).await;
            continue;
        };

        if pseudo.path() != REDIRECT_PATH {
            debug!(path = pseudo.path(), "ignoring a request to another path");
            respond(reader, 404, NOT_FOUND_BODY).await;
            continue;
        }

        let mut code = None;
        let mut state = None;
        let mut err_param = None;
        let mut err_description = None;
        for (k, v) in pseudo.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                "error" => err_param = Some(v.into_owned()),
                "error_description" => err_description = Some(v.into_owned()),
                _ => {}
            }
        }

        // The browser is told the outcome of the REDIRECT, not of the sign-in:
        // the state check still has to run, and it runs in the caller. Saying
        // "Verbunden" here and then failing that check would leave the user
        // reading a success page about a sign-in that did not happen, so the
        // wording is about the hand-off being received.
        let received = err_param.is_none() && code.is_some();
        respond(
            reader,
            200,
            if received {
                RECEIVED_BODY
            } else {
                FAILURE_BODY
            },
        )
        .await;

        if let Some(err) = err_param {
            // Everything in this query came over an unauthenticated loopback
            // port, so it is attacker-reachable by RFC 8252's own design. Cap
            // it and strip control characters before it reaches a message the
            // host renders.
            let err = sanitize_redirect_text(&err);
            let detail = err_description.map(|d| sanitize_redirect_text(&d));
            return Err(VcError::Authentication(match detail {
                Some(d) if !d.is_empty() => format!("Webex declined the sign-in: {err} — {d}"),
                _ => format!("Webex declined the sign-in: {err}"),
            }));
        }
        return Ok((
            code.ok_or_else(|| VcError::Protocol("the redirect carried no code".into()))?,
            state.ok_or_else(|| VcError::Protocol("the redirect carried no state".into()))?,
        ));
    }
}

/// Read the request line and drain the headers, bounded.
///
/// The byte budget is the belt to the deadline's braces: a peer that streams
/// headers slowly enough to keep resetting a per-read timeout still runs out of
/// budget. 16 KiB is far more than any real redirect and far less than memory.
async fn read_request(reader: &mut BufReader<tokio::net::TcpStream>) -> std::io::Result<String> {
    let mut request_line = String::new();
    let mut budget = MAX_REQUEST_BYTES;
    budget = budget.saturating_sub(reader.read_line(&mut request_line).await?);
    loop {
        let mut header = String::new();
        let read = reader.read_line(&mut header).await?;
        budget = budget.saturating_sub(read);
        // End of headers, peer closed, or it has said more than enough.
        if read == 0 || header == "\r\n" || header == "\n" || budget == 0 {
            break;
        }
    }
    Ok(request_line)
}

/// Cap and de-control a value that came off the redirect query.
fn sanitize_redirect_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// Minimal markup on purpose: corporate proxies that strip scripts and styles
/// leave this readable, and a screen reader gets a heading and a sentence.
const RECEIVED_BODY: &str = "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
     <title>Aperio</title></head><body style=\"font-family:sans-serif;padding:2rem\">\
     <h1>Anmeldung empfangen</h1><p>Du kannst diesen Tab schließen. Aperio schließt die Verbindung ab.</p>\
     </body></html>";

const FAILURE_BODY: &str = "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
     <title>Aperio</title></head><body style=\"font-family:sans-serif;padding:2rem\">\
     <h1>Verbindung fehlgeschlagen</h1><p>Du kannst diesen Tab schließen und es in Aperio \
     erneut versuchen.</p></body></html>";

const NOT_FOUND_BODY: &str = "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
     <title>Aperio</title></head><body style=\"font-family:sans-serif;padding:2rem\">\
     <p>Diese Adresse gehört nicht zur Anmeldung.</p></body></html>";

async fn respond(reader: BufReader<tokio::net::TcpStream>, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
    );
    let mut socket = reader.into_inner();
    socket.write_all(response.as_bytes()).await.ok();
    socket.shutdown().await.ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_and_challenge_follow_rfc_7636() {
        let (verifier, challenge) = generate_pkce();
        assert_eq!(verifier.len(), 43, "32 random bytes, base64url unpadded");
        assert!(!verifier.contains('=') && !verifier.contains('+') && !verifier.contains('/'));
        assert!(!challenge.contains('=') && !challenge.contains('+') && !challenge.contains('/'));
        // The challenge must be the digest OF THE VERIFIER STRING, not of the
        // random bytes — getting that wrong fails only at the token endpoint.
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
    }

    #[test]
    fn two_authorizations_never_share_a_verifier_or_state() {
        let a = authorize("c", &loopback_redirect_uri(), WEBEX_AUTH_URL).unwrap();
        let b = authorize("c", &loopback_redirect_uri(), WEBEX_AUTH_URL).unwrap();
        assert_ne!(a.pkce_verifier, b.pkce_verifier);
        assert_ne!(a.state, b.state);
    }

    #[test]
    fn the_authorize_url_carries_everything_webex_expects() {
        let redirect = loopback_redirect_uri();
        let authz = authorize("my-client", &redirect, WEBEX_AUTH_URL).unwrap();
        let url = url::Url::parse(&authz.authorize_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(url.host_str(), Some("webexapis.com"));
        assert_eq!(url.path(), "/v1/authorize");
        assert_eq!(q.get("client_id").map(String::as_str), Some("my-client"));
        assert_eq!(q.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            q.get("redirect_uri").map(String::as_str),
            Some(&redirect[..])
        );
        assert_eq!(
            q.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(q.get("state").map(String::as_str), Some(&authz.state[..]));
        // Untested until now: deleting the whole `scope` line left every test
        // green, and a consent screen with no scopes grants nothing.
        assert_eq!(q.get("scope").map(String::as_str), Some(SCOPES));
        // Pin the space separation on the WIRE, not only in the constant — the
        // form encoder writes spaces as `+`, so matching the decoded pair
        // exactly also proves the round-trip restores them.
        assert!(
            q["scope"].contains(' '),
            "Webex wants space-separated scopes"
        );
        assert!(!q["scope"].contains("spark:all"));
        // The challenge, never the verifier — sending the verifier would defeat
        // the whole point of PKCE.
        assert!(q.contains_key("code_challenge"));
        assert!(!authz.authorize_url.contains(&authz.pkce_verifier));
    }

    #[test]
    fn the_requested_scopes_are_the_three_meeting_ones_and_nothing_wider() {
        let scopes: Vec<&str> = SCOPES.split(' ').collect();
        assert_eq!(
            scopes,
            vec![
                SCOPE_SCHEDULES_WRITE,
                SCOPE_SCHEDULES_READ,
                SCOPE_PREFERENCES_READ
            ]
        );
        // `spark:all` is an automatic App Hub rejection and grants far more
        // than a calendar app has any business asking for.
        assert!(!SCOPES.contains("spark:all"));
    }

    #[test]
    fn the_redirect_uri_is_the_exact_registered_one() {
        // Webex compares verbatim, path included. If this ever drifts from what
        // is registered in the portal, every sign-in fails with an error that
        // names neither value.
        assert_eq!(loopback_redirect_uri(), "http://127.0.0.1:8080/oauth/webex");
    }

    #[test]
    fn an_empty_client_id_is_refused_before_any_network_call() {
        assert!(matches!(
            authorize("   ", &loopback_redirect_uri(), WEBEX_AUTH_URL),
            Err(VcError::InvalidInput(_))
        ));
    }

    #[test]
    fn error_bodies_are_summarised_in_all_the_shapes_webex_uses() {
        assert_eq!(
            summarise_error(r#"{"error":"invalid_client","error_description":"bad secret"}"#),
            // BOTH halves: `invalid_client` and `invalid_grant` are entirely
            // different problems — wrong credentials versus a spent code — and
            // only the machine-readable code says which.
            "invalid_client: bad secret"
        );
        assert_eq!(
            summarise_error(r#"{"message":"nope","trackingId":"ROUTER_123"}"#),
            "nope (Webex tracking id ROUTER_123)"
        );
        assert_eq!(
            summarise_error(r#"{"errors":[{"description":"deep"}]}"#),
            "deep"
        );
        assert_eq!(summarise_error("not json at all"), "not json at all");
        assert_eq!(summarise_error("   "), "no response body");
    }

    #[tokio::test]
    async fn exchange_sends_pkce_and_reports_the_rotated_refresh_token() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/access_token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "authorization_code".into()),
                mockito::Matcher::UrlEncoded("code_verifier".into(), "the-verifier".into()),
                mockito::Matcher::UrlEncoded("client_secret".into(), "s3cret".into()),
                mockito::Matcher::UrlEncoded(
                    "redirect_uri".into(),
                    "http://127.0.0.1:8080/oauth/webex".into(),
                ),
            ]))
            .with_status(200)
            .with_body(
                r#"{"access_token":"AT","refresh_token":"RT-new","expires_in":1209599,
                    "refresh_token_expires_in":7776000,"scope":"meeting:schedules_read"}"#,
            )
            .create_async()
            .await;

        let before = Utc::now();
        let tokens = exchange_code(
            &reqwest::Client::new(),
            &format!("{}/access_token", server.url()),
            "client",
            Some("s3cret"),
            "the-code",
            "the-verifier",
            &loopback_redirect_uri(),
        )
        .await
        .expect("exchange");

        assert_eq!(tokens.access_token, "AT");
        assert_eq!(tokens.refresh_token.as_deref(), Some("RT-new"));

        // Pin WHICH clock is WHICH. Swapping the two sources still passes an
        // is_some() check, but makes every access token look valid for 90 days
        // — nothing would ever refresh and every call would 401 — while the
        // refresh token would look dead in 14.
        let access_secs = (tokens.expires_at - before).num_seconds();
        assert!(
            (1_209_599 - 5..=1_209_599).contains(&access_secs),
            "expires_at must come from expires_in (1209599 s), got {access_secs}"
        );
        let refresh_at = tokens
            .refresh_expires_at
            .expect("the 90-day refresh clock must be tracked");
        let refresh_secs = (refresh_at - before).num_seconds();
        assert!(
            (7_776_000 - 5..=7_776_000).contains(&refresh_secs),
            "refresh_expires_at must come from refresh_token_expires_in (7776000 s),              got {refresh_secs}"
        );
        m.assert_async().await;
    }

    #[test]
    fn the_public_client_posture_sends_pkce_and_no_secret() {
        // Whether Webex ACCEPTS this is the open question; that the code can
        // ask, and asks correctly, is what this pins down.
        let form = code_exchange_form("client", None, "the-code", "the-verifier", "http://r");
        let keys: Vec<&str> = form.iter().map(|(k, _)| *k).collect();
        assert!(!keys.contains(&"client_secret"), "no secret may be sent");
        assert!(keys.contains(&"code_verifier"), "PKCE always rides along");
        assert_eq!(form[0], ("grant_type", "authorization_code"));
    }

    #[test]
    fn the_confidential_posture_adds_the_secret_and_keeps_pkce() {
        let form = code_exchange_form("client", Some("s3"), "c", "v", "http://r");
        assert!(form.contains(&("client_secret", "s3")));
        assert!(form.contains(&("code_verifier", "v")));
    }

    #[test]
    fn the_refresh_form_mirrors_the_same_two_postures() {
        let public = refresh_form("client", None, "RT");
        assert!(!public.iter().any(|(k, _)| *k == "client_secret"));
        assert!(public.contains(&("grant_type", "refresh_token")));
        assert!(public.contains(&("refresh_token", "RT")));

        let confidential = refresh_form("client", Some("s3"), "RT");
        assert!(confidential.contains(&("client_secret", "s3")));
    }

    #[tokio::test]
    async fn a_refusal_says_which_posture_was_used() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/access_token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_request","error_description":"missing secret"}"#)
            .create_async()
            .await;
        let err = exchange_code(
            &reqwest::Client::new(),
            &format!("{}/access_token", server.url()),
            "client",
            None,
            "c",
            "v",
            &loopback_redirect_uri(),
        )
        .await
        .expect_err("must fail");
        let text = err.to_string();
        assert!(text.contains("public client"), "got {text}");
        assert!(text.contains("missing secret"), "got {text}");
    }

    #[test]
    fn an_absurd_lifetime_does_not_panic_the_plugin() {
        // chrono::Duration::seconds panics out of range, and this crate links
        // into a dlopen'd plugin that aborts on panic. A hostile or broken
        // server must cost one refresh, not the process.
        assert_eq!(checked_seconds(i64::MAX), chrono::Duration::zero());
        assert_eq!(checked_seconds(-1), chrono::Duration::zero());
        assert_eq!(checked_seconds(0), chrono::Duration::zero());
        assert_eq!(checked_seconds(60), chrono::Duration::seconds(60));
    }

    #[test]
    fn redirect_text_is_capped_and_stripped_of_control_characters() {
        // The loopback port is unauthenticated by RFC 8252's own design, so
        // anything in that query is attacker-reachable before it reaches a
        // message the host renders.
        let hostile = format!("a\nb\r\x07{}", "x".repeat(500));
        let clean = sanitize_redirect_text(&hostile);
        assert!(clean.chars().count() <= 200);
        assert!(!clean.contains('\n') && !clean.contains('\r') && !clean.contains('\x07'));
    }

    #[tokio::test]
    async fn a_server_error_is_transient_not_a_protocol_bug() {
        // `Protocol` reads as "this adapter is broken" and is not retried; a
        // 503 is the server having a bad minute and must stay retryable.
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/access_token")
            .with_status(503)
            .with_body("upstream unavailable")
            .create_async()
            .await;
        let err = refresh(
            &reqwest::Client::new(),
            &format!("{}/access_token", server.url()),
            "client",
            Some("s"),
            "RT",
        )
        .await
        .expect_err("must fail");
        assert!(matches!(err, VcError::Network(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_missing_expiry_reads_as_already_expired() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/access_token")
            .with_status(200)
            .with_body(r#"{"access_token":"AT"}"#)
            .create_async()
            .await;
        let tokens = refresh(
            &reqwest::Client::new(),
            &format!("{}/access_token", server.url()),
            "client",
            Some("s"),
            "RT",
        )
        .await
        .unwrap();
        assert!(
            tokens.access_expired(Utc::now(), Duration::from_secs(0)),
            "an unknown lifetime must refresh, not be trusted forever"
        );
    }
}
