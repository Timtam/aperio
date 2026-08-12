//! Full-text search over events and tasks via the FTS5 indexes
//! provisioned by migration 0002.
//!
//! The adapter wraps `LIKE`-style user input into an FTS5 prefix query.
//! Bare strings (e.g. "team") become `team*`, which lets the user see
//! results before they finish typing. Multi-word queries are AND-ed:
//! `"team meet"` → `team* AND meet*`. Hostile inputs (FTS5 query
//! operators, quotes) are stripped — the search box is not a query
//! editor, it is a free-text field.

use cal_core::{Event, Task};
use serde::{Deserialize, Serialize};

use crate::calendars::row_to_event;
use crate::map_sql_err;
use crate::tasks::row_to_task;
use crate::LocalAdapter;

/// Optional filter applied on top of the full-text query.
///
/// All fields are additive: an empty list / `None` / `Any` means no
/// restriction on that dimension. Backend SQL appends WHERE clauses
/// only for the fields that are actually set.
#[derive(Debug, Default, Deserialize)]
pub struct SearchFilters {
    #[serde(default)]
    pub kind: SearchKind,
    #[serde(default)]
    pub calendar_ids: Vec<String>,
    #[serde(default)]
    pub list_ids: Vec<String>,
    /// ISO 8601 lower bound. Applies to event start_utc and to the
    /// task's scheduled / deadline date (whichever is set).
    #[serde(default)]
    pub since: Option<String>,
    /// ISO 8601 upper bound (inclusive day).
    #[serde(default)]
    pub until: Option<String>,
    /// Event-type filter — ignored when `kind = Tasks`.
    #[serde(default)]
    pub event_type: EventTypeFilter,
    /// Task-status whitelist — ignored when `kind = Events`. Empty
    /// vector = no restriction.
    #[serde(default)]
    pub task_statuses: Vec<String>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchKind {
    #[default]
    Both,
    Events,
    Tasks,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventTypeFilter {
    #[default]
    Any,
    Single,
    Recurring,
    AllDay,
}

/// Combined hit list returned by [`LocalAdapter::search`].
#[derive(Debug, Serialize)]
pub struct SearchResults {
    pub events: Vec<Event>,
    pub tasks: Vec<Task>,
}

impl LocalAdapter {
    /// Run a single user query against both the events_fts and
    /// tasks_fts indexes. Returns the matching rows in full so the UI
    /// can render them without a second round-trip.
    pub fn search(&self, query: &str, filters: &SearchFilters) -> cal_core::Result<SearchResults> {
        let prepared = prepare_fts_query(query);
        if prepared.is_empty() {
            return Ok(SearchResults {
                events: Vec::new(),
                tasks: Vec::new(),
            });
        }

        let conn = self.db().lock().expect("db mutex poisoned");

        // Limit caps the result list at a comfortable scrolling length;
        // anything beyond that and the user should refine the query.
        const LIMIT: usize = 200;

        let events = if filters.kind == SearchKind::Tasks {
            Vec::new()
        } else {
            let mut clauses = Vec::<String>::new();
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            binds.push(Box::new(prepared.clone()));

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
                EventTypeFilter::Single => {
                    clauses.push(" AND e.rrule IS NULL AND e.all_day = 0".into());
                }
                EventTypeFilter::Recurring => {
                    clauses.push(" AND e.rrule IS NOT NULL".into());
                }
                EventTypeFilter::AllDay => {
                    clauses.push(" AND e.all_day = 1".into());
                }
            }
            let where_extra = clauses.concat();
            let sql = format!(
                "SELECT e.id, e.calendar_id, e.title, e.description, e.location,
                        e.start_utc, e.end_utc, e.all_day, e.rrule, e.rrule_exceptions,
                        e.color_label_id, e.reminders, e.sound, e.attendees,
                        e.created_at, e.updated_at, e.etag, e.rrule_tzid
                   FROM events_fts f
                   JOIN events e ON e.id = f.id
                  WHERE events_fts MATCH ?{where_extra}
                  ORDER BY rank
                  LIMIT ?"
            );
            binds.push(Box::new(LIMIT as i64));
            let mut stmt = conn.prepare(&sql).map_err(map_sql_err)?;
            let bind_refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bind_refs), |row| {
                    Ok(row_to_event(row))
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)??);
            }
            out
        };

        let tasks = if filters.kind == SearchKind::Events {
            Vec::new()
        } else {
            let mut clauses = Vec::<String>::new();
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            binds.push(Box::new(prepared.clone()));

            if !filters.list_ids.is_empty() {
                clauses.push(in_placeholders("t.list_id", filters.list_ids.len()));
                for id in &filters.list_ids {
                    binds.push(Box::new(id.clone()));
                }
            }
            // Date range: a task matches if its scheduled or deadline
            // date falls inside the window. Tasks with no date at all
            // are excluded when a range is active — they can't be
            // shown as "in this period".
            if let Some(since) = &filters.since {
                clauses.push(" AND COALESCE(t.scheduled_date, t.deadline_date) >= ?".into());
                let date_part = iso_date_part(since);
                binds.push(Box::new(date_part));
            }
            if let Some(until) = &filters.until {
                clauses.push(" AND COALESCE(t.scheduled_date, t.deadline_date) <= ?".into());
                let date_part = iso_date_part(until);
                binds.push(Box::new(date_part));
            }
            if !filters.task_statuses.is_empty() {
                clauses.push(in_placeholders("t.status", filters.task_statuses.len()));
                for s in &filters.task_statuses {
                    binds.push(Box::new(s.clone()));
                }
            }
            let where_extra = clauses.concat();
            let sql = format!(
                "SELECT t.id, t.list_id, t.parent_id, t.title, t.description,
                        t.status, t.priority, t.scheduled_date, t.scheduled_time,
                        t.deadline_date, t.deadline_time, t.recurrence, t.color_label_id,
                        t.reminders, t.sound, t.created_at, t.updated_at, t.completed_at,
                        t.etag, t.section_id, t.resurface_date, t.series_id, t.effort,
                        t.deadline_reminder_days, t.scheduled_end_time
                   FROM tasks_fts f
                   JOIN tasks t ON t.id = f.id
                  WHERE tasks_fts MATCH ?{where_extra}
                  ORDER BY rank
                  LIMIT ?"
            );
            binds.push(Box::new(LIMIT as i64));
            let mut stmt = conn.prepare(&sql).map_err(map_sql_err)?;
            let bind_refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bind_refs), |row| {
                    Ok(row_to_task(row))
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)??);
            }
            out
        };

        Ok(SearchResults { events, tasks })
    }
}

/// Build a ` AND column IN (?, ?, …)` clause for an arbitrary number
/// of values. Caller is responsible for binding the values in the same
/// order they appear in the original list.
fn in_placeholders(column: &str, n: usize) -> String {
    let placeholders = std::iter::repeat_n("?", n).collect::<Vec<_>>().join(",");
    format!(" AND {column} IN ({placeholders})")
}

/// Truncate an ISO 8601 datetime string to its YYYY-MM-DD prefix so it
/// can be compared against task date columns (which store dates only).
fn iso_date_part(iso: &str) -> String {
    iso.get(..10).unwrap_or(iso).to_string()
}

/// Translate raw user input into an FTS5 query.
///
/// Tokenises on whitespace, strips characters that FTS5 treats as
/// operators (`"` `'` `(` `)` `:` `*`), then appends `*` to each token
/// so the query is prefix-matched and AND-combined. An empty result
/// means the input had nothing tokenisable — the caller short-circuits
/// to no hits.
pub fn prepare_fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|tok| {
            tok.chars()
                .filter(|c| !matches!(c, '"' | '\'' | '(' | ')' | ':' | '*'))
                .collect::<String>()
                // FTS5's query parser treats uppercase AND / OR / NOT /
                // NEAR as operators even with a prefix suffix. The
                // index itself is case-insensitive via the unicode61
                // tokenizer, so lowercasing the query is safe and side-
                // steps the operator-name collision.
                .to_lowercase()
        })
        .filter(|tok| !tok.is_empty())
        .map(|tok| format!("{tok}*"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_test_db;
    use cal_core::{
        CalendarFeature, ContainerColor, NewEvent, NewTask, TaskEffort, TaskPriority, TaskStatus,
        TasksFeature,
    };
    use chrono::{Duration, Utc};

    fn adapter_with_data() -> (LocalAdapter, String, String) {
        let a = LocalAdapter::new(open_test_db());
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let list = a.create_task_list("Inbox", None, None, None, None).unwrap();
        (a, cal.id, list.id)
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc::now()
    }

    fn make_event(title: &str) -> NewEvent {
        NewEvent {
            title: title.into(),
            description: None,
            location: None,
            start: now(),
            end: now() + Duration::hours(1),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            send_invitations: false,
        }
    }

    fn make_task(title: &str) -> NewTask {
        NewTask {
            assignees: Vec::new(),
            title: title.into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            effort: TaskEffort::Medium,
            scheduled_date: None,
            scheduled_time: None,
            scheduled_end_time: None,
            deadline_date: None,
            deadline_time: None,
            deadline_reminder_days: None,
            recurrence: None,
            resurface_date: None,
            series_id: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: vec![],
            sound: None,
        }
    }

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    /// Test shorthand: search with no filters.
    impl LocalAdapter {
        fn search_default(&self, q: &str) -> cal_core::Result<SearchResults> {
            self.search(q, &SearchFilters::default())
        }
    }

    #[test]
    fn kind_filter_excludes_other_side() {
        let (a, cal, list) = adapter_with_data();
        block(a.create_event(&cal, make_event("Match"))).unwrap();
        block(a.create_task(&list, make_task("Match"))).unwrap();

        let only_events = a
            .search(
                "match",
                &SearchFilters {
                    kind: SearchKind::Events,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(only_events.events.len(), 1);
        assert!(only_events.tasks.is_empty());

        let only_tasks = a
            .search(
                "match",
                &SearchFilters {
                    kind: SearchKind::Tasks,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(only_tasks.events.is_empty());
        assert_eq!(only_tasks.tasks.len(), 1);
    }

    #[test]
    fn calendar_id_filter_restricts_events() {
        let (a, cal_a, _list) = adapter_with_data();
        let cal_b = a.create_calendar("Other", None, None, None).unwrap();
        block(a.create_event(&cal_a, make_event("Sync"))).unwrap();
        block(a.create_event(&cal_b.id, make_event("Sync"))).unwrap();

        let only_a = a
            .search(
                "sync",
                &SearchFilters {
                    calendar_ids: vec![cal_a.clone()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(only_a.events.len(), 1);
        assert_eq!(only_a.events[0].calendar_id, cal_a);

        let both = a
            .search(
                "sync",
                &SearchFilters {
                    calendar_ids: vec![cal_a, cal_b.id],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(both.events.len(), 2);
    }

    #[test]
    fn list_id_filter_restricts_tasks() {
        let (a, _cal, list_a) = adapter_with_data();
        let list_b = a.create_task_list("Other", None, None, None, None).unwrap();
        block(a.create_task(&list_a, make_task("Buy"))).unwrap();
        block(a.create_task(&list_b.id, make_task("Buy"))).unwrap();

        let only_a = a
            .search(
                "buy",
                &SearchFilters {
                    list_ids: vec![list_a.clone()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(only_a.tasks.len(), 1);
        assert_eq!(only_a.tasks[0].list_id, list_a);
    }

    #[test]
    fn finds_event_by_title_prefix() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("Team meeting"))).unwrap();
        let hits = a.search_default("team").unwrap();
        assert_eq!(hits.events.len(), 1);
        assert_eq!(hits.events[0].title, "Team meeting");
    }

    #[test]
    fn finds_task_by_title_prefix() {
        let (a, _cal, list) = adapter_with_data();
        block(a.create_task(&list, make_task("Write report"))).unwrap();
        let hits = a.search_default("rep").unwrap();
        assert_eq!(hits.tasks.len(), 1);
        assert_eq!(hits.tasks[0].title, "Write report");
    }

    #[test]
    fn multi_word_query_is_anded() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("Team standup meeting"))).unwrap();
        block(a.create_event(&cal, make_event("Yoga class"))).unwrap();
        // Both words must match.
        let hits = a.search_default("team meeting").unwrap();
        assert_eq!(hits.events.len(), 1);
        assert_eq!(hits.events[0].title, "Team standup meeting");
    }

    #[test]
    fn finds_event_by_calendar_name() {
        let (a, _cal, _list) = adapter_with_data();
        let other = a
            .create_calendar(
                "Birthdays",
                Some(ContainerColor::custom("#fb8c00")),
                None,
                None,
            )
            .unwrap();
        block(a.create_event(&other.id, make_event("Cake"))).unwrap();
        let hits = a.search_default("birthday").unwrap();
        assert_eq!(hits.events.len(), 1);
    }

    #[test]
    fn renaming_calendar_updates_index() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("Sync"))).unwrap();
        let hits_before = a.search_default("work").unwrap();
        assert_eq!(hits_before.events.len(), 1);

        // Rename the calendar — the fts trigger should propagate the
        // new name and the old one should stop matching.
        let cals = block(a.list_calendars()).unwrap();
        let mut work = cals.into_iter().find(|c| c.id == cal).unwrap();
        work.name = "Office".into();
        a.update_calendar(work).unwrap();

        let hits_after_old = a.search_default("work").unwrap();
        assert_eq!(hits_after_old.events.len(), 0);
        let hits_after_new = a.search_default("office").unwrap();
        assert_eq!(hits_after_new.events.len(), 1);
    }

    #[test]
    fn empty_query_returns_empty_results() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("X"))).unwrap();
        let hits = a.search_default("   ").unwrap();
        assert!(hits.events.is_empty());
        assert!(hits.tasks.is_empty());
    }

    #[test]
    fn fts_operators_in_input_are_stripped() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("AND OR foo"))).unwrap();
        // Without stripping, "AND" would be parsed as an FTS operator
        // and the query would fail. With stripping, it is searched
        // verbatim and matches.
        let hits = a.search_default("\"AND\"").unwrap();
        assert_eq!(hits.events.len(), 1);
    }

    #[test]
    fn event_type_filter_distinguishes_recurring_and_single() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("Plain"))).unwrap();
        let mut recurring = make_event("Plain");
        recurring.recurrence = Some(cal_core::EventRecurrence {
            rrule: "FREQ=WEEKLY".into(),
            exceptions: vec![],
            tzid: None,
        });
        block(a.create_event(&cal, recurring)).unwrap();

        let only_single = a
            .search(
                "plain",
                &SearchFilters {
                    event_type: EventTypeFilter::Single,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(only_single.events.len(), 1);
        assert!(only_single.events[0].recurrence.is_none());

        let only_recurring = a
            .search(
                "plain",
                &SearchFilters {
                    event_type: EventTypeFilter::Recurring,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(only_recurring.events.len(), 1);
        assert!(only_recurring.events[0].recurrence.is_some());
    }

    #[test]
    fn task_status_filter_restricts_results() {
        let (a, _cal, list) = adapter_with_data();
        let mut open = make_task("Ping");
        open.status = TaskStatus::Open;
        block(a.create_task(&list, open)).unwrap();
        let mut completed = make_task("Ping");
        completed.status = TaskStatus::Completed;
        block(a.create_task(&list, completed)).unwrap();

        let only_completed = a
            .search(
                "ping",
                &SearchFilters {
                    task_statuses: vec!["completed".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(only_completed.tasks.len(), 1);
        assert!(matches!(
            only_completed.tasks[0].status,
            TaskStatus::Completed
        ));
    }

    #[test]
    fn date_range_filter_restricts_events() {
        let (a, cal, _list) = adapter_with_data();
        let mut soon = make_event("Window");
        soon.start = Utc::now() + Duration::days(2);
        soon.end = soon.start + Duration::hours(1);
        block(a.create_event(&cal, soon)).unwrap();
        let mut later = make_event("Window");
        later.start = Utc::now() + Duration::days(30);
        later.end = later.start + Duration::hours(1);
        block(a.create_event(&cal, later)).unwrap();

        let bound_lo = (Utc::now() + Duration::days(1)).to_rfc3339();
        let bound_hi = (Utc::now() + Duration::days(7)).to_rfc3339();
        let in_window = a
            .search(
                "window",
                &SearchFilters {
                    since: Some(bound_lo),
                    until: Some(bound_hi),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(in_window.events.len(), 1);
    }

    #[test]
    fn deleted_event_is_dropped_from_index() {
        let (a, cal, _list) = adapter_with_data();
        let ev = block(a.create_event(&cal, make_event("Standup"))).unwrap();
        block(a.delete_event(&ev.id, false)).unwrap();
        let hits = a.search_default("standup").unwrap();
        assert!(hits.events.is_empty());
    }
}
