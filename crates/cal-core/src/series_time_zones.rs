//! Which time zones a calendar stores for a series: the manifest's
//! `recurrence.time_zones` (decision 97a). It lives in the core because the
//! zone list reads it for its groups (decision 118a); plugin-core re-exports
//! it under its old path.

use serde::{Deserialize, Serialize};

/// Which time zones a calendar stores for a series
/// (`plugin_core::RecurrenceCapabilities::time_zones`).
///
/// In `plugin.json` it is `"all"`, `"none"` or a list of zone names. A list
/// holds canonical tzdata names in exact case, and it is the adapter's own
/// statement: Exchange's is generated from CLDR by `cargo xtask
/// windows-zones`, so the host names no provider (decision 97a). What one
/// server of a provider does not know is not in it; a save that meets it
/// is refused by name ([`crate::WriteRefusal::SeriesZoneNotStorable`],
/// decision 98a).
///
/// UTC is no zone ([`crate::series_clock_zone`] reads every UTC name as
/// none) and is never listed: ask only for a named zone. A series in UTC
/// is stored by every calendar that stores a repeat rule (26a; Exchange
/// writes it without a zone).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SeriesTimeZones {
    /// Every zone (`"all"`).
    #[default]
    Every,
    /// No zone (`"none"`): a series there repeats on the device's clock or
    /// in UTC, and the editors offer no choice.
    NoZone,
    /// Only these canonical tzdata names.
    Only(Vec<String>),
}

impl SeriesTimeZones {
    /// Whether a series in this calendar can repeat on `zone`, a canonical
    /// tzdata name in exact case that is not a UTC name. The caller
    /// canonicalizes: a list names a place tzdata merged with another city
    /// (Oslo with Berlin, 25b) by the city it points to, as the editors store
    /// it. A UTC name is no zone and not asked here (see the type).
    pub fn stores(&self, zone: &str) -> bool {
        match self {
            Self::Every => true,
            Self::NoZone => false,
            Self::Only(zones) => zones.iter().any(|listed| listed == zone),
        }
    }
}

impl Serialize for SeriesTimeZones {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Every => serializer.serialize_str("all"),
            Self::NoZone => serializer.serialize_str("none"),
            Self::Only(zones) => zones.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for SeriesTimeZones {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Word(String),
            List(Vec<String>),
        }
        match Written::deserialize(deserializer)? {
            Written::Word(word) if word == "all" => Ok(Self::Every),
            Written::Word(word) if word == "none" => Ok(Self::NoZone),
            Written::Word(other) => Err(serde::de::Error::custom(format!(
                "time_zones: expected \"all\", \"none\" or a list of zone names, found \"{other}\""
            ))),
            Written::List(zones) => Ok(Self::Only(zones)),
        }
    }
}
