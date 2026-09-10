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
pub mod event_local_reminders;
pub mod extras;
pub mod group_suggestion;
pub mod recurrence;
pub mod reminder;
pub mod spawn;
pub mod suggestion_decline;
pub mod task_assignment;
pub mod task_priority;
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
pub use event_local_reminders::EventLocalReminders;
pub use extras::{
    apply_task_extras, decode_payload, encode_payload, extras_for_task, recurrence_needs_extras,
    AperioExtras,
};
pub use group_suggestion::{
    find_group_suggestions, find_group_suggestions_json, is_meeting_calendar, suggest_group_mate,
    suggest_group_mate_json, GroupSuggestion, SuggestibleEvent, MEETINGS_CALENDAR_SUFFIX,
};
pub use recurrence::{rrule_to_task_recurrence, rrule_until_instant, task_recurrence_to_rrule};
pub use reminder::{Reminder, ReminderKind, SoundConfig, SoundSource};
pub use spawn::{advance, completion_record_for, next_recurrence_instance};
pub use suggestion_decline::SuggestionDecline;
pub use task_assignment::is_mine_or_unassigned;
pub use task_priority::{normal_priority, priority_rank, PriorityScale};
pub use types::{
    AttendeeResponse, AttendeeStatus, Calendar, Contact, ContactAddress, ContactList, ContactPhoto,
    ContactValue, DateRange, Event, EventRecurrence, FreeBusy, FreeBusySlot, GroupMember,
    MemberRight, MonthDay, NewContact, NewEvent, NewTask, RecurrenceAnchor, RecurrenceEnd,
    RecurrenceFrequency, RecurrencePlacement, Section, Task, TaskEffort, TaskList, TaskListShare,
    TaskPriority, TaskRecurrence, TaskStatus, TaskUser, Weekday,
};
