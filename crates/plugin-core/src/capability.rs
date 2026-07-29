//! Capability tag — DESIGN.md §10.2 + §20.4.
//!
//! Standalone here rather than re-exported from `cal-core` so that
//! plugin-core stays at the bottom of the dependency stack — every
//! adapter crate depends on plugin-core, and cal-core itself will
//! soon depend on plugin-core (via the sdk macros). Pulling
//! cal-core in here would close the loop.
//!
//! Wire strings:
//!   - `"calendar"`
//!   - `"tasks"`
//!   - `"contacts"`
//!   - `"sync"`
//!   - `"videoconference"`
//!
//! This list is the whole answer to "what does this plugin do".
//! `plugin_type` used to answer part of it and no longer does —
//! see [`super::PluginType::Adapter`].
//!
//! Same forward-compat story as [`super::PluginType`]: unknown
//! capabilities round-trip through [`Capability::Unknown`] so a
//! plugin built for a future Aperio can still be listed.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Calendar events. Pairs with the `CalendarFeature` trait
    /// once cal-core's traits are mapped into the plugin vtable
    /// in the next phase.
    Calendar,
    /// Task lists + tasks (Vikunja, Todoist, Google Tasks, …).
    Tasks,
    /// Address books + contact rows (CardDAV, Google People,
    /// Microsoft Graph, EWS).
    Contacts,
    /// A cross-device sync backend (DESIGN.md §19).
    Sync,
    /// Videoconference meetings — link generation, room listing.
    Videoconference,
    /// Forward-compat: a capability tag from a future Aperio.
    /// Loader keeps the manifest readable but won't try to call
    /// into the unknown surface.
    Unknown(String),
}

impl Capability {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Calendar => "calendar",
            Self::Tasks => "tasks",
            Self::Contacts => "contacts",
            Self::Sync => "sync",
            Self::Videoconference => "videoconference",
            Self::Unknown(s) => s.as_str(),
        }
    }

    pub fn from_wire(s: &str) -> Self {
        match s {
            "calendar" => Self::Calendar,
            "tasks" => Self::Tasks,
            "contacts" => Self::Contacts,
            "sync" => Self::Sync,
            "videoconference" => Self::Videoconference,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }

    /// True for the three families that own containers the user sees in the
    /// sidebar — calendars, task lists, address books.
    ///
    /// The distinction that used to be `plugin_type == "calendar-adapter"`.
    /// A sync backend and a meeting service are surfaces of an account too,
    /// but neither of them puts anything in a list of calendars, so several
    /// host decisions turn on this and not on "is an adapter".
    pub fn is_data_family(&self) -> bool {
        matches!(self, Self::Calendar | Self::Tasks | Self::Contacts)
    }
}

impl Serialize for Capability {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Capability {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_wire(&s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_capabilities_round_trip() {
        for cap in [
            Capability::Calendar,
            Capability::Tasks,
            Capability::Contacts,
            Capability::Sync,
            Capability::Videoconference,
        ] {
            let json = serde_json::to_string(&cap).expect("serialise");
            let back: Capability = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(cap, back, "round trip for {}", cap.as_str());
        }
    }

    #[test]
    fn only_the_container_owning_families_are_data_families() {
        assert!(Capability::Calendar.is_data_family());
        assert!(Capability::Tasks.is_data_family());
        assert!(Capability::Contacts.is_data_family());
        assert!(!Capability::Sync.is_data_family());
        assert!(!Capability::Videoconference.is_data_family());
        assert!(!Capability::Unknown("future".into()).is_data_family());
    }

    #[test]
    fn unknown_capability_preserves_original_string() {
        let v: Capability = serde_json::from_str("\"future-cap\"").expect("deserialise");
        assert_eq!(v, Capability::Unknown("future-cap".to_string()));
        assert_eq!(v.as_str(), "future-cap");
        assert!(!v.is_known());
    }
}
