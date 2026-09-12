//! Which tasks a calendar day shows, in which order, with which time and which
//! marker — and how the backlog rail cuts its weeks.
//!
//! These are the rules the day, week and month views and the mobile day list
//! render tasks from, and they used to be TypeScript in `shared/taskDay.ts`,
//! asked inside `useMemo` on both surfaces. They moved here so that a frontend
//! which is not JavaScript can ask the same questions and get the same
//! answers, and so that the answers exist once.
//!
//! # What the core answers
//!
//! For every day asked, the tasks on it as POSITIONS in the input, in display
//! order, each with the three facts a chip is drawn from: the time it slots
//! into (if any), the end of its planned block (if any), and whether it sits
//! here because of its deadline rather than its plan. One crossing per day
//! answers the four questions the views used to ask per chip.
//!
//! # What the core deliberately does not do
//!
//! It reads no clock and no time zone (`tests/core_contracts.rs`). Which day
//! is "today" is the caller's to say. And the LOCAL day a task was completed
//! on — the whole subtlety of where a finished task sits, because
//! `completed_at` is a UTC instant and a tick at 23:30 in a positive offset is
//! already tomorrow in UTC — is the caller's to resolve, with the zone it
//! knows, and travels in as [`DayTask::completed_day`]. The core compares day
//! keys; it never converts an instant.
//!
//! Sorting timed tasks against the caller's events is not here either: the
//! caller has its events as epoch milliseconds and its own idea of what a
//! local wall-clock time means on a day with a DST jump. That is rendering,
//! and it stays where the events are.
//!
//! Every answer here is pinned by `tests/fixtures/taskDay.json`, measured by
//! running the TypeScript this replaces; the `contract` module below reads it,
//! and so does the TypeScript contract test on the other side of the boundary.
//! The wire types live outside the `collation` feature gate so that
//! `cargo xtask ts-types` can generate their TypeScript declarations; only the
//! day rule itself, which orders by title, is behind the feature.

#[cfg(feature = "collation")]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Duration, NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};

#[cfg(feature = "collation")]
use crate::collation::CollationLanguage;
#[cfg(feature = "collation")]
use crate::task_assignment::is_mine_or_unassigned;
#[cfg(feature = "collation")]
use crate::task_grouping::task_order;
use crate::task_priority::PriorityScale;
use crate::types::{TaskPriority, TaskStatus, TaskUser};

/// What the day rule reads of a task — by the same field names as `Task`, so
/// a full task's JSON deserializes into it, plus the one thing only the caller
/// can know.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayTask {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    #[serde(default)]
    pub scheduled_date: Option<NaiveDate>,
    #[serde(default)]
    pub scheduled_time: Option<NaiveTime>,
    #[serde(default)]
    pub scheduled_end_time: Option<NaiveTime>,
    #[serde(default)]
    pub deadline_date: Option<NaiveDate>,
    #[serde(default)]
    pub deadline_time: Option<NaiveTime>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub assignees: Vec<TaskUser>,
    /// The LOCAL day the task was completed on, or `None` when it carries no
    /// completion instant. Resolved by the caller from `completed_at` with the
    /// zone the caller knows; the core reads none. A finished task with no
    /// planned day belongs to this day.
    #[serde(default)]
    pub completed_day: Option<NaiveDate>,
}

/// Everything the day rule needs, and nothing it does not.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DayInput {
    pub tasks: Vec<DayTask>,
    /// The days asked about. The week view asks for all of its days at once.
    pub days: Vec<NaiveDate>,
    /// Lists whose completed tasks are shown — the per-list opt-in. Empty
    /// means none: done is done.
    #[serde(default)]
    pub completed_visible: HashSet<String>,
    /// List id → the connected user, for the ownership filter: on a shared
    /// list a task assigned to a concrete OTHER user is theirs, so it is hidden
    /// from MY calendar; mine and unassigned stay. Absent or `null` means a
    /// list with no identity, which keeps everything.
    #[serde(default)]
    pub current_user_by_list: HashMap<String, Option<TaskUser>>,
    /// The user's priority system — how many bands the ordering has.
    #[serde(default)]
    pub scale: PriorityScale,
    /// The app's language tag, for the collation of titles. Empty answers the
    /// default, like every unknown tag.
    #[serde(default)]
    pub language: String,
}

/// One task on one day: where it is in the input, and what its chip carries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DayTaskRow {
    /// Position of the task in [`DayInput::tasks`].
    pub task: usize,
    /// The time the task slots into on this day: its scheduled time when
    /// scheduled here, else its deadline time when due here, else nothing —
    /// there is no minute to honestly point at, so it keeps its place in the
    /// untimed lane. "I plan to do it then" beats "must be done by then".
    pub time: Option<NaiveTime>,
    /// The end of the task's planned block on this day. Only the scheduled
    /// slot can carry a block: a deadline is a moment, and giving it a length
    /// would draw a bar across the hours before something is due.
    pub end_time: Option<NaiveTime>,
    /// True when the task sits here BECAUSE of its deadline, not its plan — a
    /// "due here" marker rather than a "planned work" chip. A task scheduled
    /// and due on the same day is the scheduled chip.
    pub deadline_chip: bool,
}

/// The two calendar weeks the backlog rail splits its deadlines into, as
/// inclusive day bounds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct BacklogWeeks {
    /// First day of the week `today` falls in.
    pub this_week_start: NaiveDate,
    /// Last day of that week, inclusive.
    pub this_week_end: NaiveDate,
    pub next_week_start: NaiveDate,
    /// Last day of the following week, inclusive.
    pub next_week_end: NaiveDate,
}

/// The question [`backlog_weeks`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct BacklogWeeksInput {
    /// The caller's today. A parameter, never the clock.
    pub today: NaiveDate,
    /// The user's own setting: 0 = Sunday … 6 = Saturday.
    pub week_starts_on: u32,
}

/// The question [`split_deadlines_by_week`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DeadlineSplitInput {
    /// The deadline of each item, in the caller's order; `null` for an item
    /// without one.
    pub deadlines: Vec<Option<NaiveDate>>,
    pub weeks: BacklogWeeks,
}

/// Deadline-carrying items by week, as positions in the input, each bucket in
/// input order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DeadlineSplit {
    /// Due this calendar week — or already overdue, which is the most urgent
    /// thing the rail holds and belongs at the top, not in the tail.
    pub this_week: Vec<usize>,
    pub next_week: Vec<usize>,
    pub later: Vec<usize>,
}

/// Whether `task` shows on `day`.
///
/// A task surfaces on a day for either of two independent reasons: the user
/// planned it there (`scheduled_date`), or it is due there and has no planned
/// day — the deadline day is then its calendar home, as a single point marker.
/// A task with both surfaces ONCE, on its planned day, and announces its
/// deadline there; a second appearance on the deadline day read as a second
/// task. A finished task with no planned day belongs to the day it was
/// finished — but a finished task the user PLANNED onto a day keeps that day:
/// yesterday's dose of a daily task, ticked this morning, belongs to
/// yesterday, and moving it would make a medication log read as a skipped day
/// followed by a doubled one.
#[cfg(feature = "collation")]
fn shows_on(task: &DayTask, day: NaiveDate, input: &DayInput) -> bool {
    // A subtask surfaces only when it carries its own date — an undated
    // subtask travels with its parent and stays hidden. An EMPTY parent id is
    // no parent: the TypeScript this replaced tested the field for truthiness,
    // and a row with `""` here was a top-level task on every surface.
    let has_parent = task.parent_id.as_deref().is_some_and(|id| !id.is_empty());
    if has_parent && task.scheduled_date.is_none() && task.deadline_date.is_none() {
        return false;
    }
    if task.status == TaskStatus::Cancelled {
        return false;
    }
    if task.status == TaskStatus::Completed && !input.completed_visible.contains(&task.list_id) {
        return false;
    }
    let me = input
        .current_user_by_list
        .get(&task.list_id)
        .and_then(|user| user.as_ref());
    if !is_mine_or_unassigned(&task.assignees, me) {
        return false;
    }
    // Falls through when the completion day is missing (an adapter that does
    // not record one): the planned day is a worse answer than the right one
    // but a much better answer than none.
    if task.status == TaskStatus::Completed && task.scheduled_date.is_none() {
        if let Some(finished) = task.completed_day {
            return finished == day;
        }
    }
    if task.scheduled_date == Some(day) {
        return true;
    }
    task.scheduled_date.is_none() && task.deadline_date == Some(day)
}

/// The time `task` slots into on `day`, if any. See [`DayTaskRow::time`].
pub fn time_on_day(task: &DayTask, day: NaiveDate) -> Option<NaiveTime> {
    if task.scheduled_date == Some(day) {
        if let Some(time) = task.scheduled_time {
            return Some(time);
        }
    }
    if task.deadline_date == Some(day) {
        if let Some(time) = task.deadline_time {
            return Some(time);
        }
    }
    None
}

/// The end of `task`'s planned block on `day`, if any. See
/// [`DayTaskRow::end_time`].
pub fn end_time_on_day(task: &DayTask, day: NaiveDate) -> Option<NaiveTime> {
    if task.scheduled_date == Some(day) && task.scheduled_time.is_some() {
        return task.scheduled_end_time;
    }
    None
}

/// Whether `task` sits on `day` because of its deadline. See
/// [`DayTaskRow::deadline_chip`].
pub fn is_deadline_chip(task: &DayTask, day: NaiveDate) -> bool {
    task.deadline_date == Some(day) && task.scheduled_date != Some(day)
}

/// The tasks on each day asked, in display order — the same order as the task
/// list (priority band, then natural A→Z title), so a day's planned work reads
/// identically on every surface.
#[cfg(feature = "collation")]
pub fn tasks_on_days(input: &DayInput) -> BTreeMap<NaiveDate, Vec<DayTaskRow>> {
    let language = CollationLanguage::from_tag(&input.language);
    let mut out = BTreeMap::new();
    for &day in &input.days {
        let mut on_day: Vec<(usize, &DayTask)> = input
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| shows_on(task, day, input))
            .collect();
        on_day.sort_by(|(_, a), (_, b)| {
            task_order(
                a.priority,
                &a.title,
                b.priority,
                &b.title,
                input.scale,
                language,
            )
        });
        let rows = on_day
            .into_iter()
            .map(|(at, task)| DayTaskRow {
                task: at,
                time: time_on_day(task, day),
                end_time: end_time_on_day(task, day),
                deadline_chip: is_deadline_chip(task, day),
            })
            .collect();
        out.insert(day, rows);
    }
    out
}

/// The current and following CALENDAR week around `today`, as inclusive day
/// bounds.
///
/// Calendar weeks, not rolling windows: "this week" ends on the week's last
/// day however near that is, so a Friday deadline stops being "this week" the
/// moment the week turns — which is what a plan for the week means. Seven days
/// from now would keep sliding and never tell the user where a week ends.
///
/// `week_starts_on` is the user's own setting (0 = Sunday … 6 = Saturday), so
/// a Sunday week runs Sunday–Saturday and a Monday week Monday–Sunday.
pub fn backlog_weeks(today: NaiveDate, week_starts_on: u32) -> BacklogWeeks {
    // How far back the week's first day lies. The +7 keeps the result positive
    // for every combination of weekday and setting.
    let back = (today.weekday().num_days_from_sunday() + 7 - week_starts_on % 7) % 7;
    let start = today - Duration::days(i64::from(back));
    BacklogWeeks {
        this_week_start: start,
        this_week_end: start + Duration::days(6),
        next_week_start: start + Duration::days(7),
        next_week_end: start + Duration::days(13),
    }
}

/// Deadline-carrying items into this week, next week and everything after,
/// keeping the caller's order within each bucket. An item without a deadline
/// is in no bucket. A deadline that has already passed goes with THIS week:
/// it is the most urgent thing the rail holds, and burying last Tuesday's
/// deadline below everything else would be the one placement that helps
/// nobody.
pub fn split_deadlines_by_week(
    deadlines: &[Option<NaiveDate>],
    weeks: &BacklogWeeks,
) -> DeadlineSplit {
    let mut split = DeadlineSplit {
        this_week: Vec::new(),
        next_week: Vec::new(),
        later: Vec::new(),
    };
    for (at, due) in deadlines.iter().enumerate() {
        let Some(due) = due else {
            continue;
        };
        if *due <= weeks.this_week_end {
            split.this_week.push(at);
        } else if *due <= weeks.next_week_end {
            split.next_week.push(at);
        } else {
            split.later.push(at);
        }
    }
    split
}

/// [`tasks_on_days`] over the wire: a [`DayInput`] as JSON in, day → rows out.
#[cfg(feature = "collation")]
pub fn tasks_on_days_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: DayInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&tasks_on_days(&input))
}

/// [`backlog_weeks`] over the wire.
pub fn backlog_weeks_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: BacklogWeeksInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&backlog_weeks(input.today, input.week_starts_on))
}

/// [`split_deadlines_by_week`] over the wire.
pub fn split_deadlines_by_week_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: DeadlineSplitInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&split_deadlines_by_week(&input.deadlines, &input.weeks))
}

/// The contract with the TypeScript this replaced.
///
/// Its other half is `src/intl/taskDay.contract.test.ts`, reading this same
/// file. The fixture was written by running the TypeScript over the tables
/// BEFORE the port, so the port is measured against what was.
#[cfg(all(test, feature = "collation"))]
mod contract {
    use super::*;
    use serde_json::{json, Map, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/taskDay.json"
    ));

    fn merged(base: &Value, over: &Value) -> Value {
        let mut out = base.clone();
        let (Some(out_obj), Some(over_obj)) = (out.as_object_mut(), over.as_object()) else {
            panic!("base task and overrides are objects");
        };
        for (key, value) in over_obj {
            out_obj.insert(key.clone(), value.clone());
        }
        out
    }

    fn day_input(doc: &Value, case: &Value) -> DayInput {
        let base = &doc["baseTask"];
        let input = &case["input"];
        let tasks: Vec<Value> = input["tasks"]
            .as_array()
            .expect("tasks is an array")
            .iter()
            .map(|over| merged(base, over))
            .collect();
        let wire = json!({
            "tasks": tasks,
            "days": input["days"],
            "completedVisible": input.get("completedVisible").cloned().unwrap_or(json!([])),
            "currentUserByList": input.get("currentUserByList").cloned().unwrap_or(json!({})),
            "scale": input.get("scale").cloned().unwrap_or(json!("three")),
            "language": doc["language"],
        });
        serde_json::from_value(wire).expect("the case's input deserializes")
    }

    /// The rows as the fixture spells them: by id, not by position.
    fn by_id(input: &DayInput, rows: &[DayTaskRow]) -> Value {
        Value::Array(
            rows.iter()
                .map(|row| {
                    json!({
                        "id": input.tasks[row.task].id,
                        "time": row.time,
                        "endTime": row.end_time,
                        "deadlineChip": row.deadline_chip,
                    })
                })
                .collect(),
        )
    }

    #[test]
    fn every_day_case_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let cases = doc["onDay"].as_array().expect("onDay is an array");
        // Anti-silence: named rows, not a count.
        for needed in [
            "scheduled-and-due-shows-only-on-the-plan-day",
            "completed-without-a-plan-day-sits-on-its-completion-day",
            "completed-keeps-the-day-it-was-planned-for",
            "owner-filter-hides-a-colleagues-task",
            "time-scheduled-wins-over-deadline-on-the-same-day",
            "two-level-scale-has-two-bands",
        ] {
            assert!(
                cases.iter().any(|c| c["name"] == needed),
                "the contract lost `{needed}`"
            );
        }
        for case in cases {
            let name = case["name"].as_str().expect("every case has a name");
            let input = day_input(&doc, case);
            let answer = tasks_on_days(&input);
            let mut got = Map::new();
            for (day, rows) in &answer {
                got.insert(day.to_string(), by_id(&input, rows));
            }
            assert_eq!(
                Value::Object(got),
                case["expect"],
                "{name}: {}",
                case["note"].as_str().unwrap_or("")
            );
        }
    }

    #[test]
    fn every_week_case_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let cases = doc["weeks"].as_array().expect("weeks is an array");
        assert!(
            cases.iter().any(|c| c["name"] == "crosses-month-and-year"),
            "the contract lost the year-boundary week"
        );
        for case in cases {
            let name = case["name"].as_str().expect("every case has a name");
            let input: BacklogWeeksInput =
                serde_json::from_value(case["input"].clone()).expect("weeks input deserializes");
            let got = serde_json::to_value(backlog_weeks(input.today, input.week_starts_on))
                .expect("weeks serialize");
            assert_eq!(
                got,
                case["expect"],
                "{name}: {}",
                case["note"].as_str().unwrap_or("")
            );
        }
    }

    #[test]
    fn every_split_case_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let cases = doc["deadlineSplit"]
            .as_array()
            .expect("deadlineSplit is an array");
        assert!(
            cases
                .iter()
                .any(|c| c["name"] == "overdue-stays-with-this-week"),
            "the contract lost the overdue row"
        );
        for case in cases {
            let name = case["name"].as_str().expect("every case has a name");
            let items = case["input"]["deadlines"]
                .as_array()
                .expect("deadlines is an array");
            let ids: Vec<&str> = items
                .iter()
                .map(|i| i["id"].as_str().expect("id"))
                .collect();
            let deadlines: Vec<Option<NaiveDate>> = items
                .iter()
                .map(|i| {
                    serde_json::from_value(i["deadline_date"].clone()).expect("a date or null")
                })
                .collect();
            let weeks: BacklogWeeks =
                serde_json::from_value(case["input"]["weeks"].clone()).expect("weeks deserialize");
            let split = split_deadlines_by_week(&deadlines, &weeks);
            let name_them =
                |at: &[usize]| Value::Array(at.iter().map(|&i| json!(ids[i])).collect());
            let got = json!({
                "thisWeek": name_them(&split.this_week),
                "nextWeek": name_them(&split.next_week),
                "later": name_them(&split.later),
            });
            assert_eq!(
                got,
                case["expect"],
                "{name}: {}",
                case["note"].as_str().unwrap_or("")
            );
        }
    }

    #[test]
    fn the_wire_accepts_a_full_task_and_defaults_the_rest() {
        let input = r#"{
            "tasks": [{
                "id": "t", "list_id": "L", "title": "T", "description": null,
                "status": "open", "priority": "medium", "effort": "large",
                "scheduled_date": "2026-05-21", "scheduled_time": "09:00:00",
                "scheduled_end_time": null, "deadline_date": null, "deadline_time": null,
                "recurrence": null, "parent_id": null, "color_label": null,
                "reminders": [], "sound": null, "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z", "completed_at": null, "etag": null
            }],
            "days": ["2026-05-21"]
        }"#;
        let answer: BTreeMap<NaiveDate, Vec<DayTaskRow>> =
            serde_json::from_str(&tasks_on_days_json(input).expect("the input is valid"))
                .expect("rows round-trip");
        let day = NaiveDate::from_ymd_opt(2026, 5, 21).expect("a date");
        assert_eq!(
            answer[&day],
            vec![DayTaskRow {
                task: 0,
                time: NaiveTime::from_hms_opt(9, 0, 0),
                end_time: None,
                deadline_chip: false,
            }]
        );
    }
}
