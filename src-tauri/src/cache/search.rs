//! Full-text search over the external snapshot cache.
//!
//! §13.1 promises search across "alle lokal gecachten Termine und
//! Aufgaben über alle Konten und Container hinweg". The LOCAL tables are
//! covered by `cal-adapter-local`'s FTS indexes (migration 0002); this
//! module queries the cache mirrors from migration 0027
//! (`cache_events_fts` / `cache_tasks_fts`) so items living on external
//! providers (iCloud, Google, EWS, Vikunja, Todoist, …) are findable
//! too. The host `search` command merges both result sets.
//!
//! Both halves consume the SAME prepared MATCH string
//! ([`cal_adapter_local::prepare_fts_query`]) and the same
//! [`SearchFilters`], so query semantics (prefix match, AND-combined
//! terms, filter behaviour) stay in lock-step with the local search.

use cal_adapter_local::{EventTypeFilter, SearchFilters, SearchKind};
use cal_core::{Event, Task};

use super::CacheStore;
use crate::db::DbResult;

/// Same cap as the local search — beyond this the user should refine.
const LIMIT: usize = 200;

impl CacheStore {
    /// FTS search over cached EXTERNAL events. `fts_query` is the
    /// prepared MATCH string; an empty string yields no hits. Rows whose
    /// payload no longer decodes are skipped with a warning — a stale
    /// cache row must not sink the whole search.
    pub fn search_events_fts(
        &self,
        fts_query: &str,
        filters: &SearchFilters,
    ) -> DbResult<Vec<Event>> {
        if fts_query.is_empty() || filters.kind == SearchKind::Tasks {
            return Ok(Vec::new());
        }
        let mut clauses = Vec::<String>::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(fts_query.to_string())];

        if !filters.calendar_ids.is_empty() {
            clauses.push(in_placeholders("e.calendar_id", filters.calendar_ids.len()));
            for id in &filters.calendar_ids {
                binds.push(Box::new(id.clone()));
            }
        }
        if let Some(since) = &filters.since {
            clauses.push(" AND e.start_utc >= ?".into());
            binds.push(Box::new(since.clone()));
        }
        if let Some(until) = &filters.until {
            clauses.push(" AND e.start_utc <= ?".into());
            binds.push(Box::new(until.clone()));
        }
        match filters.event_type {
            EventTypeFilter::Any => {}
            // `json_extract` maps JSON true/false onto 1/0, so the
            // comparisons below work for the serialized booleans.
            EventTypeFilter::Single => clauses.push(
                " AND json_extract(e.payload, '$.recurrence') IS NULL \
                  AND json_extract(e.payload, '$.all_day') = 0"
                    .into(),
            ),
            EventTypeFilter::Recurring => {
                clauses.push(" AND json_extract(e.payload, '$.recurrence') IS NOT NULL".into())
            }
            EventTypeFilter::AllDay => {
                clauses.push(" AND json_extract(e.payload, '$.all_day') = 1".into())
            }
        }

        let where_extra = clauses.concat();
        let sql = format!(
            "SELECT e.payload
               FROM cache_events_fts f
               JOIN cache_events e
                 ON e.account_id = f.account_id
                AND e.calendar_id = f.calendar_id
                AND e.id = f.id
              WHERE cache_events_fts MATCH ?{where_extra}
              ORDER BY rank
              LIMIT {LIMIT}"
        );
        self.db.with_conn(|c| {
            let mut stmt = c.prepare(&sql)?;
            let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(refs), |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                let payload = row?;
                match serde_json::from_str::<Event>(&payload) {
                    Ok(ev) => out.push(ev),
                    Err(err) => {
                        tracing::warn!(?err, "cache search: skipping undecodable event payload")
                    }
                }
            }
            Ok(out)
        })
    }

    /// FTS search over cached EXTERNAL tasks. Filter semantics mirror
    /// the local task search: the date window matches the scheduled or
    /// deadline date, and tasks without any date are excluded while a
    /// window is active.
    pub fn search_tasks_fts(
        &self,
        fts_query: &str,
        filters: &SearchFilters,
    ) -> DbResult<Vec<Task>> {
        if fts_query.is_empty() || filters.kind == SearchKind::Events {
            return Ok(Vec::new());
        }
        let mut clauses = Vec::<String>::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(fts_query.to_string())];

        if !filters.list_ids.is_empty() {
            clauses.push(in_placeholders("e.list_id", filters.list_ids.len()));
            for id in &filters.list_ids {
                binds.push(Box::new(id.clone()));
            }
        }
        if let Some(since) = &filters.since {
            clauses.push(
                " AND COALESCE(json_extract(e.payload, '$.scheduled_date'), \
                               json_extract(e.payload, '$.deadline_date')) >= ?"
                    .into(),
            );
            binds.push(Box::new(iso_date_part(since)));
        }
        if let Some(until) = &filters.until {
            clauses.push(
                " AND COALESCE(json_extract(e.payload, '$.scheduled_date'), \
                               json_extract(e.payload, '$.deadline_date')) <= ?"
                    .into(),
            );
            binds.push(Box::new(iso_date_part(until)));
        }
        if !filters.task_statuses.is_empty() {
            clauses.push(in_placeholders(
                "json_extract(e.payload, '$.status')",
                filters.task_statuses.len(),
            ));
            for s in &filters.task_statuses {
                binds.push(Box::new(s.clone()));
            }
        }

        let where_extra = clauses.concat();
        let sql = format!(
            "SELECT e.payload
               FROM cache_tasks_fts f
               JOIN cache_tasks e
                 ON e.account_id = f.account_id
                AND e.list_id = f.list_id
                AND e.id = f.id
              WHERE cache_tasks_fts MATCH ?{where_extra}
              ORDER BY rank
              LIMIT {LIMIT}"
        );
        self.db.with_conn(|c| {
            let mut stmt = c.prepare(&sql)?;
            let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(refs), |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                let payload = row?;
                match serde_json::from_str::<Task>(&payload) {
                    Ok(task) => out.push(task),
                    Err(err) => {
                        tracing::warn!(?err, "cache search: skipping undecodable task payload")
                    }
                }
            }
            Ok(out)
        })
    }
}

/// ` AND column IN (?, ?, …)` for an arbitrary value count — same shape
/// as the local search helper.
fn in_placeholders(column: &str, n: usize) -> String {
    let placeholders = std::iter::repeat_n("?", n).collect::<Vec<_>>().join(",");
    format!(" AND {column} IN ({placeholders})")
}

/// Truncate an ISO 8601 datetime to its YYYY-MM-DD prefix so it compares
/// against the date-only task fields.
fn iso_date_part(iso: &str) -> String {
    iso.get(..10).unwrap_or(iso).to_string()
}
