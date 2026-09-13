//! The day start: which tasks are overdue, which slipped, which get pinned to
//! today, which are reminded and in which group, how many days remain to a
//! deadline, what "move to today" does to a task's dates, and whether a
//! day-start checker fires now.
//!
//! Was `shared/dayStart.ts`, run every morning on both surfaces — the desktop
//! checkers and review dialog, the mobile checks and modal, and the mobile
//! scheduler that asks about future days ahead of time for OS notifications.
//! A morning on the phone and the same morning on the desktop have to offer
//! the same tasks, and a frontend that is not JavaScript has to offer them
//! too; so the rules live here, once. The answer is POSITIONS into the given
//! tasks, days and yes/no — never wording (DESIGN §4.5 a) — and the core reads
//! no clock: the day, and for the fire gate the local time, travel in. Pinned
//! row by row in `tests/fixtures/dayStart.json`, measured from the TypeScript
//! this replaces; the `contract` module below reads it, and so does the
//! TypeScript contract test on the other side of the boundary.
//!
//! # One door, asked by rule
//!
//! Twelve rules, but a surface asks them one at a time, so they share one
//! door: a [`DayStartQuestion`] names its rule and carries what that rule
//! reads, and [`day_start_json`] answers that rule alone. Twelve doors would
//! have been twelve wirings on every surface for the same crossing.
//!
//! One rule also has a batch form, [`actionable_descendants_of`]: the
//! carry-over batches ask the walk down for every slipped root, and a crossing
//! per root sent — and indexed — every task each time, which made a large
//! batch hundreds of times slower than the JavaScript walk it replaced. One
//! crossing with all roots indexes the tasks once.
//!
//! # The plan
//!
//! [`plan`] answers the whole morning in one question: the overdue tasks, the
//! slipped rows split by each list's carry-over default, what each silent
//! batch touches, the reminder groups, and the count that opens the review.
//! The desktop checker, the mobile checks and the mobile scheduler composed
//! that inline (`tests/fixtures/dayStartPlan.json`, measured from them). Two
//! answers changed on purpose, decided 2026-09-13. A slipped row that a
//! carried root brings along is that root's, not a row of its own: it used to
//! be carried by both batches, the later one winning. And the batch walk
//! brings only tasks that are mine or nobody's, like every other day-start
//! rule. A row a coupled root takes lives in an uncoupled list (in a coupled
//! one it would already be hidden below its slipped parent), and an uncoupled
//! list brings nothing, so taking never goes both ways.
//!
//! # Day keys are text, the way the TypeScript read them
//!
//! A day is `YYYY-MM-DD`. The selectors COMPARE keys as text — earlier, the
//! same — exactly as the TypeScript did, by UTF-16 code unit, so even a key
//! that is not a day orders the same on both sides (the fixture pins one).
//! Only [`days_until_deadline`] reads a key as a date, and it reads it
//! strictly: four ASCII digits, two, two, a real calendar day, and a year of
//! at least 100 — JavaScript repaired the years 0 to 99 into 19xx, and the
//! round trip it checked with refused them. Where JavaScript's `Number` also
//! read `2026-5-23` or a hexadecimal year, this refuses; those two fixture
//! rows were changed on purpose. Nothing on the wire produces either: the
//! dates are `NaiveDate`.
//!
//! # Empty strings, kept as the TypeScript read them
//!
//! An empty date is no date (the TypeScript asked `!task.deadline_date`), an
//! empty parent id ends a climb, and an empty time still counts as a time (the
//! check was `!= null`). The wire carries `NaiveDate` and `NaiveTime`, so none
//! of them arrives; the core answers the same either way.
//!
//! # Walks that end
//!
//! Three rules follow parent links, and the TypeScript kept no visited set, so
//! a parent cycle — which an external provider can deliver — could hang the
//! day start. Here a walk down expands each id at most once, and a climb stops
//! at an id it has already passed. On a forest nothing changes, order
//! included. What does change is a duplicated id — two rows with the same id,
//! which two accounts can produce: the TypeScript walked below that id once
//! per row and collected the same subtasks twice, and this collects them once
//! (one more fixture row changed on purpose).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::task_assignment::mine_or_unassigned;
use crate::types::TaskStatus;

// ─────────────────────────────── The wire ───────────────────────────────────

/// What the day-start rules read of a task — by the same field names as
/// `Task`. A user is its id: ownership compares ids alone.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayStartTask {
    pub id: String,
    pub list_id: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub parent_id: Option<String>,
    /// `YYYY-MM-DD`, compared as text.
    #[serde(default)]
    pub scheduled_date: Option<String>,
    /// Only its presence is read.
    #[serde(default)]
    pub scheduled_time: Option<String>,
    /// `YYYY-MM-DD`, compared as text; read as a date for the countdown.
    #[serde(default)]
    pub deadline_date: Option<String>,
    /// The task's own countdown window; honoured when finite and at least 1.
    /// A number, not an integer, because the rule compared numbers.
    #[serde(default)]
    pub deadline_reminder_days: Option<f64>,
    /// The assignees' user ids.
    #[serde(default)]
    pub assignees: Vec<String>,
}

/// The account's own user id in one list, or none. A list the question does
/// not name has no identity, and a task in it is offered whoever holds it.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayStartIdentity {
    pub list_id: String,
    #[serde(default)]
    pub me: Option<String>,
}

/// The four Settings → Tasks reminder knobs.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayStartReminderSettings {
    pub remind_untimed_today: bool,
    pub remind_deadline_arrived: bool,
    pub remind_deadline_countdown: bool,
    /// The global countdown window; valid when finite and at least 1.
    #[serde(default)]
    pub deadline_countdown_days: Option<f64>,
}

/// What a list does at day start with a task whose plan lapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum CarryOverDefault {
    /// Ask in the review.
    #[default]
    Ask,
    /// Move it to today, silently.
    Today,
    /// Move it to the backlog, silently.
    Backlog,
}

/// One list's day-start settings, resolved. A list the question does not name
/// neither couples nor carries: its slipped rows are asked.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayStartListSettings {
    pub list_id: String,
    /// The parent/subtask status coupling.
    pub cascade: bool,
    pub carry_over_default: CarryOverDefault,
}

/// One question to the day-start rules. `today` is the day asked about — the
/// wall-clock day, or a future one the scheduler anchors to.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum DayStartQuestion {
    /// → positions, see [`overdue`].
    Overdue {
        tasks: Vec<DayStartTask>,
        today: String,
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
    },
    /// → positions, see [`carried_over`].
    CarriedOver {
        tasks: Vec<DayStartTask>,
        today: String,
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
        /// The lists whose status coupling is on.
        #[serde(default)]
        coupled_lists: Vec<String>,
    },
    /// → positions, see [`actionable_descendants`].
    ActionableDescendants {
        tasks: Vec<DayStartTask>,
        root_id: String,
    },
    /// → positions per root, see [`actionable_descendants_of`].
    ActionableDescendantsOf {
        tasks: Vec<DayStartTask>,
        root_ids: Vec<String>,
        /// With identities, only tasks that are mine or nobody's come along.
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
    },
    /// → yes/no, see [`has_actionable_descendants`].
    HasActionableDescendants {
        tasks: Vec<DayStartTask>,
        root_id: String,
    },
    /// → [`DayStartMoved`], see [`moved_to_today`].
    MovedToToday {
        today: String,
        #[serde(default)]
        scheduled_date: Option<String>,
    },
    /// → positions, see [`deadline_pin_targets`].
    DeadlinePinTargets {
        tasks: Vec<DayStartTask>,
        today: String,
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
    },
    /// → days or null, see [`days_until_deadline`].
    DaysUntilDeadline {
        #[serde(default)]
        deadline_date: Option<String>,
        from: String,
    },
    /// → positions, see [`untimed_today`].
    UntimedToday {
        tasks: Vec<DayStartTask>,
        today: String,
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
    },
    /// → positions, see [`deadline_arrived`].
    DeadlineArrived {
        tasks: Vec<DayStartTask>,
        today: String,
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
    },
    /// → positions, see [`deadline_countdown`].
    DeadlineCountdown {
        tasks: Vec<DayStartTask>,
        today: String,
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
        /// The global window; valid when finite and at least 1.
        #[serde(default)]
        days_until: Option<f64>,
    },
    /// → [`DayStartReminderGroups`], see [`reminder_groups`].
    ReminderGroups {
        tasks: Vec<DayStartTask>,
        today: String,
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
        settings: DayStartReminderSettings,
    },
    /// → yes/no, see [`should_fire_today`].
    ShouldFire {
        /// `app-start` or `HH:MM`.
        trigger: String,
        /// The day the checker last fired, if ever.
        #[serde(default)]
        last_fired: Option<String>,
        today: String,
        /// The local wall-clock time.
        now_hour: u32,
        now_minute: u32,
    },
    /// → [`DayStartPlan`], see [`plan`].
    Plan {
        tasks: Vec<DayStartTask>,
        today: String,
        #[serde(default)]
        identities: Vec<DayStartIdentity>,
        /// Every list the tasks live in.
        #[serde(default)]
        lists: Vec<DayStartListSettings>,
        settings: DayStartReminderSettings,
    },
}

/// A task's dates after "move to today": the deadline is today, and the plan
/// is today only when it had lapsed — absent, the plan stays as it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayStartMoved {
    pub deadline_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub scheduled_date: Option<String>,
}

/// The three reminder groups, as positions, each task in at most one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayStartReminderGroups {
    pub untimed: Vec<usize>,
    pub due_today: Vec<usize>,
    pub countdown: Vec<usize>,
}

/// The morning, as positions: see [`plan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayStartPlan {
    pub overdue: Vec<usize>,
    /// Slipped rows the review asks about.
    pub ask: Vec<usize>,
    /// Slipped rows carried to today, silently.
    pub today: Vec<usize>,
    /// Slipped rows carried to the backlog, silently.
    pub backlog: Vec<usize>,
    /// What the today batch writes: each row, then what it brings along.
    pub today_targets: Vec<usize>,
    /// What the backlog batch writes.
    pub backlog_targets: Vec<usize>,
    pub reminders: DayStartReminderGroups,
    /// What opens the review: overdue, asked, and every reminder.
    pub surfaced: usize,
}

// ─────────────────────────────── Reading ────────────────────────────────────

fn is_settled(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Completed | TaskStatus::Cancelled)
}

fn is_actionable(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Open | TaskStatus::InProgress)
}

/// An optional string as JavaScript's truthiness read it: empty is absent.
fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|s| !s.is_empty())
}

/// `a < b` as JavaScript compares strings: by UTF-16 code unit, which orders
/// the characters above U+FFFF differently from UTF-8 bytes.
fn js_less(a: &str, b: &str) -> bool {
    a.encode_utf16().cmp(b.encode_utf16()) == Ordering::Less
}

/// A `YYYY-MM-DD` day, read strictly: see the module notes.
fn parse_day_key(key: &str) -> Option<NaiveDate> {
    let bytes = key.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<u32> {
        let part = &bytes[from..to];
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(part.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0')))
    };
    let year = number(0, 4)?;
    if year < 100 {
        return None;
    }
    NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, number(5, 7)?, number(8, 10)?)
}

/// The tasks, indexed once per question.
struct Tasks<'a> {
    rows: &'a [DayStartTask],
    /// Positions by parent id, in input order.
    children: HashMap<&'a str, Vec<usize>>,
}

impl<'a> Tasks<'a> {
    fn index(rows: &'a [DayStartTask]) -> Self {
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, row) in rows.iter().enumerate() {
            if let Some(parent) = row.parent_id.as_deref() {
                children.entry(parent).or_default().push(i);
            }
        }
        Tasks { rows, children }
    }

    fn children_of(&self, id: &str) -> &[usize] {
        self.children.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Whether anything below `root_id` is still actionable — a "project"
    /// parent, whose subtasks are the asked-about units. The walk goes on
    /// through settled tasks, and ends early on the first actionable one.
    fn has_actionable_below(&self, root_id: &str) -> bool {
        let mut stack = vec![root_id];
        let mut expanded: HashSet<&str> = HashSet::from([root_id]);
        while let Some(id) = stack.pop() {
            for &child in self.children_of(id) {
                let row = &self.rows[child];
                if is_actionable(row.status) {
                    return true;
                }
                if expanded.insert(row.id.as_str()) {
                    stack.push(row.id.as_str());
                }
            }
        }
        false
    }

    /// The actionable tasks below `root_id` that `owners` counts as mine, in
    /// the order of a stack walk: all children of a node, then the LAST
    /// child's subtree, then the first's. The walk goes on through everything,
    /// settled or someone else's.
    fn actionable_below(&self, root_id: &str, owners: &Owners) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![root_id];
        let mut expanded: HashSet<&str> = HashSet::from([root_id]);
        while let Some(id) = stack.pop() {
            for &child in self.children_of(id) {
                let row = &self.rows[child];
                if row.id == root_id {
                    continue; // the root is never its own descendant
                }
                if expanded.insert(row.id.as_str()) {
                    stack.push(row.id.as_str());
                }
                if is_actionable(row.status) && owners.mine(row) {
                    out.push(child);
                }
            }
        }
        out
    }
}

/// Who the account is, per list.
struct Owners<'a>(HashMap<&'a str, Option<&'a str>>);

impl<'a> Owners<'a> {
    fn of(identities: &'a [DayStartIdentity]) -> Self {
        Owners(
            identities
                .iter()
                .map(|i| (i.list_id.as_str(), i.me.as_deref()))
                .collect(),
        )
    }

    fn nobody() -> Self {
        Owners(HashMap::new())
    }

    /// Not a task held by concrete OTHER people — someone else handles it.
    fn mine(&self, row: &DayStartTask) -> bool {
        let me = self.0.get(row.list_id.as_str()).copied().flatten();
        mine_or_unassigned(row.assignees.iter().map(String::as_str), me)
    }
}

fn positions(rows: &[DayStartTask], keep: impl Fn(&DayStartTask) -> bool) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| keep(row))
        .map(|(i, _)| i)
        .collect()
}

// ─────────────────────────────── The rules ──────────────────────────────────

fn overdue_in(tasks: &Tasks, today: &str, owners: &Owners) -> Vec<usize> {
    positions(tasks.rows, |row| {
        let Some(deadline) = present(&row.deadline_date) else {
            return false;
        };
        !is_settled(row.status)
            && js_less(deadline, today)
            && !tasks.has_actionable_below(&row.id)
            && owners.mine(row)
    })
}

/// A deadline strictly before `today`, still actionable, not a "project"
/// parent (it returns once its subtasks are settled), and not a task held only
/// by others. A lapsed plan alone is no missed commitment.
pub fn overdue(tasks: &[DayStartTask], today: &str, identities: &[DayStartIdentity]) -> Vec<usize> {
    overdue_in(&Tasks::index(tasks), today, &Owners::of(identities))
}

/// A plan strictly before `today`, still actionable, not overdue (the deadline
/// section settles those), and not a task held only by others. In a list whose
/// status coupling is on, a slipped task below a slipped ancestor is left out:
/// the user decides at the root of the slipped subtree. Only the task's own
/// list is asked; the ancestor may live anywhere.
pub fn carried_over(
    tasks: &[DayStartTask],
    today: &str,
    identities: &[DayStartIdentity],
    coupled_lists: &[String],
) -> Vec<usize> {
    let coupled: HashSet<&str> = coupled_lists.iter().map(String::as_str).collect();
    carried_over_in(
        &Tasks::index(tasks),
        today,
        &Owners::of(identities),
        &coupled,
    )
}

fn carried_over_in(
    index: &Tasks,
    today: &str,
    owners: &Owners,
    coupled: &HashSet<&str>,
) -> Vec<usize> {
    let tasks = index.rows;
    // Overdue for anyone: a colleague's overdue task is not carried either.
    let overdue_ids: HashSet<&str> = overdue_in(index, today, &Owners::nobody())
        .into_iter()
        .map(|i| tasks[i].id.as_str())
        .collect();
    let slipped = positions(tasks, |row| {
        let Some(plan) = present(&row.scheduled_date) else {
            return false;
        };
        !is_settled(row.status)
            && !overdue_ids.contains(row.id.as_str())
            && js_less(plan, today)
            && owners.mine(row)
    });
    if coupled.is_empty() {
        return slipped;
    }

    let slipped_ids: HashSet<&str> = slipped.iter().map(|&i| tasks[i].id.as_str()).collect();
    // The last row with an id wins, as a JavaScript `Map` built from the list.
    let by_id: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, row)| (row.id.as_str(), i))
        .collect();
    let has_slipped_ancestor = |row: &DayStartTask| -> bool {
        if !coupled.contains(row.list_id.as_str()) {
            return false;
        }
        let mut climbed: HashSet<&str> = HashSet::new();
        let mut parent = present(&row.parent_id);
        while let Some(id) = parent {
            if slipped_ids.contains(id) {
                return true;
            }
            if !climbed.insert(id) {
                return false; // a parent cycle with nothing slipped on it
            }
            parent = by_id.get(id).and_then(|&i| present(&tasks[i].parent_id));
        }
        false
    };
    slipped
        .into_iter()
        .filter(|&i| !has_slipped_ancestor(&tasks[i]))
        .collect()
}

/// The actionable (`open` / `in_progress`) tasks anywhere below `root_id` —
/// what a verdict on a parent row drags along, leaving settled ones alone.
pub fn actionable_descendants(tasks: &[DayStartTask], root_id: &str) -> Vec<usize> {
    Tasks::index(tasks).actionable_below(root_id, &Owners::nobody())
}

/// [`actionable_descendants`] for each of `root_ids`, in the order given, over
/// one index of the tasks. With identities, only the tasks that are mine or
/// nobody's come along; the carry-over batches pass them.
pub fn actionable_descendants_of(
    tasks: &[DayStartTask],
    root_ids: &[String],
    identities: &[DayStartIdentity],
) -> Vec<Vec<usize>> {
    let index = Tasks::index(tasks);
    let owners = Owners::of(identities);
    root_ids
        .iter()
        .map(|root_id| index.actionable_below(root_id, &owners))
        .collect()
}

/// Whether `root_id` still has an actionable task anywhere below it.
pub fn has_actionable_descendants(tasks: &[DayStartTask], root_id: &str) -> bool {
    Tasks::index(tasks).has_actionable_below(root_id)
}

/// "Move the lapsed deadline to today": the deadline is `today`, and a plan
/// strictly before today comes along (a task due today and planned for
/// yesterday describes nothing anyone can act on). A plan that is today or
/// still ahead is the user's own arrangement and stays. The times of day are
/// not the core's to touch: they stay as they were.
pub fn moved_to_today(today: &str, scheduled_date: Option<&str>) -> DayStartMoved {
    let lapsed = scheduled_date.is_some_and(|plan| !plan.is_empty() && js_less(plan, today));
    DayStartMoved {
        deadline_date: today.to_string(),
        scheduled_date: lapsed.then(|| today.to_string()),
    }
}

fn deadline_pin_targets_in(tasks: &Tasks, today: &str, owners: &Owners) -> Vec<usize> {
    positions(tasks.rows, |row| {
        present(&row.deadline_date) == Some(today)
            && !is_settled(row.status)
            && row.scheduled_date.as_deref() != Some(today)
            && !tasks.has_actionable_below(&row.id)
            && owners.mine(row)
    })
}

/// The silent "by"-deadline pin: due `today`, still actionable, not already
/// planned for today (which keeps the batch idempotent across launches), not
/// a "project" parent, not a task held only by others.
pub fn deadline_pin_targets(
    tasks: &[DayStartTask],
    today: &str,
    identities: &[DayStartIdentity],
) -> Vec<usize> {
    deadline_pin_targets_in(&Tasks::index(tasks), today, &Owners::of(identities))
}

/// Whole days from `from` until the deadline: 0 on the day, negative when
/// past, none without a deadline — or when either is not a day (see the
/// module notes), rather than a confident wrong number.
pub fn days_until_deadline(deadline_date: Option<&str>, from: &str) -> Option<i64> {
    let deadline = deadline_date.filter(|d| !d.is_empty())?;
    let from = parse_day_key(from)?;
    let deadline = parse_day_key(deadline)?;
    Some((deadline - from).num_days())
}

fn untimed_today_in(tasks: &Tasks, today: &str, owners: &Owners) -> Vec<usize> {
    positions(tasks.rows, |row| {
        row.scheduled_date.as_deref() == Some(today)
            && row.scheduled_time.is_none()
            && !is_settled(row.status)
            && !tasks.has_actionable_below(&row.id)
            && owners.mine(row)
    })
}

/// Planned for `today` with no time of day — a timed task already shows on
/// the timeline.
pub fn untimed_today(
    tasks: &[DayStartTask],
    today: &str,
    identities: &[DayStartIdentity],
) -> Vec<usize> {
    untimed_today_in(&Tasks::index(tasks), today, &Owners::of(identities))
}

fn deadline_arrived_in(tasks: &Tasks, today: &str, owners: &Owners) -> Vec<usize> {
    positions(tasks.rows, |row| {
        row.deadline_date.as_deref() == Some(today)
            && !is_settled(row.status)
            && !tasks.has_actionable_below(&row.id)
            && owners.mine(row)
    })
}

/// Due `today` — reminded whether or not it is already planned for today.
pub fn deadline_arrived(
    tasks: &[DayStartTask],
    today: &str,
    identities: &[DayStartIdentity],
) -> Vec<usize> {
    deadline_arrived_in(&Tasks::index(tasks), today, &Owners::of(identities))
}

/// A window is valid when it is finite and at least one day.
fn valid_window(days: Option<f64>) -> Option<f64> {
    days.filter(|d| d.is_finite() && *d >= 1.0)
}

fn deadline_countdown_in(
    tasks: &Tasks,
    today: &str,
    owners: &Owners,
    days_until: Option<f64>,
) -> Vec<usize> {
    let global = valid_window(days_until);
    positions(tasks.rows, |row| {
        let Some(window) = valid_window(row.deadline_reminder_days).or(global) else {
            return false;
        };
        if is_settled(row.status) {
            return false;
        }
        let Some(days) = days_until_deadline(row.deadline_date.as_deref(), today) else {
            return false;
        };
        // 1..=window: the deadline day itself is `deadline_arrived`.
        days >= 1
            && (days as f64) <= window
            && !tasks.has_actionable_below(&row.id)
            && owners.mine(row)
    })
}

/// A deadline between one and the window's days away, inclusive — so a window
/// of 3 reminds three, two and one day before. A task's own window wins when
/// valid; otherwise the global one, and without either nothing is reminded.
pub fn deadline_countdown(
    tasks: &[DayStartTask],
    today: &str,
    identities: &[DayStartIdentity],
    days_until: Option<f64>,
) -> Vec<usize> {
    deadline_countdown_in(
        &Tasks::index(tasks),
        today,
        &Owners::of(identities),
        days_until,
    )
}

/// The three reminders for `today`, each behind its own toggle, and each task
/// in exactly one group: due today, then planned today, then counting down —
/// decided by id, and only against the groups that are on.
pub fn reminder_groups(
    tasks: &[DayStartTask],
    today: &str,
    identities: &[DayStartIdentity],
    settings: &DayStartReminderSettings,
) -> DayStartReminderGroups {
    reminder_groups_in(
        &Tasks::index(tasks),
        today,
        &Owners::of(identities),
        settings,
    )
}

fn reminder_groups_in(
    index: &Tasks,
    today: &str,
    owners: &Owners,
    settings: &DayStartReminderSettings,
) -> DayStartReminderGroups {
    let tasks = index.rows;
    let due_today = if settings.remind_deadline_arrived {
        deadline_arrived_in(index, today, owners)
    } else {
        Vec::new()
    };
    let mut seen: HashSet<&str> = due_today.iter().map(|&i| tasks[i].id.as_str()).collect();
    let untimed: Vec<usize> = if settings.remind_untimed_today {
        untimed_today_in(index, today, owners)
            .into_iter()
            .filter(|&i| !seen.contains(tasks[i].id.as_str()))
            .collect()
    } else {
        Vec::new()
    };
    seen.extend(untimed.iter().map(|&i| tasks[i].id.as_str()));
    let countdown = if settings.remind_deadline_countdown {
        deadline_countdown_in(index, today, owners, settings.deadline_countdown_days)
            .into_iter()
            .filter(|&i| !seen.contains(tasks[i].id.as_str()))
            .collect()
    } else {
        Vec::new()
    };
    DayStartReminderGroups {
        untimed,
        due_today,
        countdown,
    }
}

/// The whole morning for `today`: the overdue tasks, the slipped rows split
/// by each list's carry-over default, what the silent batches write, the
/// reminder groups, and the count that opens the review (see the module
/// notes). Rows keep their input order within each part. A batch writes each
/// of its rows and, where the row's list couples, the actionable tasks below
/// it that are mine or nobody's; a slipped row one of them brings along is
/// that root's and leaves its own part. Targets are keyed by id, like the
/// JavaScript `Map` they were collected into: a later row with an id already
/// collected takes the earlier one's place.
pub fn plan(
    tasks: &[DayStartTask],
    today: &str,
    identities: &[DayStartIdentity],
    lists: &[DayStartListSettings],
    settings: &DayStartReminderSettings,
) -> DayStartPlan {
    let index = Tasks::index(tasks);
    let owners = Owners::of(identities);
    let by_list: HashMap<&str, &DayStartListSettings> =
        lists.iter().map(|l| (l.list_id.as_str(), l)).collect();
    let settings_of = |row: &DayStartTask| by_list.get(row.list_id.as_str()).copied();
    let couples = |row: &DayStartTask| settings_of(row).is_some_and(|l| l.cascade);
    let carry = |row: &DayStartTask| {
        settings_of(row).map_or(CarryOverDefault::Ask, |l| l.carry_over_default)
    };

    let overdue = overdue_in(&index, today, &owners);
    let coupled: HashSet<&str> = lists
        .iter()
        .filter(|l| l.cascade)
        .map(|l| l.list_id.as_str())
        .collect();
    let slipped = carried_over_in(&index, today, &owners, &coupled);

    // What each silently carried row brings along.
    let below: HashMap<usize, Vec<usize>> = slipped
        .iter()
        .copied()
        .filter(|&i| carry(&tasks[i]) != CarryOverDefault::Ask)
        .map(|i| {
            let brought = if couples(&tasks[i]) {
                index.actionable_below(&tasks[i].id, &owners)
            } else {
                Vec::new()
            };
            (i, brought)
        })
        .collect();
    // The parent decides: a row brought along is not a row of its own.
    let taken: HashSet<usize> = below.values().flatten().copied().collect();

    let mut ask = Vec::new();
    let mut today_rows = Vec::new();
    let mut backlog_rows = Vec::new();
    for &i in &slipped {
        if taken.contains(&i) {
            continue;
        }
        match carry(&tasks[i]) {
            CarryOverDefault::Ask => ask.push(i),
            CarryOverDefault::Today => today_rows.push(i),
            CarryOverDefault::Backlog => backlog_rows.push(i),
        }
    }

    let reminders = reminder_groups_in(&index, today, &owners, settings);
    let surfaced = overdue.len()
        + ask.len()
        + reminders.untimed.len()
        + reminders.due_today.len()
        + reminders.countdown.len();
    DayStartPlan {
        today_targets: batch_targets(tasks, &today_rows, &below),
        backlog_targets: batch_targets(tasks, &backlog_rows, &below),
        overdue,
        ask,
        today: today_rows,
        backlog: backlog_rows,
        reminders,
        surfaced,
    }
}

/// Each root, then what it brings along, keyed by id.
fn batch_targets(
    tasks: &[DayStartTask],
    roots: &[usize],
    below: &HashMap<usize, Vec<usize>>,
) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut slot: HashMap<&str, usize> = HashMap::new();
    for &root in roots {
        let brought = below.get(&root).into_iter().flatten().copied();
        for i in std::iter::once(root).chain(brought) {
            match slot.get(tasks[i].id.as_str()) {
                Some(&at) => out[at] = i,
                None => {
                    slot.insert(tasks[i].id.as_str(), out.len());
                    out.push(i);
                }
            }
        }
    }
    out
}

/// `HH:MM` as the TypeScript's `^(\d{1,2}):(\d{2})$` read it: one or two
/// ASCII digits, a colon, exactly two, nothing else.
fn parse_trigger(trigger: &str) -> Option<(u32, u32)> {
    let (hours, minutes) = trigger.split_once(':')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !(1..=2).contains(&hours.len()) || minutes.len() != 2 || !digits(hours) || !digits(minutes) {
        return None;
    }
    Some((hours.parse().ok()?, minutes.parse().ok()?))
}

/// Whether a day-start checker fires now. `app-start` fires once, ever — only
/// while no day was recorded. `HH:MM` fires once a day, when the local clock
/// has reached it; a trigger that is not one fires at once, because an
/// unreadable setting must never silence the day start.
pub fn should_fire_today(
    trigger: &str,
    last_fired: Option<&str>,
    today: &str,
    now_hour: u32,
    now_minute: u32,
) -> bool {
    if trigger == "app-start" {
        return last_fired.is_none();
    }
    if last_fired == Some(today) {
        return false;
    }
    let Some((hours, minutes)) = parse_trigger(trigger) else {
        return true;
    };
    if hours > 23 || minutes > 59 {
        return true;
    }
    now_hour > hours || (now_hour == hours && now_minute >= minutes)
}

/// The day-start rules over the wire: one [`DayStartQuestion`] in, its rule's
/// answer out.
pub fn day_start_json(input_json: &str) -> Result<String, serde_json::Error> {
    let question: DayStartQuestion = serde_json::from_str(input_json)?;
    match question {
        DayStartQuestion::Overdue {
            tasks,
            today,
            identities,
        } => serde_json::to_string(&overdue(&tasks, &today, &identities)),
        DayStartQuestion::CarriedOver {
            tasks,
            today,
            identities,
            coupled_lists,
        } => serde_json::to_string(&carried_over(&tasks, &today, &identities, &coupled_lists)),
        DayStartQuestion::ActionableDescendants { tasks, root_id } => {
            serde_json::to_string(&actionable_descendants(&tasks, &root_id))
        }
        DayStartQuestion::ActionableDescendantsOf {
            tasks,
            root_ids,
            identities,
        } => serde_json::to_string(&actionable_descendants_of(&tasks, &root_ids, &identities)),
        DayStartQuestion::HasActionableDescendants { tasks, root_id } => {
            serde_json::to_string(&has_actionable_descendants(&tasks, &root_id))
        }
        DayStartQuestion::MovedToToday {
            today,
            scheduled_date,
        } => serde_json::to_string(&moved_to_today(&today, scheduled_date.as_deref())),
        DayStartQuestion::DeadlinePinTargets {
            tasks,
            today,
            identities,
        } => serde_json::to_string(&deadline_pin_targets(&tasks, &today, &identities)),
        DayStartQuestion::DaysUntilDeadline {
            deadline_date,
            from,
        } => serde_json::to_string(&days_until_deadline(deadline_date.as_deref(), &from)),
        DayStartQuestion::UntimedToday {
            tasks,
            today,
            identities,
        } => serde_json::to_string(&untimed_today(&tasks, &today, &identities)),
        DayStartQuestion::DeadlineArrived {
            tasks,
            today,
            identities,
        } => serde_json::to_string(&deadline_arrived(&tasks, &today, &identities)),
        DayStartQuestion::DeadlineCountdown {
            tasks,
            today,
            identities,
            days_until,
        } => serde_json::to_string(&deadline_countdown(&tasks, &today, &identities, days_until)),
        DayStartQuestion::ReminderGroups {
            tasks,
            today,
            identities,
            settings,
        } => serde_json::to_string(&reminder_groups(&tasks, &today, &identities, &settings)),
        DayStartQuestion::ShouldFire {
            trigger,
            last_fired,
            today,
            now_hour,
            now_minute,
        } => serde_json::to_string(&should_fire_today(
            &trigger,
            last_fired.as_deref(),
            &today,
            now_hour,
            now_minute,
        )),
        DayStartQuestion::Plan {
            tasks,
            today,
            identities,
            lists,
            settings,
        } => serde_json::to_string(&plan(&tasks, &today, &identities, &lists, &settings)),
    }
}

/// What the fixture cannot hold: walks over parent cycles, which hung the
/// TypeScript, and the edges of the strict day reading.
#[cfg(test)]
mod walks {
    use super::*;

    fn task(id: &str, parent: Option<&str>, status: TaskStatus) -> DayStartTask {
        DayStartTask {
            id: id.into(),
            list_id: "list".into(),
            status,
            parent_id: parent.map(Into::into),
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_reminder_days: None,
            assignees: Vec::new(),
        }
    }

    #[test]
    fn the_walk_down_ends_on_a_cycle_through_its_root() {
        let tasks = vec![
            task("r", Some("s"), TaskStatus::Open),
            task("s", Some("r"), TaskStatus::Open),
        ];
        assert_eq!(actionable_descendants(&tasks, "r"), vec![1]);
    }

    #[test]
    fn the_project_parent_check_ends_on_a_settled_cycle() {
        // Reachable below an actionable candidate only through a duplicated
        // id: `x` under the candidate, and another `x` inside a cycle of
        // settled tasks. The TypeScript walked it forever.
        let mut tasks = vec![
            task("p", None, TaskStatus::Open),
            task("x", Some("p"), TaskStatus::Completed),
            task("y", Some("x"), TaskStatus::Completed),
            task("x", Some("y"), TaskStatus::Completed),
        ];
        tasks[0].deadline_date = Some("2026-05-10".into());
        assert!(!has_actionable_descendants(&tasks, "p"));
        assert_eq!(overdue(&tasks, "2026-05-20", &[]), vec![0]);
    }

    #[test]
    fn the_climb_ends_on_a_cycle_above_a_slipped_task() {
        let mut tasks = vec![
            task("c", Some("a"), TaskStatus::Open),
            task("a", Some("b"), TaskStatus::Open),
            task("b", Some("a"), TaskStatus::Open),
        ];
        tasks[0].scheduled_date = Some("2026-05-10".into());
        assert_eq!(
            carried_over(&tasks, "2026-05-20", &[], &["list".to_string()]),
            vec![0]
        );
    }

    #[test]
    fn a_task_that_is_its_own_slipped_parent_hides_itself() {
        // As the TypeScript answered: the climb asks the slipped set before
        // it notices the loop.
        let mut tasks = vec![task("c", Some("c"), TaskStatus::Open)];
        tasks[0].scheduled_date = Some("2026-05-10".into());
        assert!(carried_over(&tasks, "2026-05-20", &[], &["list".to_string()]).is_empty());
    }

    #[test]
    fn a_day_is_four_two_two_digits_and_a_real_day() {
        assert_eq!(
            parse_day_key("0100-01-01"),
            NaiveDate::from_ymd_opt(100, 1, 1)
        );
        assert_eq!(
            parse_day_key("9999-12-31"),
            NaiveDate::from_ymd_opt(9999, 12, 31)
        );
        assert_eq!(parse_day_key("0099-12-31"), None);
        assert_eq!(parse_day_key("10000-01-01"), None);
        assert_eq!(parse_day_key("2026-05-2 "), None);
        assert_eq!(parse_day_key("2026/05/20"), None);
        assert_eq!(parse_day_key("２０２６-05-20"), None);
    }

    #[test]
    fn text_order_is_javascripts() {
        // U+FF61 sorts before U+1F600 by UTF-8 byte, after it by UTF-16 unit.
        assert!("\u{FF61}" < "\u{1F600}");
        assert!(!js_less("\u{FF61}", "\u{1F600}"));
        assert!(js_less("\u{1F600}", "\u{FF61}"));
        assert!(js_less("2026-05-19", "2026-05-20"));
        assert!(!js_less("2026-05-20", "2026-05-20"));
    }
}

/// The contract with the TypeScript this replaced.
///
/// Its other half is `src/components/dayStart.contract.test.ts`, reading this
/// same file through the door. The fixture was written by running the
/// TypeScript BEFORE the port, so the port is measured against what was; it
/// spells its inputs the way that TypeScript's callers held them (a user as
/// an object, the identity as a map per list, `anchor` beside the wall-clock
/// `today`), and this test translates.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/dayStart.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the contract parses")
    }

    fn cases<'d>(doc: &'d Value, section: &str) -> &'d [Value] {
        doc[section].as_array().expect("a section is an array")
    }

    fn has_row(cases: &[Value], name: &str) {
        assert!(
            cases.iter().any(|c| c["name"] == name),
            "the contract lost `{name}`"
        );
    }

    /// The base task with one case's overrides laid over it, a user reduced
    /// to its id.
    fn row(doc: &Value, overrides: &Value) -> DayStartTask {
        let mut row = doc["baseTask"].clone();
        for (key, value) in overrides.as_object().expect("an override object") {
            row[key.as_str()] = value.clone();
        }
        let ids: Vec<Value> = row["assignees"]
            .as_array()
            .expect("assignees is an array")
            .iter()
            .map(|user| user["id"].clone())
            .collect();
        row["assignees"] = Value::Array(ids);
        serde_json::from_value(row).expect("a task row deserializes")
    }

    fn tasks(doc: &Value, case: &Value) -> Vec<DayStartTask> {
        case["input"]["tasks"]
            .as_array()
            .expect("tasks is an array")
            .iter()
            .map(|over| row(doc, over))
            .collect()
    }

    /// The day asked about: the explicit anchor, else the wall-clock day.
    fn day(case: &Value) -> String {
        let input = &case["input"];
        input["anchor"]
            .as_str()
            .or(input["today"].as_str())
            .expect("a day")
            .to_string()
    }

    fn identities(case: &Value) -> Vec<DayStartIdentity> {
        case["input"]["currentUserByList"]
            .as_object()
            .map(|by_list| {
                by_list
                    .iter()
                    .map(|(list, me)| DayStartIdentity {
                        list_id: list.clone(),
                        me: me.as_str().map(str::to_string),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn ids(tasks: &[DayStartTask], positions: &[usize]) -> Value {
        json!(positions
            .iter()
            .map(|&i| tasks[i].id.as_str())
            .collect::<Vec<_>>())
    }

    /// Run `answer` over every row of `section`, comparing the ids.
    fn selector(
        section: &str,
        needed: &[&str],
        answer: impl Fn(&[DayStartTask], &Value) -> Vec<usize>,
    ) {
        let doc = doc();
        let cases = cases(&doc, section);
        for name in needed {
            has_row(cases, name);
        }
        for case in cases {
            let tasks = tasks(&doc, case);
            let got = ids(&tasks, &answer(&tasks, case));
            assert_eq!(got, case["expect"], "{section}: {}", case["name"]);
        }
    }

    #[test]
    fn every_overdue_row_holds() {
        selector(
            "overdue",
            &[
                "a-project-parent-waits-for-its-subtasks",
                "ownership-keeps-mine-and-unassigned",
            ],
            |tasks, case| overdue(tasks, &day(case), &identities(case)),
        );
    }

    #[test]
    fn every_carried_over_row_holds() {
        selector(
            "carriedOver",
            &[
                "overdue-takes-priority",
                "a-project-parent-with-a-lapsed-deadline-and-plan-is-carried-over",
                "cascade-hides-a-grandchild-under-a-slipped-grandparent",
                "cascade-is-decided-by-the-subtasks-own-list",
            ],
            |tasks, case| {
                let coupled: Vec<String> = case["input"]["cascadeLists"]
                    .as_array()
                    .map(|lists| {
                        lists
                            .iter()
                            .map(|l| l.as_str().expect("a list id").to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                carried_over(tasks, &day(case), &identities(case), &coupled)
            },
        );
    }

    #[test]
    fn every_actionable_descendants_row_holds() {
        selector(
            "actionableDescendants",
            &[
                "the-last-child-subtree-comes-first-below-the-root",
                "a-duplicated-id-is-walked-once",
            ],
            |tasks, case| {
                actionable_descendants(tasks, case["input"]["rootId"].as_str().expect("a root"))
            },
        );
    }

    #[test]
    fn every_has_actionable_descendants_row_holds() {
        let doc = doc();
        has_row(
            cases(&doc, "hasActionableDescendants"),
            "an-open-grandchild-through-a-settled-child",
        );
        for case in cases(&doc, "hasActionableDescendants") {
            let tasks = tasks(&doc, case);
            let root = case["input"]["rootId"].as_str().expect("a root");
            assert_eq!(
                json!(has_actionable_descendants(&tasks, root)),
                case["expect"],
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn the_batch_walk_answers_what_each_single_walk_answers() {
        // Every task of every descendant row taken as a root, in one question.
        let doc = doc();
        for case in cases(&doc, "actionableDescendants") {
            let tasks = tasks(&doc, case);
            let mut roots: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
            roots.push("nobody".to_string());
            let singles: Vec<Vec<usize>> = roots
                .iter()
                .map(|root| actionable_descendants(&tasks, root))
                .collect();
            assert_eq!(
                actionable_descendants_of(&tasks, &roots, &[]),
                singles,
                "{}",
                case["name"]
            );
        }
        assert!(actionable_descendants_of(&[], &[], &[]).is_empty());
    }

    #[test]
    fn every_moved_to_today_row_holds() {
        let doc = doc();
        let cases = cases(&doc, "movedToToday");
        has_row(cases, "lifts-a-lapsed-plan-keeping-its-time");
        for case in cases {
            let task = &case["input"]["task"];
            let before = |field: &str| task.get(field).cloned().unwrap_or(Value::Null);
            let moved = moved_to_today(
                case["input"]["today"].as_str().expect("today"),
                task["scheduled_date"].as_str(),
            );
            let got = json!({
                "deadline_date": moved.deadline_date,
                "deadline_time": before("deadline_time"),
                "scheduled_date": moved.scheduled_date.map_or_else(|| before("scheduled_date"), Value::from),
                "scheduled_time": before("scheduled_time"),
            });
            assert_eq!(got, case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_deadline_pin_row_holds() {
        selector(
            "deadlinePinTargets",
            &["a-deadline-today-not-yet-planned-today"],
            |tasks, case| deadline_pin_targets(tasks, &day(case), &identities(case)),
        );
    }

    #[test]
    fn every_days_until_deadline_row_holds() {
        let doc = doc();
        let cases = cases(&doc, "daysUntilDeadline");
        has_row(cases, "a-month-thirteen-is-none");
        has_row(cases, "over-the-spring-clock-change");
        has_row(cases, "an-unpadded-date-is-none");
        for case in cases {
            let got = days_until_deadline(case["input"]["deadline"].as_str(), &day(case));
            assert_eq!(json!(got), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_untimed_today_row_holds() {
        selector(
            "untimedToday",
            &["an-empty-time-counts-as-timed"],
            |tasks, case| untimed_today(tasks, &day(case), &identities(case)),
        );
    }

    #[test]
    fn every_deadline_arrived_row_holds() {
        selector(
            "deadlineArrived",
            &["a-deadline-today-even-if-planned-today"],
            |tasks, case| deadline_arrived(tasks, &day(case), &identities(case)),
        );
    }

    #[test]
    fn every_countdown_row_holds() {
        selector(
            "deadlineCountdown",
            &[
                "the-window-is-cumulative",
                "an-override-holds-with-an-invalid-global",
            ],
            |tasks, case| {
                deadline_countdown(
                    tasks,
                    &day(case),
                    &identities(case),
                    case["input"]["daysUntil"].as_f64(),
                )
            },
        );
    }

    #[test]
    fn every_reminder_group_row_holds() {
        let doc = doc();
        let cases = cases(&doc, "reminderGroups");
        has_row(cases, "due-today-and-planned-today-counts-once-as-due");
        has_row(
            cases,
            "with-the-due-toggle-off-a-due-and-planned-task-counts-as-planned",
        );
        for case in cases {
            let tasks = tasks(&doc, case);
            let s = &case["input"]["settings"];
            let settings = DayStartReminderSettings {
                remind_untimed_today: s["remindUntimedToday"].as_bool().expect("a toggle"),
                remind_deadline_arrived: s["remindDeadlineArrived"].as_bool().expect("a toggle"),
                remind_deadline_countdown: s["remindDeadlineCountdown"]
                    .as_bool()
                    .expect("a toggle"),
                deadline_countdown_days: s["deadlineCountdownDays"].as_f64(),
            };
            let groups = reminder_groups(&tasks, &day(case), &identities(case), &settings);
            let got = json!({
                "untimed": ids(&tasks, &groups.untimed),
                "dueToday": ids(&tasks, &groups.due_today),
                "countdown": ids(&tasks, &groups.countdown),
            });
            assert_eq!(got, case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_fire_gate_row_holds() {
        let doc = doc();
        let cases = cases(&doc, "shouldFireToday");
        has_row(cases, "garbage-fires-immediately");
        has_row(cases, "app-start-never-fires-again");
        for case in cases {
            let i = &case["input"];
            let (hour, minute) = i["now"]
                .as_str()
                .and_then(|now| now.split_once(':'))
                .expect("now is HH:MM");
            let got = should_fire_today(
                i["trigger"].as_str().expect("a trigger"),
                i["lastFired"].as_str(),
                i["today"].as_str().expect("today"),
                hour.parse().expect("an hour"),
                minute.parse().expect("a minute"),
            );
            assert_eq!(json!(got), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn the_door_answers_the_rule_it_was_asked() {
        let answer = day_start_json(
            r#"{"rule":"should_fire","trigger":"08:00","last_fired":null,"today":"2026-05-20","now_hour":9,"now_minute":30}"#,
        )
        .expect("a question");
        assert_eq!(answer, "true");
        let answer = day_start_json(
            r#"{"rule":"moved_to_today","today":"2026-05-20","scheduled_date":"2026-05-25"}"#,
        )
        .expect("a question");
        assert_eq!(answer, r#"{"deadline_date":"2026-05-20"}"#);
        assert!(day_start_json(r#"{"rule":"tomorrow"}"#).is_err());
    }
}

/// The contract for the day-start plan: `tests/fixtures/dayStartPlan.json`,
/// measured from the composition the day-start sites wrote inline. The rows the
/// port changed or added on purpose say so in their notes. Its other half is
/// `src/components/dayStartPlan.contract.test.ts`, through the door.
#[cfg(test)]
mod plan_contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/dayStartPlan.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the contract parses")
    }

    /// The base task with one case's overrides laid over it, a user reduced
    /// to its id.
    fn row(doc: &Value, overrides: &Value) -> DayStartTask {
        let mut row = doc["baseTask"].clone();
        for (key, value) in overrides.as_object().expect("an override object") {
            row[key.as_str()] = value.clone();
        }
        let ids: Vec<Value> = row["assignees"]
            .as_array()
            .expect("assignees is an array")
            .iter()
            .map(|user| user["id"].clone())
            .collect();
        row["assignees"] = Value::Array(ids);
        serde_json::from_value(row).expect("a task row deserializes")
    }

    /// Every list the tasks live in, resolved the way both surfaces resolve
    /// it: the override per field, else the global.
    fn lists(case: &Value, tasks: &[DayStartTask]) -> Vec<DayStartListSettings> {
        let lists = &case["input"]["lists"];
        let mut seen = HashSet::new();
        tasks
            .iter()
            .filter(|t| seen.insert(t.list_id.clone()))
            .map(|t| {
                let over = &lists["overrides"][t.list_id.as_str()];
                let cascade = over["cascade"]
                    .as_bool()
                    .or(lists["cascade"].as_bool())
                    .expect("a coupling");
                let carry = if over["carryOverDefault"].is_string() {
                    &over["carryOverDefault"]
                } else {
                    &lists["carryOverDefault"]
                };
                DayStartListSettings {
                    list_id: t.list_id.clone(),
                    cascade,
                    carry_over_default: serde_json::from_value(carry.clone())
                        .expect("a carry-over default"),
                }
            })
            .collect()
    }

    fn identities(case: &Value) -> Vec<DayStartIdentity> {
        case["input"]["currentUserByList"]
            .as_object()
            .map(|by_list| {
                by_list
                    .iter()
                    .map(|(list, me)| DayStartIdentity {
                        list_id: list.clone(),
                        me: me.as_str().map(str::to_string),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn ids(tasks: &[DayStartTask], positions: &[usize]) -> Value {
        json!(positions
            .iter()
            .map(|&i| tasks[i].id.as_str())
            .collect::<Vec<_>>())
    }

    #[test]
    fn every_plan_row_holds() {
        let doc = doc();
        let cases = doc["cases"].as_array().expect("cases is an array");
        for name in [
            "per-list-overrides-split-one-morning",
            "overdue-counts-whatever-the-list-carries",
            "the-count-adds-all-three-sections",
            "a-coupled-root-brings-its-actionable-descendants",
            "a-subtask-in-a-backlog-list-follows-its-today-root",
            "a-colleagues-subtask-stays-behind",
            "the-scheduler-plans-a-future-morning",
        ] {
            assert!(
                cases.iter().any(|c| c["name"] == name),
                "the contract lost `{name}`"
            );
        }
        for case in cases {
            let input = &case["input"];
            let tasks: Vec<DayStartTask> = input["tasks"]
                .as_array()
                .expect("tasks is an array")
                .iter()
                .map(|over| row(&doc, over))
                .collect();
            let s = &input["reminders"];
            let settings = DayStartReminderSettings {
                remind_untimed_today: s["remindUntimedToday"].as_bool().expect("a toggle"),
                remind_deadline_arrived: s["remindDeadlineArrived"].as_bool().expect("a toggle"),
                remind_deadline_countdown: s["remindDeadlineCountdown"]
                    .as_bool()
                    .expect("a toggle"),
                deadline_countdown_days: s["deadlineCountdownDays"].as_f64(),
            };
            let day = input["anchor"]
                .as_str()
                .or(input["today"].as_str())
                .expect("a day");
            let p = plan(
                &tasks,
                day,
                &identities(case),
                &lists(case, &tasks),
                &settings,
            );
            let got = json!({
                "overdue": ids(&tasks, &p.overdue),
                "ask": ids(&tasks, &p.ask),
                "today": ids(&tasks, &p.today),
                "backlog": ids(&tasks, &p.backlog),
                "todayTargets": ids(&tasks, &p.today_targets),
                "backlogTargets": ids(&tasks, &p.backlog_targets),
                "untimed": ids(&tasks, &p.reminders.untimed),
                "dueToday": ids(&tasks, &p.reminders.due_today),
                "countdown": ids(&tasks, &p.reminders.countdown),
                "surfaced": p.surfaced,
            });
            assert_eq!(got, case["expect"], "{}", case["name"]);
        }
    }

    fn task(id: &str, parent: Option<&str>, plan: Option<&str>) -> DayStartTask {
        DayStartTask {
            id: id.into(),
            list_id: "list".into(),
            status: TaskStatus::Open,
            parent_id: parent.map(Into::into),
            scheduled_date: plan.map(Into::into),
            scheduled_time: None,
            deadline_date: None,
            deadline_reminder_days: None,
            assignees: Vec::new(),
        }
    }

    fn quiet() -> DayStartReminderSettings {
        DayStartReminderSettings {
            remind_untimed_today: false,
            remind_deadline_arrived: false,
            remind_deadline_countdown: false,
            deadline_countdown_days: Some(3.0),
        }
    }

    #[test]
    fn a_list_the_question_does_not_name_is_asked_and_brings_nothing() {
        let tasks = vec![
            task("p", None, Some("2026-05-10")),
            task("c", Some("p"), None),
        ];
        let p = plan(&tasks, "2026-05-20", &[], &[], &quiet());
        assert_eq!(p.ask, vec![0]);
        assert!(p.today.is_empty() && p.backlog.is_empty());
        assert!(p.today_targets.is_empty() && p.backlog_targets.is_empty());
        assert_eq!(p.surfaced, 1);
    }

    #[test]
    fn a_later_row_with_a_collected_id_takes_its_place() {
        // Two accounts, one id: the second row replaces the first in the batch,
        // in the first one's slot, as the JavaScript `Map` did.
        let tasks = vec![
            task("x", None, Some("2026-05-10")),
            task("x", None, Some("2026-05-11")),
        ];
        let lists = [DayStartListSettings {
            list_id: "list".into(),
            cascade: false,
            carry_over_default: CarryOverDefault::Today,
        }];
        let p = plan(&tasks, "2026-05-20", &[], &lists, &quiet());
        assert_eq!(p.today, vec![0, 1]);
        assert_eq!(p.today_targets, vec![1]);
    }

    #[test]
    fn the_door_answers_the_plan() {
        let answer = day_start_json(
            r#"{"rule":"plan","tasks":[],"today":"2026-05-20","settings":{"remind_untimed_today":true,"remind_deadline_arrived":true,"remind_deadline_countdown":true,"deadline_countdown_days":3}}"#,
        )
        .expect("a question");
        assert_eq!(
            serde_json::from_str::<Value>(&answer).expect("an answer"),
            json!({
                "overdue": [], "ask": [], "today": [], "backlog": [],
                "today_targets": [], "backlog_targets": [],
                "reminders": { "untimed": [], "due_today": [], "countdown": [] },
                "surfaced": 0
            })
        );
    }
}
