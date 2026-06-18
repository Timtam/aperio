import { NativeModule, requireNativeModule } from 'expo';

import { ParsedAttendee } from './CalFfi.types';

declare class CalFfiModule extends NativeModule<Record<never, never>> {
  /**
   * Parse an attendee entry (`"Name <email>"` or a bare address) by calling
   * the shared Rust `cal-core` parser through UniFFI. Synchronous.
   */
  parseAttendee(entry: string): ParsedAttendee;

  // ── Tasks / lists / sections (JSON bridge, sync-logged) ──
  // The full task / list / section domain crosses as a JSON string in the
  // `cal_core` serde shape — identical to the desktop's Tauri payloads. Backed
  // by the Rust `Host`, so every local mutation appends to the sync log and
  // round-trips between devices. The mobile api-client parses these into the
  // shared `@aperio/shared` types; the bridge stays a dumb passthrough so the
  // marshalling lives in one place. Each rejects with the store's typed error
  // message on failure.

  /** All task lists as a JSON `TaskList[]`. */
  taskListsJson(): Promise<string>;
  /** Create a top-level local list; returns the created `TaskList` as JSON. */
  createTaskListJson(name: string): Promise<string>;
  /** Set or clear a list's parent (`null` promotes to top level); returns the
   *  updated `TaskList` as JSON. */
  reparentTaskListJson(id: string, parentId: string | null): Promise<string>;
  /** Delete a list (its tasks cascade away). Rejects on unknown id. */
  deleteTaskList(id: string): Promise<void>;
  /** Tasks in a list as a JSON `Task[]`, ordered by date then creation time. */
  tasksJson(listId: string): Promise<string>;
  /** One task by id as JSON. Rejects (not found) when absent. */
  taskJson(id: string): Promise<string>;
  /** Create a task from a JSON `NewTask`; returns the created `Task` as JSON. */
  createTaskJson(listId: string, newTaskJson: string): Promise<string>;
  /** Update a task from a JSON `Task`; returns the updated `Task` as JSON.
   *  Completing a recurring task spawns its next instance (DESIGN §9.12). */
  updateTaskJson(taskJson: string): Promise<string>;
  /** Delete a task. `listId` (the owning list) routes the delete to the right
   *  account — omit/null for a local task. Rejects on unknown id. */
  deleteTask(taskId: string, listId: string | null): Promise<void>;
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
  /** Delete a section; its tasks fall back to ungrouped (`section_id` → null).
   *  `listId` (the owning list) routes the delete — omit/null for a local list. */
  deleteSection(id: string, listId: string | null): Promise<void>;

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

  // ── Sync (full desktop peer: same engine, statically-embedded adapters) ──
  /** Set the active sync target from a JSON `{kind, …}` (local: `{kind:"local",
   *  path}`); persists + probes. Rejects on a bad target / unsupported kind. */
  configureSyncAdapterJson(configJson: string): Promise<void>;
  /** The orchestrator status as JSON (configured / in_flight / last_synced_at /
   *  e2e_enabled / …). */
  syncStatusJson(): Promise<string>;
  /** Run one sync round (push + fetch + apply); returns the SyncRoundReport as
   *  JSON. Rejects "not configured" until a target is set. */
  syncNowJson(): Promise<string>;
  /** Push local pending logs without fetching (RN AppState background); returns
   *  the number of logs pushed. */
  pushNow(): Promise<number>;

  // ── Reminders ──
  /** Upcoming reminder triggers (local + external) within `horizonMinutes` from
   *  now, as a JSON array of `{item_id, item_kind, title, body, trigger_at}`
   *  sorted ascending — for scheduling ahead-of-time OS local notifications. */
  upcomingRemindersJson(horizonMinutes: number): Promise<string>;
}

export default requireNativeModule<CalFfiModule>('CalFfi');
