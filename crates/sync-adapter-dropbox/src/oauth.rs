//! OAuth 2.0 PKCE flow for Dropbox's desktop-app model.
//!
//! Mirrors `cal-adapter-google`'s `auth.rs` — same loopback
//! redirect pattern, same PKCE primitives. Differences worth
//! noting:
//!
//! - Dropbox supports two app types: **public** apps (PKCE
//!   only, no client_secret) and **confidential** apps
//!   (client_secret required). The adapter accepts both; an
//!   empty `client_secret` means "this is a public app".
//! - Dropbox requires `token_access_type=offline` to issue a
//!   refresh token (analogous to Google's `access_type=offline`).
//! - Dropbox doesn't have a `prompt=consent` analogue; the
//!   refresh token comes back on every successful authorise as
//!   long as the offline scope was requested.
//!
//! ## Flow
//!
//!   1. Bind a TCP listener on `127.0.0.1` on an ephemeral
//!      port; the URL `http://127.0.0.1:{port}` is our
//!      redirect_uri.
//!   2. Generate a PKCE verifier (32 random bytes →
//!      URL-safe base64) and the SHA-256 challenge.
//!   3. Open the user's default browser at the Dropbox
//!      authorisation page with client_id, challenge, state.
//!   4. The listener accepts one connection, parses
//!      `?code=…&state=…`, responds with a "you can close
//!      this tab" page.
//!   5. POST to `https://api.dropboxapi.com/oauth2/token` with
//!      the code + verifier + client_id (+ secret if any) +
//!      redirect_uri. The response carries access_token,
//!      refresh_token, expires_in.
//!
//! Refresh uses `grant_type=refresh_token` against the same
//! token endpoint. The refresh token is long-lived (no
//! published expiry on Dropbox's side; effectively until the
//! user revokes the app's access) and gets stored next to the
//! access token in the platform keychain by the command layer.

use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, warn};

use crate::error::{DropboxError, DropboxResult};

pub const DROPBOX_AUTH_URL: &str = "https://www.dropbox.com/oauth2/authorize";
pub const DROPBOX_TOKEN_URL: &str = "https://api.dropboxapi.com/oauth2/token";

/// 5 minute ceiling on the consent dance — same window the
/// Google adapter uses, and Dropbox's own session timeout sits
/// in roughly the same range.
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

/// Bundle of tokens we get back from Dropbox's `/oauth2/token`
/// endpoint plus the moment we expect the access token to
/// expire. `refresh_token` is optional because re-auths may
/// omit it (the caller is expected to preserve the previously
/// stored one in that case).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
}

/// Run the full OAuth dance: spawn loopback listener, open
/// browser, wait for redirect, exchange code for tokens.
///
/// `client_secret` is allowed to be empty — Dropbox's PKCE-only
/// public-app model omits it. Confidential apps pass the
/// secret as documented by the Dropbox developer console.
pub async fn run(
    client_id: &str,
    client_secret: &str,
    http: &reqwest::Client,
) -> DropboxResult<TokenSet> {
    run_against(
        client_id,
        client_secret,
        DROPBOX_AUTH_URL,
        DROPBOX_TOKEN_URL,
        http,
    )
    .await
}

/// Variant that accepts the auth + token URLs explicitly —
/// used by tests against a mockito server so the real Dropbox
/// endpoints don't get hit in CI.
pub async fn run_against(
    client_id: &str,
    client_secret: &str,
    auth_url: &str,
    token_url: &str,
    http: &reqwest::Client,
) -> DropboxResult<TokenSet> {
    if client_id.trim().is_empty() {
        return Err(DropboxError::Config(
            "client_id must not be empty".into(),
        ));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let auth =
        build_auth_url(auth_url, client_id, &redirect_uri, &challenge, &state)?;
    debug!(url = %auth, "opening Dropbox consent screen");
    // open::that is best-effort. On headless / no-browser hosts
    // we surface the URL via tracing so the user can copy-paste
    // it manually.
    if let Err(e) = open::that(auth.as_str()) {
        warn!(?e, "failed to launch browser; user must copy the URL manually");
    }

    let (code, returned_state) = wait_for_redirect(listener).await?;
    if returned_state != state {
        return Err(DropboxError::Csrf);
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

/// Use a refresh token to mint a fresh access token. Called by
/// the API layer when the cached token is about to expire or
/// the API responds 401.
///
/// Returns a fresh [`TokenSet`]; `refresh_token` may be `None`
/// (Dropbox's policy is to keep reusing the old one). Callers
/// preserve the previously stored refresh token if the response
/// omits it.
pub async fn refresh(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
    http: &reqwest::Client,
) -> DropboxResult<TokenSet> {
    let now = Utc::now();
    let mut pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if !client_secret.is_empty() {
        pairs.push(("client_secret", client_secret));
    }
    let body = serde_urlencoded::to_string(&pairs)
        .map_err(|e| DropboxError::Protocol(format!("encode refresh body: {e}")))?;

    let response = http
        .post(DROPBOX_TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    parse_token_response(response, now).await
}

// ── Internal helpers ─────────────────────────────────────────────

/// Generate a PKCE verifier (43-char URL-safe base64 of 32
/// random bytes) and its SHA-256 challenge (URL-safe base64 of
/// the digest). RFC 7636 §4.1 + §4.2.
fn generate_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

/// 32 hex characters of randomness for the CSRF `state`
/// parameter.
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
) -> DropboxResult<url::Url> {
    let mut url = url::Url::parse(auth_url)
        .map_err(|e| DropboxError::Config(format!("invalid auth_url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        // `token_access_type=offline` makes Dropbox issue a
        // refresh token. Without this, only an access token
        // comes back and the adapter can't keep the
        // connection alive past 4 hours.
        .append_pair("token_access_type", "offline");
    Ok(url)
}

/// Accept one HTTP request, parse `code` + `state` from the
/// query string, respond with a friendly close-this-tab page.
async fn wait_for_redirect(
    listener: TcpListener,
) -> DropboxResult<(String, String)> {
    let accept = tokio::time::timeout(AUTH_TIMEOUT, listener.accept()).await;
    let (socket, _peer) = match accept {
        Err(_) => return Err(DropboxError::AuthTimeout),
        Ok(Err(e)) => return Err(DropboxError::Io(e.to_string())),
        Ok(Ok(pair)) => pair,
    };

    let mut reader = BufReader::new(socket);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    // Drain the remaining headers so the client doesn't see a
    // half-closed connection before we write the response.
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
        .ok_or_else(|| DropboxError::Protocol("malformed request line".into()))?;
    let pseudo = url::Url::parse(&format!("http://127.0.0.1{path}"))
        .map_err(|e| DropboxError::Protocol(format!("query parse: {e}")))?;

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
        // Minimal HTML — no JS, no CSS that corporate proxies
        // might strip.
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>Aperio</title></head><body style=\"font-family:sans-serif;padding:2rem\">\
         <h1>Verbunden \u{2713}</h1><p>Du kannst diesen Tab schlie\u{00DF}en und zu Aperio zur\u{00FC}ckkehren.</p>\
         </body></html>"
    } else {
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>Aperio</title></head><body style=\"font-family:sans-serif;padding:2rem\">\
         <h1>Verbindung fehlgeschlagen</h1><p>Du kannst diesen Tab schlie\u{00DF}en und es in Aperio erneut versuchen.</p>\
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
        return Err(DropboxError::AuthDenied(err));
    }
    Ok((
        code.ok_or_else(|| {
            DropboxError::Protocol("redirect missing `code`".into())
        })?,
        state.ok_or_else(|| {
            DropboxError::Protocol("redirect missing `state`".into())
        })?,
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
) -> DropboxResult<TokenSet> {
    let now = Utc::now();
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
    ];
    if !client_secret.is_empty() {
        pairs.push(("client_secret", client_secret));
    }
    let body = serde_urlencoded::to_string(&pairs)
        .map_err(|e| DropboxError::Protocol(format!("encode token body: {e}")))?;

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
) -> DropboxResult<TokenSet> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(DropboxError::Http {
            status: status.as_u16(),
            message: text.chars().take(300).collect(),
        });
    }

    #[derive(Deserialize)]
    struct Raw {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default = "default_expires_in")]
        expires_in: i64,
    }
    /// Dropbox's contract: tokens last 4 hours by default;
    /// callers that don't see `expires_in` in the response
    /// should assume a conservative fallback. 14400 = 4h.
    fn default_expires_in() -> i64 {
        14_400
    }
    let raw: Raw = serde_json::from_str(&text)
        .map_err(|e| DropboxError::Protocol(format!("token json: {e}")))?;
    Ok(TokenSet {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        // 30 s safety margin so the adapter refreshes a little
        // before the strict expiry, saving a wasted 401.
        expires_at: requested_at
            + chrono::Duration::seconds((raw.expires_in - 30).max(0)),
    })
}

/// Mini in-house urlencoded body builder — avoids pulling in
/// the `serde_urlencoded` crate as a direct dependency for one
/// call. Mirrors the same helper in cal-adapter-google's
/// auth.rs.
mod serde_urlencoded {
    pub fn to_string(pairs: &[(&str, &str)]) -> Result<String, std::fmt::Error> {
        use std::fmt::Write;
        let mut out = String::new();
        for (i, (k, v)) in pairs.iter().enumerate() {
            if i > 0 {
                out.push('&');
            }
            write!(out, "{}=", percent_encode(k))?;
            write!(out, "{}", percent_encode(v))?;
        }
        Ok(out)
    }

    fn percent_encode(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for byte in input.bytes() {
            // RFC 3986 unreserved + the few "form-data is fine"
            // characters; everything else goes percent-encoded.
            match byte {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'.'
                | b'_'
                | b'~' => out.push(byte as char),
                _ => {
                    out.push('%');
                    out.push_str(&format!("{byte:02X}"));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_and_challenge_are_different() {
        let (verifier, challenge) = generate_pkce();
        assert_ne!(verifier, challenge);
        // Verifier is the base64 of 32 random bytes, URL-safe
        // no-pad → exactly 43 chars.
        assert_eq!(verifier.len(), 43);
        assert_eq!(challenge.len(), 43);
    }

    #[test]
    fn state_is_hex_of_16_random_bytes() {
        let state = generate_state();
        assert_eq!(state.len(), 32);
        assert!(state.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn build_auth_url_threads_every_parameter() {
        let url = build_auth_url(
            "https://example.com/auth",
            "client-x",
            "http://127.0.0.1:1234",
            "challenge-x",
            "state-x",
        )
        .unwrap();
        let s = url.to_string();
        assert!(s.contains("client_id=client-x"));
        assert!(s.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1234"));
        assert!(s.contains("response_type=code"));
        assert!(s.contains("code_challenge=challenge-x"));
        assert!(s.contains("code_challenge_method=S256"));
        assert!(s.contains("state=state-x"));
        assert!(s.contains("token_access_type=offline"));
    }
}
