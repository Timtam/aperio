// TypeScript counterparts to the cal-core types.
//
// Phase 1 maintains these by hand. The crate `cal-core` is the source of
// truth; if a field changes there, mirror it here. Phase 2 will look at
// generating these from the Rust definitions (e.g. via `specta`), at
// which point this file becomes a starting point.

export interface ContainerColor {
  hex: string;
  source: 'native' | 'custom';
}

export interface Calendar {
  id: string;
  name: string;
  color: ContainerColor | null;
  read_only: boolean;
  default_sound: SoundConfig | null;
  /** Account that owns this calendar. `"local"` for the implicit
   *  local adapter; a UUID for any external account. Backend
   *  enriches every Calendar with this field via the registry's
   *  route map so the frontend can group containers by source
   *  without a second round-trip. */
  account_id: string;
}

export interface SoundConfig {
  source:
    | { type: 'system' }
    | { type: 'silent' }
    | { type: 'custom'; sha256: string };
  volume: number;
}

export interface Reminder {
  kind:
    | { type: 'relative'; minutes_before: number }
    | { type: 'absolute'; at: string }
    | { type: 'app_start' }
    | { type: 'email'; minutes_before: number };
  sound: SoundConfig | null;
}

export interface EventRecurrence {
  rrule: string;
  exceptions: string[];
}

export interface CalendarEvent {
  id: string;
  calendar_id: string;
  title: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: EventRecurrence | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
  created_at: string;
  updated_at: string;
  etag: string | null;
}

export interface NewEvent {
  title: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: EventRecurrence | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
}

export interface TaskList {
  id: string;
  name: string;
  color: ContainerColor | null;
  default_sound: SoundConfig | null;
  embedded_in_calendar: string | null;
  read_only: boolean;
  /** Account that owns this task list. Same semantics as
   *  `Calendar.account_id` — populated by the backend's route map. */
  account_id: string;
}

export type TaskStatus = 'open' | 'in_progress' | 'completed' | 'cancelled';
export type TaskPriority = 'low' | 'medium' | 'high';

export interface Task {
  id: string;
  list_id: string;
  title: string;
  description: string | null;
  status: TaskStatus;
  priority: TaskPriority;
  /**
   * The day the task is to be done. Was historically also where the
   * old `deadline_type='on'` cases lived after migration 0006 — the
   * "Geplanter Tag" and "Konkrete Deadline" of the old enum are now
   * one and the same field.
   */
  scheduled_date: string | null;
  /**
   * Optional time-of-day on `scheduled_date`. Renders as a point
   * marker in the day grid (no block duration). Requires
   * `scheduled_date`; the DB enforces this via a CHECK constraint.
   */
  scheduled_time: string | null;
  /**
   * The day BY which the task must be done. The only deadline
   * semantic that survives migration 0006 — what used to be
   * `deadline_type='by'`. Until that day the task lives in the
   * backlog and can be scheduled per-day via `scheduled_date`; on
   * the deadline day, if still open, an app-start checker pins it
   * to today.
   */
  deadline_date: string | null;
  deadline_time: string | null;
  recurrence: unknown;
  parent_id: string | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
  etag: string | null;
}

export interface ColorLabel {
  id: string;
  name: string;
  hex: string;
}

export interface SearchResults {
  events: CalendarEvent[];
  tasks: Task[];
}

/** Adapter kinds known to the backend. Phase 6a only allows `local`
 *  to be created; the others appear in the UI as "coming soon" until
 *  their respective adapter lands. */
export type AdapterKind =
  | 'local'
  | 'caldav'
  | 'ical'
  | 'google'
  | 'microsoft_graph'
  | 'ews'
  | 'vikunja'
  | 'todoist';

export interface Account {
  id: string;
  adapter_kind: AdapterKind;
  display_name: string;
  /** Adapter-specific non-secret config as a JSON string. */
  config_json: string;
  created_at: string;
  updated_at: string;
}

export interface CommandError {
  code:
    | 'auth'
    | 'forbidden'
    | 'not_found'
    | 'conflict'
    | 'network'
    | 'protocol'
    | 'invalid_input'
    | 'unsupported'
    | 'internal';
  message: string;
}
