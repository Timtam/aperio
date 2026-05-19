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
use rusqlite::params;
use serde::Serialize;

use crate::calendars::row_to_event;
use crate::map_sql_err;
use crate::tasks::row_to_task;
use crate::LocalAdapter;

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
    pub fn search(&self, query: &str) -> cal_core::Result<SearchResults> {
        let prepared = prepare_query(query);
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

        let mut events_stmt = conn
            .prepare(
                "SELECT e.id, e.calendar_id, e.title, e.description, e.location,
                        e.start_utc, e.end_utc, e.all_day, e.rrule, e.rrule_exceptions,
                        e.color_label_id, e.reminders, e.sound, e.attendees,
                        e.created_at, e.updated_at, e.etag
                   FROM events_fts f
                   JOIN events e ON e.id = f.id
                  WHERE events_fts MATCH ?
                  ORDER BY rank
                  LIMIT ?",
            )
            .map_err(map_sql_err)?;
        let event_rows = events_stmt
            .query_map(params![prepared, LIMIT as i64], |row| Ok(row_to_event(row)))
            .map_err(map_sql_err)?;
        let mut events = Vec::new();
        for r in event_rows {
            events.push(r.map_err(map_sql_err)??);
        }

        let mut tasks_stmt = conn
            .prepare(
                "SELECT t.id, t.list_id, t.parent_id, t.title, t.description,
                        t.status, t.priority, t.scheduled_date, t.deadline_type,
                        t.deadline_date, t.deadline_time, t.recurrence, t.color_label_id,
                        t.reminders, t.sound, t.created_at, t.updated_at, t.completed_at,
                        t.etag
                   FROM tasks_fts f
                   JOIN tasks t ON t.id = f.id
                  WHERE tasks_fts MATCH ?
                  ORDER BY rank
                  LIMIT ?",
            )
            .map_err(map_sql_err)?;
        let task_rows = tasks_stmt
            .query_map(params![prepared, LIMIT as i64], |row| Ok(row_to_task(row)))
            .map_err(map_sql_err)?;
        let mut tasks = Vec::new();
        for r in task_rows {
            tasks.push(r.map_err(map_sql_err)??);
        }

        Ok(SearchResults { events, tasks })
    }
}

/// Translate raw user input into an FTS5 query.
///
/// Tokenises on whitespace, strips characters that FTS5 treats as
/// operators (`"` `'` `(` `)` `:` `*`), then appends `*` to each token
/// so the query is prefix-matched and AND-combined. An empty result
/// means the input had nothing tokenisable — the caller short-circuits
/// to no hits.
fn prepare_query(input: &str) -> String {
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
        CalendarFeature, ContainerColor, NewEvent, NewTask, TaskPriority, TaskStatus, TasksFeature,
    };
    use chrono::{Duration, Utc};

    fn adapter_with_data() -> (LocalAdapter, String, String) {
        let a = LocalAdapter::new(open_test_db());
        let cal = a.create_calendar("Work", None, None).unwrap();
        let list = a.create_task_list("Inbox", None, None, None).unwrap();
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
            reminders: vec![],
            sound: None,
            attendees: vec![],
        }
    }

    fn make_task(title: &str) -> NewTask {
        NewTask {
            title: title.into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            deadline_type: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
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

    #[test]
    fn finds_event_by_title_prefix() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("Team meeting"))).unwrap();
        let hits = a.search("team").unwrap();
        assert_eq!(hits.events.len(), 1);
        assert_eq!(hits.events[0].title, "Team meeting");
    }

    #[test]
    fn finds_task_by_title_prefix() {
        let (a, _cal, list) = adapter_with_data();
        block(a.create_task(&list, make_task("Write report"))).unwrap();
        let hits = a.search("rep").unwrap();
        assert_eq!(hits.tasks.len(), 1);
        assert_eq!(hits.tasks[0].title, "Write report");
    }

    #[test]
    fn multi_word_query_is_anded() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("Team standup meeting"))).unwrap();
        block(a.create_event(&cal, make_event("Yoga class"))).unwrap();
        // Both words must match.
        let hits = a.search("team meeting").unwrap();
        assert_eq!(hits.events.len(), 1);
        assert_eq!(hits.events[0].title, "Team standup meeting");
    }

    #[test]
    fn finds_event_by_calendar_name() {
        let (a, _cal, _list) = adapter_with_data();
        let other = a
            .create_calendar("Birthdays", Some(ContainerColor::custom("#fb8c00")), None)
            .unwrap();
        block(a.create_event(&other.id, make_event("Cake"))).unwrap();
        let hits = a.search("birthday").unwrap();
        assert_eq!(hits.events.len(), 1);
    }

    #[test]
    fn renaming_calendar_updates_index() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("Sync"))).unwrap();
        let hits_before = a.search("work").unwrap();
        assert_eq!(hits_before.events.len(), 1);

        // Rename the calendar — the fts trigger should propagate the
        // new name and the old one should stop matching.
        let cals = block(a.list_calendars()).unwrap();
        let mut work = cals.into_iter().find(|c| c.id == cal).unwrap();
        work.name = "Office".into();
        a.update_calendar(work).unwrap();

        let hits_after_old = a.search("work").unwrap();
        assert_eq!(hits_after_old.events.len(), 0);
        let hits_after_new = a.search("office").unwrap();
        assert_eq!(hits_after_new.events.len(), 1);
    }

    #[test]
    fn empty_query_returns_empty_results() {
        let (a, cal, _list) = adapter_with_data();
        block(a.create_event(&cal, make_event("X"))).unwrap();
        let hits = a.search("   ").unwrap();
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
        let hits = a.search("\"AND\"").unwrap();
        assert_eq!(hits.events.len(), 1);
    }

    #[test]
    fn deleted_event_is_dropped_from_index() {
        let (a, cal, _list) = adapter_with_data();
        let ev = block(a.create_event(&cal, make_event("Standup"))).unwrap();
        block(a.delete_event(&ev.id)).unwrap();
        let hits = a.search("standup").unwrap();
        assert!(hits.events.is_empty());
    }
}
