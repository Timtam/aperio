//! Vikunja REST client — `<server>/api/v1/...`.
//!
//! Vikunja is a self-hosted Open-Source task manager (AGPLv3). Its
//! REST API is straightforward Bearer-token JSON-over-HTTP — no
//! discovery, no auth dance, no token refresh. The user mints an API
//! token in their Vikunja UI (Settings → API Tokens), pastes it into
//! Aperio's account dialog, and Aperio sends it as
//! `Authorization: Bearer <token>` on every request.
//!
//! This module owns the transport + helpers; the actual task list /
//! task CRUD lives in `tasks.rs` and uses these helpers to talk to
//! Vikunja.

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, Response};
use serde::Serialize;
use tracing::debug;

use crate::error::{VikunjaError, VikunjaResult};

/// Path under the server root where Vikunja's REST surface lives.
/// Both Vikunja Cloud and self-hosted installs use the same prefix.
const API_PATH: &str = "/api/v1";

/// Shared transport state. The token is treated as opaque (Vikunja
/// emits JWT-shaped strings for its own login API and free-form
/// strings for user-minted API tokens — we don't peek inside).
#[derive(Debug, Clone)]
pub struct VikunjaClient {
    /// Server root, normalised to the form `https://host[:port]`
    /// (no trailing slash, no `/api/v1` suffix).
    server_root: String,
    /// User-supplied API token. Sent as a Bearer credential.
    token: String,
    http: reqwest::Client,
}

impl VikunjaClient {
    /// Build a client from a server URL + token. The URL is
    /// canonicalised so the caller can paste `https://vikunja.example.org`
    /// or `https://vikunja.example.org/api/v1/` or any of the
    /// variations in between and get a working client.
    pub fn new(server_url: &str, token: String, http: reqwest::Client) -> VikunjaResult<Self> {
        Ok(Self {
            server_root: canonicalise_server_url(server_url)?,
            token,
            http,
        })
    }

    /// Read the canonicalised server root back out. Useful for
    /// `Debug` output / tracing the `account_label` from a
    /// `connect_vikunja_account` command.
    pub fn server_root(&self) -> &str {
        &self.server_root
    }

    /// Build the auth + content-type headers used on every request.
    /// Kept private — callers go through `send_*` instead.
    fn auth_headers(&self) -> VikunjaResult<HeaderMap> {
        let mut headers = HeaderMap::new();
        let value = format!("Bearer {}", self.token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&value)
                .map_err(|e| VikunjaError::Config(format!("auth header: {e}")))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    /// `GET /api/v1{path}` returning the raw response so callers can
    /// decide between "parse JSON" and "check status only" (the
    /// `/user` token-test endpoint uses the latter).
    pub async fn get(&self, path: &str) -> VikunjaResult<Response> {
        self.send(Method::GET, path, Option::<&()>::None).await
    }

    /// `GET /api/v1{path}` and decode the JSON body. Returns
    /// `VikunjaError::Http` on non-2xx with the response body
    /// trimmed to 300 chars (enough to spot a Vikunja error
    /// envelope without overwhelming logs).
    pub async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> VikunjaResult<T> {
        let response = self.send(Method::GET, path, Option::<&()>::None).await?;
        decode_json(response).await
    }

    /// `POST /api/v1{path}` with a JSON body. Vikunja conventions:
    ///
    ///   - `POST /tasks/{id}` updates an existing task
    ///   - `POST /projects/{id}` updates an existing project
    ///   - `PUT  /projects/{id}/tasks` creates a new task
    ///
    /// We don't try to enforce that here; callers pick the right
    /// HTTP verb.
    pub async fn post_json<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> VikunjaResult<T> {
        let response = self.send(Method::POST, path, Some(body)).await?;
        decode_json(response).await
    }

    /// `PUT /api/v1{path}` with a JSON body.
    pub async fn put_json<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> VikunjaResult<T> {
        let response = self.send(Method::PUT, path, Some(body)).await?;
        decode_json(response).await
    }

    /// `DELETE /api/v1{path}`. Returns `Ok(())` on any 2xx (Vikunja
    /// returns 200 + `{"message": "Successfully deleted."}` for the
    /// task delete endpoint).
    pub async fn delete(&self, path: &str) -> VikunjaResult<()> {
        let response = self.send(Method::DELETE, path, Option::<&()>::None).await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(VikunjaError::Http {
            status: status.as_u16(),
            message: trim_message(&text),
        })
    }

    /// Underlying request driver. Kept private so the
    /// `Authorization` header logic lives in one place.
    async fn send<B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> VikunjaResult<Response> {
        let url = format!("{}{}{}", self.server_root, API_PATH, path);
        debug!(?method, %url, "vikunja request");
        let mut builder = self
            .http
            .request(method, &url)
            .headers(self.auth_headers()?);
        if let Some(b) = body {
            builder = builder.json(b);
        }
        Ok(builder.send().await?)
    }
}

/// Decode a 2xx JSON body or surface a non-2xx as
/// `VikunjaError::Http`.
pub(crate) async fn decode_json<T: serde::de::DeserializeOwned>(
    response: Response,
) -> VikunjaResult<T> {
    let status = response.status();
    if status.is_success() {
        let text = response.text().await?;
        return serde_json::from_str(&text).map_err(|e| {
            VikunjaError::Protocol(format!(
                "json decode failed: {e} — body started with: {}",
                text.chars().take(120).collect::<String>(),
            ))
        });
    }
    let text = response.text().await.unwrap_or_default();
    Err(VikunjaError::Http {
        status: status.as_u16(),
        message: trim_message(&text),
    })
}

fn trim_message(text: &str) -> String {
    // Vikunja error responses are usually short JSON envelopes; 300
    // chars is enough to keep the structure intact while not blowing
    // up logs.
    text.chars().take(300).collect()
}

/// Normalise the URL the user typed into a server root we can hang
/// `/api/v1/...` off of:
///
///   - scheme is required (Vikunja-over-HTTP is a thing; HTTPS is
///     the default in practice but we don't impose it)
///   - trailing slashes get stripped
///   - if the user pasted `https://host/api/v1` or
///     `https://host/api/v1/` we trim that suffix too
///   - paths beyond `/api/v1/...` are rejected — we can't guess what
///     the user meant
fn canonicalise_server_url(input: &str) -> VikunjaResult<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(VikunjaError::Config("server URL must not be empty".into()));
    }
    let parsed =
        url::Url::parse(trimmed).map_err(|e| VikunjaError::Config(format!("server URL: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(VikunjaError::Config(format!(
            "server URL scheme must be http(s), got '{}'",
            parsed.scheme(),
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| VikunjaError::Config("server URL must have a host".into()))?;
    let mut root = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        root.push(':');
        root.push_str(&port.to_string());
    }
    // Allow `https://host/api/v1[/]` or bare host; reject deeper paths
    // (the user probably pasted a UI URL by accident — better to fail
    // loudly than to silently mis-route every request).
    let path = parsed.path().trim_end_matches('/');
    if !path.is_empty() && path != "/api/v1" && path != API_PATH {
        return Err(VikunjaError::Config(format!(
            "server URL must point at the server root or `/api/v1`, got path '{path}'",
        )));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalise_strips_trailing_slash() {
        assert_eq!(
            canonicalise_server_url("https://try.vikunja.io/").unwrap(),
            "https://try.vikunja.io",
        );
    }

    #[test]
    fn canonicalise_drops_api_v1_suffix() {
        assert_eq!(
            canonicalise_server_url("https://try.vikunja.io/api/v1").unwrap(),
            "https://try.vikunja.io",
        );
        assert_eq!(
            canonicalise_server_url("https://try.vikunja.io/api/v1/").unwrap(),
            "https://try.vikunja.io",
        );
    }

    #[test]
    fn canonicalise_keeps_non_default_port() {
        assert_eq!(
            canonicalise_server_url("http://localhost:3456").unwrap(),
            "http://localhost:3456",
        );
    }

    #[test]
    fn canonicalise_rejects_non_http_scheme() {
        let err = canonicalise_server_url("ftp://example.org").unwrap_err();
        match err {
            VikunjaError::Config(msg) => assert!(msg.contains("scheme")),
            other => panic!("expected Config, got {other:?}"),
        }
    }

    #[test]
    fn canonicalise_rejects_deep_path() {
        let err = canonicalise_server_url("https://vikunja.example.org/some/page").unwrap_err();
        match err {
            VikunjaError::Config(msg) => assert!(msg.contains("path")),
            other => panic!("expected Config, got {other:?}"),
        }
    }
}
