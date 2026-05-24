//! Per-feature vtable structs (DESIGN.md §20.3).
//!
//! Each vtable is a `#[repr(C)]` struct of `Option<VtableMethodFn>`
//! pointers — one per method on the corresponding cal-core /
//! sync-core trait. The pointer is wrapped in [`Option`] for the
//! same reason the lifecycle `init` / `destroy` are: a plugin
//! that doesn't implement a method can leave the slot `None` and
//! the host's shim wrapper returns `cal_core::Error::Unsupported`
//! (or the sync equivalent) verbatim — exactly the same UX as the
//! existing default-Unsupported trait methods.
//!
//! Every method takes JSON-encoded arguments (a const-pointer + len
//! pair) and returns a [`super::PluginCallResult`]. See
//! [`super::ffi`] for the full ownership + threading rules.
//!
//! ## What's NOT here in P1
//!
//! The Rust-side shim wrappers (FfiCalendarAdapter etc.) that
//! implement the corresponding cal-core trait by calling into
//! these vtables. Only [`super::shim::FfiCalendarAdapter`] lands
//! in P1 as the canonical pattern example; the Tasks / Contacts /
//! Sync shims are P1b work — pure mechanical pattern-application
//! against the vtable surfaces below.

use crate::ffi::PluginCallResult;

/// Method-pointer type used by every vtable slot. Takes JSON
/// args (pointer + length; may be `(NULL, 0)` for void-arg
/// methods) and returns a [`PluginCallResult`].
///
/// The shim wrappers wrap each call in `tokio::task::spawn_blocking`
/// so a slow plugin can't stall the async runtime. Sync-shape
/// trait methods (e.g. `CalendarFeature::calendar_color`) call
/// these directly — the plugin's implementation is expected to
/// answer from in-memory state without IO.
pub type VtableMethodFn = unsafe extern "C" fn(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult;

pub mod calendar;
pub mod contacts;
pub mod sync;
pub mod tasks;

pub use calendar::CalendarVtable;
pub use contacts::ContactsVtable;
pub use sync::SyncVtable;
pub use tasks::TasksVtable;
