import { NativeModule, requireNativeModule } from 'expo';

import { ParsedAttendee, TaskListView, TaskView } from './CalFfi.types';

declare class CalFfiModule extends NativeModule<{}> {
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
}

export default requireNativeModule<CalFfiModule>('CalFfi');
