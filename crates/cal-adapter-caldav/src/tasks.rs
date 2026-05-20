//! VTODO (RFC 5545) read/write for CalDAV servers that surface task
//! collections.
//!
//! CalDAV doesn't separate "calendars" from "task lists" at the
//! protocol level — both are calendar collections that advertise
//! either VEVENT, VTODO, or both via
//! `supported-calendar-component-set`. The listing side filters on
//! VTODO; the iCal payload uses BEGIN:VTODO/END:VTODO instead of
//! VEVENT.
//!
//! Phase 6b.3 ships a minimal-but-correct subset:
//!   - list_task_lists: PROPFIND depth 1 on the calendar-home,
//!     keeping only VTODO-capable collections
//!   - get_tasks: REPORT calendar-query with a VTODO comp-filter
//!     (no time-range — many task servers reject it, and a task
//!     without a due date should still come back)
//!   - create_task / update_task / delete_task: PUT/DELETE on the
//!     resource URL with the matching iCal body
//!
//! Status, priority, scheduled/due dates, and the completed_at flag
//! all round-trip. Reminders (VALARM) and sound overrides come with
//! the later wave that addresses VALARM mapping in general.

use cal_core::{
    AdapterSource, Calendar, DeadlineType, NewTask, Task, TaskList, TaskPriority,
    TaskStatus,
};
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use icalendar::{Calendar as ICalendar, Component, Todo};
use reqwest::{
    header::{HeaderName, HeaderValue, CONTENT_TYPE, IF_MATCH, IF_NONE_MATCH, ETAG},
    Client, Method, StatusCode,
};
use url::Url;
use uuid::Uuid;

use crate::auth::auth_header;
use crate::calendars;
use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::xml::{parse_multistatus, ResponseEntry};

const TASK_LIST_PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"
           xmlns:ical="http://apple.com/ns/ical/">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <c:supported-calendar-component-set/>
    <ical:calendar-color/>
  </d:prop>
</d:propfind>"#;

const VTODO_QUERY_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VTODO"/>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#;

/// PROPFIND the calendar-home with depth 1 and keep only the
/// collections that declare VTODO support.
pub async fn list_task_lists(
    client: &Client,
    home_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<TaskList>> {
    let entries = propfind(client, home_url, TASK_LIST_PROPFIND_BODY, credentials, 1)
        .await?;
    Ok(entries
        .into_iter()
        .filter(|e| e.is_calendar)
        .filter(|e| supports_vtodo(e))
        .map(|e| to_task_list(home_url, e))
        .collect())
}

fn supports_vtodo(entry: &ResponseEntry) -> bool {
    // When the server is silent we conservatively assume the
    // collection is event-only — there are far more event-only
    // calendars in the wild than universally-typed ones.
    entry
        .supported_components
        .iter()
        .any(|c| c.eq_ignore_ascii_case("VTODO"))
}

fn to_task_list(home_url: &Url, entry: ResponseEntry) -> TaskList {
    let id = home_url
        .join(&entry.href)
        .map(|u| u.to_string())
        .unwrap_or(entry.href.clone());

    let color = entry.calendar_color.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.starts_with('#') && (trimmed.len() == 7 || trimmed.len() == 9) {
            Some(cal_core::ContainerColor {
                hex: trimmed[..7].to_string(),
                source: cal_core::ColorSource::Native,
            })
        } else {
            None
        }
    });

    TaskList {
        id,
        name: entry
            .displayname
            .unwrap_or_else(|| "Unnamed task list".into()),
        color,
        default_sound: None,
        embedded_in_calendar: None,
        read_only: false,
    }
}

/// REPORT calendar-query for VTODO and map each task into the
/// `cal_core::Task` shape. ETag from the server is preserved on
/// every task so the write paths can use If-Match.
pub async fn get_tasks(
    client: &Client,
    list_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<Task>> {
    let method = Method::from_bytes(b"REPORT").expect("REPORT");
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_static("1"),
    );
    let response = client
        .request(method, list_url.clone())
        .headers(headers)
        .body(VTODO_QUERY_BODY)
        .send()
        .await?;
    let status = response.status();
    if status != StatusCode::from_u16(207).unwrap() && !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: if body.is_empty() {
                status.canonical_reason().unwrap_or("").to_string()
            } else {
                body.chars().take(200).collect()
            },
        });
    }
    let text = response.text().await?;
    let entries = parse_multistatus(&text)?;
    let list_id = list_url.as_str();
    let mut out = Vec::new();
    for entry in entries {
        let Some(ical) = entry.calendar_data else {
            continue;
        };
        let parsed: ICalendar = ical
            .parse()
            .map_err(|err: String| CaldavError::Protocol(format!("ical: {err}")))?;
        for comp in parsed.components {
            if let icalendar::CalendarComponent::Todo(todo) = comp {
                if let Some(mut task) = map_todo(&todo, list_id) {
                    if let Some(etag) = &entry.etag {
                        task.etag = Some(etag.clone());
                    }
                    out.push(task);
                }
            }
        }
    }
    Ok(out)
}

pub async fn create_task(
    client: &Client,
    list_url: &Url,
    new: NewTask,
    credentials: &Credentials,
) -> CaldavResult<Task> {
    let uid = format!("{}@aperio", Uuid::new_v4());
    let resource = resource_url(list_url, &uid)?;
    let body = build_vtodo_body(&uid, &new, None);
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
    let response = client
        .put(resource.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;
    let etag = expect_write(&response)?;
    let now = Utc::now();
    Ok(Task {
        id: uid,
        list_id: list_url.to_string(),
        title: new.title,
        description: new.description,
        status: new.status,
        priority: new.priority,
        scheduled_date: new.scheduled_date,
        deadline_type: new.deadline_type,
        deadline_date: new.deadline_date,
        deadline_time: new.deadline_time,
        recurrence: new.recurrence,
        parent_id: new.parent_id,
        color_label: new.color_label,
        reminders: new.reminders,
        sound: new.sound,
        created_at: now,
        updated_at: now,
        completed_at: None,
        etag,
    })
}

pub async fn update_task(
    client: &Client,
    task: Task,
    credentials: &Credentials,
) -> CaldavResult<Task> {
    let list_url = Url::parse(&task.list_id).map_err(|e| {
        CaldavError::Config(format!("task.list_id is not a URL: {e}"))
    })?;
    let resource = resource_url(&list_url, &task.id)?;
    let body = build_vtodo_from_task(&task);
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Some(etag) = &task.etag {
        let value = HeaderValue::from_str(etag)
            .map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let response = client
        .put(resource.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;
    let new_etag = expect_write(&response)?;
    Ok(Task {
        etag: new_etag.or(task.etag.clone()),
        updated_at: Utc::now(),
        ..task
    })
}

pub async fn delete_task(
    client: &Client,
    list_url: &Url,
    task_id: &str,
    etag: Option<&str>,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let resource = resource_url(list_url, task_id)?;
    let mut headers = auth_header(credentials)?;
    if let Some(etag) = etag {
        let value =
            HeaderValue::from_str(etag).map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let response = client.delete(resource).headers(headers).send().await?;
    let status = response.status();
    if !status.is_success() && status != StatusCode::NOT_FOUND {
        let body = response.text().await.unwrap_or_default();
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: if body.is_empty() {
                status.canonical_reason().unwrap_or("").to_string()
            } else {
                body.chars().take(200).collect()
            },
        });
    }
    Ok(())
}

/// Resolve a [`Calendar`] (in CalDAV-land same as a task list, the
/// adapter doesn't enforce the split) by walking the home set when
/// the caller didn't keep the URL around. Used by the trait's
/// `delete_task(task_id)` path which doesn't know the list id.
pub async fn find_calendar_for_id(
    client: &Client,
    home_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<Calendar>> {
    calendars::list_calendars(client, home_url, credentials).await
}

// ── iCal helpers ─────────────────────────────────────────────────────────

fn build_vtodo_body(uid: &str, task: &NewTask, completed_at: Option<DateTime<Utc>>) -> String {
    let mut todo = Todo::new();
    apply_common(&mut todo, uid, task, completed_at);
    let mut cal = ICalendar::new();
    cal.push(todo.done());
    cal.to_string()
}

fn build_vtodo_from_task(task: &Task) -> String {
    let new = NewTask {
        title: task.title.clone(),
        description: task.description.clone(),
        status: task.status,
        priority: task.priority,
        scheduled_date: task.scheduled_date,
        deadline_type: task.deadline_type,
        deadline_date: task.deadline_date,
        deadline_time: task.deadline_time.clone(),
        recurrence: task.recurrence.clone(),
        parent_id: task.parent_id.clone(),
        color_label: task.color_label.clone(),
        reminders: task.reminders.clone(),
        sound: task.sound.clone(),
    };
    build_vtodo_body(&task.id, &new, task.completed_at)
}

fn apply_common(
    todo: &mut Todo,
    uid: &str,
    task: &NewTask,
    completed_at: Option<DateTime<Utc>>,
) {
    todo.uid(uid);
    todo.summary(&task.title);
    if let Some(desc) = &task.description {
        todo.description(desc);
    }
    // STATUS is mandatory for an interpretable VTODO.
    let status_str = match task.status {
        TaskStatus::Open => "NEEDS-ACTION",
        TaskStatus::InProgress => "IN-PROCESS",
        TaskStatus::Completed => "COMPLETED",
        TaskStatus::Cancelled => "CANCELLED",
    };
    todo.add_property("STATUS", status_str);

    // PRIORITY: RFC 5545 reserves 1 (high) – 9 (low). We map our
    // three-level priority into the three canonical anchor points
    // so other clients can round-trip a sensible value.
    let prio = match task.priority {
        TaskPriority::High => 1,
        TaskPriority::Medium => 5,
        TaskPriority::Low => 9,
    };
    todo.add_property("PRIORITY", &prio.to_string());

    if let Some(date) = task.scheduled_date {
        todo.add_property("DTSTART", &format_date_compact(date));
    }
    if let Some(date) = task.deadline_date {
        let value = if let Some(time) = task.deadline_time {
            // DATE+TIME → compact UTC YYYYMMDDTHHMMSSZ.
            Utc.from_utc_datetime(&date.and_time(time))
                .format("%Y%m%dT%H%M%SZ")
                .to_string()
        } else {
            format_date_compact(date)
        };
        todo.add_property("DUE", &value);
    }
    if let Some(completed) = completed_at {
        todo.add_property(
            "COMPLETED",
            &completed.format("%Y%m%dT%H%M%SZ").to_string(),
        );
    }
    if let Some(deadline_type) = task.deadline_type {
        // Aperio's deadline_type isn't part of RFC 5545; we stash it
        // in an X- property so a future read can recover the
        // distinction between "due on" vs "due by" the user picked.
        let v = match deadline_type {
            DeadlineType::On => "ON",
            DeadlineType::By => "BY",
        };
        todo.add_property("X-APERIO-DEADLINE-TYPE", v);
    }
}

fn format_date_compact(date: NaiveDate) -> String {
    date.format("%Y%m%d").to_string()
}

fn map_todo(todo: &Todo, list_id: &str) -> Option<Task> {
    let uid = todo.get_uid()?.to_string();
    let title = todo.get_summary().unwrap_or("").to_string();
    let description = todo.get_description().map(|s| s.to_string());

    let status = match todo
        .property_value("STATUS")
        .map(|s| s.to_ascii_uppercase())
        .as_deref()
    {
        Some("IN-PROCESS") => TaskStatus::InProgress,
        Some("COMPLETED") => TaskStatus::Completed,
        Some("CANCELLED") => TaskStatus::Cancelled,
        _ => TaskStatus::Open,
    };
    let priority = match todo
        .property_value("PRIORITY")
        .and_then(|s| s.parse::<u8>().ok())
    {
        Some(p) if p <= 3 => TaskPriority::High,
        Some(p) if p >= 7 => TaskPriority::Low,
        _ => TaskPriority::Medium,
    };

    let scheduled_date = todo
        .property_value("DTSTART")
        .and_then(parse_compact_date);
    let (deadline_date, deadline_time) = parse_due(todo);
    let deadline_type = todo
        .property_value("X-APERIO-DEADLINE-TYPE")
        .and_then(|v| match v.to_ascii_uppercase().as_str() {
            "ON" => Some(DeadlineType::On),
            "BY" => Some(DeadlineType::By),
            _ => None,
        });
    let completed_at = todo
        .property_value("COMPLETED")
        .and_then(parse_compact_utc);
    let created_at = todo
        .property_value("CREATED")
        .and_then(parse_compact_utc)
        .or_else(|| todo.property_value("DTSTAMP").and_then(parse_compact_utc))
        .unwrap_or_else(Utc::now);
    let updated_at = todo
        .property_value("LAST-MODIFIED")
        .and_then(parse_compact_utc)
        .or_else(|| todo.property_value("DTSTAMP").and_then(parse_compact_utc))
        .unwrap_or(created_at);

    Some(Task {
        id: uid,
        list_id: list_id.to_string(),
        title,
        description,
        status,
        priority,
        scheduled_date,
        deadline_type,
        deadline_date,
        deadline_time,
        recurrence: None,
        parent_id: None,
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        created_at,
        updated_at,
        completed_at,
        etag: None,
    })
}

fn parse_due(todo: &Todo) -> (Option<NaiveDate>, Option<NaiveTime>) {
    let Some(raw) = todo.property_value("DUE") else {
        return (None, None);
    };
    if let Some(date) = parse_compact_date(raw) {
        return (Some(date), None);
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%SZ") {
        return (Some(dt.date()), Some(dt.time()));
    }
    (None, None)
}

fn parse_compact_date(s: &str) -> Option<NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%Y%m%d").ok()
}

fn parse_compact_utc(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

async fn propfind(
    client: &Client,
    url: &Url,
    body: &'static str,
    credentials: &Credentials,
    depth: u8,
) -> CaldavResult<Vec<ResponseEntry>> {
    let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND");
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_str(&depth.to_string()).expect("digit"),
    );
    let response = client
        .request(method, url.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;
    let status = response.status();
    if status != StatusCode::from_u16(207).unwrap() && !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: if body.is_empty() {
                status.canonical_reason().unwrap_or("").to_string()
            } else {
                body.chars().take(200).collect()
            },
        });
    }
    let text = response.text().await?;
    parse_multistatus(&text)
}

fn resource_url(list_url: &Url, uid: &str) -> CaldavResult<Url> {
    let slug = format!("{}.ics", urlencoding(uid));
    list_url.join(&slug).map_err(Into::into)
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn expect_write(response: &reqwest::Response) -> CaldavResult<Option<String>> {
    let status = response.status();
    if !status.is_success() {
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: status.canonical_reason().unwrap_or("").to_string(),
        });
    }
    Ok(response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string()))
}

// AdapterSource isn't used here today but kept in the import set so
// the future Aperio source-stamping at the persistence layer
// compiles without re-importing.
#[allow(dead_code)]
fn _touch_adapter_source(_: AdapterSource) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthKind, CaldavAccountConfig};
    use mockito::Server;

    fn creds(server_url: &str) -> Credentials {
        Credentials::new(
            CaldavAccountConfig {
                server_url: server_url.into(),
                username: "alice".into(),
                auth_kind: AuthKind::Basic,
            },
            "hunter2".into(),
        )
    }

    fn client() -> Client {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    fn sample_new_task() -> NewTask {
        NewTask {
            title: "Buy milk".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            deadline_type: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
        }
    }

    #[tokio::test]
    async fn list_task_lists_keeps_only_vtodo_capable() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/tasks/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Tasks</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      <c:supported-calendar-component-set>
        <c:comp name="VTODO"/>
      </c:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Work</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      <c:supported-calendar-component-set>
        <c:comp name="VEVENT"/>
      </c:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let mut server = Server::new_async().await;
        let _m = server
            .mock("PROPFIND", "/calendars/alice/")
            .with_status(207)
            .with_body(body)
            .create_async()
            .await;
        let home = Url::parse(&format!("{}/calendars/alice/", server.url())).unwrap();
        let lists = list_task_lists(&client(), &home, &creds(&server.url()))
            .await
            .unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, "Tasks");
    }

    #[tokio::test]
    async fn get_tasks_returns_mapped_vtodos() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/tasks/t1.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"todo-1"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//test//EN
BEGIN:VTODO
UID:todo-1@aperio
SUMMARY:Buy milk
STATUS:NEEDS-ACTION
PRIORITY:5
DTSTART:20260520
END:VTODO
END:VCALENDAR</c:calendar-data>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let mut server = Server::new_async().await;
        let _m = server
            .mock("REPORT", "/calendars/alice/tasks/")
            .with_status(207)
            .with_body(body)
            .create_async()
            .await;
        let url =
            Url::parse(&format!("{}/calendars/alice/tasks/", server.url())).unwrap();
        let tasks = get_tasks(&client(), &url, &creds(&server.url())).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Buy milk");
        assert!(matches!(tasks[0].status, TaskStatus::Open));
        assert!(matches!(tasks[0].priority, TaskPriority::Medium));
        assert_eq!(
            tasks[0].scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap())
        );
        assert_eq!(tasks[0].etag.as_deref(), Some("\"todo-1\""));
    }

    #[tokio::test]
    async fn create_task_puts_a_vtodo_body() {
        let mut server = Server::new_async().await;
        let m = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/tasks/.+\.ics$".into()),
            )
            .match_header("if-none-match", "*")
            .with_status(201)
            .with_header("etag", "\"new\"")
            .create_async()
            .await;
        let url =
            Url::parse(&format!("{}/calendars/alice/tasks/", server.url())).unwrap();
        let created = create_task(&client(), &url, sample_new_task(), &creds(&server.url()))
            .await
            .unwrap();
        m.assert_async().await;
        assert!(created.id.contains("@aperio"));
        assert_eq!(created.etag.as_deref(), Some("\"new\""));
    }
}
