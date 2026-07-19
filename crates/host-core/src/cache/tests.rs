//! Unit tests for the external-adapter snapshot cache (CACHE-0).

use super::{CacheStore, Delta, RefreshCoordinator, SyncScope, SyncState};
use crate::db::DbHandle;
use cal_core::{
    Calendar, Contact, ContactList, DateRange, Event, EventRecurrence, Section, Task, TaskList,
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
        truncate_tail_overrides: false,
        created_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        etag: Some(format!("etag-{id}")),
        organizer: None,
        attendee_responses: Vec::new(),
        cancelled: false,
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
        effort: cal_core::TaskEffort::Medium,
        deadline_reminder_days: None,
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

fn section(id: &str, order: u32) -> Section {
    Section {
        id: id.into(),
        list_id: LIST.into(),
        name: format!("Section {id}"),
        color_label: None,
        order,
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
        tzid: None,
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
fn write_through_task_updates_retained_rows_and_marks_stale() {
    let store = setup();
    store
        .replace_list_tasks(ACC, LIST, &[task("t1"), task("t2")])
        .unwrap();

    // A check-off write-through: the returned row lands in the snapshot
    // immediately, so the SWR retained fallback serves the truth instead of
    // the pre-write state — and the list is marked stale for reconciling.
    let mut done = task("t1");
    done.status = TaskStatus::Completed;
    store.write_through_task(ACC, LIST, &done).unwrap();
    let rows = store.read_tasks(ACC, LIST).unwrap();
    let t1 = rows.iter().find(|t| t.id == "t1").unwrap();
    assert_eq!(t1.status, TaskStatus::Completed);
    let state = store
        .get_sync_state(ACC, SyncScope::Tasks, LIST)
        .unwrap()
        .unwrap();
    assert!(state.last_refreshed_at.is_none(), "must be marked stale");

    // Removal write-through drops the row so it can't resurrect.
    store.write_through_task_removal(ACC, LIST, "t2").unwrap();
    let ids: Vec<String> = store
        .read_tasks(ACC, LIST)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(!ids.contains(&"t2".to_string()));
}

#[test]
fn write_through_task_dedups_a_rotated_composite_id() {
    // EWS rotates the ChangeKey suffix of a task's id on every edit, so an
    // update returns `item|ckB` for the row the snapshot holds as `item|ckA`.
    // A plain upsert (keyed on the full id) would leave BOTH — the task would
    // show twice, one copy still open. The write-through purges the native
    // group (everything before `|`) first, so the stale row is replaced, not
    // duplicated.
    let store = setup();
    let mut before = task("item|ckA");
    before.status = TaskStatus::Open;
    store.replace_list_tasks(ACC, LIST, &[before]).unwrap();

    let mut after = task("item|ckB");
    after.status = TaskStatus::Completed;
    store.write_through_task(ACC, LIST, &after).unwrap();

    let rows = store.read_tasks(ACC, LIST).unwrap();
    assert_eq!(rows.len(), 1, "the rotated id must not duplicate the row");
    assert_eq!(rows[0].id, "item|ckB");
    assert_eq!(rows[0].status, TaskStatus::Completed);
}

#[test]
fn write_through_task_skips_upsert_on_a_never_warmed_list() {
    // No snapshot, no rows: the cold fallback live-reads, and planting a
    // lone row here would masquerade as the whole list. The write-through
    // must leave the cache row-less.
    let store = setup();
    store.write_through_task(ACC, LIST, &task("t1")).unwrap();
    assert!(store.read_tasks(ACC, LIST).unwrap().is_empty());
}

#[test]
fn sections_roundtrip_and_replace_stamps_freshness() {
    let store = setup();
    // Cold: no snapshot yet.
    assert!(store
        .get_sync_state(ACC, SyncScope::Sections, LIST)
        .unwrap()
        .is_none());
    assert!(store.read_sections(ACC, LIST).unwrap().is_empty());

    // A full replace mirrors the provider set + stamps freshness.
    store
        .replace_sections(ACC, LIST, &[section("s1", 0), section("s2", 1)])
        .unwrap();
    let ids: Vec<String> = store
        .read_sections(ACC, LIST)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, ["s1", "s2"]);
    assert!(super::has_snapshot(
        &store
            .get_sync_state(ACC, SyncScope::Sections, LIST)
            .unwrap()
    ));

    // A second replace mirrors the new set, not the union (replace, not append).
    store
        .replace_sections(ACC, LIST, &[section("only", 0)])
        .unwrap();
    let after: Vec<String> = store
        .read_sections(ACC, LIST)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(after, ["only"]);

    // Sections are scoped per (account, list) — a different list is unaffected.
    assert!(store.read_sections(ACC, "other-list").unwrap().is_empty());
}

#[test]
fn prune_account_wipes_cached_sections() {
    let store = setup();
    store
        .replace_sections(ACC, LIST, &[section("s1", 0)])
        .unwrap();
    assert_eq!(store.read_sections(ACC, LIST).unwrap().len(), 1);
    store.prune_account(ACC).unwrap();
    assert!(store.read_sections(ACC, LIST).unwrap().is_empty());
    assert!(store
        .get_sync_state(ACC, SyncScope::Sections, LIST)
        .unwrap()
        .is_none());
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

use std::sync::Arc;

use async_trait::async_trait;
use cal_core::{Adapter, AuthToken, Capability, Credentials, NewTask, TasksFeature};

/// A fake tasks adapter whose `get_tasks` invalidates the list MID-FETCH —
/// simulating a local mutation (e.g. completing a task in the day-start review)
/// landing while a slow warm refresh is already in flight. Proves the generation
/// guard in `refresh_tasks` drops the now-stale write.
struct MidFetchInvalidator {
    cache: Arc<CacheStore>,
    tasks: Vec<Task>,
}

#[async_trait]
impl Adapter for MidFetchInvalidator {
    async fn authenticate(&self, _credentials: Credentials) -> cal_core::Result<AuthToken> {
        Err(cal_core::Error::Unsupported("test fake".into()))
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }
}

#[async_trait]
impl TasksFeature for MidFetchInvalidator {
    async fn list_task_lists(&self) -> cal_core::Result<Vec<TaskList>> {
        Ok(vec![])
    }
    async fn get_tasks(&self, _list_id: &str) -> cal_core::Result<Vec<Task>> {
        // The mutation lands DURING the fetch (before this returns its snapshot).
        self.cache.invalidate(ACC, SyncScope::Tasks, LIST).unwrap();
        Ok(self.tasks.clone())
    }
    async fn create_task(&self, _list_id: &str, _task: NewTask) -> cal_core::Result<Task> {
        unreachable!("not exercised by the refresh path")
    }
    async fn update_task(&self, _task: Task) -> cal_core::Result<Task> {
        unreachable!("not exercised by the refresh path")
    }
    async fn delete_task(&self, _task_id: &str) -> cal_core::Result<()> {
        unreachable!("not exercised by the refresh path")
    }
}

#[test]
fn invalidate_bumps_the_refresh_generation_per_container() {
    let store = setup();
    assert_eq!(store.refresh_generation(ACC, SyncScope::Tasks, LIST), 0);
    store.invalidate(ACC, SyncScope::Tasks, LIST).unwrap();
    store.invalidate(ACC, SyncScope::Tasks, LIST).unwrap();
    assert_eq!(store.refresh_generation(ACC, SyncScope::Tasks, LIST), 2);
    // Independent per container + per scope.
    assert_eq!(
        store.refresh_generation(ACC, SyncScope::Tasks, "other-list"),
        0
    );
    assert_eq!(store.refresh_generation(ACC, SyncScope::Events, LIST), 0);
}

#[tokio::test]
async fn refresh_drops_a_write_whose_fetch_predates_an_invalidate() {
    let store = Arc::new(setup());
    // Warm the list with task t1.
    store.replace_list_tasks(ACC, LIST, &[task("t1")]).unwrap();

    // The fake invalidates the list mid-fetch (a local mutation), then returns a
    // DIFFERENT snapshot (t2). The generation guard must drop that stale write,
    // leaving the warmed t1 in place (the invalidation forces a cold re-read).
    let adapter = MidFetchInvalidator {
        cache: store.clone(),
        tasks: vec![task("t2")],
    };
    super::refresh_tasks(&store, &adapter, ACC, LIST)
        .await
        .unwrap();

    let cached = store.read_tasks(ACC, LIST).unwrap();
    let ids: Vec<&str> = cached.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        ids,
        ["t1"],
        "a refresh whose fetch predates an invalidate must not overwrite the cache",
    );
}

/// A fake tasks adapter that returns a fixed SECTION set from `list_sections`
/// and counts the calls — so a warm read (served from the cache) can be proven
/// NOT to hit the adapter, and a cold one to hit it exactly once and warm.
struct SectionAdapter {
    sections: Vec<Section>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl Adapter for SectionAdapter {
    async fn authenticate(&self, _credentials: Credentials) -> cal_core::Result<AuthToken> {
        Err(cal_core::Error::Unsupported("test fake".into()))
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }
}

#[async_trait]
impl TasksFeature for SectionAdapter {
    async fn list_task_lists(&self) -> cal_core::Result<Vec<TaskList>> {
        Ok(vec![])
    }
    async fn get_tasks(&self, _list_id: &str) -> cal_core::Result<Vec<Task>> {
        Ok(vec![])
    }
    async fn list_sections(&self, _list_id: &str) -> cal_core::Result<Vec<Section>> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(self.sections.clone())
    }
    async fn create_task(&self, _list_id: &str, _task: NewTask) -> cal_core::Result<Task> {
        unreachable!("not exercised by the sections refresh path")
    }
    async fn update_task(&self, _task: Task) -> cal_core::Result<Task> {
        unreachable!("not exercised by the sections refresh path")
    }
    async fn delete_task(&self, _task_id: &str) -> cal_core::Result<()> {
        unreachable!("not exercised by the sections refresh path")
    }
}

#[tokio::test]
async fn refresh_sections_warms_the_cache_from_the_adapter() {
    let store = Arc::new(setup());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let adapter = SectionAdapter {
        sections: vec![section("s1", 0), section("s2", 1)],
        calls: calls.clone(),
    };
    // Cold: no snapshot → a section read would go live. The refresh hits the
    // adapter once and warms the cache with its sections.
    super::refresh_sections(&store, &adapter, ACC, LIST)
        .await
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    let ids: Vec<String> = store
        .read_sections(ACC, LIST)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, ["s1", "s2"]);
    // Warm: the sections now serve from the cache — `has_snapshot` is true, so
    // the SWR read path uses `read_sections` instead of the live adapter call.
    assert!(super::has_snapshot(
        &store
            .get_sync_state(ACC, SyncScope::Sections, LIST)
            .unwrap()
    ));
}

#[tokio::test]
async fn refresh_sections_drops_a_write_whose_fetch_predates_an_invalidate() {
    let store = Arc::new(setup());
    // Warm the list's sections with s1.
    store
        .replace_sections(ACC, LIST, &[section("s1", 0)])
        .unwrap();

    // Invalidate mid-fetch (a section edit landing during a slow warm), then the
    // adapter returns a DIFFERENT set (s2). The generation guard must drop that
    // stale write, leaving the warmed s1 in place.
    struct MidFetchSectionInvalidator {
        cache: Arc<CacheStore>,
        sections: Vec<Section>,
    }
    #[async_trait]
    impl Adapter for MidFetchSectionInvalidator {
        async fn authenticate(&self, _c: Credentials) -> cal_core::Result<AuthToken> {
            Err(cal_core::Error::Unsupported("test fake".into()))
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }
    #[async_trait]
    impl TasksFeature for MidFetchSectionInvalidator {
        async fn list_task_lists(&self) -> cal_core::Result<Vec<TaskList>> {
            Ok(vec![])
        }
        async fn get_tasks(&self, _l: &str) -> cal_core::Result<Vec<Task>> {
            Ok(vec![])
        }
        async fn list_sections(&self, _l: &str) -> cal_core::Result<Vec<Section>> {
            self.cache
                .invalidate(ACC, SyncScope::Sections, LIST)
                .unwrap();
            Ok(self.sections.clone())
        }
        async fn create_task(&self, _l: &str, _t: NewTask) -> cal_core::Result<Task> {
            unreachable!()
        }
        async fn update_task(&self, _t: Task) -> cal_core::Result<Task> {
            unreachable!()
        }
        async fn delete_task(&self, _id: &str) -> cal_core::Result<()> {
            unreachable!()
        }
    }

    let adapter = MidFetchSectionInvalidator {
        cache: store.clone(),
        sections: vec![section("s2", 0)],
    };
    super::refresh_sections(&store, &adapter, ACC, LIST)
        .await
        .unwrap();

    let ids: Vec<String> = store
        .read_sections(ACC, LIST)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(
        ids,
        ["s1"],
        "a sections refresh whose fetch predates an invalidate must not overwrite the cache",
    );
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
        unfetched: Vec::new(),
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

// ── Cache-generation auto re-bootstrap (reconcile_cache_generation) ──

#[test]
fn reconcile_cache_generation_resets_external_accounts_once() {
    use crate::accounts::AccountsRepo;
    use crate::user_prefs::UserPrefsRepo;

    let db = DbHandle::open_in_memory().unwrap();
    let store = CacheStore::new(db.clone());
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);

    // One external (caldav) account — the implicit 'local' account already
    // exists from the migrations. Give each a sync-state row carrying a token +
    // window so a reset is observable as those going NULL.
    db.with_conn(|c| {
        c.execute(
            "INSERT INTO accounts (id, adapter_kind, display_name, config_json, created_at, updated_at)
             VALUES ('ext','caldav','Work','{}','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            params![],
        )?;
        for acc in ["ext", "local"] {
            c.execute(
                "INSERT INTO cache_sync_state
                   (account_id, scope, container_id, sync_token, window_start, window_end, last_refreshed_at)
                 VALUES (?1, 'events', 'c1', 'tok', '2026-01-01T00:00:00Z', '2026-02-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                params![acc],
            )?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .unwrap();

    let accounts = AccountsRepo::new(&shared).list().unwrap();
    let window_of = |acc: &str| -> Option<String> {
        db.with_conn(|c| {
            c.query_row(
                "SELECT window_start FROM cache_sync_state WHERE account_id = ?1",
                params![acc],
                |r| r.get::<_, Option<String>>(0),
            )
        })
        .unwrap()
    };

    // First run: only the EXTERNAL account is reset; the generation is recorded.
    let n = super::reconcile_cache_generation(&store, &accounts, &prefs).unwrap();
    assert_eq!(n, 1, "exactly the external account's sync-state row reset");
    assert!(window_of("ext").is_none(), "external sync window cleared");
    assert!(window_of("local").is_some(), "local account left intact");
    assert_eq!(
        prefs.get(super::CACHE_GENERATION_KEY).unwrap().as_deref(),
        Some(super::CACHE_GENERATION.to_string().as_str()),
    );

    // Second run is a no-op — the generation is already applied.
    assert_eq!(
        super::reconcile_cache_generation(&store, &accounts, &prefs).unwrap(),
        0,
    );
}

// ── Change detection (no-op refreshes stay UI-silent) ────────────────
//
// The replace_*/apply_*_delta writes report whether cached CONTENT
// changed so the refresh paths can skip `cache-updated` notifications.
// A warm pass that re-fetches identical data must not trigger frontend
// reload waves (the app-start entry-count oscillation).

#[test]
fn replace_identical_events_reports_unchanged() {
    let store = setup();
    let events = [event("e1", 8, 9), event("e2", 10, 11)];
    assert!(
        store
            .replace_calendar_events(ACC, CAL, wide(), &events)
            .unwrap(),
        "first write populates → changed"
    );
    assert!(
        !store
            .replace_calendar_events(ACC, CAL, wide(), &events)
            .unwrap(),
        "byte-identical re-fetch → unchanged"
    );
    // Freshness is still stamped on the unchanged path.
    let state = store
        .get_sync_state(ACC, SyncScope::Events, CAL)
        .unwrap()
        .unwrap();
    assert!(state.last_refreshed_at.is_some());
}

#[test]
fn replace_detects_edits_additions_and_removals() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[event("e1", 8, 9), event("e2", 10, 11)])
        .unwrap();

    // Same ids, one edited payload → changed.
    let mut edited = event("e1", 8, 9);
    edited.title = "Renamed".into();
    assert!(store
        .replace_calendar_events(ACC, CAL, wide(), &[edited.clone(), event("e2", 10, 11)])
        .unwrap());

    // Removal (subset) → changed.
    assert!(store
        .replace_calendar_events(ACC, CAL, wide(), &[edited.clone()])
        .unwrap());

    // Addition → changed.
    assert!(store
        .replace_calendar_events(ACC, CAL, wide(), &[edited, event("e3", 12, 13)])
        .unwrap());
}

#[test]
fn replace_identical_content_still_updates_the_window() {
    // A wide warm pass after a narrow fetch of the SAME rows must record
    // the wider window (coverage bookkeeping) even though content — and
    // thus the UI notification — is unchanged.
    let store = setup();
    let events = [event("e1", 8, 9)];
    store
        .replace_calendar_events(ACC, CAL, range(8, 18), &events)
        .unwrap();
    let changed = store
        .replace_calendar_events(ACC, CAL, wide(), &events)
        .unwrap();
    assert!(!changed, "identical rows → unchanged");
    let (ws, we) = store.event_window(ACC, CAL).unwrap().unwrap();
    assert_eq!(ws, wide().start);
    assert_eq!(we, wide().end);
}

#[test]
fn empty_events_delta_reports_unchanged_but_persists_the_token() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[event("e1", 8, 9)])
        .unwrap();
    let changed = store
        .apply_events_delta(
            ACC,
            CAL,
            &Delta {
                changes: Vec::new(),
                deletions: Vec::new(),
                new_token: Some("cursor-2".into()),
            },
        )
        .unwrap();
    assert!(!changed, "token-only delta → unchanged");
    let state = store
        .get_sync_state(ACC, SyncScope::Events, CAL)
        .unwrap()
        .unwrap();
    assert_eq!(state.sync_token.as_deref(), Some("cursor-2"));
}

#[test]
fn delta_deleting_a_phantom_row_reports_unchanged() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[event("e1", 8, 9)])
        .unwrap();
    let changed = store
        .apply_events_delta(
            ACC,
            CAL,
            &Delta {
                changes: Vec::new(),
                deletions: vec!["never-cached".into()],
                new_token: None,
            },
        )
        .unwrap();
    assert!(!changed, "deletion that matched no row → unchanged");
}

#[test]
fn delta_with_a_real_change_or_deletion_reports_changed() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[event("e1", 8, 9)])
        .unwrap();
    assert!(store
        .apply_events_delta(
            ACC,
            CAL,
            &Delta {
                changes: vec![event("e2", 10, 11)],
                deletions: Vec::new(),
                new_token: None,
            },
        )
        .unwrap());
    assert!(store
        .apply_events_delta(
            ACC,
            CAL,
            &Delta {
                changes: Vec::new(),
                deletions: vec!["e1".into()],
                new_token: None,
            },
        )
        .unwrap());
}

#[test]
fn replace_identical_tasks_and_sections_report_unchanged() {
    let store = setup();
    let tasks = [task("t1"), task("t2")];
    assert!(store.replace_list_tasks(ACC, LIST, &tasks).unwrap());
    assert!(!store.replace_list_tasks(ACC, LIST, &tasks).unwrap());

    let sections = [section("s1", 0)];
    assert!(store.replace_sections(ACC, LIST, &sections).unwrap());
    assert!(!store.replace_sections(ACC, LIST, &sections).unwrap());
}

#[test]
fn replace_identical_listings_report_unchanged() {
    let store = setup();
    let cals = [calendar("a"), calendar("b")];
    assert!(store.replace_calendars(ACC, &cals).unwrap());
    assert!(!store.replace_calendars(ACC, &cals).unwrap());
    // Reordering the slice is NOT a change — comparison is by id.
    assert!(!store
        .replace_calendars(ACC, &[calendar("b"), calendar("a")])
        .unwrap());

    let lists = [task_list("x")];
    assert!(store.replace_task_lists(ACC, &lists).unwrap());
    assert!(!store.replace_task_lists(ACC, &lists).unwrap());

    let books = [contact_list("z")];
    assert!(store.replace_contact_lists(ACC, &books).unwrap());
    assert!(!store.replace_contact_lists(ACC, &books).unwrap());
}

#[test]
fn empty_tasks_delta_reports_unchanged() {
    let store = setup();
    store.replace_list_tasks(ACC, LIST, &[task("t1")]).unwrap();
    assert!(!store
        .apply_tasks_delta(
            ACC,
            LIST,
            &Delta {
                changes: Vec::new(),
                deletions: Vec::new(),
                new_token: Some("cursor-9".into()),
            },
        )
        .unwrap());
}

// ── Full-resync preservation of unfetched resources ──────────────────

/// A fake calendar adapter whose delta reports a FULL RESYNC that could
/// not fetch some enumerated resources (`unfetched`) — the CalDAV
/// partial-multiget shape. The host must preserve those resources'
/// previously cached rows instead of dropping them with the replace.
struct PartialBootstrapAdapter {
    changes: Vec<Event>,
    unfetched: Vec<String>,
}

#[async_trait]
impl Adapter for PartialBootstrapAdapter {
    async fn authenticate(&self, _c: Credentials) -> cal_core::Result<AuthToken> {
        Err(cal_core::Error::Unsupported("test fake".into()))
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }
}

#[async_trait]
impl cal_core::CalendarFeature for PartialBootstrapAdapter {
    async fn list_calendars(&self) -> cal_core::Result<Vec<Calendar>> {
        Ok(vec![])
    }
    async fn get_events(
        &self,
        _calendar_id: &str,
        _range: DateRange,
    ) -> cal_core::Result<Vec<Event>> {
        unreachable!("delta path is used")
    }
    async fn get_events_delta(
        &self,
        _calendar_id: &str,
        _range: DateRange,
        _since_token: Option<&str>,
    ) -> cal_core::Result<cal_core::ChangeSet<Event>> {
        Ok(cal_core::ChangeSet {
            changes: self.changes.clone(),
            full_resync: true,
            complete: true,
            unfetched: self.unfetched.clone(),
            ..Default::default()
        })
    }
    async fn create_event(
        &self,
        _calendar_id: &str,
        _event: cal_core::NewEvent,
    ) -> cal_core::Result<Event> {
        unreachable!()
    }
    async fn update_event(&self, _event: Event) -> cal_core::Result<Event> {
        unreachable!()
    }
    async fn delete_event(
        &self,
        _event_id: &str,
        _send_cancellations: bool,
    ) -> cal_core::Result<()> {
        unreachable!()
    }
    async fn get_free_busy(
        &self,
        _emails: &[&str],
        _range: DateRange,
    ) -> cal_core::Result<Vec<cal_core::FreeBusy>> {
        unreachable!()
    }
    fn calendar_color(&self, _calendar_id: &str) -> Option<cal_core::ContainerColor> {
        None
    }
    async fn add_event_exdate(
        &self,
        _event_id: &str,
        _date: chrono::DateTime<chrono::Utc>,
        _send_cancellations: bool,
    ) -> cal_core::Result<()> {
        unreachable!()
    }
}

#[tokio::test]
async fn full_resync_preserves_unfetched_resources() {
    let store = setup();
    // Warm snapshot: two resources (CalDAV-style `href|uid` ids, so the
    // native id is the href).
    let kept = event("hrefA|uid-a", 8, 9);
    let refreshed = event("hrefB|uid-b", 10, 11);
    store
        .replace_calendar_events(ACC, CAL, wide(), &[kept.clone(), refreshed.clone()])
        .unwrap();

    // Re-bootstrap: the server refused hrefA this time; the full set only
    // carries an UPDATED hrefB.
    let mut updated = event("hrefB|uid-b", 10, 12);
    updated.title = "Updated".into();
    let adapter = PartialBootstrapAdapter {
        changes: vec![updated.clone()],
        unfetched: vec!["hrefA".into()],
    };
    let changed = super::swr::refresh_events(&store, &adapter, ACC, CAL, wide())
        .await
        .unwrap();
    assert!(changed, "the update to hrefB is a real change");

    let events = store.read_events(ACC, CAL, wide()).unwrap();
    let mut ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec!["hrefA|uid-a", "hrefB|uid-b"],
        "the unfetched resource's cached row survives the full replace"
    );
    let b = events.iter().find(|e| e.id == "hrefB|uid-b").unwrap();
    assert_eq!(b.title, "Updated");
}

#[test]
fn read_events_by_native_matches_the_native_column() {
    let store = setup();
    store
        .replace_calendar_events(
            ACC,
            CAL,
            wide(),
            &[event("hrefA|uid-a", 8, 9), event("hrefB|uid-b", 10, 11)],
        )
        .unwrap();
    let rows = store
        .read_events_by_native(ACC, CAL, &["hrefA".into()])
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "hrefA|uid-a");
}

// ── Range-scoped replace (no-delta adapters) ─────────────────────────

#[test]
fn in_range_replace_keeps_rows_outside_the_fetched_range() {
    let store = setup();
    // Warm cache: morning + afternoon events over the whole day.
    store
        .replace_calendar_events(ACC, CAL, wide(), &[event("m", 8, 9), event("a", 14, 15)])
        .unwrap();

    // A view-sized refresh over the morning half replaces only that swath.
    let mut renamed = event("m", 8, 10);
    renamed.title = "Extended".into();
    let changed = store
        .replace_calendar_events_in_range(ACC, CAL, range(6, 12), &[renamed])
        .unwrap();
    assert!(changed);

    let events = store.read_events(ACC, CAL, wide()).unwrap();
    let mut ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec!["a", "m"],
        "afternoon row survives the morning refresh"
    );
    // Window is the union (the fetched range touches/overlaps the wide one).
    let (ws, we) = store.event_window(ACC, CAL).unwrap().unwrap();
    assert_eq!(ws, wide().start);
    assert_eq!(we, wide().end);
}

#[test]
fn in_range_replace_reports_unchanged_for_identical_rows() {
    let store = setup();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[event("m", 8, 9), event("a", 14, 15)])
        .unwrap();
    assert!(!store
        .replace_calendar_events_in_range(ACC, CAL, range(6, 12), &[event("m", 8, 9)])
        .unwrap());
}

#[test]
fn in_range_replace_disjoint_window_records_only_the_fetched_range() {
    let store = setup();
    // Existing window: morning only.
    store
        .replace_calendar_events(ACC, CAL, range(6, 8), &[])
        .unwrap();
    // Disjoint fetch (afternoon) — union across the gap would fabricate
    // coverage of the 8–14h hole, so only the fetched range is recorded.
    store
        .replace_calendar_events_in_range(ACC, CAL, range(14, 18), &[event("a", 14, 15)])
        .unwrap();
    let (ws, we) = store.event_window(ACC, CAL).unwrap().unwrap();
    assert_eq!(ws, range(14, 18).start);
    assert_eq!(we, range(14, 18).end);
}

// ── Listing prune ────────────────────────────────────────────────────

#[test]
fn dropping_a_calendar_from_the_listing_prunes_its_cached_events() {
    let store = setup();
    store
        .replace_calendars(ACC, &[calendar(CAL), calendar("other")])
        .unwrap();
    store
        .replace_calendar_events(ACC, CAL, wide(), &[event("e1", 8, 9)])
        .unwrap();
    store
        .replace_calendar_events(ACC, "other", wide(), &[event("o1", 10, 11)])
        .unwrap();

    // The provider no longer lists CAL — an authoritative removal.
    store.replace_calendars(ACC, &[calendar("other")]).unwrap();

    assert!(
        store.read_events(ACC, CAL, wide()).unwrap().is_empty(),
        "dropped calendar's event rows pruned"
    );
    assert!(
        store
            .get_sync_state(ACC, SyncScope::Events, CAL)
            .unwrap()
            .is_none(),
        "dropped calendar's sync state pruned"
    );
    assert_eq!(
        store.read_events(ACC, "other", wide()).unwrap().len(),
        1,
        "surviving calendar untouched"
    );
}

#[test]
fn dropping_a_task_list_prunes_tasks_and_sections() {
    let store = setup();
    store.replace_task_lists(ACC, &[task_list(LIST)]).unwrap();
    store.replace_list_tasks(ACC, LIST, &[task("t1")]).unwrap();
    store
        .replace_sections(ACC, LIST, &[section("s1", 0)])
        .unwrap();

    store.replace_task_lists(ACC, &[]).unwrap();

    assert!(store.read_tasks(ACC, LIST).unwrap().is_empty());
    assert!(store.read_sections(ACC, LIST).unwrap().is_empty());
}

// ── Forced rewrite after a sync-state reset ──────────────────────────

#[test]
fn identical_replace_after_reset_rewrites_rows() {
    let store = setup();
    let events = [event("e1", 8, 9)];
    store
        .replace_calendar_events(ACC, CAL, wide(), &events)
        .unwrap();
    let stamp_before: String = store
        .db
        .with_conn(|c| {
            c.query_row(
                "SELECT cached_at FROM cache_events WHERE id = 'e1'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();

    // A generation reset / re-sync-from-scratch NULLs the freshness; the
    // next full fetch must REWRITE even byte-identical rows so payload-
    // derived columns are recomputed with current code…
    store.reset_account_sync(ACC).unwrap();
    // cached_at is millisecond-precision — make sure the rewrite lands on a
    // distinct stamp even on a fast machine.
    std::thread::sleep(std::time::Duration::from_millis(3));
    let changed = store
        .replace_calendar_events(ACC, CAL, wide(), &events)
        .unwrap();
    assert!(
        !changed,
        "…while still reporting unchanged content to the UI"
    );
    let stamp_after: String = store
        .db
        .with_conn(|c| {
            c.query_row(
                "SELECT cached_at FROM cache_events WHERE id = 'e1'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_ne!(stamp_before, stamp_after, "row was physically rewritten");
}

// ── Per-account refresh-error surface ────────────────────────────────

#[test]
fn refresh_errors_groups_per_account_and_resolves_names() {
    let store = setup();
    // A listed calendar whose events refresh failed → named entry.
    store.replace_calendars(ACC, &[calendar(CAL)]).unwrap();
    store
        .mark_error(ACC, SyncScope::Events, CAL, "HTTP 401 Unauthorized")
        .unwrap();
    // A task list the listing does NOT know → unnamed entry, non-auth.
    store
        .mark_error(
            ACC,
            SyncScope::Tasks,
            "list-unknown",
            "connection timed out",
        )
        .unwrap();

    // A SECOND account failing independently → its own group, and its
    // non-auth error must not inherit the first account's auth flag.
    store
        .db
        .with_conn(|c| {
            c.execute(
                "INSERT INTO accounts (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES ('acc-2', 'caldav', 'Home', '{}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
        })
        .unwrap();
    store
        .mark_error("acc-2", SyncScope::Contacts, "", "connection reset")
        .unwrap();

    let errors = store.refresh_errors().unwrap();
    assert_eq!(errors.len(), 2, "one group per failing account");
    let acc = errors.iter().find(|a| a.account_id == ACC).unwrap();
    assert!(acc.auth_suspected, "401 counts as auth-shaped");
    assert_eq!(acc.errors.len(), 2);
    let events_err = acc.errors.iter().find(|e| e.scope == "events").unwrap();
    assert_eq!(events_err.container_name.as_deref(), Some("Cal cal-1"));
    let tasks_err = acc.errors.iter().find(|e| e.scope == "tasks").unwrap();
    assert!(tasks_err.container_name.is_none());
    let other = errors.iter().find(|a| a.account_id == "acc-2").unwrap();
    assert!(
        !other.auth_suspected,
        "auth flag must not leak across accounts"
    );
    assert_eq!(other.errors.len(), 1);
}

#[test]
fn refresh_errors_prefer_the_rename_override() {
    let store = setup();
    store.replace_calendars(ACC, &[calendar(CAL)]).unwrap();
    store
        .db
        .with_conn(|c| {
            c.execute(
                "INSERT INTO container_name_overrides (container_id, kind, name, updated_at)
                 VALUES (?1, 'calendar', 'Arbeit', '2026-01-01T00:00:00Z')",
                rusqlite::params![CAL],
            )
        })
        .unwrap();
    store
        .mark_error(ACC, SyncScope::Events, CAL, "HTTP 401 Unauthorized")
        .unwrap();

    let errors = store.refresh_errors().unwrap();
    let err = &errors[0].errors[0];
    assert_eq!(
        err.container_name.as_deref(),
        Some("Arbeit"),
        "the error surface must use the same name as every other surface"
    );
}

#[test]
fn refresh_errors_skip_containers_dropped_from_the_listing() {
    let store = setup();
    // Non-empty calendar listing that does NOT contain "cal-gone": the
    // container was deleted server-side; a refresh against a stale
    // persisted selection re-created its sync-state row. Without the
    // orphan filter this would be a permanent unnamed warning.
    store.replace_calendars(ACC, &[calendar(CAL)]).unwrap();
    store
        .mark_error(ACC, SyncScope::Events, "cal-gone", "HTTP 404 Not Found")
        .unwrap();
    assert!(
        store.refresh_errors().unwrap().is_empty(),
        "orphaned container rows must not surface"
    );

    // But with a COLD (empty) listing the same row must surface — the
    // listing has no authority yet (it may itself be what is failing).
    store
        .mark_error(ACC, SyncScope::Tasks, "list-cold", "connection timed out")
        .unwrap();
    let errors = store.refresh_errors().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].errors[0].container_id, "list-cold");
}

#[test]
fn refresh_errors_skip_containers_after_the_listing_emptied() {
    let store = setup();
    // The account's ONLY calendar is deleted server-side: a successful
    // listing pass replaces the listing with the EMPTY set. A refresh
    // against a stale persisted selection then 404s and re-creates the
    // sync-state row. Empty + succeeded-at-least-once = authoritative,
    // so the orphan must not surface (it could never clear).
    store.replace_calendars(ACC, &[calendar(CAL)]).unwrap();
    store.replace_calendars(ACC, &[]).unwrap();
    store
        .mark_error(ACC, SyncScope::Events, CAL, "HTTP 404 Not Found")
        .unwrap();
    assert!(
        store.refresh_errors().unwrap().is_empty(),
        "authoritatively-empty listing must orphan the row"
    );
}

#[test]
fn refresh_errors_clear_after_a_successful_write() {
    let store = setup();
    store
        .mark_error(ACC, SyncScope::Tasks, LIST, "boom")
        .unwrap();
    assert_eq!(store.refresh_errors().unwrap().len(), 1);
    // Any successful replace clears last_error for the container.
    store.replace_list_tasks(ACC, LIST, &[task("t1")]).unwrap();
    assert!(store.refresh_errors().unwrap().is_empty());
}

#[test]
fn auth_shaped_heuristic() {
    assert!(super::is_auth_shaped("HTTP 401 Unauthorized"));
    assert!(super::is_auth_shaped("server said: invalid credentials"));
    assert!(super::is_auth_shaped("403 Forbidden"));
    // Revoked OAuth grant: the token endpoint's HTTP 400 body embedded
    // in a protocol error — the exact string shape both OAuth adapters
    // record (Google/Graph map token failures to a 400, not a 401).
    assert!(super::is_auth_shaped(
        "protocol error: Google HTTP 400: {\"error\":\"invalid_grant\",\
         \"error_description\":\"Token has been expired or revoked.\"}"
    ));
    assert!(!super::is_auth_shaped("connection reset by peer"));
    assert!(!super::is_auth_shaped("timeout after 30s"));
}
