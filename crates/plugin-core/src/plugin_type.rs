//! Plugin-type tag — DESIGN.md §20.2.
//!
//! Two entries: `adapter` and `notification`. Mirror of the
//! `AperioPluginType` enum in `aperio_plugin.h` (in numeric form
//! there for C consumers).
//!
//! There used to be four, one per surface — `calendar-adapter`,
//! `sync-adapter`, `videoconference-adapter`. They are gone
//! because the tag was answering a question that belongs to
//! `capabilities`, and answering it worse: it allowed exactly one
//! surface per plugin, so a provider that is a calendar AND a
//! place to sync into had to ship as two libraries with two
//! sign-ins to the same account.
//!
//! Forward-compat: an unknown tag deserialises to
//! [`PluginType::Unknown`] with the original string preserved, so a
//! future Aperio that introduces a new tag can ship plugins that
//! older builds list but mark as "not supported on this version".

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PluginType {
    /// One provider, however many surfaces — `"adapter"`.
    ///
    /// What a plugin actually does is its `capabilities` list: any subset of
    /// `calendar`, `tasks`, `contacts`, `sync`, `videoconference`. The vtable
    /// mirrors it — an [`crate::vtables::AdapterVtable`] with one pointer per
    /// family, null for the ones it does not serve.
    ///
    /// One tag rather than one per surface, because one PROVIDER is not one
    /// feature. Google is a calendar, an address book, a task list, a file
    /// store to sync into and a meeting service; four plugins for that means
    /// four OAuth registrations and four sign-ins to the same account, with
    /// four refresh tokens in the keychain that are all the same credential.
    Adapter,
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
            Self::Adapter => "adapter",
            Self::Notification => "notification",
            Self::Unknown(s) => s.as_str(),
        }
    }

    /// Inverse of [`Self::as_str`]. Unknown strings round-trip
    /// through [`Self::Unknown`] rather than erroring.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "adapter" => Self::Adapter,
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
        for tag in [PluginType::Adapter, PluginType::Notification] {
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
        assert!(PluginType::Adapter.is_known());
        assert!(PluginType::Notification.is_known());
    }

    /// The retired per-surface tags are not silently accepted as `adapter`.
    ///
    /// They land in `Unknown`, so a plugin still carrying one is listed as
    /// unsupported instead of being loaded with an empty capability list and
    /// then wondered about.
    #[test]
    fn the_retired_per_surface_tags_are_unknown() {
        for old in [
            "calendar-adapter",
            "sync-adapter",
            "videoconference-adapter",
        ] {
            assert_eq!(
                PluginType::from_wire(old),
                PluginType::Unknown(old.to_string())
            );
        }
    }
}
