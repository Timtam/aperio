//! Reminders and sound configuration (`DESIGN.md` section 14).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single reminder attached to an event or task.
///
/// An item may carry multiple reminders. Each reminder can override the
/// item-level sound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub kind: ReminderKind,
    /// When set, this reminder's sound overrides the item-level default
    /// (see `resolve_sound` in section 14.4).
    pub sound: Option<SoundConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReminderKind {
    /// Relative to the event start or task deadline.
    /// `minutes_before` may be negative to fire after the reference time.
    Relative { minutes_before: i64 },
    /// Fixed point in time, independent of the event.
    Absolute { at: DateTime<Utc> },
    /// Fires on the next app start after the due time.
    AppStart,
    /// E-mail reminder (delivered by the adapter where supported).
    Email { minutes_before: i64 },
}

/// Sound configuration for notifications.
///
/// Used both at the container level (calendar / task list) and the item
/// level (event / task); `resolve_sound` (section 14.4) implements the
/// inheritance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundConfig {
    pub source: SoundSource,
    /// Volume 0–100, independent of the system volume.
    pub volume: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SoundSource {
    /// Platform default notification sound.
    System,
    /// Silent notification (visual only).
    Silent,
    /// User-supplied audio file, referenced by its content hash
    /// (section 19.2.2). The file itself lives in the sync asset store.
    Custom { sha256: String },
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            source: SoundSource::System,
            volume: 80,
        }
    }
}
