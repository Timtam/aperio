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
//! Status, priority, scheduled/due dates, recurrence (`RRULE`, via
//! `cal_core::{task_recurrence_to_rrule, rrule_to_task_recurrence}`),
//! the completed_at flag and the subtask link (`RELATED-TO` with the
//! default RELTYPE=PARENT; see `resolve_parent_ids` for how the bare
//! UID becomes a composite task id) all round-trip. Per-occurrence
//! overrides (`EXDATE`) aren't surfaced for tasks yet. Reminders
//! (VALARM) and sound overrides come with the later wave that
//! addresses VALARM mapping in general.

use cal_core::{
    apply_task_extras, decode_payload, encode_payload, extras_for_task, recurrence_needs_extras,
    rrule_to_task_recurrence, task_recurrence_to_rrule, AdapterSource, Calendar, NewTask, Task,
    TaskList, TaskPriority, TaskStatus,
};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
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

/// REPORT calendar-query for VTODO, returning the raw multistatus
/// entries. Shared by `get_tasks` (full mapping) and
/// `get_task_uid_index` (id harvesting for the delta path's parent
/// resolution).
async fn vtodo_query(
    client: &Client,
    list_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<ResponseEntry>> {
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
    parse_multistatus(&text)
}

/// `uid → composite id` over the FULL collection, parsed tolerantly (a
/// single garbled VTODO is skipped like in `parse_task_entries`, not
/// fatal — the delta path only needs the ids it can see, and a strict
/// parse would make one broken resource sink every cross-delta parent
/// resolution forever).
pub async fn get_task_uid_index(
    client: &Client,
    list_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<std::collections::HashMap<String, String>> {
    let entries = vtodo_query(client, list_url, credentials).await?;
    Ok(uid_index(&parse_task_entries(&entries, list_url.as_str())))
}

/// REPORT calendar-query for VTODO and map each task into the
/// `cal_core::Task` shape. ETag from the server is preserved on
/// every task so the write paths can use If-Match.
pub async fn get_tasks(
    client: &Client,
    list_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<Task>> {
    let entries = vtodo_query(client, list_url, credentials).await?;
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
                if let Some(mut task) = map_todo(&todo, list_id, Some(&entry.href), Some(&ical)) {
                    if let Some(etag) = &entry.etag {
                        task.etag = Some(etag.clone());
                    }
                    out.push(task);
                }
            }
        }
    }
    // The full listing is the authoritative set: a parent UID it can't
    // resolve doesn't exist on the server, so it's dropped inside.
    let index = uid_index(&out);
    resolve_parent_ids(&mut out, &index);
    Ok(out)
}

/// Map multistatus entries carrying `calendar-data` (from `get_tasks`'s
/// VTODO query or a sync-collection `calendar-multiget`) into tasks.
/// Tolerant — a single unparseable resource is skipped, not fatal — so
/// the delta read can't be sunk by one bad VTODO. The `{href}|{uid}` id
/// shape matches `get_tasks` exactly so the cache stays consistent across
/// the full and incremental read paths.
///
/// `parent_id` still holds the BARE parent UID here: an incremental
/// change set may reference a parent that didn't itself change, so the
/// caller decides which set to resolve against (see `uid_index` +
/// `resolve_parent_ids`; the delta path in `lib.rs` falls back to
/// `get_task_uid_index` when a UID points outside the change set).
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
                if let Some(mut task) = map_todo(&todo, list_id, Some(&entry.href), Some(ical)) {
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
    // A task CREATED as completed stamps "now" as its completion instant:
    // `NewTask` carries none, and a VTODO claiming STATUS:COMPLETED without
    // a COMPLETED date is a shape Apple's own clients never produce —
    // servers that key completion on the date (EventKit does) may normalize
    // it back to NEEDS-ACTION, silently reopening the task on the next read.
    let completed_at = matches!(new.status, TaskStatus::Completed).then(Utc::now);
    let body = build_vtodo_body(&uid, &new, completed_at);
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
        effort: new.effort,
        scheduled_date: new.scheduled_date,
        scheduled_time: new.scheduled_time,
        scheduled_end_time: new.scheduled_end_time,
        deadline_date: new.deadline_date,
        deadline_time: new.deadline_time,
        deadline_reminder_days: new.deadline_reminder_days,
        recurrence: new.recurrence,
        parent_id: new.parent_id,
        section_id: None,
        color_label: new.color_label,
        reminders: new.reminders,
        sound: new.sound,
        created_at: now,
        updated_at: now,
        completed_at,
        etag,
        resurface_date: new.resurface_date,
        series_id: new.series_id,
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
        effort: task.effort,
        scheduled_date: task.scheduled_date,
        scheduled_time: task.scheduled_time,
        scheduled_end_time: task.scheduled_end_time,
        deadline_date: task.deadline_date,
        deadline_time: task.deadline_time,
        deadline_reminder_days: task.deadline_reminder_days,
        recurrence: task.recurrence.clone(),
        parent_id: task.parent_id.clone(),
        section_id: None,
        color_label: task.color_label.clone(),
        reminders: task.reminders.clone(),
        sound: task.sound.clone(),
        resurface_date: task.resurface_date,
        series_id: task.series_id.clone(),
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
    apply_task_dates(
        todo,
        task.scheduled_date,
        task.scheduled_time,
        task.scheduled_end_time,
        task.deadline_date,
        task.deadline_time,
    );
    if let Some(completed) = completed_at {
        todo.add_property("COMPLETED", completed.format("%Y%m%dT%H%M%SZ").to_string());
    }
    // RRULE: a plain scheduled rule serializes to an RFC 5545 rule. A
    // backlog/on-demand rule (DESIGN §9.12) has no RRULE expression, so it
    // rides the X-property below instead and no RRULE is written.
    // (EXDATE / per-occurrence overrides aren't surfaced for tasks yet.)
    if let Some(rec) = &task.recurrence {
        if !recurrence_needs_extras(rec) {
            todo.add_property("RRULE", task_recurrence_to_rrule(rec));
        }
    }

    // DESIGN §9.12: the Aperio-only fields ride an invisible `X-APERIO-EXTRAS`
    // property — CalDAV has a real custom-property channel, so nothing
    // pollutes the user-facing description. Empty bag ⇒ no property.
    let extras = extras_for_task(
        task.recurrence.as_ref(),
        task.resurface_date,
        task.series_id.as_deref(),
        task.effort,
        task.deadline_reminder_days,
    );
    if let Some(payload) = encode_payload(&extras) {
        todo.add_property("X-APERIO-EXTRAS", payload);
    }

    // Subtask link: RELATED-TO names the PARENT's UID (RFC 5545 §3.2.15;
    // the parameter-less form defaults to RELTYPE=PARENT, which is what
    // every other client emits). `parent_id` may be our composite
    // `{href}|{uid}` — strip it to the bare UID so other clients (and our
    // own read path) can resolve it. No parent ⇒ no property, which is
    // also how an update REMOVES the link: the whole VTODO is regenerated.
    if let Some(parent) = &task.parent_id {
        let (_, parent_uid) = decode_id(parent);
        if !parent_uid.is_empty() {
            todo.add_property("RELATED-TO", parent_uid);
        }
    }
}

/// `raw` is the resource's raw iCalendar text when the caller has it —
/// needed for a correct RELATED-TO read (see the comment at the
/// extraction below); `None` falls back to the parsed component.
fn map_todo(todo: &Todo, list_id: &str, href: Option<&str>, raw: Option<&str>) -> Option<Task> {
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
        _ => uid_raw.clone(),
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

    let (scheduled_date, scheduled_time) = local_date_time(read_task_dt(todo, "DTSTART"));
    let (deadline_date, deadline_time) = local_date_time(read_task_dt(todo, "DUE"));
    let scheduled_end_time = read_planned_end(todo, scheduled_time);
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

    // Subtask link: RELATED-TO carries the PARENT's bare UID (a missing
    // RELTYPE parameter defaults to PARENT per RFC 5545 §3.2.15;
    // CHILD/SIBLING entries are other clients' bookkeeping and are
    // ignored). The bare UID is rewritten to the fetched set's composite
    // `{href}|{uid}` id by `resolve_parent_ids` once the whole set is
    // known — a raw `map_todo` result is NOT directly comparable to task
    // ids yet.
    //
    // The link is scanned from the RAW iCalendar text when the caller has
    // it: icalendar 0.16.17 files RELATED-TO into its single-value
    // property map (the property is missing from the parser's multi
    // list), so of several RELATED-TO lines — RFC 5545 allows any number,
    // and clients like jtx Board write reciprocal RELTYPE=CHILD entries
    // next to the parent link — only the LAST survives parsing. Reading
    // the parsed component would flatten such tasks order-dependently,
    // and the next Aperio edit would then regenerate the VTODO without
    // the link, deleting it from the server. The parsed-property fallback
    // below only serves callers without the raw text.
    let parent_uid = raw
        .and_then(|text| parent_uid_from_raw(text, &uid_raw))
        .unwrap_or_else(|| {
            todo.properties()
                .get("RELATED-TO")
                .filter(|p| {
                    related_to_is_parent(
                        p.params()
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("RELTYPE"))
                            .map(|(_, v)| v.value()),
                    )
                })
                .map(|p| p.value().to_string())
        })
        .filter(|uid| !uid.is_empty());

    // DESIGN §9.12: a plain scheduled rule comes from RRULE; the on-demand
    // axes / resurface_date / series_id come from the X-property and
    // override it (the bag's recurrence is authoritative when present).
    let mut recurrence = todo
        .property_value("RRULE")
        .and_then(rrule_to_task_recurrence);
    let mut resurface_date = None;
    let mut series_id = None;
    let mut effort = cal_core::TaskEffort::default();
    let mut deadline_reminder_days = None;
    if let Some(extras) = todo
        .property_value("X-APERIO-EXTRAS")
        .and_then(decode_payload)
    {
        apply_task_extras(
            &extras,
            &mut recurrence,
            &mut resurface_date,
            &mut series_id,
            &mut effort,
            &mut deadline_reminder_days,
        );
    }

    Some(Task {
        assignees: Vec::new(),
        id: uid,
        list_id: list_id.to_string(),
        title,
        description,
        status,
        priority,
        effort,
        scheduled_date,
        scheduled_time,
        scheduled_end_time,
        deadline_date,
        deadline_time,
        deadline_reminder_days,
        recurrence,
        parent_id: parent_uid,
        section_id: None,
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        created_at,
        updated_at,
        completed_at,
        etag: None,
        resurface_date,
        series_id,
    })
}

/// DTSTART or DUE as a typed value, falling back to the layouts servers emit
/// when the property is not strictly conformant.
///
/// The typed reader is the one that understands the `TZID` parameter, so it
/// goes first. It also insists on `VALUE=DATE` before it will read `20260520`
/// as a date — correct per RFC 5545, and not what every server sends. A bare
/// date with no parameter is common enough in the wild (it is what one of this
/// module's own fixtures carries) that refusing it would re-open the bug this
/// function exists to close, one shape further along.
fn read_task_dt(todo: &Todo, prop: &str) -> Option<DatePerhapsTime> {
    if let Some(typed) = match prop {
        "DTSTART" => todo.get_start(),
        "DUE" => todo.get_due(),
        _ => None,
    } {
        return Some(typed);
    }
    let raw = todo.property_value(prop)?;
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y%m%d") {
        return Some(DatePerhapsTime::Date(date));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%SZ") {
        return Some(DatePerhapsTime::DateTime(CalendarDateTime::Utc(
            Utc.from_utc_datetime(&naive),
        )));
    }
    // A naive DATE-TIME. Its zone, if the property names one, rides the TZID
    // parameter rather than the value — which is exactly the case the old
    // string parser could not see.
    let naive = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S").ok()?;
    match todo
        .properties()
        .get(prop)
        .and_then(|p| p.params().get("TZID"))
        .map(|p| p.value().to_string())
    {
        Some(tzid) => Some(DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone {
            date_time: naive,
            tzid,
        })),
        None => Some(DatePerhapsTime::DateTime(CalendarDateTime::Floating(naive))),
    }
}

/// An iCalendar DTSTART / DUE value as cal-core stores it: a calendar date
/// plus an optional LOCAL wall-clock time.
///
/// It takes the typed `DatePerhapsTime` the icalendar crate parses (the same
/// shape the event path resolves in `mapping.rs`) rather than re-reading the
/// raw string, because the string alone cannot answer the question. RFC 5545
/// gives a DATE-TIME three forms — UTC with a `Z`, zoned via a `TZID`
/// PARAMETER, and floating — and the old string parser recognised exactly two
/// literal layouts, `YYYYMMDD` and `YYYYMMDDTHHMMSSZ`. Everything else fell
/// through to "no date at all": a VTODO written by Thunderbird, Tasks.org or
/// iCloud as `DUE;TZID=Europe/Berlin:20260812T090000` did not merely lose its
/// time in Aperio, it lost its DAY and showed up undated.
///
/// All three forms land on the same answer, the local wall clock, because that
/// is what `Task.scheduled_time` / `deadline_time` mean everywhere else in the
/// app (the reminder engine reads them back through `Local`). A zoned value is
/// resolved in its own zone and then converted; a floating one is already local
/// by definition and is taken verbatim.
///
/// A DATE value yields no time — that is iCalendar's all-day shape, and the
/// absence of a time is how Aperio says the same thing.
///
/// A time of exactly 00:00 also reads as "no time". That is the price of the
/// value-type rule on the write side (see `apply_task_dates`): a task with a
/// deadline time but no scheduled time has to emit midnight for DTSTART, and
/// midnight is the one wall-clock value that cannot then be told apart from a
/// deliberate choice. A task genuinely scheduled at midnight reads back as
/// scheduled for the day, which is the smaller of the two losses.
fn local_date_time(value: Option<DatePerhapsTime>) -> (Option<NaiveDate>, Option<NaiveTime>) {
    let Some(value) = value else {
        return (None, None);
    };
    let local: NaiveDateTime = match value {
        DatePerhapsTime::Date(d) => return (Some(d), None),
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(instant)) => {
            instant.with_timezone(&Local).naive_local()
        }
        DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time, tzid }) => {
            match resolve_in_zone(date_time, &tzid) {
                Some(instant) => instant.with_timezone(&Local).naive_local(),
                // An unknown TZID must not cost the DAY. Reading the value as
                // local wall-clock keeps the date and is at worst an
                // offset-sized error on the time.
                None => date_time,
            }
        }
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(naive)) => naive,
    };
    let time = local.time();
    if time == NaiveTime::from_hms_opt(0, 0, 0).expect("00:00 is a valid time") {
        (Some(local.date()), None)
    } else {
        (Some(local.date()), Some(time))
    }
}

/// The end of a task's planned block, from whichever of the two carriers the
/// VTODO uses.
///
/// RFC 5545 says a VTODO's span is `DTSTART`..`DUE`, or `DTSTART` plus
/// `DURATION` — and that DUE and DURATION are mutually exclusive. Aperio
/// already spends DUE on the DEADLINE, which is a different question ("by
/// when") from the block ("while"), so the two cannot both live in the
/// standard slots. DURATION carries it where there is no deadline in the way,
/// which is the common case and the one Thunderbird draws as a real block;
/// where a deadline occupies DUE, an X- property carries it instead. Both are
/// read, whichever a server hands back.
fn read_planned_end(todo: &Todo, start: Option<NaiveTime>) -> Option<NaiveTime> {
    let start = start?;
    let end = match todo.property_value("DURATION").and_then(parse_iso_duration) {
        Some(span) => start.overflowing_add_signed(span).0,
        None => {
            let raw = todo.property_value(X_SCHEDULED_END)?;
            NaiveTime::parse_from_str(raw, "%H%M%S").ok()?
        }
    };
    (end > start).then_some(end)
}

/// An iCalendar DURATION as a chrono span. Only the shapes a task block can
/// have are accepted — days, hours, minutes, seconds, no weeks and no negative
/// sign, since a block that runs backwards is not one.
fn parse_iso_duration(raw: &str) -> Option<chrono::TimeDelta> {
    let rest = raw.strip_prefix('P')?;
    let (date_part, time_part) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    let mut seconds: i64 = 0;
    let mut digits = String::new();
    for ch in date_part.chars() {
        match ch {
            '0'..='9' => digits.push(ch),
            'D' => seconds += digits.parse::<i64>().ok()? * 86_400,
            // Weeks (and anything else) describe a span no single day can hold.
            _ => return None,
        }
        if !ch.is_ascii_digit() {
            digits.clear();
        }
    }
    if let Some(time_part) = time_part {
        for ch in time_part.chars() {
            match ch {
                '0'..='9' => digits.push(ch),
                'H' => seconds += digits.parse::<i64>().ok()? * 3_600,
                'M' => seconds += digits.parse::<i64>().ok()? * 60,
                'S' => seconds += digits.parse::<i64>().ok()?,
                _ => return None,
            }
            if !ch.is_ascii_digit() {
                digits.clear();
            }
        }
    }
    (seconds > 0).then(|| chrono::TimeDelta::seconds(seconds))
}

/// Where a planned end goes when DUE is already spoken for by the deadline.
const X_SCHEDULED_END: &str = "X-APERIO-SCHEDULED-END";

/// Write DTSTART and DUE for a task.
///
/// A time is emitted FLOATING — `20260812T090000`, no `Z`, no `TZID`. That is
/// not a shortcut, it is the only honest encoding of what cal-core holds: a
/// naive wall-clock time with no zone attached. The old code declared that time
/// to be UTC, so a task set to 09:00 in Berlin reached every other client as
/// 11:00; Aperio agreed with itself only because it made the same mistake in
/// reverse when reading. Floating means "09:00 wherever you are", which is what
/// the user chose, and it needs no VTIMEZONE — a TZID that no VTIMEZONE defines
/// is the shape iCloud has already been seen to drop on the floor.
///
/// The two properties are written with the SAME value type, because RFC 5545
/// §3.8.2.3 requires it of DUE ("MUST be the same as the DTSTART property").
/// A task with a deadline time but no scheduled time therefore emits midnight
/// for DTSTART rather than a bare DATE next to a DATE-TIME. `local_date_time`
/// reads midnight back as "no time", which closes the round trip.
fn apply_task_dates(
    todo: &mut Todo,
    scheduled_date: Option<NaiveDate>,
    scheduled_time: Option<NaiveTime>,
    scheduled_end_time: Option<NaiveTime>,
    deadline_date: Option<NaiveDate>,
    deadline_time: Option<NaiveTime>,
) {
    // Either property carrying a time forces both to DATE-TIME.
    let timed = scheduled_time.is_some() || deadline_time.is_some();
    let value_for = |date: NaiveDate, time: Option<NaiveTime>| -> DatePerhapsTime {
        if timed {
            let at = time.unwrap_or_else(|| {
                NaiveTime::from_hms_opt(0, 0, 0).expect("00:00 is a valid time")
            });
            DatePerhapsTime::DateTime(CalendarDateTime::Floating(date.and_time(at)))
        } else {
            DatePerhapsTime::Date(date)
        }
    };
    if let Some(date) = scheduled_date {
        let value = value_for(date, scheduled_time);
        todo.append_property(value.to_property("DTSTART"));
    }
    if let Some(date) = deadline_date {
        todo.due(value_for(date, deadline_time));
    }
    // The block's end. DURATION is the standard carrier and the one other
    // clients understand, but RFC 5545 §3.8.2.3 forbids it beside DUE — so a
    // task that also has a deadline hands the end to an X- property instead of
    // emitting something no server should accept.
    if let (Some(start), Some(end)) = (scheduled_time, scheduled_end_time) {
        if end > start {
            let minutes = (end - start).num_minutes();
            if deadline_date.is_none() {
                todo.add_property("DURATION", format!("PT{minutes}M"));
            } else {
                todo.add_property(X_SCHEDULED_END, end.format("%H%M%S").to_string());
            }
        }
    }
}

/// A naive wall-clock time read in a named IANA zone. `None` for a zone name
/// chrono-tz does not know (Windows-style names, or a server's invention) and
/// for a local time that does not exist there (the hour a DST jump skips).
fn resolve_in_zone(naive: NaiveDateTime, tzid: &str) -> Option<DateTime<Utc>> {
    let tz: chrono_tz::Tz = tzid.parse().ok()?;
    Some(tz.from_local_datetime(&naive).single()?.with_timezone(&Utc))
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

/// `uid → task id` lookup over mapped tasks, for resolving the bare
/// parent UIDs a `RELATED-TO` property carries into the composite
/// `{href}|{uid}` ids the rest of the app compares against.
pub fn uid_index(tasks: &[Task]) -> std::collections::HashMap<String, String> {
    tasks
        .iter()
        .map(|t| {
            let (_, uid) = decode_id(&t.id);
            (uid.to_string(), t.id.clone())
        })
        .collect()
}

/// True when at least one task's `parent_id` (a bare UID fresh out of
/// `map_todo`) has no entry in `index` — i.e. the parent lives outside
/// the mapped set and resolving needs a wider index.
pub fn any_unresolved_parent(
    tasks: &[Task],
    index: &std::collections::HashMap<String, String>,
) -> bool {
    tasks.iter().any(|t| {
        t.parent_id
            .as_ref()
            .is_some_and(|uid| !index.contains_key(uid))
    })
}

/// Rewrite the bare parent UIDs `map_todo` leaves in `parent_id` into
/// the composite ids from `index`, so a subtask's `parent_id` matches
/// its parent's `Task.id` exactly. A UID with no entry (parent deleted
/// or a dangling RELATED-TO) and a self-reference (malformed VTODO —
/// keeping it would send the UI's tree walk in a circle) both clear to
/// `None`.
pub fn resolve_parent_ids(tasks: &mut [Task], index: &std::collections::HashMap<String, String>) {
    for task in tasks.iter_mut() {
        let Some(uid) = task.parent_id.take() else {
            continue;
        };
        match index.get(&uid) {
            Some(id) if *id != task.id => task.parent_id = Some(id.clone()),
            _ => {}
        }
    }
}

/// Unfold RFC 5545 §3.1 line continuations: a line break followed by a
/// space or tab continues the previous line. Handles CRLF and bare-LF
/// input (servers emit both).
fn unfold_ical(raw: &str) -> String {
    raw.replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "")
}

/// Split one unfolded content line into its `name[;params]` head and its
/// value, honouring quoted parameter values (a `:` inside `"…"` is not
/// the separator).
fn split_property_line(line: &str) -> Option<(&str, &str)> {
    let mut in_quotes = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => return Some((&line[..i], &line[i + 1..])),
            _ => {}
        }
    }
    None
}

/// Whether a RELATED-TO's RELTYPE value means "this names my parent".
/// `None` (no parameter) defaults to PARENT per RFC 5545 §3.2.15; the
/// compare is case-insensitive and tolerates a quoted value.
fn related_to_is_parent(reltype: Option<&str>) -> bool {
    match reltype {
        None => true,
        Some(v) => v.trim_matches('"').eq_ignore_ascii_case("PARENT"),
    }
}

/// Scan the raw iCalendar text for the parent UID of the VTODO carrying
/// `uid`. Returns `None` when no VTODO block with that UID exists in the
/// text (caller falls back to the parsed component), `Some(None)` when
/// the block exists but has no parent-typed RELATED-TO, and
/// `Some(Some(parent_uid))` for the first parent link.
///
/// This exists because icalendar 0.16.17 keeps only ONE RELATED-TO per
/// component (last line wins — the property is missing from its
/// multi-property list), while RFC 5545 allows several and real clients
/// write reciprocal RELTYPE=CHILD entries next to the parent link.
/// Names and parameter keys compare case-insensitively (RFC 5545 §2).
fn parent_uid_from_raw(raw: &str, uid: &str) -> Option<Option<String>> {
    let unfolded = unfold_ical(raw);
    let mut in_vtodo = false;
    let mut block_uid: Option<String> = None;
    let mut block_parent: Option<String> = None;
    for line in unfolded.lines() {
        let line = line.trim_end_matches('\r');
        if line.eq_ignore_ascii_case("BEGIN:VTODO") {
            in_vtodo = true;
            block_uid = None;
            block_parent = None;
            continue;
        }
        if line.eq_ignore_ascii_case("END:VTODO") {
            if in_vtodo && block_uid.as_deref() == Some(uid) {
                return Some(block_parent);
            }
            in_vtodo = false;
            continue;
        }
        if !in_vtodo {
            continue;
        }
        let Some((head, value)) = split_property_line(line) else {
            continue;
        };
        let mut parts = head.split(';');
        let name = parts.next().unwrap_or("");
        if name.eq_ignore_ascii_case("UID") {
            block_uid = Some(value.trim().to_string());
        } else if name.eq_ignore_ascii_case("RELATED-TO") && block_parent.is_none() {
            let reltype = parts
                .filter_map(|param| param.split_once('='))
                .find(|(k, _)| k.trim().eq_ignore_ascii_case("RELTYPE"))
                .map(|(_, v)| v);
            if related_to_is_parent(reltype) {
                let value = value.trim();
                if !value.is_empty() {
                    block_parent = Some(value.to_string());
                }
            }
        }
    }
    None
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
            effort: cal_core::TaskEffort::Medium,
            deadline_reminder_days: None,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            scheduled_time: None,
            scheduled_end_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            resurface_date: None,
            series_id: None,
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

    #[tokio::test]
    async fn get_tasks_resolves_related_to_into_the_parents_composite_id() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/tasks/p.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"p"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//test//EN
BEGIN:VTODO
UID:parent-1@aperio
SUMMARY:Parent
STATUS:NEEDS-ACTION
END:VTODO
END:VCALENDAR</c:calendar-data>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/tasks/c.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"c"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//test//EN
BEGIN:VTODO
UID:child-1@aperio
SUMMARY:Child
STATUS:NEEDS-ACTION
RELATED-TO:parent-1@aperio
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
        // The bare RELATED-TO UID resolves to the parent's full
        // `{href}|{uid}` id, so the subtask groups under it everywhere
        // task ids are compared.
        let child = tasks.iter().find(|t| t.title == "Child").unwrap();
        assert_eq!(
            child.parent_id.as_deref(),
            Some("/calendars/alice/tasks/p.ics|parent-1@aperio"),
        );
        let parent = tasks.iter().find(|t| t.title == "Parent").unwrap();
        assert_eq!(parent.parent_id, None);
    }

    /// Parse a one-VTODO calendar exactly like the read path does and map
    /// it. `extra_lines` lets a test inject e.g. a RELATED-TO property
    /// (include the trailing newline).
    fn mapped_task(uid: &str, extra_lines: &str, href: &str) -> Task {
        let body = format!(
            "BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:-//test//EN\nBEGIN:VTODO\nUID:{uid}\nSUMMARY:x\nSTATUS:NEEDS-ACTION\n{extra_lines}END:VTODO\nEND:VCALENDAR",
        );
        let parsed = body.parse::<ICalendar>().expect("valid ical");
        let todo = parsed
            .components
            .iter()
            .find_map(|c| match c {
                icalendar::CalendarComponent::Todo(t) => Some(t),
                _ => None,
            })
            .expect("has a VTODO");
        map_todo(todo, "list", Some(href), Some(&body)).expect("maps")
    }

    #[test]
    fn map_todo_reads_only_parent_typed_related_to() {
        // Parameter-less RELATED-TO defaults to RELTYPE=PARENT → captured
        // as the (unresolved) bare parent UID.
        let plain = mapped_task("c@x", "RELATED-TO:p@x\n", "/cal/c.ics");
        assert_eq!(plain.parent_id.as_deref(), Some("p@x"));
        // An explicit PARENT (any case, quoted or not) is the same link.
        let explicit = mapped_task("c@x", "RELATED-TO;RELTYPE=parent:p@x\n", "/cal/c.ics");
        assert_eq!(explicit.parent_id.as_deref(), Some("p@x"));
        let quoted = mapped_task("c@x", "RELATED-TO;RELTYPE=\"PARENT\":p@x\n", "/cal/c.ics");
        assert_eq!(quoted.parent_id.as_deref(), Some("p@x"));
        // CHILD/SIBLING relations are other clients' bookkeeping, not a
        // parent link — regardless of the parameter key's case (RFC 5545
        // names are case-insensitive; some emitters lowercase them).
        let child_typed = mapped_task("c@x", "RELATED-TO;RELTYPE=CHILD:p@x\n", "/cal/c.ics");
        assert_eq!(child_typed.parent_id, None);
        let lowercase = mapped_task("c@x", "RELATED-TO;reltype=CHILD:p@x\n", "/cal/c.ics");
        assert_eq!(lowercase.parent_id, None);
    }

    #[test]
    fn map_todo_survives_multiple_related_to_lines() {
        // RFC 5545 allows several RELATED-TO per component; clients like
        // jtx Board keep reciprocal RELTYPE=CHILD bookkeeping next to the
        // parent link. icalendar 0.16.17 only keeps the LAST parsed line,
        // so the raw-text scan must find the parent REGARDLESS of order —
        // before it, a trailing CHILD line flattened the task and the next
        // Aperio edit deleted the parent link from the server.
        let child_last = mapped_task(
            "c@x",
            "RELATED-TO:p@x\nRELATED-TO;RELTYPE=CHILD:kid@x\n",
            "/cal/c.ics",
        );
        assert_eq!(child_last.parent_id.as_deref(), Some("p@x"));
        let child_first = mapped_task(
            "c@x",
            "RELATED-TO;RELTYPE=CHILD:kid@x\nRELATED-TO:p@x\n",
            "/cal/c.ics",
        );
        assert_eq!(child_first.parent_id.as_deref(), Some("p@x"));
        // Only CHILD entries ⇒ no parent.
        let only_child = mapped_task(
            "c@x",
            "RELATED-TO;RELTYPE=CHILD:kid@x\nRELATED-TO;RELTYPE=CHILD:kid2@x\n",
            "/cal/c.ics",
        );
        assert_eq!(only_child.parent_id, None);
    }

    #[test]
    fn parent_uid_from_raw_unfolds_continuation_lines() {
        // RFC 5545 §3.1 folds long lines with CRLF + space; the raw scan
        // must see the logical line, not the physical fragments.
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:c@x\r\nRELATED-TO:very-long-\r\n parent-uid@x\r\nEND:VTODO\r\nEND:VCALENDAR";
        assert_eq!(
            parent_uid_from_raw(raw, "c@x"),
            Some(Some("very-long-parent-uid@x".into())),
        );
        // Unknown UID ⇒ outer None (caller falls back to the parsed
        // component); block without a parent link ⇒ Some(None).
        assert_eq!(parent_uid_from_raw(raw, "other@x"), None);
        let flat = "BEGIN:VTODO\nUID:c@x\nSUMMARY:x\nEND:VTODO";
        assert_eq!(parent_uid_from_raw(flat, "c@x"), Some(None));
    }

    #[test]
    fn resolve_parent_ids_maps_dangling_and_self_references_to_none() {
        // Resolvable: the child's bare UID becomes the parent's composite id.
        let parent = mapped_task("p@x", "", "/cal/p.ics");
        let child = mapped_task("c@x", "RELATED-TO:p@x\n", "/cal/c.ics");
        let mut tasks = vec![parent, child];
        let index = uid_index(&tasks);
        assert!(!any_unresolved_parent(&tasks, &index));
        resolve_parent_ids(&mut tasks, &index);
        assert_eq!(tasks[1].parent_id.as_deref(), Some("/cal/p.ics|p@x"));

        // Dangling: the referenced UID isn't in the set → flagged for the
        // delta path's wider lookup, cleared on resolve.
        let mut dangling = vec![mapped_task("c@x", "RELATED-TO:gone@x\n", "/cal/c.ics")];
        let index = uid_index(&dangling);
        assert!(any_unresolved_parent(&dangling, &index));
        resolve_parent_ids(&mut dangling, &index);
        assert_eq!(dangling[0].parent_id, None);

        // Self-reference: a malformed VTODO naming itself must not produce
        // a task that is its own parent (the UI's tree walk would cycle).
        let mut selfy = vec![mapped_task("s@x", "RELATED-TO:s@x\n", "/cal/s.ics")];
        let index = uid_index(&selfy);
        resolve_parent_ids(&mut selfy, &index);
        assert_eq!(selfy[0].parent_id, None);
    }

    #[test]
    fn build_vtodo_completed_carries_the_completion_instant() {
        // STATUS:COMPLETED alone is a shape Apple's own clients never write
        // (EventKit keys completion on the date); a server normalizing it
        // could silently reopen the task. The body builder must emit the
        // COMPLETED property whenever a completion instant is supplied.
        let mut new = sample_new_task();
        new.status = TaskStatus::Completed;
        let body = build_vtodo_body("uid-done", &new, Some(Utc::now()));
        assert!(body.contains("STATUS:COMPLETED"), "got:\n{body}");
        assert!(body.contains("COMPLETED:"), "got:\n{body}");
    }

    #[tokio::test]
    async fn create_task_completed_stamps_completed_at() {
        // The create path supplies that instant itself (NewTask carries
        // none) and reports it on the returned Task, so later edits keep
        // re-uploading the COMPLETED property instead of stripping it.
        let mut server = Server::new_async().await;
        let m = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/tasks/.+\.ics$".into()),
            )
            .match_body(mockito::Matcher::Regex(r"COMPLETED:\d{8}T\d{6}Z".into()))
            .with_status(201)
            .with_header("etag", "\"done\"")
            .create_async()
            .await;
        let url = Url::parse(&format!("{}/calendars/alice/tasks/", server.url())).unwrap();
        let mut new = sample_new_task();
        new.status = TaskStatus::Completed;
        let created = create_task(&client(), &url, new, &creds(&server.url()))
            .await
            .unwrap();
        m.assert_async().await;
        assert_eq!(created.status, TaskStatus::Completed);
        assert!(created.completed_at.is_some());
    }

    #[test]
    fn build_vtodo_emits_related_to_with_the_bare_parent_uid() {
        let mut new = sample_new_task();
        // The caller hands us the parent's COMPOSITE id; the wire property
        // must carry only the UID so other clients can resolve it.
        new.parent_id = Some("/calendars/alice/tasks/p.ics|parent-1@aperio".into());
        let body = build_vtodo_body("uid-child", &new, None);
        assert!(
            body.contains("RELATED-TO:parent-1@aperio"),
            "VTODO must carry the bare parent UID, got:\n{body}",
        );
        // No parent ⇒ no property (also how an update removes the link,
        // since the whole VTODO is regenerated).
        let mut flat = sample_new_task();
        flat.parent_id = None;
        let body = build_vtodo_body("uid-flat", &flat, None);
        assert!(!body.contains("RELATED-TO"), "got:\n{body}");
    }

    #[tokio::test]
    async fn get_tasks_keeps_the_day_of_a_zoned_vtodo() {
        // What Thunderbird, Tasks.org and Nextcloud write when a task has a
        // time: the value is a naive wall clock and the zone rides a TZID
        // PARAMETER. The old parser matched the value against two literal
        // layouts, neither of which this is, so the task arrived with NO date
        // and sat in the backlog as if it had never been planned.
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/tasks/zoned.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"zoned-1"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//test//EN
BEGIN:VTODO
UID:zoned@aperio
SUMMARY:Take pills
STATUS:NEEDS-ACTION
DTSTART;TZID=Europe/Berlin:20260812T120000
DUE;TZID=Europe/Berlin:20260812T180000
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
        // Midday in Berlin is the same calendar day at every offset a test
        // machine plausibly runs at, so the DAY is assertable outright.
        assert_eq!(
            tasks[0].scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()),
            "a zoned DTSTART must not cost the task its day",
        );
        assert!(
            tasks[0].scheduled_time.is_some(),
            "and it carries a time of day",
        );
        assert_eq!(
            tasks[0].deadline_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()),
        );
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
            effort: cal_core::TaskEffort::Medium,
            deadline_reminder_days: None,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap()),
            scheduled_time: None,
            scheduled_end_time: None,
            deadline_date: Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            resurface_date: None,
            series_id: None,
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
    fn build_vtodo_with_due_time_emits_floating_datetime() {
        // The other half of the same property: when the user picked a specific
        // time of day, DUE is a FLOATING DATE-TIME — no VALUE parameter (RFC
        // 5545's default) and no `Z`. It used to be written as UTC, which
        // declared a Berlin user's 14:30 to be 16:30 for every other client.
        let new = NewTask {
            assignees: Vec::new(),
            title: "Status call".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            effort: cal_core::TaskEffort::Medium,
            deadline_reminder_days: None,
            scheduled_date: None,
            scheduled_time: None,
            scheduled_end_time: None,
            deadline_date: Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
            deadline_time: Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap()),
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            resurface_date: None,
            series_id: None,
        };
        let body = build_vtodo_body("uid-2", &new, None);
        assert!(
            body.contains("DUE:20260522T143000\r\n") || body.contains("DUE:20260522T143000\n"),
            "DUE must be a floating DATE-TIME when a time is set, got:\n{body}",
        );
        assert!(
            !body.contains("DUE:20260522T143000Z"),
            "the wall clock the user chose must not be labelled UTC, got:\n{body}",
        );
        assert!(
            !body.contains("VALUE=DATE-TIME"),
            "DUE date-time should not carry VALUE=DATE-TIME (RFC 5545 default), got:\n{body}",
        );
    }

    #[test]
    fn build_vtodo_matches_the_value_types_of_dtstart_and_due() {
        // RFC 5545 §3.8.2.3: DUE's value type MUST be the same as DTSTART's. A
        // task planned for a day but due at a time would otherwise emit a bare
        // DATE next to a DATE-TIME, which is malformed — and malformed is the
        // shape iCloud answers by dropping the property.
        let mut new = sample_new_task();
        new.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap());
        new.scheduled_time = None;
        new.deadline_date = Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap());
        new.deadline_time = Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap());
        let body = build_vtodo_body("uid-mixed", &new, None);
        assert!(
            body.contains("DTSTART:20260521T000000"),
            "DTSTART must follow DUE into DATE-TIME, got:\n{body}",
        );
        assert!(
            !body.contains("VALUE=DATE:"),
            "neither property may stay a DATE while the other is a DATE-TIME, got:\n{body}",
        );
        // … and midnight reads back as "no time", so the round trip is closed.
        assert_eq!(
            local_date_time(Some(DatePerhapsTime::DateTime(CalendarDateTime::Floating(
                NaiveDate::from_ymd_opt(2026, 5, 21)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            )))),
            (Some(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap()), None),
        );
    }

    // ── reading every shape RFC 5545 allows ────────────────────────────────

    #[test]
    fn reads_a_floating_date_time_as_the_wall_clock_it_is() {
        let naive = NaiveDate::from_ymd_opt(2026, 8, 12)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        assert_eq!(
            local_date_time(Some(DatePerhapsTime::DateTime(CalendarDateTime::Floating(
                naive
            )))),
            (
                Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()),
                Some(NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
            ),
        );
    }

    #[test]
    fn reads_a_date_value_as_a_day_without_a_time() {
        assert_eq!(
            local_date_time(Some(DatePerhapsTime::Date(
                NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()
            ))),
            (Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()), None),
        );
    }

    #[test]
    fn a_zoned_date_time_keeps_its_day() {
        // THE regression. `DUE;TZID=Europe/Berlin:20260812T120000` matched
        // neither branch of the old string parser, so the task arrived with no
        // date at all — not a wrong time, no day. Midday keeps the assertion
        // true at every offset a test machine might run at.
        let berlin = chrono_tz::Europe::Berlin;
        let naive = NaiveDate::from_ymd_opt(2026, 8, 12)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let expected = berlin
            .from_local_datetime(&naive)
            .single()
            .unwrap()
            .with_timezone(&Local)
            .naive_local();
        assert_eq!(
            local_date_time(Some(DatePerhapsTime::DateTime(
                CalendarDateTime::WithTimezone {
                    date_time: naive,
                    tzid: "Europe/Berlin".into(),
                }
            ))),
            (Some(expected.date()), Some(expected.time())),
        );
    }

    #[test]
    fn an_unknown_zone_still_costs_only_the_offset_never_the_day() {
        // Windows zone names and server inventions do not parse. Falling back
        // to the wall clock keeps the day, which is the part that matters.
        assert_eq!(
            local_date_time(Some(DatePerhapsTime::DateTime(
                CalendarDateTime::WithTimezone {
                    date_time: NaiveDate::from_ymd_opt(2026, 8, 12)
                        .unwrap()
                        .and_hms_opt(12, 0, 0)
                        .unwrap(),
                    tzid: "W. Europe Standard Time".into(),
                }
            ))),
            (
                Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()),
                Some(NaiveTime::from_hms_opt(12, 0, 0).unwrap()),
            ),
        );
    }

    #[test]
    fn a_utc_date_time_arrives_in_local_time() {
        let instant = Utc
            .with_ymd_and_hms(2026, 8, 12, 12, 0, 0)
            .single()
            .unwrap();
        let expected = instant.with_timezone(&Local).naive_local();
        assert_eq!(
            local_date_time(Some(DatePerhapsTime::DateTime(CalendarDateTime::Utc(
                instant
            )))),
            (Some(expected.date()), Some(expected.time())),
        );
    }

    #[test]
    fn build_vtodo_round_trips_recurrence_through_rrule() {
        use cal_core::{RecurrenceEnd, RecurrenceFrequency, TaskRecurrence, Weekday};
        let rec = TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 2,
            day_of_week: Some(vec![Weekday::Monday, Weekday::Wednesday]),
            day_of_month: None,
            end: Some(RecurrenceEnd::After { occurrences: 6 }),
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
        };
        let new = NewTask {
            assignees: Vec::new(),
            title: "Water plants".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            effort: cal_core::TaskEffort::Medium,
            deadline_reminder_days: None,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap()),
            scheduled_time: None,
            scheduled_end_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: Some(rec.clone()),
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            resurface_date: None,
            series_id: None,
        };
        let body = build_vtodo_body("uid-rec", &new, None);
        assert!(
            body.contains("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=6"),
            "VTODO must carry the RRULE, got:\n{body}",
        );
        // Parse it back exactly as the read path does and confirm the
        // structured recurrence survives the round-trip.
        let parsed = body.parse::<ICalendar>().expect("valid ical");
        let todo = parsed
            .components
            .iter()
            .find_map(|c| match c {
                icalendar::CalendarComponent::Todo(t) => Some(t),
                _ => None,
            })
            .expect("has a VTODO");
        let task = map_todo(todo, "list", None, None).expect("maps");
        assert_eq!(task.recurrence, Some(rec));
    }

    #[test]
    fn build_vtodo_carries_backlog_extras_in_x_property() {
        use cal_core::{
            MonthDay, RecurrenceAnchor, RecurrenceEnd, RecurrenceFrequency, RecurrencePlacement,
            TaskRecurrence,
        };
        let rec = TaskRecurrence {
            frequency: RecurrenceFrequency::Yearly,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: Some(RecurrenceEnd::Never),
            anchor: RecurrenceAnchor::FromCompletion,
            placement: RecurrencePlacement::Backlog,
            fixed_dates: Some(vec![
                MonthDay { month: 4, day: 1 },
                MonthDay { month: 10, day: 1 },
            ]),
        };
        let mut new = sample_new_task();
        new.title = "Swap shoes".into();
        new.recurrence = Some(rec.clone());
        new.resurface_date = Some(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        new.series_id = Some("series-shoes".into());

        let body = build_vtodo_body("uid-bl", &new, None);
        // Rides the invisible X-property; no RRULE for a backlog rule.
        assert!(body.contains("X-APERIO-EXTRAS:aperio:1:"), "got:\n{body}");
        assert!(
            !body.contains("RRULE"),
            "a backlog rule must not emit an RRULE:\n{body}"
        );

        let parsed = body.parse::<ICalendar>().expect("valid ical");
        let todo = parsed
            .components
            .iter()
            .find_map(|c| match c {
                icalendar::CalendarComponent::Todo(t) => Some(t),
                _ => None,
            })
            .expect("has a VTODO");
        let task = map_todo(todo, "list", None, None).expect("maps");
        assert_eq!(task.recurrence, Some(rec));
        assert_eq!(
            task.resurface_date,
            Some(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
        );
        assert_eq!(task.series_id.as_deref(), Some("series-shoes"));
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
