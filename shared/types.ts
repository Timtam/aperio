// TypeScript counterparts to the cal-core types — the platform-agnostic task
// domain shared by the desktop frontend and the mobile app.
//
// The crate `cal-core` is the source of truth; if a field changes there, mirror
// it here. These match the JSON the backend (Tauri on desktop, the cal-ffi
// LocalStore on mobile) serialises, so both frontends parse the same shape.

export interface ContainerColor {
  hex: string;
  source: 'native' | 'custom';
}

/** Which recurrence shapes the owning adapter can store. Mirrors
 *  `plugin_core::RecurrenceCapabilities`; stamped onto every Calendar/TaskList
 *  by the backend. The editors grey out options the source can't round-trip —
 *  e.g. EWS omits `"yearly"`. Local + unknown sources report full RFC-5545
 *  support. */
export interface RecurrenceCapabilities {
  frequencies: RecurrenceFreq[];
  interval_frequencies: RecurrenceFreq[];
  relative_monthly: boolean;
  relative_yearly: boolean;
  weekly_byday: boolean;
  /** An explicit monthly day-of-month can be stored. Vikunja repeats monthly
   *  on the task's due-date day implicitly, so it sets this false and the task
   *  editor disables the "day of month" field. Calendar events always store
   *  BYMONTHDAY → default true. */
  monthly_day_of_month: boolean;
  count: boolean;
  until: boolean;
}

export type RecurrenceFreq = 'daily' | 'weekly' | 'monthly' | 'yearly';

/** Which task-organisation features the owning adapter supports. Mirrors
 *  `plugin_core::TaskCapabilities`; stamped onto every `TaskList` by the
 *  backend. The task UI gates affordances on these. Absent → cal-core-native
 *  default (flat lists, single-level subtasks, cross-list move). */
export interface TaskCapabilities {
  nested_projects: boolean;
  subtasks: boolean;
  /** `null` ⇒ unlimited nesting depth. */
  max_subtask_depth: number | null;
  sections: boolean;
  /** The adapter can create / rename / delete sections at the source (Todoist
   *  sections, Vikunja kanban buckets). Coloring a section is independent —
   *  always a local override, offered wherever `sections` is true. */
  manageable_sections: boolean;
  multiple_labels: boolean;
  task_recurrence: boolean;
  /** Which recurrence shapes the adapter can store for tasks. Absent ⇒ full
   *  support. */
  recurrence?: RecurrenceCapabilities;
  /** The source stores the "in progress" status as a distinct state. Backends
   *  with only open/done set this false. Absent → true. */
  supports_in_progress: boolean;
  move_between_projects: boolean;
  /** ONE occurrence of a repeating task can be moved to an arbitrary day and
   *  stay there. Absent → true.
   *
   *  False where the source treats the due date as the SERIES anchor rather
   *  than a property of the occurrence — iOS Reminders does, so an arbitrary
   *  date written to a repeating reminder does not survive the round trip.
   *  Callers that want to move a single occurrence advance the series by one
   *  step instead, which is what the source can actually do. */
  reschedule_single_occurrence?: boolean;
  /** The adapter can create new task lists (projects) at the source. */
  create_lists: boolean;
  /** The adapter can delete task lists at the source. */
  delete_lists: boolean;
  /** The adapter can manage a list's membership/sharing. */
  manageable: boolean;
  /** How members are added when `manageable`: directory search (Vikunja) or
   *  raw-email invite (Todoist). */
  member_add_by: MemberAddMethod;
}

/** How members are added to a task list (DESIGN §9.7): pick from a user
 *  directory (Vikunja) or invite by raw email (Todoist). */
export type MemberAddMethod = 'search' | 'email';

/** A sub-grouping of tasks within one list — a Vikunja bucket or a Todoist
 *  section. Mirrors `cal_core::Section`. */
export interface Section {
  id: string;
  list_id: string;
  name: string;
  /** Optional color-label binding. Cascades to the section's tasks that carry
   *  no color of their own (resolution chain task → section → list). `null` ⇒
   *  no color. */
  color_label: string | null;
  /** Display order within the list; lower sorts first. */
  order: number;
}

/** A user in the task domain — a task assignee, a member of a task list's
 *  collaborator pool, or the connected account's own identity ("me"). `id` is
 *  the provider-native user id. See DESIGN §9.7. */
export interface TaskUser {
  id: string;
  name: string;
  email: string | null;
}

/** Permission level on a task-list share (Vikunja). `null` on backends without
 *  per-share roles (Todoist). */
export type MemberRight = 'read' | 'write' | 'admin';

/** One editable membership/share of a task list (DESIGN §9.7). Distinct from
 *  the read-only assignee pool. */
export interface TaskListShare {
  user: TaskUser;
  right: MemberRight | null;
  /** Invitation not yet accepted (Todoist email invites). */
  pending: boolean;
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

export interface TaskList {
  id: string;
  name: string;
  color: ContainerColor | null;
  /** Bound color-label id — see `Calendar.color_label`. */
  color_label: string | null;
  default_sound: SoundConfig | null;
  embedded_in_calendar: string | null;
  read_only: boolean;
  /** Account that owns this task list. */
  account_id: string;
  /** Parent project id for backends with nested projects (Vikunja, Todoist).
   *  `null` for top-level lists and flat backends. Refers to another
   *  `TaskList.id` owned by the same account. */
  parent_id: string | null;
  /** Task-organisation capabilities of the owning adapter. Optional in the wire
   *  shape so a pre-capabilities snapshot still parses. Absent → cal-core
   *  default. */
  task_capabilities?: TaskCapabilities;
  /** Recurrence capabilities of the owning adapter (frequencies, interval,
   *  weekday/day-of-month, end modes). Optional so a pre-capabilities snapshot
   *  still parses. Absent → full RFC-5545. */
  recurrence_capabilities?: RecurrenceCapabilities;
}

export type TaskStatus = 'open' | 'in_progress' | 'completed' | 'cancelled';
export type TaskPriority = 'low' | 'medium' | 'high';
/** Aperio-only effort estimate, modelled like `priority`. Drives a purely
 *  visual, toggleable tile size. Host-only: it rides the AperioExtras bag on
 *  external providers (no provider has a native effort field). */
export type TaskEffort = 'small' | 'medium' | 'large';

export interface Task {
  id: string;
  list_id: string;
  title: string;
  description: string | null;
  status: TaskStatus;
  priority: TaskPriority;
  effort: TaskEffort;
  /** The day the task is to be done. */
  scheduled_date: string | null;
  /** Optional time-of-day on `scheduled_date`. Requires `scheduled_date`; the
   *  DB enforces this via a CHECK constraint. */
  scheduled_time: string | null;
  /** The day BY which the task must be done (the surviving deadline semantic).
   *  Until that day the task lives in the backlog and can be scheduled per-day
   *  via `scheduled_date`. */
  deadline_date: string | null;
  deadline_time: string | null;
  /** Aperio-only per-task override for the day-start deadline countdown:
   *  remind this many days before `deadline_date`, overriding the global
   *  `tasks.deadlineCountdownDays` for this task. `null` ⇒ use the global
   *  default. Host-only: it rides the AperioExtras bag on external providers
   *  (no provider has a native field for it). */
  deadline_reminder_days: number | null;
  recurrence: unknown;
  /** DESIGN §9.12: a backlog task surfaces in the active backlog only on/after
   *  this date (the recurrence "resurface" trigger). `null` ⇒ visible now;
   *  until then it sits in the "Zukünftig" group. */
  resurface_date: string | null;
  /** DESIGN §9.12: stable id of the recurring series this instance belongs to.
   *  `null` ⇒ not managed. */
  series_id: string | null;
  parent_id: string | null;
  /** Section this task is filed under within its list. `null` ⇒ ungrouped, or
   *  a backend with no sections. Refers to a `Section.id` whose
   *  `list_id == this.list_id`. */
  section_id: string | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  /** Users this task is assigned to (DESIGN §9.7). Empty ⇒ unassigned. */
  assignees: TaskUser[];
  created_at: string;
  updated_at: string;
  completed_at: string | null;
  etag: string | null;
}

export interface ColorLabel {
  id: string;
  name: string;
  hex: string;
  /** `true` for a hidden "ad-hoc" one-off color composed via the custom color
   *  picker (name = hex, deduped by hex). Excluded from the palette UI, but
   *  resolves like any label. Optional so older payloads default to named. */
  ad_hoc?: boolean;
}

/** A pair the user has said is NOT one appointment (migration 0037).
 *
 *  Stored in a canonical order — the smaller (calendar, event) first as text —
 *  so "A and B" and "B and A" are one decision. Mirrors
 *  `cal_core::SuggestionDecline`. */
export interface SuggestionDecline {
  calendar_a: string;
  event_a: string;
  calendar_b: string;
  event_b: string;
  declined_at: string;
  /** When the pair was last grouped BY HAND, if ever. Refused iff
   *  `declined_at` is the later of the two — see migration 0038. The host
   *  filters on it already; this is here so a row read from a snapshot keeps
   *  its shape. */
  cleared_at?: string | null;
}
