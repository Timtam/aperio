//! Google Calendar REST API client.
//!
//! Wraps the v3 endpoints we need for read access plus the
//! transparent token-refresh dance: every request runs with the
//! current access token; on a 401 we use the stored refresh token
//! to mint a new access token (`grant_type=refresh_token`) and
//! retry the request once. Refresh failures bubble out as
//! `GoogleError::Http { status: 401, .. }` so the trait-impl
//! mapping marks them as `Authentication` — the user is asked to
//! reconnect the account.

use std::sync::Arc;

use cal_core::{Calendar, Event};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use tracing::warn;
use url::Url;

use crate::auth::{self, TokenSet, GOOGLE_TOKEN_URL};
use crate::error::{GoogleError, GoogleResult};
use crate::mapping::{
    map_calendar, map_event, CalendarListResponse, EventListResponse,
};

const API_BASE: &str = "https://www.googleapis.com/calendar/v3";

/// Shared mutable state across API calls. Wrapped in `Arc<Mutex>`
/// so the adapter — itself shared behind `Arc<dyn CalendarFeature>`
/// in the registry — can mutate the token set during refresh
/// without the caller having to hold a write-lock.
#[derive(Debug, Clone)]
pub struct ApiState {
    pub tokens: Arc<Mutex<TokenSet>>,
    pub client_id: String,
    pub http: reqwest::Client,
    /// Token endpoint. Production code passes the Google URL; tests
    /// inject mockito.
    pub token_url: String,
    /// API base URL — overridable for tests.
    pub api_base: String,
}

impl ApiState {
    pub fn new(
        tokens: TokenSet,
        client_id: String,
        http: reqwest::Client,
    ) -> Self {
        Self {
            tokens: Arc::new(Mutex::new(tokens)),
            client_id,
            http,
            token_url: GOOGLE_TOKEN_URL.to_string(),
            api_base: API_BASE.to_string(),
        }
    }

    /// Send a GET, decode the JSON body, transparently refresh on
    /// 401. The closure builds the URL — accepting it as a fn so
    /// the retry doesn't accidentally reuse a stale URL when a
    /// caller embeds the access token in the URL itself (we don't,
    /// but defensive).
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path_and_query: &str,
    ) -> GoogleResult<T> {
        // First try with whatever access token we currently have.
        let url = Url::parse(&format!("{}{}", self.api_base, path_and_query))
            .map_err(|e| GoogleError::Config(format!("bad url: {e}")))?;
        let access = self.tokens.lock().await.access_token.clone();
        let response = self
            .http
            .get(url.clone())
            .bearer_auth(&access)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            // Refresh and retry exactly once. If the second attempt
            // also bounces with 401 we propagate the error so the
            // user sees an honest "reconnect required" prompt.
            self.refresh_access_token().await?;
            let new_access = self.tokens.lock().await.access_token.clone();
            let retry = self
                .http
                .get(url)
                .bearer_auth(&new_access)
                .send()
                .await?;
            return decode_json(retry).await;
        }
        decode_json(response).await
    }

    /// Run the refresh-token grant and replace the in-memory token
    /// set. The refresh token typically lives in keychain across
    /// app restarts; the adapter only updates the in-memory copy
    /// here.
    async fn refresh_access_token(&self) -> GoogleResult<()> {
        let refresh = self
            .tokens
            .lock()
            .await
            .refresh_token
            .clone()
            .ok_or_else(|| GoogleError::Http {
                status: 401,
                message: "no refresh token; reconnect required".into(),
            })?;
        let fresh = auth::refresh(&self.client_id, &self.token_url, &refresh, &self.http)
            .await
            .map_err(|err| {
                warn!(?err, "refresh-token grant failed");
                err
            })?;
        let mut guard = self.tokens.lock().await;
        guard.access_token = fresh.access_token;
        // Google sometimes omits the refresh token in the response —
        // RFC 6749 §6 allows reusing the previously stored value.
        if let Some(rt) = fresh.refresh_token {
            guard.refresh_token = Some(rt);
        }
        guard.expires_at = fresh.expires_at;
        Ok(())
    }
}

async fn decode_json<T: DeserializeOwned>(
    response: reqwest::Response,
) -> GoogleResult<T> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(GoogleError::Http {
            status: status.as_u16(),
            message: text.chars().take(300).collect(),
        });
    }
    serde_json::from_str(&text)
        .map_err(|e| GoogleError::Protocol(format!("json: {e}: {text}")))
}

/// `GET /users/me/calendarList`. Pages through `nextPageToken` until
/// Google stops returning one — for typical users this is one call.
pub async fn list_calendars(state: &ApiState) -> GoogleResult<Vec<Calendar>> {
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let path = match &page_token {
            Some(t) => format!("/users/me/calendarList?pageToken={t}"),
            None => "/users/me/calendarList".to_string(),
        };
        let resp: CalendarListResponse = state.get_json(&path).await?;
        for entry in resp.items {
            out.push(map_calendar(entry));
        }
        match resp.next_page_token {
            Some(t) => page_token = Some(t),
            None => break,
        }
    }
    Ok(out)
}

/// `GET /calendars/{id}/events?timeMin={from}&timeMax={to}&singleEvents=false`.
///
/// `singleEvents=false` keeps recurring-master rows intact (with their
/// `RRULE` / `EXDATE` in `recurrence: [...]`); the frontend expands
/// them via rrule.js, the same model the local + CalDAV + iCal
/// adapters already use.
pub async fn get_events(
    state: &ApiState,
    calendar_id: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> GoogleResult<Vec<Event>> {
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    let cal_enc = urlencoding(calendar_id);
    loop {
        let mut path = format!(
            "/calendars/{cal_enc}/events?singleEvents=false&maxResults=2500\
             &timeMin={tm}&timeMax={tx}",
            tm = urlencoding(&start.to_rfc3339()),
            tx = urlencoding(&end.to_rfc3339()),
        );
        if let Some(t) = &page_token {
            path.push_str("&pageToken=");
            path.push_str(&urlencoding(t));
        }
        let resp: EventListResponse = state.get_json(&path).await?;
        for entry in resp.items {
            if let Some(ev) = map_event(entry, calendar_id)? {
                out.push(ev);
            }
        }
        match resp.next_page_token {
            Some(t) => page_token = Some(t),
            None => break,
        }
    }
    Ok(out)
}

/// Percent-encode for URL query / path segment use. We do this
/// manually so we don't pull in a dedicated crate for one helper.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixture_state(server_url: &str) -> ApiState {
        ApiState {
            tokens: Arc::new(Mutex::new(TokenSet {
                access_token: "initial-token".into(),
                refresh_token: Some("refresh-token".into()),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                scope: None,
            })),
            client_id: "test-client".into(),
            http: reqwest::Client::new(),
            token_url: format!("{server_url}/token"),
            api_base: server_url.to_string(),
        }
    }

    #[tokio::test]
    async fn list_calendars_maps_response() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/users/me/calendarList")
            .with_status(200)
            .with_body(
                r##"{
                    "items": [
                      {"id": "primary", "summary": "Me", "accessRole": "owner", "backgroundColor": "#1e88e5"},
                      {"id": "team@google", "summary": "Team", "accessRole": "reader"}
                    ]
                }"##,
            )
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        let cals = list_calendars(&state).await.unwrap();
        assert_eq!(cals.len(), 2);
        assert_eq!(cals[0].name, "Me");
        assert!(!cals[0].read_only);
        assert!(cals[1].read_only);
    }

    #[tokio::test]
    async fn get_events_passes_range_to_query() {
        let mut server = mockito::Server::new_async().await;
        // Mockito's path matcher receives the full request target
        // (path + query string), so we match on a regex that just
        // checks both timeMin and timeMax are present together with
        // the singleEvents=false flag we always send.
        let m = server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/calendars/primary/events\?.*singleEvents=false.*timeMin=.*timeMax="
                        .to_string(),
                ),
            )
            .with_status(200)
            .with_body(r#"{"items":[]}"#)
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        let from = chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let evs = get_events(&state, "primary", from, to).await.unwrap();
        assert!(evs.is_empty());
        m.assert_async().await;
    }

    #[tokio::test]
    async fn get_json_refreshes_token_on_401_and_retries() {
        let mut server = mockito::Server::new_async().await;
        // First request with old token → 401.
        let initial = server
            .mock("GET", "/users/me/calendarList")
            .match_header("authorization", "Bearer initial-token")
            .with_status(401)
            .with_body("expired")
            .expect(1)
            .create_async()
            .await;
        // Refresh-token POST → mint a new access token.
        let refresh = server
            .mock("POST", "/token")
            .match_body(mockito::Matcher::Regex(
                "grant_type=refresh_token".into(),
            ))
            .with_status(200)
            .with_body(
                r#"{"access_token":"new-token","expires_in":3600,"token_type":"Bearer"}"#,
            )
            .expect(1)
            .create_async()
            .await;
        // Retry with new token → succeed.
        let retry = server
            .mock("GET", "/users/me/calendarList")
            .match_header("authorization", "Bearer new-token")
            .with_status(200)
            .with_body(r#"{"items":[]}"#)
            .expect(1)
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        let cals = list_calendars(&state).await.unwrap();
        assert!(cals.is_empty());
        initial.assert_async().await;
        refresh.assert_async().await;
        retry.assert_async().await;

        // The in-memory token set should now hold the refreshed value.
        let current = state.tokens.lock().await;
        assert_eq!(current.access_token, "new-token");
        // Refresh token wasn't returned by the refresh response — old
        // value must be retained.
        assert_eq!(current.refresh_token.as_deref(), Some("refresh-token"));
    }

    #[tokio::test]
    async fn get_json_surfaces_non_401_failure_immediately() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/users/me/calendarList")
            .with_status(500)
            .with_body("server error")
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let err = list_calendars(&state).await.unwrap_err();
        assert!(matches!(err, GoogleError::Http { status: 500, .. }));
    }
}
