//! Notification-sound resolution (DESIGN.md §14.4).
//!
//! Aperio resolves the effective [`SoundConfig`] for a reminder
//! occurrence from a four-level hierarchy. All "override" levels live
//! in `user_prefs` (prefix `sound.`, already on the event-log sync
//! whitelist), so the same mechanism works for local AND external
//! calendars/items and survives a cache refresh:
//!
//! ```text
//! reminder.sound                       (Reminder.sound, per alarm)
//!   ?? prefs["sound.item.{itemId}"]    (per event / task override)
//!   ?? prefs["sound.{calendar|tasklist}.{containerId}"]  (container)
//!   ?? prefs["sound.global"]           (global default)
//!   ?? System                          (SoundConfig::default())
//! ```
//!
//! The scheduler loads a [`SoundPrefs`] snapshot ONCE per scan and then
//! resolves purely in memory. That's not just an optimisation: the
//! local-trigger scan holds the DB mutex for its whole pass, and
//! `std::sync::Mutex` isn't reentrant — re-locking it mid-scan to read
//! a pref would deadlock. Loading the snapshot before the scan locks
//! the connection sidesteps that entirely.

use std::collections::HashMap;

use cal_core::SoundConfig;
use rusqlite::params;
use tracing::warn;

use crate::db::SharedConn;

/// Which container a reminder's item belongs to — selects the
/// `sound.calendar.{id}` vs `sound.tasklist.{id}` pref namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Calendar,
    TaskList,
}

/// In-memory snapshot of every `sound.*` user pref. Built once per
/// scheduler scan via [`SoundPrefs::load`]; resolution is then pure
/// map lookups (see module docs for why we don't read the DB lazily).
#[derive(Debug, Default, Clone)]
pub struct SoundPrefs {
    /// `sound.global` — the global default. `None` falls through to
    /// `SoundConfig::default()` (System).
    global: Option<SoundConfig>,
    by_calendar: HashMap<String, SoundConfig>,
    by_tasklist: HashMap<String, SoundConfig>,
    by_item: HashMap<String, SoundConfig>,
}

impl SoundPrefs {
    /// Read every `sound.%` row from `user_prefs` into the snapshot.
    /// Unparseable or unknown-shaped values are skipped — resolution
    /// just falls through to the next level for those. A poisoned
    /// mutex or query error yields an empty snapshot (everything
    /// resolves to System), never a panic on the scheduler thread.
    pub fn load(db: &SharedConn) -> Self {
        let conn = match db.lock() {
            Ok(c) => c,
            Err(err) => {
                warn!(?err, "sound prefs DB mutex poisoned; using empty snapshot");
                return Self::default();
            }
        };
        let mut stmt =
            match conn.prepare("SELECT key, value FROM user_prefs WHERE key LIKE 'sound.%'") {
                Ok(s) => s,
                Err(err) => {
                    warn!(?err, "couldn't prepare sound prefs query");
                    return Self::default();
                }
            };
        let rows = match stmt.query_map(params![], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            Ok(r) => r,
            Err(err) => {
                warn!(?err, "couldn't query sound prefs");
                return Self::default();
            }
        };
        let mut out = Self::default();
        for (key, value) in rows.flatten() {
            let Ok(cfg) = serde_json::from_str::<SoundConfig>(&value) else {
                continue;
            };
            out.insert_key(&key, cfg);
        }
        out
    }

    /// Route one `sound.*` key into the right bucket. Unknown
    /// sub-namespaces are ignored.
    fn insert_key(&mut self, key: &str, cfg: SoundConfig) {
        if key == "sound.global" {
            self.global = Some(cfg);
        } else if let Some(id) = key.strip_prefix("sound.calendar.") {
            self.by_calendar.insert(id.to_string(), cfg);
        } else if let Some(id) = key.strip_prefix("sound.tasklist.") {
            self.by_tasklist.insert(id.to_string(), cfg);
        } else if let Some(id) = key.strip_prefix("sound.item.") {
            self.by_item.insert(id.to_string(), cfg);
        }
    }

    /// Resolve the effective sound for a single reminder occurrence,
    /// honouring the full precedence chain (see module docs).
    pub fn resolve(
        &self,
        reminder_sound: Option<&SoundConfig>,
        item_id: &str,
        container_kind: ContainerKind,
        container_id: &str,
    ) -> SoundConfig {
        if let Some(s) = reminder_sound {
            return s.clone();
        }
        self.item_fallback(item_id, container_kind, container_id)
    }

    /// The sound an item resolves to BEFORE any per-reminder override —
    /// item ?? container ?? global ?? System.
    pub fn item_fallback(
        &self,
        item_id: &str,
        container_kind: ContainerKind,
        container_id: &str,
    ) -> SoundConfig {
        if let Some(s) = self.by_item.get(item_id) {
            return s.clone();
        }
        let by_container = match container_kind {
            ContainerKind::Calendar => self.by_calendar.get(container_id),
            ContainerKind::TaskList => self.by_tasklist.get(container_id),
        };
        if let Some(s) = by_container {
            return s.clone();
        }
        self.global.clone().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::SoundSource;

    fn custom(tag: &str) -> SoundConfig {
        SoundConfig {
            source: SoundSource::Custom {
                sha256: tag.to_string(),
            },
            volume: 80,
        }
    }

    fn silent() -> SoundConfig {
        SoundConfig {
            source: SoundSource::Silent,
            volume: 80,
        }
    }

    #[test]
    fn empty_snapshot_resolves_to_system() {
        let prefs = SoundPrefs::default();
        let got = prefs.resolve(None, "ev-1", ContainerKind::Calendar, "cal-1");
        assert_eq!(got, SoundConfig::default());
        assert_eq!(got.source, SoundSource::System);
    }

    #[test]
    fn reminder_override_beats_everything() {
        let mut prefs = SoundPrefs::default();
        prefs.insert_key("sound.global", custom("global"));
        prefs.insert_key("sound.calendar.cal-1", custom("cal"));
        prefs.insert_key("sound.item.ev-1", custom("item"));
        let reminder = silent();
        let got = prefs.resolve(Some(&reminder), "ev-1", ContainerKind::Calendar, "cal-1");
        assert_eq!(got, silent());
    }

    #[test]
    fn item_override_beats_container_and_global() {
        let mut prefs = SoundPrefs::default();
        prefs.insert_key("sound.global", custom("global"));
        prefs.insert_key("sound.calendar.cal-1", custom("cal"));
        prefs.insert_key("sound.item.ev-1", custom("item"));
        let got = prefs.resolve(None, "ev-1", ContainerKind::Calendar, "cal-1");
        assert_eq!(got, custom("item"));
    }

    #[test]
    fn container_override_beats_global() {
        let mut prefs = SoundPrefs::default();
        prefs.insert_key("sound.global", custom("global"));
        prefs.insert_key("sound.calendar.cal-1", custom("cal"));
        let got = prefs.resolve(None, "ev-1", ContainerKind::Calendar, "cal-1");
        assert_eq!(got, custom("cal"));
    }

    #[test]
    fn calendar_and_tasklist_namespaces_are_distinct() {
        let mut prefs = SoundPrefs::default();
        // Same id under both kinds — must not cross over.
        prefs.insert_key("sound.calendar.shared-id", custom("cal"));
        prefs.insert_key("sound.tasklist.shared-id", custom("list"));
        assert_eq!(
            prefs.resolve(None, "x", ContainerKind::Calendar, "shared-id"),
            custom("cal"),
        );
        assert_eq!(
            prefs.resolve(None, "x", ContainerKind::TaskList, "shared-id"),
            custom("list"),
        );
    }

    #[test]
    fn global_used_when_no_more_specific_level() {
        let mut prefs = SoundPrefs::default();
        prefs.insert_key("sound.global", custom("global"));
        let got = prefs.resolve(None, "ev-1", ContainerKind::TaskList, "list-1");
        assert_eq!(got, custom("global"));
    }
}
