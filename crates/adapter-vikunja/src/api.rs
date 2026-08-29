//! Vikunja REST client — `<server>/api/v1/...` or `/api/v2/...`.
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

/// Path prefixes under the server root where Vikunja's REST surface
/// lives. Both Vikunja Cloud and self-hosted installs use the same
/// prefixes; v2 shipped with Vikunja 2.4.0 (v1 is frozen there,
/// deprecated in 3.0 and removed in 4.0).
const API_V1_PATH: &str = "/api/v1";
const API_V2_PATH: &str = "/api/v2";

/// Which REST surface a server offers. Detected once per client (see
/// [`VikunjaClient::version`]) and pinned for its lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiVersion {
    V1,
    V2,
}

impl ApiVersion {
    fn path(self) -> &'static str {
        match self {
            ApiVersion::V1 => API_V1_PATH,
            ApiVersion::V2 => API_V2_PATH,
        }
    }
}

/// Deserialize a JSON `null` as the type's default. Go marshals a nil
/// slice as `null`, and the v2 spec types every list-ish field —
/// `items` here, a task's `buckets`/`related_tasks` in `tasks.rs` — as
/// array-or-null, so an empty result may well arrive as `null`.
/// `#[serde(default)]` alone only covers an ABSENT key, not an explicit
/// null.
pub(crate) fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    Ok(<Option<T> as serde::Deserialize>::deserialize(deserializer)?.unwrap_or_default())
}

/// One page of a v2 list response. v1 returns a bare array with the
/// pagination hidden in response headers; v2 wraps it in this envelope.
#[derive(Debug, serde::Deserialize)]
#[serde(bound(deserialize = "T: serde::Deserialize<'de>"))]
struct PageEnvelope<T> {
    #[serde(default, deserialize_with = "null_as_default")]
    items: Vec<T>,
    #[serde(default)]
    total_pages: Option<u32>,
}

/// Shared transport state. The token is treated as opaque (Vikunja
/// emits JWT-shaped strings for its own login API and free-form
/// strings for user-minted API tokens — we don't peek inside).
#[derive(Debug, Clone)]
pub struct VikunjaClient {
    /// Server root, normalised to the form `https://host[:port]`
    /// (no trailing slash, no `/api/v{1,2}` suffix).
    server_root: String,
    /// User-supplied API token. Sent as a Bearer credential.
    token: String,
    http: reqwest::Client,
    /// The detected (or test-pinned) API surface. Shared across clones so
    /// one probe serves every copy of this client; concurrent first calls
    /// dedupe on the cell, and a failed probe leaves it EMPTY so the next
    /// call retries instead of living with a guess.
    version: std::sync::Arc<tokio::sync::OnceCell<ApiVersion>>,
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
            version: std::sync::Arc::new(tokio::sync::OnceCell::new()),
        })
    }

    /// A client pinned to one API surface — no probe will run. Production
    /// uses [`Self::new`] and detects; the test suites pin, so the v1 suite
    /// keeps its `/api/v1` mocks verbatim and the v2 suite mocks `/api/v2`.
    pub fn with_api_version(
        server_url: &str,
        token: String,
        http: reqwest::Client,
        version: ApiVersion,
    ) -> VikunjaResult<Self> {
        let client = Self::new(server_url, token, http)?;
        let _ = client.version.set(version);
        Ok(client)
    }

    /// The server's API surface, probed once and pinned. Concurrent first
    /// calls share ONE probe (the async `OnceCell` dedupes them).
    ///
    /// The probe asks `/api/v2/projects?page=1&per_page=1` and pins v2
    /// only on positive evidence that Vikunja's v2 router answered: the
    /// router always speaks JSON — the list envelope, or a problem+json
    /// error for a bad token — while the things that stand in FRONT of a
    /// server (SPA fallbacks, auth gates, WAFs) answer HTML or nothing.
    /// 404/405 (no such route) mean a pre-2.4 server: v1. Transient
    /// answers (5xx from a restarting backend, 408/429) and transport
    /// errors propagate WITHOUT pinning, so a v1 account whose first
    /// request raced a proxy hiccup isn't dead for the whole session —
    /// the next call re-probes. Anything else that proves a server
    /// answered without proving v2 pins v1: of the two possible wrong
    /// pins, v1-on-a-v2-server fails with clear 404s and heals on
    /// restart, while v2-on-a-v1-server fails with confusing decode
    /// errors everywhere.
    pub async fn version(&self) -> VikunjaResult<ApiVersion> {
        self.version
            .get_or_try_init(|| self.probe_version())
            .await
            .copied()
    }

    async fn probe_version(&self) -> VikunjaResult<ApiVersion> {
        let url = format!(
            "{}{}/projects?page=1&per_page=1",
            self.server_root, API_V2_PATH
        );
        debug!(%url, "vikunja api-version probe");
        let response = self
            .http
            .get(&url)
            .headers(self.auth_headers()?)
            .send()
            .await?;
        let status = response.status().as_u16();
        if matches!(status, 404 | 405) {
            return Ok(ApiVersion::V1);
        }
        if matches!(status, 408 | 429) || status >= 500 {
            let text = response.text().await.unwrap_or_default();
            return Err(VikunjaError::Http {
                status,
                message: trim_message(&text),
            });
        }
        // A body that dies mid-read is a transport flake like any other:
        // propagate (`?`) rather than defaulting to an empty string,
        // which would fail the JSON check below and PIN v1 on what might
        // be a v2 server.
        let text = response.text().await?;
        if serde_json::from_str::<serde_json::Value>(&text).is_ok() {
            Ok(ApiVersion::V2)
        } else {
            Ok(ApiVersion::V1)
        }
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

    /// CREATE a resource — the verb swap in one place: v1's unusual
    /// "`PUT` creates" becomes v2's `POST`.
    pub async fn create_json<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> VikunjaResult<T> {
        let method = match self.version().await? {
            ApiVersion::V1 => Method::PUT,
            ApiVersion::V2 => Method::POST,
        };
        let response = self.send(method, path, Some(body)).await?;
        decode_json(response).await
    }

    /// UPDATE a resource. v1's "`POST` updates" becomes v2's `PATCH`
    /// (JSON Merge Patch) — deliberately PATCH rather than v2's
    /// full-replace `PUT`, because our bodies only carry the fields Aperio
    /// models: a `PUT` would silently wipe the ones it doesn't (reminders,
    /// favourites) that the user maintains in Vikunja itself. Every field
    /// we mean to CLEAR is serialized explicitly (`null` / `0`), which is
    /// exactly merge-patch's removal form.
    pub async fn update_json<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> VikunjaResult<T> {
        let method = match self.version().await? {
            ApiVersion::V1 => Method::POST,
            ApiVersion::V2 => Method::PATCH,
        };
        let response = self.send(method, path, Some(body)).await?;
        decode_json(response).await
    }

    /// One page of a LIST endpoint: the items plus, when the server says
    /// (v2's envelope), the total page count. v1 pages are bare arrays and
    /// report `None` — its walkers keep their "short page = last page"
    /// stop; v2 walkers can trust the count.
    pub async fn get_page<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> VikunjaResult<(Vec<T>, Option<u32>)> {
        match self.version().await? {
            ApiVersion::V1 => {
                let items: Vec<T> = self.get_json(path).await?;
                Ok((items, None))
            }
            ApiVersion::V2 => {
                let envelope: PageEnvelope<T> = self.get_json(path).await?;
                Ok((envelope.items, envelope.total_pages))
            }
        }
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
        let version = self.version().await?;
        let url = format!("{}{}{}", self.server_root, version.path(), path);
        debug!(?method, %url, "vikunja request");
        let mut headers = self.auth_headers()?;
        // v2 dispatches its three PATCH dialects by media type, and plain
        // application/json is not one of them — a conforming server would
        // answer 415. Our PATCH bodies are RFC 7386 merge patches, so say
        // so. (The body is serialized here rather than via
        // `RequestBuilder::json` so no reqwest version can second-guess
        // the content type.)
        if method == Method::PATCH {
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/merge-patch+json"),
            );
        }
        let mut builder = self.http.request(method, &url).headers(headers);
        if let Some(b) = body {
            builder = builder.body(serde_json::to_vec(b)?);
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
    // Pull the human-readable part out of the two error shapes the two
    // surfaces use: v1's `{"code": …, "message": "…"}` and v2's RFC 9457
    // problem+json (`detail`, falling back to `title`). Anything else —
    // HTML from a proxy, plain text — is passed through truncated. 300
    // chars keeps the structure intact while not blowing up logs.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        for key in ["message", "detail", "title"] {
            if let Some(s) = value.get(key).and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    return s.chars().take(300).collect();
                }
            }
        }
    }
    text.chars().take(300).collect()
}

/// Normalise the URL the user typed into a server root we can hang
/// `/api/v1/...` off of:
///
///   - scheme is required (Vikunja-over-HTTP is a thing; HTTPS is
///     the default in practice but we don't impose it)
///   - trailing slashes get stripped
///   - if the user pasted `https://host/api/v1[/]` or
///     `https://host/api/v2[/]` we trim that suffix too (which surface
///     is actually used is DETECTED, not taken from the paste)
///   - deeper paths are rejected — we can't guess what the user meant
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
    if !path.is_empty() && path != API_V1_PATH && path != API_V2_PATH {
        return Err(VikunjaError::Config(format!(
            "server URL must point at the server root, `/api/v1` or `/api/v2`, got path '{path}'",
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

    #[test]
    fn canonicalise_drops_api_v2_suffix() {
        assert_eq!(
            canonicalise_server_url("https://try.vikunja.io/api/v2").unwrap(),
            "https://try.vikunja.io",
        );
        assert_eq!(
            canonicalise_server_url("https://try.vikunja.io/api/v2/").unwrap(),
            "https://try.vikunja.io",
        );
    }

    // ── Error-message extraction ───────────────────────────────

    #[test]
    fn trim_message_reads_both_error_shapes() {
        // v1: `{"code": …, "message": "…"}`.
        assert_eq!(
            trim_message(r#"{"code":4009,"message":"The task relation already exists."}"#),
            "The task relation already exists.",
        );
        // v2: RFC 9457 problem+json — `detail` first, `title` fallback.
        assert_eq!(
            trim_message(
                r#"{"type":"about:blank","title":"Unprocessable Entity","status":422,"detail":"title must not be empty"}"#
            ),
            "title must not be empty",
        );
        assert_eq!(
            trim_message(r#"{"type":"about:blank","title":"Forbidden","status":403}"#),
            "Forbidden",
        );
        // Anything else passes through as-is.
        assert_eq!(
            trim_message("<html>proxy login</html>"),
            "<html>proxy login</html>"
        );
    }

    // ── API-version detection ──────────────────────────────────

    fn probe_client(server_url: &str) -> VikunjaClient {
        VikunjaClient::new(server_url, "test-token".into(), reqwest::Client::new())
            .expect("probe client")
    }

    #[tokio::test]
    async fn version_probe_detects_v2_and_runs_once() {
        let mut server = mockito::Server::new_async().await;
        let probe = server
            .mock("GET", "/api/v2/projects?page=1&per_page=1")
            .with_status(200)
            .with_body(r#"{"items":[],"total":0,"page":1,"per_page":1,"total_pages":0}"#)
            .expect(1)
            .create_async()
            .await;
        let client = probe_client(&server.url());
        assert_eq!(client.version().await.unwrap(), ApiVersion::V2);
        // Second ask must come from the pinned cell, not a second request.
        assert_eq!(client.version().await.unwrap(), ApiVersion::V2);
        probe.assert_async().await;
    }

    #[tokio::test]
    async fn version_probe_falls_back_to_v1_on_404() {
        let mut server = mockito::Server::new_async().await;
        let _probe = server
            .mock("GET", "/api/v2/projects?page=1&per_page=1")
            .with_status(404)
            .with_body("404 page not found")
            .create_async()
            .await;
        let list = server
            .mock("GET", "/api/v1/user")
            .with_status(200)
            .with_body(r#"{"id":1,"username":"toni"}"#)
            .create_async()
            .await;
        let client = probe_client(&server.url());
        assert_eq!(client.version().await.unwrap(), ApiVersion::V1);
        // …and requests are routed under /api/v1 from then on.
        let _: serde_json::Value = client.get_json("/user").await.unwrap();
        list.assert_async().await;
    }

    #[tokio::test]
    async fn version_probe_treats_auth_errors_as_v2() {
        // 401 problem+json comes FROM the v2 router — the surface
        // exists; only the token is bad. Detection must not mistake that
        // for a pre-2.4 server and silently downgrade an authenticated
        // user to v1.
        let mut server = mockito::Server::new_async().await;
        let _probe = server
            .mock("GET", "/api/v2/projects?page=1&per_page=1")
            .with_status(401)
            .with_body(r#"{"type":"about:blank","title":"Unauthorized","status":401}"#)
            .create_async()
            .await;
        let client = probe_client(&server.url());
        assert_eq!(client.version().await.unwrap(), ApiVersion::V2);
    }

    #[tokio::test]
    async fn version_probe_treats_405_as_v1() {
        let mut server = mockito::Server::new_async().await;
        let _probe = server
            .mock("GET", "/api/v2/projects?page=1&per_page=1")
            .with_status(405)
            .with_body("method not allowed")
            .create_async()
            .await;
        let client = probe_client(&server.url());
        assert_eq!(client.version().await.unwrap(), ApiVersion::V1);
    }

    #[tokio::test]
    async fn version_probe_treats_html_answers_as_v1() {
        // An SPA fallback / auth-gate proxy answering 200 HTML for
        // unknown paths did NOT prove the v2 router exists — pinning V2
        // there would turn every request into a decode error on a
        // working v1 account.
        let mut server = mockito::Server::new_async().await;
        let _probe = server
            .mock("GET", "/api/v2/projects?page=1&per_page=1")
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body("<html><body>app shell</body></html>")
            .create_async()
            .await;
        let client = probe_client(&server.url());
        assert_eq!(client.version().await.unwrap(), ApiVersion::V1);
    }

    #[tokio::test]
    async fn version_probe_does_not_pin_on_5xx_and_retries() {
        // A proxy 502 while the backend restarts is transient: the probe
        // must error WITHOUT pinning, so the next call re-probes and
        // finds the real answer.
        let mut server = mockito::Server::new_async().await;
        let bad = server
            .mock("GET", "/api/v2/projects?page=1&per_page=1")
            .with_status(502)
            .with_body("<html>bad gateway</html>")
            .expect(1)
            .create_async()
            .await;
        let client = probe_client(&server.url());
        let err = client.version().await.unwrap_err();
        match err {
            VikunjaError::Http { status: 502, .. } => {}
            other => panic!("expected Http 502, got {other:?}"),
        }
        bad.assert_async().await;
        bad.remove_async().await;
        let _good = server
            .mock("GET", "/api/v2/projects?page=1&per_page=1")
            .with_status(200)
            .with_body(r#"{"items":[],"total":0,"page":1,"per_page":1,"total_pages":0}"#)
            .create_async()
            .await;
        assert_eq!(client.version().await.unwrap(), ApiVersion::V2);
    }
}
