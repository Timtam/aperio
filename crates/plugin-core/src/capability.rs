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
            Self::Unknown(s) => s.as_str(),
        }
    }

    pub fn from_wire(s: &str) -> Self {
        match s {
            "calendar" => Self::Calendar,
            "tasks" => Self::Tasks,
            "contacts" => Self::Contacts,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
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
        for cap in [Capability::Calendar, Capability::Tasks, Capability::Contacts] {
            let json = serde_json::to_string(&cap).expect("serialise");
            let back: Capability = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(cap, back, "round trip for {}", cap.as_str());
        }
    }

    #[test]
    fn unknown_capability_preserves_original_string() {
        let v: Capability = serde_json::from_str("\"future-cap\"").expect("deserialise");
        assert_eq!(v, Capability::Unknown("future-cap".to_string()));
        assert_eq!(v.as_str(), "future-cap");
        assert!(!v.is_known());
    }
}
