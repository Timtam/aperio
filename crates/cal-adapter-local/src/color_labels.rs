//! Color-label management against the local SQLite store.
//!
//! Labels are app-level metadata — they don't fit any adapter feature
//! trait, but the local adapter is the canonical owner of the
//! `color_labels` table. Inherent methods on [`LocalAdapter`] match the
//! style used for local-only calendar/task management (see
//! `calendars.rs` and `tasks.rs`).

use cal_core::{ColorLabel, ColorLabelId};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::mapping::req_text;
use crate::{map_sql_err, LocalAdapter};

impl LocalAdapter {
    pub fn list_color_labels(&self) -> cal_core::Result<Vec<ColorLabel>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT id, name, hex, ad_hoc FROM color_labels ORDER BY name COLLATE NOCASE")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    req_text(row, 0),
                    req_text(row, 1),
                    req_text(row, 2),
                    row.get::<_, i64>(3),
                ))
            })
            .map_err(map_sql_err)?;

        let mut out = Vec::new();
        for r in rows {
            let (id, name, hex, ad_hoc) = r.map_err(map_sql_err)?;
            out.push(ColorLabel {
                id: ColorLabelId::new(id?),
                name: name?,
                hex: hex?,
                ad_hoc: ad_hoc.map_err(map_sql_err)? != 0,
            });
        }
        Ok(out)
    }

    /// Fetch one color label by id. Returns `None` when missing.
    /// Used by the conflict-detection path so it can compare a
    /// proposed patch against the live row.
    pub fn get_color_label_by_id(&self, id: &str) -> cal_core::Result<Option<ColorLabel>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT id, name, hex, ad_hoc FROM color_labels WHERE id = ?")
            .map_err(map_sql_err)?;
        let row = stmt
            .query_row(params![id], |r| {
                Ok((
                    req_text(r, 0),
                    req_text(r, 1),
                    req_text(r, 2),
                    r.get::<_, i64>(3),
                ))
            })
            .optional()
            .map_err(map_sql_err)?;
        let Some(parts) = row else {
            return Ok(None);
        };
        let (id, name, hex, ad_hoc) = parts;
        Ok(Some(ColorLabel {
            id: ColorLabelId::new(id?),
            name: name?,
            hex: hex?,
            ad_hoc: ad_hoc.map_err(map_sql_err)? != 0,
        }))
    }

    /// Resolve a color-label id to its hex (`#rrggbb`), or `None` when the
    /// label doesn't exist. The write-path counterpart to
    /// [`match_hex_to_label`](Self::match_hex_to_label): the host turns an
    /// event's `color_label` into the `color_hex` a color-capable provider
    /// stores natively (RFC 7986 `COLOR`).
    pub fn resolve_label_to_hex(&self, id: &str) -> cal_core::Result<Option<String>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.query_row(
            "SELECT hex FROM color_labels WHERE id = ?",
            params![id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sql_err)
    }

    /// Find a color-label id whose hex matches `hex` (case-insensitive),
    /// preferring a *named* label over an ad-hoc one. `None` when no label
    /// carries that hex. The read-path counterpart to
    /// [`resolve_label_to_hex`](Self::resolve_label_to_hex): the host maps a
    /// provider's native `COLOR` back to a label. No ad-hoc label is minted
    /// on read, so a foreign color with no matching label resolves to *no*
    /// label (acceptable for v1).
    pub fn match_hex_to_label(&self, hex: &str) -> cal_core::Result<Option<String>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.query_row(
            "SELECT id FROM color_labels WHERE hex = ?1 COLLATE NOCASE \
             ORDER BY ad_hoc ASC LIMIT 1",
            params![hex],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sql_err)
    }

    pub fn create_color_label(&self, name: &str, hex: &str) -> cal_core::Result<ColorLabel> {
        let id = Uuid::new_v4().to_string();
        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO color_labels (id, name, hex, ad_hoc) VALUES (?, ?, ?, 0)",
                params![id, name, hex],
            )
            .map_err(map_sql_err)?;
        Ok(ColorLabel {
            id: ColorLabelId::new(id),
            name: name.to_string(),
            hex: hex.to_string(),
            ad_hoc: false,
        })
    }

    /// Resolve a custom one-off color to a hidden *ad-hoc* color label,
    /// reusing an existing ad-hoc label with the same hex (dedup) or
    /// creating one (with `name == hex`). Returns the label plus whether
    /// it was newly created, so the caller can emit a sync event only on
    /// creation.
    pub fn get_or_create_ad_hoc_color_label(
        &self,
        hex: &str,
    ) -> cal_core::Result<(ColorLabel, bool)> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM color_labels WHERE hex = ? AND ad_hoc = 1 LIMIT 1",
                params![hex],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sql_err)?;
        if let Some(id) = existing {
            return Ok((
                ColorLabel {
                    id: ColorLabelId::new(id),
                    name: hex.to_string(),
                    hex: hex.to_string(),
                    ad_hoc: true,
                },
                false,
            ));
        }
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO color_labels (id, name, hex, ad_hoc) VALUES (?, ?, ?, 1)",
            params![id, hex, hex],
        )
        .map_err(map_sql_err)?;
        Ok((
            ColorLabel {
                id: ColorLabelId::new(id),
                name: hex.to_string(),
                hex: hex.to_string(),
                ad_hoc: true,
            },
            true,
        ))
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
        let cal = a.create_calendar("Work", None, None, None).unwrap();
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

    #[test]
    fn ad_hoc_color_label_is_created_then_deduped() {
        let a = adapter();
        let (first, created1) = a.get_or_create_ad_hoc_color_label("#3366cc").unwrap();
        assert!(created1);
        assert!(first.ad_hoc);
        assert_eq!(first.name, "#3366cc");
        assert_eq!(first.hex, "#3366cc");

        // Same hex → same label, no second row.
        let (second, created2) = a.get_or_create_ad_hoc_color_label("#3366cc").unwrap();
        assert!(!created2);
        assert_eq!(second.id, first.id);

        // A different hex makes a distinct ad-hoc label.
        let (other, created3) = a.get_or_create_ad_hoc_color_label("#cc3366").unwrap();
        assert!(created3);
        assert_ne!(other.id, first.id);

        // Both ad-hoc labels are listed (the UI filters them out, not the store).
        let all = a.list_color_labels().unwrap();
        assert_eq!(all.iter().filter(|l| l.ad_hoc).count(), 2);
    }

    #[test]
    fn named_label_is_not_ad_hoc() {
        let a = adapter();
        let l = a.create_color_label("Work", "#e53935").unwrap();
        assert!(!l.ad_hoc);
        let fetched = a.get_color_label_by_id(l.id.as_str()).unwrap().unwrap();
        assert!(!fetched.ad_hoc);
    }

    #[test]
    fn resolve_label_to_hex_round_trips() {
        let a = adapter();
        let l = a.create_color_label("Work", "#4285f4").unwrap();
        assert_eq!(
            a.resolve_label_to_hex(l.id.as_str()).unwrap().as_deref(),
            Some("#4285f4"),
        );
        // Unknown id → None.
        assert_eq!(a.resolve_label_to_hex("does-not-exist").unwrap(), None);
    }

    #[test]
    fn match_hex_to_label_finds_label_case_insensitively() {
        let a = adapter();
        let l = a.create_color_label("Travel", "#fb8c00").unwrap();
        // Exact match.
        assert_eq!(
            a.match_hex_to_label("#fb8c00").unwrap().as_deref(),
            Some(l.id.as_str()),
        );
        // Case-insensitive (CalDAV reads normalise hex to lowercase, but a
        // stored label may be upper/mixed case).
        assert_eq!(
            a.match_hex_to_label("#FB8C00").unwrap().as_deref(),
            Some(l.id.as_str()),
        );
        // No label carries this hex → None (no ad-hoc label is minted here).
        assert_eq!(a.match_hex_to_label("#000000").unwrap(), None);
    }

    #[test]
    fn match_hex_to_label_prefers_named_over_ad_hoc() {
        let a = adapter();
        // An ad-hoc label and a named label share the same hex.
        let (adhoc, _) = a.get_or_create_ad_hoc_color_label("#34a853").unwrap();
        let named = a.create_color_label("Done", "#34a853").unwrap();
        let got = a.match_hex_to_label("#34a853").unwrap();
        assert_eq!(got.as_deref(), Some(named.id.as_str()));
        assert_ne!(got.as_deref(), Some(adhoc.id.as_str()));
    }
}
