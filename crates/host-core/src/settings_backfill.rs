//! One-time re-emit of settings that became syncable after they were written.
//!
//! A `user_prefs` key only reaches the sync log when it is on the
//! `SYNC_WHITELIST` at the moment it is WRITTEN. Widening the whitelist later
//! therefore leaves every existing value stranded on the device that holds it:
//! the setting is right there in its table, the other devices never hear of
//! it, and nothing re-emits it until the user happens to edit it again. The
//! signature list was the case that made this visible — written on the
//! desktop, bound to calendars whose bindings DID travel, and absent on the
//! phone, which then pointed at signature ids it had never seen.
//!
//! The mechanism is the credential backfill's, applied to settings: a
//! versioned list of keys that joined the whitelist, and a per-device marker
//! of the generation already pushed. Bumping [`SETTINGS_BACKFILL_VERSION`] and
//! adding the keys makes every device emit its local values exactly once.
//!
//! What this can and cannot promise, because the receiver applies a
//! `settings.updated` as a plain replace (last one applied wins):
//!
//!   - An EMPTY value is never re-emitted. A device that holds `[]` for the
//!     list has nothing to contribute, and pushing it would wipe the other
//!     device's real list on arrival.
//!   - Two devices that each hold a different NON-empty value both push it;
//!     whichever arrives last at a given device is what it keeps. That is a
//!     swap, not a merge — the price of a whole-list setting, and the same
//!     rule every live edit of it already obeys. Both values stay in the log.
//!   - The append is fire-and-forget (the writer drains asynchronously and
//!     warns rather than fails), so the marker is advanced once the LOCAL
//!     reads succeeded; a writer that cannot open its session file on that one
//!     launch loses the re-emit until the user edits the value. Same property
//!     as the credential backfill.

use sync_core::{SettingsPayload, SyncEvent};
use sync_engine::{whitelist::is_synced_key, EventLogWriter};

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// Device-local marker: the settings-backfill generation already pushed.
const SETTINGS_BACKFILL_PREF: &str = "settingsSync.backfillVersion";

/// The current generation. 1 = `signatures.list` joined the whitelist
/// (2026-08); the bindings had been syncing without it.
const SETTINGS_BACKFILL_VERSION: i64 = 1;

/// Keys that joined the whitelist, by the generation that added them. A key
/// is re-emitted by every device whose recorded generation is older.
const NEWLY_SYNCED_KEYS: &[(i64, &str)] = &[(1, "signatures.list")];

/// Re-emit every local value of a newly whitelisted key, once per generation.
/// Best-effort and silent: a failed read skips that key (the next launch
/// tries again, because the marker is only advanced afterwards), a failed
/// marker write costs one redundant re-emit next launch.
pub fn backfill_newly_synced_settings(event_log: &EventLogWriter, conn: &SharedConn) {
    backfill_with(
        event_log,
        conn,
        NEWLY_SYNCED_KEYS,
        SETTINGS_BACKFILL_VERSION,
    );
}

/// The table-driven core, so the tests can feed their own generations.
fn backfill_with(
    event_log: &EventLogWriter,
    conn: &SharedConn,
    table: &[(i64, &str)],
    version: i64,
) {
    let prefs = UserPrefsRepo::new(conn);
    let done = prefs
        .get(SETTINGS_BACKFILL_PREF)
        .ok()
        .flatten()
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(0);
    if done >= version {
        return;
    }
    let mut all_read = true;
    for (generation, key) in table {
        if *generation <= done {
            continue;
        }
        // The key must actually be on the whitelist NOW — a stale entry in
        // this table must never push a device-local value onto the wire.
        if !is_synced_key(key) {
            tracing::warn!(key, "settings backfill: key is not whitelisted, skipping");
            continue;
        }
        match prefs.get(key) {
            Ok(Some(value)) => {
                // Same wire shape as the live write path: the stored string
                // parsed as JSON, else wrapped as a JSON string.
                let payload_value = serde_json::from_str(&value)
                    .unwrap_or_else(|_| serde_json::Value::String(value.clone()));
                if is_empty_value(&payload_value) {
                    // Nothing to contribute — and a replace with nothing would
                    // erase what another device wrote.
                    tracing::info!(
                        key,
                        "settings backfill: local value is empty, not re-emitted"
                    );
                    continue;
                }
                event_log.append(SyncEvent::SettingsUpdated(SettingsPayload {
                    key: (*key).to_string(),
                    value: payload_value,
                }));
                tracing::info!(key, "settings backfill: re-emitted a newly synced setting");
            }
            Ok(None) => {}
            Err(err) => {
                all_read = false;
                tracing::warn!(
                    ?err,
                    key,
                    "settings backfill: read failed, will retry next launch"
                );
            }
        }
    }
    if !all_read {
        return;
    }
    if let Err(err) = prefs.set(SETTINGS_BACKFILL_PREF, &version.to_string()) {
        tracing::warn!(?err, "settings backfill: couldn't record the version");
    }
}

/// An empty list / object / string: a value with nothing in it.
fn is_empty_value(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.is_empty(),
        serde_json::Value::Object(map) => map.is_empty(),
        serde_json::Value::String(s) => s.trim().is_empty(),
        serde_json::Value::Null => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use std::sync::Arc;
    use sync_core::{DeviceId, IdPayload};
    use tempfile::TempDir;

    /// Appended last by [`flush_and_read`] — the writer drains asynchronously,
    /// so "the backfill emitted nothing" is only assertable once a LATER
    /// append is known to be on disk.
    const SENTINEL: &str = "sentinel-after-the-backfill";

    fn device() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(dir.path().join("test.sqlite")).unwrap();
        (dir, db)
    }

    fn writer(tmp: &TempDir, id: &str) -> Arc<EventLogWriter> {
        EventLogWriter::spawn(tmp.path().to_path_buf(), DeviceId::from_string(id.into()))
    }

    fn marker(db: &DbHandle) -> Option<String> {
        UserPrefsRepo::new(&db.shared())
            .get(SETTINGS_BACKFILL_PREF)
            .unwrap()
    }

    async fn flush_and_read(tmp: &TempDir, writer: Arc<EventLogWriter>) -> String {
        writer.append(SyncEvent::EventDeleted(IdPayload {
            id: SENTINEL.to_string(),
        }));
        drop(writer);
        let pending = tmp.path().join("sync").join("log").join("pending");
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let Ok(mut entries) = tokio::fs::read_dir(&pending).await else {
                continue;
            };
            let Ok(Some(entry)) = entries.next_entry().await else {
                continue;
            };
            let Ok(bytes) = tokio::fs::read(entry.path()).await else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if text.contains(SENTINEL) {
                return text;
            }
        }
        panic!("the event-log writer never flushed the sentinel within 2 s");
    }

    #[tokio::test]
    async fn an_existing_signature_list_is_emitted_once() {
        let (tmp, db) = device();
        let shared = db.shared();
        let list = r#"[{"id":"s1","name":"Raum 12","body":"Zugang: 4711"}]"#;
        UserPrefsRepo::new(&shared)
            .set("signatures.list", list)
            .unwrap();
        let writer = writer(&tmp, "dev-backfill");

        backfill_newly_synced_settings(&writer, &shared);
        // The marker is advanced, so a second launch re-emits nothing.
        assert_eq!(marker(&db).as_deref(), Some("1"));
        backfill_newly_synced_settings(&writer, &shared);

        let text = flush_and_read(&tmp, writer).await;
        assert!(text.contains("settings.updated"), "got: {text}");
        assert!(text.contains("signatures.list"), "got: {text}");
        assert!(text.contains("Raum 12"), "the list itself travels: {text}");
        assert_eq!(
            text.matches("signatures.list").count(),
            1,
            "emitted exactly once across two launches: {text}",
        );
    }

    #[tokio::test]
    async fn a_device_without_signatures_emits_nothing_but_still_records_the_generation() {
        let (tmp, db) = device();
        let shared = db.shared();
        let writer = writer(&tmp, "dev-empty");

        backfill_newly_synced_settings(&writer, &shared);

        assert_eq!(marker(&db).as_deref(), Some("1"));
        let text = flush_and_read(&tmp, writer).await;
        assert!(!text.contains("settings.updated"), "got: {text}");
    }

    #[tokio::test]
    async fn an_empty_list_is_not_re_emitted() {
        // A device that deleted its last signature holds "[]". Pushing that
        // would replace — erase — the other device's real list on arrival.
        let (tmp, db) = device();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set("signatures.list", "[]")
            .unwrap();
        let writer = writer(&tmp, "dev-emptied");

        backfill_newly_synced_settings(&writer, &shared);

        assert_eq!(marker(&db).as_deref(), Some("1"));
        let text = flush_and_read(&tmp, writer).await;
        assert!(!text.contains("settings.updated"), "got: {text}");
    }

    #[tokio::test]
    async fn only_the_generations_after_the_recorded_one_are_re_emitted() {
        // A device that already ran generation 1 must not push generation 1's
        // key again when generation 2 arrives — only the newcomer.
        let (tmp, db) = device();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        prefs
            .set("signatures.list", r#"[{"id":"s1","name":"A","body":"x"}]"#)
            .unwrap();
        prefs.set("snooze.options", "[5,10]").unwrap();
        prefs.set(SETTINGS_BACKFILL_PREF, "1").unwrap();
        let writer = writer(&tmp, "dev-gen2");

        backfill_with(
            &writer,
            &shared,
            &[(1, "signatures.list"), (2, "snooze.options")],
            2,
        );

        assert_eq!(marker(&db).as_deref(), Some("2"));
        let text = flush_and_read(&tmp, writer).await;
        assert!(text.contains("snooze.options"), "got: {text}");
        assert!(!text.contains("signatures.list"), "got: {text}");
    }

    #[test]
    fn empty_values_are_recognised() {
        assert!(is_empty_value(&serde_json::json!([])));
        assert!(is_empty_value(&serde_json::json!({})));
        assert!(is_empty_value(&serde_json::json!("  ")));
        assert!(is_empty_value(&serde_json::Value::Null));
        assert!(!is_empty_value(&serde_json::json!([1])));
        assert!(!is_empty_value(&serde_json::json!("x")));
        assert!(!is_empty_value(&serde_json::json!(0)));
        assert!(!is_empty_value(&serde_json::json!(false)));
    }
}
