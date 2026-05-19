//! cal-core — Shared types and traits for all Aperio adapters.
//!
//! This crate has no dependency on any concrete adapter. All adapters
//! depend on `cal-core`, never the other way around.

pub mod adapter;
pub mod color;
pub mod error;
pub mod reminder;
pub mod types;

pub use adapter::{
    Adapter, AdapterSource, AuthToken, CalendarFeature, Capability, ContactsFeature, Container,
    Credentials, Reminderable, TasksFeature,
};
pub use color::{ColorLabel, ColorLabelId, ColorSource, ContainerColor};
pub use error::{Error, Result};
pub use reminder::{Reminder, ReminderKind, SoundConfig, SoundSource};
pub use types::{
    Calendar, Contact, DateRange, DeadlineType, Event, EventRecurrence, FreeBusy, FreeBusySlot,
    NewEvent, NewTask, RecurrenceEnd, RecurrenceFrequency, Task, TaskList, TaskPriority,
    TaskRecurrence, TaskStatus, Weekday,
};
