//! What a task's state, effort and priority are announced with, and how far a
//! parent's subtasks are.
//!
//! Two of the answers `shared/taskStatus.ts` used to give in TypeScript. What
//! did NOT move is deliberate, and it is the decision of 2026-09-09: the core
//! answers a STATE or a KEY, never a character and never a sentence. The
//! glyphs (○ ◐ ● ⊘, ! !! !!! and ★) stay on each surface — an e-ink display
//! plausibly wants others — and so does every suffix built with `t`. What
//! moved is what every surface has to agree on: which i18n key a value is
//! announced with (DESIGN §4.5 a; `ConferenceProvider::i18n_key` is the
//! precedent), and the one arithmetic in the module.
//!
//! # Published once, not asked per chip
//!
//! The keys are constants, and the views ask for them per chip. A door per
//! call would cross the phone's native bridge once per row to learn a word it
//! could have read at startup, so the core publishes the whole table as
//! [`TaskI18nKeys`] and a surface reads it once when it installs its door.
//!
//! # Progress, for every parent at once
//!
//! The views ask "how far is this parent" per row, over the same task list.
//! The core answers for every parent in one crossing; the shell remembers the
//! answer per task array.
//!
//! Every answer here is pinned by `tests/fixtures/taskStatus.json`, measured by
//! running the TypeScript this replaces; the `contract` module below reads it,
//! and so does the TypeScript contract test on the other side of the boundary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::task_priority::PriorityScale;
use crate::types::{TaskEffort, TaskPriority, TaskStatus};

/// The i18n key a task's state is announced with. Every state has one.
pub fn status_i18n_key(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Open => "views.tasks.stateOpen",
        TaskStatus::InProgress => "views.tasks.stateInProgress",
        TaskStatus::Completed => "views.tasks.stateDone",
        TaskStatus::Cancelled => "views.tasks.stateCancelled",
    }
}

/// The i18n key a task's effort is announced with, or `None` for the neutral
/// middle — nothing to announce.
pub fn effort_i18n_key(effort: TaskEffort) -> Option<&'static str> {
    match effort {
        TaskEffort::Small => Some("views.tasks.effortSmall"),
        TaskEffort::Medium => None,
        TaskEffort::Large => Some("views.tasks.effortLarge"),
    }
}

/// The i18n key a task's priority is announced with under `scale`, or `None`
/// when there is nothing to announce: the middle of the three-level system,
/// and everything below the top in the two-level one. Two-level says
/// "important", not "high priority": in a system with one mark, naming a level
/// the user cannot choose between would describe a scale that is no longer
/// there.
pub fn priority_i18n_key(priority: TaskPriority, scale: PriorityScale) -> Option<&'static str> {
    match scale {
        PriorityScale::Two => priority
            .is_important()
            .then_some("views.tasks.priorityImportant"),
        PriorityScale::Three => match priority {
            TaskPriority::High => Some("views.tasks.priorityHigh"),
            TaskPriority::Medium => None,
            TaskPriority::Low => Some("views.tasks.priorityLow"),
        },
    }
}

/// The keys for every state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct StatusKeys {
    pub open: String,
    pub in_progress: String,
    pub completed: String,
    pub cancelled: String,
}

/// The keys for every effort; `None` where nothing is announced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct EffortKeys {
    pub small: Option<String>,
    pub medium: Option<String>,
    pub large: Option<String>,
}

/// The keys for every priority under one scale; `None` where nothing is
/// announced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct PriorityKeys {
    pub low: Option<String>,
    pub medium: Option<String>,
    pub high: Option<String>,
}

/// The priority keys per scale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct PriorityKeysByScale {
    pub three: PriorityKeys,
    pub two: PriorityKeys,
}

/// Every i18n key of the task vocabulary, as one table — what a surface reads
/// once when it installs its door.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TaskI18nKeys {
    pub status: StatusKeys,
    pub effort: EffortKeys,
    pub priority: PriorityKeysByScale,
}

fn priority_keys(scale: PriorityScale) -> PriorityKeys {
    let key = |p| priority_i18n_key(p, scale).map(str::to_string);
    PriorityKeys {
        low: key(TaskPriority::Low),
        medium: key(TaskPriority::Medium),
        high: key(TaskPriority::High),
    }
}

/// The whole table. See the module doc for why it is a table.
pub fn task_i18n_keys() -> TaskI18nKeys {
    TaskI18nKeys {
        status: StatusKeys {
            open: status_i18n_key(TaskStatus::Open).to_string(),
            in_progress: status_i18n_key(TaskStatus::InProgress).to_string(),
            completed: status_i18n_key(TaskStatus::Completed).to_string(),
            cancelled: status_i18n_key(TaskStatus::Cancelled).to_string(),
        },
        effort: EffortKeys {
            small: effort_i18n_key(TaskEffort::Small).map(str::to_string),
            medium: effort_i18n_key(TaskEffort::Medium).map(str::to_string),
            large: effort_i18n_key(TaskEffort::Large).map(str::to_string),
        },
        priority: PriorityKeysByScale {
            three: priority_keys(PriorityScale::Three),
            two: priority_keys(PriorityScale::Two),
        },
    }
}

/// [`task_i18n_keys`] over the wire.
pub fn task_i18n_keys_json() -> String {
    serde_json::to_string(&task_i18n_keys()).expect("a table of strings serializes")
}

/// What the progress reads of a task — by the same field names as `Task`, so
/// a full task's JSON deserializes into it.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ProgressTask {
    pub id: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// The question [`subtask_progress`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct SubtaskProgressInput {
    pub tasks: Vec<ProgressTask>,
}

/// How far a parent's subtasks are: completed direct children over direct
/// children that are not cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct SubtaskProgress {
    pub done: usize,
    pub total: usize,
}

/// Parent id → progress, for every parent that has children that count. A
/// parent absent from the answer has no children, or only cancelled ones:
/// nothing left to do and nothing done, no fraction to speak. Only DIRECT
/// children count — a grandchild is its own parent's business. Whether the
/// parent's own row is in the list does not matter; the count is by
/// `parent_id`.
pub fn subtask_progress(tasks: &[ProgressTask]) -> BTreeMap<String, SubtaskProgress> {
    let mut out: BTreeMap<String, SubtaskProgress> = BTreeMap::new();
    for task in tasks {
        let Some(parent) = task.parent_id.as_deref() else {
            continue;
        };
        if task.status == TaskStatus::Cancelled {
            continue;
        }
        let progress = out
            .entry(parent.to_string())
            .or_insert(SubtaskProgress { done: 0, total: 0 });
        progress.total += 1;
        if task.status == TaskStatus::Completed {
            progress.done += 1;
        }
    }
    out
}

/// [`subtask_progress`] over the wire.
pub fn subtask_progress_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: SubtaskProgressInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&subtask_progress(&input.tasks))
}

/// The contract with the TypeScript this replaced.
///
/// Its other half is `src/intl/taskStatus.contract.test.ts`, reading this
/// same file. The fixture was written by running the TypeScript BEFORE the
/// port, so the port is measured against what was.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/taskStatus.json"
    ));

    #[test]
    fn every_key_in_the_table_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        // Anti-silence: the rows the rules turn on, named.
        assert_eq!(
            doc["keys"]["priority"]["two"]["high"],
            "views.tasks.priorityImportant"
        );
        assert_eq!(doc["keys"]["priority"]["two"]["low"], Value::Null);
        assert_eq!(doc["keys"]["priority"]["three"]["medium"], Value::Null);
        assert_eq!(doc["keys"]["effort"]["medium"], Value::Null);
        let got = serde_json::to_value(task_i18n_keys()).expect("the table serializes");
        assert_eq!(got, doc["keys"]);
    }

    #[test]
    fn every_progress_case_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let cases = doc["progress"].as_array().expect("progress is an array");
        for needed in [
            "counts-completed-and-drops-cancelled-from-the-total",
            "direct-children-only",
            "only-cancelled-children-is-null",
        ] {
            assert!(
                cases.iter().any(|c| c["name"] == needed),
                "the contract lost `{needed}`"
            );
        }
        let base = &doc["baseTask"];
        for case in cases {
            let name = case["name"].as_str().expect("every case has a name");
            let tasks: Vec<ProgressTask> = case["input"]["tasks"]
                .as_array()
                .expect("tasks is an array")
                .iter()
                .map(|over| {
                    let mut row = base.clone();
                    for (k, v) in over.as_object().expect("an override object") {
                        row[k] = v.clone();
                    }
                    serde_json::from_value(row).expect("a task row deserializes")
                })
                .collect();
            let answer = subtask_progress(&tasks);
            let mut got = serde_json::Map::new();
            for parent in case["input"]["parents"]
                .as_array()
                .expect("parents is an array")
            {
                let parent = parent.as_str().expect("a parent id");
                got.insert(
                    parent.to_string(),
                    answer
                        .get(parent)
                        .map(|p| json!({ "done": p.done, "total": p.total }))
                        .unwrap_or(Value::Null),
                );
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
    fn the_wire_accepts_a_full_task() {
        let input = r#"{"tasks": [
            {"id": "p", "list_id": "L", "title": "P", "status": "open", "priority": "medium",
             "effort": "medium", "parent_id": null, "assignees": [], "reminders": [],
             "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"},
            {"id": "c", "list_id": "L", "title": "C", "status": "completed", "priority": "medium",
             "parent_id": "p"}
        ]}"#;
        let answer: BTreeMap<String, SubtaskProgress> =
            serde_json::from_str(&subtask_progress_json(input).expect("the input is valid"))
                .expect("the answer round-trips");
        assert_eq!(answer["p"], SubtaskProgress { done: 1, total: 1 });
        assert!(!task_i18n_keys_json().is_empty());
    }
}
