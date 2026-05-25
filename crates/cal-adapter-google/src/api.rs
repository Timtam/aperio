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

use cal_core::{Calendar, Event, NewEvent};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::warn;
use url::Url;

use crate::auth::{self, TokenSet, GOOGLE_TOKEN_URL};
use crate::error::{GoogleError, GoogleResult};
use crate::mapping::{
    event_to_body, map_calendar, map_event, new_event_to_body, CalendarListResponse, EventEntry,
    EventListResponse,
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
    /// Google's token endpoint demands the OAuth client secret on
    /// every grant (auth-code exchange + refresh), even with PKCE
    /// in the mix — see `auth::run` for the chapter and verse.
    pub client_secret: String,
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
        client_secret: String,
        http: reqwest::Client,
    ) -> Self {
        Self {
            tokens: Arc::new(Mutex::new(tokens)),
            client_id,
            client_secret,
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
    pub async fn get_json<T: DeserializeOwned>(&self, path_and_query: &str) -> GoogleResult<T> {
        let url = self.build_url(path_and_query)?;
        let response = self
            .send_with_refresh(|access| self.http.get(url.clone()).bearer_auth(access))
            .await?;
        decode_json(response).await
    }

    /// POST a JSON body to `path`, return the decoded response.
    /// Same 401-refresh-and-retry-once semantics as `get_json`.
    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> GoogleResult<T> {
        let url = self.build_url(path)?;
        let json = serde_json::to_string(body)?;
        let response = self
            .send_with_refresh(|access| {
                self.http
                    .post(url.clone())
                    .bearer_auth(access)
                    .header("content-type", "application/json")
                    .body(json.clone())
            })
            .await?;
        decode_json(response).await
    }

    /// PATCH a JSON body. Same model as POST.
    pub async fn patch_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> GoogleResult<T> {
        let url = self.build_url(path)?;
        let json = serde_json::to_string(body)?;
        let response = self
            .send_with_refresh(|access| {
                self.http
                    .patch(url.clone())
                    .bearer_auth(access)
                    .header("content-type", "application/json")
                    .body(json.clone())
            })
            .await?;
        decode_json(response).await
    }

    /// DELETE with no body, no response payload. Returns Ok(()) on
    /// any 2xx (including the typical 204 No Content).
    pub async fn delete_request(&self, path: &str) -> GoogleResult<()> {
        let url = self.build_url(path)?;
        let response = self
            .send_with_refresh(|access| self.http.delete(url.clone()).bearer_auth(access))
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(GoogleError::Http {
            status: status.as_u16(),
            message: text.chars().take(300).collect(),
        })
    }

    fn build_url(&self, path_and_query: &str) -> GoogleResult<Url> {
        Url::parse(&format!("{}{}", self.api_base, path_and_query))
            .map_err(|e| GoogleError::Config(format!("bad url: {e}")))
    }

    /// Common send-then-refresh-on-401 wrapper. Builds the request
    /// twice (once with current access token, once with a refreshed
    /// one) using a caller-provided closure. The closure receives
    /// the bearer token to attach and returns a `RequestBuilder` —
    /// keeping body construction in the caller so we don't have to
    /// genericise over body types here.
    ///
    /// `pub(crate)` so the tasks module can drive its own
    /// absolute-URL flow without duplicating the refresh logic. The
    /// public Calendar surface still uses the convenience wrappers
    /// above (`get_json`, `post_json`, …) — this lower-level entry
    /// stays internal because it requires the caller to build the
    /// full RequestBuilder themselves.
    pub(crate) async fn send_with_refresh<F>(&self, build: F) -> GoogleResult<reqwest::Response>
    where
        F: Fn(&str) -> reqwest::RequestBuilder,
    {
        let access = self.tokens.lock().await.access_token.clone();
        let response = build(&access).send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.refresh_access_token().await?;
            let new_access = self.tokens.lock().await.access_token.clone();
            return Ok(build(&new_access).send().await?);
        }
        Ok(response)
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
        let fresh = auth::refresh(
            &self.client_id,
            &self.client_secret,
            &self.token_url,
            &refresh,
            &self.http,
        )
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

/// Decode the response body as JSON, surfacing HTTP errors as
/// `GoogleError::Http`. `pub(crate)` for the tasks module — same
/// rationale as `send_with_refresh` above.
pub(crate) async fn decode_json<T: DeserializeOwned>(
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
    serde_json::from_str(&text).map_err(|e| GoogleError::Protocol(format!("json: {e}: {text}")))
}

// ── Absolute-URL helpers shared across modules ─────────────────────────
//
// Calendar's REST surface is under `googleapis.com/calendar/v3`,
// Tasks under `tasks.googleapis.com/tasks/v1`, People under
// `people.googleapis.com/v1`. The `ApiState::get_json` etc.
// shortcuts assume the Calendar base URL; modules targeting the
// other hosts build absolute URLs and route through these.

/// GET an absolute URL, decode the JSON body, transparent refresh
/// on 401.
pub(crate) async fn get_absolute<T: DeserializeOwned>(
    state: &ApiState,
    url: &str,
) -> GoogleResult<T> {
    let url_owned = url.to_string();
    let response = state
        .send_with_refresh(|access| state.http.get(&url_owned).bearer_auth(access))
        .await?;
    decode_json(response).await
}

/// POST a JSON body to an absolute URL, decode the JSON response.
pub(crate) async fn post_absolute<B: Serialize, T: DeserializeOwned>(
    state: &ApiState,
    url: &str,
    body: &B,
) -> GoogleResult<T> {
    let url_owned = url.to_string();
    let json = serde_json::to_string(body)?;
    let response = state
        .send_with_refresh(|access| {
            state
                .http
                .post(&url_owned)
                .bearer_auth(access)
                .header("content-type", "application/json")
                .body(json.clone())
        })
        .await?;
    decode_json(response).await
}

/// PATCH a JSON body to an absolute URL, decode the JSON response.
pub(crate) async fn patch_absolute<B: Serialize, T: DeserializeOwned>(
    state: &ApiState,
    url: &str,
    body: &B,
) -> GoogleResult<T> {
    let url_owned = url.to_string();
    let json = serde_json::to_string(body)?;
    let response = state
        .send_with_refresh(|access| {
            state
                .http
                .patch(&url_owned)
                .bearer_auth(access)
                .header("content-type", "application/json")
                .body(json.clone())
        })
        .await?;
    decode_json(response).await
}

/// PUT a JSON body to an absolute URL, decode the JSON response.
/// ContactGroup updates use PUT (not PATCH) per the People API
/// reference.
pub(crate) async fn put_absolute<B: Serialize, T: DeserializeOwned>(
    state: &ApiState,
    url: &str,
    body: &B,
) -> GoogleResult<T> {
    let url_owned = url.to_string();
    let json = serde_json::to_string(body)?;
    let response = state
        .send_with_refresh(|access| {
            state
                .http
                .put(&url_owned)
                .bearer_auth(access)
                .header("content-type", "application/json")
                .body(json.clone())
        })
        .await?;
    decode_json(response).await
}

/// DELETE an absolute URL. Returns Ok(()) on any 2xx (typically 200
/// for People-API contact deletes, 204 for the rest).
pub(crate) async fn delete_absolute(state: &ApiState, url: &str) -> GoogleResult<()> {
    let url_owned = url.to_string();
    let response = state
        .send_with_refresh(|access| state.http.delete(&url_owned).bearer_auth(access))
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let text = response.text().await.unwrap_or_default();
    Err(GoogleError::Http {
        status: status.as_u16(),
        message: text.chars().take(300).collect(),
    })
}

/// GET an absolute URL with the OAuth bearer attached and return
/// the raw response bytes. Used by the photo-fetch path — Google
/// returns photo URLs (CDN endpoints) rather than inline bytes,
/// and we need to download the binary on the user's behalf.
pub(crate) async fn get_absolute_bytes(
    state: &ApiState,
    url: &str,
) -> GoogleResult<(Vec<u8>, Option<String>)> {
    let url_owned = url.to_string();
    let response = state
        .send_with_refresh(|access| state.http.get(&url_owned).bearer_auth(access))
        .await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if !status.is_success() {
        return Err(GoogleError::Http {
            status: status.as_u16(),
            message: format!("photo fetch returned status {status}"),
        });
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| GoogleError::Network(e.to_string()))?
        .to_vec();
    Ok((bytes, content_type))
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

/// `POST /calendars/{id}/events` — create a new event.
pub async fn create_event(
    state: &ApiState,
    calendar_id: &str,
    new: NewEvent,
) -> GoogleResult<Event> {
    let cal_enc = urlencoding(calendar_id);
    let path = format!("/calendars/{cal_enc}/events");
    let body = new_event_to_body(&new);
    let entry: EventEntry = state.post_json(&path, &body).await?;
    map_event(entry, calendar_id)?
        .ok_or_else(|| GoogleError::Protocol("create returned cancelled event".into()))
}

/// `PATCH /calendars/{id}/events/{eventId}` — update an existing
/// event. Google accepts a partial body, but we send the full
/// user-visible state so the local copy and the server stay in
/// step without diffing.
pub async fn update_event(state: &ApiState, ev: &Event) -> GoogleResult<Event> {
    let cal_enc = urlencoding(&ev.calendar_id);
    let ev_enc = urlencoding(&ev.id);
    let path = format!("/calendars/{cal_enc}/events/{ev_enc}");
    let body = event_to_body(ev);
    let entry: EventEntry = state.patch_json(&path, &body).await?;
    map_event(entry, &ev.calendar_id)?
        .ok_or_else(|| GoogleError::Protocol("update returned cancelled event".into()))
}

/// `DELETE /calendars/{id}/events/{eventId}` — delete an entire
/// event row (or a single non-recurring instance).
pub async fn delete_event(state: &ApiState, calendar_id: &str, event_id: &str) -> GoogleResult<()> {
    let cal_enc = urlencoding(calendar_id);
    let ev_enc = urlencoding(event_id);
    let path = format!("/calendars/{cal_enc}/events/{ev_enc}");
    state.delete_request(&path).await
}

/// Fetch the recurring master, append `occurrence` to its EXDATE
/// list, PATCH back. Used for Aperio's "delete only this
/// occurrence" flow on a recurring series.
pub async fn add_event_exdate(
    state: &ApiState,
    calendar_id: &str,
    event_id: &str,
    occurrence: chrono::DateTime<chrono::Utc>,
) -> GoogleResult<()> {
    let cal_enc = urlencoding(calendar_id);
    let ev_enc = urlencoding(event_id);
    let path = format!("/calendars/{cal_enc}/events/{ev_enc}");
    let entry: EventEntry = state.get_json(&path).await?;
    let mut master = map_event(entry, calendar_id)?.ok_or_else(|| {
        GoogleError::Protocol("cannot add EXDATE to a cancelled / missing master".into())
    })?;
    // Append the new exception, keeping any existing ones. The
    // RRULE itself is unchanged.
    let mut rec = master
        .recurrence
        .unwrap_or_else(|| cal_core::EventRecurrence {
            rrule: String::new(),
            exceptions: Vec::new(),
        });
    rec.exceptions.push(occurrence);
    master.recurrence = Some(rec);
    let body = event_to_body(&master);
    let _: EventEntry = state.patch_json(&path, &body).await?;
    Ok(())
}

/// `PATCH /calendars/{id}` with `{ "summary": "..." }`. Google's
/// calendar-rename endpoint; counterpart to CalDAV's PROPPATCH
/// `displayname`.
pub async fn rename_calendar(
    state: &ApiState,
    calendar_id: &str,
    new_name: &str,
) -> GoogleResult<()> {
    let cal_enc = urlencoding(calendar_id);
    let path = format!("/calendars/{cal_enc}");
    let body = serde_json::json!({ "summary": new_name });
    // We don't care about the response body; serde_json::Value is
    // a fine generic catch-all for "decode whatever Google sent".
    let _: serde_json::Value = state.patch_json(&path, &body).await?;
    Ok(())
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
            client_secret: "test-secret".into(),
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
            .match_body(mockito::Matcher::Regex("grant_type=refresh_token".into()))
            .with_status(200)
            .with_body(r#"{"access_token":"new-token","expires_in":3600,"token_type":"Bearer"}"#)
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
    async fn create_event_posts_json_and_maps_response() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/calendars/primary/events")
            .match_header("authorization", "Bearer initial-token")
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::Regex("\"summary\":\"Standup\"".into()))
            .with_status(200)
            .with_body(
                r##"{
                  "id": "abc123",
                  "summary": "Standup",
                  "start": { "dateTime": "2026-05-25T10:00:00Z" },
                  "end":   { "dateTime": "2026-05-25T10:30:00Z" }
                }"##,
            )
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        let new = NewEvent {
            title: "Standup".into(),
            description: None,
            location: None,
            start: chrono::Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap(),
            end: chrono::Utc
                .with_ymd_and_hms(2026, 5, 25, 10, 30, 0)
                .unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
        };
        let ev = create_event(&state, "primary", new).await.unwrap();
        assert_eq!(ev.id, "abc123");
        assert_eq!(ev.title, "Standup");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn delete_event_204_is_ok() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("DELETE", "/calendars/primary/events/abc123")
            .with_status(204)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        delete_event(&state, "primary", "abc123").await.unwrap();
    }

    #[tokio::test]
    async fn rename_calendar_patches_summary() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("PATCH", "/calendars/primary")
            .match_body(mockito::Matcher::Regex("\"summary\":\"Arbeit\"".into()))
            .with_status(200)
            .with_body(r##"{"id":"primary","summary":"Arbeit"}"##)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        rename_calendar(&state, "primary", "Arbeit").await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn add_event_exdate_fetches_patches_with_new_exception() {
        let mut server = mockito::Server::new_async().await;
        // GET the master.
        let get_mock = server
            .mock("GET", "/calendars/primary/events/master-1")
            .with_status(200)
            .with_body(
                r##"{
                  "id": "master-1",
                  "summary": "Weekly",
                  "start": { "dateTime": "2026-05-25T18:00:00Z" },
                  "end":   { "dateTime": "2026-05-25T19:00:00Z" },
                  "recurrence": ["RRULE:FREQ=WEEKLY"]
                }"##,
            )
            .create_async()
            .await;
        // PATCH with the new EXDATE in the recurrence body.
        let patch_mock = server
            .mock("PATCH", "/calendars/primary/events/master-1")
            .match_body(mockito::Matcher::Regex(
                "EXDATE;VALUE=DATE-TIME:20260601T180000Z".into(),
            ))
            .with_status(200)
            .with_body(
                r##"{
                "id": "master-1",
                "summary": "Weekly",
                "start": { "dateTime": "2026-05-25T18:00:00Z" },
                "end":   { "dateTime": "2026-05-25T19:00:00Z" }
            }"##,
            )
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        let occ = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 18, 0, 0).unwrap();
        add_event_exdate(&state, "primary", "master-1", occ)
            .await
            .unwrap();
        get_mock.assert_async().await;
        patch_mock.assert_async().await;
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
