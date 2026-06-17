//! Unit tests for the external-adapter snapshot cache (CACHE-0).

use super::{CacheStore, Delta, RefreshCoordinator, SyncScope, SyncState};
use crate::db::DbHandle;
use cal_core::{
    Calendar, Contact, ContactList, DateRange, Event, EventRecurrence, Task, TaskList,
    TaskPriority, TaskStatus,
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
        color_hex: None,
        reminders: Vec::new(),
        sound: None,
        attendees: Vec::new(),
        send_invitations: false,
        created_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        etag: Some(format!("etag-{id}")),
        organizer: None,
        attendee_responses: Vec::new(),
    }
}

fn task(id: &str) -> Task {
    Task {
        assignees: Vec::new(),
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
        resurface_date: None,
        series_id: None,
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
        color_label: None,
        supports_scheduling: false,
        supports_event_color: false,
        id: id.into(),
        name: format!("Cal {id}"),
        color: None,
        read_only: false,
        default_sound: None,
    }
}

fn task_list(id: &str) -> TaskList {
    TaskList {
        color_label: None,
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
        color_label: None,
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

// ── Recurring masters with a past start (iCloud/CalDAV) ──────────────
//
// A weekly meeting that began long ago has its `start`/`end` in the past,
// but recurs into the current view. The cache must still return the
// master for a future range so the frontend can expand its occurrences —
// the row's `end_utc` column reflects the recurrence reach, not the first
// occurrence's end.

/// Build a recurring master whose first occurrence is `start_h..end_h` on
/// 2025-01-06 (well before the 2026 read ranges below).
fn recurring_master(id: &str, rrule: &str) -> Event {
    let mut ev = event(id, 0, 0);
    ev.start = Utc.with_ymd_and_hms(2025, 1, 6, 10, 0, 0).unwrap();
    ev.end = Utc.with_ymd_and_hms(2025, 1, 6, 11, 0, 0).unwrap();
    ev.recurrence = Some(EventRecurrence {
        rrule: rrule.into(),
        exceptions: Vec::new(),
    });
    ev
}

/// June 2026 — a month a year and a half after the master's first run.
fn june_2026() -> DateRange {
    DateRange::new(
        Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
    )
}

#[test]
fn open_ended_recurring_master_survives_a_future_range_read() {
    let store = setup();
    store
        .replace_calendar_events(
            ACC,
            CAL,
            june_2026(),
            &[recurring_master("weekly", "FREQ=WEEKLY")],
        )
        .unwrap();
    // The regression: an interval-overlap query on the first occurrence
    // (Jan 2025) would drop this master; the recurrence-aware end_utc
    // keeps it so the frontend can expand June's occurrences.
    let rows = store.read_events(ACC, CAL, june_2026()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "weekly");
    // The payload round-trips the TRUE (past) start — only row selection
    // changed, not the event the frontend expands.
    assert_eq!(
        rows[0].start,
        Utc.with_ymd_and_hms(2025, 1, 6, 10, 0, 0).unwrap()
    );
}

#[test]
fn recurring_master_with_until_in_the_past_is_excluded() {
    let store = setup();
    store
        .replace_calendar_events(
            ACC,
            CAL,
            june_2026(),
            &[recurring_master(
                "expired",
                "FREQ=WEEKLY;UNTIL=20250201T100000Z",
            )],
        )
        .unwrap();
    // A series that ended Feb 2025 has no occurrences in June 2026.
    assert!(store.read_events(ACC, CAL, june_2026()).unwrap().is_empty());
}

#[test]
fn non_recurring_past_event_is_not_widened() {
    let store = setup();
    let mut once = event("once", 0, 0);
    once.start = Utc.with_ymd_and_hms(2025, 1, 6, 10, 0, 0).unwrap();
    once.end = Utc.with_ymd_and_hms(2025, 1, 6, 11, 0, 0).unwrap();
    store
        .replace_calendar_events(ACC, CAL, june_2026(), &[once])
        .unwrap();
    // The fix must not leak one-off events into unrelated ranges.
    assert!(store.read_events(ACC, CAL, june_2026()).unwrap().is_empty());
}

#[test]
fn recurrence_until_parses_ical_forms() {
    assert_eq!(
        super::recurrence_until("FREQ=WEEKLY;UNTIL=20270101T100000Z"),
        Some(Utc.with_ymd_and_hms(2027, 1, 1, 10, 0, 0).unwrap()),
    );
    // Date-only UNTIL covers the whole day.
    assert_eq!(
        super::recurrence_until("FREQ=DAILY;UNTIL=20270101"),
        Some(Utc.with_ymd_and_hms(2027, 1, 1, 23, 59, 59).unwrap()),
    );
    // COUNT-based / open-ended → no UNTIL.
    assert!(super::recurrence_until("FREQ=WEEKLY;COUNT=10").is_none());
    assert!(super::recurrence_until("FREQ=WEEKLY").is_none());
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
fn delta_deletes_list_task_by_full_composite_id() {
    let store = setup();
    // Graph To Do-style ids: `{list}|{task}`. `native_id` derives to the
    // list (everything before `|`), which is shared across the list's
    // tasks — so a per-resource deletion can't match by native id and
    // must carry the full composite id instead.
    store
        .replace_list_tasks(ACC, LIST, &[task("list-1|t1"), task("list-1|t2")])
        .unwrap();
    assert_eq!(store.read_tasks(ACC, LIST).unwrap().len(), 2);

    store
        .apply_tasks_delta(
            ACC,
            LIST,
            &Delta {
                changes: Vec::new(),
                deletions: vec!["list-1|t1".into()],
                new_token: Some("d2".into()),
            },
        )
        .unwrap();

    // Only t1 is gone — the shared native_id ("list-1") must NOT have
    // taken t2 down with it.
    let rows = store.read_tasks(ACC, LIST).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "list-1|t2");
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
fn native_id_strips_kind_prefix_and_change_key() {
    use super::native_id;
    // EWS: `{kind}:{item_id}|{change_key}` → item_id.
    assert_eq!(native_id("S:AAA|CK"), "AAA");
    assert_eq!(native_id("M:item-1|ck-v2"), "item-1");
    assert_eq!(native_id("S:noCK"), "noCK");
    // CalDAV: `{href}|{uid}` → href (no `X:` prefix to strip).
    assert_eq!(native_id("https://h/p/e.ics|uid@h"), "https://h/p/e.ics");
    // Already-native (Google / Graph / Vikunja / local) → unchanged.
    assert_eq!(native_id("plain-id-123"), "plain-id-123");
}

#[test]
fn delta_deletes_by_native_id() {
    let store = setup();
    // An EWS-style event: composite cal-core id, native id "item-1".
    let ev = event("S:item-1|ck-v1", 8, 9);
    store
        .replace_calendar_events(ACC, CAL, range(8, 18), &[ev])
        .unwrap();
    assert_eq!(store.read_events(ACC, CAL, wide()).unwrap().len(), 1);

    // The delta deletion carries ONLY the native id (no change key).
    store
        .apply_events_delta(
            ACC,
            CAL,
            &Delta {
                changes: Vec::new(),
                deletions: vec!["item-1".into()],
                new_token: Some("c2".into()),
            },
        )
        .unwrap();
    assert!(store.read_events(ACC, CAL, wide()).unwrap().is_empty());
}

#[test]
fn delta_update_replaces_stale_change_key_row() {
    let store = setup();
    // An EWS-style single: native id "item-1", composite id carries ck-v1.
    store
        .replace_calendar_events(ACC, CAL, wide(), &[event("S:item-1|ck-v1", 8, 9)])
        .unwrap();
    assert_eq!(store.read_events(ACC, CAL, wide()).unwrap().len(), 1);

    // The item is edited: same native ItemId, rotated ChangeKey ⇒ a NEW
    // composite cal-core id arrives as a delta change (with no matching
    // deletion, the way SyncFolderItems reports an Update).
    store
        .apply_events_delta(
            ACC,
            CAL,
            &Delta {
                changes: vec![event("S:item-1|ck-v2", 8, 10)],
                deletions: Vec::new(),
                new_token: Some("c2".into()),
            },
        )
        .unwrap();

    // Exactly one row survives — the pre-update ck-v1 copy was purged via
    // its native id, not left behind as a duplicate.
    let rows = store.read_events(ACC, CAL, wide()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "S:item-1|ck-v2");
    assert_eq!(
        rows[0].end,
        Utc.with_ymd_and_hms(2026, 5, 30, 10, 0, 0).unwrap()
    );
}

#[test]
fn delta_update_replaces_master_and_its_overrides() {
    let store = setup();
    // A recurring master plus a synthetic occurrence override. Both share
    // native id "master-1" — the override's cal-core id is derived from
    // the master's ItemId, so it strips down to the same native id.
    store
        .replace_calendar_events(
            ACC,
            CAL,
            wide(),
            &[
                event("M:master-1|ck-v1", 8, 9),
                event(
                    "M:master-1|ck-v1#override:2026-05-30T08:00:00+00:00",
                    10,
                    11,
                ),
            ],
        )
        .unwrap();
    assert_eq!(store.read_events(ACC, CAL, wide()).unwrap().len(), 2);

    // The master is edited: ChangeKey rotates and its single override moves
    // to a new slot. The delta carries the fresh master + its one current
    // override (both ck-v2). The whole native group must be replaced — no
    // ck-v1 leftovers from either row.
    store
        .apply_events_delta(
            ACC,
            CAL,
            &Delta {
                changes: vec![
                    event("M:master-1|ck-v2", 8, 9),
                    event(
                        "M:master-1|ck-v2#override:2026-05-30T08:00:00+00:00",
                        12,
                        13,
                    ),
                ],
                deletions: Vec::new(),
                new_token: Some("c2".into()),
            },
        )
        .unwrap();

    let rows = store.read_events(ACC, CAL, wide()).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|e| e.id.contains("ck-v2")));
}

#[test]
fn change_set_wire_defaults_and_roundtrip() {
    use cal_core::ChangeSet;

    // Minimal payload: only `changes`; the rest default (deletions
    // empty, no token, not a full resync).
    let cs: ChangeSet<String> = serde_json::from_str(r#"{"changes":["a","b"]}"#).unwrap();
    assert_eq!(cs.changes, ["a", "b"]);
    assert!(cs.deletions.is_empty());
    assert!(cs.new_token.is_none());
    assert!(!cs.full_resync);

    let full = ChangeSet {
        changes: vec![1, 2, 3],
        deletions: vec!["x".into()],
        new_token: Some("tok".into()),
        full_resync: true,
        // Non-default so the round-trip also proves `complete` survives
        // the serde boundary the FFI shim ships ChangeSets across.
        complete: true,
    };
    let encoded = serde_json::to_string(&full).unwrap();
    let back: ChangeSet<i32> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(back, full);
}

#[test]
fn folder_complete_window_round_trips_and_covers_any_range() {
    // Regression: the folder-complete "unbounded" snapshot window must
    // survive the cache's text-timestamp round-trip. `DateTime::MIN_UTC`/
    // `MAX_UTC` format to non-4-digit years (`-262143-…` / `+262142-…`) that
    // `parse_from_rfc3339` rejects, so they'd be dropped on read —
    // `event_window` would return None, `covers` would never hold, and an
    // external read would spin forever in the cold path (refresh →
    // cache-updated → re-read → refresh). Representable sentinels (year
    // 1 … 9999) round-trip and cover every realistic range.
    let store = setup();
    let window = super::unbounded_window();
    store
        .replace_calendar_events(ACC, CAL, window, &[])
        .unwrap();

    let (ws, we) = store
        .event_window(ACC, CAL)
        .unwrap()
        .expect("unbounded window must read back");
    // Covers a present-day view range …
    let view = range(8, 18);
    assert!(ws <= view.start && we >= view.end);
    // … and a far-future one, proving it is effectively unbounded.
    let far = DateRange::new(
        Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2099, 12, 31, 0, 0, 0).unwrap(),
    );
    assert!(ws <= far.start && we >= far.end);
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

// ── External-cache full-text search (migration 0027 + cache/search.rs) ──

use cal_adapter_local::SearchFilters;

#[test]
fn search_finds_cached_external_event_by_prefix() {
    // The reported bug: an Exchange event "WG: Diversity Audit" was
    // unfindable via "diver" because only LOCAL tables were indexed.
    let store = setup();
    let mut ev = event("e1", 9, 10);
    ev.title = "WG: Diversity Audit".into();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[ev])
        .unwrap();

    let hits = store
        .search_events_fts("diver*", &SearchFilters::default())
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "WG: Diversity Audit");

    // Unrelated terms stay quiet.
    assert!(store
        .search_events_fts("zzznope*", &SearchFilters::default())
        .unwrap()
        .is_empty());
}

#[test]
fn search_index_follows_upserts_and_deletes() {
    let store = setup();
    let mut ev = event("e1", 9, 10);
    ev.title = "Quarterly Review".into();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[ev.clone()])
        .unwrap();

    // Upsert with a new title: the old term must stop matching, the new
    // one must match exactly once (no duplicate index rows).
    ev.title = "Team Offsite".into();
    store.upsert_event(ACC, CAL, &ev).unwrap();
    assert!(store
        .search_events_fts("quarter*", &SearchFilters::default())
        .unwrap()
        .is_empty());
    let hits = store
        .search_events_fts("offsite*", &SearchFilters::default())
        .unwrap();
    assert_eq!(hits.len(), 1);

    // Removal clears the index row.
    store.remove_event(ACC, CAL, "e1").unwrap();
    assert!(store
        .search_events_fts("offsite*", &SearchFilters::default())
        .unwrap()
        .is_empty());
}

#[test]
fn search_event_calendar_filter_restricts() {
    let store = setup();
    let mut ev = event("e1", 9, 10);
    ev.title = "Diversity Audit".into();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[ev])
        .unwrap();

    let other_cal = SearchFilters {
        calendar_ids: vec!["some-other-cal".into()],
        ..Default::default()
    };
    assert!(store
        .search_events_fts("diver*", &other_cal)
        .unwrap()
        .is_empty());

    let this_cal = SearchFilters {
        calendar_ids: vec![CAL.into()],
        ..Default::default()
    };
    assert_eq!(
        store.search_events_fts("diver*", &this_cal).unwrap().len(),
        1
    );
}

#[test]
fn search_finds_cached_external_task_with_status_filter() {
    let store = setup();
    let mut t1 = task("t1");
    t1.title = "Diversity training prep".into();
    store.replace_list_tasks(ACC, LIST, &[t1]).unwrap();

    let hits = store
        .search_tasks_fts("diver*", &SearchFilters::default())
        .unwrap();
    assert_eq!(hits.len(), 1);

    // Status whitelist that excludes 'open' hides the task.
    let filters = SearchFilters {
        task_statuses: vec!["completed".into()],
        ..Default::default()
    };
    assert!(store
        .search_tasks_fts("diver*", &filters)
        .unwrap()
        .is_empty());
}
