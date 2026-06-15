//! cal-core — Shared types and traits for all Aperio adapters.
//!
//! This crate has no dependency on any concrete adapter. All adapters
//! depend on `cal-core`, never the other way around.

pub mod adapter;
pub mod attendee;
pub mod color;
pub mod error;
pub mod recurrence;
pub mod reminder;
pub mod types;

pub use adapter::{
    Adapter, AdapterSource, AuthToken, CalendarFeature, Capability, ChangeSet, ContactsFeature,
    Container, Credentials, Reminderable, TasksFeature,
};
pub use color::{ColorLabel, ColorLabelId, ColorSource, ContainerColor};
pub use error::{Error, Result};
pub use recurrence::{rrule_to_task_recurrence, task_recurrence_to_rrule};
pub use reminder::{Reminder, ReminderKind, SoundConfig, SoundSource};
pub use types::{
    AttendeeResponse, AttendeeStatus, Calendar, Contact, ContactAddress, ContactList, ContactPhoto,
    DateRange, Event, EventRecurrence, FreeBusy, FreeBusySlot, GroupMember, MemberRight,
    NewContact, NewEvent, NewTask, RecurrenceEnd, RecurrenceFrequency, Section, Task, TaskList,
    TaskListShare, TaskPriority, TaskRecurrence, TaskStatus, TaskUser, Weekday,
};
