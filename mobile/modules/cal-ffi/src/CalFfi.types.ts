/** Result of `CalFfi.parseAttendee`, produced by the Rust cal-core parser. */
export type ParsedAttendee = {
  /** Display name, or `null` for a bare email entry. */
  name: string | null;
  /** The email address. */
  email: string;
};

/** A task list as the on-device store returns it. */
export type TaskListView = {
  id: string;
  name: string;
  /** Parent project id for nested backends; `null` for a top-level list. */
  parentId: string | null;
  readOnly: boolean;
};

/** Lifecycle state of a task (mirrors the Rust `TaskStatus`). */
export type TaskStatus = 'open' | 'in_progress' | 'completed' | 'cancelled';

/**
 * The reduced task shape the UI consumes. The full lossless task (reminders,
 * sound, recurrence rule, timestamps, …) lives in the Rust store; these are
 * the fields the minimal tasks UI needs to render and act on.
 */
export type TaskView = {
  id: string;
  listId: string;
  title: string;
  description: string | null;
  /** `true` when the task is completed. */
  done: boolean;
  status: TaskStatus;
  /** `YYYY-MM-DD`, or `null`. The day the task is planned for. */
  scheduledDate: string | null;
  /** `YYYY-MM-DD`, or `null`. The day the task is due by. */
  deadlineDate: string | null;
  /** Whether the task carries a recurrence rule. */
  hasRecurrence: boolean;
  /** RFC 3339, set once completed; `null` otherwise. */
  completedAt: string | null;
};
