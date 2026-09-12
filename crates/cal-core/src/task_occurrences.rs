//! The recurring-task projection: which occurrences a recurring scheduled
//! task shows inside a window, one step of the same walk, and what "move to
//! this day" can be on a source that owns the date.
//!
//! Was `shared/expandTaskOccurrences.ts`, run on both calendar surfaces and
//! by the widget snapshot while rendering. A recurring dated task shows on
//! EVERY planned day of the window, like a recurring event, not only on its
//! single current `scheduled_date`; the days it shows on have to be the same
//! on the phone and on the desktop, and they have to match the instances the
//! backend spawner ([`crate::spawn`]) will actually create — so the walk lives
//! here, once, on the spawner's own stepping, and each surface asks it through
//! its door.
//!
//! # Positions and days, never rows
//!
//! The answer is a list of [`OccurrenceRow`]s: which input task, on which
//! day, real or projected. The occurrence on the task's own `scheduled_date`
//! is the REAL, interactive task; every other one is a read-only projection.
//! The shell builds the projected copy and its id (`<id> occ <day>`) and
//! decodes it back — the encoding is the surface's (DESIGN §4.5 a), the core
//! never echoes a row.
//!
//! # What can be projected
//!
//! Only a DETERMINISTIC, DATED, SCHEDULE-placement rule on a non-terminal
//! task: anchor `from_date` (a `from_completion` rule's future dates depend on
//! when each turn is checked off), placement `schedule` (a backlog rule's next
//! turn is undated), a `scheduled_date` as the base, and a status that is not
//! completed or cancelled (a terminal instance's future days belong to the
//! next open instance the backend spawns). Everything else passes through
//! unchanged, in place, with its own day.
//!
//! # The projector's reading of a rule
//!
//! The spawner and the projector step the same way ([`crate::spawn::advance`])
//! with one pinned difference: the projector DROPS an invalid fixed date (day
//! 0 or 32, month 13) and, with none left, walks by the frequency — the
//! TypeScript sanitised the rule before it looked — while the spawner clamps
//! the day. A day of month outside 1..=31 is likewise no day of month. Pinned
//! row by row in `tests/fixtures/taskOccurrences.json`, measured from the
//! TypeScript this replaces; the `contract` module below reads it, and so does
//! the TypeScript contract test on the other side of the boundary.
//!
//! # Bounds
//!
//! `max_per_task` (default 400) caps the RENDERED occurrences of one task; a
//! separate step cap of 100 000 bounds the walk itself, so a base far before
//! the window still arrives (an old, never-completed daily task keeps its
//! original date — only completion advances it) and a base absurdly far back
//! still terminates. A count end (`after N`) does not end the walk here — only
//! a date bound does; the count is the provider's, and the spawner does the
//! same.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::spawn::{next_trigger, recurrence_ended};
use crate::types::{RecurrenceAnchor, RecurrencePlacement, TaskRecurrence, TaskStatus};

/// What the projector reads of a task — by the same field names as `Task`,
/// so a full task's JSON deserializes into it.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct OccurrenceTask {
    pub status: TaskStatus,
    #[serde(default)]
    pub scheduled_date: Option<NaiveDate>,
    #[serde(default)]
    pub recurrence: Option<TaskRecurrence>,
}

/// The question [`expand_task_occurrences`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct OccurrenceInput {
    pub tasks: Vec<OccurrenceTask>,
    /// The window, both ends inclusive.
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Cap on the rendered occurrences of one task; absent = 400.
    #[serde(default)]
    pub max_per_task: Option<usize>,
}

/// One occurrence: which input task, on which day, real or projected. A
/// pass-through row carries the task's own day, which may be none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct OccurrenceRow {
    pub task: usize,
    pub day: Option<NaiveDate>,
    pub projection: bool,
}

/// The question [`next_task_occurrence`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct NextOccurrenceInput {
    pub scheduled: NaiveDate,
    #[serde(default)]
    pub rule: Option<TaskRecurrence>,
}

/// The question [`occurrence_move_target`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct MoveTargetInput {
    #[serde(default)]
    pub scheduled_date: Option<NaiveDate>,
    #[serde(default)]
    pub recurrence: Option<TaskRecurrence>,
    /// The day asked for; none = clearing the date.
    #[serde(default)]
    pub date: Option<NaiveDate>,
    /// Whether the task's source can store an arbitrary day on a repeating
    /// task (most can; iOS Reminders cannot — there the date is the series
    /// anchor).
    pub can_reschedule_occurrence: bool,
}

/// What the move can be: the day to write, and whether it is the series
/// advanced by one step rather than the day that was asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct MoveTarget {
    pub date: Option<NaiveDate>,
    pub advanced: bool,
}

/// Rendered occurrences per task when the caller names no cap.
pub const DEFAULT_MAX_PER_TASK: usize = 400;

/// Hard cap on the walk of one task (advance steps), independent of how many
/// occurrences are emitted — termination even for a base very far from the
/// window. 100k daily steps ≈ 270 years, far beyond any real gap.
const MAX_STEPS: usize = 100_000;

/// The rule as the projector reads it (see the module doc): invalid fixed
/// dates dropped, and none left means none; a day of month outside 1..=31
/// means none.
fn projector_rule(rule: &TaskRecurrence) -> TaskRecurrence {
    let mut r = rule.clone();
    r.day_of_month = r.day_of_month.filter(|d| (1..=31).contains(d));
    r.fixed_dates = r
        .fixed_dates
        .as_ref()
        .map(|dates| {
            dates
                .iter()
                .copied()
                .filter(|md| (1..=12).contains(&md.month) && (1..=31).contains(&md.day))
                .collect::<Vec<_>>()
        })
        .filter(|dates| !dates.is_empty());
    r
}

/// The rule IF this task can be projected (module doc: deterministic, dated,
/// scheduled, not terminal), read the projector's way.
fn expandable(task: &OccurrenceTask) -> Option<TaskRecurrence> {
    if matches!(task.status, TaskStatus::Completed | TaskStatus::Cancelled) {
        return None;
    }
    let rule = task.recurrence.as_ref()?;
    if rule.anchor != RecurrenceAnchor::FromDate || rule.placement != RecurrencePlacement::Schedule
    {
        return None;
    }
    Some(projector_rule(rule))
}

/// The occurrences of every task inside `[from, to]`, in emission order: for
/// each expandable task its occurrences in the window, the base day as the
/// real task; every other task passes through in place. A base after the
/// window is absent (the walk only goes forward); a task whose bound ended
/// before the window is absent too.
pub fn expand_task_occurrences(
    tasks: &[OccurrenceTask],
    from: NaiveDate,
    to: NaiveDate,
    max_per_task: Option<usize>,
) -> Vec<OccurrenceRow> {
    let cap = max_per_task.unwrap_or(DEFAULT_MAX_PER_TASK);
    let mut out = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        let (Some(rule), Some(base)) = (expandable(task), task.scheduled_date) else {
            out.push(OccurrenceRow {
                task: index,
                day: task.scheduled_date,
                projection: false,
            });
            continue;
        };
        // `emitted` bounds the RENDERED occurrences; `steps` bounds the walk.
        let mut date = base;
        let mut emitted = 0usize;
        let mut steps = 0usize;
        while date <= to && emitted < cap && steps < MAX_STEPS {
            // A date bound is monotonic: once past it nothing later can emit.
            if recurrence_ended(&rule, date) {
                break;
            }
            if date >= from {
                out.push(OccurrenceRow {
                    task: index,
                    day: Some(date),
                    projection: date != base,
                });
                emitted += 1;
            }
            let Some(next) = next_trigger(date, &rule) else {
                break;
            };
            if next == date {
                break; // guard against a non-advancing rule
            }
            date = next;
            steps += 1;
        }
    }
    out
}

/// [`expand_task_occurrences`] over the wire.
pub fn expand_task_occurrences_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: OccurrenceInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&expand_task_occurrences(
        &input.tasks,
        input.from,
        input.to,
        input.max_per_task,
    ))
}

/// The next occurrence strictly after `scheduled` for a repeating task, or
/// none when there is no rule, the rule has no next step, or the series has
/// run past its date bound. One step of the same walk the projector uses.
pub fn next_task_occurrence(
    scheduled: NaiveDate,
    rule: Option<&TaskRecurrence>,
) -> Option<NaiveDate> {
    let rule = projector_rule(rule?);
    let next = next_trigger(scheduled, &rule)?;
    if next == scheduled {
        return None; // non-advancing rule guard
    }
    if recurrence_ended(&rule, next) {
        return None;
    }
    Some(next)
}

/// [`next_task_occurrence`] over the wire; the answer is the day or `null`.
pub fn next_task_occurrence_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: NextOccurrenceInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&next_task_occurrence(input.scheduled, input.rule.as_ref()))
}

/// What "move this task to `date`" can actually be, given what its source
/// supports. Most backends store the due date on the task, so the answer is
/// the day asked for. Where the source owns the date of a repeating task (iOS
/// Reminders: the date is the series anchor, and an arbitrary day does not
/// survive the round trip), the honest equivalent is the series advanced by
/// one step — the move "skip this occurrence" already makes — and `advanced`
/// says so, so the caller can tell the user. Clearing the date, a task that
/// does not repeat, an undated one, or a series with no later turn: the
/// requested day, unchanged — failing to move is better than moving somewhere
/// invented.
pub fn occurrence_move_target(
    scheduled_date: Option<NaiveDate>,
    recurrence: Option<&TaskRecurrence>,
    date: Option<NaiveDate>,
    can_reschedule_occurrence: bool,
) -> MoveTarget {
    let as_asked = MoveTarget {
        date,
        advanced: false,
    };
    if date.is_none() || can_reschedule_occurrence {
        return as_asked;
    }
    let (Some(scheduled), Some(rule)) = (scheduled_date, recurrence) else {
        return as_asked;
    };
    match next_task_occurrence(scheduled, Some(rule)) {
        Some(next) => MoveTarget {
            date: Some(next),
            advanced: true,
        },
        None => as_asked,
    }
}

/// [`occurrence_move_target`] over the wire.
pub fn occurrence_move_target_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: MoveTargetInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&occurrence_move_target(
        input.scheduled_date,
        input.recurrence.as_ref(),
        input.date,
        input.can_reschedule_occurrence,
    ))
}

/// The contract with the TypeScript this replaced.
///
/// Its other half is `src/intl/expandTaskOccurrences.contract.test.ts`,
/// reading this same file through the doors. The fixture was written by
/// running the TypeScript BEFORE the port, so the port is measured against
/// what was. The shell reads an empty `scheduled_date` as no date before it
/// crosses; this test does the same to the fixture's rows, so both sides see
/// the same question.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/taskOccurrences.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the contract parses")
    }

    /// A task row over the base task, with the shell's one normalisation.
    fn task(over: &Value, base: &Value) -> Value {
        let mut row = base.clone();
        for (k, v) in over.as_object().expect("an override object") {
            row[k] = v.clone();
        }
        if row["scheduled_date"] == "" {
            row["scheduled_date"] = Value::Null;
        }
        row
    }

    fn date(v: &Value) -> NaiveDate {
        serde_json::from_value(v.clone()).expect("a YYYY-MM-DD day")
    }

    fn has_row(cases: &[Value], name: &str) {
        assert!(
            cases.iter().any(|c| c["name"] == name),
            "the contract lost `{name}`"
        );
    }

    #[test]
    fn every_expansion_holds() {
        let doc = doc();
        let base = &doc["baseTask"];
        let cases = doc["expand"].as_array().expect("expand is an array");
        for needed in [
            "daily-projects-every-day-in-the-window",
            "the-real-task-is-the-one-on-its-own-day",
            "weekly-by-day-projects-only-the-listed-weekdays",
            "interval-with-named-weekdays-is-not-dropped",
            "monthly-day-of-month-clamps-to-short-months",
            "far-past-daily-base-reaches-the-window",
            "stops-at-the-until-bound",
            "completed-passes-through",
            "from-completion-passes-through",
            "a-fixed-date-with-day-32-is-dropped",
            "a-base-beyond-the-step-cap-emits-nothing",
            "an-empty-scheduled-date-is-undated",
        ] {
            has_row(cases, needed);
        }
        for case in cases {
            let tasks: Vec<OccurrenceTask> = case["input"]["tasks"]
                .as_array()
                .expect("tasks is an array")
                .iter()
                .map(|over| {
                    serde_json::from_value(task(over, base)).expect("a task row deserializes")
                })
                .collect();
            let got = expand_task_occurrences(
                &tasks,
                date(&case["input"]["from"]),
                date(&case["input"]["to"]),
                case["input"]["maxPerTask"].as_u64().map(|n| n as usize),
            );
            assert_eq!(
                json!(got),
                case["expect"],
                "{}: {}",
                case["name"],
                case["note"].as_str().unwrap_or("")
            );
        }
    }

    #[test]
    fn every_step_holds() {
        let doc = doc();
        let cases = doc["next"].as_array().expect("next is an array");
        has_row(cases, "past-an-until-end-is-null");
        has_row(cases, "all-invalid-fixed-dates-fall-back-to-the-frequency");
        for case in cases {
            let rule: Option<TaskRecurrence> =
                serde_json::from_value(case["input"]["rule"].clone()).expect("a rule or null");
            let got = next_task_occurrence(date(&case["input"]["scheduled"]), rule.as_ref());
            assert_eq!(json!(got), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_move_holds() {
        let doc = doc();
        let base = &doc["baseTask"];
        let cases = doc["move"].as_array().expect("move is an array");
        has_row(cases, "advances-the-series-where-the-source-owns-the-date");
        has_row(cases, "an-ended-series-keeps-the-requested-day");
        for case in cases {
            let row = task(&case["input"]["task"], base);
            let t: OccurrenceTask = serde_json::from_value(row).expect("a task row deserializes");
            let asked: Option<NaiveDate> =
                serde_json::from_value(case["input"]["dateKey"].clone()).expect("a day or null");
            let got = occurrence_move_target(
                t.scheduled_date,
                t.recurrence.as_ref(),
                asked,
                case["input"]["canRescheduleOccurrence"]
                    .as_bool()
                    .expect("a bool"),
            );
            assert_eq!(json!(got), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn the_wire_accepts_a_full_task_and_defaults_the_cap() {
        let input = r#"{"tasks": [
            {"id": "t", "list_id": "L", "title": "T", "status": "open", "priority": "medium",
             "effort": "medium", "scheduled_date": "2026-05-01", "scheduled_time": "09:00",
             "recurrence": {"frequency": "daily", "interval": 1, "day_of_week": null,
                            "day_of_month": null, "end": {"type": "never"}},
             "assignees": [], "reminders": []}
        ], "from": "2026-05-01", "to": "2026-05-03"}"#;
        let rows: Vec<OccurrenceRow> =
            serde_json::from_str(&expand_task_occurrences_json(input).expect("valid"))
                .expect("the answer round-trips");
        assert_eq!(rows.len(), 3);
        assert!(!rows[0].projection && rows[1].projection);
        assert_eq!(
            next_task_occurrence_json(r#"{"scheduled": "2026-05-01"}"#).expect("valid"),
            "null"
        );
        assert_eq!(
            occurrence_move_target_json(
                r#"{"scheduled_date": null, "recurrence": null, "date": "2026-05-02",
                    "can_reschedule_occurrence": false}"#
            )
            .expect("valid"),
            r#"{"date":"2026-05-02","advanced":false}"#
        );
    }
}
