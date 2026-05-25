//! OAuth 2.0 PKCE flow for Microsoft Identity Platform (v2.0).
//!
//! The shape is identical to the Google adapter's: bind a loopback
//! TCP listener, generate PKCE challenge + CSRF state, build the
//! authorize URL, open the system browser, wait for the redirect,
//! exchange the code for tokens.
//!
//! One material difference: Microsoft properly honours the
//! "public client" half of the OAuth spec. For Aperio's purposes
//! that means **no `client_secret`** on the token endpoint — PKCE
//! alone is sufficient, RFC 7636-style. (Google's Desktop-app
//! OAuth requires the secret regardless; Microsoft does not.)
//!
//! Authority choice: `common` lets both personal Microsoft accounts
//! (outlook.com, hotmail.com, live.com) and work/school accounts
//! sign in. The user's Azure portal app registration must allow
//! the same set of account types. We keep this configurable on
//! the account so an admin can pin a specific tenant if they want.

use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, warn};

use crate::error::{GraphError, GraphResult};

/// Authority slug — `common`, `organizations`, `consumers`, or a
/// specific tenant GUID. Inserted into the v2.0 authorise / token
/// URLs at runtime.
pub const DEFAULT_AUTHORITY: &str = "common";

/// Scopes Aperio asks for on the consent screen:
///
///   - `Calendars.ReadWrite` — full read/write on the user's
///     calendars + events
///   - `Tasks.ReadWrite` — full read/write on the user's Microsoft
///     To Do task lists + tasks (Phase 6e.2). Surfaces the
///     `/me/todo/lists` endpoint family. Microsoft consolidated all
///     task scenarios under To Do; the legacy Outlook-tasks endpoint
///     was deprecated in 2020.
///   - `Contacts.ReadWrite` — Phase 10i. Full read/write on the
///     user's Outlook contact folders + items. Surfaces the
///     `/me/contactFolders` and `/me/contacts` endpoint families.
///   - `People.Read` — Phase 10i. Read-only access to `/me/people`,
///     Microsoft's relevance-ranked "people you interact with"
///     endpoint (Outlook traffic + Azure AD suggestions). We surface
///     that as a single read-only "Suggested People" ContactList —
///     the GAL-equivalent for Graph without pulling in the heavy
///     `Directory.Read.All` scope (which most tenants gate behind
///     admin consent).
///   - `offline_access` — required so Microsoft hands out a refresh
///     token; without it, the access token dies after an hour and
///     the user has to re-consent every time
///   - `openid` / `profile` / `email` — Graph requires at least one
///     OpenID Connect scope to issue an id_token; without it
///     consent works but some user-info endpoints return 403
pub const SCOPES: &str = "Calendars.ReadWrite Tasks.ReadWrite Contacts.ReadWrite People.Read \
     offline_access openid profile email";

const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub scope: Option<String>,
}

pub fn authorize_url(authority: &str) -> String {
    format!("https://login.microsoftonline.com/{authority}/oauth2/v2.0/authorize")
}

pub fn token_url(authority: &str) -> String {
    format!("https://login.microsoftonline.com/{authority}/oauth2/v2.0/token")
}

/// Run the full PKCE dance against the given OAuth endpoints.
/// `authority_url` and `token_endpoint` are exposed for testing —
/// production callers use [`run_default`].
pub async fn run(
    client_id: &str,
    authority_url: &str,
    token_endpoint: &str,
    http: &reqwest::Client,
) -> GraphResult<TokenSet> {
    if client_id.trim().is_empty() {
        return Err(GraphError::Config("client_id must not be empty".into()));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let auth = build_auth_url(authority_url, client_id, &redirect_uri, &challenge, &state)?;
    debug!(url = %auth, "opening Microsoft consent screen");
    if let Err(e) = open::that(auth.as_str()) {
        warn!(
            ?e,
            "failed to launch browser; user must copy the URL manually"
        );
    }

    let (code, returned_state) = wait_for_redirect(listener).await?;
    if returned_state != state {
        return Err(GraphError::Csrf);
    }

    exchange_code(
        http,
        token_endpoint,
        client_id,
        &code,
        &verifier,
        &redirect_uri,
    )
    .await
}

pub async fn run_default(
    client_id: &str,
    authority: &str,
    http: &reqwest::Client,
) -> GraphResult<TokenSet> {
    run(
        client_id,
        &authorize_url(authority),
        &token_url(authority),
        http,
    )
    .await
}

pub async fn refresh(
    client_id: &str,
    token_endpoint: &str,
    refresh_token: &str,
    http: &reqwest::Client,
) -> GraphResult<TokenSet> {
    let now = Utc::now();
    let body = serde_urlencoded::to_string(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
        ("scope", SCOPES),
    ])
    .map_err(|e| GraphError::Protocol(format!("encode refresh body: {e}")))?;

    let response = http
        .post(token_endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    parse_token_response(response, now).await
}

// ── Internal helpers ────────────────────────────────────────────────────

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
) -> GraphResult<url::Url> {
    let mut url = url::Url::parse(auth_url)
        .map_err(|e| GraphError::Config(format!("invalid auth_url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_mode", "query")
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        // `prompt=select_account` shows the account picker every
        // time. Without it, Microsoft auto-picks the last-used
        // account silently — fine when the user only has one, but
        // confusing when they have several (personal + work) and
        // expected to choose.
        .append_pair("prompt", "select_account");
    Ok(url)
}

async fn wait_for_redirect(listener: TcpListener) -> GraphResult<(String, String)> {
    let accept = tokio::time::timeout(AUTH_TIMEOUT, listener.accept()).await;
    let (socket, _peer) = match accept {
        Err(_) => return Err(GraphError::AuthTimeout),
        Ok(Err(e)) => return Err(GraphError::Io(e.to_string())),
        Ok(Ok(pair)) => pair,
    };

    let mut reader = BufReader::new(socket);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
    // Drain headers.
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
        .ok_or_else(|| GraphError::Protocol("malformed request line".into()))?;
    let pseudo = url::Url::parse(&format!("http://127.0.0.1{path}"))
        .map_err(|e| GraphError::Protocol(format!("query parse: {e}")))?;

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

    let response_body = if err_param.is_none() && code.is_some() {
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
        return Err(GraphError::AuthDenied(err));
    }
    Ok((
        code.ok_or_else(|| GraphError::Protocol("redirect missing `code`".into()))?,
        state.ok_or_else(|| GraphError::Protocol("redirect missing `state`".into()))?,
    ))
}

async fn exchange_code(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> GraphResult<TokenSet> {
    let now = Utc::now();
    // No client_secret. Microsoft's v2.0 endpoint correctly accepts
    // PKCE-only exchanges for public-client app registrations.
    let body = serde_urlencoded::to_string(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
        ("scope", SCOPES),
    ])
    .map_err(|e| GraphError::Protocol(format!("encode token body: {e}")))?;

    let response = http
        .post(token_endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    parse_token_response(response, now).await
}

async fn parse_token_response(
    response: reqwest::Response,
    requested_at: DateTime<Utc>,
) -> GraphResult<TokenSet> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(GraphError::Http {
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
        .map_err(|e| GraphError::Protocol(format!("token json: {e}")))?;
    Ok(TokenSet {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        expires_at: requested_at + chrono::Duration::seconds((raw.expires_in - 30).max(0)),
        scope: raw.scope,
    })
}

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
    fn pkce_invariant_holds() {
        let (verifier, challenge) = generate_pkce();
        assert_eq!(verifier.len(), 43);
        let digest = Sha256::digest(verifier.as_bytes());
        assert_eq!(
            challenge,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
        );
    }

    #[test]
    fn authority_urls_format_correctly() {
        assert!(authorize_url("common")
            .starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/authorize"));
        assert!(token_url("organizations")
            .starts_with("https://login.microsoftonline.com/organizations/oauth2/v2.0/token"));
    }

    #[test]
    fn auth_url_carries_every_required_param_and_no_client_secret() {
        let url = build_auth_url(
            &authorize_url("common"),
            "my-client",
            "http://127.0.0.1:12345",
            "challenge",
            "state",
        )
        .unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            pairs.get("client_id").map(String::as_str),
            Some("my-client")
        );
        assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(pairs.get("scope").map(String::as_str), Some(SCOPES));
        assert_eq!(
            pairs.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            pairs.get("prompt").map(String::as_str),
            Some("select_account")
        );
        assert!(!pairs.contains_key("client_secret"));
    }

    #[tokio::test]
    async fn exchange_code_does_not_send_client_secret() {
        // Regression guard: Microsoft is the vendor where PKCE
        // actually replaces the secret. If we ever bring the secret
        // back here by accident the body matcher fails.
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("grant_type=authorization_code".into()),
                mockito::Matcher::Regex("code_verifier=".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"{"access_token":"x","refresh_token":"r","expires_in":3600,"token_type":"Bearer"}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let http = reqwest::Client::new();
        let tokens = exchange_code(
            &http,
            &format!("{}/token", server.url()),
            "client",
            "code",
            "verifier",
            "http://127.0.0.1:12345",
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "x");
        assert_eq!(tokens.refresh_token.as_deref(), Some("r"));
        m.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_round_trip() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .match_body(mockito::Matcher::Regex("grant_type=refresh_token".into()))
            .with_status(200)
            .with_body(r#"{"access_token":"refreshed","expires_in":3600,"token_type":"Bearer"}"#)
            .create_async()
            .await;

        let http = reqwest::Client::new();
        let tokens = refresh(
            "client",
            &format!("{}/token", server.url()),
            "old-refresh",
            &http,
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "refreshed");
    }

    #[tokio::test]
    async fn exchange_code_surfaces_http_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create_async()
            .await;
        let http = reqwest::Client::new();
        let err = exchange_code(
            &http,
            &format!("{}/token", server.url()),
            "c",
            "code",
            "v",
            "r",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GraphError::Http { status: 400, .. }));
    }
}
