//! The container rows the frontends actually receive.
//!
//! A calendar, task list or address book does not reach a frontend as the bare
//! `cal_core` type. The host stamps on what only it knows — which account owns
//! the container, and what the owning adapter declared it can store — because
//! the frontends gate affordances on exactly that: the sidebar groups by
//! source, the editors grey out what the source would silently drop.
//!
//! # Why these live here and not beside each command
//!
//! They used to be declared twice: once in `src-tauri/src/commands/` for the
//! desktop's Tauri commands, once in `crates/cal-ffi/src/host.rs` for the
//! mobile bridge. Two structs, one wire format, kept in step by hand and by
//! comments saying "mirrors the desktop `TaskListRow`".
//!
//! They had drifted, and it cost something real. The mobile `TaskListRow`
//! carried a `recurrence_capabilities` field the desktop's did not, so the two
//! task editors gated recurrence on two DIFFERENT manifest fields — the desktop
//! on [`TaskCapabilities::recurrence`], mobile on the plugin's top-level
//! [`PluginManifest::recurrence`], which describes CALENDAR EVENT recurrence.
//! For eleven of twelve adapters those agree. For Vikunja they do not: it
//! declares `tasks.recurrence` (no weekday picker, no day-of-month, no count,
//! no until) and no top-level `recurrence` at all, so mobile fell back to full
//! RFC 5545 and offered repeat shapes Vikunja cannot store — the save reported
//! success and the server dropped the value.
//!
//! One declaration cannot disagree with itself. That is the whole reason this
//! module exists; the deduplication is a side effect.
//!
//! # `serde(flatten)`
//!
//! Every row flattens its inner type, so the JSON is `{id, name, …,
//! account_id}` rather than `{inner: {…}, account_id}`. The frontends read one
//! flat object, and a field added to `cal_core::TaskList` rides along without
//! anything here changing.

use cal_core::{Calendar, ContactList, TaskList};
use plugin_core::{RecurrenceCapabilities, TaskCapabilities};
use serde::Serialize;

/// A calendar plus the account that owns it and the recurrence shapes that
/// account's adapter can store.
///
/// The event editor greys out options the source cannot round-trip — EWS has no
/// yearly interval, for instance. The local store and any account whose plugin
/// cannot be resolved report [`RecurrenceCapabilities::default`], i.e. full
/// RFC 5545: a missing manifest must not silently strip options the source may
/// well support.
#[derive(Debug, Serialize)]
pub struct CalendarRow {
    #[serde(flatten)]
    pub inner: Calendar,
    pub account_id: String,
    pub recurrence_capabilities: RecurrenceCapabilities,
}

impl CalendarRow {
    pub fn new(
        inner: Calendar,
        account_id: String,
        recurrence_capabilities: RecurrenceCapabilities,
    ) -> Self {
        Self {
            inner,
            account_id,
            recurrence_capabilities,
        }
    }
}

/// A task list plus the account that owns it and the task-organisation shapes
/// that account's adapter supports.
///
/// The frontends gate affordances on the capabilities — "add section" only
/// where `sections` is true, the recurrence editor only where
/// `task_recurrence` is, and within it only the shapes
/// [`TaskCapabilities::recurrence`] names. The local store and any account
/// whose plugin cannot be resolved report [`TaskCapabilities::default`].
///
/// There is deliberately no `recurrence_capabilities` here. The plugin
/// manifest's top-level `recurrence` describes what the adapter can store on a
/// calendar EVENT; a task's recurrence lives in `tasks.recurrence`, inside the
/// capabilities below. Carrying both on one row is what let the two surfaces
/// read different fields — see the module docs.
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TaskListRow {
    #[serde(flatten)]
    pub inner: TaskList,
    /// Account that owns this list; `"local"` for the built-in store.
    pub account_id: String,
    pub task_capabilities: TaskCapabilities,
}

/// An address book plus the account that owns it.
///
/// Nothing is gated on capabilities here yet; the account id is what lets the
/// UI tell a local (deletable) address book from a provider-managed one.
#[derive(Debug, Serialize)]
pub struct ContactListRow {
    #[serde(flatten)]
    pub inner: ContactList,
    pub account_id: String,
}
