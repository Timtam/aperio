//! cal-core — Shared types and traits for all Aperio adapters.
//!
//! This crate has no dependency on any concrete adapter. All adapters
//! depend on `cal-core`, never the other way around.

pub mod adapter;
pub mod attendee;
pub mod color;
pub mod conferencing;
pub mod error;
pub mod event_group;
pub mod extras;
pub mod recurrence;
pub mod reminder;
pub mod spawn;
pub mod suggestion_decline;
pub mod types;

pub use adapter::{
    Adapter, AdapterSource, AuthToken, CalendarFeature, Capability, ChangeSet, ContactsFeature,
    Container, Credentials, Reminderable, TasksFeature,
};
pub use color::{ColorLabel, ColorLabelId, ColorSource, ContainerColor};
pub use error::{Error, Result};
pub use event_group::{EventGroup, EventGroupMember};
pub use extras::{
    apply_task_extras, decode_payload, encode_payload, extras_for_task, recurrence_needs_extras,
    AperioExtras,
};
pub use recurrence::{rrule_to_task_recurrence, rrule_until_instant, task_recurrence_to_rrule};
pub use reminder::{Reminder, ReminderKind, SoundConfig, SoundSource};
pub use spawn::{advance, completion_record_for, next_recurrence_instance};
pub use suggestion_decline::SuggestionDecline;
pub use types::{
    AttendeeResponse, AttendeeStatus, Calendar, Contact, ContactAddress, ContactList, ContactPhoto,
    ContactValue, DateRange, Event, EventRecurrence, FreeBusy, FreeBusySlot, GroupMember,
    MemberRight, MonthDay, NewContact, NewEvent, NewTask, RecurrenceAnchor, RecurrenceEnd,
    RecurrenceFrequency, RecurrencePlacement, Section, Task, TaskEffort, TaskList, TaskListShare,
    TaskPriority, TaskRecurrence, TaskStatus, TaskUser, Weekday,
};
