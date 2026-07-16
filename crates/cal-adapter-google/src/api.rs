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

use cal_core::{AttendeeStatus, Calendar, DateRange, Event, FreeBusy, FreeBusySlot, NewEvent};
use chrono::{DateTime, Duration, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};
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
        debug!(method = "GET", path = path_and_query, "google request");
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
        debug!(method = "POST", path, "google request");
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
        debug!(method = "PATCH", path, "google request");
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
        debug!(method = "DELETE", path, "google request");
        let url = self.build_url(path)?;
        let response = self
            .send_with_refresh(|access| self.http.delete(url.clone()).bearer_auth(access))
            .await?;
        let status = response.status();
        debug!(
            method = "DELETE",
            path,
            status = status.as_u16(),
            "google response"
        );
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        let message: String = text.chars().take(300).collect();
        warn!(
            method = "DELETE",
            path,
            status = status.as_u16(),
            body = %message,
            "google request failed"
        );
        Err(GoogleError::Http {
            status: status.as_u16(),
            message,
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
        let message: String = text.chars().take(300).collect();
        // Log the body — Google's error payload says WHY (e.g. "Invalid resource id
        // value") and is otherwise lost inside the propagated error.
        warn!(status = status.as_u16(), body = %message, "google request failed");
        return Err(GoogleError::Http {
            status: status.as_u16(),
            message,
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
    debug!(method = "GET", url, "google request");
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
    debug!(method = "POST", url, "google request");
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
    debug!(method = "PATCH", url, "google request");
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
    debug!(method = "PUT", url, "google request");
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
    debug!(method = "DELETE", url, "google request");
    let url_owned = url.to_string();
    let response = state
        .send_with_refresh(|access| state.http.delete(&url_owned).bearer_auth(access))
        .await?;
    let status = response.status();
    debug!(
        method = "DELETE",
        url,
        status = status.as_u16(),
        "google response"
    );
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
    // The non-delta read path discards the sync token; the delta path
    // (`list_events_full`) keeps it.
    Ok(list_events_full(state, calendar_id, start, end).await?.0)
}

/// Full window sync: page every live event in `[start, end)` AND capture
/// the `nextSyncToken` Google returns on the last page. The token seeds
/// the incremental [`list_events_incremental`] path so subsequent
/// refreshes only pull the delta.
///
/// `showDeleted=true` so ALREADY-cancelled recurring instances are fetched: the
/// master's RRULE still generates their slots, so `map_event` turns each into a
/// cancelled RECURRENCE-ID override that suppresses the master occurrence (the
/// show-cancelled filter then hides it). Whole-event tombstones map to `None` and
/// are simply absent from the live set. The token bakes in `showDeleted` +
/// `timeMin`/`timeMax`, so the incremental leg inherits them without re-sending
/// (they're incompatible with `syncToken`).
pub async fn list_events_full(
    state: &ApiState,
    calendar_id: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> GoogleResult<(Vec<Event>, Option<String>)> {
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    let mut sync_token: Option<String> = None;
    let cal_enc = urlencoding(calendar_id);
    loop {
        let mut path = format!(
            "/calendars/{cal_enc}/events?singleEvents=false&showDeleted=true\
             &maxResults=2500&timeMin={tm}&timeMax={tx}",
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
        // The token only appears on the final page.
        if resp.next_sync_token.is_some() {
            sync_token = resp.next_sync_token;
        }
        match resp.next_page_token {
            Some(t) => page_token = Some(t),
            None => break,
        }
    }
    Ok((out, sync_token))
}

/// Outcome of one incremental sync round: upserted/created rows, the
/// native ids of removed rows, and the refreshed token for the next call.
#[derive(Debug, Default)]
pub struct EventDelta {
    pub changes: Vec<Event>,
    pub deletions: Vec<String>,
    pub new_token: Option<String>,
}

/// Incremental sync via `syncToken`. Returns only what changed since the
/// token was issued: created/updated events in `changes`, the ids of
/// deleted (Google `status: "cancelled"`) events in `deletions`, and the
/// fresh `nextSyncToken`.
///
/// A `410 Gone` surfaces as `GoogleError::Http { status: 410, .. }` — the
/// caller drops the token and re-runs a full sync.
///
/// `singleEvents=false` MUST match the full sync that issued the token.
/// `timeMin`/`timeMax` are deliberately omitted — they're incompatible
/// with `syncToken`, and the window is already baked into it. Created /
/// updated singles are range-filtered to the cache's window (`[start,
/// end)`); recurring masters always pass, matching the full read and the
/// EWS adapter. Deletions are NOT range-filtered — a cancelled row often
/// carries no usable start/end, and removing an id that isn't cached is a
/// harmless no-op host-side.
pub async fn list_events_incremental(
    state: &ApiState,
    calendar_id: &str,
    sync_token: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> GoogleResult<EventDelta> {
    let mut delta = EventDelta::default();
    let mut page_token: Option<String> = None;
    let cal_enc = urlencoding(calendar_id);
    loop {
        let mut path = format!(
            "/calendars/{cal_enc}/events?singleEvents=false&maxResults=2500\
             &syncToken={st}",
            st = urlencoding(sync_token),
        );
        if let Some(t) = &page_token {
            path.push_str("&pageToken=");
            path.push_str(&urlencoding(t));
        }
        let resp: EventListResponse = state.get_json(&path).await?;
        for entry in resp.items {
            // A cancelled WHOLE event is a tombstone → delete by its native id. A
            // cancelled recurring INSTANCE instead flows through `map_event`, which
            // turns it into a `cancelled` RECURRENCE-ID override kept in `changes`
            // so it SUPPRESSES (and hides) the master's now-deleted occurrence —
            // deleting it would let the master's slot ghost back.
            if entry.status.as_deref() == Some("cancelled") && entry.recurring_event_id.is_none() {
                delta.deletions.push(entry.id);
                continue;
            }
            if let Some(ev) = map_event(entry, calendar_id)? {
                // A cancelled recurring-instance override is a SUPPRESSION marker,
                // not a displayable event: it MUST reach the cache so the master's
                // now-deleted occurrence stays hidden — even when the override's own
                // (zero-duration, often PAST-dated) slot falls outside the sync
                // window. Range-filtering it here, as we do for real singles to keep
                // the cache lean, dropped it while the token still advanced past the
                // cancellation, so the deleted occurrence ghosted back and only a
                // full resync could recover it. It carries its OWN native id (the
                // `::rid::` id, not the master's), so keeping it never purges the
                // master group in apply_events_delta.
                if ev.cancelled || event_in_window(&ev, start, end) {
                    delta.changes.push(ev);
                }
            }
        }
        if resp.next_sync_token.is_some() {
            delta.new_token = resp.next_sync_token;
        }
        match resp.next_page_token {
            Some(t) => page_token = Some(t),
            None => break,
        }
    }
    Ok(delta)
}

/// Window predicate shared by the incremental sync: recurring masters
/// always pass (the frontend expander handles the visible window);
/// singles must overlap `[start, end)`.
fn event_in_window(
    ev: &Event,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> bool {
    ev.recurrence.is_some() || (ev.end >= start && ev.start < end)
}

/// `POST /calendars/{id}/events` — create a new event.
pub async fn create_event(
    state: &ApiState,
    calendar_id: &str,
    new: NewEvent,
) -> GoogleResult<Event> {
    let cal_enc = urlencoding(calendar_id);
    // `sendUpdates=all` makes Google email the attendees; `none` stores
    // them silently. Only notify when the user opted in and there's
    // someone to notify.
    let su = send_updates_param(new.send_invitations && !new.attendees.is_empty());
    let path = format!("/calendars/{cal_enc}/events?sendUpdates={su}");
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
    let su = send_updates_param(ev.send_invitations && !ev.attendees.is_empty());
    let path = format!("/calendars/{cal_enc}/events/{ev_enc}?sendUpdates={su}");
    let body = event_to_body(ev);
    let entry: EventEntry = state.patch_json(&path, &body).await?;
    map_event(entry, &ev.calendar_id)?
        .ok_or_else(|| GoogleError::Protocol("update returned cancelled event".into()))
}

/// `DELETE /calendars/{id}/events/{eventId}` — delete an entire
/// event row (or a single non-recurring instance).
pub async fn delete_event(
    state: &ApiState,
    calendar_id: &str,
    event_id: &str,
    send_cancellations: bool,
) -> GoogleResult<()> {
    let cal_enc = urlencoding(calendar_id);
    let ev_enc = urlencoding(event_id);
    let su = send_updates_param(send_cancellations);
    let path = format!("/calendars/{cal_enc}/events/{ev_enc}?sendUpdates={su}");
    state.delete_request(&path).await
}

/// Google's `sendUpdates` query value: `all` emails attendees, `none`
/// stores/deletes silently.
fn send_updates_param(notify: bool) -> &'static str {
    if notify {
        "all"
    } else {
        "none"
    }
}

/// `POST /freeBusy` — attendee availability. Returns one [`FreeBusy`] per
/// requested email the API answered for; an email Google can't see (no
/// permission / unknown) is simply omitted (the host reads that as
/// "couldn't determine").
pub async fn query_free_busy(
    state: &ApiState,
    emails: &[&str],
    range: DateRange,
) -> GoogleResult<Vec<FreeBusy>> {
    let body = FreeBusyQuery {
        time_min: range.start.to_rfc3339(),
        time_max: range.end.to_rfc3339(),
        items: emails.iter().map(|e| FreeBusyItem { id: e }).collect(),
    };
    let resp: FreeBusyResponse = state.post_json("/freeBusy", &body).await?;
    Ok(emails
        .iter()
        .filter_map(|email| {
            let cal = resp.calendars.get(*email)?;
            Some(FreeBusy {
                email: (*email).to_string(),
                slots: cal
                    .busy
                    .iter()
                    .map(|b| FreeBusySlot {
                        start: b.start,
                        end: b.end,
                    })
                    .collect(),
            })
        })
        .collect())
}

/// The connected account's email. On Google, the **primary calendar's
/// id IS the user's address**, so a single GET on `/calendars/primary`
/// yields the identity without needing the `userinfo` scope.
pub async fn current_user_email(state: &ApiState) -> GoogleResult<Option<String>> {
    #[derive(Deserialize)]
    struct PrimaryCalendar {
        id: String,
    }
    let cal: PrimaryCalendar = state.get_json("/calendars/primary").await?;
    Ok(cal.id.contains('@').then_some(cal.id))
}

/// RSVP to an event. Google has no dedicated RSVP endpoint: we PATCH the
/// event's `attendees`, flipping the connected user's `responseStatus`,
/// with `sendUpdates` controlling whether the organizer is emailed. The
/// trait signature doesn't carry the calendar id, so we walk the
/// listing — same approach as [`super::delete_event`]'s caller — GET each
/// candidate event by id and patch the one that resolves.
pub async fn respond_to_event(
    state: &ApiState,
    event_id: &str,
    status: AttendeeStatus,
    send_response: bool,
) -> GoogleResult<()> {
    let resp_status = match status {
        AttendeeStatus::Accepted => "accepted",
        AttendeeStatus::Declined => "declined",
        AttendeeStatus::Tentative => "tentative",
        AttendeeStatus::NeedsAction => {
            return Err(GoogleError::Protocol(
                "cannot RSVP with status needs-action".into(),
            ));
        }
    };
    let me = current_user_email(state).await?.ok_or_else(|| {
        GoogleError::Protocol("cannot RSVP without knowing the account's own email".into())
    })?;

    let ev_enc = urlencoding(event_id);
    let su = send_updates_param(send_response);
    let mut last_err: Option<GoogleError> = None;
    for cal in list_calendars(state).await? {
        let cal_enc = urlencoding(&cal.id);
        let event: EventEntry = match state
            .get_json(&format!("/calendars/{cal_enc}/events/{ev_enc}"))
            .await
        {
            Ok(e) => e,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };
        // Replace the whole attendees array (Google's PATCH semantics for
        // this field), flipping only our own row's responseStatus.
        let attendees: Vec<AttendeePatch> = event
            .attendees
            .into_iter()
            .filter_map(|a| {
                let email = a.email?;
                let is_self = email.eq_ignore_ascii_case(&me);
                Some(AttendeePatch {
                    response_status: if is_self {
                        Some(resp_status.to_string())
                    } else {
                        a.response_status
                    },
                    email,
                })
            })
            .collect();
        let path = format!("/calendars/{cal_enc}/events/{ev_enc}?sendUpdates={su}");
        let _: EventEntry = state
            .patch_json(&path, &EventAttendeesPatch { attendees })
            .await?;
        return Ok(());
    }
    Err(last_err.unwrap_or_else(|| GoogleError::Http {
        status: 404,
        message: format!("event '{event_id}' not found in any calendar"),
    }))
}

#[derive(Serialize)]
struct AttendeePatch {
    email: String,
    #[serde(rename = "responseStatus", skip_serializing_if = "Option::is_none")]
    response_status: Option<String>,
}

#[derive(Serialize)]
struct EventAttendeesPatch {
    attendees: Vec<AttendeePatch>,
}

#[derive(Serialize)]
struct FreeBusyQuery<'a> {
    #[serde(rename = "timeMin")]
    time_min: String,
    #[serde(rename = "timeMax")]
    time_max: String,
    items: Vec<FreeBusyItem<'a>>,
}

#[derive(Serialize)]
struct FreeBusyItem<'a> {
    id: &'a str,
}

#[derive(Deserialize)]
struct FreeBusyResponse {
    #[serde(default)]
    calendars: std::collections::HashMap<String, FreeBusyCalendar>,
}

#[derive(Deserialize)]
struct FreeBusyCalendar {
    #[serde(default)]
    busy: Vec<BusyPeriod>,
}

#[derive(Deserialize)]
struct BusyPeriod {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

/// The result of asking one calendar to cancel a single occurrence.
#[derive(Debug)]
pub enum ExdateOutcome {
    /// The occurrence is cancelled on the server — deleted just now, or it was
    /// already cancelled (a re-delete), which we treat as an idempotent success.
    Cancelled,
    /// The master isn't on this calendar, so the caller's calendar-walk should
    /// keep looking rather than surfacing an error.
    MasterNotHere,
}

/// Delete a single occurrence of a recurring event (Aperio's "delete only this
/// occurrence" flow).
///
/// Google models a per-occurrence deletion as a cancelled INSTANCE resource, NOT
/// as an EXDATE on the master. Patching the master's `recurrence` with a UTC
/// EXDATE is silently dropped for a zoned series (RFC 5545 wants the EXDATE in the
/// DTSTART zone) and isn't Google's mechanism anyway, so it was a no-op. Instead
/// we DELETE the instance `{master}_{originalStart}` — the exact id shape
/// [`map_event`] turns back into a `{master}::rid::{start}` cancelled override, so
/// the next read suppresses the occurrence. Never rewrites the master.
pub async fn add_event_exdate(
    state: &ApiState,
    calendar_id: &str,
    event_id: &str,
    occurrence: DateTime<Utc>,
) -> GoogleResult<ExdateOutcome> {
    let cal_enc = urlencoding(calendar_id);
    let ev_enc = urlencoding(event_id);
    // The master's presence on THIS calendar tells "wrong calendar" (keep walking)
    // apart from a real failure (surface it) — a 404 here is the only thing that
    // lets the walk continue; anything else propagates.
    match state
        .get_json::<EventEntry>(&format!("/calendars/{cal_enc}/events/{ev_enc}"))
        .await
    {
        Ok(_) => {}
        Err(GoogleError::Http { status: 404, .. }) => return Ok(ExdateOutcome::MasterNotHere),
        Err(e) => return Err(e),
    }
    // Cancel one occurrence the way Google documents: expand the master over the
    // occurrence's day and cancel the instance id GOOGLE returns — do NOT build
    // `{master}_{time}` ourselves. A self-constructed instance id is rejected with
    // HTTP 400 ("invalid resource id") even when the UTC instant is correct; only an
    // id the API handed back is a valid address. Google returns each instance's
    // start in the EVENT's zone (e.g. `2026-07-23T13:00:00+02:00`), so we match on
    // the parsed-to-UTC instant, never the wall-clock digits. `showDeleted=true`
    // keeps an already-cancelled slot visible so a re-delete stays idempotent.
    let lo = occurrence - Duration::days(1);
    let hi = occurrence + Duration::days(1);
    let path = format!(
        "/calendars/{cal_enc}/events/{ev_enc}/instances\
         ?showDeleted=true&maxResults=250&timeMin={}&timeMax={}",
        urlencoding(&lo.to_rfc3339()),
        urlencoding(&hi.to_rfc3339()),
    );
    let resp: EventListResponse = state.get_json(&path).await?;
    // Nearest instance by resolved (offset-parsed) start to the wanted occurrence.
    let target = resp
        .items
        .iter()
        .filter_map(|it| {
            let src = it.original_start_time.as_ref().unwrap_or(&it.start);
            let (got, _) = src.resolve().ok()?;
            Some((it, (got - occurrence).num_seconds().abs()))
        })
        .min_by_key(|(_, dist)| *dist);
    let matched = target.map(|(it, dist)| format!("{} (delta {dist}s)", it.id));
    debug!(
        event_id,
        occurrence = %occurrence,
        instances = resp.items.len(),
        ?matched,
        "add_event_exdate: instance lookup"
    );
    let Some((instance, _)) = target else {
        // No instance for that day — already cancelled or never materialised; the
        // occurrence is gone either way, so a delete is an idempotent success.
        return Ok(ExdateOutcome::Cancelled);
    };
    if instance.status.as_deref() == Some("cancelled") {
        return Ok(ExdateOutcome::Cancelled);
    }
    // Cancel via PATCH status:"cancelled" on the id GOOGLE returned — its documented
    // "retrieve the instance then update it" path. The id is guaranteed valid, so no
    // 400; `map_event` reads the resulting cancelled instance back as a
    // `{master}::rid::{start}` override that suppresses the occurrence on next read.
    let inst_enc = urlencoding(&instance.id);
    let patch_path = format!("/calendars/{cal_enc}/events/{inst_enc}?sendUpdates=none");
    match state
        .patch_json::<_, serde_json::Value>(
            &patch_path,
            &serde_json::json!({ "status": "cancelled" }),
        )
        .await
    {
        Ok(_) => Ok(ExdateOutcome::Cancelled),
        // Already gone → idempotent success.
        Err(GoogleError::Http {
            status: 404 | 410, ..
        }) => Ok(ExdateOutcome::Cancelled),
        Err(e) => Err(e),
    }
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
    async fn respond_to_event_patches_self_attendee_status() {
        let mut server = mockito::Server::new_async().await;
        // current_user_email → primary calendar id is the account email.
        server
            .mock("GET", "/calendars/primary")
            .with_status(200)
            .with_body(r#"{"id": "me@gmail.com"}"#)
            .create_async()
            .await;
        // list_calendars → one calendar ("primary").
        server
            .mock("GET", "/users/me/calendarList")
            .with_status(200)
            .with_body(r#"{"items":[{"id":"primary","summary":"Me","accessRole":"owner"}]}"#)
            .create_async()
            .await;
        // GET the event to read its attendees.
        server
            .mock("GET", "/calendars/primary/events/EV-1")
            .with_status(200)
            .with_body(
                r#"{"id":"EV-1","start":{"dateTime":"2026-05-25T10:00:00Z"},
                    "end":{"dateTime":"2026-05-25T11:00:00Z"},
                    "attendees":[
                      {"email":"boss@example.com","responseStatus":"accepted"},
                      {"email":"me@gmail.com","responseStatus":"needsAction"}
                    ]}"#,
            )
            .create_async()
            .await;
        // PATCH flips our row to "declined" and notifies (sendUpdates=all).
        let patch = server
            .mock("PATCH", "/calendars/primary/events/EV-1?sendUpdates=all")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex(r#""email":"me@gmail.com""#.into()),
                mockito::Matcher::Regex(r#""responseStatus":"declined""#.into()),
                // The other attendee's accepted status is preserved.
                mockito::Matcher::Regex(r#""responseStatus":"accepted""#.into()),
            ]))
            .with_status(200)
            .with_body(r#"{"id":"EV-1","start":{"dateTime":"2026-05-25T10:00:00Z"},"end":{"dateTime":"2026-05-25T11:00:00Z"}}"#)
            .expect(1)
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        respond_to_event(&state, "EV-1", AttendeeStatus::Declined, true)
            .await
            .unwrap();
        patch.assert_async().await;
    }

    #[tokio::test]
    async fn current_user_email_reads_primary_calendar_id() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/calendars/primary")
            .with_status(200)
            .with_body(r#"{"id": "alice@gmail.com", "summary": "alice@gmail.com"}"#)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let email = current_user_email(&state).await.unwrap();
        assert_eq!(email.as_deref(), Some("alice@gmail.com"));
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
                    r"^/calendars/primary/events\?.*singleEvents=false.*showDeleted=true.*timeMin=.*timeMax="
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
    async fn list_events_full_captures_sync_token() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"^/calendars/primary/events\?.*timeMin=".to_string()),
            )
            .with_status(200)
            .with_body(
                r##"{
                  "items": [
                    {"id":"e1","summary":"One",
                     "start":{"dateTime":"2026-05-10T08:00:00Z"},
                     "end":{"dateTime":"2026-05-10T09:00:00Z"}}
                  ],
                  "nextSyncToken":"TOK-1"
                }"##,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let from = chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let (evs, tok) = list_events_full(&state, "primary", from, to).await.unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(tok.as_deref(), Some("TOK-1"));
    }

    #[tokio::test]
    async fn list_events_incremental_splits_changes_and_deletions() {
        let mut server = mockito::Server::new_async().await;
        // The incremental request must carry syncToken and must NOT send
        // timeMin/timeMax (incompatible with a sync token).
        let m = server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/calendars/primary/events\?.*syncToken=TOK-1".to_string(),
                ),
            )
            .with_status(200)
            .with_body(
                r##"{
                  "items": [
                    {"id":"e1","summary":"Updated",
                     "start":{"dateTime":"2026-05-10T08:00:00Z"},
                     "end":{"dateTime":"2026-05-10T09:00:00Z"}},
                    {"id":"e2","status":"cancelled",
                     "start":{"dateTime":"2026-05-11T08:00:00Z"},
                     "end":{"dateTime":"2026-05-11T09:00:00Z"}},
                    {"id":"e3","summary":"OutOfWindow",
                     "start":{"dateTime":"2030-01-01T08:00:00Z"},
                     "end":{"dateTime":"2030-01-01T09:00:00Z"}}
                  ],
                  "nextSyncToken":"TOK-2"
                }"##,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let from = chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let delta = list_events_incremental(&state, "primary", "TOK-1", from, to)
            .await
            .unwrap();
        // e1 in window → change; e3 out-of-window single → filtered;
        // e2 cancelled → deletion (its bare id).
        assert_eq!(
            delta
                .changes
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["e1"]
        );
        assert_eq!(delta.deletions, vec!["e2".to_string()]);
        assert_eq!(delta.new_token.as_deref(), Some("TOK-2"));
        m.assert_async().await;
    }

    #[tokio::test]
    async fn cancelled_instance_surfaces_as_override_whole_event_is_a_deletion() {
        // A cancelled recurring INSTANCE (has recurringEventId) must become a
        // `cancelled` RECURRENCE-ID override in `changes` so it suppresses the
        // master's slot — NOT a plain deletion, which would let the slot ghost
        // back. A cancelled WHOLE event (no recurringEventId) stays a deletion.
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/calendars/primary/events\?.*syncToken=TOK-1".to_string(),
                ),
            )
            .with_status(200)
            .with_body(
                r##"{
                  "items": [
                    {"id":"master-1_20260614T100000Z","status":"cancelled",
                     "recurringEventId":"master-1",
                     "originalStartTime":{"dateTime":"2026-06-14T10:00:00Z"}},
                    {"id":"whole-gone","status":"cancelled"}
                  ],
                  "nextSyncToken":"TOK-2"
                }"##,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let from = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let delta = list_events_incremental(&state, "primary", "TOK-1", from, to)
            .await
            .unwrap();
        // The instance is a suppressing cancelled override…
        let override_ids: Vec<_> = delta.changes.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(override_ids, ["master-1::rid::2026-06-14T10:00:00Z"]);
        assert!(delta.changes[0].cancelled);
        // …and only the whole-event cancellation is a deletion.
        assert_eq!(delta.deletions, vec!["whole-gone".to_string()]);
        m.assert_async().await;
    }

    #[tokio::test]
    async fn cancelled_instance_override_is_kept_even_when_out_of_window() {
        // A PAST-dated cancelled instance — its slot is BEFORE the sync window start
        // — must still be kept as a suppressing override. Range-filtering it (as we
        // do for real singles) dropped the cancellation while the token advanced
        // past it, so the deleted occurrence ghosted back until a full resync. The
        // override carries its own `::rid::` native id, so caching it never purges
        // the master group. (Root cause of Lea's Google occurrence-delete ghost.)
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/calendars/primary/events\?.*syncToken=TOK-1".to_string(),
                ),
            )
            .with_status(200)
            .with_body(
                r##"{
                  "items": [
                    {"id":"m_20260713T103000Z","status":"cancelled",
                     "recurringEventId":"m",
                     "originalStartTime":{"dateTime":"2026-07-13T10:30:00Z"}}
                  ],
                  "nextSyncToken":"TOK-2"
                }"##,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        // The window STARTS after the cancelled slot (07-13 is in the past).
        let from = chrono::Utc.with_ymd_and_hms(2026, 7, 16, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap();
        let delta = list_events_incremental(&state, "primary", "TOK-1", from, to)
            .await
            .unwrap();
        let ids: Vec<_> = delta.changes.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["m::rid::2026-07-13T10:30:00Z"]);
        assert!(delta.changes[0].cancelled);
        m.assert_async().await;
    }

    #[tokio::test]
    async fn list_events_incremental_410_bubbles_as_http() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"^/calendars/primary/events\?.*syncToken=".to_string()),
            )
            .with_status(410)
            .with_body("Sync token is no longer valid")
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let from = chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let err = list_events_incremental(&state, "primary", "STALE", from, to)
            .await
            .unwrap_err();
        assert!(matches!(err, GoogleError::Http { status: 410, .. }));
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
            .match_query(mockito::Matcher::Any)
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
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            send_invitations: false,
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
            .match_query(mockito::Matcher::Any)
            .with_status(204)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        delete_event(&state, "primary", "abc123", false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn query_free_busy_parses_busy_blocks() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/freeBusy")
            .match_body(mockito::Matcher::Regex(
                r#""id":"alice@example.com""#.into(),
            ))
            .with_status(200)
            .with_body(
                r##"{ "calendars": {
                    "alice@example.com": { "busy": [
                      { "start": "2026-05-25T10:00:00Z", "end": "2026-05-25T11:00:00Z" }
                    ] },
                    "bob@example.com": { "busy": [] }
                } }"##,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let range = DateRange::new(
            chrono::Utc.with_ymd_and_hms(2026, 5, 25, 9, 0, 0).unwrap(),
            chrono::Utc.with_ymd_and_hms(2026, 5, 25, 17, 0, 0).unwrap(),
        );
        let fb = query_free_busy(&state, &["alice@example.com", "bob@example.com"], range)
            .await
            .unwrap();
        m.assert_async().await;
        assert_eq!(fb.len(), 2);
        let alice = fb.iter().find(|f| f.email == "alice@example.com").unwrap();
        assert_eq!(alice.slots.len(), 1);
        assert_eq!(
            alice.slots[0].start.to_rfc3339(),
            "2026-05-25T10:00:00+00:00"
        );
        assert!(fb
            .iter()
            .find(|f| f.email == "bob@example.com")
            .unwrap()
            .slots
            .is_empty());
    }

    #[tokio::test]
    async fn create_with_attendees_and_notify_sets_send_updates_all() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/calendars/primary/events")
            .match_query(mockito::Matcher::UrlEncoded(
                "sendUpdates".into(),
                "all".into(),
            ))
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex(r#""email":"alice@example.com""#.into()),
                mockito::Matcher::Regex(r#""displayName":"Alice""#.into()),
            ]))
            .with_status(200)
            .with_body(
                r##"{ "id": "ev1", "summary": "Review",
                      "start": { "dateTime": "2026-05-25T10:00:00Z" },
                      "end":   { "dateTime": "2026-05-25T10:30:00Z" } }"##,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let new = NewEvent {
            title: "Review".into(),
            description: None,
            location: None,
            start: chrono::Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap(),
            end: chrono::Utc
                .with_ymd_and_hms(2026, 5, 25, 10, 30, 0)
                .unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec!["Alice <alice@example.com>".into()],
            send_invitations: true,
        };
        create_event(&state, "primary", new).await.unwrap();
        m.assert_async().await;
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
    async fn add_event_exdate_cancels_the_matched_instance_via_patch() {
        let mut server = mockito::Server::new_async().await;
        // GET master (the calendar-walk lands here).
        let get_master = server
            .mock("GET", "/calendars/primary/events/master-1")
            .with_status(200)
            .with_body(
                r##"{"id":"master-1","start":{"dateTime":"2026-05-25T18:00:00Z"},
                     "end":{"dateTime":"2026-05-25T19:00:00Z"},
                     "recurrence":["RRULE:FREQ=WEEKLY"]}"##,
            )
            .create_async()
            .await;
        // Expand the day → Google returns the instance in the EVENT'S ZONE (+02:00)
        // with its own id. We must match on the parsed-to-UTC instant (18:00Z), not
        // the wall-clock "20:00" — the bug that made the earlier lookup find nothing.
        let get_instances = server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/calendars/primary/events/master-1/instances\?.*showDeleted=true"
                        .to_string(),
                ),
            )
            .with_status(200)
            .with_body(
                r##"{"items":[
                  {"id":"master-1_20260601T180000Z","status":"confirmed",
                   "start":{"dateTime":"2026-06-01T20:00:00+02:00"},
                   "originalStartTime":{"dateTime":"2026-06-01T20:00:00+02:00"}}
                ]}"##,
            )
            .create_async()
            .await;
        // Cancel via PATCH status:cancelled on the id GOOGLE returned.
        let patch = server
            .mock(
                "PATCH",
                "/calendars/primary/events/master-1_20260601T180000Z?sendUpdates=none",
            )
            .match_body(mockito::Matcher::Regex(
                r#""status":"cancelled""#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"id":"master-1_20260601T180000Z","status":"cancelled"}"#)
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        let occ = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 18, 0, 0).unwrap();
        let outcome = add_event_exdate(&state, "primary", "master-1", occ)
            .await
            .unwrap();
        assert!(matches!(outcome, ExdateOutcome::Cancelled));
        get_master.assert_async().await;
        get_instances.assert_async().await;
        patch.assert_async().await;
    }

    #[tokio::test]
    async fn add_event_exdate_cancels_an_all_day_instance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/calendars/primary/events/bday-1")
            .with_status(200)
            .with_body(
                r##"{"id":"bday-1","start":{"date":"2026-05-25"},"end":{"date":"2026-05-26"},
                     "recurrence":["RRULE:FREQ=DAILY"]}"##,
            )
            .create_async()
            .await;
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/calendars/primary/events/bday-1/instances\?.*".to_string(),
                ),
            )
            .with_status(200)
            .with_body(
                r##"{"items":[
                  {"id":"bday-1_20260601","status":"confirmed",
                   "start":{"date":"2026-06-01"},
                   "originalStartTime":{"date":"2026-06-01"}}
                ]}"##,
            )
            .create_async()
            .await;
        let patch = server
            .mock(
                "PATCH",
                "/calendars/primary/events/bday-1_20260601?sendUpdates=none",
            )
            .with_status(200)
            .with_body(r#"{"id":"bday-1_20260601","status":"cancelled"}"#)
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        let occ = chrono::Local
            .with_ymd_and_hms(2026, 6, 1, 0, 0, 0)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let outcome = add_event_exdate(&state, "primary", "bday-1", occ)
            .await
            .unwrap();
        assert!(matches!(outcome, ExdateOutcome::Cancelled));
        patch.assert_async().await;
    }

    #[tokio::test]
    async fn add_event_exdate_is_idempotent_when_instance_already_cancelled() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/calendars/primary/events/master-2")
            .with_status(200)
            .with_body(
                r##"{"id":"master-2","start":{"dateTime":"2026-05-25T18:00:00Z"},
                     "end":{"dateTime":"2026-05-25T19:00:00Z"},
                     "recurrence":["RRULE:FREQ=WEEKLY"]}"##,
            )
            .create_async()
            .await;
        // The slot is ALREADY cancelled — showDeleted surfaces it — so we recognise
        // it and issue NO PATCH (no PATCH mock below, so a stray call would 501/panic).
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/calendars/primary/events/master-2/instances\?.*showDeleted=true"
                        .to_string(),
                ),
            )
            .with_status(200)
            .with_body(
                r##"{"items":[
                  {"id":"master-2_20260601T180000Z","status":"cancelled",
                   "originalStartTime":{"dateTime":"2026-06-01T18:00:00Z"}}
                ]}"##,
            )
            .create_async()
            .await;

        let state = fixture_state(&server.url());
        let occ = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 18, 0, 0).unwrap();
        let outcome = add_event_exdate(&state, "primary", "master-2", occ)
            .await
            .unwrap();
        assert!(matches!(outcome, ExdateOutcome::Cancelled));
    }

    #[tokio::test]
    async fn add_event_exdate_reports_master_not_here_on_404() {
        let mut server = mockito::Server::new_async().await;
        // The master isn't on THIS calendar → the walk should keep looking.
        server
            .mock("GET", "/calendars/other/events/master-3")
            .with_status(404)
            .with_body(r#"{"error":{"code":404,"message":"Not Found"}}"#)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let occ = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 18, 0, 0).unwrap();
        let outcome = add_event_exdate(&state, "other", "master-3", occ)
            .await
            .unwrap();
        assert!(matches!(outcome, ExdateOutcome::MasterNotHere));
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
