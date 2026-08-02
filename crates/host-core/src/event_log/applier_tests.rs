//! DB-backed integration tests for the event-log applier, which now lives
//! in `sync-engine`. They drive the real `EventLogApplier` against the
//! real `DesktopSyncStore` + `LocalAdapter` over a migrated in-memory
//! SQLite, with an in-memory keychain (`FakeSecrets`) so credential events
//! don't touch the OS store. Moved here when the applier was extracted —
//! same assertions; only the applier construction changed (it now goes
//! through `make_applier`, which wires up the two platform seams).

use std::sync::Arc;

use adapter_local::LocalAdapter;
use cal_core::{Calendar, ColorLabel, Event, Reminder};
use chrono::{TimeZone, Utc};
use rusqlite::params;
use sync_core::{DeviceId, EventEnvelope, EventPayload, IdPayload, SettingsPayload, SyncEvent};
use sync_engine::test_support::FakeSecrets;
use sync_engine::EventLogApplier;

use crate::conflicts::ConflictKind;
use crate::db::SharedConn;
use crate::event_log::DesktopSyncStore;
use crate::user_prefs::UserPrefsRepo;

/// Set up an in-memory DB + adapter for the test. The
/// LocalAdapter's `open_test_db` already runs every migration
/// including 0012 (sync_applied_events), so the applier can
/// write its idempotency rows.
fn fixture() -> (Arc<LocalAdapter>, SharedConn) {
    let shared = adapter_local::test_support::open_test_db();
    let adapter = Arc::new(LocalAdapter::new(shared.clone()));
    (adapter, shared)
}

/// Build a real `EventLogApplier` over the desktop store + an in-memory
/// keychain. Takes references so call sites keep their `db` / `adapter`
/// handles for post-apply assertions.
fn make_applier(db: &SharedConn, adapter: &Arc<LocalAdapter>, device: DeviceId) -> EventLogApplier {
    let store = Arc::new(DesktopSyncStore::new(db.clone(), Arc::clone(adapter)));
    EventLogApplier::new(
        store,
        Arc::new(FakeSecrets::default()),
        Arc::clone(adapter),
        device,
    )
}

fn fixture_event(id: &str, calendar_id: &str) -> Event {
    Event {
        id: id.into(),
        calendar_id: calendar_id.into(),
        title: "Synced from elsewhere".into(),
        description: None,
        location: None,
        start: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0).unwrap(),
        all_day: false,
        recurrence: None,
        color_label: None,
        color_hex: None,
        reminders: Vec::<Reminder>::new(),
        sound: None,
        attendees: Vec::new(),
        send_invitations: false,
        truncate_tail_overrides: false,
        created_at: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
        etag: None,
        organizer: None,
        attendee_responses: Vec::new(),
        cancelled: false,
    }
}

fn fixture_envelope(device_id: DeviceId, event: SyncEvent, timestamp_secs: i64) -> EventEnvelope {
    EventEnvelope {
        id: format!("evt_{:013x}", timestamp_secs),
        device_id,
        timestamp: Utc.timestamp_opt(timestamp_secs, 0).unwrap(),
        event,
    }
}

fn fixture_calendar(id: &str) -> Calendar {
    Calendar {
        color_label: None,
        supports_scheduling: false,
        supports_event_color: false,
        id: id.into(),
        name: "From remote".into(),
        color: None,
        read_only: false,
        default_sound: None,
    }
}

#[test]
fn apply_event_created_inserts_row() {
    let (adapter, db) = fixture();
    let other = DeviceId::from_string("dev-other".into());
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me);

    // Need a calendar locally for the FK to succeed —
    // apply CalendarCreated first.
    let cal = fixture_calendar("cal-x");
    let env_cal = fixture_envelope(
        other.clone(),
        SyncEvent::CalendarCreated(EventPayload {
            id: cal.id.clone(),
            fields: serde_json::to_value(&cal).unwrap(),
        }),
        1000,
    );
    let env_ev = fixture_envelope(
        other,
        SyncEvent::EventCreated(EventPayload {
            id: "ev-1".into(),
            fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
        }),
        2000,
    );

    let report = applier
        .apply_envelopes(vec![env_ev, env_cal]) // out of order on purpose
        .unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(report.skipped_already_applied, 0);
    assert_eq!(report.failed, 0);

    // Row should be queryable from SQLite.
    let conn = db.lock().unwrap();
    let title: String = conn
        .query_row(
            "SELECT title FROM events WHERE id = ?",
            params!["ev-1"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(title, "Synced from elsewhere");
}

#[test]
fn applying_same_envelope_twice_is_idempotent() {
    let (adapter, db) = fixture();
    let other = DeviceId::from_string("dev-other".into());
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me);

    let cal = fixture_calendar("cal-x");
    let envelopes = vec![
        fixture_envelope(
            other.clone(),
            SyncEvent::CalendarCreated(EventPayload {
                id: cal.id.clone(),
                fields: serde_json::to_value(&cal).unwrap(),
            }),
            1000,
        ),
        fixture_envelope(
            other,
            SyncEvent::EventCreated(EventPayload {
                id: "ev-1".into(),
                fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
            }),
            2000,
        ),
    ];

    let first = applier.apply_envelopes(envelopes.clone()).unwrap();
    assert_eq!(first.applied, 2);

    let second = applier.apply_envelopes(envelopes).unwrap();
    // Second pass: both rows hit sync_applied_events.
    assert_eq!(second.applied, 0);
    assert_eq!(second.skipped_already_applied, 2);
}

#[test]
fn applying_own_device_envelopes_skips_them() {
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me.clone());

    let cal = fixture_calendar("cal-x");
    // Both envelopes from this device — the applier should
    // count them as `skipped_own` and not touch the DB.
    let envelopes = vec![
        fixture_envelope(
            me.clone(),
            SyncEvent::CalendarCreated(EventPayload {
                id: cal.id.clone(),
                fields: serde_json::to_value(&cal).unwrap(),
            }),
            1000,
        ),
        fixture_envelope(
            me,
            SyncEvent::EventCreated(EventPayload {
                id: "ev-1".into(),
                fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
            }),
            2000,
        ),
    ];
    let report = applier.apply_envelopes(envelopes).unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.skipped_own, 2);
    // Calendar table is empty — own-device envelopes never
    // touched it.
    let conn = db.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM calendars WHERE id = 'cal-x'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn force_own_re_applies_own_device_envelopes() {
    // §19.10 stale-resume invariant: after a snapshot apply
    // overwrites local rows, our own pending logs should be
    // replayable through the applier so offline edits come
    // back. `apply_envelopes_force_own` is the path that
    // makes this possible — bypass the skip_own filter that
    // a normal sync round honours.
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me.clone());

    let cal = fixture_calendar("cal-x");
    let envelopes = vec![
        fixture_envelope(
            me.clone(),
            SyncEvent::CalendarCreated(EventPayload {
                id: cal.id.clone(),
                fields: serde_json::to_value(&cal).unwrap(),
            }),
            1000,
        ),
        fixture_envelope(
            me,
            SyncEvent::EventCreated(EventPayload {
                id: "ev-1".into(),
                fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
            }),
            2000,
        ),
    ];
    let report = applier.apply_envelopes_force_own(envelopes).unwrap();
    // Both envelopes flowed through the dispatch instead of
    // being skipped — calendar + event landed in SQLite.
    assert_eq!(report.applied, 2);
    assert_eq!(report.skipped_own, 0);
    let conn = db.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM calendars WHERE id = 'cal-x'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn force_own_honours_already_applied_check() {
    // A second `apply_envelopes_force_own` pass over the
    // same envelopes is idempotent — the first pass writes
    // to `sync_applied_events`, so the second sees them as
    // already-applied and counts them in the right bucket.
    // Important because stale-resume might re-run if the
    // user dismisses and re-triggers.
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me.clone());
    let cal = fixture_calendar("cal-y");
    let env = fixture_envelope(
        me,
        SyncEvent::CalendarCreated(EventPayload {
            id: cal.id.clone(),
            fields: serde_json::to_value(&cal).unwrap(),
        }),
        3000,
    );
    let first = applier
        .apply_envelopes_force_own(vec![env.clone()])
        .unwrap();
    assert_eq!(first.applied, 1);
    let second = applier.apply_envelopes_force_own(vec![env]).unwrap();
    assert_eq!(second.applied, 0);
    assert_eq!(second.skipped_already_applied, 1);
}

#[test]
fn apply_event_deleted_removes_row() {
    let (adapter, db) = fixture();
    let other = DeviceId::from_string("dev-other".into());
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me);

    let cal = fixture_calendar("cal-x");
    // Apply create + delete.
    applier
        .apply_envelopes(vec![
            fixture_envelope(
                other.clone(),
                SyncEvent::CalendarCreated(EventPayload {
                    id: cal.id.clone(),
                    fields: serde_json::to_value(&cal).unwrap(),
                }),
                1000,
            ),
            fixture_envelope(
                other.clone(),
                SyncEvent::EventCreated(EventPayload {
                    id: "ev-1".into(),
                    fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
                }),
                2000,
            ),
            fixture_envelope(
                other,
                SyncEvent::EventDeleted(IdPayload { id: "ev-1".into() }),
                3000,
            ),
        ])
        .unwrap();

    let conn = db.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM events WHERE id = 'ev-1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn apply_color_label_created_inserts_row() {
    let (adapter, db) = fixture();
    let other = DeviceId::from_string("dev-other".into());
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me);

    let label = ColorLabel {
        id: cal_core::ColorLabelId::new("lbl-a"),
        name: "Work".into(),
        hex: "#ff0000".into(),
        ad_hoc: false,
    };
    let env = fixture_envelope(
        other,
        SyncEvent::ColorLabelCreated(EventPayload {
            id: "lbl-a".into(),
            fields: serde_json::to_value(&label).unwrap(),
        }),
        1000,
    );
    applier.apply_envelopes(vec![env]).unwrap();

    let conn = db.lock().unwrap();
    let name: String = conn
        .query_row(
            "SELECT name FROM color_labels WHERE id = ?",
            params!["lbl-a"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(name, "Work");
}

#[test]
fn settings_updated_writes_user_prefs() {
    let (adapter, db) = fixture();
    let other = DeviceId::from_string("dev-other".into());
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me);

    let env = fixture_envelope(
        other.clone(),
        SyncEvent::SettingsUpdated(SettingsPayload {
            key: "appearance.darkMode".into(),
            value: serde_json::json!(true),
        }),
        1000,
    );
    let env_delete = fixture_envelope(
        other,
        SyncEvent::SettingsUpdated(SettingsPayload {
            key: "appearance.colorScheme".into(),
            value: serde_json::Value::Null,
        }),
        2000,
    );
    // Pre-seed a value the delete event will remove.
    {
        let shared = db.clone();
        let repo = UserPrefsRepo::new(&shared);
        repo.set("appearance.colorScheme", "blue").unwrap();
    }

    applier.apply_envelopes(vec![env, env_delete]).unwrap();

    let shared = db.clone();
    let repo = UserPrefsRepo::new(&shared);
    // The set event arrived as a bool — encoded as "true".
    let dark = repo.get("appearance.darkMode").unwrap();
    assert_eq!(dark.as_deref(), Some("true"));
    // The delete event wiped the seeded row.
    assert!(repo.get("appearance.colorScheme").unwrap().is_none());
}

#[test]
fn unsupported_variants_count_as_skipped_not_failed() {
    let (adapter, db) = fixture();
    let other = DeviceId::from_string("dev-other".into());
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me);

    let env = fixture_envelope(
        other,
        SyncEvent::ShortcutSet(sync_core::ShortcutPayload {
            action: "event.save".into(),
            binding: "Mod+S".into(),
        }),
        1000,
    );
    let report = applier.apply_envelopes(vec![env]).unwrap();
    // We don't have a shortcut store yet — counted as
    // skipped_unsupported, not failed.
    assert_eq!(report.applied, 0);
    assert_eq!(report.skipped_unsupported, 1);
    assert_eq!(report.failed, 0);
}

#[test]
fn apply_section_created_then_deleted() {
    let (adapter, db) = fixture();
    let other = DeviceId::from_string("dev-other".into());
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me);

    // Seed the owning list so the section's FK is satisfied.
    let list = cal_core::TaskList {
        color_label: None,
        id: "list-1".into(),
        name: "Inbox".into(),
        color: None,
        default_sound: None,
        embedded_in_calendar: None,
        parent_id: None,
        read_only: false,
    };
    adapter.upsert_task_list_from_sync(&list).unwrap();

    let section = cal_core::Section {
        id: "sec-1".into(),
        list_id: "list-1".into(),
        name: "Doing".into(),
        color_label: None,
        order: 0,
    };
    let create = fixture_envelope(
        other.clone(),
        SyncEvent::SectionCreated(EventPayload {
            id: "sec-1".into(),
            fields: serde_json::to_value(&section).unwrap(),
        }),
        1000,
    );
    let report = applier.apply_envelopes(vec![create]).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(
        adapter.get_section_by_id("sec-1").unwrap().unwrap().name,
        "Doing",
    );

    let delete = fixture_envelope(
        other,
        SyncEvent::SectionDeleted(IdPayload { id: "sec-1".into() }),
        2000,
    );
    let report = applier.apply_envelopes(vec![delete]).unwrap();
    assert_eq!(report.applied, 1);
    assert!(adapter.get_section_by_id("sec-1").unwrap().is_none());
}

// -----------------------------------------------------------------
// Phase Sh — field-level merge + conflict detection.
// -----------------------------------------------------------------

use crate::conflicts::{ConflictsRepo, ResolutionChoice};

fn seed_event(adapter: &Arc<LocalAdapter>, id: &str) {
    let cal = fixture_calendar("cal-merge");
    adapter.upsert_calendar_from_sync(&cal).unwrap();
    let mut ev = fixture_event(id, "cal-merge");
    ev.title = "Original".into();
    ev.location = Some("Room A".into());
    // Seed a known updated_at so the merge timestamp math is
    // deterministic.
    ev.updated_at = Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap();
    adapter.upsert_event_from_sync(&ev).unwrap();
}

#[test]
fn merge_auto_merges_when_local_was_updated_before_envelope() {
    // Local last edited at T1 = 09:00. Remote envelope at T2 = 10:00.
    // The remote update is "newer" — auto-merge takes the remote
    // value for the differing field; no conflict recorded.
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let other = DeviceId::from_string("dev-other".into());
    let applier = make_applier(&db, &adapter, me);

    seed_event(&adapter, "ev-merge-1");

    // Build the patch — only the title changed remotely.
    let env = fixture_envelope(
        other.clone(),
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-merge-1".into(),
            fields: serde_json::json!({
                "title": "Updated remotely",
            }),
        }),
        // 10:00:00 UTC == 2026-05-12T10:00:00Z
        Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
            .unwrap()
            .timestamp(),
    );

    let report = applier.apply_envelopes(vec![env]).unwrap();
    assert_eq!(report.applied, 1);

    // Local row reflects the remote title; location preserved.
    let row = adapter.get_event_by_id("ev-merge-1").unwrap().unwrap();
    assert_eq!(row.title, "Updated remotely");
    assert_eq!(row.location.as_deref(), Some("Room A"));

    // No conflict recorded.
    let shared = db.clone();
    let repo = ConflictsRepo::new(&shared);
    assert_eq!(repo.unresolved_count().unwrap(), 0);
}

#[test]
fn merge_records_conflict_when_local_was_updated_after_envelope() {
    // Local last edited at T2 = 11:00 (e.g. user just made a
    // change). Remote envelope arrives with timestamp T1 = 10:00
    // — divergent timelines on the same field. The merge keeps
    // the local value and records a conflict row.
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let other = DeviceId::from_string("dev-other".into());
    let applier = make_applier(&db, &adapter, me);

    // Seed with a row whose updated_at is at T2.
    let cal = fixture_calendar("cal-conflict");
    adapter.upsert_calendar_from_sync(&cal).unwrap();
    let mut ev = fixture_event("ev-conflict", "cal-conflict");
    ev.title = "Local title".into();
    ev.updated_at = Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0).unwrap();
    adapter.upsert_event_from_sync(&ev).unwrap();

    // Remote envelope at T1 (older than local's updated_at).
    let env = fixture_envelope(
        other,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-conflict".into(),
            fields: serde_json::json!({
                "title": "Remote title",
            }),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
            .unwrap()
            .timestamp(),
    );

    applier.apply_envelopes(vec![env]).unwrap();

    // Local row keeps its value.
    let row = adapter.get_event_by_id("ev-conflict").unwrap().unwrap();
    assert_eq!(row.title, "Local title");

    // A conflict row was recorded.
    let shared = db.clone();
    let repo = ConflictsRepo::new(&shared);
    let conflicts = repo.list_unresolved().unwrap();
    assert_eq!(conflicts.len(), 1);
    let c = &conflicts[0];
    assert_eq!(c.field, "title");
    assert_eq!(c.row_kind, ConflictKind::Event);
    assert_eq!(c.row_id, "ev-conflict");
    // Values are JSON-encoded strings, so `"Local title"` not
    // `Local title`.
    assert_eq!(c.local_value.as_deref(), Some("\"Local title\""));
    assert_eq!(c.remote_value.as_deref(), Some("\"Remote title\""));
}

#[test]
fn merge_only_touches_changed_fields() {
    // Patch carries the full row but only the title differs from
    // local. Other fields (location, start, end) shouldn't
    // produce conflicts even when local's updated_at is newer —
    // they're equal, so the diff check short-circuits.
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let other = DeviceId::from_string("dev-other".into());
    let applier = make_applier(&db, &adapter, me);

    let cal = fixture_calendar("cal-equal");
    adapter.upsert_calendar_from_sync(&cal).unwrap();
    let mut ev = fixture_event("ev-equal", "cal-equal");
    ev.title = "Local".into();
    ev.location = Some("Room X".into());
    ev.updated_at = Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0).unwrap();
    adapter.upsert_event_from_sync(&ev).unwrap();

    // Remote patch carries the FULL row (which is how the
    // current writer emits) but only the title differs.
    let mut patched = ev.clone();
    patched.title = "Remote".into();
    let env = fixture_envelope(
        other,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-equal".into(),
            fields: serde_json::to_value(&patched).unwrap(),
        }),
        // Envelope older than local's updated_at → conflict
        // territory.
        Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
            .unwrap()
            .timestamp(),
    );

    applier.apply_envelopes(vec![env]).unwrap();
    let shared = db.clone();
    let repo = ConflictsRepo::new(&shared);
    let conflicts = repo.list_unresolved().unwrap();
    // Exactly one conflict: the title. Location / start / end
    // matched on both sides so no conflict for those.
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].field, "title");
}

#[test]
fn merge_skips_conflict_detection_on_metadata_fields() {
    // `updated_at` / `created_at` / `etag` diverge mechanically.
    // The applier silently takes the remote value for these,
    // never as a user-facing conflict.
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let other = DeviceId::from_string("dev-other".into());
    let applier = make_applier(&db, &adapter, me);

    let cal = fixture_calendar("cal-meta");
    adapter.upsert_calendar_from_sync(&cal).unwrap();
    let mut ev = fixture_event("ev-meta", "cal-meta");
    ev.updated_at = Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0).unwrap();
    ev.etag = Some("local-etag".into());
    adapter.upsert_event_from_sync(&ev).unwrap();

    // Remote patch: only `updated_at` differs (an "older" T1).
    let env = fixture_envelope(
        other,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-meta".into(),
            fields: serde_json::json!({
                "updated_at": "2026-05-12T10:00:00Z",
                "etag": "remote-etag",
            }),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
            .unwrap()
            .timestamp(),
    );
    applier.apply_envelopes(vec![env]).unwrap();

    // No conflict surfaced for metadata-only divergence.
    let shared = db.clone();
    let repo = ConflictsRepo::new(&shared);
    assert_eq!(repo.unresolved_count().unwrap(), 0);
}

#[test]
fn merge_falls_back_to_upsert_when_local_row_absent() {
    // Apply an `EventUpdated` before the corresponding
    // `EventCreated`. The merge path detects "no local row",
    // falls back to the regular upsert with the full payload —
    // same end state as if Created had arrived.
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let other = DeviceId::from_string("dev-other".into());
    let applier = make_applier(&db, &adapter, me);

    let cal = fixture_calendar("cal-absent");
    adapter.upsert_calendar_from_sync(&cal).unwrap();

    let new_event = fixture_event("ev-absent", "cal-absent");
    let env = fixture_envelope(
        other,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-absent".into(),
            fields: serde_json::to_value(&new_event).unwrap(),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
            .unwrap()
            .timestamp(),
    );
    applier.apply_envelopes(vec![env]).unwrap();

    let row = adapter.get_event_by_id("ev-absent").unwrap().unwrap();
    assert_eq!(row.title, "Synced from elsewhere");
}

/// The canonical field-level-merge scenario from DESIGN.md
/// §19.3: two devices each touch a different field of the
/// same event; both edits arrive at this device and both
/// must land without raising a conflict. This is the
/// promise that distinguishes Aperio from last-write-wins
/// designs.
#[test]
fn merge_concurrent_edits_to_different_fields_both_land() {
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let device_a = DeviceId::from_string("dev-a".into());
    let device_b = DeviceId::from_string("dev-b".into());
    let applier = make_applier(&db, &adapter, me);
    seed_event(&adapter, "ev-multifield");

    // Device A pushes a title change at T1=10:00.
    let env_a = fixture_envelope(
        device_a,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-multifield".into(),
            fields: serde_json::json!({ "title": "Title from A" }),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
            .unwrap()
            .timestamp(),
    );
    // Device B pushes a location change at T2=11:00 — touches
    // a *different* field than device A.
    let env_b = fixture_envelope(
        device_b,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-multifield".into(),
            fields: serde_json::json!({ "location": "Room from B" }),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0)
            .unwrap()
            .timestamp(),
    );

    applier.apply_envelopes(vec![env_a, env_b]).unwrap();

    // Both edits landed.
    let row = adapter.get_event_by_id("ev-multifield").unwrap().unwrap();
    assert_eq!(row.title, "Title from A");
    assert_eq!(row.location.as_deref(), Some("Room from B"));

    // No conflicts — the edits touched disjoint fields, so
    // the per-field "remote vs local" check never had a
    // reason to fire.
    let shared = db.clone();
    let repo = ConflictsRepo::new(&shared);
    assert_eq!(repo.unresolved_count().unwrap(), 0);
}

/// Order-independence of the previous scenario. Applying
/// device B's envelope first then device A's should produce
/// the same end state. Two devices reaching cluster-wide
/// consistency via the event log must not depend on the
/// order envelopes happen to be downloaded in.
#[test]
fn merge_concurrent_edits_converge_regardless_of_apply_order() {
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let device_a = DeviceId::from_string("dev-a".into());
    let device_b = DeviceId::from_string("dev-b".into());
    let applier = make_applier(&db, &adapter, me);
    seed_event(&adapter, "ev-order");

    let env_a = fixture_envelope(
        device_a,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-order".into(),
            fields: serde_json::json!({ "title": "Title from A" }),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
            .unwrap()
            .timestamp(),
    );
    let env_b = fixture_envelope(
        device_b,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-order".into(),
            fields: serde_json::json!({ "location": "Room from B" }),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0)
            .unwrap()
            .timestamp(),
    );

    // Apply in REVERSE order — B then A.
    applier.apply_envelopes(vec![env_b, env_a]).unwrap();

    let row = adapter.get_event_by_id("ev-order").unwrap().unwrap();
    assert_eq!(row.title, "Title from A");
    assert_eq!(row.location.as_deref(), Some("Room from B"));
    let shared = db.clone();
    let repo = ConflictsRepo::new(&shared);
    assert_eq!(repo.unresolved_count().unwrap(), 0);
}

/// Successive updates from the same device to the same
/// field LWW correctly — the second envelope's value wins,
/// no conflicts. Covers the simple "device A made two edits
/// in a row to the title; we get both eventually" case.
#[test]
fn merge_sequential_updates_from_same_device_last_write_wins() {
    let (adapter, db) = fixture();
    let me = DeviceId::from_string("dev-me".into());
    let device_a = DeviceId::from_string("dev-a".into());
    let applier = make_applier(&db, &adapter, me);
    seed_event(&adapter, "ev-lww");

    let env_t1 = fixture_envelope(
        device_a.clone(),
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-lww".into(),
            fields: serde_json::json!({ "title": "First edit" }),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
            .unwrap()
            .timestamp(),
    );
    let env_t2 = fixture_envelope(
        device_a,
        SyncEvent::EventUpdated(EventPayload {
            id: "ev-lww".into(),
            fields: serde_json::json!({ "title": "Second edit" }),
        }),
        Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0)
            .unwrap()
            .timestamp(),
    );

    applier.apply_envelopes(vec![env_t1, env_t2]).unwrap();

    let row = adapter.get_event_by_id("ev-lww").unwrap().unwrap();
    assert_eq!(row.title, "Second edit");
    let shared = db.clone();
    let repo = ConflictsRepo::new(&shared);
    assert_eq!(repo.unresolved_count().unwrap(), 0);
}

// Re-export `ResolutionChoice` so the warning compiler check
// doesn't fire on the new conflicts use.
#[allow(dead_code)]
fn _exercise_resolution_choice() {
    let _ = ResolutionChoice::KeepLocal;
}

/// AccountCreated from another device should land as an
/// upsert in the local `accounts` table; AccountUpdated
/// on the same id mutates the row; AccountDeleted drops it.
#[test]
fn account_events_round_trip_through_applier() {
    use sync_core::AccountPayload;
    let (adapter, db) = fixture();
    let other = DeviceId::from_string("dev-other".into());
    let me = DeviceId::from_string("dev-me".into());
    let applier = make_applier(&db, &adapter, me);

    let envs = vec![
        fixture_envelope(
            other.clone(),
            SyncEvent::AccountCreated(AccountPayload {
                id: "acc-1".into(),
                adapter_kind: "caldav".into(),
                display_name: "Work".into(),
                config_json: r#"{"server_url":"https://dav.example.com"}"#.into(),
                created_at: "2026-05-12T09:14:22Z".into(),
                updated_at: "2026-05-12T09:14:22Z".into(),
            }),
            1000,
        ),
        fixture_envelope(
            other.clone(),
            SyncEvent::AccountUpdated(AccountPayload {
                id: "acc-1".into(),
                adapter_kind: "caldav".into(),
                display_name: "Work (renamed)".into(),
                config_json: r#"{"server_url":"https://dav.example.com"}"#.into(),
                created_at: "2026-05-12T09:14:22Z".into(),
                updated_at: "2026-05-12T09:20:00Z".into(),
            }),
            2000,
        ),
    ];
    let report = applier.apply_envelopes(envs).unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(report.failed, 0);

    let conn = db.lock().unwrap();
    let name: String = conn
        .query_row(
            "SELECT display_name FROM accounts WHERE id = ?",
            params!["acc-1"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "Work (renamed)");
    drop(conn);

    let env_del = fixture_envelope(
        other,
        SyncEvent::AccountDeleted(IdPayload { id: "acc-1".into() }),
        3000,
    );
    let report = applier.apply_envelopes(vec![env_del]).unwrap();
    assert_eq!(report.applied, 1);

    let conn = db.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM accounts WHERE id = ?",
            params!["acc-1"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
