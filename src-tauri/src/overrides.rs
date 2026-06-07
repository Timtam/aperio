//! Local rename overrides for calendars and task lists.
//!
//! Sits next to `accounts.rs` — same shape: thin repo over the SQLite
//! `container_name_overrides` table, plus a couple of helpers that
//! the command layer uses to apply overrides on top of whatever the
//! adapters return.
//!
//! The data flow on read is:
//!
//!   list_calendars (command)
//!     ↓
//!   local + external adapters return their raw Calendar rows
//!     ↓
//!   apply_overrides() rewrites `.name` for any row that has an
//!   override → frontend sees the renamed value
//!
//! Writes never touch the adapter; they only land in this table.
//! Pushing the rename out to the source server (CalDAV PROPPATCH,
//! local UPDATE) is a follow-up step that goes through a new
//! `rename_calendar` trait method.

use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::db::SharedConn;

/// Which container namespace an override belongs to. Calendars and
/// task-lists have disjoint id namespaces today, but keeping `kind`
/// explicit makes the API self-describing and lets a future check
/// reject "rename calendar X with a task-list id".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerKind {
    Calendar,
    TaskList,
    ContactList,
}

impl ContainerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ContainerKind::Calendar => "calendar",
            ContainerKind::TaskList => "task_list",
            ContainerKind::ContactList => "contact_list",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "calendar" => ContainerKind::Calendar,
            "task_list" => ContainerKind::TaskList,
            "contact_list" => ContainerKind::ContactList,
            _ => return None,
        })
    }
}

#[derive(Debug, Error)]
pub enum OverridesError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("override name must not be empty — DELETE the row to revert")]
    EmptyName,
}

pub struct OverridesRepo<'a> {
    pub(crate) db: &'a SharedConn,
}

impl<'a> OverridesRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// All overrides keyed by `(kind, container_id)`. The frontend
    /// calls this rarely; the command layer joins it onto adapter
    /// output on every `list_calendars` / `list_task_lists` call.
    pub fn list(&self) -> Result<Vec<NameOverride>, OverridesError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT container_id, kind, name, updated_at
               FROM container_name_overrides",
        )?;
        let rows = stmt.query_map([], |row| {
            let kind_str: String = row.get(1)?;
            Ok(NameOverride {
                container_id: row.get(0)?,
                kind: ContainerKind::parse(&kind_str).unwrap_or(ContainerKind::Calendar),
                name: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Upsert one override. Empty `name` is rejected — the caller
    /// must call `clear()` to revert to the source name.
    pub fn set(
        &self,
        container_id: &str,
        kind: ContainerKind,
        name: &str,
    ) -> Result<(), OverridesError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(OverridesError::EmptyName);
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO container_name_overrides (container_id, kind, name, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(container_id, kind) DO UPDATE SET
                 name = excluded.name,
                 updated_at = excluded.updated_at",
            params![container_id, kind.as_str(), trimmed, now],
        )?;
        Ok(())
    }

    /// Drop the override. The next read of the container will use
    /// the source name again.
    pub fn clear(&self, container_id: &str, kind: ContainerKind) -> Result<(), OverridesError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "DELETE FROM container_name_overrides
              WHERE container_id = ? AND kind = ?",
            params![container_id, kind.as_str()],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameOverride {
    pub container_id: String,
    pub kind: ContainerKind,
    pub name: String,
    pub updated_at: String,
}

/// Apply every override that matches one of the given calendars in place.
pub fn apply_to_calendars(repo: &OverridesRepo<'_>, calendars: &mut [cal_core::Calendar]) {
    let overrides = match repo.list() {
        Ok(o) => o,
        Err(err) => {
            tracing::warn!(
                ?err,
                "failed to load name overrides; falling back to source names"
            );
            return;
        }
    };
    let map: std::collections::HashMap<String, &NameOverride> = overrides
        .iter()
        .filter(|o| o.kind == ContainerKind::Calendar)
        .map(|o| (o.container_id.clone(), o))
        .collect();
    for cal in calendars {
        if let Some(o) = map.get(&cal.id) {
            cal.name = o.name.clone();
        }
    }
}

/// Apply every override that matches one of the given task lists in place.
pub fn apply_to_task_lists(repo: &OverridesRepo<'_>, lists: &mut [cal_core::TaskList]) {
    let overrides = match repo.list() {
        Ok(o) => o,
        Err(err) => {
            tracing::warn!(
                ?err,
                "failed to load name overrides; falling back to source names"
            );
            return;
        }
    };
    let map: std::collections::HashMap<String, &NameOverride> = overrides
        .iter()
        .filter(|o| o.kind == ContainerKind::TaskList)
        .map(|o| (o.container_id.clone(), o))
        .collect();
    for list in lists {
        if let Some(o) = map.get(&list.id) {
            list.name = o.name.clone();
        }
    }
}

// ── Color-label overrides (DESIGN §6.5 / §8.2) ──────────────────────────
//
// Same shape as the name overrides, but binding a container's COLOR to a
// global color-label. Local containers store their binding on the row
// (and sync it); these host-local overrides are for EXTERNAL containers,
// where the provider only knows a hex and the user's label binding has
// nowhere else to live.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorOverride {
    pub container_id: String,
    pub kind: ContainerKind,
    pub color_label_id: String,
    pub updated_at: String,
}

impl OverridesRepo<'_> {
    /// All color-label overrides. Joined onto adapter output on every
    /// `list_*` call, alongside the name overrides.
    pub fn list_color_overrides(&self) -> Result<Vec<ColorOverride>, OverridesError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT container_id, kind, color_label_id, updated_at
               FROM container_color_overrides",
        )?;
        let rows = stmt.query_map([], |row| {
            let kind_str: String = row.get(1)?;
            Ok(ColorOverride {
                container_id: row.get(0)?,
                kind: ContainerKind::parse(&kind_str).unwrap_or(ContainerKind::Calendar),
                color_label_id: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Upsert a container's color-label binding.
    pub fn set_color_label(
        &self,
        container_id: &str,
        kind: ContainerKind,
        color_label_id: &str,
    ) -> Result<(), OverridesError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO container_color_overrides
                 (container_id, kind, color_label_id, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(container_id, kind) DO UPDATE SET
                 color_label_id = excluded.color_label_id,
                 updated_at = excluded.updated_at",
            params![container_id, kind.as_str(), color_label_id, now],
        )?;
        Ok(())
    }

    /// Drop the color binding — the container reverts to its provider color.
    pub fn clear_color_label(
        &self,
        container_id: &str,
        kind: ContainerKind,
    ) -> Result<(), OverridesError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "DELETE FROM container_color_overrides
              WHERE container_id = ? AND kind = ?",
            params![container_id, kind.as_str()],
        )?;
        Ok(())
    }
}

/// Build a `container_id → label_id` map for one kind, or `None` if the
/// overrides can't be loaded (degrade to provider colors).
fn color_override_map(
    repo: &OverridesRepo<'_>,
    kind: ContainerKind,
) -> Option<std::collections::HashMap<String, String>> {
    match repo.list_color_overrides() {
        Ok(o) => Some(
            o.into_iter()
                .filter(|c| c.kind == kind)
                .map(|c| (c.container_id, c.color_label_id))
                .collect(),
        ),
        Err(err) => {
            tracing::warn!(
                ?err,
                "failed to load color overrides; using provider colors"
            );
            None
        }
    }
}

/// Stamp color-label bindings onto external calendars in place. Local
/// calendars carry their own (synced) binding and have no override row,
/// so they're left untouched.
pub fn apply_color_to_calendars(repo: &OverridesRepo<'_>, calendars: &mut [cal_core::Calendar]) {
    let Some(map) = color_override_map(repo, ContainerKind::Calendar) else {
        return;
    };
    for cal in calendars {
        if let Some(label) = map.get(&cal.id) {
            cal.color_label = Some(cal_core::ColorLabelId(label.clone()));
        }
    }
}

/// Stamp color-label bindings onto external task lists in place.
pub fn apply_color_to_task_lists(repo: &OverridesRepo<'_>, lists: &mut [cal_core::TaskList]) {
    let Some(map) = color_override_map(repo, ContainerKind::TaskList) else {
        return;
    };
    for list in lists {
        if let Some(label) = map.get(&list.id) {
            list.color_label = Some(cal_core::ColorLabelId(label.clone()));
        }
    }
}

/// Stamp color-label bindings onto external contact lists in place.
pub fn apply_color_to_contact_lists(repo: &OverridesRepo<'_>, lists: &mut [cal_core::ContactList]) {
    let Some(map) = color_override_map(repo, ContainerKind::ContactList) else {
        return;
    };
    for list in lists {
        if let Some(label) = map.get(&list.id) {
            list.color_label = Some(cal_core::ColorLabelId(label.clone()));
        }
    }
}

// ── Section color overrides ─────────────────────────────────────────────
//
// EXTERNAL sections (Todoist sections, Vikunja kanban buckets) have no
// provider color field, so a user's color binding lives here, host-local.
// LOCAL sections carry their (synced) binding on the section row instead
// and never appear here. Sections are a single id namespace, so — unlike
// `ColorOverride` — there's no `kind`.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionColorOverride {
    pub section_id: String,
    pub color_label_id: String,
    pub updated_at: String,
}

impl OverridesRepo<'_> {
    /// All section color-label overrides (merged onto external sections in
    /// `get_sections`).
    pub fn list_section_color_overrides(
        &self,
    ) -> Result<Vec<SectionColorOverride>, OverridesError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT section_id, color_label_id, updated_at
               FROM section_color_overrides",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SectionColorOverride {
                section_id: row.get(0)?,
                color_label_id: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Upsert an external section's color-label binding.
    pub fn set_section_color_label(
        &self,
        section_id: &str,
        color_label_id: &str,
    ) -> Result<(), OverridesError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO section_color_overrides
                 (section_id, color_label_id, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(section_id) DO UPDATE SET
                 color_label_id = excluded.color_label_id,
                 updated_at = excluded.updated_at",
            params![section_id, color_label_id, now],
        )?;
        Ok(())
    }

    /// Drop an external section's color binding (reverts to no color).
    pub fn clear_section_color_label(&self, section_id: &str) -> Result<(), OverridesError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "DELETE FROM section_color_overrides WHERE section_id = ?",
            params![section_id],
        )?;
        Ok(())
    }
}

/// Stamp color-label bindings onto external sections in place. Local
/// sections carry their own (synced) binding and have no override row, so
/// they're left untouched.
pub fn apply_color_to_sections(repo: &OverridesRepo<'_>, sections: &mut [cal_core::Section]) {
    let map: std::collections::HashMap<String, String> = match repo.list_section_color_overrides() {
        Ok(o) => o
            .into_iter()
            .map(|s| (s.section_id, s.color_label_id))
            .collect(),
        Err(err) => {
            tracing::warn!(?err, "failed to load section color overrides; using none");
            return;
        }
    };
    for section in sections {
        if let Some(label) = map.get(&section.id) {
            section.color_label = Some(cal_core::ColorLabelId(label.clone()));
        }
    }
}

// ── Event color overrides ───────────────────────────────────────────────
//
// EXTERNAL events whose calendar can't store a per-event color (iCloud, and
// any provider / account without RFC 7986 COLOR support, plus Graph / EWS)
// keep the user's color binding here, host-local. LOCAL events carry their
// (synced) binding on the event row, and external events on color-capable
// calendars round-trip the color through the provider — neither appears
// here. Keyed by the series master id (the color applies to the whole
// series). Like sections, a single id namespace, so no `kind`.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventColorOverride {
    pub event_id: String,
    pub color_label_id: String,
    pub updated_at: String,
}

impl OverridesRepo<'_> {
    /// All event color-label overrides (merged onto external events in
    /// `get_events`).
    pub fn list_event_color_overrides(&self) -> Result<Vec<EventColorOverride>, OverridesError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT event_id, color_label_id, updated_at
               FROM event_color_overrides",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(EventColorOverride {
                event_id: row.get(0)?,
                color_label_id: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Upsert an external event's color-label binding (host-local).
    pub fn set_event_color_label(
        &self,
        event_id: &str,
        color_label_id: &str,
    ) -> Result<(), OverridesError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO event_color_overrides
                 (event_id, color_label_id, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(event_id) DO UPDATE SET
                 color_label_id = excluded.color_label_id,
                 updated_at = excluded.updated_at",
            params![event_id, color_label_id, now],
        )?;
        Ok(())
    }

    /// Drop an external event's color binding (reverts to no color).
    pub fn clear_event_color_label(&self, event_id: &str) -> Result<(), OverridesError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "DELETE FROM event_color_overrides WHERE event_id = ?",
            params![event_id],
        )?;
        Ok(())
    }
}

/// Stamp color-label bindings onto external events in place. Local events and
/// events on color-capable calendars carry their own binding and have no
/// override row, so they're left untouched.
pub fn apply_color_to_events(repo: &OverridesRepo<'_>, events: &mut [cal_core::Event]) {
    let map: std::collections::HashMap<String, String> = match repo.list_event_color_overrides() {
        Ok(o) => o
            .into_iter()
            .map(|e| (e.event_id, e.color_label_id))
            .collect(),
        Err(err) => {
            tracing::warn!(?err, "failed to load event color overrides; using none");
            return;
        }
    };
    for event in events {
        if let Some(label) = map.get(&event.id) {
            event.color_label = Some(cal_core::ColorLabelId(label.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.sqlite");
        let db = DbHandle::open(&path).unwrap();
        (dir, db)
    }

    #[test]
    fn set_then_list_roundtrips() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = OverridesRepo::new(&shared);
        repo.set("ical:abc", ContainerKind::Calendar, "Schulferien")
            .unwrap();
        let all = repo.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].container_id, "ical:abc");
        assert_eq!(all[0].kind, ContainerKind::Calendar);
        assert_eq!(all[0].name, "Schulferien");
    }

    #[test]
    fn set_is_upsert() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = OverridesRepo::new(&shared);
        repo.set("c1", ContainerKind::Calendar, "First").unwrap();
        repo.set("c1", ContainerKind::Calendar, "Second").unwrap();
        let all = repo.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Second");
    }

    #[test]
    fn calendar_and_task_list_with_same_id_coexist() {
        // Disjoint namespaces today but the PK includes `kind`, so
        // even if a freak collision happened the two rows wouldn't
        // overwrite each other.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = OverridesRepo::new(&shared);
        repo.set("x", ContainerKind::Calendar, "A").unwrap();
        repo.set("x", ContainerKind::TaskList, "B").unwrap();
        let all = repo.list().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn empty_name_is_rejected() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = OverridesRepo::new(&shared);
        let err = repo.set("c1", ContainerKind::Calendar, "  ").unwrap_err();
        assert!(matches!(err, OverridesError::EmptyName));
    }

    #[test]
    fn clear_removes_the_row() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = OverridesRepo::new(&shared);
        repo.set("c1", ContainerKind::Calendar, "Nope").unwrap();
        repo.clear("c1", ContainerKind::Calendar).unwrap();
        assert!(repo.list().unwrap().is_empty());
    }

    #[test]
    fn apply_to_calendars_overwrites_matching_names() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = OverridesRepo::new(&shared);
        repo.set("ical:42", ContainerKind::Calendar, "Ferien")
            .unwrap();

        let mut cals = vec![
            cal_core::Calendar {
                color_label: None,
                supports_scheduling: false,
                id: "ical:42".into(),
                name: "schulferien-sachsen-anhalt".into(),
                color: None,
                read_only: true,
                default_sound: None,
            },
            cal_core::Calendar {
                color_label: None,
                supports_scheduling: false,
                id: "local-1".into(),
                name: "Persönlich".into(),
                color: None,
                read_only: false,
                default_sound: None,
            },
        ];
        apply_to_calendars(&repo, &mut cals);
        assert_eq!(cals[0].name, "Ferien");
        // Unchanged — no override registered.
        assert_eq!(cals[1].name, "Persönlich");
    }

    #[test]
    fn color_override_roundtrips_and_applies() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        // The override FKs onto color_labels — seed the label it binds to.
        shared
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO color_labels (id, name, hex) VALUES ('label-1', 'Work', '#4285f4')",
                [],
            )
            .unwrap();
        let repo = OverridesRepo::new(&shared);
        repo.set_color_label("google:work", ContainerKind::Calendar, "label-1")
            .unwrap();
        // Upsert + list.
        let all = repo.list_color_overrides().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].color_label_id, "label-1");

        // Apply stamps the binding onto the matching external calendar.
        let mut cals = vec![
            cal_core::Calendar {
                color_label: None,
                supports_scheduling: false,
                id: "google:work".into(),
                name: "Work".into(),
                color: Some(cal_core::ContainerColor::native("#4285f4")),
                read_only: false,
                default_sound: None,
            },
            cal_core::Calendar {
                color_label: None,
                supports_scheduling: false,
                id: "google:other".into(),
                name: "Other".into(),
                color: None,
                read_only: false,
                default_sound: None,
            },
        ];
        apply_color_to_calendars(&repo, &mut cals);
        assert_eq!(
            cals[0].color_label.as_ref().map(|c| c.as_str()),
            Some("label-1"),
        );
        // Untouched — no override.
        assert!(cals[1].color_label.is_none());

        // Clear reverts.
        repo.clear_color_label("google:work", ContainerKind::Calendar)
            .unwrap();
        assert!(repo.list_color_overrides().unwrap().is_empty());
    }

    #[test]
    fn section_color_override_roundtrips_and_applies() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        shared
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO color_labels (id, name, hex) VALUES ('label-2', 'Doing', '#34a853')",
                [],
            )
            .unwrap();
        let repo = OverridesRepo::new(&shared);
        repo.set_section_color_label("todoist:sec-1", "label-2")
            .unwrap();
        let all = repo.list_section_color_overrides().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].color_label_id, "label-2");

        // Apply stamps the binding onto the matching external section only.
        let mut sections = vec![
            cal_core::Section {
                id: "todoist:sec-1".into(),
                list_id: "todoist:p1".into(),
                name: "Doing".into(),
                color_label: None,
                order: 0,
            },
            cal_core::Section {
                id: "todoist:sec-2".into(),
                list_id: "todoist:p1".into(),
                name: "Done".into(),
                color_label: None,
                order: 1,
            },
        ];
        apply_color_to_sections(&repo, &mut sections);
        assert_eq!(
            sections[0].color_label.as_ref().map(|c| c.as_str()),
            Some("label-2"),
        );
        assert!(sections[1].color_label.is_none());

        // Clear reverts.
        repo.clear_section_color_label("todoist:sec-1").unwrap();
        assert!(repo.list_section_color_overrides().unwrap().is_empty());
    }

    #[test]
    fn event_color_override_roundtrips_and_applies() {
        use chrono::{TimeZone, Utc};
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        shared
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO color_labels (id, name, hex) VALUES ('label-3', 'Travel', '#fb8c00')",
                [],
            )
            .unwrap();
        let repo = OverridesRepo::new(&shared);
        repo.set_event_color_label("icloud:evt-1", "label-3")
            .unwrap();
        let all = repo.list_event_color_overrides().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].color_label_id, "label-3");

        let mk = |id: &str| cal_core::Event {
            id: id.into(),
            calendar_id: "icloud:cal".into(),
            title: "Trip".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            created_at: Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap(),
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
        };
        // Apply stamps the binding onto the matching external event only.
        let mut events = vec![mk("icloud:evt-1"), mk("icloud:evt-2")];
        apply_color_to_events(&repo, &mut events);
        assert_eq!(
            events[0].color_label.as_ref().map(|c| c.as_str()),
            Some("label-3"),
        );
        assert!(events[1].color_label.is_none());

        // Clear reverts.
        repo.clear_event_color_label("icloud:evt-1").unwrap();
        assert!(repo.list_event_color_overrides().unwrap().is_empty());
    }
}
