//! Plugin-type tag — DESIGN.md §20.2.
//!
//! Four entries: `calendar-adapter`, `sync-adapter`,
//! `videoconference-adapter`, `notification`. The wire string is
//! kebab-case to match the rest of the manifest; the Rust enum is
//! `CalendarAdapter` etc. Mirror of the `AperioPluginType` enum in
//! `aperio_plugin.h` (in numeric form there for C consumers).
//!
//! Forward-compat: an unknown tag deserialises to
//! [`PluginType::Unknown`] with the original string preserved, so a
//! future Aperio that introduces a new tag can ship plugins that
//! older builds list but mark as "not supported on this version".

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PluginType {
    /// Calendar / tasks / contacts data source. The detailed
    /// surface is governed by the `capabilities` field in the
    /// manifest; a single calendar-adapter can declare any subset
    /// of `["calendar", "tasks", "contacts"]`.
    CalendarAdapter,
    /// Cross-device sync backend (DESIGN.md §19).
    SyncAdapter,
    /// Videoconference link generation + room listing (Zoom,
    /// Teams, Meet, WebEx). VC adapters land in a phase after the
    /// plugin one — declaring the tag now keeps the manifest stable.
    VideoconferenceAdapter,
    /// Notification channel (system, e-mail, webhook). Not yet
    /// wired into the app; declared for forward-compat with
    /// DESIGN.md §20.2.
    Notification,
    /// Forward-compat: an unknown tag from a future Aperio. The
    /// loader will skip these with a "not supported on this build"
    /// note rather than rejecting the whole plugin file.
    Unknown(String),
}

impl PluginType {
    /// Canonical kebab-case wire string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::CalendarAdapter => "calendar-adapter",
            Self::SyncAdapter => "sync-adapter",
            Self::VideoconferenceAdapter => "videoconference-adapter",
            Self::Notification => "notification",
            Self::Unknown(s) => s.as_str(),
        }
    }

    /// Inverse of [`Self::as_str`]. Unknown strings round-trip
    /// through [`Self::Unknown`] rather than erroring.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "calendar-adapter" => Self::CalendarAdapter,
            "sync-adapter" => Self::SyncAdapter,
            "videoconference-adapter" => Self::VideoconferenceAdapter,
            "notification" => Self::Notification,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// `true` iff this is a tag the current host knows how to
    /// dispatch to. Used by the manager to decide whether to call
    /// the plugin's vtable at all vs. just listing it in the
    /// "installed plugins" panel as inert.
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

impl Serialize for PluginType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PluginType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_wire(&s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tags_round_trip() {
        for tag in [
            PluginType::CalendarAdapter,
            PluginType::SyncAdapter,
            PluginType::VideoconferenceAdapter,
            PluginType::Notification,
        ] {
            let json = serde_json::to_string(&tag).expect("serialise");
            let back: PluginType = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(tag, back, "round trip for {}", tag.as_str());
        }
    }

    #[test]
    fn unknown_tag_preserves_original_string() {
        let v: PluginType = serde_json::from_str("\"future-thing\"").expect("deserialise");
        assert_eq!(v, PluginType::Unknown("future-thing".to_string()));
        assert_eq!(v.as_str(), "future-thing");
        assert!(!v.is_known());
    }

    #[test]
    fn known_tags_are_marked_known() {
        assert!(PluginType::CalendarAdapter.is_known());
        assert!(PluginType::SyncAdapter.is_known());
        assert!(PluginType::Notification.is_known());
    }
}
