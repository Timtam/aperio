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

use cal_core::{AdapterSource, Calendar, NewTask, Task, TaskList, TaskPriority, TaskStatus};
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use icalendar::{Calendar as ICalendar, CalendarDateTime, Component, DatePerhapsTime, Todo};
use reqwest::{
    header::{HeaderName, HeaderValue, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH},
    Client, Method, StatusCode,
};
use url::Url;
use uuid::Uuid;

use crate::auth::auth_header;
use crate::calendars;
use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::http::SendRetrying;
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
    let entries = propfind(client, home_url, TASK_LIST_PROPFIND_BODY, credentials, 1).await?;
    Ok(entries
        .into_iter()
        .filter(|e| e.is_calendar)
        .filter(supports_vtodo)
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
        color_label: None,
        id,
        name: entry
            .displayname
            .unwrap_or_else(|| "Unnamed task list".into()),
        color,
        default_sound: None,
        embedded_in_calendar: None,
        parent_id: None,
        read_only: false,
    }
}

/// `MKCALENDAR` a new VTODO collection under the calendar-home set
/// (RFC 4791 §5.3.1). The new collection lives at `home_url + {uuid}/`
/// and is created with the VTODO component set so the server lists it
/// as a task collection. CalDAV task lists are flat — `parent_id` is
/// ignored. Returns the created list (its URL becomes the list id).
pub async fn create_task_list(
    client: &Client,
    home_url: &Url,
    name: &str,
    credentials: &Credentials,
) -> CaldavResult<TaskList> {
    let segment = format!("{}/", Uuid::new_v4());
    let collection_url = home_url
        .join(&segment)
        .map_err(|e| CaldavError::Config(format!("building collection url: {e}")))?;

    let method = Method::from_bytes(b"MKCALENDAR").expect("MKCALENDAR");
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    let escaped = calendars::escape_xml(name);
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set>
    <D:prop>
      <D:displayname>{escaped}</D:displayname>
      <C:supported-calendar-component-set>
        <C:comp name="VTODO"/>
      </C:supported-calendar-component-set>
    </D:prop>
  </D:set>
</C:mkcalendar>"#
    );

    let response = client
        .request(method, collection_url.clone())
        .headers(headers)
        .body(body)
        .send_retrying()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: if text.is_empty() {
                status.canonical_reason().unwrap_or("").to_string()
            } else {
                text.chars().take(200).collect()
            },
        });
    }

    Ok(TaskList {
        color_label: None,
        id: collection_url.to_string(),
        name: name.to_string(),
        color: None,
        default_sound: None,
        embedded_in_calendar: None,
        parent_id: None,
        read_only: false,
    })
}

/// `DELETE` a task collection at `list_url` (RFC 4918 §9.6). The
/// server removes the collection and every VTODO inside it.
pub async fn delete_task_list(
    client: &Client,
    list_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let headers = auth_header(credentials)?;
    let response = client
        .delete(list_url.clone())
        .headers(headers)
        .send_retrying()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: if text.is_empty() {
                status.canonical_reason().unwrap_or("").to_string()
            } else {
                text.chars().take(200).collect()
            },
        });
    }
    Ok(())
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
        .send_retrying()
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
                // Pass the server's actual href into the id encoder
                // — without it, delete/update later would build a
                // URL from the UID alone, which doesn't match how
                // iCloud (and others) name their VTODO resources.
                if let Some(mut task) = map_todo(&todo, list_id, Some(&entry.href)) {
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

/// Map multistatus entries carrying `calendar-data` (from `get_tasks`'s
/// VTODO query or a sync-collection `calendar-multiget`) into tasks.
/// Tolerant — a single unparseable resource is skipped, not fatal — so
/// the delta read can't be sunk by one bad VTODO. The `{href}|{uid}` id
/// shape matches `get_tasks` exactly so the cache stays consistent across
/// the full and incremental read paths.
pub fn parse_task_entries(entries: &[ResponseEntry], list_id: &str) -> Vec<Task> {
    let mut out = Vec::new();
    for entry in entries {
        let Some(ical) = entry.calendar_data.as_deref() else {
            continue;
        };
        let Ok(parsed) = ical.parse::<ICalendar>() else {
            continue;
        };
        for comp in parsed.components {
            if let icalendar::CalendarComponent::Todo(todo) = comp {
                if let Some(mut task) = map_todo(&todo, list_id, Some(&entry.href)) {
                    if let Some(etag) = &entry.etag {
                        task.etag = Some(etag.clone());
                    }
                    out.push(task);
                }
            }
        }
    }
    out
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
        .send_retrying()
        .await?;
    let etag = expect_write(&response)?;
    let now = Utc::now();
    Ok(Task {
        assignees: Vec::new(),
        id: uid,
        list_id: list_url.to_string(),
        title: new.title,
        description: new.description,
        status: new.status,
        priority: new.priority,
        scheduled_date: new.scheduled_date,
        scheduled_time: new.scheduled_time,
        deadline_date: new.deadline_date,
        deadline_time: new.deadline_time,
        recurrence: new.recurrence,
        parent_id: new.parent_id,
        section_id: None,
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
    let list_url = Url::parse(&task.list_id)
        .map_err(|e| CaldavError::Config(format!("task.list_id is not a URL: {e}")))?;
    // Resolve the actual resource URL the server stored the VTODO at.
    // `get_tasks` encodes `{href}|{uid}` into task.id so we can recover
    // the server's filename here — iCloud chooses its own paths for
    // VTODO resources, and falling back to `{list}/{uid}.ics` reaches
    // a 404 every time. The legacy uid-only path still applies to
    // freshly-created tasks that haven't been refetched yet.
    let resource = resource_url_for_task(&list_url, &task.id)?;
    let body = build_vtodo_from_task(&task);
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Some(etag) = &task.etag {
        let value = HeaderValue::from_str(etag).map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let response = client
        .put(resource.clone())
        .headers(headers)
        .body(body)
        .send_retrying()
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
) -> CaldavResult<crate::events::DeleteOutcome> {
    // See `update_task` for why the resource URL has to come from the
    // server-provided href encoded into the id rather than from
    // `{list}/{uid}.ics`.
    //
    // 404 → `DeleteOutcome::NotFound` rather than success. The
    // direct-API contract still treats it as a non-error
    // (idempotent), but the home-set walker in `lib.rs::delete_task`
    // reads the typed outcome so it doesn't short-circuit on the
    // first task list that returns 404.
    let resource = resource_url_for_task(list_url, task_id)?;
    let mut headers = auth_header(credentials)?;
    if let Some(etag) = etag {
        let value = HeaderValue::from_str(etag).map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let response = client
        .delete(resource)
        .headers(headers)
        .send_retrying()
        .await?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(crate::events::DeleteOutcome::NotFound);
    }
    if !status.is_success() {
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
    Ok(crate::events::DeleteOutcome::Deleted)
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
    // Tasks routing only needs the ids; the scheduling flag is irrelevant.
    calendars::list_calendars(client, home_url, credentials, false).await
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
        assignees: Vec::new(),
        title: task.title.clone(),
        description: task.description.clone(),
        status: task.status,
        priority: task.priority,
        scheduled_date: task.scheduled_date,
        scheduled_time: task.scheduled_time,
        deadline_date: task.deadline_date,
        deadline_time: task.deadline_time,
        recurrence: task.recurrence.clone(),
        parent_id: task.parent_id.clone(),
        section_id: None,
        color_label: task.color_label.clone(),
        reminders: task.reminders.clone(),
        sound: task.sound.clone(),
    };
    // The id may be the composite `{href}|{uid}` we encode in
    // `map_todo`; strip the href back off so the iCal UID matches what
    // the server already stored.
    let (_, uid) = decode_id(&task.id);
    build_vtodo_body(uid, &new, task.completed_at)
}

fn apply_common(todo: &mut Todo, uid: &str, task: &NewTask, completed_at: Option<DateTime<Utc>>) {
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
    todo.add_property("PRIORITY", prio.to_string());

    // DTSTART / DUE: emit through the typed icalendar helpers so the
    // `VALUE=DATE` parameter goes on the wire when we hand over a
    // date-only value. The previous raw `add_property("DTSTART", "20260521")`
    // path produced a property *without* the parameter, which RFC 5545
    // says is a malformed DATE-TIME — iCloud's CalDAV server reacted
    // by silently dropping the property, leaving every stored task
    // "dateless" no matter how many times the user set the date.
    //
    // Mapping: DTSTART carries Aperio's `scheduled_date` (+ optional
    // `scheduled_time` as a UTC DATE-TIME), DUE carries `deadline_date`
    // (+ optional `deadline_time`). The previous "on" vs "by" enum
    // we used to stash in `X-APERIO-DEADLINE-TYPE` is gone — every
    // deadline is now "by" semantics — and the X- property is no
    // longer written. Older VTODOs with that property are read by
    // ignoring it; DTSTART and DUE flow into their natural slots.
    if let Some(date) = task.scheduled_date {
        let value: DatePerhapsTime = if let Some(time) = task.scheduled_time {
            CalendarDateTime::Utc(Utc.from_utc_datetime(&date.and_time(time))).into()
        } else {
            DatePerhapsTime::Date(date)
        };
        todo.append_property(value.to_property("DTSTART"));
    }
    if let Some(date) = task.deadline_date {
        let due: DatePerhapsTime = if let Some(time) = task.deadline_time {
            // DATE+TIME → UTC date-time. The typed conversion emits
            // `YYYYMMDDTHHMMSSZ` for us; no manual format needed.
            CalendarDateTime::Utc(Utc.from_utc_datetime(&date.and_time(time))).into()
        } else {
            DatePerhapsTime::Date(date)
        };
        todo.due(due);
    }
    if let Some(completed) = completed_at {
        todo.add_property("COMPLETED", completed.format("%Y%m%dT%H%M%SZ").to_string());
    }
}

fn map_todo(todo: &Todo, list_id: &str, href: Option<&str>) -> Option<Task> {
    let uid_raw = todo.get_uid()?.to_string();
    // Encode the server's resource href into the id when we have it
    // (`{href}|{uid}`). This lets the write paths reach the right URL
    // even when the server names its VTODO files arbitrarily — iCloud
    // does this for its Reminders collections. Without the href the
    // delete path falls through to a 404 (which we treat as success)
    // and the user sees "deleted" with the task still present on the
    // server. Tasks read before this fix kept the legacy bare-UID id
    // and continue to work via the fallback in `resource_url_for_task`.
    let uid = match href {
        Some(h) if !h.is_empty() => format!("{h}|{uid_raw}"),
        _ => uid_raw,
    };
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

    let (scheduled_date, scheduled_time) = parse_dt(todo, "DTSTART");
    let (deadline_date, deadline_time) = parse_dt(todo, "DUE");
    let completed_at = todo.property_value("COMPLETED").and_then(parse_compact_utc);
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
        assignees: Vec::new(),
        id: uid,
        list_id: list_id.to_string(),
        title,
        description,
        status,
        priority,
        scheduled_date,
        scheduled_time,
        deadline_date,
        deadline_time,
        recurrence: None,
        parent_id: None,
        section_id: None,
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        created_at,
        updated_at,
        completed_at,
        etag: None,
    })
}

/// Parse an iCalendar date/date-time property by name.
///
/// Used for both DTSTART (the "Geplant für" tag, optionally with a
/// time-of-day) and DUE (the "Spätestens bis" deadline, same shape).
/// Returns the date component plus an optional time when the value
/// was emitted as a UTC DATE-TIME. Date-only values yield a `None`
/// time. Unrecognised formats fall through to `(None, None)`.
fn parse_dt(todo: &Todo, prop: &str) -> (Option<NaiveDate>, Option<NaiveTime>) {
    let Some(raw) = todo.property_value(prop) else {
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
        .send_retrying()
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

/// Split a task id into `(Some(href), uid)` when it carries the
/// composite `{href}|{uid}` we mint in `map_todo`, or `(None, id)`
/// when it's a plain UID (freshly-created tasks before refetch, plus
/// rows persisted by older Aperio versions).
fn decode_id(task_id: &str) -> (Option<&str>, &str) {
    match task_id.split_once('|') {
        Some((href, uid)) if !href.is_empty() => (Some(href), uid),
        _ => (None, task_id),
    }
}

/// Resolve the absolute URL of the VTODO resource. Prefers the
/// server-provided href encoded into the id by `map_todo`; falls back
/// to the legacy `{list}/{uid}.ics` shape for tasks that haven't been
/// refetched since the bug fix.
fn resource_url_for_task(list_url: &Url, task_id: &str) -> CaldavResult<Url> {
    let (href, uid) = decode_id(task_id);
    if let Some(href) = href {
        // `Url::join` resolves both absolute-path-only ("/calendars/…")
        // and absolute-URL hrefs against the list base, so this works
        // regardless of how the server formatted its href.
        return list_url.join(href).map_err(Into::into);
    }
    resource_url(list_url, uid)
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
            assignees: Vec::new(),
            title: "Buy milk".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
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
        let url = Url::parse(&format!("{}/calendars/alice/tasks/", server.url())).unwrap();
        let tasks = get_tasks(&client(), &url, &creds(&server.url()))
            .await
            .unwrap();
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

    #[test]
    fn build_vtodo_emits_value_date_parameter_for_date_only_fields() {
        // Regression for the iCloud "task saves but date is gone"
        // bug: when scheduled_date / deadline_date are pure NaiveDate
        // (no time), DTSTART and DUE must carry `VALUE=DATE`. Without
        // it, RFC 5545 interprets a bare `20260520` as a malformed
        // DATE-TIME and Apple's CalDAV server silently drops the
        // property when it persists the VTODO.
        let new = NewTask {
            assignees: Vec::new(),
            title: "Pay bill".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap()),
            scheduled_time: None,
            deadline_date: Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
        };
        let body = build_vtodo_body("uid-1", &new, None);
        assert!(
            body.contains("DTSTART;VALUE=DATE:20260521"),
            "DTSTART must include VALUE=DATE parameter, got:\n{body}",
        );
        assert!(
            body.contains("DUE;VALUE=DATE:20260522"),
            "DUE must include VALUE=DATE parameter, got:\n{body}",
        );
    }

    #[test]
    fn build_vtodo_with_due_time_emits_utc_datetime() {
        // The other half of the same property: when the user picked a
        // specific time of day, DUE is a regular UTC DATE-TIME (no
        // VALUE parameter, RFC 5545 default).
        let new = NewTask {
            assignees: Vec::new(),
            title: "Status call".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
            deadline_time: Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap()),
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
        };
        let body = build_vtodo_body("uid-2", &new, None);
        assert!(
            body.contains("DUE:20260522T143000Z"),
            "DUE must be a UTC DATE-TIME when a time is set, got:\n{body}",
        );
        assert!(
            !body.contains("VALUE=DATE-TIME"),
            "DUE date-time should not carry VALUE=DATE-TIME (RFC 5545 default), got:\n{body}",
        );
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
        let url = Url::parse(&format!("{}/calendars/alice/tasks/", server.url())).unwrap();
        let created = create_task(&client(), &url, sample_new_task(), &creds(&server.url()))
            .await
            .unwrap();
        m.assert_async().await;
        assert!(created.id.contains("@aperio"));
        assert_eq!(created.etag.as_deref(), Some("\"new\""));
    }

    #[test]
    fn decode_id_splits_composite_form() {
        // Plain UID stays plain — covers tasks created before this fix
        // and freshly-created tasks before the next refetch.
        assert_eq!(decode_id("todo-1@aperio"), (None, "todo-1@aperio"));
        // Composite `{href}|{uid}` splits at the first pipe; the href
        // can be any of the URL shapes a server might hand back.
        assert_eq!(
            decode_id("/calendars/alice/tasks/EE.ics|todo-1@aperio"),
            (Some("/calendars/alice/tasks/EE.ics"), "todo-1@aperio"),
        );
    }

    #[test]
    fn resource_url_for_task_prefers_server_href() {
        let list_url = Url::parse("https://server.example/calendars/alice/tasks/").unwrap();
        // With href the resolved URL must use the server's filename,
        // not `{list}/{uid}.ics` — that's the regression that made
        // every iCloud task delete silently 404.
        let resolved = resource_url_for_task(
            &list_url,
            "/calendars/alice/tasks/8B0F-EE.ics|todo-1@aperio",
        )
        .unwrap();
        assert_eq!(
            resolved.as_str(),
            "https://server.example/calendars/alice/tasks/8B0F-EE.ics",
        );
        // Without href we fall back to the legacy UID-derived URL so
        // tasks that haven't been refetched since the fix still work.
        let legacy = resource_url_for_task(&list_url, "todo-1@aperio").unwrap();
        assert_eq!(
            legacy.as_str(),
            "https://server.example/calendars/alice/tasks/todo-1%40aperio.ics",
        );
    }

    #[tokio::test]
    async fn delete_task_uses_server_provided_href() {
        // Mirrors how iCloud names VTODO resources: the file lives at
        // a server-chosen path (`8B0F-EE.ics`), not at `{uid}.ics`.
        // The test asserts that delete actually hits the server-chosen
        // path; before the fix this DELETE would go to
        // `/calendars/alice/tasks/todo-1%40aperio.ics`, 404 silently,
        // and report success.
        let mut server = Server::new_async().await;
        let delete_mock = server
            .mock("DELETE", "/calendars/alice/tasks/8B0F-EE.ics")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        let url = Url::parse(&format!("{}/calendars/alice/tasks/", server.url())).unwrap();
        let composite_id = "/calendars/alice/tasks/8B0F-EE.ics|todo-1@aperio";
        delete_task(&client(), &url, composite_id, None, &creds(&server.url()))
            .await
            .unwrap();
        delete_mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_tasks_encodes_href_into_id() {
        // Verifies the read path stamps the href onto each task so
        // later writes can route to the right resource. Without this
        // round-trip the composite id never appears and the delete
        // fix above would never trigger.
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/tasks/8B0F-EE.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"todo-1"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//test//EN
BEGIN:VTODO
UID:todo-1@aperio
SUMMARY:Buy milk
STATUS:NEEDS-ACTION
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
        let url = Url::parse(&format!("{}/calendars/alice/tasks/", server.url())).unwrap();
        let tasks = get_tasks(&client(), &url, &creds(&server.url()))
            .await
            .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].id,
            "/calendars/alice/tasks/8B0F-EE.ics|todo-1@aperio",
        );
    }

    #[tokio::test]
    async fn create_task_list_mkcalendars_a_vtodo_collection() {
        let mut server = Server::new_async().await;
        // The collection path is a fresh UUID, so match on method +
        // any path under the home set and assert the body carries the
        // VTODO component and the escaped display name.
        let create_mock = server
            .mock("MKCALENDAR", mockito::Matcher::Any)
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("<C:comp name=\"VTODO\"/>".into()),
                mockito::Matcher::Regex("<D:displayname>Errands &amp; Co</D:displayname>".into()),
            ]))
            .with_status(201)
            .expect(1)
            .create_async()
            .await;
        let home = Url::parse(&format!("{}/calendars/alice/", server.url())).unwrap();
        let created = create_task_list(&client(), &home, "Errands & Co", &creds(&server.url()))
            .await
            .unwrap();
        create_mock.assert_async().await;
        assert!(created
            .id
            .starts_with(&format!("{}/calendars/alice/", server.url())));
        assert_eq!(created.name, "Errands & Co");
        assert!(created.parent_id.is_none());
    }

    #[tokio::test]
    async fn create_task_list_surfaces_http_failure() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("MKCALENDAR", mockito::Matcher::Any)
            .with_status(507)
            .with_body("Insufficient Storage")
            .create_async()
            .await;
        let home = Url::parse(&format!("{}/calendars/alice/", server.url())).unwrap();
        let err = create_task_list(&client(), &home, "Whatever", &creds(&server.url()))
            .await
            .unwrap_err();
        assert!(matches!(err, CaldavError::Http { status: 507, .. }));
    }

    #[tokio::test]
    async fn delete_task_list_deletes_the_collection() {
        let mut server = Server::new_async().await;
        let delete_mock = server
            .mock("DELETE", "/calendars/alice/work/")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        let url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        delete_task_list(&client(), &url, &creds(&server.url()))
            .await
            .unwrap();
        delete_mock.assert_async().await;
    }

    #[tokio::test]
    async fn delete_task_list_surfaces_http_failure() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/calendars/alice/work/")
            .with_status(403)
            .with_body("Forbidden")
            .create_async()
            .await;
        let url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let err = delete_task_list(&client(), &url, &creds(&server.url()))
            .await
            .unwrap_err();
        assert!(matches!(err, CaldavError::Http { status: 403, .. }));
    }
}
