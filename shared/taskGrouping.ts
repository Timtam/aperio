// The task view's grouping — this surface's door into
// `cal_core::task_grouping`.
//
// Which group a task lands in, in which order, under which header, at what
// depth, and whether a collapse hides it: that decision lives in the core now,
// and both surfaces ask it while rendering. What stays here is the shell —
// turning the core's answer (ids, kinds, counts, positions) back into the
// `Entry` rows the views render, and wording the headers with `t`, which the
// core cannot hold and must not (DESIGN §4.5 a).
//
// Pinned by `crates/cal-core/tests/fixtures/taskGrouping.json`, measured from
// the TypeScript this replaced; `taskGrouping.contract.test.ts` replays it
// through this door, and the core's own contract test reads the same file.

import type {
  GroupHead,
  GroupKind,
  GroupableTask,
  GroupingInput,
  GroupingRow,
  Section,
  Task,
  TaskGroupBy,
  TaskUser,
} from './types';
import { compareTitles } from './ordering';
import type { PriorityScale } from './taskStatus';

/** Sentinel id of the synthetic "Done (N)" group row. */
export const DONE_GROUP_ID = '__aperio_done_group__';
/** Sentinel id of the synthetic "Backlog" group row. */
export const BACKLOG_GROUP_ID = '__aperio_backlog_group__';
/** Sentinel id of the synthetic "Zukünftig (N)" group row — tasks waiting for a
 *  future day: backlog tasks whose `resurface_date` is still ahead (DESIGN
 *  §9.12) AND tasks scheduled on a fixed FUTURE day. */
export const DEFERRED_GROUP_ID = '__aperio_deferred_group__';
/** Sentinel id of the synthetic "Überfällig (N)" group row — open tasks whose
 *  fixed scheduled day is already in the past. */
export const OVERDUE_GROUP_ID = '__aperio_overdue_group__';
/** Sentinel id of the synthetic "Heute (N)" group row — open tasks scheduled for
 *  today. */
export const TODAY_GROUP_ID = '__aperio_today_group__';
/** Sentinel id of the synthetic "Abgebrochen (N)" group row — cancelled tasks. */
export const CANCELLED_GROUP_ID = '__aperio_cancelled_group__';

// `TaskGroupBy` (how the top level groups: 'state' by lifecycle, 'list' by
// list) is generated from `cal_core::task_grouping::TaskGroupBy` and exported
// by `./types`, like every other wire type of this door.

/**
 * A backlog task is **deferred** when its `resurface_date` is strictly after
 * `today` (`YYYY-MM-DD`): it's waiting to come back and must be held out of
 * the active backlog (DESIGN §9.3 / §9.12). The week/month backlog rail asks
 * this directly; the task view's grouping asks the core, whose
 * `is_task_deferred` is the same comparison. The two are pinned against each
 * other by the `deferred` rows of `taskGrouping.json`, read by both contract
 * tests.
 */
export function isTaskDeferred(task: Task, today: string): boolean {
  return task.resurface_date != null && task.resurface_date > today;
}

/** Metadata carried by a group-header row (Backlog / a list / a section /
 *  the synthetic Done or Deferred group). When `group` is set on an
 *  {@link Entry}, the row is a collapsible header rather than a real task. */
export interface GroupMeta {
  kind: GroupKind;
  /** Section row id — present only for `kind: 'section'`; lets the header
   *  tint to the section colour and offer the ⋮ actions. */
  sectionId?: string;
  /** The owning list — present for list + section headers. */
  listId?: string;
  /** The section row itself (for the colour + ⋮ menu + drop target). */
  section?: Section;
}

/**
 * One row in the flattened task tree the TaskView renders. Every row is a
 * `treeitem`: real tasks, plus the synthetic group headers (Backlog, each
 * list, each section, the Done group) which carry {@link GroupMeta}. Headers
 * being real tree rows is what lets a screen-reader user arrow onto them and
 * hear "Backlog, level 1, collapsed" instead of the grouping living only in
 * each task's label.
 */
export interface Entry {
  kind: 'task';
  task: Task;
  listName: string;
  /** Position in `flatTasks` — what `focusIndex` indexes into. */
  index: number;
  /** 0 for a top-level row, +1 per nesting level (drives aria-level + indent). */
  depth: number;
  /** Set when this row has at least one child row. */
  hasChildren: boolean;
  /** True when an ancestor row is collapsed (renderer skips it; the index
   *  space stays stable so keyboard nav can clamp). */
  hidden: boolean;
  /** Present when this row is a group header rather than a real task. */
  group?: GroupMeta;
}

/** Minimal synthetic Task standing in for a group header. Only its `id`
 *  (collapse key + activedescendant target) and `parent_id` (Left-arrow
 *  parent jump) matter — the render branches on `entry.group` before it
 *  would read any task-shaped field. */
function groupTask(
  id: string,
  title: string,
  listId: string,
  parentId: string | null,
): Task {
  return {
    id,
    list_id: listId,
    title,
    description: null,
    status: 'open',
    priority: 'medium',
    effort: 'medium',
    scheduled_date: null,
    scheduled_time: null,
    scheduled_end_time: null,
    deadline_date: null,
    deadline_time: null,
    deadline_reminder_days: null,
    recurrence: null,
    resurface_date: null,
    series_id: null,
    parent_id: parentId,
    section_id: null,
    color_label: null,
    reminders: [],
    assignees: [],
    sound: null,
    created_at: '',
    updated_at: '',
    completed_at: null,
    etag: null,
  };
}

/** Natural (numeric-aware) ascending title compare: "Aufgabe 2" sorts before
 *  "Aufgabe 10", not after. */
function naturalCompare(a: string, b: string): number {
  // `cal_core::compare_titles` through the surface's door — see
  // `installTextCollation`. Digit runs order by value, so "Kapitel 2" comes
  // before "Kapitel 10", and umlauts sort with their base letter instead of
  // behind every ASCII word.
  return compareTitles(a, b);
}

// `taskOrder` — priority band, then natural title — is the core's now:
// `cal_core::task_grouping::task_order`, asked by the task view through
// `buildEntries` and by the calendar days through `filterTasksOnDay`. The
// TypeScript twin went with the last caller that used it directly.

/**
 * THE section ordering: by name, the same natural compare the tasks inside
 * them use.
 *
 * `Section.order` is what this replaces, and it was never a user's choice.
 * Nothing in either frontend lets a section be dragged or renumbered — the
 * field is assigned once at creation as "append to the end" and never touched
 * again. So it is creation order wearing the name of a preference, and reading
 * a list meant remembering which evening you happened to add which section.
 *
 * Applied at DISPLAY time rather than in the core: the field stays as the
 * providers send it, so a backend that grows a real reorder gesture later has
 * something to reorder, and nothing that writes `order` has to change today.
 *
 * The one place this must be applied and cannot be reached from here is a
 * caller that reads sections straight from the API instead of the store cache.
 */
export function sectionOrder(
  a: { name: string },
  b: { name: string },
): number {
  return naturalCompare(a.name, b.name);
}

/** `sections`, sorted for display. A copy — callers hold arrays that came out
 *  of a store or an API response, and sorting those in place mutates state
 *  somebody else is rendering from. */
export function sortSections<T extends { name: string }>(sections: T[]): T[] {
  return [...sections].sort(sectionOrder);
}

// ─────────────────────────────── The door ───────────────────────────────────

// What the rule reads of a task, and nothing else — `GroupableTask` from
// `./types`. Sent instead of the whole row: a task carries twice as many
// fields, and the core has no business knowing about the rest.
function onlyGroupable(task: Task): GroupableTask {
  return {
    id: task.id,
    list_id: task.list_id,
    title: task.title,
    status: task.status,
    priority: task.priority,
    scheduled_date: task.scheduled_date,
    resurface_date: task.resurface_date,
    parent_id: task.parent_id,
    section_id: task.section_id,
    assignees: task.assignees,
    updated_at: task.updated_at,
    completed_at: task.completed_at,
  };
}

/** This surface's door into `cal_core::task_grouping`. */
export interface TaskGroupingRules {
  groupTasksJson(inputJson: string): string;
  /** The app's language tag, read when the view asks: the collation of
   *  titles, section names and list names depends on it, and the user can
   *  change it while the app runs. */
  languageTag(): string;
}

let installedRules: TaskGroupingRules | null = null;

/** Bind this surface's door into the core. */
export function installTaskGroupingRules(rules: TaskGroupingRules): void {
  installedRules = rules;
}

function rules(): TaskGroupingRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the second
    // implementation all over again — and this one is asked on every render
    // of the task list, on both surfaces, so a quiet divergence would be the
    // most visible kind there is and still nobody would know which side is
    // right.
    throw new Error(
      'task grouping used before installTaskGroupingRules() — the surface must ' +
        'bind its door into cal_core::task_grouping at startup',
    );
  }
  return installedRules;
}

/** The words for a header, from what the core answered. The only place the
 *  grouping meets `t`. */
function headerTitle(
  head: GroupHead,
  t: (key: string, vars?: Record<string, unknown>) => string,
  nameOf: (listId: string) => string,
  section: Section | undefined,
): string {
  switch (head.kind) {
    case 'backlog':
      return `${t('views.tasks.backlog')} (${head.count})`;
    case 'list':
      return `${nameOf(head.listId ?? '')} (${head.count})`;
    case 'section':
      return `${section?.name ?? head.sectionId ?? ''} (${head.count})`;
    case 'done':
      // Split into mine (unassigned OR assigned to me) vs others (assigned to
      // a concrete other user) when at least one done task is someone else's;
      // otherwise a single count (personal lists never split).
      return head.others !== undefined && head.others > 0
        ? t('views.tasks.doneSplit', { mine: head.mine, others: head.others })
        : t('views.tasks.done', { count: head.count });
    case 'deferred':
      return t('views.tasks.deferred', { count: head.count });
    case 'overdue':
      return t('views.tasks.overdue', { count: head.count });
    case 'today':
      return t('views.tasks.today', { count: head.count });
    case 'cancelled':
      return t('views.tasks.cancelled', { count: head.count });
  }
}

/**
 * The rows the task view renders, in depth-first order, with every group
 * header as a real tree row.
 *
 * The core decides; this lays its answer over the tasks the caller holds
 * (positions, not rows — see the module comment) and words the headers.
 */
export function buildEntries(
  tasks: Task[],
  taskListById: Map<string, { name: string }>,
  t: (key: string, vars?: Record<string, unknown>) => string,
  collapsed: Set<string>,
  sectionsByList: Record<string, Section[]>,
  /** Today as `YYYY-MM-DD`; tasks whose `resurface_date` is strictly after
   *  it are held back in the "Zukünftig" group (DESIGN §9.12). */
  today: string,
  /** Connected user per list id (Vikunja & co.), used to split the Done count
   *  into "mine vs others". Lists without an identity are absent/null and never
   *  split. */
  currentUserByList: Record<string, TaskUser | null> = {},
  /** Top-level grouping (see {@link TaskGroupBy}). Defaults to `'state'` so
   *  existing callers keep the historical lifecycle grouping. */
  groupBy: TaskGroupBy = 'state',
  /** The user's priority system — decides how many bands the sibling ordering
   *  has (`cal_core::task_grouping::task_order`). Defaults to the three-level
   *  original. */
  scale: PriorityScale = 'three',
): { entries: Entry[]; flatTasks: Task[] } {
  const door = rules();
  const input: GroupingInput = {
    tasks: tasks.map(onlyGroupable),
    lists: Object.fromEntries(
      Array.from(taskListById, ([id, list]) => [id, list.name] as const),
    ),
    sections: sectionsByList,
    today,
    currentUserByList,
    groupBy,
    scale,
    collapsed: Array.from(collapsed),
    language: door.languageTag(),
  };
  const rows = JSON.parse(door.groupTasksJson(JSON.stringify(input))) as GroupingRow[];

  // Ids are unique within a snapshot (they are the store's primary key, and
  // the views already key their rows by them), so last-wins is never asked
  // to choose; the core answers with the same rows either way.
  const byId = new Map(tasks.map((task) => [task.id, task]));
  const nameOf = (listId: string) => taskListById.get(listId)?.name ?? listId;

  const entries: Entry[] = [];
  const flatTasks: Task[] = [];
  rows.forEach((row, index) => {
    if (!row.group) {
      const task = byId.get(row.id);
      if (!task) {
        // Cannot happen: the core answers with the ids it was given. If it
        // ever does, a silent skip would drop a task from the view.
        throw new Error(`task grouping answered with a task this view does not hold: ${row.id}`);
      }
      entries.push({
        kind: 'task',
        task,
        listName: nameOf(task.list_id),
        index,
        depth: row.depth,
        hasChildren: row.hasChildren,
        hidden: row.hidden,
      });
      flatTasks.push(task);
      return;
    }
    const head = row.group;
    const meta: GroupMeta = { kind: head.kind };
    if (head.listId !== undefined) meta.listId = head.listId;
    if (head.sectionId !== undefined) {
      meta.sectionId = head.sectionId;
      meta.section = (sectionsByList[head.listId ?? ''] ?? []).find(
        (section) => section.id === head.sectionId,
      );
    }
    const synthetic = groupTask(
      row.id,
      headerTitle(head, t, nameOf, meta.section),
      head.listId ?? '',
      head.parentId,
    );
    entries.push({
      kind: 'task',
      task: synthetic,
      listName: '',
      index,
      depth: row.depth,
      hasChildren: row.hasChildren,
      hidden: row.hidden,
      group: meta,
    });
    flatTasks.push(synthetic);
  });

  return { entries, flatTasks };
}
