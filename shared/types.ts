// The task domain, shared by the desktop frontend and the mobile app.
//
// These types are NOT written by hand any more. `shared/generated/` is produced
// from the Rust definitions by ts-rs (`cargo xtask ts-types`), so a field that
// changes in `cal-core` / `plugin-core` / `host-core` changes here in the same
// commit or CI fails. This file is only the door: it decides which of the
// generated declarations are part of `@aperio/shared`'s public surface, and it
// carries the one shape that has no single Rust home (see `TaskList` below).
//
// The generated shapes describe what the backend SERIALISES — Tauri commands on
// the desktop, the cal-ffi `LocalStore` on mobile. Both go through serde, so
// both produce the same JSON, and a field with `#[serde(default)]` is therefore
// always PRESENT here even though Rust would accept its absence.
//
// One path does not go through serde at all: a stored user pref that the
// frontend `JSON.parse`s itself. There a value written before a field existed
// really has no key, and the Rust declaration says so with `#[ts(optional)]` —
// `DefaultReminder.attach` is the only one today.

import type { RecurrenceCapabilities } from './generated/RecurrenceCapabilities';
import type { TaskCapabilities } from './generated/TaskCapabilities';
import type { TaskList as CoreTaskList } from './generated/TaskList';

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

/**
 * A task list as the frontends receive it when they LIST them:
 * `cal_core::TaskList` plus the fields the host stamps on while listing.
 *
 * This one is assembled here rather than generated, because it has no single
 * Rust declaration to generate from — the desktop builds it in
 * `src-tauri/src/commands/tasks.rs` (`TaskListRow`) and mobile in
 * `crates/cal-ffi/src/host.rs` (`TaskListRow`), and the two structs are
 * maintained separately. They do not currently agree:
 * `recurrence_capabilities` is stamped on mobile only, which is why it is the
 * one optional field here.
 *
 * Commands that return a task list WITHOUT the enrichment return
 * [`TaskListCore`] instead — see `reparentTaskList`.
 */
export type TaskList = CoreTaskList & {
  /** Account that owns this task list; `"local"` for the local store. */
  account_id: string;
  /** Task-organisation shapes the owning adapter supports, resolved from its
   *  plugin manifest. */
  task_capabilities: TaskCapabilities;
  /** Recurrence shapes the owning adapter can store, from the plugin's
   *  top-level `recurrence` — stamped by mobile only, so it is ALWAYS absent
   *  on the desktop. The desktop task editor reads
   *  `task_capabilities.recurrence` instead, which is a different manifest
   *  field; see TODO A11. */
  recurrence_capabilities?: RecurrenceCapabilities;
};

/**
 * A task list exactly as `cal_core` declares it, with no host enrichment.
 *
 * `reparent_task_list` (both surfaces) answers with this shape — it hands back
 * the row it just wrote rather than re-running the listing that stamps
 * `account_id` and the capabilities on. Callers that need those re-read the
 * listing; the two that exist today discard the answer entirely.
 */
export type TaskListCore = CoreTaskList;
