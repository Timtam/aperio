// The mobile api-client — the engine-reuse boundary for the tasks port.
//
// Mirrors the desktop's `src/api/client.ts` function names + return types so
// ported logic reads the same, but every body is a `JSON.parse` over a
// `CalFfi.*Json` bridge call. The JSON wire is the `cal_core` serde shape
// (snake_case) — identical to the desktop's Tauri payloads — so the SAME
// `@aperio/shared` domain types parse on both sides. The native module stays a
// dumb string passthrough; all marshalling lives here.
//
// Where a bridge method takes fewer args than the desktop command (the local
// store has no account / colour / calendar-embedding backing yet), the request
// type still mirrors the desktop's so call sites match 1:1, and the unused
// fields are dropped before the bridge call — documented per function.

import CalFfi from '../../modules/cal-ffi';
import type { Section, Task, TaskList } from '@aperio/shared';

import { scheduleBackgroundPush } from './syncTriggers';

export interface CreateTaskListRequest {
  name: string;
  /** Account to create the list in. Local store ignores it (always local). */
  account_id?: string | null;
  /** Parent list id for nesting; the local store creates top-level lists. */
  parent_id?: string | null;
  embedded_in_calendar: string | null;
  /** Local-only colour-label binding (not yet wired through the bridge). */
  color_label?: string | null;
}

export interface CreateTaskRequest {
  list_id: string;
  title: string;
  description: string | null;
  status: Task['status'];
  priority: Task['priority'];
  scheduled_date: string | null;
  scheduled_time: string | null;
  deadline_date: string | null;
  deadline_time: string | null;
  recurrence: unknown;
  parent_id: string | null;
  section_id: string | null;
  color_label: string | null;
  reminders: Task['reminders'];
  assignees: Task['assignees'];
  sound: Task['sound'];
}

export interface CreateSectionRequest {
  list_id: string;
  name: string;
  position: number;
  color_label?: string | null;
}

// ── Task lists ───────────────────────────────────────────────────────────────

export const listTaskLists = async (): Promise<TaskList[]> =>
  JSON.parse(await CalFfi.taskListsJson()) as TaskList[];

/** Create a top-level local list. The bridge takes only `name`; the other
 *  request fields have no local backing yet, so they're forwarded as the name
 *  alone (the full request type is kept for desktop call-site parity). */
export const createTaskList = async (
  request: CreateTaskListRequest,
): Promise<TaskList> => {
  const created = JSON.parse(
    await CalFfi.createTaskListJson(request.name),
  ) as TaskList;
  scheduleBackgroundPush();
  return created;
};

export const reparentTaskList = async (
  id: string,
  parentId: string | null,
): Promise<TaskList> => {
  const updated = JSON.parse(
    await CalFfi.reparentTaskListJson(id, parentId),
  ) as TaskList;
  scheduleBackgroundPush();
  return updated;
};

export const deleteTaskList = async (id: string): Promise<void> => {
  await CalFfi.deleteTaskList(id);
  scheduleBackgroundPush();
};

// ── Tasks ────────────────────────────────────────────────────────────────────

export const getTasks = async (list_id: string): Promise<Task[]> =>
  JSON.parse(await CalFfi.tasksJson(list_id)) as Task[];

/** The bridge REJECTS (not found) for an unknown id; translate to `null` to
 *  mirror the desktop's `Task | null`. */
export const getTaskById = async (id: string): Promise<Task | null> => {
  try {
    return JSON.parse(await CalFfi.taskJson(id)) as Task;
  } catch {
    return null;
  }
};

/** Create a task. Builds a `cal_core::NewTask` in the serde shape the desktop
 *  also produces: `list_id` is passed positionally and `assignees` is dropped
 *  (the local store doesn't persist them); `resurface_date`/`series_id` are
 *  store-managed (sent null). A recurring task gets a stable series id, and
 *  completing one later spawns its next instance — visible on the next fetch. */
export const createTask = async (request: CreateTaskRequest): Promise<Task> => {
  const newTask = {
    title: request.title,
    description: request.description,
    status: request.status,
    priority: request.priority,
    scheduled_date: request.scheduled_date,
    scheduled_time: request.scheduled_time,
    deadline_date: request.deadline_date,
    deadline_time: request.deadline_time,
    recurrence: request.recurrence ?? null,
    resurface_date: null,
    series_id: null,
    parent_id: request.parent_id,
    section_id: request.section_id,
    color_label: request.color_label,
    reminders: request.reminders,
    sound: request.sound,
    assignees: [],
  };
  const created = JSON.parse(
    await CalFfi.createTaskJson(request.list_id, JSON.stringify(newTask)),
  ) as Task;
  scheduleBackgroundPush();
  return created;
};

/** Full-overwrite update. Send the task exactly as read so the store-managed
 *  `series_id` / `resurface_date` round-trip. Completing a recurring task
 *  spawns its next instance (visible on the next fetch) — callers must refetch
 *  (bump dataVersion), never optimistically splice. */
export const updateTask = async (task: Task): Promise<Task> => {
  const updated = JSON.parse(
    await CalFfi.updateTaskJson(JSON.stringify(task)),
  ) as Task;
  scheduleBackgroundPush();
  return updated;
};

export const deleteTask = async (id: string): Promise<void> => {
  await CalFfi.deleteTask(id);
  scheduleBackgroundPush();
};

// ── Sections ─────────────────────────────────────────────────────────────────

export const getSections = async (list_id: string): Promise<Section[]> =>
  JSON.parse(await CalFfi.sectionsJson(list_id)) as Section[];

export const createSection = async (
  request: CreateSectionRequest,
): Promise<Section> => {
  const created = JSON.parse(
    await CalFfi.createSectionJson(
      request.list_id,
      request.name,
      request.position,
      request.color_label ?? null,
    ),
  ) as Section;
  scheduleBackgroundPush();
  return created;
};

export const updateSection = async (section: Section): Promise<Section> => {
  const updated = JSON.parse(
    await CalFfi.updateSectionJson(JSON.stringify(section)),
  ) as Section;
  scheduleBackgroundPush();
  return updated;
};

export const deleteSection = async (id: string): Promise<void> => {
  await CalFfi.deleteSection(id);
  scheduleBackgroundPush();
};
