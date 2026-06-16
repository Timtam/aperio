//! Settings sync whitelist (DESIGN.md §19.2.1).
//!
//! Lifted out of `commands::user_prefs` so both the command layer
//! (which gates writes) and the snapshot builder (which dumps the
//! current values) reference the same single source of truth. It now
//! lives in `sync-engine` because the snapshot builder and applier do,
//! and the desktop command layer reaches it through the re-export in
//! `event_log::whitelist`.
//!
//! Anything NOT on this list stays device-local. The scheduler's own
//! per-device state (`contacts.lastSyncedAt`, `sync.deviceId`,
//! `sync.adapter.*`, `sync.cursor.*`, `sync.compaction.*`) is
//! deliberately excluded — those are per-device, not preferences.

/// Per-key sync whitelist. Entries ending in `.` are prefix patterns
/// matching any key that starts with them; bare entries are exact
/// matches.
pub const SYNC_WHITELIST: &[&str] = &[
    // Appearance + locale (always-sync per §19.2.1).
    "appearance.darkMode",
    "appearance.colorScheme",
    "locale",
    // View defaults (always-sync).
    "view.preferred",
    "view.weekStart",
    "view.weekNumbers",
    // Backlog column width in the week/month planner — a layout choice the
    // user wants kept consistent across devices.
    "backlog.width",
    // Reminder defaults — one entry per calendar / task list goes
    // under the calendar.<id>.defaultReminders namespace.
    "calendar.",
    // All task settings follow the user across devices — they're user
    // preferences, not device state. Covers the display toggle
    // (`tasks.showCompleted`) plus every behaviour knob from the Tasks
    // settings tab: `tasks.cascadeStatusCoupling`, `tasks.autoDateOnStart`,
    // `tasks.carryOverDefault`, `tasks.dayStartTrigger`, `tasks.checkoffMode`
    // and the per-list overrides (`tasks.listOverrides`, keyed by synced
    // list ids). The `aperio.tasks.*` UI state lives in localStorage, not
    // user_prefs, so it isn't affected by this prefix.
    "tasks.",
    "reminders.defaults.",
    // Sound configuration (Phase 14.4 — container/event/task
    // overrides + the global default). Asset files themselves
    // sync via the SyncAdapter's push_sound_asset path; the
    // user_prefs values reference them by hash.
    "sound.",
    // Snooze options (configurable per §19.2.1).
    "snooze.options",
    // Window behaviour: close/minimize-to-tray. Synced so the choice
    // follows the user; a device without a usable tray simply ignores the
    // value (the toggle is gated on tray availability). Exact keys, not a
    // `window.` prefix, so any future device-local window state stays local.
    "window.closeToTray",
    "window.minimizeToTray",
    // Which sync adapter is active. Note: adapter *credentials*
    // stay local (keychain), only the choice itself syncs.
    "sync.adapter",
    "sync.intervalMinutes",
];

/// Decide whether a key participates in cross-device sync. Both
/// exact matches and prefix matches (entries ending in `.`)
/// count.
pub fn is_synced_key(key: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_matches_are_recognised() {
        assert!(is_synced_key("locale"));
        assert!(is_synced_key("appearance.darkMode"));
        assert!(is_synced_key("snooze.options"));
    }

    #[test]
    fn window_tray_prefs_sync() {
        assert!(is_synced_key("window.closeToTray"));
        assert!(is_synced_key("window.minimizeToTray"));
    }

    #[test]
    fn backlog_width_syncs() {
        assert!(is_synced_key("backlog.width"));
    }

    #[test]
    fn task_settings_sync() {
        // The whole `tasks.` namespace follows the user — display toggle,
        // behaviour knobs, and per-list overrides.
        assert!(is_synced_key("tasks.showCompleted"));
        assert!(is_synced_key("tasks.cascadeStatusCoupling"));
        assert!(is_synced_key("tasks.autoDateOnStart"));
        assert!(is_synced_key("tasks.carryOverDefault"));
        assert!(is_synced_key("tasks.dayStartTrigger"));
        assert!(is_synced_key("tasks.checkoffMode"));
        assert!(is_synced_key("tasks.listOverrides"));
        // Bare prefix isn't a real key and must not match.
        assert!(!is_synced_key("tasks."));
    }

    #[test]
    fn prefix_patterns_require_a_suffix() {
        // "sound." matches any sub-key but NOT the bare "sound."
        // string (which isn't a valid setting anyway).
        assert!(is_synced_key("sound.default"));
        assert!(is_synced_key("sound.event.event-id"));
        assert!(!is_synced_key("sound."));
        assert!(!is_synced_key("sound"));
    }

    #[test]
    fn unrelated_keys_are_rejected() {
        assert!(!is_synced_key("sync.deviceId"));
        assert!(!is_synced_key("sync.cursor.lastSeenLog"));
        assert!(!is_synced_key("contacts.lastSyncedAt"));
        assert!(!is_synced_key("foo.bar.baz"));
    }
}
