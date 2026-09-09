// The cal-core-native capability defaults, in one place.
//
// A task list whose adapter says nothing about a capability gets the value
// `TaskCapabilities::default()` / `RecurrenceCapabilities::default()` would
// have given it in Rust (`crates/plugin-core/src/manifest.rs`). Both frontends
// need them: the backend stamps every LISTED task list with the adapter's real
// capabilities, but a list that arrives from anywhere else — a command that
// answers with the bare `cal_core` row, a pre-capabilities snapshot — carries
// none, and the editors have to decide what to offer anyway.
//
// The SHAPES here are generated (see `shared/generated/`); only the default
// VALUES are restated, because ts-rs generates types, not `impl Default`. A
// field added in Rust therefore fails to compile here until its default is
// written down too.

import type { RecurrenceCapabilities, TaskCapabilities } from './types';

/** Mirrors `plugin_core::RecurrenceCapabilities::default()` — full RFC 5545. */
export const DEFAULT_RECURRENCE_CAPABILITIES: RecurrenceCapabilities = {
  frequencies: ['daily', 'weekly', 'monthly', 'yearly'],
  interval_frequencies: ['daily', 'weekly', 'monthly', 'yearly'],
  relative_monthly: true,
  relative_yearly: true,
  weekly_byday: true,
  monthly_day_of_month: true,
  count: true,
  until: true,
};

/** Mirrors `plugin_core::TaskCapabilities::default()` — flat lists,
 *  single-level subtasks, cross-list move. */
export const DEFAULT_TASK_CAPABILITIES: TaskCapabilities = {
  nested_projects: false,
  subtasks: true,
  max_subtask_depth: null,
  sections: false,
  manageable_sections: false,
  multiple_labels: false,
  task_recurrence: true,
  supports_in_progress: true,
  move_between_projects: true,
  reschedule_single_occurrence: true,
  task_time_of_day: true,
  task_span: false,
  create_lists: false,
  delete_lists: false,
  manageable: false,
  member_add_by: 'search',
  task_assignment: 'none',
  recurrence: DEFAULT_RECURRENCE_CAPABILITIES,
};
