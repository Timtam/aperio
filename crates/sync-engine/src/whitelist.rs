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
    // On app launch, seed every view to today instead of restoring the
    // last-opened day (default off — restore).
    "view.startOnToday",
    // Show cancelled events (RFC 5545 STATUS:CANCELLED / EWS IsCancelled /
    // Graph isCancelled) in the calendar, or hide them (default on — show, for
    // Outlook consistency). Reminders for cancelled events are always
    // suppressed regardless of this toggle.
    "view.showCancelledEvents",
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
    // How often to sync. The only sync key that crosses devices — which
    // adapter a device uses, and where it points, is that device's own
    // business and must stay off this list. See the module doc.
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
    fn view_defaults_sync() {
        assert!(is_synced_key("view.weekStart"));
        assert!(is_synced_key("view.startOnToday"));
        assert!(is_synced_key("view.showCancelledEvents"));
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

    /// Where a device syncs TO is that device's own business, and every key
    /// that expresses it must stay off the list.
    ///
    /// This is a trap with history. The list used to carry a bare
    /// `"sync.adapter"` entry, commented "only the choice itself syncs" — but
    /// entries without a trailing dot match exactly, and the key actually
    /// written is `sync.adapter.kind`, so it never matched and the choice never
    /// synced. The comment described an intention the code did not implement.
    ///
    /// Had anyone "fixed" it by adding the dot, every device would have been
    /// dragged to one target: one machine's SFTP host, another machine's local
    /// folder path. These assertions exist so that edit fails here instead.
    #[test]
    fn no_key_that_names_this_devices_sync_target_ever_syncs() {
        for key in [
            "sync.adapter",
            "sync.adapter.kind",
            "sync.adapter.webdav.url",
            "sync.adapter.sftp.host",
            "sync.adapter.sftp.keyPath",
            "sync.adapter.local.path",
            "sync.adapter.e2eEnabled",
            "sync.target.accountId",
        ] {
            assert!(
                !is_synced_key(key),
                "{key} must stay device-local — see this test's doc comment",
            );
        }
    }
}
