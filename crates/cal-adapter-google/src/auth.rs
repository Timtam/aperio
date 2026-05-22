//! OAuth 2.0 PKCE flow for Google's "installed app" model.
//!
//! Google's recommended OAuth pattern for desktop apps is the
//! authorisation-code flow with PKCE (RFC 7636) and a loopback
//! redirect:
//!
//!   1. Aperio binds a TCP listener on 127.0.0.1 on an ephemeral
//!      port and remembers the URL `http://127.0.0.1:{port}`.
//!   2. Aperio constructs an authorisation URL with the user's
//!      Google Cloud Console OAuth client id, a freshly generated
//!      PKCE challenge, a CSRF `state` nonce, and the loopback
//!      redirect URI, then opens the user's default browser at it
//!      via the `open` crate.
//!   3. The user signs in / chooses an account / consents in the
//!      browser. Google redirects to the loopback URI with `?code=…
//!      &state=…` (or `?error=…` on failure).
//!   4. The listener accepts one connection, parses the request
//!      line, extracts the code, verifies the state matches, and
//!      responds with a "you can close this tab" page.
//!   5. Aperio POSTs to `https://oauth2.googleapis.com/token` with
//!      the code, the PKCE verifier, the client id and the same
//!      redirect URI. Google returns `access_token`,
//!      `refresh_token`, `expires_in`.
//!
//! Refresh uses `grant_type=refresh_token` against the same token
//! endpoint. The refresh token is long-lived (≈ 6 months) and gets
//! stored next to the access token in the platform keychain.
//!
//! `oauth2`-the-crate was considered and rejected: its default
//! pulls in reqwest with `native-tls`, which would force Aperio to
//! link against OpenSSL on Linux. PKCE is ~20 lines of code and
//! the token POST is one more, so a manual implementation keeps
//! the dependency tree tight and aligned with the rest of the app
//! (rustls-only).

use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, warn};

use crate::error::{GoogleError, GoogleResult};

pub const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Read + write calendar access. Read-only would be
/// `https://www.googleapis.com/auth/calendar.readonly`; we ask for
/// full access up front so the write paths don't trigger a second
/// consent screen.
pub const SCOPE_CALENDAR: &str = "https://www.googleapis.com/auth/calendar";
/// Read + write Tasks access. Same rationale as the calendar scope
/// — the listing/CRUD flows both need write so we may as well ask
/// up front.
pub const SCOPE_TASKS: &str = "https://www.googleapis.com/auth/tasks";
/// The combined scope string we request on the consent screen.
/// Google's OAuth dialog renders one entry per space-separated
/// scope; granting once covers every feature this adapter exposes
/// (Calendar + Tasks, Phase 6d.3) so users don't see a separate
/// dialog when they later open the tasks side of the app.
pub const SCOPES: &str = "https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/tasks";
/// 5 minute ceiling on the consent dance. Google rejects unused
/// codes after a similar window anyway and waiting longer means
/// Aperio is hung holding a TCP port.
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

/// Bundle of tokens we get back from Google's `/token` endpoint plus
/// the moment we expect the access token to expire. Everything except
/// `access_token` is optional because Google only returns
/// `refresh_token` on the FIRST consent — on subsequent re-auths
/// (e.g. when the user re-grants permission) the refresh token may
/// be omitted and the previously stored one must be reused.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub scope: Option<String>,
}

/// Run the full OAuth dance: spawn listener, open browser, wait
/// for redirect, exchange code for tokens. Returns the resulting
/// [`TokenSet`] on success.
///
/// `client_id` and `client_secret` are both issued together when
/// the user creates a Desktop-app OAuth client in their Google
/// Cloud Console. PKCE on its own (RFC 7636) would obviate the
/// secret; Google's implementation still requires it on the token
/// endpoint regardless. Their own docs concede the point —
/// "Desktop apps store the client secret in the source code. In
/// this context, the client secret is not treated as a secret."
///
/// `token_url` and `auth_url` are exposed as parameters for tests;
/// production callers pass the constants above via [`run_default`].
pub async fn run(
    client_id: &str,
    client_secret: &str,
    auth_url: &str,
    token_url: &str,
    http: &reqwest::Client,
) -> GoogleResult<TokenSet> {
    if client_id.trim().is_empty() {
        return Err(GoogleError::Config("client_id must not be empty".into()));
    }
    if client_secret.trim().is_empty() {
        return Err(GoogleError::Config(
            "client_secret must not be empty — Google's token endpoint rejects requests without it"
                .into(),
        ));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let auth = build_auth_url(auth_url, client_id, &redirect_uri, &challenge, &state)?;
    debug!(url = %auth, "opening Google consent screen");
    // open::that is best-effort. On the rare host where it fails (no
    // default browser, headless container) we surface the URL via the
    // error so the user can copy-paste it manually.
    if let Err(e) = open::that(auth.as_str()) {
        warn!(?e, "failed to launch browser; user must copy the URL manually");
    }

    let (code, returned_state) = wait_for_redirect(listener).await?;
    if returned_state != state {
        return Err(GoogleError::Csrf);
    }

    exchange_code(
        http,
        token_url,
        client_id,
        client_secret,
        &code,
        &verifier,
        &redirect_uri,
    )
    .await
}

/// Convenience wrapper that hits the Google production endpoints.
pub async fn run_default(
    client_id: &str,
    client_secret: &str,
    http: &reqwest::Client,
) -> GoogleResult<TokenSet> {
    run(
        client_id,
        client_secret,
        GOOGLE_AUTH_URL,
        GOOGLE_TOKEN_URL,
        http,
    )
    .await
}

/// Use a refresh token to mint a new access token. Called by the API
/// layer when a request comes back with 401. Returns a new [`TokenSet`]
/// — the `refresh_token` field may be `None` (Google's policy is to
/// keep reusing the old one), in which case the caller is expected to
/// preserve the previously stored refresh token.
pub async fn refresh(
    client_id: &str,
    client_secret: &str,
    token_url: &str,
    refresh_token: &str,
    http: &reqwest::Client,
) -> GoogleResult<TokenSet> {
    let now = Utc::now();
    let body = serde_urlencoded::to_string(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ])
    .map_err(|e| GoogleError::Protocol(format!("encode refresh body: {e}")))?;

    let response = http
        .post(token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    parse_token_response(response, now).await
}

// ── Internal helpers ─────────────────────────────────────────────────────

/// Generate a PKCE verifier (43-char URL-safe base64 of 32 random
/// bytes) and its SHA-256 challenge (URL-safe base64 of the digest).
/// RFC 7636 §4.1 + §4.2.
fn generate_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

/// 32 hex characters of randomness for the CSRF `state` parameter.
fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn build_auth_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> GoogleResult<url::Url> {
    let mut url = url::Url::parse(auth_url)
        .map_err(|e| GoogleError::Config(format!("invalid auth_url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        // `access_type=offline` makes Google issue a refresh token.
        // Without this, only an access token comes back and Aperio
        // can't keep the connection alive past the first hour.
        .append_pair("access_type", "offline")
        // `prompt=consent` forces the consent screen even on re-auth
        // so the refresh token always shows up — otherwise Google
        // omits it after the first grant and we lose the chance.
        .append_pair("prompt", "consent");
    Ok(url)
}

/// Accept one HTTP request, parse `code` + `state` from the query,
/// respond with a friendly close-this-tab page.
async fn wait_for_redirect(
    listener: TcpListener,
) -> GoogleResult<(String, String)> {
    let accept = tokio::time::timeout(AUTH_TIMEOUT, listener.accept()).await;
    let (socket, _peer) = match accept {
        Err(_) => return Err(GoogleError::AuthTimeout),
        Ok(Err(e)) => return Err(GoogleError::Io(e.to_string())),
        Ok(Ok(pair)) => pair,
    };

    let mut reader = BufReader::new(socket);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    // Drain the remaining headers so the client doesn't see a half-
    // closed connection before we write the response. We don't need
    // the values for anything.
    loop {
        let mut header = String::new();
        let read = reader.read_line(&mut header).await?;
        if read == 0 || header == "\r\n" || header == "\n" {
            break;
        }
    }

    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| GoogleError::Protocol("malformed request line".into()))?;
    let pseudo = url::Url::parse(&format!("http://127.0.0.1{path}"))
        .map_err(|e| GoogleError::Protocol(format!("query parse: {e}")))?;

    let mut code = None;
    let mut state = None;
    let mut err_param = None;
    for (k, v) in pseudo.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => err_param = Some(v.into_owned()),
            _ => {}
        }
    }

    let success = err_param.is_none() && code.is_some();
    let response_body = if success {
        // The body is plain HTML so it renders nicely if the user
        // sees it; we keep the markup minimal so corporate proxies
        // that strip scripts / styles don't make it weird.
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>Aperio</title></head><body style=\"font-family:sans-serif;padding:2rem\">\
         <h1>Verbunden ✓</h1><p>Du kannst diesen Tab schließen und zu Aperio zurückkehren.</p>\
         </body></html>"
    } else {
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>Aperio</title></head><body style=\"font-family:sans-serif;padding:2rem\">\
         <h1>Verbindung fehlgeschlagen</h1><p>Du kannst diesen Tab schließen und es in Aperio erneut versuchen.</p>\
         </body></html>"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = response_body.len(),
        body = response_body,
    );
    let mut socket = reader.into_inner();
    socket.write_all(response.as_bytes()).await.ok();
    socket.shutdown().await.ok();

    if let Some(err) = err_param {
        return Err(GoogleError::AuthDenied(err));
    }
    Ok((
        code.ok_or_else(|| GoogleError::Protocol("redirect missing `code`".into()))?,
        state.ok_or_else(|| GoogleError::Protocol("redirect missing `state`".into()))?,
    ))
}

async fn exchange_code(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> GoogleResult<TokenSet> {
    let now = Utc::now();
    let body = serde_urlencoded::to_string(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
    ])
    .map_err(|e| GoogleError::Protocol(format!("encode token body: {e}")))?;

    let response = http
        .post(token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    parse_token_response(response, now).await
}

async fn parse_token_response(
    response: reqwest::Response,
    requested_at: DateTime<Utc>,
) -> GoogleResult<TokenSet> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(GoogleError::Http {
            status: status.as_u16(),
            message: text.chars().take(300).collect(),
        });
    }

    #[derive(Deserialize)]
    struct Raw {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        expires_in: i64,
        #[serde(default)]
        scope: Option<String>,
    }
    let raw: Raw = serde_json::from_str(&text)
        .map_err(|e| GoogleError::Protocol(format!("token json: {e}")))?;
    Ok(TokenSet {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        // Subtract a 30 s safety margin from `expires_in` so the
        // adapter starts trying to refresh before the token strictly
        // dies — saves a wasted 401 round-trip.
        expires_at: requested_at
            + chrono::Duration::seconds((raw.expires_in - 30).max(0)),
        scope: raw.scope,
    })
}

// Tiny shim for the missing `serde_urlencoded` re-export — we use
// reqwest's bundled copy via a local module so we don't pull in the
// crate as a direct dep just for one call.
mod serde_urlencoded {
    pub fn to_string(pairs: &[(&str, &str)]) -> Result<String, std::fmt::Error> {
        use std::fmt::Write;
        let mut out = String::new();
        for (i, (k, v)) in pairs.iter().enumerate() {
            if i > 0 {
                out.push('&');
            }
            write!(out, "{}={}", url_encode(k), url_encode(v))?;
        }
        Ok(out)
    }

    fn url_encode(s: &str) -> String {
        // Form-url-encoding: same as percent-encoding but with space
        // → `+`. Google's token endpoint is happy with either, but
        // strict spec says `+`.
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char);
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{:02X}", b)),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_matches_challenge_via_sha256() {
        let (verifier, challenge) = generate_pkce();
        // Length sanity: 32 bytes base64-no-pad → 43 chars.
        assert_eq!(verifier.len(), 43);
        assert_eq!(challenge.len(), 43);
        // The challenge is the URL-safe base64 of SHA-256(verifier).
        let digest = Sha256::digest(verifier.as_bytes());
        let expected =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(challenge, expected);
    }

    #[test]
    fn state_is_32_hex_chars() {
        let s = generate_state();
        assert_eq!(s.len(), 32);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn build_auth_url_carries_every_required_param() {
        let url = build_auth_url(
            GOOGLE_AUTH_URL,
            "my-client-id.apps.googleusercontent.com",
            "http://127.0.0.1:12345",
            "challenge",
            "state",
        )
        .unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs.get("client_id").map(String::as_str), Some("my-client-id.apps.googleusercontent.com"));
        assert_eq!(pairs.get("redirect_uri").map(String::as_str), Some("http://127.0.0.1:12345"));
        assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(pairs.get("scope").map(String::as_str), Some(SCOPES));
        assert_eq!(pairs.get("code_challenge_method").map(String::as_str), Some("S256"));
        assert_eq!(pairs.get("code_challenge").map(String::as_str), Some("challenge"));
        assert_eq!(pairs.get("state").map(String::as_str), Some("state"));
        assert_eq!(pairs.get("access_type").map(String::as_str), Some("offline"));
        assert_eq!(pairs.get("prompt").map(String::as_str), Some("consent"));
    }

    #[tokio::test]
    async fn exchange_code_parses_a_well_formed_response() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                  "access_token": "ya29.fake",
                  "refresh_token": "1//0e.fake",
                  "expires_in": 3600,
                  "scope": "https://www.googleapis.com/auth/calendar",
                  "token_type": "Bearer"
                }"#,
            )
            .create_async()
            .await;

        let http = reqwest::Client::new();
        let tokens = exchange_code(
            &http,
            &format!("{}/token", server.url()),
            "my-client",
            "shh-secret",
            "auth-code",
            "verifier",
            "http://127.0.0.1:12345",
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "ya29.fake");
        assert_eq!(tokens.refresh_token.as_deref(), Some("1//0e.fake"));
        assert!(tokens.expires_at > Utc::now());
    }

    #[tokio::test]
    async fn refresh_works_without_a_returned_refresh_token() {
        // Google's refresh response often omits `refresh_token` — the
        // caller is expected to keep using the previously stored one.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .with_status(200)
            .with_body(
                r#"{
                  "access_token": "ya29.refreshed",
                  "expires_in": 3600,
                  "scope": "https://www.googleapis.com/auth/calendar",
                  "token_type": "Bearer"
                }"#,
            )
            .create_async()
            .await;

        let http = reqwest::Client::new();
        let tokens = refresh(
            "my-client",
            "shh-secret",
            &format!("{}/token", server.url()),
            "old-refresh",
            &http,
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "ya29.refreshed");
        assert!(tokens.refresh_token.is_none());
    }

    #[tokio::test]
    async fn exchange_code_sends_client_secret_in_form_body() {
        // Regression: Google's token endpoint rejects PKCE Desktop-
        // app exchanges that omit client_secret with
        // `invalid_request: client_secret is missing`, even though
        // RFC 7636 says PKCE alone should be enough. We have to
        // send the secret regardless.
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/token")
            .match_body(mockito::Matcher::Regex(
                "client_secret=shh-secret".to_string(),
            ))
            .with_status(200)
            .with_body(
                r#"{"access_token":"ya29.fake","expires_in":3600,"token_type":"Bearer"}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let http = reqwest::Client::new();
        exchange_code(
            &http,
            &format!("{}/token", server.url()),
            "my-client",
            "shh-secret",
            "code",
            "verifier",
            "http://127.0.0.1:12345",
        )
        .await
        .unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_also_sends_client_secret() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("grant_type=refresh_token".into()),
                mockito::Matcher::Regex("client_secret=shh-secret".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"{"access_token":"ya29.refreshed","expires_in":3600,"token_type":"Bearer"}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let http = reqwest::Client::new();
        refresh(
            "my-client",
            "shh-secret",
            &format!("{}/token", server.url()),
            "old-refresh",
            &http,
        )
        .await
        .unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn exchange_code_surfaces_http_failure() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant","error_description":"code expired"}"#,
            )
            .create_async()
            .await;
        let http = reqwest::Client::new();
        let err = exchange_code(
            &http,
            &format!("{}/token", server.url()),
            "c",
            "s",
            "code",
            "v",
            "r",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GoogleError::Http { status: 400, .. }));
    }
}
