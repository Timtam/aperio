//! Generic user-preferences key/value commands.
//!
//! Three verbs:
//!
//!   - `get_user_pref(key)` — returns the stored value, or `null`
//!     when nothing is set.
//!   - `set_user_pref(key, value)` — upsert.
//!   - `delete_user_pref(key)` — drop the row.
//!
//! Values are opaque strings. The frontend serialises JSON when it
//! needs structure (sidebar tree state, view defaults, …); the
//! command layer doesn't interpret the content.
//!
//! ## Sync integration (Phase Sb)
//!
//! Not every preference is meaningful across devices. Window size,
//! local-only state (e.g. last-opened-view), and adapter
//! credentials all stay on-device. The §19.2.1 whitelist names
//! the ones that DO propagate — `appearance.darkMode`,
//! `view.weekStart`, `locale`, sound configuration, default
//! reminders, etc.
//!
//! When a write or delete lands against a whitelisted key, this
//! module emits a `settings.updated` sync event so the change
//! ripples to the user's other devices once Phase Sd's adapter
//! ships the log entries.

use std::sync::Arc;

use sync_core::{SettingsPayload, SyncEvent};
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::user_prefs::{UserPrefsError, UserPrefsRepo};

/// Whitelist of `user_prefs` keys (or key-prefixes) that
/// participate in cross-device sync per DESIGN.md §19.2.1.
///
/// Prefix matching: an entry ending in `.` matches any key that
/// starts with it. That covers families like `sound.*` or
/// `reminders.defaults.*` without listing every sub-key here.
/// Exact matches are listed without the trailing dot.
///
/// Anything NOT on this list stays device-local. The contact-sync
/// scheduler's own prefs (`contacts.lastSyncedAt`, the
/// `sync.deviceId` itself) are deliberately excluded — they're
/// per-device state, not user preferences.
const SYNC_WHITELIST: &[&str] = &[
    // Appearance + locale (always-sync per §19.2.1).
    "appearance.darkMode",
    "appearance.colorScheme",
    "locale",
    // View defaults (always-sync).
    "view.preferred",
    "view.weekStart",
    "view.weekNumbers",
    // Reminder defaults — one entry per calendar / task list goes
    // under the calendar.<id>.defaultReminders namespace.
    "calendar.",
    "tasks.showCompleted",
    "reminders.defaults.",
    // Sound configuration (Phase 14.4 — container/event/task
    // overrides + the global default). Asset files themselves
    // sync via the SyncAdapter's push_sound_asset path; the
    // user_prefs values reference them by hash.
    "sound.",
    // Snooze options (configurable per §19.2.1).
    "snooze.options",
    // Which sync adapter is active. Note: adapter *credentials*
    // stay local (keychain), only the choice itself syncs.
    "sync.adapter",
    "sync.intervalMinutes",
];

/// Decide whether a key participates in cross-device sync. Both
/// exact matches and prefix matches (entries ending in `.`)
/// count.
fn is_synced_key(key: &str) -> bool {
    SYNC_WHITELIST.iter().any(|pattern| {
        if pattern.ends_with('.') {
            // Prefix-with-trailing-dot pattern: match keys that
            // start with the full pattern AND have at least one
            // more character after it. The bare prefix itself
            // (e.g. "sound." with nothing after) is NOT a valid
            // sub-key so it doesn't match either.
            key.starts_with(*pattern) && key.len() > pattern.len()
        } else {
            *pattern == key
        }
    })
}

#[tauri::command]
pub async fn get_user_pref(
    db: State<'_, DbHandle>,
    key: String,
) -> CommandResult<Option<String>> {
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    Ok(repo.get(&key)?)
}

#[tauri::command]
pub async fn set_user_pref(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    key: String,
    value: String,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    repo.set(&key, &value)?;
    if is_synced_key(&key) {
        // The wire value is JSON. We attempt to parse the stored
        // string as JSON first (the frontend usually writes JSON
        // for structured values); if it isn't valid JSON, fall
        // back to wrapping it as a JSON string. Either way the
        // receiver round-trips it through the same logic.
        let payload_value = serde_json::from_str(&value)
            .unwrap_or_else(|_| serde_json::Value::String(value.clone()));
        event_log.append(SyncEvent::SettingsUpdated(SettingsPayload {
            key: key.clone(),
            value: payload_value,
        }));
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_user_pref(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    key: String,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    repo.delete(&key)?;
    if is_synced_key(&key) {
        // Encode a delete as `settings.updated` with value =
        // null. The applier interprets `null` as "remove the
        // row" so the wire shape stays uniform across set / del.
        event_log.append(SyncEvent::SettingsUpdated(SettingsPayload {
            key: key.clone(),
            value: serde_json::Value::Null,
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_keys_match_the_whitelist() {
        // Exact entries.
        assert!(is_synced_key("appearance.darkMode"));
        assert!(is_synced_key("locale"));
        assert!(is_synced_key("view.weekStart"));
        // Prefix entries.
        assert!(is_synced_key("sound.global"));
        assert!(is_synced_key("sound.calendar.abc.default"));
        assert!(is_synced_key("reminders.defaults.events"));
        assert!(is_synced_key("calendar.foo.defaultReminders"));
        // Not on the list.
        assert!(!is_synced_key("sidebar.expansion"));
        assert!(!is_synced_key("contacts.lastSyncedAt"));
        assert!(!is_synced_key("sync.deviceId"));
        // Prefix entry doesn't match the bare prefix itself.
        assert!(!is_synced_key("sound"));
        assert!(!is_synced_key("sound."));
    }
}

impl From<UserPrefsError> for CommandError {
    fn from(err: UserPrefsError) -> Self {
        match err {
            UserPrefsError::Sqlite(e) => CommandError {
                code: "internal",
                message: e.to_string(),
            },
        }
    }
}
