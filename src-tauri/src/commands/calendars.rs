//! Calendar and event commands.

use cal_adapter_local::LocalAdapter;
use cal_core::{Calendar, CalendarFeature, ContainerColor, DateRange, Event, NewEvent};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tauri::State;

use super::CommandResult;
use crate::reminders::SchedulerHandle;

/// Frontend-supplied payload for creating a local calendar.
#[derive(Debug, Deserialize)]
pub struct CreateCalendarRequest {
    pub name: String,
    pub color_hex: Option<String>,
}

#[tauri::command]
pub async fn list_calendars(adapter: State<'_, LocalAdapter>) -> CommandResult<Vec<Calendar>> {
    Ok(adapter.list_calendars().await?)
}

#[tauri::command]
pub async fn create_calendar(
    adapter: State<'_, LocalAdapter>,
    request: CreateCalendarRequest,
) -> CommandResult<Calendar> {
    let color = request
        .color_hex
        .map(|hex| ContainerColor::custom(hex.trim().to_string()));
    Ok(adapter.create_calendar(&request.name, color, None)?)
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
    request: EventRangeRequest,
) -> CommandResult<Vec<Event>> {
    let range = DateRange::new(request.start, request.end);
    Ok(adapter.get_events(&request.calendar_id, range).await?)
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
    scheduler: State<'_, SchedulerHandle>,
    request: CreateEventRequest,
) -> CommandResult<Event> {
    let event = adapter
        .create_event(&request.calendar_id, request.event)
        .await?;
    scheduler.invalidate();
    Ok(event)
}

#[tauri::command]
pub async fn update_event(
    adapter: State<'_, LocalAdapter>,
    scheduler: State<'_, SchedulerHandle>,
    event: Event,
) -> CommandResult<Event> {
    let event = adapter.update_event(event).await?;
    scheduler.invalidate();
    Ok(event)
}

#[tauri::command]
pub async fn delete_event(
    adapter: State<'_, LocalAdapter>,
    scheduler: State<'_, SchedulerHandle>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_event(&id).await?;
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
    scheduler: State<'_, SchedulerHandle>,
    id: String,
    occurrence: DateTime<Utc>,
) -> CommandResult<()> {
    adapter.add_event_exdate(&id, occurrence)?;
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
