//! Birthday calendars (DESIGN.md §10.3).
//!
//! The whole implementation — the pure synthesis (id helpers,
//! `synthesise_calendar`, `events_for_contacts`, age description) AND the
//! orchestration (walking local + external contact books, reading the snapshot
//! cache) — now lives in the shared, Tauri-free [`host_core::birthdays`] so the
//! desktop and the mobile cal-ffi Host produce identical birthday layers for
//! local AND external contacts. This module just re-exports the entry points the
//! command layer references through `super::birthdays::*`.

pub use host_core::birthdays::{
    is_birthday_calendar_id, list_birthday_calendars, synthesise_birthday_events,
};
