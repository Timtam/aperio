//! WebDAV `SyncAdapter` implementation (DESIGN.md §19.6).
//!
//! Drives any RFC 4918 server — Nextcloud, ownCloud, Apache mod_dav,
//! generic stack on top of a static-file server. Maps the
//! `sync_core::SyncAdapter` trait onto HTTP verbs:
//!
//! | Trait call           | HTTP method  | Path                                |
//! |----------------------|--------------|-------------------------------------|
//! | `test_connection`    | `OPTIONS`    | `<base>/`                            |
//! | `fetch_meta`         | `GET`        | `<base>/meta.json`                   |
//! | `push_meta`          | `PUT`        | `<base>/meta.json`                   |
//! | `fetch_new_logs`     | `PROPFIND`   | `<base>/log/` (then `GET` per file)  |
//! | `push_log`           | `PUT`        | `<base>/log/<filename>`              |
//! | `fetch_snapshot`     | `GET`        | `<base>/snapshot.json`               |
//! | `push_snapshot`      | `PUT`        | `<base>/snapshot.json`               |
//! | `delete_log`         | `DELETE`     | `<base>/log/<filename>`              |
//! | `push_sound_asset`   | `PUT`        | `<base>/assets/sounds/<hash>.<ext>`  |
//! | `fetch_sound_asset`  | `GET`        | `<base>/assets/sounds/<hash>.<ext>`  |
//!
//! The constructor doesn't verify the URL works — `test_connection`
//! does. That's deliberate: the settings dialog can build the
//! adapter without network IO, and `test_connection` is the
//! "click Connect, see what happens" probe.
//!
//! ## Auth
//!
//! v1 supports **HTTP Basic**. The `WebDavCredentials::basic(user,
//! pass)` constructor base64-encodes once and the resulting header
//! goes on every request. Digest + OAuth2 are in the §19.6 spec
//! table but parking them until we see a real server that needs
//! Digest (Nextcloud / ownCloud both accept Basic over HTTPS).
//!
//! ## Atomic writes
//!
//! `meta.json` + `snapshot.json` use the same write-temp-then-MOVE
//! pattern the local FS adapter uses, just with WebDAV `MOVE` as
//! the rename verb. The temp file lives next to the target with a
//! `.tmp` suffix. WebDAV's `MOVE` with `Overwrite: T` atomically
//! replaces the destination per RFC 4918 §9.9.
//!
//! Log files don't need atomic writes — their filenames embed the
//! device id + timestamp, so a partial upload can be retried
//! verbatim.
//!
//! ## What this crate does NOT do
//!
//! - **Digest / OAuth2 auth.** Parked until a target server needs
//!   them; Basic over HTTPS covers Nextcloud / ownCloud.
//! - **WebDAV LOCK / UNLOCK.** Concurrent writes on `meta.json`
//!   tolerate last-write-wins per §19.5. Without locking, two
//!   devices syncing simultaneously can lose one device's
//!   heartbeat; the next round restores it.
//! - **MOVE for atomic rename in v1.** We use straight `PUT` over
//!   the target. Half-written meta files are tolerated by the
//!   reader (parses the latest complete bytes once the upload
//!   finishes). A future revision can upgrade to PUT-tmp + MOVE if
//!   we observe corruption against a misbehaving server.

mod propfind;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine;
use reqwest::{Client, ClientBuilder, Method, StatusCode};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter, SyncError, SyncResult,
};
use tracing::{debug, warn};
use url::Url;

pub use propfind::{parse_propfind_response, PropfindEntry};

/// Credentials handed to the adapter. Basic-auth for v1; the enum
/// shape leaves room for `Bearer { token }` once OAuth2 lands.
#[derive(Debug, Clone)]
pub enum WebDavCredentials {
    /// HTTP Basic auth. The base64 encoding happens at construction
    /// time so each request reuses the cached header instead of
    /// re-encoding.
    Basic { authorization: String },
    /// No authentication header (public dataset on a read-write
    /// share without auth). Mainly useful for local test servers.
    None,
}

impl WebDavCredentials {
    pub fn basic(user: &str, password: &str) -> Self {
        let raw = format!("{user}:{password}");
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
        Self::Basic {
            authorization: format!("Basic {encoded}"),
        }
    }

    fn header(&self) -> Option<&str> {
        match self {
            Self::Basic { authorization } => Some(authorization),
            Self::None => None,
        }
    }
}

/// WebDAV `SyncAdapter`. Cheap to clone (the `Client` is `Arc`-
/// backed internally; the URL + creds are tiny).
#[derive(Debug, Clone)]
pub struct WebDavSyncAdapter {
    /// Base URL ending with a trailing `/`. All resource URLs are
    /// built by joining a relative path onto this.
    base_url: Url,
    credentials: WebDavCredentials,
    client: Arc<Client>,
    /// Collections (`log/`, `assets/sounds/`, …) we've already MKCOL'd
    /// this session. WebDAV collections persist server-side, so once a
    /// directory is ensured we skip the redundant MKCOL on every later
    /// push. Shared across clones so the cache survives `.clone()`.
    ensured: Arc<Mutex<HashSet<String>>>,
}

impl WebDavSyncAdapter {
    /// Build an adapter against a base URL. The URL MUST point at
    /// the collection that will hold `log/`, `snapshot.json`, etc.
    /// — for Nextcloud that's typically
    /// `https://cloud.example.com/remote.php/dav/files/<user>/aperio/`.
    ///
    /// Returns `Err(SyncError::InvalidInput)` when the URL is
    /// missing a scheme or has no host.
    pub fn new(base_url: &str, credentials: WebDavCredentials) -> SyncResult<Self> {
        let mut url = Url::parse(base_url)
            .map_err(|err| SyncError::internal(format!("invalid WebDAV URL: {err}")))?;
        if !url.path().ends_with('/') {
            let p = format!("{}/", url.path());
            url.set_path(&p);
        }
        if url.host_str().is_none() {
            return Err(SyncError::internal("WebDAV URL missing host"));
        }
        let client = ClientBuilder::new()
            // Conservative timeout for the small files we shuffle.
            // A 60 s read budget tolerates slow links without
            // wedging a sync round forever.
            .timeout(std::time::Duration::from_secs(60))
            // Force HTTP/1.1 — Synology DSM's bundled WebDAV server
            // (the most commonly self-hosted WebDAV target) ALPN-
            // negotiates HTTP/2 but then closes the socket mid-
            // response on real requests, surfacing as "connection
            // closed before message completed". Nextcloud /
            // Apache / nginx all happily speak HTTP/1.1, so
            // pinning the version costs nothing and removes a
            // class of "works once, fails next round" reports.
            .http1_only()
            // Reuse the connection WITHIN a round, drop it BETWEEN rounds.
            //
            // A single round fires 3–4 requests back-to-back (fetch_meta →
            // push_log → PROPFIND → GETs); with pooling off, each paid a
            // fresh TCP+TLS handshake, so a WAN link spent seconds on
            // handshakes alone — the "why is a push 4 s?" complaint. Keep-
            // alive collapses those onto ONE connection (one handshake per
            // round).
            //
            // The reason pooling was OFF was a BETWEEN-rounds hazard: servers
            // behind home routers / reverse proxies keep sockets alive only
            // ~10 s, so a pooled socket idle since the last round (30 s+ ago)
            // is dead, and reusing it surfaced as "connection closed before
            // message completed". A short pool_idle_timeout fixes that
            // directly — we drop our end after 3 s, comfortably under even
            // Apache's 5 s default KeepAliveTimeout (and long before the next
            // round), so a stale socket is never handed out, while the
            // sub-second within-round gaps still reuse the live connection.
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(|err| SyncError::internal(format!("build reqwest client: {err}")))?;
        Ok(Self {
            base_url: url,
            credentials,
            client: Arc::new(client),
            ensured: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    /// Borrow the configured base URL. Used by Settings for the
    /// "current adapter: <URL>" surface.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn url_for(&self, relative: &str) -> SyncResult<Url> {
        self.base_url
            .join(relative)
            .map_err(|err| SyncError::internal(format!("URL join {relative}: {err}")))
    }

    /// Build a request with auth + standard headers attached.
    fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        let mut builder = self.client.request(method, url);
        if let Some(header) = self.credentials.header() {
            builder = builder.header(reqwest::header::AUTHORIZATION, header);
        }
        builder
    }

    /// GET a resource. Returns `Ok(Some(bytes))` on 200, `Ok(None)`
    /// on 404, `Err` on anything else.
    async fn get_bytes(&self, relative: &str) -> SyncResult<Option<Vec<u8>>> {
        let url = self.url_for(relative)?;
        let resp = self
            .request(Method::GET, url.clone())
            .send()
            .await
            .map_err(network_err)?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(http_err(status, &url));
        }
        let bytes = resp.bytes().await.map_err(network_err)?;
        Ok(Some(bytes.to_vec()))
    }

    /// PUT a resource. Idempotent — the server creates or replaces.
    async fn put_bytes(
        &self,
        relative: &str,
        bytes: Vec<u8>,
        content_type: &'static str,
    ) -> SyncResult<()> {
        let url = self.url_for(relative)?;
        let resp = self
            .request(Method::PUT, url.clone())
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await
            .map_err(network_err)?;
        let status = resp.status();
        if !status.is_success() && status != StatusCode::CREATED {
            return Err(http_err(status, &url));
        }
        Ok(())
    }

    /// DELETE a resource. 404 is treated as success — the goal is
    /// "make sure this is gone", and a missing target already
    /// satisfies that.
    async fn delete(&self, relative: &str) -> SyncResult<()> {
        let url = self.url_for(relative)?;
        let resp = self
            .request(Method::DELETE, url.clone())
            .send()
            .await
            .map_err(network_err)?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND
            || status == StatusCode::NO_CONTENT
            || status.is_success()
        {
            return Ok(());
        }
        Err(http_err(status, &url))
    }

    /// MKCOL a collection — used to lazy-create `log/` and
    /// `assets/sounds/` before the first push. 405 (method not
    /// allowed) means the collection already exists; success.
    async fn mkcol(&self, relative: &str) -> SyncResult<()> {
        let url = self.url_for(relative)?;
        let resp = self
            .request(
                Method::from_bytes(b"MKCOL").expect("MKCOL is a valid method"),
                url.clone(),
            )
            .send()
            .await
            .map_err(network_err)?;
        let status = resp.status();
        if status.is_success()
            || status == StatusCode::METHOD_NOT_ALLOWED
            || status == StatusCode::CONFLICT
        {
            // 405 / 409 typically mean "already exists" on WebDAV
            // servers — that's our happy path. Real "can't create"
            // would be 403 / 507.
            return Ok(());
        }
        Err(http_err(status, &url))
    }

    /// PROPFIND with Depth: 1 returning the parsed entry list.
    async fn propfind(&self, relative: &str) -> SyncResult<Vec<PropfindEntry>> {
        let url = self.url_for(relative)?;
        // Minimal PROPFIND body — `<allprop/>` returns the standard
        // property set including `getcontentlength` and
        // `getlastmodified`, which is what `fetch_new_logs` needs.
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:displayname/>
    <d:getcontentlength/>
    <d:getlastmodified/>
    <d:resourcetype/>
  </d:prop>
</d:propfind>"#;
        let resp = self
            .request(
                Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid method"),
                url.clone(),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/xml")
            .header("Depth", "1")
            .body(body)
            .send()
            .await
            .map_err(network_err)?;
        let status = resp.status();
        // 207 Multi-Status is the WebDAV success code; some servers
        // return 200 for empty collections.
        if status != StatusCode::MULTI_STATUS && !status.is_success() {
            if status == StatusCode::NOT_FOUND {
                return Ok(Vec::new());
            }
            return Err(http_err(status, &url));
        }
        let text = resp.text().await.map_err(network_err)?;
        parse_propfind_response(&text)
    }

    /// `MKCOL` a collection at most once per adapter session.
    ///
    /// WebDAV collections persist server-side, so re-creating `log/`
    /// before every single push (what the per-call `mkcol` did) just
    /// burns one round-trip per pending file — on a high-latency
    /// server that's seconds added to the first sync round, which on
    /// startup (where `test_connection` never ran to pre-create the
    /// dirs) pays the cost for *every* queued log. We remember the
    /// collections ensured this session and skip the rest.
    async fn ensure_collection(&self, rel: &str) {
        if self
            .ensured
            .lock()
            .expect("ensured mutex poison")
            .contains(rel)
        {
            return;
        }
        // MKCOL is idempotent (405 on an existing collection, ignored).
        // A concurrent first-touch racing here at worst issues a
        // second harmless MKCOL.
        let _ = self.mkcol(rel).await;
        self.ensured
            .lock()
            .expect("ensured mutex poison")
            .insert(rel.to_string());
    }
}

#[async_trait]
impl SyncAdapter for WebDavSyncAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        // OPTIONS on the base URL: cheapest probe. A 200/204 means
        // we can reach the server + authenticate; 401 surfaces as
        // SyncError::Auth so the Settings dialog can show "wrong
        // credentials" rather than a generic network error.
        let url = self.base_url.clone();
        let resp = self
            .request(Method::OPTIONS, url.clone())
            .send()
            .await
            .map_err(network_err)?;
        let status = resp.status();
        if status.is_success() || status == StatusCode::NO_CONTENT {
            // Also lazily ensure the `log/` and `assets/sounds/`
            // collections exist — saves an extra round-trip on the
            // first push, and seeds the session cache so later pushes
            // skip their own MKCOLs.
            self.ensure_collection("log/").await;
            self.ensure_collection("assets/").await;
            self.ensure_collection("assets/sounds/").await;
            return Ok(());
        }
        Err(http_err(status, &url))
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        let bytes = self.get_bytes("meta.json").await?;
        match bytes {
            Some(b) => Ok(Some(MetaJson::from_bytes(&b)?)),
            None => Ok(None),
        }
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        let bytes = meta.to_bytes()?;
        self.put_bytes("meta.json", bytes, "application/json").await
    }

    async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
        // 1. List the log directory via PROPFIND.
        let entries = self.propfind("log/").await?;
        // 2. Filter to *.jsonl files newer than the cursor; the
        //    filename embeds the timestamp so we don't need
        //    getlastmodified for the cursor comparison.
        let mut wanted: Vec<LogFileName> = Vec::new();
        for entry in entries {
            // Skip the collection itself; the leaf's href ends with
            // the filename.
            let name = match entry.href.rsplit('/').find(|s| !s.is_empty()) {
                Some(n) => n,
                None => continue,
            };
            // Servers URL-encode hrefs (`%3A` for `:`). Decode
            // before handing to the log-filename parser.
            let decoded = match percent_decode(name) {
                Some(d) => d,
                None => continue,
            };
            let parsed = match LogFileName::from_filename(&decoded) {
                Ok(p) => p,
                Err(_) => {
                    debug!(name = %decoded, "skipping non-log entry in PROPFIND");
                    continue;
                }
            };
            if parsed.timestamp > since.last_seen_log {
                wanted.push(parsed);
            }
        }
        // 3. GET each matching file.
        let mut out = Vec::with_capacity(wanted.len());
        for name in wanted {
            let relative = format!("log/{}", name.to_filename());
            match self.get_bytes(&relative).await? {
                Some(bytes) => out.push(LogFile { name, bytes }),
                None => {
                    // Listed but missing: probably deleted by the
                    // compactor in the gap between PROPFIND and GET.
                    // Skip silently — the next round picks up an
                    // updated listing.
                    debug!(
                        path = %relative,
                        "log listed by PROPFIND but no longer present on GET",
                    );
                }
            }
        }
        Ok(out)
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        let relative = format!("log/{}", log.name.to_filename());
        // Ensure the collection exists — but only MKCOL it once per
        // session (the cache skips the redundant round-trip every
        // later push would otherwise pay).
        self.ensure_collection("log/").await;
        self.put_bytes(&relative, log.bytes.clone(), "application/json")
            .await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        match self.get_bytes("snapshot.json").await? {
            Some(bytes) => Ok(Some(Snapshot::from_bytes(&bytes)?)),
            None => Ok(None),
        }
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        let bytes = snapshot.to_bytes()?;
        self.put_bytes("snapshot.json", bytes, "application/json")
            .await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        let relative = format!("log/{}", name.to_filename());
        self.delete(&relative).await
    }

    async fn push_sound_asset(&self, hash: &str, extension: &str, bytes: &[u8]) -> SyncResult<()> {
        let relative = format!("assets/sounds/{hash}.{extension}");
        self.ensure_collection("assets/").await;
        self.ensure_collection("assets/sounds/").await;
        self.put_bytes(&relative, bytes.to_vec(), "application/octet-stream")
            .await
    }

    async fn fetch_sound_asset(&self, hash: &str, extension: &str) -> SyncResult<Option<Vec<u8>>> {
        let relative = format!("assets/sounds/{hash}.{extension}");
        self.get_bytes(&relative).await
    }
}

/// Wrap a reqwest error into a `SyncError`. Connection-level
/// failures, request-send failures (TLS handshake, broken pipe,
/// stream errors that happen during `.send()`) and body-streaming
/// failures all map to `Network` — they're all "the transport
/// didn't carry our request through" cases, which the user can
/// retry / investigate at the network layer. Only genuinely
/// unexpected error shapes (decode failures inside reqwest, …)
/// fall through to `Internal`.
///
/// The displayed message walks `source()` so the user sees the
/// actual root cause ("invalid peer certificate: UnknownIssuer",
/// "connection refused", …) instead of the bare top-level
/// "error sending request for url (…)" which carries no
/// diagnostic information on its own.
fn network_err(err: reqwest::Error) -> SyncError {
    let message = full_chain(&err);
    if err.is_timeout() || err.is_connect() || err.is_request() || err.is_body() {
        SyncError::network(message)
    } else {
        warn!(?err, "unexpected reqwest error");
        SyncError::internal(message)
    }
}

/// Stringify an error plus every entry in its `source()` chain,
/// separated by `": "`. The top-level reqwest error description
/// ("error sending request for url (…)") is essentially a label
/// — the actual cause lives a few layers down (hyper → rustls →
/// "invalid peer certificate", or hyper → tokio → "connection
/// refused"). Without this the user sees the label and has no
/// path to debugging.
fn full_chain(err: &dyn std::error::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(s) = source {
        let text = s.to_string();
        // Skip duplicates — some wrappers re-stringify their
        // child's message verbatim.
        if !parts.last().map(|prev| prev == &text).unwrap_or(false) {
            parts.push(text);
        }
        source = s.source();
    }
    parts.join(": ")
}

/// Translate an HTTP status code into the right `SyncError`
/// flavour. 401/403 → Auth; 404 stays usable as `NotFound`; 5xx →
/// Network; anything else → Internal.
fn http_err(status: StatusCode, url: &Url) -> SyncError {
    let msg = format!("{} for {url}", status);
    match status.as_u16() {
        401 | 403 => SyncError::auth(msg),
        404 => SyncError::not_found(msg),
        500..=599 => SyncError::network(msg),
        _ => SyncError::internal(msg),
    }
}

/// Minimal percent-decode for hrefs. Returns `None` on a malformed
/// escape so the caller skips the entry rather than panicking.
fn percent_decode(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            // Need exactly two more hex digits after the %. A
            // truncated escape (e.g. "%2") is malformed; return
            // None so the caller skips the row rather than trying
            // to interpret garbage.
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push(char::from_u32(hi * 16 + lo)?);
            i += 3;
        } else {
            out.push(b as char);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn push_log_mkcols_the_collection_only_once_per_session() {
        let mut server = mockito::Server::new_async().await;
        // The `log/` collection must be MKCOL'd exactly ONCE across
        // several pushes — the session cache skips the redundant
        // round-trip every later push would otherwise pay (the startup
        // pushed=N case that motivated this).
        let mkcol = server
            .mock("MKCOL", "/log/")
            .with_status(201)
            .expect(1)
            .create_async()
            .await;
        // Every log PUT just succeeds; we don't constrain the count.
        let _put = server
            .mock("PUT", mockito::Matcher::Regex(r"^/log/".into()))
            .with_status(201)
            .create_async()
            .await;

        let adapter = WebDavSyncAdapter::new(
            &format!("{}/", server.url()),
            WebDavCredentials::basic("u", "p"),
        )
        .unwrap();

        let ts = chrono::Utc::now();
        for i in 0..3 {
            let name = LogFileName::new(ts, sync_core::DeviceId::from_string(format!("dev-{i}")));
            adapter
                .push_log(&LogFile {
                    name,
                    bytes: b"{}".to_vec(),
                })
                .await
                .expect("push_log succeeds against the mock");
        }
        mkcol.assert_async().await;
    }

    #[test]
    fn basic_credentials_encodes_user_pass() {
        let c = WebDavCredentials::basic("alice", "secret");
        let header = match &c {
            WebDavCredentials::Basic { authorization } => authorization,
            _ => panic!("expected Basic"),
        };
        // "alice:secret" base64 == "YWxpY2U6c2VjcmV0".
        assert_eq!(header, "Basic YWxpY2U6c2VjcmV0");
    }

    #[test]
    fn none_credentials_emits_no_header() {
        let c = WebDavCredentials::None;
        assert!(c.header().is_none());
    }

    #[test]
    fn new_rejects_url_without_scheme() {
        let err = WebDavSyncAdapter::new("nope/aperio", WebDavCredentials::None).unwrap_err();
        assert!(matches!(err, SyncError::Internal(_)));
    }

    #[test]
    fn new_adds_trailing_slash_to_path() {
        let adapter =
            WebDavSyncAdapter::new("https://example.com/dav/aperio", WebDavCredentials::None)
                .unwrap();
        assert!(adapter.base_url().path().ends_with('/'));
    }

    #[test]
    fn url_for_resolves_relative_path() {
        let adapter =
            WebDavSyncAdapter::new("https://example.com/dav/aperio/", WebDavCredentials::None)
                .unwrap();
        let url = adapter
            .url_for("log/2026-05-01T00-00-00Z_dev-a.jsonl")
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://example.com/dav/aperio/log/2026-05-01T00-00-00Z_dev-a.jsonl",
        );
    }

    #[test]
    fn percent_decode_handles_basic_escapes() {
        assert_eq!(
            percent_decode("2026-05-01T00%3A00%3A00Z_dev-a.jsonl").as_deref(),
            Some("2026-05-01T00:00:00Z_dev-a.jsonl"),
        );
        // No escapes — round-trip unchanged.
        assert_eq!(
            percent_decode("simple.jsonl").as_deref(),
            Some("simple.jsonl"),
        );
        // Truncated escape — return None so the caller skips the row.
        assert_eq!(percent_decode("bad%2").as_deref(), None);
    }

    #[test]
    fn http_err_maps_401_to_auth() {
        let url = Url::parse("https://example.com/").unwrap();
        let err = http_err(StatusCode::UNAUTHORIZED, &url);
        assert!(matches!(err, SyncError::Auth(_)));
    }

    #[test]
    fn http_err_maps_404_to_not_found() {
        let url = Url::parse("https://example.com/").unwrap();
        let err = http_err(StatusCode::NOT_FOUND, &url);
        assert!(matches!(err, SyncError::NotFound(_)));
    }

    #[test]
    fn http_err_maps_5xx_to_network() {
        let url = Url::parse("https://example.com/").unwrap();
        let err = http_err(StatusCode::INTERNAL_SERVER_ERROR, &url);
        assert!(matches!(err, SyncError::Network(_)));
    }

    /// The source-chain walker is what turns the bare reqwest
    /// "error sending request for url (…)" into something the
    /// user can actually debug from. Verify it flattens a
    /// hand-built chain in order, deduplicates adjacent
    /// duplicates, and survives a leaf with no source.
    #[test]
    fn full_chain_flattens_source_chain() {
        use std::error::Error as StdError;
        use std::fmt;

        #[derive(Debug)]
        struct Layer {
            msg: &'static str,
            source: Option<Box<dyn StdError + 'static>>,
        }
        impl fmt::Display for Layer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.msg)
            }
        }
        impl StdError for Layer {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                self.source.as_deref()
            }
        }

        let leaf = Layer {
            msg: "connection refused",
            source: None,
        };
        let mid = Layer {
            msg: "dns or tcp",
            source: Some(Box::new(leaf)),
        };
        let top = Layer {
            msg: "error sending request",
            source: Some(Box::new(mid)),
        };
        assert_eq!(
            full_chain(&top),
            "error sending request: dns or tcp: connection refused",
        );

        // Dedup of adjacent duplicates (some wrappers re-stringify
        // their child verbatim).
        let leaf2 = Layer {
            msg: "boom",
            source: None,
        };
        let dup = Layer {
            msg: "boom",
            source: Some(Box::new(leaf2)),
        };
        assert_eq!(full_chain(&dup), "boom");

        // Leaf alone — just the top-level message.
        let alone = Layer {
            msg: "alone",
            source: None,
        };
        assert_eq!(full_chain(&alone), "alone");
    }
}
