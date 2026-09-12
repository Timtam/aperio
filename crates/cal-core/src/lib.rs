//! cal-core — Shared types and traits for all Aperio adapters.
//!
//! This crate has no dependency on any concrete adapter. All adapters
//! depend on `cal-core`, never the other way around.

pub mod adapter;
pub mod attendee;
// Behind the `collation` feature: it carries about a megabyte of baked ICU
// data, and the twelve adapter repositories that compile this crate never
// sort a list a person reads. Only the two frontend bindings switch it on.
#[cfg(feature = "collation")]
pub mod collation;
pub mod color;
pub mod conferencing;
pub mod day_marker;
pub mod error;
pub mod event_anchor;
pub mod event_group;
pub mod event_group_fold;
pub mod event_local_reminders;
pub mod extras;
pub mod group_carry;
pub mod group_suggestion;
pub mod meeting_events;
pub mod meeting_link_grouping;
pub mod recurrence;
pub mod reminder;
pub mod spawn;
pub mod suggestion_decline;
pub mod task_assignment;
// The grouping itself is behind the same feature (it orders titles, section
// names and list names, and an adapter never builds a task view); the wire
// types are not, so `cargo xtask ts-types` can generate them.
pub mod task_cascade;
pub mod task_day;
pub mod task_grouping;
// Anti-silence: `cargo test -p cal-core` without the feature compiles the
// grouping out and would report green having run none of its pinned cases.
// Say so instead. (The workspace run has the feature through cal-core-wasm
// and cal-ffi; CI runs that.)
#[cfg(all(test, not(feature = "collation")))]
mod task_grouping_gate {
    #[test]
    fn the_grouping_contract_needs_the_collation_feature() {
        panic!(concat!(
            "cal_core::task_grouping, cal_core::task_day and their fixture contracts ",
            "are behind the `collation` feature and were not compiled: run ",
            "`cargo test -p cal-core --features collation` (or the workspace run, ",
            "which enables it through cal-core-wasm and cal-ffi)"
        ));
    }
}
pub mod task_priority;
pub mod task_status;
pub mod types;

pub use adapter::{
    Adapter, AdapterSource, AuthToken, CalendarFeature, Capability, ChangeSet, ContactsFeature,
    Container, Credentials, Reminderable, TasksFeature,
};
#[cfg(feature = "collation")]
pub use collation::{compare_names, compare_titles, CollationLanguage};
pub use color::{ColorLabel, ColorLabelId, ColorSource, ContainerColor};
pub use day_marker::{DayLog, DayMarker};
pub use error::{Error, Result};
pub use event_anchor::{plan_repairs, series_master_id, Anchored, Repair};
pub use event_group::{normalized_title, EventGroup, EventGroupMember};
pub use event_group_fold::{
    collapse_event_groups, collapse_event_groups_json, CollapsedRow, FoldableEvent,
};
pub use event_local_reminders::EventLocalReminders;
pub use extras::{
    apply_task_extras, decode_payload, encode_payload, extras_for_task, recurrence_needs_extras,
    AperioExtras,
};
pub use group_carry::{
    carry_onto, carry_onto_json, future_carry_fields, future_carry_fields_json,
    occurrence_carry_fields, occurrence_carry_fields_json, plan_carry, plan_carry_json, CarryField,
    CarryPlan, CarryTarget, CarryableFields,
};
pub use group_suggestion::{
    find_group_suggestions, find_group_suggestions_json, is_meeting_calendar, suggest_group_mate,
    suggest_group_mate_json, GroupSuggestion, SuggestibleEvent, MEETINGS_CALENDAR_SUFFIX,
};
pub use meeting_events::{
    join_url_of, meeting_join_url, without_duplicate_meetings, without_duplicate_meetings_json,
    MeetingFilterEvent,
};
pub use meeting_link_grouping::{
    find_meeting_link_pairs, find_meeting_link_pairs_json, normalize_join_url, LinkableEvent,
    MeetingLinkPair,
};
pub use recurrence::{rrule_to_task_recurrence, rrule_until_instant, task_recurrence_to_rrule};
pub use reminder::{Reminder, ReminderKind, SoundConfig, SoundSource};
pub use spawn::{advance, completion_record_for, next_recurrence_instance};
pub use suggestion_decline::SuggestionDecline;
pub use task_assignment::is_mine_or_unassigned;
pub use task_cascade::{
    auto_date_on_start, auto_date_on_start_json, derive_status_from_children,
    plan_ancestor_recompute, plan_ancestor_recompute_json, plan_status_cascade,
    plan_status_cascade_json, AutoDateInput, CascadeInput, CascadeOptions, CascadeTask,
    RecomputeInput, StatusWrite,
};
pub use task_day::{
    backlog_weeks, backlog_weeks_json, end_time_on_day, is_deadline_chip, split_deadlines_by_week,
    split_deadlines_by_week_json, time_on_day, BacklogWeeks, BacklogWeeksInput, DayInput, DayTask,
    DayTaskRow, DeadlineSplit, DeadlineSplitInput,
};
#[cfg(feature = "collation")]
pub use task_day::{tasks_on_days, tasks_on_days_json};
#[cfg(feature = "collation")]
pub use task_grouping::{group_tasks, group_tasks_json};
pub use task_grouping::{
    is_task_deferred, GroupHead, GroupKind, GroupableTask, GroupingInput, GroupingRow, TaskGroupBy,
};
pub use task_priority::{normal_priority, priority_rank, PriorityScale};
pub use task_status::{
    effort_i18n_key, priority_i18n_key, status_i18n_key, subtask_progress, subtask_progress_json,
    task_i18n_keys, task_i18n_keys_json, EffortKeys, PriorityKeys, PriorityKeysByScale,
    ProgressTask, StatusKeys, SubtaskProgress, SubtaskProgressInput, TaskI18nKeys,
};
pub use types::{
    AttendeeResponse, AttendeeStatus, Calendar, Contact, ContactAddress, ContactList, ContactPhoto,
    ContactValue, DateRange, Event, EventRecurrence, FreeBusy, FreeBusySlot, GroupMember,
    MemberRight, MonthDay, NewContact, NewEvent, NewTask, RecurrenceAnchor, RecurrenceEnd,
    RecurrenceFrequency, RecurrencePlacement, Section, Task, TaskEffort, TaskList, TaskListShare,
    TaskPriority, TaskRecurrence, TaskStatus, TaskUser, Weekday,
};
