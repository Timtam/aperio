//! The Webex REST layer: one authenticated client over `https://webexapis.com/v1`.
//!
//! Everything the adapter does goes through [`ApiState::request`], which owns
//! the three concerns that would otherwise be repeated at every call site:
//!
//! **Tokens.** The access token lasts 14 days and the state refreshes it before
//! it lapses, and again — once — if a call comes back 401 anyway, because a
//! token can be revoked out of band by a password change. The refreshed set is
//! kept in memory; persisting it is the host's job and needs a plugin-to-host
//! channel that does not exist yet, so see the note on [`ApiState::tokens`].
//!
//! **Errors.** Webex answers with a `trackingId` that is the only handle Cisco
//! support accepts, so it is carried into every message and every log line.
//! Statuses are sorted into the [`VcError`] variants by what the user can do
//! about them, not by what they are: a 503 is transient and retryable, a 403 on
//! a meeting scope means the licence is wrong and retrying will never help.
//!
//! **Rate limits.** Regular users may create 100 meetings per 24 hours and the
//! whole API is throttled around 300 requests per minute. A 429 carries
//! `Retry-After` in seconds; it is surfaced in the message rather than slept
//! through, because a plugin call has no cancellation and a silent multi-minute
//! sleep inside one is indistinguishable from a hang.

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::{debug, warn};
use vc_core::{VcError, VcResult};

use crate::oauth::{self, TokenSet};

pub const API_BASE: &str = "https://webexapis.com/v1";

/// Refresh this far before the access token actually lapses, so a request that
/// starts just under the wire does not land just over it.
const REFRESH_SKEW: Duration = Duration::from_secs(120);

/// One authenticated Webex session, shared by every call for one account.
pub struct ApiState {
    /// The current tokens.
    ///
    /// A refresh updates this in memory only. Webex was measured to return the
    /// SAME refresh-token value with a fresh 90-day expiry, so nothing is lost
    /// today — but the host's stored expiry does go stale, and the day Webex
    /// starts rotating values this becomes a real leak of the only working
    /// credential. Persisting it needs a plugin-to-host channel; that is
    /// designed (an optional `aperio_plugin_set_host_channel` export) and not
    /// yet built.
    pub tokens: Mutex<TokenSet>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub http: reqwest::Client,
    /// Overridable for tests.
    pub api_base: String,
    pub token_url: String,
    /// The host-channel capability token for this account, if the host gave
    /// one. Without it a refreshed credential cannot be reported and stays in
    /// memory — which is the whole failure this exists to prevent.
    pub scope_token: Option<String>,
    /// Told to the host, so it can persist what the provider handed back.
    ///
    /// Injected rather than called directly so this crate stays free of the
    /// plugin SDK: the adapter is a plain library that a `-plugin` crate wraps,
    /// and only that wrapper knows about FFI.
    pub credential_sink: Option<Box<dyn CredentialSink>>,
}

/// How the adapter tells whoever owns it that a credential changed.
///
/// Implemented by the plugin wrapper over the host channel; `None` in tests and
/// in any embedding that does not persist credentials.
pub trait CredentialSink: Send + Sync {
    /// `slot` is the host's vocabulary — `refresh_token`, `access_token`.
    /// `expires_at` is RFC 3339 when the provider reported one.
    fn credential_rotated(&self, scope: &str, slot: &str, value: &str, expires_at: Option<&str>);
}

impl ApiState {
    pub fn new(
        tokens: TokenSet,
        client_id: String,
        client_secret: Option<String>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            tokens: Mutex::new(tokens),
            client_id,
            client_secret,
            http,
            api_base: API_BASE.to_string(),
            token_url: oauth::WEBEX_TOKEN_URL.to_string(),
            scope_token: None,
            credential_sink: None,
        }
    }

    /// Attach the host channel, so a rotated credential is persisted rather
    /// than kept in memory until the instance closes.
    pub fn with_credential_sink(
        mut self,
        scope_token: Option<String>,
        sink: Box<dyn CredentialSink>,
    ) -> Self {
        self.scope_token = scope_token;
        self.credential_sink = Some(sink);
        self
    }

    fn access_token(&self) -> String {
        self.tokens
            .lock()
            .expect("token mutex poisoned")
            .access_token
            .clone()
    }

    /// Refresh if the access token is spent (or nearly). Returns whether a
    /// refresh actually happened, so a 401 retry can avoid refreshing twice.
    async fn refresh_if_needed(&self) -> VcResult<bool> {
        let (needed, refresh_token) = {
            let tokens = self.tokens.lock().expect("token mutex poisoned");
            (
                tokens.access_expired(Utc::now(), REFRESH_SKEW),
                tokens.refresh_token.clone(),
            )
        };
        if !needed {
            return Ok(false);
        }
        let Some(refresh_token) = refresh_token else {
            return Err(VcError::Authentication(
                "the Webex access token has expired and there is no refresh token to renew it \
                 with — sign in to Webex again"
                    .into(),
            ));
        };
        self.force_refresh(&refresh_token).await?;
        Ok(true)
    }

    async fn force_refresh(&self, refresh_token: &str) -> VcResult<()> {
        debug!("refreshing the Webex access token");
        let fresh = oauth::refresh(
            &self.http,
            &self.token_url,
            &self.client_id,
            self.client_secret.as_deref(),
            refresh_token,
        )
        .await?;
        let mut tokens = self.tokens.lock().expect("token mutex poisoned");
        // Keep the previous refresh token when the response omits one: Webex
        // usually echoes it unchanged, but an omission means "the one you have
        // is still good", not "you no longer have one".
        let carried = fresh
            .refresh_token
            .clone()
            .or_else(|| tokens.refresh_token.clone());
        *tokens = TokenSet {
            refresh_token: carried,
            ..fresh
        };
        let reportable = tokens.refresh_token.clone();
        let expires = tokens.refresh_expires_at.map(|at| at.to_rfc3339());
        drop(tokens);

        // Tell the host. Webex was measured returning the SAME value with a
        // fresh 90-day clock, so this usually only moves an expiry — but the
        // expiry is exactly what a host that stores values alone would miss,
        // and the day Webex starts rotating values this is what keeps the
        // account alive. Reported AFTER the lock is released: the sink may do
        // real work and must not hold the token mutex while it does.
        if let (Some(sink), Some(scope), Some(value)) = (
            self.credential_sink.as_ref(),
            self.scope_token.as_deref(),
            reportable.as_deref(),
        ) {
            sink.credential_rotated(scope, "refresh_token", value, expires.as_deref());
        }
        Ok(())
    }

    /// Issue one authenticated request, refreshing around it as needed.
    ///
    /// `path` is relative to the API base and must start with a slash.
    async fn request<B: Serialize + ?Sized>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> VcResult<(reqwest::StatusCode, String)> {
        self.request_paged(method, path, body)
            .await
            .map(|(status, text, _)| (status, text))
    }

    /// [`Self::request`], keeping the paging cursor. Takes a FULL url when the
    /// caller is following a cursor, since Webex's `Link` header is absolute.
    async fn request_paged<B: Serialize + ?Sized>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> VcResult<(reqwest::StatusCode, String, Option<String>)> {
        let refreshed = self.refresh_if_needed().await?;
        // A cursor from the Link header is already absolute; a caller-built
        // path is relative to the API base.
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{path}", self.api_base)
        };

        let response = self.send_once(&method, &url, body).await?;
        // A 401 after a valid-looking token means it was revoked out of band —
        // a password change, an admin action, a user pulling the grant. One
        // refresh-and-retry turns that into a hiccup instead of an error the
        // user has to act on; a second would just be a loop.
        if response.0 == reqwest::StatusCode::UNAUTHORIZED && !refreshed {
            let refresh_token = self
                .tokens
                .lock()
                .expect("token mutex poisoned")
                .refresh_token
                .clone();
            if let Some(refresh_token) = refresh_token {
                debug!("Webex answered 401; refreshing once and retrying");
                self.force_refresh(&refresh_token).await?;
                return self.send_once(&method, &url, body).await;
            }
        }
        Ok(response)
    }

    /// One attempt.
    ///
    /// The third element is the RFC 5988 `rel="next"` cursor, when the response
    /// carried one. Webex pages its listings through a `Link` HEADER rather
    /// than through the body, so a caller that only ever looks at the JSON
    /// silently reads the first page and calls it the whole answer.
    async fn send_once<B: Serialize + ?Sized>(
        &self,
        method: &reqwest::Method,
        url: &str,
        body: Option<&B>,
    ) -> VcResult<(reqwest::StatusCode, String, Option<String>)> {
        let mut req = self
            .http
            .request(method.clone(), url)
            .bearer_auth(self.access_token());
        if let Some(body) = body {
            req = req.json(body);
        }
        let response = req
            .send()
            .await
            .map_err(|e| VcError::Network(e.to_string()))?;
        let status = response.status();

        // Webex returns 200 with a `Warning` header on "converged sites" where
        // several requested attributes were silently ignored. Saying nothing
        // would let the create look faithful when it was not.
        if let Some(warning) = response.headers().get("warning") {
            warn!(
                warning = %String::from_utf8_lossy(warning.as_bytes()),
                "Webex accepted the request but ignored part of it"
            );
        }
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let next = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .and_then(next_link);
        let text = response
            .text()
            .await
            .map_err(|e| VcError::Network(e.to_string()))?;

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(rate_limited(&text, retry_after.as_deref()));
        }
        Ok((status, text, next))
    }

    /// GET a path and decode it, mapping the failure statuses.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> VcResult<T> {
        let (status, text) = self.request::<()>(reqwest::Method::GET, path, None).await?;
        if !status.is_success() {
            return Err(map_status(status, &text, path));
        }
        decode(&text, path)
    }

    /// GET a path, treating 404 as "not there" rather than as an error.
    pub async fn get_json_opt<T: DeserializeOwned>(&self, path: &str) -> VcResult<Option<T>> {
        let (status, text) = self.request::<()>(reqwest::Method::GET, path, None).await?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(map_status(status, &text, path));
        }
        decode(&text, path).map(Some)
    }

    /// GET a path and decode it, returning the `rel="next"` cursor alongside.
    ///
    /// The cursor is an absolute URL and is handed straight back to this
    /// method to fetch the following page.
    pub async fn get_json_paged<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> VcResult<(T, Option<String>)> {
        let (status, text, next) = self
            .request_paged::<()>(reqwest::Method::GET, path, None)
            .await?;
        if !status.is_success() {
            return Err(map_status(status, &text, path));
        }
        Ok((decode(&text, path)?, next))
    }

    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> VcResult<T> {
        let (status, text) = self
            .request(reqwest::Method::POST, path, Some(body))
            .await?;
        if !status.is_success() {
            return Err(map_status(status, &text, path));
        }
        decode(&text, path)
    }

    pub async fn delete(&self, path: &str) -> VcResult<()> {
        let (status, text) = self
            .request::<()>(reqwest::Method::DELETE, path, None)
            .await?;
        if status.is_success() || status == reqwest::StatusCode::NO_CONTENT {
            return Ok(());
        }
        Err(map_status(status, &text, path))
    }
}

fn decode<T: DeserializeOwned>(text: &str, path: &str) -> VcResult<T> {
    serde_json::from_str(text).map_err(|e| {
        // Carry a bounded excerpt. On a CREATE this is the difference between
        // an orphan and a recoverable one: Webex emits the meeting id first, so
        // it lands inside the excerpt and the meeting stays findable even
        // though the response could not be decoded. Never auto-deleted — a
        // response we could not read is not evidence about what exists.
        let excerpt: String = text.trim().chars().take(300).collect();
        warn!(path, error = %e, "could not decode a Webex response");
        VcError::Protocol(format!(
            "Webex answered with something Aperio could not read. {e} (response began:              {excerpt})"
        ))
    })
}

/// Sort a failure into the variant that matches what the user can DO.
fn map_status(status: reqwest::StatusCode, body: &str, path: &str) -> VcError {
    let detail = describe(body);
    // The path — percent-encoded id, query flags and all — belongs in the log,
    // not in a sentence a screen reader reads out. It is genuinely useful here:
    // it records which meeting and, on a delete, which notify flag.
    warn!(status = status.as_u16(), path, detail = %detail, "a Webex request failed");
    match status.as_u16() {
        400 => VcError::InvalidInput(format!("Webex rejected the request: {detail}")),
        401 => VcError::Authentication(format!(
            "Webex no longer accepts this account's credentials: {detail}"
        )),
        // A 403 on a meeting scope is nearly always a licence, not a
        // permission the user can grant — retrying is pointless, so say so
        // through the variant that means "this will not work here".
        403 => VcError::Forbidden(format!(
            "This Webex account is not allowed to do that: {detail}. Meetings need a Webex \
             Meetings subscription on a site backed by Cisco Common Identity."
        )),
        // No path and no id in the message: both are opaque blobs a screen
        // reader spells out character by character, and neither tells the user
        // anything they can act on.
        404 => VcError::NotFound(format!(
            "Webex could not find what this request asked for; it may already have been              deleted on Webex's side. {detail}"
        )),
        409 => VcError::Protocol(format!("Webex reported a conflict: {detail}")),
        // Transient. `Network` is the variant the UI offers a retry for.
        408 | 500..=599 => VcError::Network(format!(
            "Webex is not responding just now — this usually passes. {detail}"
        )),
        _ => VcError::Protocol(format!(
            "Webex answered in a way Aperio did not expect. {detail}"
        )),
    }
}

/// The `rel="next"` URL out of an RFC 5988 `Link` header, if there is one.
///
/// Deliberately tolerant about spacing and quoting and strict about the
/// relation: a `prev` cursor followed as if it were `next` would page
/// backwards forever.
fn next_link(header: &str) -> Option<String> {
    for part in header.split(',') {
        let mut segments = part.split(';');
        let Some(target) = segments.next() else {
            continue;
        };
        let target = target.trim();
        let Some(url) = target
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
        else {
            continue;
        };
        let is_next = segments.any(|param| {
            let param = param.trim();
            let Some((key, value)) = param.split_once('=') else {
                return false;
            };
            key.trim().eq_ignore_ascii_case("rel")
                && value.trim().trim_matches('"').eq_ignore_ascii_case("next")
        });
        if is_next && !url.trim().is_empty() {
            return Some(url.trim().to_string());
        }
    }
    None
}

fn rate_limited(body: &str, retry_after: Option<&str>) -> VcError {
    let detail = describe(body);
    // Deliberately NOT slept through. A vtable call has no cancellation, so a
    // silent multi-minute sleep inside one is indistinguishable from a hang;
    // the wait belongs to whoever can show it and let the user abandon it.
    match retry_after.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(secs) => VcError::Network(format!(
            "Webex is rate-limiting this account; it asks to wait {secs} seconds. {detail}"
        )),
        None => VcError::Network(format!("Webex is rate-limiting this account. {detail}")),
    }
}

/// Human-readable summary of a Webex error body, keeping the tracking id.
///
/// Webex uses `{"message":…,"errors":[{"description":…}],"trackingId":…}` for
/// API failures. The tracking id is the only handle Cisco support accepts, so
/// it survives into the message the user can copy.
pub(crate) fn describe(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        let trimmed = body.trim();
        return if trimmed.is_empty() {
            "no details".to_string()
        } else {
            trimmed.chars().take(300).collect()
        };
    };
    let message = json
        .get("message")
        .and_then(|v| v.as_str())
        .or_else(|| {
            json.get("errors")
                .and_then(|e| e.get(0))
                .and_then(|e| e.get("description"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("no details");
    match json.get("trackingId").and_then(|v| v.as_str()) {
        Some(id) => format!("{message} (Webex tracking id {id})"),
        None => message.to_string(),
    }
}

/// Format an instant the way the Meetings API wants it.
pub(crate) fn wire_time(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Fixtures the sibling modules' tests build on.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// An [`ApiState`] pointed at a mock server, with a token that is nowhere
    /// near expiry so a test exercises the call rather than the refresh.
    pub(crate) fn state(server_url: &str) -> ApiState {
        let mut s = ApiState::new(
            TokenSet {
                access_token: "AT".into(),
                refresh_token: Some("RT".into()),
                expires_at: Utc::now() + chrono::Duration::seconds(100_000),
                refresh_expires_at: None,
                scope: None,
            },
            "client".into(),
            Some("secret".into()),
            reqwest::Client::new(),
        );
        s.api_base = server_url.to_string();
        s.token_url = format!("{server_url}/access_token");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn state(server_url: &str, expires_in_secs: i64) -> ApiState {
        let mut s = ApiState::new(
            TokenSet {
                access_token: "AT".into(),
                refresh_token: Some("RT".into()),
                expires_at: Utc::now() + chrono::Duration::seconds(expires_in_secs),
                refresh_expires_at: None,
                scope: None,
            },
            "client".into(),
            Some("secret".into()),
            reqwest::Client::new(),
        );
        s.api_base = server_url.to_string();
        s.token_url = format!("{server_url}/access_token");
        s
    }

    #[test]
    fn wire_time_is_second_precision_utc() {
        let at = Utc.with_ymd_and_hms(2026, 7, 28, 9, 30, 0).unwrap();
        assert_eq!(wire_time(at), "2026-07-28T09:30:00Z");
    }

    #[test]
    fn a_tracking_id_survives_into_the_message() {
        // It is the only handle Cisco support accepts, so losing it means the
        // user cannot get help with the failure they just saw.
        let d = describe(r#"{"message":"nope","trackingId":"ROUTERGW_abc"}"#);
        assert!(d.contains("nope") && d.contains("ROUTERGW_abc"), "got {d}");
        let deep = describe(r#"{"errors":[{"description":"deeper"}]}"#);
        assert!(deep.contains("deeper"), "got {deep}");
        assert_eq!(describe(""), "no details");
        assert_eq!(describe("plain text"), "plain text");
    }

    #[test]
    fn statuses_map_to_what_the_user_can_do_about_them() {
        let body = r#"{"message":"m"}"#;
        assert!(matches!(
            map_status(reqwest::StatusCode::BAD_REQUEST, body, "/meetings"),
            VcError::InvalidInput(_)
        ));
        assert!(matches!(
            map_status(reqwest::StatusCode::FORBIDDEN, body, "/meetings"),
            VcError::Forbidden(_)
        ));
        assert!(matches!(
            map_status(reqwest::StatusCode::NOT_FOUND, body, "/meetings/x"),
            VcError::NotFound(_)
        ));
        // Transient, so it must be the retryable variant rather than one that
        // reads as "this adapter is broken".
        assert!(matches!(
            map_status(reqwest::StatusCode::SERVICE_UNAVAILABLE, body, "/meetings"),
            VcError::Network(_)
        ));
        assert!(matches!(
            map_status(reqwest::StatusCode::REQUEST_TIMEOUT, body, "/meetings"),
            VcError::Network(_)
        ));
    }

    #[test]
    fn user_facing_messages_carry_no_path_status_code_or_meeting_id() {
        // These strings are read aloud verbatim. A percent-encoded API route
        // and an opaque base64-ish meeting id are spelled out character by
        // character and tell the user nothing they can act on; both belong in
        // the log, which map_status now writes.
        let body = r#"{"message":"m","trackingId":"ROUTERGW_x"}"#;
        for (status, path) in [
            (reqwest::StatusCode::NOT_FOUND, "/meetings/AbC%2F123%2Bx"),
            (
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "/meetings/AbC%2F123%2Bx?sendEmail=false",
            ),
            (reqwest::StatusCode::IM_A_TEAPOT, "/meetings/AbC%2F123%2Bx"),
        ] {
            let text = map_status(status, body, path).to_string();
            assert!(!text.contains('%'), "percent-encoding leaked: {text}");
            assert!(!text.contains("AbC"), "the meeting id leaked: {text}");
            assert!(!text.contains("sendEmail"), "a query flag leaked: {text}");
            assert!(!text.contains("/meetings"), "the API path leaked: {text}");
            // The tracking id is the one opaque string worth keeping — it is
            // the only handle Cisco support accepts — and it sits at the end.
            assert!(text.contains("ROUTERGW_x"), "tracking id lost: {text}");
        }
    }

    #[test]
    fn an_undecodable_response_keeps_enough_to_find_what_was_created() {
        // A create that Webex accepted but whose body we could not read would
        // otherwise leave a meeting nothing can name. Webex emits the id first,
        // so the excerpt keeps it findable — and nothing is auto-deleted, since
        // a response we could not read is not evidence about what exists.
        let err = decode::<serde_json::Value>(
            r#"{"id":"MEETING-abc123","webLink":<broken>}"#,
            "/meetings",
        )
        .expect_err("must fail");
        assert!(err.to_string().contains("MEETING-abc123"), "got {err}");
    }

    #[test]
    fn a_rate_limit_reports_the_wait_instead_of_sleeping_through_it() {
        let err = rate_limited(r#"{"message":"slow down"}"#, Some("42"));
        let text = err.to_string();
        assert!(text.contains("42 seconds"), "got {text}");
        assert!(text.contains("slow down"), "got {text}");
        // No Retry-After is still a rate limit, just without a number.
        assert!(rate_limited("{}", None)
            .to_string()
            .contains("rate-limiting"));
    }

    #[tokio::test]
    async fn an_expiring_token_is_refreshed_before_the_call() {
        let mut server = mockito::Server::new_async().await;
        let token = server
            .mock("POST", "/access_token")
            .with_status(200)
            .with_body(r#"{"access_token":"AT2","refresh_token":"RT","expires_in":1209599}"#)
            .create_async()
            .await;
        let call = server
            .mock("GET", "/thing")
            .match_header("authorization", "Bearer AT2")
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .create_async()
            .await;

        // Already expired by the skew.
        let state = state(&server.url(), 10);
        let _: serde_json::Value = state.get_json("/thing").await.expect("call");
        token.assert_async().await;
        call.assert_async().await;
    }

    #[tokio::test]
    async fn a_401_on_a_fresh_token_refreshes_once_and_retries() {
        // Tokens can be revoked out of band — a password change, an admin
        // pulling the grant. One retry turns that into a hiccup.
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("GET", "/thing")
            .match_header("authorization", "Bearer AT")
            .with_status(401)
            .with_body(r#"{"message":"token revoked"}"#)
            .expect(1)
            .create_async()
            .await;
        let token = server
            .mock("POST", "/access_token")
            .with_status(200)
            .with_body(r#"{"access_token":"AT2","expires_in":1209599}"#)
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("GET", "/thing")
            .match_header("authorization", "Bearer AT2")
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .expect(1)
            .create_async()
            .await;

        let state = state(&server.url(), 100_000);
        let _: serde_json::Value = state.get_json("/thing").await.expect("call");
        first.assert_async().await;
        token.assert_async().await;
        second.assert_async().await;
    }

    #[derive(Default)]
    struct RecordingSink {
        seen: std::sync::Mutex<Vec<(String, String, Option<String>)>>,
    }

    impl CredentialSink for RecordingSink {
        fn credential_rotated(
            &self,
            scope: &str,
            slot: &str,
            value: &str,
            expires_at: Option<&str>,
        ) {
            self.seen.lock().unwrap().push((
                format!("{scope}/{slot}"),
                value.to_string(),
                expires_at.map(str::to_owned),
            ));
        }
    }

    #[tokio::test]
    async fn a_refresh_reports_the_credential_so_the_host_can_persist_it() {
        // Webex was measured returning the SAME value with a fresh 90-day
        // clock, so this usually only moves an expiry — which is precisely
        // what a host storing values alone would miss, watching a working
        // credential appear to die on day 90.
        let mut server = mockito::Server::new_async().await;
        let _t = server
            .mock("POST", "/access_token")
            .with_status(200)
            .with_body(
                r#"{"access_token":"AT2","refresh_token":"RT","expires_in":1209599,
                    "refresh_token_expires_in":7776000}"#,
            )
            .create_async()
            .await;
        let _c = server
            .mock("GET", "/thing")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;

        let sink = std::sync::Arc::new(RecordingSink::default());
        struct Forward(std::sync::Arc<RecordingSink>);
        impl CredentialSink for Forward {
            fn credential_rotated(
                &self,
                scope: &str,
                slot: &str,
                value: &str,
                expires_at: Option<&str>,
            ) {
                self.0.credential_rotated(scope, slot, value, expires_at);
            }
        }
        let state = state(&server.url(), 10)
            .with_credential_sink(Some("the-token".into()), Box::new(Forward(sink.clone())));
        let _: serde_json::Value = state.get_json("/thing").await.expect("call");

        let seen = sink.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "exactly one report per refresh");
        assert_eq!(seen[0].0, "the-token/refresh_token");
        assert_eq!(seen[0].1, "RT");
        assert!(seen[0].2.is_some(), "the expiry must ride along");
    }

    #[tokio::test]
    async fn without_a_scope_token_nothing_is_reported() {
        // A host that predates the channel gives no token; reporting into the
        // void would be noise.
        let mut server = mockito::Server::new_async().await;
        let _t = server
            .mock("POST", "/access_token")
            .with_status(200)
            .with_body(r#"{"access_token":"AT2","refresh_token":"RT","expires_in":1209599}"#)
            .create_async()
            .await;
        let _c = server
            .mock("GET", "/thing")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let sink = std::sync::Arc::new(RecordingSink::default());
        struct Forward(std::sync::Arc<RecordingSink>);
        impl CredentialSink for Forward {
            fn credential_rotated(&self, s: &str, sl: &str, v: &str, e: Option<&str>) {
                self.0.credential_rotated(s, sl, v, e);
            }
        }
        let state =
            state(&server.url(), 10).with_credential_sink(None, Box::new(Forward(sink.clone())));
        let _: serde_json::Value = state.get_json("/thing").await.expect("call");
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_refresh_that_omits_the_token_keeps_the_one_we_have() {
        // An omitted refresh_token means "the one you have is still good", not
        // "you no longer have one" — dropping it would strand the account.
        let mut server = mockito::Server::new_async().await;
        let _t = server
            .mock("POST", "/access_token")
            .with_status(200)
            .with_body(r#"{"access_token":"AT2","expires_in":1209599}"#)
            .create_async()
            .await;
        let _c = server
            .mock("GET", "/thing")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;

        let state = state(&server.url(), 10);
        let _: serde_json::Value = state.get_json("/thing").await.expect("call");
        assert_eq!(
            state.tokens.lock().unwrap().refresh_token.as_deref(),
            Some("RT")
        );
    }

    #[tokio::test]
    async fn a_429_is_reported_and_never_retried_silently() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/thing")
            .with_status(429)
            .with_header("retry-after", "30")
            .with_body(r#"{"message":"too many"}"#)
            .expect(1)
            .create_async()
            .await;
        let state = state(&server.url(), 100_000);
        let err = state
            .get_json::<serde_json::Value>("/thing")
            .await
            .expect_err("must fail");
        assert!(err.to_string().contains("30 seconds"), "got {err}");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn a_404_is_absence_not_failure_on_the_optional_getter() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/gone")
            .with_status(404)
            .with_body(r#"{"message":"not found"}"#)
            .create_async()
            .await;
        let state = state(&server.url(), 100_000);
        let got: Option<serde_json::Value> = state.get_json_opt("/gone").await.expect("call");
        assert!(got.is_none());
    }
}
