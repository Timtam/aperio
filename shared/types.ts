// The task domain, shared by the desktop frontend and the mobile app.
//
// These types are NOT written by hand any more. `shared/generated/` is produced
// from the Rust definitions by ts-rs (`cargo xtask ts-types`), so a field that
// changes in `cal-core` / `plugin-core` / `host-core` changes here in the same
// commit or CI fails. This file is only the door: it decides which of the
// generated declarations are part of `@aperio/shared`'s public surface.
//
// The generated shapes describe what the backend SERIALISES — Tauri commands on
// the desktop, the cal-ffi `Host` on mobile. Both go through serde, so
// both produce the same JSON, and a field with `#[serde(default)]` is therefore
// always PRESENT here even though Rust would accept its absence.
//
// One path does not go through serde at all: a stored user pref that the
// frontend `JSON.parse`s itself. There a value written before a field existed
// really has no key, and the Rust declaration says so with `#[ts(optional)]` —
// `DefaultReminder.attach` is the only one today.

// Colors
export type { ContainerColor } from './generated/ContainerColor';
export type { ColorSource } from './generated/ColorSource';
export type { ColorLabel } from './generated/ColorLabel';
export type { ColorLabelId } from './generated/ColorLabelId';

// Adapter capabilities, declared in each plugin's manifest
export type { RecurrenceCapabilities } from './generated/RecurrenceCapabilities';
export type { RecurrenceFreq } from './generated/RecurrenceFreq';
export type { TaskCapabilities } from './generated/TaskCapabilities';
export type { MemberAddMethod } from './generated/MemberAddMethod';
export type { TaskAssignment } from './generated/TaskAssignment';

// Task lists and their people
export type { Section } from './generated/Section';
export type { TaskUser } from './generated/TaskUser';
export type { MemberRight } from './generated/MemberRight';
export type { TaskListShare } from './generated/TaskListShare';

// Reminders and sounds
export type { Reminder } from './generated/Reminder';
export type { ReminderKind } from './generated/ReminderKind';
export type { SoundConfig } from './generated/SoundConfig';
export type { SoundSource } from './generated/SoundSource';
export type { DefaultReminder } from './generated/DefaultReminder';

// Tasks
export type { Task } from './generated/Task';
export type { TaskStatus } from './generated/TaskStatus';
export type { TaskPriority } from './generated/TaskPriority';
export type { TaskEffort } from './generated/TaskEffort';

// Task recurrence (DESIGN §9.12)
export type { TaskRecurrence } from './generated/TaskRecurrence';
export type { RecurrenceFrequency } from './generated/RecurrenceFrequency';
export type { RecurrenceEnd } from './generated/RecurrenceEnd';
export type { RecurrenceAnchor } from './generated/RecurrenceAnchor';
export type { RecurrencePlacement } from './generated/RecurrencePlacement';
export type { MonthDay } from './generated/MonthDay';
export type { Weekday } from './generated/Weekday';

// Grouping
export type { SuggestionDecline } from './generated/SuggestionDecline';
// The task view's grouping, `cal_core::task_grouping` — what the door is
// asked with and what it answers. The shell in `shared/taskGrouping.ts`
// speaks these; nothing else builds them by hand.
export type { GroupableTask } from './generated/GroupableTask';
export type { GroupingInput } from './generated/GroupingInput';
export type { GroupingRow } from './generated/GroupingRow';
export type { GroupHead } from './generated/GroupHead';
export type { GroupKind } from './generated/GroupKind';
export type { TaskGroupBy } from './generated/TaskGroupBy';

/**
 * A task list as the frontends receive it when they LIST them: the
 * `cal_core::TaskList` fields plus what the host stamps on while listing.
 *
 * Generated, like everything else here, from `host_core::wire::TaskListRow` —
 * ONE Rust declaration used by the desktop's Tauri command and the mobile
 * bridge alike, so the two cannot answer with different fields. They used to,
 * and it cost something: see that module's docs.
 *
 * Commands that answer with a task list WITHOUT the enrichment return
 * [`TaskListCore`] — see `reparentTaskList`.
 */
export type { TaskListRow as TaskList } from './generated/TaskListRow';

/**
 * A task list exactly as `cal_core` declares it, with no host enrichment.
 *
 * `reparent_task_list` (both surfaces) answers with this shape — it hands back
 * the row it just wrote rather than re-running the listing that stamps
 * `account_id` and the capabilities on. Callers that need those re-read the
 * listing; the two that exist today discard the answer entirely.
 */
export type { TaskList as TaskListCore } from './generated/TaskList';
