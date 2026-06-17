import { NativeModule, requireNativeModule } from 'expo';

import { ParsedAttendee, TaskListView, TaskView } from './CalFfi.types';

declare class CalFfiModule extends NativeModule<Record<never, never>> {
  /**
   * Parse an attendee entry (`"Name <email>"` or a bare address) by calling
   * the shared Rust `cal-core` parser through UniFFI. Synchronous.
   */
  parseAttendee(entry: string): ParsedAttendee;

  // ── Task lists ──
  /** All task lists, ordered by name (case-insensitive). */
  taskLists(): Promise<TaskListView[]>;
  /** Create a top-level, local task list. */
  createTaskList(name: string): Promise<TaskListView>;
  /** Rename a list. Rejects on an empty/whitespace name or unknown id. */
  renameTaskList(id: string, name: string): Promise<void>;
  /** Delete a list (its tasks cascade away). Rejects on unknown id. */
  deleteTaskList(id: string): Promise<void>;

  // ── Tasks ──
  /** Tasks in a list, ordered by date then creation time. */
  tasks(listId: string): Promise<TaskView[]>;
  /**
   * Create a task in `listId`. `scheduledDate` is `YYYY-MM-DD` or `null`;
   * an unparseable date rejects with an "invalid value" error from the store.
   */
  createTask(
    listId: string,
    title: string,
    description: string | null,
    scheduledDate: string | null,
  ): Promise<TaskView>;
  /** Mark a task completed (`done = true`) or reopen it (`done = false`). */
  setTaskDone(taskId: string, done: boolean): Promise<TaskView>;
  /** Change a task's title. */
  renameTask(taskId: string, title: string): Promise<TaskView>;
  /** Set or clear (`null`) a task's scheduled date (`YYYY-MM-DD`). */
  rescheduleTask(taskId: string, scheduledDate: string | null): Promise<TaskView>;
  /** Delete a task. Rejects on unknown id. */
  deleteTask(taskId: string): Promise<void>;

  // ── JSON bridge (the faithful tasks port) ──
  // The full task / list / section domain crosses as a JSON string in the
  // `cal_core` serde shape — identical to the desktop's Tauri payloads. The
  // mobile api-client parses these into the shared `@aperio/shared` types; the
  // bridge stays a dumb passthrough so the marshalling lives in one place.
  // Each rejects with the store's typed error message on failure.

  /** All task lists as a JSON `TaskList[]`. */
  taskListsJson(): Promise<string>;
  /** Create a top-level local list; returns the created `TaskList` as JSON. */
  createTaskListJson(name: string): Promise<string>;
  /** Set or clear a list's parent (`null` promotes to top level); returns the
   *  updated `TaskList` as JSON. */
  reparentTaskListJson(id: string, parentId: string | null): Promise<string>;
  /** Tasks in a list as a JSON `Task[]`, ordered by date then creation time. */
  tasksJson(listId: string): Promise<string>;
  /** One task by id as JSON. Rejects (not found) when absent. */
  taskJson(id: string): Promise<string>;
  /** Create a task from a JSON `NewTask`; returns the created `Task` as JSON. */
  createTaskJson(listId: string, newTaskJson: string): Promise<string>;
  /** Update a task from a JSON `Task`; returns the updated `Task` as JSON.
   *  Completing a recurring task spawns its next instance (DESIGN §9.12). */
  updateTaskJson(taskJson: string): Promise<string>;
  /** Sections of a list as a JSON `Section[]`. */
  sectionsJson(listId: string): Promise<string>;
  /** Create a section; returns the created `Section` as JSON. */
  createSectionJson(
    listId: string,
    name: string,
    position: number,
    colorLabel: string | null,
  ): Promise<string>;
  /** Update a section from a JSON `Section`; returns it as JSON. */
  updateSectionJson(sectionJson: string): Promise<string>;
  /** Delete a section; its tasks fall back to ungrouped (`section_id` → null). */
  deleteSection(id: string): Promise<void>;
}

export default requireNativeModule<CalFfiModule>('CalFfi');
