//! The parent/subtask status coupling: what a parent is, given its children,
//! and which writes a status change plans — the root, the descendants that
//! follow, the ancestors re-derived — in the order the caller applies them.
//!
//! Was `shared/taskCascade.ts`, run on both surfaces when a task is checked
//! off. A check-off on the phone and the same check-off on the desktop have to
//! plan the same writes in the same order, and a frontend that is not
//! JavaScript has to plan them too; so the planner lives here, once, and each
//! surface asks it through its door. The answer is WRITES — ids and states,
//! never wording (DESIGN §4.5 a) — and the core reads no clock: today travels
//! in as [`CascadeOptions::today`] (`tests/core_contracts.rs`). Whether it
//! travels at all — the auto-date setting, a provider that cannot hold
//! `in_progress`, a per-list rule — is the surface's decision, as is applying
//! the writes and announcing the outcome.
//!
//! # Two directions
//!
//! **Up** — every time a child's status changes, the parent's status is
//! re-derived from the *combined* state of all its children
//! ([`derive_status_from_children`]), and the re-derivation climbs: a changed
//! parent re-derives its own parent, up to the root — and stops at the first
//! ancestor that does not change.
//!
//! **Down** — when a parent flips to `completed` or `cancelled`, the
//! descendants follow, respecting prior decisions: a cancelled descendant of a
//! completed parent stays cancelled (the user dropped it), a completed
//! descendant of a cancelled parent stays completed (the work is done). Moves
//! to `open` or `in_progress` cascade nowhere; the up-rule re-derives the
//! parent on the next child change.
//!
//! # The order is the contract
//!
//! The callers apply the writes one by one against a provider, with no
//! transaction behind them (DESIGN §9.1, "alles versuchen, dann berichten").
//! The root comes first, then the descendants in a stack walk (both children
//! of a node, then the LAST child's subtree, then the first's), then the
//! ancestors nearest first. Pinned row by row in
//! `tests/fixtures/taskCascade.json`, measured from the TypeScript this
//! replaces; the `contract` module below reads it, and so does the TypeScript
//! contract test on the other side of the boundary.
//!
//! # What the TypeScript did with empty strings, kept
//!
//! Three JS truthiness tests became rules and are pinned as they were: an
//! empty `parent_id` is no parent (in both directions), an empty today key is
//! no today, and an empty `scheduled_date` counts as dated. The shell sends
//! `null` for the first two anyway; the core reads them the same way so the
//! answer does not depend on which side normalised.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::types::TaskStatus;

/// What the planners read of a task — by the same field names as `Task`, so a
/// full task's JSON deserializes into it. `scheduled_date` stays text: only
/// its presence matters here, and the core never compares it.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct CascadeTask {
    pub id: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub scheduled_date: Option<String>,
}

/// How the planners behave, decided by the surface.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct CascadeOptions {
    /// The parent/subtask coupling (Settings → Tasks); off degrades
    /// [`plan_status_cascade`] to the single root write and
    /// [`plan_ancestor_recompute`] to nothing. Absent = on.
    #[serde(default = "coupling_on")]
    pub cascade_enabled: bool,
    /// Today as `YYYY-MM-DD`, present only when the "started → pin to today"
    /// rule applies: every write that moves a dateless task into
    /// `in_progress` then carries it as [`StatusWrite::scheduled_date`].
    /// Absent = the rule is off (the setting, or a provider that cannot hold
    /// `in_progress`). Independent of the coupling.
    #[serde(default)]
    pub today: Option<String>,
}

fn coupling_on() -> bool {
    true
}

impl Default for CascadeOptions {
    fn default() -> Self {
        Self {
            cascade_enabled: true,
            today: None,
        }
    }
}

/// The question [`plan_status_cascade`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct CascadeInput {
    pub tasks: Vec<CascadeTask>,
    /// The task whose status changes.
    pub task_id: String,
    /// What it changes to.
    pub status: TaskStatus,
    #[serde(default)]
    pub options: CascadeOptions,
}

/// The question [`plan_ancestor_recompute`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct RecomputeInput {
    pub tasks: Vec<CascadeTask>,
    /// The parent to re-derive first; the climb continues above it.
    pub parent_id: String,
    #[serde(default)]
    pub options: CascadeOptions,
}

/// The question [`auto_date_on_start`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AutoDateInput {
    pub status: TaskStatus,
    #[serde(default)]
    pub scheduled_date: Option<String>,
    #[serde(default)]
    pub today: Option<String>,
}

/// One write the caller applies: `task_id` becomes `status`; when
/// `scheduled_date` is set, the task's scheduled day becomes it too — the
/// "started → pin to today" companion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct StatusWrite {
    pub task_id: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub scheduled_date: Option<String>,
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.is_empty())
}

/// The "started → pin to today" rule: the day to pin when `status` moves a
/// task into `in_progress`, a today is given, and the task has no scheduled
/// day; `None` = leave the date alone. Shared by both planners and by the
/// task editor's own root write — both mean "the task is now being worked on".
pub fn auto_date_on_start(
    status: TaskStatus,
    scheduled_date: Option<&str>,
    today: Option<&str>,
) -> Option<String> {
    if status != TaskStatus::InProgress {
        return None;
    }
    let today = non_empty(today)?;
    if scheduled_date.is_some() {
        return None;
    }
    Some(today.to_string())
}

/// [`auto_date_on_start`] over the wire; the answer is the day or `null`.
pub fn auto_date_on_start_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: AutoDateInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&auto_date_on_start(
        input.status,
        input.scheduled_date.as_deref(),
        input.today.as_deref(),
    ))
}

/// A parent's status, derived from its children; `None` when there are none
/// (no derivation possible — the caller keeps what the parent has).
///
/// Presence decides, never count:
/// - any child in progress → in progress;
/// - else any completed AND any open → in progress (started, not finished);
/// - else no open: all completed or cancelled → completed if any completed,
///   cancelled if all cancelled (a cancelled child does not keep the parent
///   unfinished — the user walked away from it);
/// - else open (only open, or open plus cancelled: intent dropped, work not
///   started).
pub fn derive_status_from_children(
    children: impl IntoIterator<Item = TaskStatus>,
) -> Option<TaskStatus> {
    let mut any = false;
    let mut has_in_progress = false;
    let mut has_open = false;
    let mut has_completed = false;
    for status in children {
        any = true;
        match status {
            TaskStatus::InProgress => has_in_progress = true,
            TaskStatus::Open => has_open = true,
            TaskStatus::Completed => has_completed = true,
            TaskStatus::Cancelled => {}
        }
    }
    if !any {
        return None;
    }
    if has_in_progress {
        return Some(TaskStatus::InProgress);
    }
    if has_completed && has_open {
        return Some(TaskStatus::InProgress);
    }
    if !has_open {
        return Some(if has_completed {
            TaskStatus::Completed
        } else {
            TaskStatus::Cancelled
        });
    }
    Some(TaskStatus::Open)
}

/// The rows, indexed: by id (the last row wins a duplicate id, as a JS `Map`
/// built from the entries did), and children by parent in row order. An
/// empty parent id is no parent.
struct Rows<'a> {
    by_id: HashMap<&'a str, &'a CascadeTask>,
    children: HashMap<&'a str, Vec<&'a CascadeTask>>,
}

impl<'a> Rows<'a> {
    fn index(tasks: &'a [CascadeTask]) -> Self {
        let mut by_id = HashMap::new();
        let mut children: HashMap<&str, Vec<&CascadeTask>> = HashMap::new();
        for task in tasks {
            by_id.insert(task.id.as_str(), task);
            if let Some(parent) = non_empty(task.parent_id.as_deref()) {
                children.entry(parent).or_default().push(task);
            }
        }
        Self { by_id, children }
    }

    fn children_of(&self, id: &str) -> &[&'a CascadeTask] {
        self.children.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn parent_of(&self, task: &'a CascadeTask) -> Option<&'a str> {
        non_empty(task.parent_id.as_deref())
    }
}

/// A write, with the companion date when the rule fires for `target`.
fn write(
    task_id: &str,
    status: TaskStatus,
    target: Option<&CascadeTask>,
    options: &CascadeOptions,
) -> StatusWrite {
    let scheduled_date = target.and_then(|t| {
        auto_date_on_start(
            status,
            t.scheduled_date.as_deref(),
            options.today.as_deref(),
        )
    });
    StatusWrite {
        task_id: task_id.to_string(),
        status,
        scheduled_date,
    }
}

/// Every write a single status change plans: `task_id` becomes `status`;
/// descendants follow (down-rule), ancestors are re-derived (up-rule). The
/// list starts with the root, then the descendants, then the ancestors nearest
/// first; writes that would change nothing are left out. With the coupling
/// off, only the root write remains (or nothing, when the status already
/// matches); the auto-date rule still applies to it. A root that is not in
/// the list still gets its write (nothing to compare it with), without a date,
/// and its children in the list still follow.
pub fn plan_status_cascade(
    tasks: &[CascadeTask],
    task_id: &str,
    status: TaskStatus,
    options: &CascadeOptions,
) -> Vec<StatusWrite> {
    let rows = Rows::index(tasks);
    let root = rows.by_id.get(task_id).copied();

    if !options.cascade_enabled {
        return match root {
            Some(r) if r.status != status => vec![write(task_id, status, Some(r), options)],
            _ => Vec::new(),
        };
    }

    let mut writes = Vec::new();
    // The running snapshot of statuses as the cascade decides them, so the
    // up-rule sees what the down-rule already changed.
    let mut overrides: HashMap<&str, TaskStatus> = HashMap::new();
    let status_of = |overrides: &HashMap<&str, TaskStatus>, id: &str| -> Option<TaskStatus> {
        overrides
            .get(id)
            .copied()
            .or_else(|| rows.by_id.get(id).map(|t| t.status))
    };

    if root.map(|r| r.status) != Some(status) {
        writes.push(write(task_id, status, root, options));
        overrides.insert(task_id, status);
    }

    if matches!(status, TaskStatus::Completed | TaskStatus::Cancelled) {
        let target = status;
        let opposite = if target == TaskStatus::Completed {
            TaskStatus::Cancelled
        } else {
            TaskStatus::Completed
        };
        // External providers can deliver a parent CYCLE (two tasks each
        // carrying a relation onto the other): every node is expanded once,
        // or the walk re-pushes the cycle members forever.
        let mut stack: Vec<&str> = vec![task_id];
        let mut expanded: HashSet<&str> = HashSet::from([task_id]);
        while let Some(id) = stack.pop() {
            for kid in rows.children_of(id) {
                let kid_id = kid.id.as_str();
                if !expanded.insert(kid_id) {
                    continue;
                }
                let kid_status = status_of(&overrides, kid_id);
                if kid_status == Some(opposite) {
                    continue;
                }
                if kid_status == Some(target) {
                    stack.push(kid_id);
                    continue;
                }
                // A descendant's write never carries a date: the target is
                // terminal, and the rule fires for `in_progress` only.
                writes.push(write(kid_id, target, None, options));
                overrides.insert(kid_id, target);
                stack.push(kid_id);
            }
        }
    }

    // Up: re-derive each ancestor against its children with the overrides
    // applied, and stop at the first one that does not change.
    let mut current = root;
    let mut climbed: HashSet<&str> = HashSet::from([task_id]);
    while let Some(cur) = current {
        let Some(parent_id) = rows.parent_of(cur) else {
            break;
        };
        if !climbed.insert(parent_id) {
            break; // parent cycle — stop the climb
        }
        let Some(parent) = rows.by_id.get(parent_id).copied() else {
            break; // orphan — stop
        };
        let derived = derive_status_from_children(
            rows.children_of(parent_id)
                .iter()
                .map(|s| overrides.get(s.id.as_str()).copied().unwrap_or(s.status)),
        );
        let Some(derived) = derived else {
            break;
        };
        if status_of(&overrides, parent_id) == Some(derived) {
            break; // no change → no further propagation
        }
        writes.push(write(parent_id, derived, Some(parent), options));
        overrides.insert(parent_id, derived);
        current = Some(parent);
    }

    writes
}

/// [`plan_status_cascade`] over the wire.
pub fn plan_status_cascade_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: CascadeInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&plan_status_cascade(
        &input.tasks,
        &input.task_id,
        input.status,
        &input.options,
    ))
}

/// The ancestors re-derived after a non-status change — a subtask created or
/// deleted — starting at `parent_id` and climbing to the root. The up-half of
/// [`plan_status_cascade`] with one deliberate difference: an ancestor that
/// does not change does NOT end the climb; the level above it is re-derived
/// as well. Nothing with the coupling off, nothing for a parent that is not in
/// the list or has no children.
pub fn plan_ancestor_recompute(
    tasks: &[CascadeTask],
    parent_id: &str,
    options: &CascadeOptions,
) -> Vec<StatusWrite> {
    if !options.cascade_enabled {
        return Vec::new();
    }
    let rows = Rows::index(tasks);
    let mut writes = Vec::new();
    let mut overrides: HashMap<&str, TaskStatus> = HashMap::new();
    let mut current_id = non_empty(Some(parent_id));
    let mut climbed: HashSet<&str> = HashSet::new();
    while let Some(id) = current_id {
        if !climbed.insert(id) {
            break; // parent cycle — stop the climb
        }
        let Some(current) = rows.by_id.get(id).copied() else {
            break;
        };
        let derived = derive_status_from_children(
            rows.children_of(id)
                .iter()
                .map(|s| overrides.get(s.id.as_str()).copied().unwrap_or(s.status)),
        );
        let Some(derived) = derived else {
            break;
        };
        let effective = overrides.get(id).copied().unwrap_or(current.status);
        if effective != derived {
            writes.push(write(id, derived, Some(current), options));
            overrides.insert(id, derived);
        }
        current_id = rows.parent_of(current);
    }
    writes
}

/// [`plan_ancestor_recompute`] over the wire.
pub fn plan_ancestor_recompute_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: RecomputeInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&plan_ancestor_recompute(
        &input.tasks,
        &input.parent_id,
        &input.options,
    ))
}

/// The contract with the TypeScript this replaced.
///
/// Its other half is `src/state/taskCascade.contract.test.ts`, reading this
/// same file through the doors. The fixture was written by running the
/// TypeScript BEFORE the port, so the port is measured against what was; it
/// spells writes and options the way that TypeScript did (`taskId`,
/// `scheduledDate`, `cascadeEnabled`, `todayKey`), and this test translates.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/taskCascade.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the contract parses")
    }

    fn status(v: &Value) -> TaskStatus {
        serde_json::from_value(v.clone()).expect("a task status")
    }

    fn tasks(case: &Value, base: &Value) -> Vec<CascadeTask> {
        case["input"]["tasks"]
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
            .collect()
    }

    fn options(case: &Value) -> CascadeOptions {
        let o = &case["input"]["options"];
        CascadeOptions {
            cascade_enabled: o["cascadeEnabled"].as_bool().unwrap_or(true),
            today: o["todayKey"].as_str().map(str::to_string),
        }
    }

    fn spelled(writes: &[StatusWrite]) -> Value {
        Value::Array(
            writes
                .iter()
                .map(|w| {
                    let mut v = json!({ "taskId": w.task_id, "status": w.status });
                    if let Some(d) = &w.scheduled_date {
                        v["scheduledDate"] = json!(d);
                    }
                    v
                })
                .collect(),
        )
    }

    fn has_row(cases: &[Value], name: &str) {
        assert!(
            cases.iter().any(|c| c["name"] == name),
            "the contract lost `{name}`"
        );
    }

    #[test]
    fn every_auto_date_row_holds() {
        let doc = doc();
        let cases = doc["autoDate"].as_array().expect("autoDate is an array");
        has_row(cases, "a-dateless-task-entering-in-progress-gets-today");
        has_row(cases, "an-empty-scheduled-date-counts-as-dated");
        for case in cases {
            let i = &case["input"];
            let got = auto_date_on_start(
                status(&i["newStatus"]),
                i["currentScheduledDate"].as_str(),
                i["todayKey"].as_str(),
            );
            assert_eq!(json!(got), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_derivation_holds() {
        let doc = doc();
        let cases = doc["derive"].as_array().expect("derive is an array");
        // Anti-silence: the rows the derivation turns on.
        for (name, want) in [
            ("presence-completed+cancelled", "completed"),
            ("presence-open+cancelled", "open"),
            ("presence-open+completed", "in_progress"),
        ] {
            let case = cases
                .iter()
                .find(|c| c["name"] == name)
                .unwrap_or_else(|| panic!("the contract lost `{name}`"));
            assert_eq!(case["expect"], want);
        }
        for case in cases {
            let children = case["input"]["children"]
                .as_array()
                .expect("children is an array")
                .iter()
                .map(status);
            let got = derive_status_from_children(children);
            assert_eq!(json!(got), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_cascade_case_holds() {
        let doc = doc();
        let base = &doc["baseTask"];
        let cases = doc["cascade"].as_array().expect("cascade is an array");
        for needed in [
            "completing-a-parent-follows-every-non-cancelled-descendant",
            "descendants-come-depth-first-last-child-first",
            "the-climb-stops-where-nothing-changes",
            "an-empty-parent-id-is-no-parent-on-the-way-up",
            "an-empty-parent-id-is-no-parent-on-the-way-down",
            "cycle-down-two-tasks",
            "autodate-also-pins-a-dateless-parent-derived-to-in-progress",
            "decoupled-no-up-cascade",
            "a-root-absent-from-the-list-still-gets-its-write",
        ] {
            has_row(cases, needed);
        }
        for case in cases {
            let rows = tasks(case, base);
            let got = plan_status_cascade(
                &rows,
                case["input"]["taskId"].as_str().expect("a task id"),
                status(&case["input"]["newStatus"]),
                &options(case),
            );
            assert_eq!(
                spelled(&got),
                case["expect"],
                "{}: {}",
                case["name"],
                case["note"].as_str().unwrap_or("")
            );
        }
    }

    #[test]
    fn every_recompute_case_holds() {
        let doc = doc();
        let base = &doc["baseTask"];
        let cases = doc["recompute"].as_array().expect("recompute is an array");
        has_row(cases, "the-recompute-climbs-past-an-unchanged-level");
        has_row(cases, "autodate-pins-a-dateless-parent-newly-in-progress");
        has_row(cases, "an-empty-parent-id-ends-the-climb");
        for case in cases {
            let rows = tasks(case, base);
            let got = plan_ancestor_recompute(
                &rows,
                case["input"]["parentId"].as_str().expect("a parent id"),
                &options(case),
            );
            assert_eq!(
                spelled(&got),
                case["expect"],
                "{}: {}",
                case["name"],
                case["note"].as_str().unwrap_or("")
            );
        }
    }

    #[test]
    fn the_wire_accepts_a_full_task_and_defaults_the_options() {
        let input = r#"{"tasks": [
            {"id": "p", "list_id": "L", "title": "P", "status": "in_progress", "priority": "medium",
             "effort": "medium", "parent_id": null, "scheduled_date": null, "assignees": [],
             "reminders": [], "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"},
            {"id": "c", "status": "open", "parent_id": "p"}
        ], "task_id": "c", "status": "completed"}"#;
        let writes: Vec<StatusWrite> =
            serde_json::from_str(&plan_status_cascade_json(input).expect("the input is valid"))
                .expect("the answer round-trips");
        assert_eq!(
            writes,
            vec![
                StatusWrite {
                    task_id: "c".into(),
                    status: TaskStatus::Completed,
                    scheduled_date: None
                },
                StatusWrite {
                    task_id: "p".into(),
                    status: TaskStatus::Completed,
                    scheduled_date: None
                },
            ]
        );
        // The companion date is absent from the wire when it is not set.
        assert!(!plan_status_cascade_json(input)
            .expect("valid")
            .contains("scheduled_date"));
        assert_eq!(
            auto_date_on_start_json(r#"{"status": "in_progress", "today": "2026-05-21"}"#)
                .expect("valid"),
            "\"2026-05-21\""
        );
        assert_eq!(
            plan_ancestor_recompute_json(r#"{"tasks": [], "parent_id": "p"}"#).expect("valid"),
            "[]"
        );
    }
}
