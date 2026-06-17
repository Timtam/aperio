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

  // ── Accounts (the full engine: external adapters + secrets) ──
  // Backed by the Rust `Host` (statically-embedded adapter plugins + the
  // keychain-bridged SecretStore). Same JSON-passthrough convention as the
  // task bridge; each rejects with the store's typed error message on failure.

  /** All persisted accounts as a JSON `Account[]` (the desktop wire shape). */
  accountsJson(): Promise<string>;
  /**
   * Create an account from a JSON request (`adapter_kind`, `display_name`,
   * `config_json`, optional `secret`); persists the row, stores the secret via
   * the platform keychain, and registers the adapter. Returns the created
   * `Account` as JSON. Rejects for OAuth kinds (a later phase) + on bad config.
   */
  createAccountJson(requestJson: string): Promise<string>;
  /** Delete an account: unregister its adapter, clear its secrets, drop the
   *  row. Rejects when deleting the implicit local account. */
  deleteAccount(accountId: string): Promise<void>;

  // ── Calendars + events ──
  // JSON passthrough in the cal_core/desktop wire shape; routing (local vs
  // external account) happens Rust-side in the Host. Rejects with the typed
  // store error (not_found / conflict / auth / …) on failure.

  /** All calendars (local + external) as a JSON `CalendarRow[]`. Also primes
   *  the Host's calendar→account route map, so call it before event ops. */
  listCalendarsJson(): Promise<string>;
  /** Create a local calendar from `{name, color_label?}`; returns the created
   *  `CalendarRow` as JSON. */
  createCalendarJson(requestJson: string): Promise<string>;
  /** Delete a local calendar (its events cascade). */
  deleteCalendar(id: string): Promise<void>;
  /** Events overlapping a range, from `{calendar_id, start, end}` (RFC-3339);
   *  returns a JSON `Event[]`. */
  getEventsJson(requestJson: string): Promise<string>;
  /** One local event by id as JSON (`Event` or `null`). */
  getEventByIdJson(id: string): Promise<string>;
  /** Create an event from `{calendar_id, …NewEvent}`; returns the created
   *  `Event` as JSON. */
  createEventJson(requestJson: string): Promise<string>;
  /** Update an event in place from a JSON `Event` (its `calendar_id` selects
   *  the route); returns the updated `Event` as JSON. */
  updateEventJson(eventJson: string): Promise<string>;
  /** Delete an event. `calendarId` is routing-only (null → local);
   *  `sendCancellations` (external only) defaults to false. */
  deleteEvent(
    id: string,
    calendarId: string | null,
    sendCancellations: boolean | null,
  ): Promise<void>;
}

export default requireNativeModule<CalFfiModule>('CalFfi');
