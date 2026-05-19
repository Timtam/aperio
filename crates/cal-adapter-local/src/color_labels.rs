//! Color-label management against the local SQLite store.
//!
//! Labels are app-level metadata — they don't fit any adapter feature
//! trait, but the local adapter is the canonical owner of the
//! `color_labels` table. Inherent methods on [`LocalAdapter`] match the
//! style used for local-only calendar/task management (see
//! `calendars.rs` and `tasks.rs`).

use cal_core::{ColorLabel, ColorLabelId};
use rusqlite::params;
use uuid::Uuid;

use crate::mapping::req_text;
use crate::{map_sql_err, LocalAdapter};

impl LocalAdapter {
    pub fn list_color_labels(&self) -> cal_core::Result<Vec<ColorLabel>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT id, name, hex FROM color_labels ORDER BY name COLLATE NOCASE")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((req_text(row, 0), req_text(row, 1), req_text(row, 2)))
            })
            .map_err(map_sql_err)?;

        let mut out = Vec::new();
        for r in rows {
            let (id, name, hex) = r.map_err(map_sql_err)?;
            out.push(ColorLabel {
                id: ColorLabelId::new(id?),
                name: name?,
                hex: hex?,
            });
        }
        Ok(out)
    }

    pub fn create_color_label(&self, name: &str, hex: &str) -> cal_core::Result<ColorLabel> {
        let id = Uuid::new_v4().to_string();
        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO color_labels (id, name, hex) VALUES (?, ?, ?)",
                params![id, name, hex],
            )
            .map_err(map_sql_err)?;
        Ok(ColorLabel {
            id: ColorLabelId::new(id),
            name: name.to_string(),
            hex: hex.to_string(),
        })
    }

    pub fn update_color_label(&self, label: ColorLabel) -> cal_core::Result<ColorLabel> {
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE color_labels SET name = ?, hex = ? WHERE id = ?",
                params![label.name, label.hex, label.id.as_str()],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "color label '{}' not found",
                label.id.as_str()
            )));
        }
        Ok(label)
    }

    pub fn delete_color_label(&self, id: &str) -> cal_core::Result<()> {
        // The schema declares ON DELETE SET NULL on events.color_label_id
        // and tasks.color_label_id, so removing a label drops it from
        // every item without losing the items themselves.
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM color_labels WHERE id = ?", params![id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "color label '{id}' not found"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_test_db;

    fn adapter() -> LocalAdapter {
        LocalAdapter::new(open_test_db())
    }

    #[test]
    fn create_and_list() {
        let a = adapter();
        let l = a.create_color_label("Work", "#e53935").unwrap();
        let all = a.list_color_labels().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, l.id);
        assert_eq!(all[0].name, "Work");
        assert_eq!(all[0].hex, "#e53935");
    }

    #[test]
    fn rename_label() {
        let a = adapter();
        let mut l = a.create_color_label("Work", "#e53935").unwrap();
        l.name = "Office".into();
        a.update_color_label(l.clone()).unwrap();
        let all = a.list_color_labels().unwrap();
        assert_eq!(all[0].name, "Office");
    }

    #[test]
    fn delete_label_clears_reference_on_events() {
        let a = adapter();
        // Set up a calendar + event referring to the label.
        let cal = a.create_calendar("Work", None, None).unwrap();
        let label = a.create_color_label("Urgent", "#fb8c00").unwrap();

        // Create event referencing the label.
        let conn = a.db().lock().unwrap();
        let now = "2026-05-19T10:00:00.000+00:00";
        conn.execute(
            "INSERT INTO events (id, calendar_id, title, start_utc, end_utc, color_label_id, created_at, updated_at)
             VALUES ('e1', ?, 'X', ?, ?, ?, ?, ?)",
            params![cal.id, now, now, label.id.as_str(), now, now],
        ).unwrap();
        drop(conn);

        a.delete_color_label(label.id.as_str()).unwrap();

        // Event still exists, but its color_label_id is now NULL.
        let conn = a.db().lock().unwrap();
        let lbl: Option<String> = conn
            .query_row(
                "SELECT color_label_id FROM events WHERE id = 'e1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(lbl.is_none());
    }

    #[test]
    fn delete_returns_not_found_when_missing() {
        let a = adapter();
        let err = a.delete_color_label("nope").unwrap_err();
        assert!(matches!(err, cal_core::Error::NotFound(_)));
    }
}
