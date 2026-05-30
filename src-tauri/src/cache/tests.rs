//! Unit tests for the external-adapter snapshot cache (CACHE-0).

use super::{CacheStore, Delta, RefreshCoordinator, SyncScope, SyncState};
use crate::db::DbHandle;
use cal_core::{
    Calendar, Contact, ContactList, DateRange, Event, Task, TaskList, TaskPriority, TaskStatus,
};
use chrono::{TimeZone, Utc};
use rusqlite::params;

const ACC: &str = "acc-1";
const CAL: &str = "cal-1";
const LIST: &str = "list-1";

fn setup() -> CacheStore {
    let db = DbHandle::open_in_memory().unwrap();
    let store = CacheStore::new(db);
    // Cache rows FK onto an account, so seed an external one.
    store
        .db
        .with_conn(|c| {
            c.execute(
                "INSERT INTO accounts (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES (?1, 'caldav', 'Work', '{}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                params![ACC],
            )
        })
        .unwrap();
    store
}

fn range(start_h: u32, end_h: u32) -> DateRange {
    DateRange::new(
        Utc.with_ymd_and_hms(2026, 5, 30, start_h, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 30, end_h, 0, 0).unwrap(),
    )
}

/// Whole-day range (midnight to next midnight) for "read everything" assertions.
fn wide() -> DateRange {
    DateRange::new(
        Utc.with_ymd_and_hms(2026, 5, 30, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap(),
    )
}

fn event(id: &str, start_h: u32, end_h: u32) -> Event {
    Event {
        id: id.into(),
        calendar_id: CAL.into(),
        title: format!("Event {id}"),
        description: None,
        location: None,
        start: Utc.with_ymd_and_hms(2026, 5, 30, start_h, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 5, 30, end_h, 0, 0).unwrap(),
        all_day: false,
        recurrence: None,
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        attendees: Vec::new(),
        created_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        etag: Some(format!("etag-{id}")),
    }
}

fn task(id: &str) -> Task {
    Task {
        id: id.into(),
        list_id: LIST.into(),
        title: format!("Task {id}"),
        description: None,
        status: TaskStatus::Open,
        priority: TaskPriority::Medium,
        scheduled_date: None,
        scheduled_time: None,
        deadline_date: None,
        deadline_time: None,
        recurrence: None,
        parent_id: None,
        section_id: None,
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        created_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        completed_at: None,
        etag: None,
    }
}

fn contact(id: &str) -> Contact {
    Contact {
        id: id.into(),
        list_id: LIST.into(),
        display_name: format!("Person {id}"),
        given_name: None,
        family_name: None,
        organization: None,
        emails: Vec::new(),
        phone_numbers: Vec::new(),
        birthday: None,
        notes: None,
        members: None,
        has_photo: false,
        addresses: Vec::new(),
        created_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        etag: None,
    }
}

fn calendar(id: &str) -> Calendar {
    Calendar {
        id: id.into(),
        name: format!("Cal {id}"),
        color: None,
        read_only: false,
        default_sound: None,
    }
}

fn task_list(id: &str) -> TaskList {
    TaskList {
        id: id.into(),
        name: format!("List {id}"),
        color: None,
        default_sound: None,
        embedded_in_calendar: None,
        parent_id: None,
        read_only: false,
    }
}

fn contact_list(id: &str) -> ContactList {
    ContactList {
        id: id.into(),
        name: format!("Book {id}"),
        color: None,
        read_only: false,
    }
}

#[test]
fn events_roundtrip_and_window_coverage() {
    let store = setup();
    store
        .replace_calendar_events(
            ACC,
            CAL,
            range(8, 18),
            &[event("e1", 8, 9), event("e2", 12, 13)],
        )
        .unwrap();

    // Whole-window read returns both, ordered by start.
    let all = store.read_events(ACC, CAL, range(8, 18)).unwrap();
    assert_eq!(
        all.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["e1", "e2"]
    );
    // Payload round-trips fully (etag preserved).
    assert_eq!(all[0].etag.as_deref(), Some("etag-e1"));

    // Narrow half-open overlap [8,10): e1 (8-9) in, e2 (12-13) out.
    let narrow = store.read_events(ACC, CAL, range(8, 10)).unwrap();
    assert_eq!(
        narrow.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["e1"]
    );

    // The covered window is recorded so callers can tell
    // "cached, empty" from "not cached".
    assert_eq!(
        store.event_window(ACC, CAL).unwrap(),
        Some((range(8, 18).start, range(8, 18).end))
    );
}

#[test]
fn events_delta_upserts_changes_and_removes_deletions() {
    let store = setup();
    store
        .replace_calendar_events(
            ACC,
            CAL,
            range(8, 18),
            &[event("e1", 8, 9), event("e2", 12, 13)],
        )
        .unwrap();

    store
        .apply_events_delta(
            ACC,
            CAL,
            &Delta {
                // e2 moves to the afternoon; e3 is new.
                changes: vec![event("e2", 14, 15), event("e3", 16, 17)],
                deletions: vec!["e1".into()],
                new_token: Some("tok-2".into()),
            },
        )
        .unwrap();

    let all = store.read_events(ACC, CAL, wide()).unwrap();
    assert_eq!(
        all.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["e2", "e3"]
    );
    // e2 was updated in place (moved to 14:00), not duplicated.
    assert_eq!(
        all[0].start,
        Utc.with_ymd_and_hms(2026, 5, 30, 14, 0, 0).unwrap()
    );

    let state = store
        .get_sync_state(ACC, SyncScope::Events, CAL)
        .unwrap()
        .unwrap();
    assert_eq!(state.sync_token.as_deref(), Some("tok-2"));
    // Delta keeps the window the full refresh established.
    assert_eq!(state.window_start, Some(range(8, 18).start));
}

#[test]
fn prune_events_outside_drops_only_out_of_window_rows() {
    let store = setup();
    store
        .replace_calendar_events(
            ACC,
            CAL,
            range(8, 18),
            &[event("e1", 8, 9), event("e2", 12, 13)],
        )
        .unwrap();
    // Keep only the afternoon: e1 (8-9) ends before 11 → dropped.
    store.prune_events_outside(ACC, CAL, range(11, 18)).unwrap();
    let all = store.read_events(ACC, CAL, wide()).unwrap();
    assert_eq!(
        all.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["e2"]
    );
}

#[test]
fn tasks_roundtrip_and_delta() {
    let store = setup();
    store
        .replace_list_tasks(ACC, LIST, &[task("t1"), task("t2")])
        .unwrap();
    assert_eq!(store.read_tasks(ACC, LIST).unwrap().len(), 2);

    store
        .apply_tasks_delta(
            ACC,
            LIST,
            &Delta {
                changes: vec![task("t3")],
                deletions: vec!["t1".into()],
                new_token: Some("tt".into()),
            },
        )
        .unwrap();
    let ids: Vec<String> = store
        .read_tasks(ACC, LIST)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, ["t2", "t3"]);
    assert_eq!(
        store
            .get_sync_state(ACC, SyncScope::Tasks, LIST)
            .unwrap()
            .unwrap()
            .sync_token
            .as_deref(),
        Some("tt")
    );
}

#[test]
fn contacts_roundtrip_and_delta() {
    let store = setup();
    store
        .replace_list_contacts(ACC, LIST, &[contact("c1"), contact("c2")])
        .unwrap();
    assert_eq!(store.read_contacts(ACC, LIST).unwrap().len(), 2);

    store
        .apply_contacts_delta(
            ACC,
            LIST,
            &Delta {
                changes: vec![contact("c3")],
                deletions: vec!["c2".into()],
                new_token: None,
            },
        )
        .unwrap();
    let ids: Vec<String> = store
        .read_contacts(ACC, LIST)
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids, ["c1", "c3"]);
}

#[test]
fn listings_replace_is_not_append() {
    let store = setup();
    store
        .replace_calendars(ACC, &[calendar("a"), calendar("b")])
        .unwrap();
    store.replace_task_lists(ACC, &[task_list("x")]).unwrap();
    store
        .replace_contact_lists(ACC, &[contact_list("z")])
        .unwrap();

    assert_eq!(store.read_calendars(ACC).unwrap().len(), 2);
    assert_eq!(store.read_task_lists(ACC).unwrap()[0].id, "x");
    assert_eq!(store.read_contact_lists(ACC).unwrap()[0].id, "z");

    // A second replace mirrors the new set, not the union.
    store.replace_calendars(ACC, &[calendar("only")]).unwrap();
    let cals = store.read_calendars(ACC).unwrap();
    assert_eq!(
        cals.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["only"]
    );

    // Listing scope freshness was stamped.
    assert!(store
        .get_sync_state(ACC, SyncScope::Calendars, "")
        .unwrap()
        .unwrap()
        .last_refreshed_at
        .is_some());
}

#[test]
fn sync_state_set_get_and_mark_error() {
    let store = setup();
    let state = SyncState {
        sync_token: Some("abc".into()),
        ctag: Some("ctag-1".into()),
        window_start: Some(range(8, 18).start),
        window_end: Some(range(8, 18).end),
        last_refreshed_at: Some(Utc.with_ymd_and_hms(2026, 5, 30, 9, 30, 0).unwrap()),
        last_error: None,
    };
    store
        .set_sync_state(ACC, SyncScope::Events, CAL, &state)
        .unwrap();
    assert_eq!(
        store.get_sync_state(ACC, SyncScope::Events, CAL).unwrap(),
        Some(state.clone())
    );

    // mark_error stamps the error without clobbering token/window.
    store
        .mark_error(ACC, SyncScope::Events, CAL, "timeout")
        .unwrap();
    let after = store
        .get_sync_state(ACC, SyncScope::Events, CAL)
        .unwrap()
        .unwrap();
    assert_eq!(after.last_error.as_deref(), Some("timeout"));
    assert_eq!(after.sync_token.as_deref(), Some("abc"));
    assert_eq!(after.window_end, state.window_end);
}

#[test]
fn prune_account_wipes_every_scope() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, range(8, 18), &[event("e1", 8, 9)])
        .unwrap();
    store.replace_list_tasks(ACC, LIST, &[task("t1")]).unwrap();
    store
        .replace_list_contacts(ACC, LIST, &[contact("c1")])
        .unwrap();
    store.replace_calendars(ACC, &[calendar("a")]).unwrap();

    store.prune_account(ACC).unwrap();

    assert!(store.read_events(ACC, CAL, wide()).unwrap().is_empty());
    assert!(store.read_tasks(ACC, LIST).unwrap().is_empty());
    assert!(store.read_contacts(ACC, LIST).unwrap().is_empty());
    assert!(store.read_calendars(ACC).unwrap().is_empty());
    assert!(store
        .get_sync_state(ACC, SyncScope::Events, CAL)
        .unwrap()
        .is_none());
}

#[test]
fn deleting_account_cascades_cache_rows() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, range(8, 18), &[event("e1", 8, 9)])
        .unwrap();
    // Removing the owning account must take its cache with it (FK
    // ON DELETE CASCADE) — no orphaned snapshot rows.
    store
        .db
        .with_conn(|c| c.execute("DELETE FROM accounts WHERE id = ?1", params![ACC]))
        .unwrap();
    assert!(store.read_events(ACC, CAL, wide()).unwrap().is_empty());
}

#[test]
fn write_through_upsert_and_remove_single_rows() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, range(8, 18), &[event("e1", 8, 9)])
        .unwrap();

    // Upsert a freshly created row WITHOUT touching the window/freshness.
    store.upsert_event(ACC, CAL, &event("e2", 12, 13)).unwrap();
    let ids: Vec<String> = store
        .read_events(ACC, CAL, wide())
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, ["e1", "e2"]);
    // Window from the full refresh is untouched by write-through.
    assert_eq!(
        store.event_window(ACC, CAL).unwrap(),
        Some((range(8, 18).start, range(8, 18).end))
    );

    store.remove_event(ACC, CAL, "e1").unwrap();
    let ids: Vec<String> = store
        .read_events(ACC, CAL, wide())
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, ["e2"]);

    // Tasks + task-list write-through.
    store.replace_list_tasks(ACC, LIST, &[task("t1")]).unwrap();
    store.upsert_task(ACC, LIST, &task("t2")).unwrap();
    assert_eq!(store.read_tasks(ACC, LIST).unwrap().len(), 2);
    store.remove_task(ACC, LIST, "t1").unwrap();
    assert_eq!(store.read_tasks(ACC, LIST).unwrap()[0].id, "t2");

    store.replace_task_lists(ACC, &[task_list("p1")]).unwrap();
    store.upsert_task_list(ACC, &task_list("p2")).unwrap();
    assert_eq!(store.read_task_lists(ACC).unwrap().len(), 2);
    store.remove_task_list(ACC, "p1").unwrap();
    assert_eq!(store.read_task_lists(ACC).unwrap()[0].id, "p2");
}

#[test]
fn invalidate_forces_next_read_cold() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, range(8, 18), &[event("e1", 8, 9)])
        .unwrap();
    // Covered + fresh before invalidation.
    assert!(store.event_window(ACC, CAL).unwrap().is_some());

    store.invalidate(ACC, SyncScope::Events, CAL).unwrap();
    // Window cleared → the command's coverage check fails → cold fetch.
    assert!(store.event_window(ACC, CAL).unwrap().is_none());
    // The cached rows survive as an offline fallback.
    assert_eq!(store.read_events(ACC, CAL, wide()).unwrap().len(), 1);

    // For a listing scope, invalidation drops the freshness stamp.
    store.replace_task_lists(ACC, &[task_list("p1")]).unwrap();
    store.invalidate(ACC, SyncScope::TaskLists, "").unwrap();
    assert!(store
        .get_sync_state(ACC, SyncScope::TaskLists, "")
        .unwrap()
        .unwrap()
        .last_refreshed_at
        .is_none());
}

#[test]
fn refresh_coordinator_dedups_until_released() {
    let coord = RefreshCoordinator::new();
    let key = "events:acc-1:cal-1";
    // First claim wins; a second concurrent claim of the same key is
    // rejected so we never stack redundant background refreshes.
    assert!(coord.try_claim(key));
    assert!(!coord.try_claim(key));
    // A different container is independent.
    assert!(coord.try_claim("events:acc-1:cal-2"));
    // Once released, the key can be claimed again (next refresh cycle).
    coord.release(key);
    assert!(coord.try_claim(key));
}
