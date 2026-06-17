//! Desktop `event_log` shim.
//!
//! The Tauri-free event-log machinery (the SQLite-backed `DesktopSyncStore` +
//! `DesktopSyncRoundHooks`, `OnboardingService`, `load_or_mint_device_id`, and
//! the `sync-engine` re-exports) was extracted into `host_core::event_log` so
//! the mobile UniFFI host reuses the exact same sync stack. This shim re-exports
//! all of it — so every existing `crate::event_log::*` reference keeps
//! resolving — and adds the one genuinely desktop-bound piece: the
//! `SyncScheduler` (`tauri::AppHandle` + `Emitter`), which mobile replaces with
//! a JS-driven trigger model.

pub use host_core::event_log::*;

pub mod scheduler;
pub use scheduler::{
    read_interval_minutes, SyncScheduler, SyncStatusPayload, DEFAULT_SYNC_INTERVAL_MINUTES,
    PREF_SYNC_INTERVAL_MINUTES,
};
