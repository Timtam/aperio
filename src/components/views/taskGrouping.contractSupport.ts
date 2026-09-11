// Test support for the taskGrouping contract: the wire shape the core will
// answer with, and the reduction of today's TypeScript answer to that shape.
// Shared by the contract test (which replays the fixture) and the one-off
// measure test (which wrote it) — nothing in the app imports this.
import type { Section, Task, TaskUser } from '../../api/types';
import type { PriorityScale } from '../../../shared/taskStatus';
import { buildEntries, type Entry, type TaskGroupBy } from './taskGrouping';

/** One row of the answer, as the core will give it: a real task by id, or a
 *  synthetic header with what it takes to word it. */
export interface WireRow {
  id: string;
  depth: number;
  hidden: boolean;
  hasChildren: boolean;
  group?: {
    kind: string;
    parentId: string | null;
    listId?: string;
    sectionId?: string;
    count?: number;
    mine?: number;
    others?: number;
  };
}

export interface WireInput {
  tasks: Partial<Task>[];
  lists: Record<string, string>;
  sections?: Record<string, Section[]>;
  today: string;
  currentUserByList?: Record<string, TaskUser | null>;
  groupBy?: TaskGroupBy;
  scale?: PriorityScale;
  collapsed?: string[];
}

/** A `t` that leaks what it was asked, so the header count can be read back
 *  out of the title the TypeScript builds. Identity on the key would lose
 *  the interpolated count — this keeps it. */
export const recordingT = (key: string, vars?: Record<string, unknown>): string =>
  JSON.stringify(vars === undefined ? { key } : { key, vars });

const TRAILING_COUNT = /^(.*) \((\d+)\)$/s;

/** The TypeScript answer, reduced to the wire shape. */
export function toWire(entries: Entry[]): WireRow[] {
  return entries.map((e) => {
    const row: WireRow = {
      id: e.task.id,
      depth: e.depth,
      hidden: e.hidden,
      hasChildren: e.hasChildren,
    };
    if (!e.group) return row;
    const group: NonNullable<WireRow['group']> = {
      kind: e.group.kind,
      parentId: e.task.parent_id,
    };
    if (e.group.listId !== undefined) group.listId = e.group.listId;
    if (e.group.sectionId !== undefined) group.sectionId = e.group.sectionId;
    const title = e.task.title;
    switch (e.group.kind) {
      case 'backlog':
      case 'list':
      case 'section': {
        // `${name} (${count})` — the name is the caller's, only the count is
        // the rule's.
        const m = TRAILING_COUNT.exec(title);
        if (!m) throw new Error(`${e.group.kind} header without a count: ${title}`);
        group.count = Number(m[2]);
        break;
      }
      default: {
        // A pure `t` call: the recording `t` handed the vars straight back.
        const asked = JSON.parse(title) as { key: string; vars?: Record<string, number> };
        if (asked.key === 'views.tasks.doneSplit') {
          group.mine = asked.vars!.mine;
          group.others = asked.vars!.others;
          group.count = asked.vars!.mine + asked.vars!.others;
        } else {
          group.count = asked.vars!.count;
        }
      }
    }
    row.group = group;
    return row;
  });
}

/** Run one fixture case through the TypeScript and reduce the answer. */
export function answer(input: WireInput, baseTask: Task): WireRow[] {
  const tasks = input.tasks.map((over) => ({ ...baseTask, ...over }) as Task);
  const lists = new Map(Object.entries(input.lists).map(([id, name]) => [id, { name }]));
  return toWire(
    buildEntries(
      tasks,
      lists,
      recordingT,
      new Set(input.collapsed ?? []),
      input.sections ?? {},
      input.today,
      input.currentUserByList ?? {},
      input.groupBy ?? 'state',
      input.scale ?? 'three',
    ).entries,
  );
}
