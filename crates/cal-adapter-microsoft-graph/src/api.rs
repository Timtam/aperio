//! Microsoft Graph REST client. Shape: same as the Google adapter
//! — `ApiState` carries the access token, the token endpoint URL,
//! the API base URL, and the client_id. `send_with_refresh` wraps
//! every request with a 401 → refresh-token-grant → retry-once
//! dance so callers don't have to think about expiry.

use std::sync::Arc;

use cal_core::{Calendar, Event, NewEvent, NewTask, Task, TaskList};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::warn;
use url::Url;

use crate::auth::{self, TokenSet};
use crate::error::{GraphError, GraphResult};
use crate::mapping::{
    event_to_body, map_calendar, map_event, map_task, map_task_list, new_event_to_body,
    new_task_to_body, split_task_id, task_to_body, CalendarListResponse, EventEntry,
    EventListResponse, TodoListResponse, TodoTaskEntry, TodoTaskResponse,
};

pub const API_BASE: &str = "https://graph.microsoft.com/v1.0";

#[derive(Debug, Clone)]
pub struct ApiState {
    pub tokens: Arc<Mutex<TokenSet>>,
    pub client_id: String,
    pub http: reqwest::Client,
    pub token_url: String,
    pub api_base: String,
}

impl ApiState {
    pub fn new(
        tokens: TokenSet,
        client_id: String,
        token_url: String,
        http: reqwest::Client,
    ) -> Self {
        Self {
            tokens: Arc::new(Mutex::new(tokens)),
            client_id,
            http,
            token_url,
            api_base: API_BASE.to_string(),
        }
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path_and_query: &str) -> GraphResult<T> {
        let url = self.build_url(path_and_query)?;
        let response = self
            .send_with_refresh(|access| self.http.get(url.clone()).bearer_auth(access))
            .await?;
        decode_json(response).await
    }

    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> GraphResult<T> {
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

    pub async fn patch_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> GraphResult<T> {
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

    pub async fn delete_request(&self, path: &str) -> GraphResult<()> {
        let url = self.build_url(path)?;
        let response = self
            .send_with_refresh(|access| self.http.delete(url.clone()).bearer_auth(access))
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(GraphError::Http {
            status: status.as_u16(),
            message: text.chars().take(300).collect(),
        })
    }

    /// GET a binary blob and return it together with the
    /// `Content-Type` header. Used by Phase 10i for contact photo
    /// downloads (`/me/contacts/{id}/photo/$value` returns raw
    /// image bytes, not JSON).
    pub async fn get_bytes(&self, path: &str) -> GraphResult<(Vec<u8>, Option<String>)> {
        let url = self.build_url(path)?;
        let response = self
            .send_with_refresh(|access| self.http.get(url.clone()).bearer_auth(access))
            .await?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let bytes = response.bytes().await.map_err(GraphError::from)?.to_vec();
        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).to_string();
            return Err(GraphError::Http {
                status: status.as_u16(),
                message: preview,
            });
        }
        Ok((bytes, content_type))
    }

    /// PUT raw bytes with a chosen `Content-Type`. Used by Phase
    /// 10i for contact photo uploads — Graph expects the image
    /// payload as the request body, no base64 wrapping.
    pub async fn put_bytes(
        &self,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> GraphResult<()> {
        let url = self.build_url(path)?;
        let response = self
            .send_with_refresh(|access| {
                self.http
                    .put(url.clone())
                    .bearer_auth(access)
                    .header("content-type", content_type)
                    .body(body.clone())
            })
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(GraphError::Http {
            status: status.as_u16(),
            message: text.chars().take(300).collect(),
        })
    }

    fn build_url(&self, path_and_query: &str) -> GraphResult<Url> {
        // Graph paginates via `@odata.nextLink` which is an
        // absolute URL — allow callers to pass either a relative
        // path or a full link.
        if path_and_query.starts_with("http://") || path_and_query.starts_with("https://") {
            Url::parse(path_and_query).map_err(|e| GraphError::Config(format!("bad url: {e}")))
        } else {
            Url::parse(&format!("{}{}", self.api_base, path_and_query))
                .map_err(|e| GraphError::Config(format!("bad url: {e}")))
        }
    }

    async fn send_with_refresh<F>(&self, build: F) -> GraphResult<reqwest::Response>
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

    async fn refresh_access_token(&self) -> GraphResult<()> {
        let refresh = self
            .tokens
            .lock()
            .await
            .refresh_token
            .clone()
            .ok_or_else(|| GraphError::Http {
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
        if let Some(rt) = fresh.refresh_token {
            guard.refresh_token = Some(rt);
        }
        guard.expires_at = fresh.expires_at;
        Ok(())
    }
}

async fn decode_json<T: DeserializeOwned>(response: reqwest::Response) -> GraphResult<T> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(GraphError::Http {
            status: status.as_u16(),
            message: text.chars().take(300).collect(),
        });
    }
    serde_json::from_str(&text).map_err(|e| GraphError::Protocol(format!("json: {e}: {text}")))
}

// ── Reads ───────────────────────────────────────────────────────────────

pub async fn list_calendars(state: &ApiState) -> GraphResult<Vec<Calendar>> {
    let mut out = Vec::new();
    let mut next: Option<String> = Some("/me/calendars".into());
    while let Some(path) = next {
        let resp: CalendarListResponse = state.get_json(&path).await?;
        for entry in resp.value {
            out.push(map_calendar(entry));
        }
        next = resp.next_link;
    }
    Ok(out)
}

/// Range-scoped event read. We use the `/calendarView` endpoint
/// (`startDateTime` / `endDateTime` query params) so Graph expands
/// recurring events server-side. The local + Google + iCal paths
/// expand client-side via rrule.js; for Graph it's simpler to use
/// the expanded view since the structured-recurrence ↔ RRULE
/// translation isn't 1:1 for the relative-monthly / relative-yearly
/// shapes we deliberately don't write.
pub async fn get_events(
    state: &ApiState,
    calendar_id: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> GraphResult<Vec<Event>> {
    let cal_enc = urlencoding(calendar_id);
    let path = format!(
        "/me/calendars/{cal_enc}/calendarView?startDateTime={tm}&endDateTime={tx}\
         &$top=500",
        tm = urlencoding(&start.to_rfc3339()),
        tx = urlencoding(&end.to_rfc3339()),
    );
    let mut out = Vec::new();
    let mut next: Option<String> = Some(path);
    while let Some(p) = next {
        let resp: EventListResponse = state.get_json(&p).await?;
        for entry in resp.value {
            if let Some(ev) = map_event(entry, calendar_id)? {
                out.push(ev);
            }
        }
        next = resp.next_link;
    }
    Ok(out)
}

// ── Writes ──────────────────────────────────────────────────────────────

pub async fn create_event(
    state: &ApiState,
    calendar_id: &str,
    new: NewEvent,
) -> GraphResult<Event> {
    let cal_enc = urlencoding(calendar_id);
    let path = format!("/me/calendars/{cal_enc}/events");
    let body = new_event_to_body(&new)?;
    let entry: EventEntry = state.post_json(&path, &body).await?;
    map_event(entry, calendar_id)?
        .ok_or_else(|| GraphError::Protocol("create returned cancelled event".into()))
}

pub async fn update_event(state: &ApiState, ev: &Event) -> GraphResult<Event> {
    // Graph's PATCH endpoint is mounted at `/me/events/{id}` —
    // there's no per-calendar variant. The event id is globally
    // unique within the mailbox.
    let id_enc = urlencoding(&ev.id);
    let path = format!("/me/events/{id_enc}");
    let body = event_to_body(ev)?;
    let entry: EventEntry = state.patch_json(&path, &body).await?;
    map_event(entry, &ev.calendar_id)?
        .ok_or_else(|| GraphError::Protocol("update returned cancelled event".into()))
}

pub async fn delete_event(state: &ApiState, event_id: &str) -> GraphResult<()> {
    let id_enc = urlencoding(event_id);
    let path = format!("/me/events/{id_enc}");
    state.delete_request(&path).await
}

pub async fn rename_calendar(
    state: &ApiState,
    calendar_id: &str,
    new_name: &str,
) -> GraphResult<()> {
    let cal_enc = urlencoding(calendar_id);
    let path = format!("/me/calendars/{cal_enc}");
    let body = serde_json::json!({ "name": new_name });
    let _: serde_json::Value = state.patch_json(&path, &body).await?;
    Ok(())
}

// ── Microsoft To Do ─────────────────────────────────────────────────────

/// Enumerate every Microsoft To Do task list reachable from the
/// signed-in user. Pagination same as `list_calendars`.
pub async fn list_task_lists(state: &ApiState) -> GraphResult<Vec<TaskList>> {
    let mut out = Vec::new();
    let mut next: Option<String> = Some("/me/todo/lists".into());
    while let Some(path) = next {
        let resp: TodoListResponse = state.get_json(&path).await?;
        for entry in resp.value {
            out.push(map_task_list(entry));
        }
        next = resp.next_link;
    }
    Ok(out)
}

/// Pull every task from a To Do list. Tasks come back paginated; we
/// follow `@odata.nextLink` to gather the whole set.
pub async fn get_tasks(state: &ApiState, list_id: &str) -> GraphResult<Vec<Task>> {
    let list_enc = urlencoding(list_id);
    let path = format!("/me/todo/lists/{list_enc}/tasks?$top=200");
    let mut out = Vec::new();
    let mut next: Option<String> = Some(path);
    while let Some(p) = next {
        let resp: TodoTaskResponse = state.get_json(&p).await?;
        for entry in resp.value {
            out.push(map_task(entry, list_id)?);
        }
        next = resp.next_link;
    }
    Ok(out)
}

pub async fn create_task(state: &ApiState, list_id: &str, new: NewTask) -> GraphResult<Task> {
    let list_enc = urlencoding(list_id);
    let path = format!("/me/todo/lists/{list_enc}/tasks");
    let body = new_task_to_body(&new)?;
    let entry: TodoTaskEntry = state.post_json(&path, &body).await?;
    map_task(entry, list_id)
}

pub async fn update_task(state: &ApiState, task: &Task) -> GraphResult<Task> {
    let (list_id, raw_task_id) = split_task_id(&task.id);
    if list_id.is_empty() {
        return Err(GraphError::Protocol(format!(
            "task id '{}' is missing the list-id prefix; can't route update",
            task.id
        )));
    }
    let list_enc = urlencoding(&list_id);
    let task_enc = urlencoding(&raw_task_id);
    let path = format!("/me/todo/lists/{list_enc}/tasks/{task_enc}");
    let body = task_to_body(task)?;
    let entry: TodoTaskEntry = state.patch_json(&path, &body).await?;
    map_task(entry, &list_id)
}

pub async fn delete_task(state: &ApiState, task_id: &str) -> GraphResult<()> {
    let (list_id, raw_task_id) = split_task_id(task_id);
    if list_id.is_empty() {
        return Err(GraphError::Protocol(format!(
            "task id '{task_id}' is missing the list-id prefix; can't route delete"
        )));
    }
    let list_enc = urlencoding(&list_id);
    let task_enc = urlencoding(&raw_task_id);
    let path = format!("/me/todo/lists/{list_enc}/tasks/{task_enc}");
    state.delete_request(&path).await
}

pub async fn rename_task_list(state: &ApiState, list_id: &str, new_name: &str) -> GraphResult<()> {
    let list_enc = urlencoding(list_id);
    let path = format!("/me/todo/lists/{list_enc}");
    let body = serde_json::json!({ "displayName": new_name });
    let _: serde_json::Value = state.patch_json(&path, &body).await?;
    Ok(())
}

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
            .mock("GET", "/me/calendars")
            .with_status(200)
            .with_body(
                r##"{
                    "value": [
                        {"id": "AAAA", "name": "Main", "hexColor": "#0078d4", "canEdit": true},
                        {"id": "BBBB", "name": "Shared", "canEdit": false}
                    ]
                }"##,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let cals = list_calendars(&state).await.unwrap();
        assert_eq!(cals.len(), 2);
        assert_eq!(cals[0].name, "Main");
        assert!(!cals[0].read_only);
        assert!(cals[1].read_only);
    }

    #[tokio::test]
    async fn get_events_uses_calendar_view_with_range() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    r"^/me/calendars/cal-1/calendarView\?startDateTime=.*endDateTime=".to_string(),
                ),
            )
            .with_status(200)
            .with_body(r#"{"value":[]}"#)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let from = chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let evs = get_events(&state, "cal-1", from, to).await.unwrap();
        assert!(evs.is_empty());
        m.assert_async().await;
    }

    #[tokio::test]
    async fn create_event_posts_to_calendar_events() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/me/calendars/cal-1/events")
            .match_body(mockito::Matcher::Regex("\"subject\":\"Standup\"".into()))
            .with_status(200)
            .with_body(
                r##"{
                    "id": "ev1",
                    "subject": "Standup",
                    "isAllDay": false,
                    "isReminderOn": false,
                    "start": {"dateTime": "2026-05-25T10:00:00", "timeZone": "UTC"},
                    "end":   {"dateTime": "2026-05-25T10:30:00", "timeZone": "UTC"}
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
        let ev = create_event(&state, "cal-1", new).await.unwrap();
        assert_eq!(ev.id, "ev1");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn delete_event_hits_me_events_id() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("DELETE", "/me/events/ev-x")
            .with_status(204)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        delete_event(&state, "ev-x").await.unwrap();
    }

    #[tokio::test]
    async fn rename_calendar_patches_name() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("PATCH", "/me/calendars/cal-1")
            .match_body(mockito::Matcher::Regex("\"name\":\"Arbeit\"".into()))
            .with_status(200)
            .with_body(r##"{"id":"cal-1","name":"Arbeit"}"##)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        rename_calendar(&state, "cal-1", "Arbeit").await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn get_json_refreshes_on_401() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/me/calendars")
            .match_header("authorization", "Bearer initial-token")
            .with_status(401)
            .expect(1)
            .create_async()
            .await;
        server
            .mock("POST", "/token")
            .with_status(200)
            .with_body(r#"{"access_token":"new","expires_in":3600,"token_type":"Bearer"}"#)
            .expect(1)
            .create_async()
            .await;
        server
            .mock("GET", "/me/calendars")
            .match_header("authorization", "Bearer new")
            .with_status(200)
            .with_body(r#"{"value":[]}"#)
            .expect(1)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let cals = list_calendars(&state).await.unwrap();
        assert!(cals.is_empty());
    }

    #[tokio::test]
    async fn list_task_lists_paginates_via_next_link() {
        let mut server = mockito::Server::new_async().await;
        let next_link = format!("{}/me/todo/lists?$skiptoken=abc", server.url());
        server
            .mock("GET", "/me/todo/lists")
            .with_status(200)
            .with_body(format!(
                r#"{{
                    "value": [
                        {{"id":"L1","displayName":"Tasks","isOwner":true}}
                    ],
                    "@odata.nextLink": "{next_link}"
                }}"#,
            ))
            .create_async()
            .await;
        server
            .mock("GET", "/me/todo/lists?$skiptoken=abc")
            .with_status(200)
            .with_body(r#"{"value": [{"id":"L2","displayName":"Groceries","isOwner":true}]}"#)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let lists = list_task_lists(&state).await.unwrap();
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].name, "Tasks");
        assert_eq!(lists[1].name, "Groceries");
    }

    #[tokio::test]
    async fn get_tasks_round_trips_and_packs_list_id() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock(
                "GET",
                mockito::Matcher::Regex("/me/todo/lists/.+/tasks".into()),
            )
            .with_status(200)
            .with_body(
                r#"{"value":[
                    {"id":"T1","title":"Call mom","importance":"normal","status":"notStarted"},
                    {"id":"T2","title":"Read book","importance":"low","status":"completed"}
                ]}"#,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let tasks = get_tasks(&state, "LIST-X").await.unwrap();
        assert_eq!(tasks.len(), 2);
        // Composite ids carry the list prefix so update/delete can
        // route without a separate list-id parameter.
        assert_eq!(tasks[0].id, "LIST-X|T1");
        assert_eq!(tasks[1].id, "LIST-X|T2");
        assert_eq!(tasks[1].status, cal_core::TaskStatus::Completed);
    }

    #[tokio::test]
    async fn delete_task_errors_when_composite_id_is_legacy() {
        // A bare task id (no list prefix) can't be routed — we
        // surface a clear Protocol error rather than hitting Graph
        // with a malformed URL.
        let server = mockito::Server::new_async().await;
        let state = fixture_state(&server.url());
        let err = delete_task(&state, "NO-PREFIX").await.unwrap_err();
        match err {
            GraphError::Protocol(m) => assert!(m.contains("list-id prefix")),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_task_round_trips_against_composite_id() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("DELETE", "/me/todo/lists/LIST-A/tasks/T1")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        delete_task(&state, "LIST-A|T1").await.unwrap();
    }

    #[tokio::test]
    async fn rename_task_list_patches_display_name() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("PATCH", "/me/todo/lists/LIST-A")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "displayName": "Renamed"
            })))
            .with_status(200)
            .with_body(r#"{"id":"LIST-A","displayName":"Renamed"}"#)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        rename_task_list(&state, "LIST-A", "Renamed").await.unwrap();
    }
}
