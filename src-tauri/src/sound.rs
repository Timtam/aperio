//! Notification-sound resolution (DESIGN.md §14.4).
//!
//! Moved to `host_core::sound` (Tauri-free) so the desktop reminder
//! scheduler and the mobile cal-ffi reminder surface resolve the
//! effective sound through one shared code path. Re-exported here so
//! existing `crate::sound::{SoundPrefs, ContainerKind}` references keep
//! resolving. See [`host_core::sound`] for the resolution hierarchy +
//! the why-we-snapshot rationale.

pub use host_core::sound::{ContainerKind, SoundPrefs};
