//! Day markers + the per-day log, against the local SQLite store.
//!
//! App-level metadata like `color_labels.rs` next door: no adapter feature
//! trait fits, and the local store is the canonical owner. Nothing here talks
//! to an external provider — no provider models "how was Tuesday", and this is
//! the most private thing in the app.

use cal_core::{ColorLabelId, DayLog, DayMarker};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::mapping::{opt_text, req_text};
use crate::{map_sql_err, LocalAdapter};

/// Parse a stored `YYYY-MM-DD`. A row whose day cannot be read is not a row
/// anyone can act on, so the readers skip it rather than guessing a date.
fn parse_day(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
}

fn parse_stamp(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// The marker ids stored on a row. A malformed blob reads as "nothing ticked"
/// rather than failing the whole day — the log is an annotation, and losing
/// the view of a month because one row is bad would be the worse outcome.
fn decode_markers(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

impl LocalAdapter {
    // ── The vocabulary ───────────────────────────────────────────────────

    /// Every marker, in the order the user arranged them.
    ///
    /// `position` then `name`: two markers can share a position (a reorder
    /// that raced, or a sync from a device mid-edit), and a stable second key
    /// keeps the list from shuffling under the reader between two loads.
    pub fn list_day_markers(&self) -> cal_core::Result<Vec<DayMarker>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, symbol, color_label, position, created_at, updated_at
                   FROM day_markers
                  ORDER BY position, name COLLATE NOCASE",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    req_text(row, 0),
                    req_text(row, 1),
                    opt_text(row, 2),
                    opt_text(row, 3),
                    row.get::<_, i64>(4),
                    req_text(row, 5),
                    req_text(row, 6),
                ))
            })
            .map_err(map_sql_err)?;

        let mut out = Vec::new();
        for r in rows {
            let (id, name, symbol, color, position, created, updated) = r.map_err(map_sql_err)?;
            out.push(DayMarker {
                id: id?,
                name: name?,
                symbol: symbol?.filter(|s| !s.is_empty()),
                color_label: color?.filter(|s| !s.is_empty()).map(ColorLabelId::new),
                position: position.map_err(map_sql_err)?,
                created_at: parse_stamp(&created?),
                updated_at: parse_stamp(&updated?),
            });
        }
        Ok(out)
    }

    /// Add a marker at the end of the list.
    pub fn create_day_marker(
        &self,
        name: &str,
        symbol: Option<&str>,
        color_label: Option<&ColorLabelId>,
    ) -> cal_core::Result<DayMarker> {
        let now = Utc::now();
        let marker = DayMarker {
            id: Uuid::new_v4().to_string(),
            name: name.trim().to_string(),
            symbol: symbol
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            color_label: color_label.cloned(),
            position: self.next_day_marker_position()?,
            created_at: now,
            updated_at: now,
        };
        self.write_day_marker(&marker)?;
        Ok(marker)
    }

    /// Position for a new marker: one past the current last. Read and write
    /// are separate statements, so two rapid adds can land on the same
    /// position — harmless, because `list_day_markers` breaks the tie by name.
    fn next_day_marker_position(&self) -> cal_core::Result<i64> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let max: Option<i64> = conn
            .query_row("SELECT MAX(position) FROM day_markers", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(map_sql_err)?
            .flatten();
        Ok(max.unwrap_or(-1) + 1)
    }

    /// Insert or replace a marker verbatim — the shape the sync applier needs,
    /// and what `create`/`update` funnel through.
    pub fn write_day_marker(&self, marker: &DayMarker) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO day_markers
                 (id, name, symbol, color_label, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 symbol = excluded.symbol,
                 color_label = excluded.color_label,
                 position = excluded.position,
                 updated_at = excluded.updated_at",
            params![
                marker.id,
                marker.name,
                marker.symbol,
                marker.color_label.as_ref().map(|c| c.as_str()),
                marker.position,
                marker.created_at.to_rfc3339(),
                marker.updated_at.to_rfc3339(),
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    /// Drop a marker from the vocabulary.
    ///
    /// The day rows are NOT rewritten. Every reader resolves ids against the
    /// vocabulary and drops what it cannot find, so the marker disappears from
    /// history by itself — and a delete that was a mistake can be undone by
    /// re-creating the marker with the same id, which a sync replay does.
    /// Rewriting thousands of day rows to chase one deletion would be a large,
    /// irreversible write in exchange for nothing the user can see.
    pub fn delete_day_marker(&self, id: &str) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM day_markers WHERE id = ?", params![id])
            .map_err(map_sql_err)?;
        Ok(())
    }

    // ── The per-day log ──────────────────────────────────────────────────

    /// What one day was marked with. An untouched day reads as an empty log
    /// rather than absence, so callers never branch on "was there a row".
    pub fn day_log(&self, day: NaiveDate) -> cal_core::Result<DayLog> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let row = conn
            .query_row(
                "SELECT markers, rating, updated_at FROM day_log WHERE day = ?",
                params![day.format("%Y-%m-%d").to_string()],
                |row| {
                    Ok((
                        req_text(row, 0),
                        row.get::<_, Option<i64>>(1),
                        req_text(row, 2),
                    ))
                },
            )
            .optional()
            .map_err(map_sql_err)?;

        let Some((markers, rating, updated)) = row else {
            return Ok(DayLog::empty(day));
        };
        Ok(DayLog {
            day,
            markers: decode_markers(&markers?),
            rating: rating.map_err(map_sql_err)?,
            updated_at: parse_stamp(&updated?),
        })
    }

    /// Every logged day in `[from, to]`, inclusive.
    ///
    /// Days with nothing on them are simply absent — a month view asks for 31
    /// days and gets back only the ones that say something, which is what the
    /// summaries want anyway.
    pub fn day_logs_in_range(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> cal_core::Result<Vec<DayLog>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT day, markers, rating, updated_at
                   FROM day_log
                  WHERE day >= ?1 AND day <= ?2
                  ORDER BY day",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map(
                params![
                    from.format("%Y-%m-%d").to_string(),
                    to.format("%Y-%m-%d").to_string()
                ],
                |row| {
                    Ok((
                        req_text(row, 0),
                        req_text(row, 1),
                        row.get::<_, Option<i64>>(2),
                        req_text(row, 3),
                    ))
                },
            )
            .map_err(map_sql_err)?;

        let mut out = Vec::new();
        for r in rows {
            let (day, markers, rating, updated) = r.map_err(map_sql_err)?;
            let Some(day) = parse_day(&day?) else {
                continue;
            };
            out.push(DayLog {
                day,
                markers: decode_markers(&markers?),
                rating: rating.map_err(map_sql_err)?,
                updated_at: parse_stamp(&updated?),
            });
        }
        Ok(out)
    }

    /// Write a day's log.
    ///
    /// A log with nothing left on it DELETES the row rather than storing an
    /// empty one: "I unticked everything" and "I never touched this day" are
    /// the same statement, and keeping the difference would leave the store
    /// growing a row for every day the user ever opened.
    pub fn set_day_log(&self, log: &DayLog) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let key = log.day.format("%Y-%m-%d").to_string();
        if log.is_empty() {
            conn.execute("DELETE FROM day_log WHERE day = ?", params![key])
                .map_err(map_sql_err)?;
            return Ok(());
        }
        let markers = serde_json::to_string(&log.markers)
            .map_err(|e| cal_core::Error::Internal(e.to_string()))?;
        conn.execute(
            "INSERT INTO day_log (day, markers, rating, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(day) DO UPDATE SET
                 markers = excluded.markers,
                 rating = excluded.rating,
                 updated_at = excluded.updated_at",
            params![key, markers, log.rating, log.updated_at.to_rfc3339()],
        )
        .map_err(map_sql_err)?;
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

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn markers_keep_the_order_they_were_built_in() {
        let a = adapter();
        for name in ["Sport", "Gelesen", "Meditiert"] {
            a.create_day_marker(name, None, None).unwrap();
        }
        let names: Vec<String> = a
            .list_day_markers()
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, ["Sport", "Gelesen", "Meditiert"]);
    }

    #[test]
    fn an_untouched_day_reads_as_an_empty_log() {
        // Callers must never have to branch on "was there a row".
        let log = adapter().day_log(day("2026-08-17")).unwrap();
        assert!(log.is_empty());
        assert_eq!(log.day, day("2026-08-17"));
    }

    #[test]
    fn a_day_round_trips_its_markers() {
        let a = adapter();
        let sport = a.create_day_marker("Sport", Some("🏃"), None).unwrap();
        let read = a.create_day_marker("Gelesen", None, None).unwrap();

        let mut log = DayLog::empty(day("2026-08-17"));
        log.markers = vec![sport.id.clone(), read.id.clone()];
        a.set_day_log(&log).unwrap();

        let back = a.day_log(day("2026-08-17")).unwrap();
        assert_eq!(back.markers, vec![sport.id, read.id]);
        assert!(!back.is_empty());
    }

    #[test]
    fn unticking_everything_removes_the_row_rather_than_storing_a_blank() {
        // "I unticked everything" and "I never touched this day" are the same
        // statement; keeping both would grow a row per day ever opened.
        let a = adapter();
        let sport = a.create_day_marker("Sport", None, None).unwrap();
        let mut log = DayLog::empty(day("2026-08-17"));
        log.markers = vec![sport.id];
        a.set_day_log(&log).unwrap();

        log.markers.clear();
        a.set_day_log(&log).unwrap();
        assert!(a.day_log(day("2026-08-17")).unwrap().is_empty());
        assert!(a
            .day_logs_in_range(day("2026-08-01"), day("2026-08-31"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_range_returns_only_the_days_that_say_something() {
        let a = adapter();
        let m = a.create_day_marker("Sport", None, None).unwrap();
        for d in ["2026-08-16", "2026-08-18", "2026-09-02"] {
            let mut log = DayLog::empty(day(d));
            log.markers = vec![m.id.clone()];
            a.set_day_log(&log).unwrap();
        }
        let got = a
            .day_logs_in_range(day("2026-08-01"), day("2026-08-31"))
            .unwrap();
        assert_eq!(
            got.iter().map(|l| l.day).collect::<Vec<_>>(),
            [day("2026-08-16"), day("2026-08-18")]
        );
    }

    #[test]
    fn deleting_a_marker_leaves_history_alone_and_the_readers_resolve_it_away() {
        // The day rows keep the id; the vocabulary is the source of truth, so
        // the marker simply stops resolving. No sweeping rewrite of history.
        let a = adapter();
        let m = a.create_day_marker("Sport", None, None).unwrap();
        let mut log = DayLog::empty(day("2026-08-17"));
        log.markers = vec![m.id.clone()];
        a.set_day_log(&log).unwrap();

        a.delete_day_marker(&m.id).unwrap();
        assert!(a.list_day_markers().unwrap().is_empty());
        // The row survives untouched — resolving is the reader's job.
        assert_eq!(a.day_log(day("2026-08-17")).unwrap().markers, vec![m.id]);
    }
}
