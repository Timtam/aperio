//! OAuth 2.0 PKCE flow for Google's installed-app model,
//! Drive-API scoped.
//!
//! Same shape as `cal-adapter-google`'s `auth.rs` and the
//! Dropbox adapter's `oauth.rs`. The only Google-specific bits
//! are the scope (`drive.file` — covers exactly the files this
//! app creates, never the rest of the user's Drive) and the
//! `access_type=offline` + `prompt=consent` parameters Google
//! requires to mint a refresh token reliably.
//!
//! The flow:
//!
//!   1. Bind a TCP listener on `127.0.0.1` on an ephemeral
//!      port; `http://127.0.0.1:{port}` is the redirect_uri.
//!   2. Generate a PKCE verifier (32 random bytes →
//!      URL-safe base64) and the SHA-256 challenge.
//!   3. Open the user's browser at the Google consent page
//!      with `client_id`, `scope=drive.file`, the PKCE
//!      challenge, a CSRF `state` nonce.
//!   4. The listener accepts one connection, parses
//!      `?code=…&state=…`, responds with a "you can close
//!      this tab" page.
//!   5. POST to `https://oauth2.googleapis.com/token` with the
//!      code + verifier + client credentials + redirect URI.
//!      The response carries access_token, refresh_token,
//!      expires_in.
//!
//! Google requires `client_secret` for all installed apps —
//! unlike Dropbox's optional secret. The cal-adapter-google
//! crate's docstring sums up the rationale verbatim: "Desktop
//! apps store the client secret in the source code. In this
//! context, the client secret is not treated as a secret."

use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, warn};

use super::error::{GoogleDriveError, GoogleDriveResult};

// Google's endpoints are Google's, whichever half of this crate is asking.
// They were declared twice while Drive lived in its own crate, and two copies
// of a URL is two things to keep in step for no benefit.
pub use crate::auth::{GOOGLE_AUTH_URL, GOOGLE_TOKEN_URL};

/// `drive.file` is the per-app scope: Aperio can read + write
/// only the files it creates itself, never see the rest of the
/// user's Drive. That's the right principle-of-least-privilege
/// choice for a sync target — we don't need to browse the
/// user's files, only manage our own dataset folder.
pub const SCOPE_DRIVE_FILE: &str = "https://www.googleapis.com/auth/drive.file";

/// 5 minute ceiling on the consent dance.
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

/// Bundle of tokens we get back from Google's `/token`
/// endpoint plus the moment we expect the access token to
/// expire. `refresh_token` is optional because Google only
/// returns it on the FIRST consent — subsequent re-auths may
/// omit it and the previously stored value must be reused.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
}

/// The host-drivable **authorize** phase: build the consent URL + the PKCE
/// verifier + the CSRF `state`. Pure (no I/O) — the CALLER opens the URL and
/// captures the redirect: the desktop loopback (via [`run`]) or, on mobile, a
/// native auth session. The caller then hands `code` + the returned `state` +
/// this `pkce_verifier` to [`exchange_code`]. `redirect_uri` is caller-supplied
/// (`http://127.0.0.1:{port}` for the loopback, `aperio://oauth-callback` for a
/// native session) and MUST match the one used at exchange. Google requires the
/// `client_secret` only at the [`exchange_code`] step, not here.
pub fn authorize(
    client_id: &str,
    redirect_uri: &str,
    auth_url: &str,
) -> GoogleDriveResult<AuthorizeResponse> {
    if client_id.trim().is_empty() {
        return Err(GoogleDriveError::Config(
            "client_id must not be empty".into(),
        ));
    }
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();
    let url = build_auth_url(auth_url, client_id, redirect_uri, &challenge, &state)?;
    Ok(AuthorizeResponse {
        authorize_url: url.to_string(),
        pkce_verifier: verifier,
        state,
    })
}

/// Output of [`authorize`]. The host keeps `pkce_verifier` + `state` opaque
/// between the two phases (the adapter holds no cross-phase state) and replays
/// them into [`exchange_code`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeResponse {
    pub authorize_url: String,
    pub pkce_verifier: String,
    pub state: String,
}

/// Run the full OAuth dance: spawn loopback listener, open
/// browser, wait for redirect, exchange code for tokens. The desktop path;
/// mobile drives [`authorize`] + [`exchange_code`] around a native auth session.
pub async fn run(
    client_id: &str,
    client_secret: &str,
    http: &reqwest::Client,
) -> GoogleDriveResult<TokenSet> {
    run_against(
        client_id,
        client_secret,
        GOOGLE_AUTH_URL,
        GOOGLE_TOKEN_URL,
        http,
    )
    .await
}

/// Variant that accepts auth + token URLs explicitly — used by
/// tests against a mockito server.
pub async fn run_against(
    client_id: &str,
    client_secret: &str,
    auth_url: &str,
    token_url: &str,
    http: &reqwest::Client,
) -> GoogleDriveResult<TokenSet> {
    if client_id.trim().is_empty() {
        return Err(GoogleDriveError::Config(
            "client_id must not be empty".into(),
        ));
    }
    if client_secret.trim().is_empty() {
        return Err(GoogleDriveError::Config(
            "client_secret must not be empty — Google's token endpoint \
             rejects requests without it"
                .into(),
        ));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let authz = authorize(client_id, &redirect_uri, auth_url)?;
    debug!(url = %authz.authorize_url, "opening Google Drive consent screen");
    if let Err(e) = open::that(authz.authorize_url.as_str()) {
        warn!(
            ?e,
            "failed to launch browser; user must copy the URL manually"
        );
    }

    let (code, returned_state) = wait_for_redirect(listener).await?;
    if returned_state != authz.state {
        return Err(GoogleDriveError::Csrf);
    }

    exchange_code(
        http,
        token_url,
        client_id,
        client_secret,
        &code,
        &authz.pkce_verifier,
        &redirect_uri,
    )
    .await
}

/// Use a refresh token to mint a fresh access token. Called
/// by the API layer on cache miss or 401.
pub async fn refresh(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
    http: &reqwest::Client,
) -> GoogleDriveResult<TokenSet> {
    let now = Utc::now();
    let body = serde_urlencoded::to_string(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ])
    .map_err(|e| GoogleDriveError::Protocol(format!("encode refresh body: {e}")))?;
    let response = http
        .post(GOOGLE_TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    parse_token_response(response, now).await
}

// ── Internal helpers ─────────────────────────────────────────────

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

fn build_auth_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> GoogleDriveResult<url::Url> {
    let mut url = url::Url::parse(auth_url)
        .map_err(|e| GoogleDriveError::Config(format!("invalid auth_url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE_DRIVE_FILE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        // `access_type=offline` makes Google issue a refresh
        // token. Without it, only an access token comes back
        // and the adapter can't keep the connection alive past
        // the first hour.
        .append_pair("access_type", "offline")
        // `prompt=consent` forces the consent screen even on
        // re-auth so the refresh token always shows up — Google
        // omits it after the first grant otherwise.
        .append_pair("prompt", "consent");
    Ok(url)
}

async fn wait_for_redirect(listener: TcpListener) -> GoogleDriveResult<(String, String)> {
    let accept = tokio::time::timeout(AUTH_TIMEOUT, listener.accept()).await;
    let (socket, _peer) = match accept {
        Err(_) => return Err(GoogleDriveError::AuthTimeout),
        Ok(Err(e)) => return Err(GoogleDriveError::Io(e.to_string())),
        Ok(Ok(pair)) => pair,
    };

    let mut reader = BufReader::new(socket);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
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
        .ok_or_else(|| GoogleDriveError::Protocol("malformed request line".into()))?;
    let pseudo = url::Url::parse(&format!("http://127.0.0.1{path}"))
        .map_err(|e| GoogleDriveError::Protocol(format!("query parse: {e}")))?;

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
        return Err(GoogleDriveError::AuthDenied(err));
    }
    Ok((
        code.ok_or_else(|| GoogleDriveError::Protocol("redirect missing `code`".into()))?,
        state.ok_or_else(|| GoogleDriveError::Protocol("redirect missing `state`".into()))?,
    ))
}

/// The **exchange** phase: POST the authorization `code` + the PKCE `verifier`
/// (from [`authorize`]) to the token endpoint and parse the [`TokenSet`].
/// `redirect_uri` must match the one used at authorize; the caller validates the
/// CSRF `state` (returned vs. issued) before calling. Google requires a
/// non-empty `client_secret` here.
pub async fn exchange_code(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> GoogleDriveResult<TokenSet> {
    let now = Utc::now();
    let body = serde_urlencoded::to_string(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
    ])
    .map_err(|e| GoogleDriveError::Protocol(format!("encode token body: {e}")))?;
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
) -> GoogleDriveResult<TokenSet> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(GoogleDriveError::Http {
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
    }
    let raw: Raw = serde_json::from_str(&text)
        .map_err(|e| GoogleDriveError::Protocol(format!("token json: {e}")))?;
    Ok(TokenSet {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        expires_at: requested_at + chrono::Duration::seconds((raw.expires_in - 30).max(0)),
    })
}

/// Mini in-house urlencoded body builder. Same helper as in
/// cal-adapter-google + sync-adapter-dropbox; avoids pulling
/// in `serde_urlencoded` as a direct dependency for one call.
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
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    out.push(byte as char)
                }
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
        assert_eq!(verifier.len(), 43);
        assert_eq!(challenge.len(), 43);
    }

    #[test]
    fn build_auth_url_includes_drive_scope() {
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
        assert!(s.contains("scope=https"));
        assert!(s.contains("drive.file"));
        assert!(s.contains("code_challenge_method=S256"));
        assert!(s.contains("access_type=offline"));
        assert!(s.contains("prompt=consent"));
    }

    #[test]
    fn authorize_builds_url_with_pkce_and_state() {
        let authz = authorize("client-x", "aperio://oauth-callback", GOOGLE_AUTH_URL).unwrap();
        assert_eq!(authz.pkce_verifier.len(), 43);
        assert_eq!(authz.state.len(), 32);
        let url = url::Url::parse(&authz.authorize_url).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            pairs.get("redirect_uri").map(String::as_str),
            Some("aperio://oauth-callback"),
        );
        assert!(pairs
            .get("scope")
            .map(|s| s.contains("drive.file"))
            .unwrap_or(false));
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(authz.pkce_verifier.as_bytes()));
        assert_eq!(
            pairs.get("code_challenge").map(String::as_str),
            Some(expected.as_str())
        );
    }

    #[test]
    fn authorize_rejects_an_empty_client_id() {
        assert!(matches!(
            authorize("", "aperio://oauth-callback", GOOGLE_AUTH_URL),
            Err(GoogleDriveError::Config(_)),
        ));
    }
}
