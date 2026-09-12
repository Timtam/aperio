//! Grouping the task view: which group a task lands in, in which order, under
//! which header, at what depth, and whether a collapse hides it.
//!
//! This is the rule both surfaces render the task list from, and it used to be
//! TypeScript in `shared/taskGrouping.ts` — asked inside `useMemo` on the
//! desktop and on the phone alike. It moved here so that a frontend which is
//! not JavaScript can ask the same question and get the same answer, and so
//! that the answer exists once.
//!
//! # What the core answers, and what it deliberately does not
//!
//! The answer is a flat list of [`Row`]s in depth-first order: a real task by
//! its id, or a synthetic header carrying its [`GroupKind`], its count(s) and
//! the ids it points at. It never carries a title. "Erledigt (3)" and
//! "Inbox (2)" are the caller's to build from kind + count + ids — the core
//! cannot hold the `t` callback that turns a key into words, and it should not
//! (DESIGN §4.5 a). Neither does it carry the caller's task rows back: the
//! caller holds them already, and echoing them would double the payload for
//! nothing. Positions, not rows.
//!
//! The synthetic header ids (`__aperio_done_group__`, `grp:bl:list:<list>`,
//! `grp:sec:bl:<list>:<section>`, …) are part of the contract. Both surfaces
//! persist them as collapse keys, so a change to one would silently expand
//! every group the user had folded.
//!
//! # The shape of the grouping
//!
//! In `state` mode: Überfällig (a fixed day already past) → Heute → Backlog
//! (no planned day; grouped by list, then section) → Zukünftig (a fixed future
//! day, or a backlog task whose `resurface_date` is still ahead, DESIGN §9.12)
//! → Erledigt → Abgebrochen. In `list` mode every open task sits in its list
//! (with sections) regardless of state; only the two terminal groups stay
//! separate. Subtasks follow their parent depth-first in either mode, and a
//! terminal subtask under an open parent stays inline rather than being
//! hoisted.
//!
//! Every answer here is pinned by `tests/fixtures/taskGrouping.json`, measured
//! by running the TypeScript this replaces; the `contract` module below reads
//! it, and so does the TypeScript contract test on the other side of the
//! boundary.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::collation::{compare_names, compare_titles, CollationLanguage};
use crate::task_assignment::is_mine_or_unassigned;
use crate::task_priority::{priority_rank, PriorityScale};
use crate::types::{Section, TaskPriority, TaskStatus, TaskUser};

/// Sentinel id of the synthetic "Erledigt (N)" header.
pub const DONE_GROUP_ID: &str = "__aperio_done_group__";
/// Sentinel id of the synthetic "Backlog (N)" header.
pub const BACKLOG_GROUP_ID: &str = "__aperio_backlog_group__";
/// Sentinel id of the synthetic "Zukünftig (N)" header.
pub const DEFERRED_GROUP_ID: &str = "__aperio_deferred_group__";
/// Sentinel id of the synthetic "Überfällig (N)" header.
pub const OVERDUE_GROUP_ID: &str = "__aperio_overdue_group__";
/// Sentinel id of the synthetic "Heute (N)" header.
pub const TODAY_GROUP_ID: &str = "__aperio_today_group__";
/// Sentinel id of the synthetic "Abgebrochen (N)" header.
pub const CANCELLED_GROUP_ID: &str = "__aperio_cancelled_group__";

/// What the rule reads of a task — by the same field names as `Task`, so a
/// full task's JSON deserializes into it and the caller may send either.
#[derive(Debug, Clone, Deserialize)]
pub struct GroupableTask {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    #[serde(default)]
    pub scheduled_date: Option<NaiveDate>,
    #[serde(default)]
    pub resurface_date: Option<NaiveDate>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub section_id: Option<String>,
    #[serde(default)]
    pub assignees: Vec<TaskUser>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
}

/// How the top level is grouped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    /// By lifecycle: Überfällig → Heute → Backlog → Zukünftig → Erledigt →
    /// Abgebrochen. The historical grouping.
    #[default]
    State,
    /// Every open task in its own list (+ sections), regardless of state;
    /// only the terminal groups stay separate.
    List,
}

/// Everything the rule needs, and nothing it does not.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupingInput {
    pub tasks: Vec<GroupableTask>,
    /// List id → display name. A list absent here is named — and ordered —
    /// by its id.
    #[serde(default)]
    pub lists: HashMap<String, String>,
    /// List id → its sections, as the store holds them. Order irrelevant:
    /// sections are sorted by name here.
    #[serde(default)]
    pub sections: HashMap<String, Vec<Section>>,
    /// The day the deferred gate and the time groups compare against. A
    /// parameter, never the clock (see `tests/core_contracts.rs`).
    pub today: NaiveDate,
    /// List id → the connected user, for the Erledigt mine/others split.
    /// Absent or `null` means a personal list: everything counts as mine.
    #[serde(default)]
    pub current_user_by_list: HashMap<String, Option<TaskUser>>,
    #[serde(default)]
    pub group_by: GroupBy,
    /// The user's priority system — decides how many bands the sibling
    /// ordering has.
    #[serde(default)]
    pub scale: PriorityScale,
    /// Ids whose children are hidden: task ids and synthetic header ids alike.
    #[serde(default)]
    pub collapsed: HashSet<String>,
    /// The app's language tag, for the collation of titles and names. Empty
    /// answers the default, like every unknown tag.
    #[serde(default)]
    pub language: String,
}

/// What kind of header a synthetic row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupKind {
    Backlog,
    List,
    Section,
    Done,
    Deferred,
    Overdue,
    Today,
    Cancelled,
}

/// What it takes to word a header — and to find it again.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupHead {
    pub kind: GroupKind,
    /// The enclosing header's id; `None` at the top level. What a synthetic
    /// row's `parent_id` is on the caller's side.
    pub parent_id: Option<String>,
    /// The owning list, for list and section headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    /// The section row, for section headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    /// Tasks under this header. The whole subtree for the active groups; the
    /// top-level items only for the terminal ones (Erledigt, Abgebrochen).
    pub count: usize,
    /// Present only when Erledigt splits: done tasks that are mine or nobody's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mine: Option<usize>,
    /// Present only when Erledigt splits: done tasks assigned to someone else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub others: Option<usize>,
}

/// One row of the flattened tree, in depth-first order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// A task id, or a synthetic header id.
    pub id: String,
    /// 0 at the top; +1 per nesting level.
    pub depth: usize,
    /// True when an ancestor is collapsed. The row stays in the list so the
    /// index space is stable across a collapse; the renderer skips it.
    pub hidden: bool,
    pub has_children: bool,
    /// Present when the row is a header rather than a task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupHead>,
}

/// A backlog task is **deferred** when its `resurface_date` is strictly after
/// `today`: it is waiting to come back and is held out of the active backlog
/// (DESIGN §9.3 / §9.12). One gate for every backlog surface, so the task view
/// and the backlog rail can never show the same task in two places.
pub fn is_task_deferred(resurface_date: Option<NaiveDate>, today: NaiveDate) -> bool {
    resurface_date.is_some_and(|day| day > today)
}

/// Sibling order everywhere: priority band first (how many bands there are is
/// the user's `scale`), then the title, natural — "Aufgabe 2" before
/// "Aufgabe 10".
fn task_order(
    a: &GroupableTask,
    b: &GroupableTask,
    scale: PriorityScale,
    language: CollationLanguage,
) -> Ordering {
    priority_rank(a.priority, scale)
        .cmp(&priority_rank(b.priority, scale))
        .then_with(|| compare_titles(&a.title, &b.title, language))
}

/// What a done task sorts by inside Erledigt: `completed_at` when the provider
/// gave one, `updated_at` when it did not. Vikunja stamps `done_at` only when
/// a task FLIPS to done, so a task created already-done — a completion record
/// for a repeating task — never gets one; `updated_at` is its completion
/// moment. The empty-key fallback this replaced filed those as the oldest
/// thing in the list seconds after they were ticked off.
fn done_order_key(task: &GroupableTask) -> DateTime<Utc> {
    task.completed_at.unwrap_or(task.updated_at)
}

/// A node of the grouping forest before it is flattened.
enum Node<'a> {
    Task(&'a GroupableTask),
    Group {
        id: String,
        kind: GroupKind,
        list_id: Option<String>,
        section_id: Option<String>,
        count: usize,
        mine: Option<usize>,
        others: Option<usize>,
        children: Vec<Node<'a>>,
    },
}

/// What every step of the grouping reads: the input, the resolved language,
/// and the subtask buckets once the cycle guard has settled them.
struct Grouping<'a> {
    input: &'a GroupingInput,
    language: CollationLanguage,
    children: HashMap<&'a str, Vec<&'a GroupableTask>>,
}

impl<'a> Grouping<'a> {
    /// Tasks contained under one task: its whole subtask subtree.
    fn count_subtasks(&self, id: &str) -> usize {
        self.children
            .get(id)
            .map(|kids| {
                kids.iter()
                    .map(|kid| 1 + self.count_subtasks(&kid.id))
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Tasks contained under a list of tasks — the "(N)" on headers.
    fn total_under(&self, items: &[&'a GroupableTask]) -> usize {
        items
            .iter()
            .map(|task| 1 + self.count_subtasks(&task.id))
            .sum()
    }

    fn name_of(&self, list_id: &str) -> String {
        self.input
            .lists
            .get(list_id)
            .cloned()
            .unwrap_or_else(|| list_id.to_string())
    }

    fn by_name(&self, a: &str, b: &str) -> Ordering {
        compare_names(&self.name_of(a), &self.name_of(b), self.language)
    }

    /// A list's tasks → the ungrouped ones, then one group per non-empty
    /// section, by section name. `scope` keeps the synthetic section ids apart
    /// between a list's backlog and list-mode appearances.
    fn list_children(
        &self,
        list_id: &str,
        items: &[&'a GroupableTask],
        scope: &str,
    ) -> Vec<Node<'a>> {
        let Some(sections) = self.input.sections.get(list_id).filter(|s| !s.is_empty()) else {
            return items.iter().map(|task| Node::Task(task)).collect();
        };
        let section_ids: HashSet<&str> = sections.iter().map(|s| s.id.as_str()).collect();
        let mut by_section: HashMap<&str, Vec<&'a GroupableTask>> = HashMap::new();
        let mut ungrouped: Vec<&'a GroupableTask> = Vec::new();
        for task in items {
            match task
                .section_id
                .as_deref()
                .filter(|s| section_ids.contains(s))
            {
                Some(section) => by_section.entry(section).or_default().push(task),
                None => ungrouped.push(task),
            }
        }
        let mut out: Vec<Node<'a>> = ungrouped.iter().map(|task| Node::Task(task)).collect();
        // By NAME, like the tasks inside them. `Section.order` is creation
        // order wearing the name of a preference — nothing can move a section.
        let mut sorted: Vec<&Section> = sections.iter().collect();
        sorted.sort_by(|a, b| compare_titles(&a.name, &b.name, self.language));
        for section in sorted {
            let Some(sec_tasks) = by_section.get(section.id.as_str()) else {
                continue;
            };
            out.push(Node::Group {
                id: format!("grp:sec:{scope}:{}", section.id),
                kind: GroupKind::Section,
                list_id: Some(list_id.to_string()),
                section_id: Some(section.id.clone()),
                count: self.total_under(sec_tasks),
                mine: None,
                others: None,
                children: sec_tasks.iter().map(|task| Node::Task(task)).collect(),
            });
        }
        out
    }

    /// Tasks grouped by list, by list name. First-seen order underneath, and
    /// the sort is stable, so two lists sharing a name keep their arrival
    /// order.
    fn by_list(&self, items: &[&'a GroupableTask]) -> Vec<(String, Vec<&'a GroupableTask>)> {
        let mut groups: Vec<(String, Vec<&'a GroupableTask>)> = Vec::new();
        for task in items {
            match groups.iter_mut().find(|(id, _)| *id == task.list_id) {
                Some((_, bucket)) => bucket.push(task),
                None => groups.push((task.list_id.clone(), vec![task])),
            }
        }
        groups.sort_by(|(a, _), (b, _)| self.by_name(a, b));
        groups
    }

    /// A list header with its sections and tasks underneath.
    fn list_group(
        &self,
        id: String,
        list_id: String,
        items: &[&'a GroupableTask],
        scope: &str,
    ) -> Node<'a> {
        Node::Group {
            id,
            kind: GroupKind::List,
            count: self.total_under(items),
            children: self.list_children(&list_id, items, scope),
            list_id: Some(list_id),
            section_id: None,
            mine: None,
            others: None,
        }
    }

    /// A flat group: the tasks straight underneath, no list or section level.
    fn flat_group(&self, id: &str, kind: GroupKind, items: &[&'a GroupableTask]) -> Node<'a> {
        Node::Group {
            id: id.to_string(),
            kind,
            list_id: None,
            section_id: None,
            count: self.total_under(items),
            mine: None,
            others: None,
            children: items.iter().map(|task| Node::Task(task)).collect(),
        }
    }

    // Depth-first emit. Hidden rows stay in the list so the index space is
    // stable across a collapse; the renderer skips them.
    fn emit_task(&self, task: &'a GroupableTask, depth: usize, hidden: bool, rows: &mut Vec<Row>) {
        let kids = self
            .children
            .get(task.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        rows.push(Row {
            id: task.id.clone(),
            depth,
            hidden,
            has_children: !kids.is_empty(),
            group: None,
        });
        let child_hidden = hidden || self.input.collapsed.contains(&task.id);
        for kid in kids {
            self.emit_task(kid, depth + 1, child_hidden, rows);
        }
    }

    fn emit_node(
        &self,
        node: Node<'a>,
        depth: usize,
        hidden: bool,
        parent_id: Option<&str>,
        rows: &mut Vec<Row>,
    ) {
        match node {
            Node::Task(task) => self.emit_task(task, depth, hidden, rows),
            Node::Group {
                id,
                kind,
                list_id,
                section_id,
                count,
                mine,
                others,
                children,
            } => {
                rows.push(Row {
                    id: id.clone(),
                    depth,
                    hidden,
                    has_children: !children.is_empty(),
                    group: Some(GroupHead {
                        kind,
                        parent_id: parent_id.map(str::to_string),
                        list_id,
                        section_id,
                        count,
                        mine,
                        others,
                    }),
                });
                let child_hidden = hidden || self.input.collapsed.contains(&id);
                for kid in children {
                    self.emit_node(kid, depth + 1, child_hidden, Some(&id), rows);
                }
            }
        }
    }
}

fn mark_reachable<'a>(
    task: &'a GroupableTask,
    children: &HashMap<&'a str, Vec<&'a GroupableTask>>,
    reachable: &mut HashSet<&'a str>,
) {
    if !reachable.insert(task.id.as_str()) {
        return;
    }
    if let Some(kids) = children.get(task.id.as_str()) {
        for kid in kids {
            mark_reachable(kid, children, reachable);
        }
    }
}

/// The task view's rows, in depth-first order. See the module doc for the
/// shape; `tests/fixtures/taskGrouping.json` for every pinned answer.
pub fn group_tasks(input: &GroupingInput) -> Vec<Row> {
    let language = CollationLanguage::from_tag(&input.language);
    let scale = input.scale;
    let today = input.today;
    let tasks = &input.tasks;
    let order = |a: &&GroupableTask, b: &&GroupableTask| task_order(a, b, scale, language);

    // Bucket children under their parent. A parent_id pointing at a task not
    // in the snapshot is an orphan → top level.
    let all_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<&GroupableTask>> = HashMap::new();
    let mut top_level: Vec<&GroupableTask> = Vec::new();
    for task in tasks {
        match task.parent_id.as_deref().filter(|p| all_ids.contains(p)) {
            Some(parent) => children.entry(parent).or_default().push(task),
            None => top_level.push(task),
        }
    }
    // Subtask siblings sort like top-level tasks: A→Z within a band, not
    // add-order.
    for bucket in children.values_mut() {
        bucket.sort_by(order);
    }

    // Cycle guard. Parent links can come from external providers (two Vikunja
    // tasks each carrying a relation onto the other), and a parent CYCLE has no
    // top-level member — without this every task in the cycle, plus its
    // subtree, would silently vanish, and emitting one would recurse forever.
    // Chase each unreachable task's parent chain onto its cycle and promote
    // that member to the top, cutting its parent edge in the LOCAL buckets only.
    let mut reachable: HashSet<&str> = HashSet::new();
    for task in &top_level {
        mark_reachable(task, &children, &mut reachable);
    }
    if reachable.len() < tasks.len() {
        let by_id: HashMap<&str, &GroupableTask> =
            tasks.iter().map(|t| (t.id.as_str(), t)).collect();
        for task in tasks {
            if reachable.contains(task.id.as_str()) {
                continue;
            }
            // Walk up until the next parent would revisit the chain — `member`
            // is then on the cycle. (Every parent exists here: a missing one
            // would have put the task at top level and made the cluster
            // reachable.)
            let mut member = task;
            let mut walked: HashSet<&str> = HashSet::from([member.id.as_str()]);
            while let Some(parent_id) = member.parent_id.as_deref() {
                let Some(parent) = by_id.get(parent_id) else {
                    break;
                };
                if !walked.insert(parent.id.as_str()) {
                    break;
                }
                member = parent;
            }
            if let Some(parent_id) = member.parent_id.as_deref() {
                if let Some(bucket) = children.get_mut(parent_id) {
                    bucket.retain(|child| child.id != member.id);
                }
            }
            top_level.push(member);
            mark_reachable(member, &children, &mut reachable);
        }
    }

    let g = Grouping {
        input,
        language,
        children,
    };

    // Terminal top-level tasks collapse into their own end-of-list groups so
    // the active groups show only open work. A terminal SUBTASK under an open
    // parent stays inline. Splitting cancelled out here also keeps a cancelled
    // task with a past day OUT of Überfällig.
    let mut done_top: Vec<&GroupableTask> = Vec::new();
    let mut cancelled_top: Vec<&GroupableTask> = Vec::new();
    let mut open_top: Vec<&GroupableTask> = Vec::new();
    for task in &top_level {
        match task.status {
            TaskStatus::Completed => done_top.push(task),
            TaskStatus::Cancelled => cancelled_top.push(task),
            TaskStatus::Open | TaskStatus::InProgress => open_top.push(task),
        }
    }

    // One sort covers backlog, lists, sections and the flat groups alike: every
    // downstream bucket is filled by walking `open_top` in this order.
    open_top.sort_by(order);

    // Deferred (DESIGN §9.12): held out of the active groups; joins the
    // fixed-future-scheduled tasks under Zukünftig below.
    let (deferred, active): (Vec<&GroupableTask>, Vec<&GroupableTask>) = open_top
        .iter()
        .partition(|task| is_task_deferred(task.resurface_date, today));

    // The active tasks by their planned day relative to `today`.
    let mut backlog: Vec<&GroupableTask> = Vec::new();
    let mut overdue: Vec<&GroupableTask> = Vec::new();
    let mut today_tasks: Vec<&GroupableTask> = Vec::new();
    let mut future_scheduled: Vec<&GroupableTask> = Vec::new();
    for task in active {
        match task.scheduled_date {
            None => backlog.push(task),
            Some(day) if day < today => overdue.push(task),
            Some(day) if day == today => today_tasks.push(task),
            Some(_) => future_scheduled.push(task),
        }
    }

    // Zukünftig reads as a countdown: by the effective future day, soonest
    // first — the resurface day for a deferred task, the scheduled day else.
    let future_day_of = |task: &GroupableTask| -> NaiveDate {
        if is_task_deferred(task.resurface_date, today) {
            task.resurface_date
        } else {
            task.scheduled_date
        }
        .unwrap_or(NaiveDate::MIN)
    };
    let mut future: Vec<&GroupableTask> = deferred.into_iter().chain(future_scheduled).collect();
    future.sort_by_key(|task| future_day_of(task));

    let mut forest: Vec<Node<'_>> = Vec::new();

    // List mode: every open task in its own list (+ sections), regardless of
    // state; the terminal groups follow below as in state mode.
    if input.group_by == GroupBy::List {
        for (list_id, items) in g.by_list(&open_top) {
            let scope = format!("ls:{list_id}");
            forest.push(g.list_group(format!("grp:list:{list_id}"), list_id, &items, &scope));
        }
    }

    if input.group_by == GroupBy::State {
        // Überfällig first, as the most pressing. Flat, all lists together.
        if !overdue.is_empty() {
            forest.push(g.flat_group(OVERDUE_GROUP_ID, GroupKind::Overdue, &overdue));
        }
        // Heute. Flat.
        if !today_tasks.is_empty() {
            forest.push(g.flat_group(TODAY_GROUP_ID, GroupKind::Today, &today_tasks));
        }
        // Backlog → list → section. Grouping the backlog too is what makes a
        // Vikunja project's buckets visible when nothing is scheduled.
        if !backlog.is_empty() {
            let list_nodes: Vec<Node<'_>> = g
                .by_list(&backlog)
                .into_iter()
                .map(|(list_id, items)| {
                    let scope = format!("bl:{list_id}");
                    g.list_group(format!("grp:bl:list:{list_id}"), list_id, &items, &scope)
                })
                .collect();
            forest.push(Node::Group {
                id: BACKLOG_GROUP_ID.to_string(),
                kind: GroupKind::Backlog,
                list_id: None,
                section_id: None,
                count: g.total_under(&backlog),
                mine: None,
                others: None,
                children: list_nodes,
            });
        }
        // Zukünftig: deferred backlog + fixed future days, soonest first.
        if !future.is_empty() {
            forest.push(g.flat_group(DEFERRED_GROUP_ID, GroupKind::Deferred, &future));
        }
    }

    // Erledigt last but one, most recently completed first. The count splits
    // into mine / others when at least one done task is somebody else's;
    // personal lists (no identity) never split.
    if !done_top.is_empty() {
        done_top.sort_by_key(|task| std::cmp::Reverse(done_order_key(task)));
        let mine = done_top
            .iter()
            .filter(|task| {
                let me = input
                    .current_user_by_list
                    .get(&task.list_id)
                    .and_then(|user| user.as_ref());
                is_mine_or_unassigned(&task.assignees, me)
            })
            .count();
        let others = done_top.len() - mine;
        forest.push(Node::Group {
            id: DONE_GROUP_ID.to_string(),
            kind: GroupKind::Done,
            list_id: None,
            section_id: None,
            count: done_top.len(),
            mine: (others > 0).then_some(mine),
            others: (others > 0).then_some(others),
            children: done_top.iter().map(|task| Node::Task(task)).collect(),
        });
    }

    // Abgebrochen at the very end, most recently changed first — there is no
    // cancelled_at, so updated_at stands in. Counts its top-level items, like
    // Erledigt, not the subtree.
    if !cancelled_top.is_empty() {
        cancelled_top.sort_by_key(|task| std::cmp::Reverse(task.updated_at));
        forest.push(Node::Group {
            id: CANCELLED_GROUP_ID.to_string(),
            kind: GroupKind::Cancelled,
            list_id: None,
            section_id: None,
            count: cancelled_top.len(),
            mine: None,
            others: None,
            children: cancelled_top.iter().map(|task| Node::Task(task)).collect(),
        });
    }

    let mut rows: Vec<Row> = Vec::new();
    for root in forest {
        g.emit_node(root, 0, false, None, &mut rows);
    }
    rows
}

/// [`group_tasks`] over the wire: a [`GroupingInput`] as JSON in, the rows as
/// JSON out. The shape every door speaks.
pub fn group_tasks_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: GroupingInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&group_tasks(&input))
}

/// The contract with the TypeScript this replaced.
///
/// Its other half is `src/components/views/taskGrouping.contract.test.ts`,
/// reading this same file. The fixture was written by running the TypeScript
/// over the table BEFORE the port, so the port is measured against what was,
/// not against what it thinks should be.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/taskGrouping.json"
    ));

    /// A case's task overrides, laid over the fixture's base task — the same
    /// spread the TypeScript side does.
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

    fn input_of(doc: &Value, case: &Value) -> GroupingInput {
        let base = &doc["baseTask"];
        let mut input = case["input"].clone();
        let tasks: Vec<Value> = input["tasks"]
            .as_array()
            .expect("tasks is an array")
            .iter()
            .map(|over| merged(base, over))
            .collect();
        input["tasks"] = Value::Array(tasks);
        // The language the answers were measured under.
        if input.get("language").is_none() {
            input["language"] = doc["language"].clone();
        }
        if input["language"].is_null() {
            input["language"] = json!("");
        }
        serde_json::from_value(input).expect("the case's input deserializes")
    }

    #[test]
    fn every_case_in_the_contract_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let cases = doc["cases"].as_array().expect("cases is an array");

        // Anti-silence: named rows, not a count. Each is the one case a whole
        // rule turns on; a fixture that lost it would pass for the wrong reason.
        for needed in [
            "done-split-when-someone-else-owns-one",
            "time-groups-in-order",
            "sections-by-name-not-by-order",
            "parent-cycle-surfaces-instead-of-vanishing",
            "done-falls-back-to-updated-at",
            "two-level-scale-has-two-bands",
            "collapsing-a-list-hides-its-sections-and-tasks",
        ] {
            assert!(
                cases.iter().any(|c| c["name"] == needed),
                "the contract lost `{needed}`",
            );
        }

        for case in cases {
            let name = case["name"].as_str().expect("every case has a name");
            let input = input_of(&doc, case);
            let got = serde_json::to_value(group_tasks(&input)).expect("rows serialize");
            assert_eq!(
                got,
                case["expect"]["rows"],
                "{name}: {}",
                case["note"].as_str().unwrap_or(""),
            );
        }
    }

    #[test]
    fn every_deferred_row_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let rows = doc["deferred"].as_array().expect("deferred is an array");
        assert!(!rows.is_empty(), "the contract carries isTaskDeferred rows");
        for row in rows {
            let resurface: Option<NaiveDate> =
                serde_json::from_value(row["resurface_date"].clone()).expect("a date or null");
            let today: NaiveDate =
                serde_json::from_value(row["today"].clone()).expect("today is a date");
            assert_eq!(
                is_task_deferred(resurface, today),
                row["deferred"].as_bool().expect("deferred is a bool"),
                "{}",
                row["note"].as_str().unwrap_or(""),
            );
        }
    }

    #[test]
    fn the_wire_accepts_a_full_task_and_defaults_the_rest() {
        // A caller may send whole `Task` rows; the fields the rule does not
        // read are ignored, and every optional input has a default.
        let input = r#"{
            "tasks": [{
                "id": "t", "list_id": "L", "title": "T", "description": null,
                "status": "open", "priority": "medium", "effort": "large",
                "scheduled_date": null, "scheduled_time": null, "deadline_date": null,
                "deadline_time": null, "recurrence": null, "parent_id": null,
                "color_label": null, "reminders": [], "sound": null,
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
                "completed_at": null, "etag": null
            }],
            "today": "2026-05-21"
        }"#;
        let rows: Vec<Row> =
            serde_json::from_str(&group_tasks_json(input).expect("the input is valid"))
                .expect("rows round-trip");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, [BACKLOG_GROUP_ID, "grp:bl:list:L", "t"]);
    }
}
