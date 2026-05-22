//! Calendar and event commands.

use cal_adapter_local::LocalAdapter;
use cal_core::{Calendar, CalendarFeature, ContainerColor, DateRange, Event, NewEvent};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

use super::birthdays::{
    is_birthday_calendar_id, list_birthday_calendars, synthesise_birthday_events,
};
use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::overrides::{apply_to_calendars, OverridesRepo};
use crate::registry::{AdapterRegistry, LOCAL_ID};
use crate::reminders::SchedulerHandle;

/// Wire-format Calendar enriched with the owning account id. Lets
/// the frontend group containers by source without a second
/// round-trip to fetch the registry's route map.
///
/// `serde(flatten)` writes every Calendar field at the top level so
/// the existing TypeScript Calendar type only needs one new field
/// (`account_id`) to consume this shape.
#[derive(Debug, Serialize)]
pub struct CalendarRow {
    #[serde(flatten)]
    pub inner: Calendar,
    pub account_id: String,
}

/// Frontend-supplied payload for creating a local calendar.
#[derive(Debug, Deserialize)]
pub struct CreateCalendarRequest {
    pub name: String,
    pub color_hex: Option<String>,
}

#[tauri::command]
pub async fn list_calendars(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<CalendarRow>> {
    // Local first so the implicit "local" account stays at the top
    // of the user's calendar list. Each local calendar id is
    // pre-registered in the route map so the write-path commands
    // can recognise it without falling back to the legacy "assume
    // local" branch.
    let local = adapter.list_calendars().await?;
    for c in &local {
        registry.note_calendar_route(&c.id, LOCAL_ID);
    }
    let mut external = registry.list_external_calendars().await;
    let mut out = local;
    out.append(&mut external);
    // Stamp local rename overrides on top of whatever each adapter
    // returned. The adapter never sees the override so its
    // edit-path (where it exists) keeps writing into the source
    // server with the source name — the override is purely a
    // frontend-facing read-time projection.
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    apply_to_calendars(&repo, &mut out);
    // Synthesised birthday calendars (DESIGN.md §10.3) — one per
    // contact list that has at least one contact with a `birthday`
    // set. Each carries `read_only = true` so the UI hides edit /
    // delete affordances; rendered events flow through `get_events`
    // with the prefix-routed shim below. We stamp the route map
    // here too so subsequent `get_events` calls reach the right
    // contacts adapter via the same registry mechanism real
    // calendars use.
    let birthday_rows = list_birthday_calendars(&adapter, &registry).await;
    for (cal, account_id) in &birthday_rows {
        registry.note_calendar_route(&cal.id, account_id);
    }

    // Decorate each row with its owning account id (from the
    // registry's route map). Local rows fall back to LOCAL_ID;
    // external rows look themselves up in the routes. The frontend
    // uses this for the account-grouped sidebar — without it,
    // grouping would need a second round-trip.
    let mut decorated: Vec<CalendarRow> = out
        .into_iter()
        .map(|cal| {
            let account_id = registry
                .account_for_calendar(&cal.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            CalendarRow {
                inner: cal,
                account_id,
            }
        })
        .collect();
    for (cal, account_id) in birthday_rows {
        decorated.push(CalendarRow {
            inner: cal,
            account_id,
        });
    }
    Ok(decorated)
}

#[tauri::command]
pub async fn create_calendar(
    adapter: State<'_, LocalAdapter>,
    request: CreateCalendarRequest,
) -> CommandResult<CalendarRow> {
    let color = request
        .color_hex
        .map(|hex| ContainerColor::custom(hex.trim().to_string()));
    let cal = adapter.create_calendar(&request.name, color, None)?;
    // Local creates always belong to the implicit local account.
    Ok(CalendarRow {
        inner: cal,
        account_id: LOCAL_ID.to_string(),
    })
}

#[tauri::command]
pub async fn delete_calendar(adapter: State<'_, LocalAdapter>, id: String) -> CommandResult<()> {
    Ok(adapter.delete_calendar(&id)?)
}

#[derive(Debug, Deserialize)]
pub struct EventRangeRequest {
    pub calendar_id: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[tauri::command]
pub async fn get_events(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    request: EventRangeRequest,
) -> CommandResult<Vec<Event>> {
    let range = DateRange::new(request.start, request.end);
    // Synthesised birthday calendars short-circuit the adapter
    // routing — they aren't backed by any provider and have no
    // events of their own. The `synthesise_birthday_events`
    // helper computes them on the fly from the underlying
    // contact list. Returns `None` when the id doesn't carry the
    // birthday prefix; in that case we fall through to the
    // regular adapter path.
    if is_birthday_calendar_id(&request.calendar_id) {
        if let Some(events) =
            synthesise_birthday_events(&adapter, &registry, &request.calendar_id, range).await
        {
            return Ok(events);
        }
        // Unknown synthesised id (e.g. the underlying contact
        // list has been removed since the listing). Surface an
        // empty list rather than 404 — the sidebar still has
        // the layer ticked, the next refresh will drop it.
        return Ok(Vec::new());
    }
    let account =
        registry.account_for_calendar(&request.calendar_id).unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(adapter.get_events(&request.calendar_id, range).await?);
    }
    let Some(ext) = registry.calendar_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("calendar '{}' is not routable", request.calendar_id),
        });
    };
    Ok(ext.get_events(&request.calendar_id, range).await?)
}

#[derive(Debug, Deserialize)]
pub struct CreateEventRequest {
    pub calendar_id: String,
    #[serde(flatten)]
    pub event: NewEvent,
}

#[tauri::command]
pub async fn create_event(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    request: CreateEventRequest,
) -> CommandResult<Event> {
    let account =
        registry.account_for_calendar(&request.calendar_id).unwrap_or_else(|| LOCAL_ID.to_string());
    let event = if account == LOCAL_ID {
        adapter
            .create_event(&request.calendar_id, request.event)
            .await?
    } else {
        let Some(ext) = registry.calendar_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("calendar '{}' is not routable", request.calendar_id),
            });
        };
        ext.create_event(&request.calendar_id, request.event).await?
    };
    scheduler.invalidate();
    Ok(event)
}

#[tauri::command]
pub async fn update_event(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    event: Event,
) -> CommandResult<Event> {
    let account =
        registry.account_for_calendar(&event.calendar_id).unwrap_or_else(|| LOCAL_ID.to_string());
    let updated = if account == LOCAL_ID {
        adapter.update_event(event).await?
    } else {
        let Some(ext) = registry.calendar_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("calendar '{}' is not routable", event.calendar_id),
            });
        };
        ext.update_event(event).await?
    };
    scheduler.invalidate();
    Ok(updated)
}

#[tauri::command]
pub async fn delete_event(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    id: String,
    calendar_id: Option<String>,
) -> CommandResult<()> {
    // The frontend now passes the parent calendar_id so the registry
    // can route the delete to the right adapter. Older callers
    // (pre-6b.4) that only sent `id` are still served by the local
    // adapter — its own data model can locate the row by uid alone.
    let account = calendar_id
        .as_deref()
        .and_then(|cid| registry.account_for_calendar(cid))
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        adapter.delete_event(&id).await?;
    } else {
        let Some(ext) = registry.calendar_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("account '{account}' is not routable"),
            });
        };
        ext.delete_event(&id).await?;
    }
    scheduler.invalidate();
    Ok(())
}

#[tauri::command]
pub async fn get_event_by_id(
    adapter: State<'_, LocalAdapter>,
    id: String,
) -> CommandResult<Option<Event>> {
    Ok(adapter.get_event_by_id(&id)?)
}

#[tauri::command]
pub async fn add_event_exdate(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    id: String,
    occurrence: DateTime<Utc>,
    calendar_id: Option<String>,
) -> CommandResult<()> {
    // `calendar_id` was added in Phase 6b.7 — older callers that
    // only pass `id` fall back to "assume local", which is still
    // right for local-only events but would have wrongly hit the
    // local adapter when the event lived on iCloud / Nextcloud.
    let account = calendar_id
        .as_deref()
        .and_then(|cid| registry.account_for_calendar(cid))
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        adapter.add_event_exdate(&id, occurrence)?;
    } else {
        let Some(ext) = registry.calendar_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("account '{account}' is not routable"),
            });
        };
        ext.add_event_exdate(&id, occurrence).await?;
    }
    scheduler.invalidate();
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Smoke test: round-trip a calendar + event through the command layer
// using an in-memory adapter. Mirrors what the frontend will do at startup.
// ────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandError;
    use cal_adapter_local::{LocalAdapter, SharedConn};
    use chrono::Duration;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn fresh_adapter() -> LocalAdapter {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(include_str!("../db/sql/0001_init.sql"))
            .unwrap();
        let db: SharedConn = Arc::new(Mutex::new(conn));
        LocalAdapter::new(db)
    }

    #[tokio::test]
    async fn create_calendar_then_event() {
        let a = fresh_adapter();
        let cal = a.create_calendar("Work", None, None).unwrap();
        let now = Utc::now();
        let ev = a
            .create_event(
                &cal.id,
                NewEvent {
                    title: "Standup".into(),
                    description: None,
                    location: None,
                    start: now,
                    end: now + Duration::minutes(15),
                    all_day: false,
                    recurrence: None,
                    color_label: None,
                    reminders: vec![],
                    sound: None,
                    attendees: vec![],
                },
            )
            .await
            .unwrap();

        // The CommandError mapping must preserve the original error code.
        let err: CommandError = cal_core::Error::NotFound("x".into()).into();
        assert_eq!(err.code, "not_found");

        let cals = a.list_calendars().await.unwrap();
        assert_eq!(cals[0].id, cal.id);
        let evs = a
            .get_events(
                &cal.id,
                DateRange::new(now - Duration::minutes(1), now + Duration::hours(1)),
            )
            .await
            .unwrap();
        assert_eq!(evs[0].id, ev.id);
    }
}
