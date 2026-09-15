//! The clock a series repeats on, read from the zone name it stores.
//!
//! A recurring event stores the IANA zone of its start, or none. With a zone it
//! repeats on that zone's wall clock, so 09:00 stays 09:00 across a clock
//! change; without one it repeats on UTC. Which stored names count as a zone
//! used to be answered three ways: the views took every name but `UTC`, the
//! reminders trimmed the name and knew it only in its exact case, and a new
//! series was stamped with whatever the device reported. A series Exchange
//! stored as `Etc/UTC` took the zoned path in the views.
//!
//! One rule now, for the views, the reminders and the device's own zone
//! (DESIGN-series-time-zone.md, "UTC und Anbieter"; decided 2026-09-14):
//!
//! - A name counts only if tzdata knows it, ignoring ASCII case and nothing
//!   else. `europe/berlin` is Berlin; ` Europe/Berlin `, a Windows name, an
//!   offset such as `+05:30` and a zone newer than [`TZDATA_VERSION`] are not
//!   zones, and such a series repeats on UTC everywhere.
//! - The eighteen names tzdata gives UTC — `Etc/UTC`, `Etc/GMT` and every link
//!   to either (`UTC`, `GMT`, `Zulu`, `Etc/Universal`, …) — are no zone either.
//!   `Etc/GMT+8` is a zone.
//!
//! The names are generated into `series_clock/zone_names.rs` by
//! `cargo xtask tz-list` from the tzdata chrono-tz ships, rather than read
//! through a chrono-tz dependency here: this crate needs names and links, not
//! offsets, and twelve adapter repositories compile it. host-core and the
//! adapters use that same chrono-tz, so every zone [`canonical_zone`] returns
//! is one chrono-tz can parse, from the same tzdata release. chrono-tz knows
//! names in their exact case only: a Rust caller that parses a series' zone
//! resolves it through [`canonical_zone`] first, as host-core's reminders do.
//! The CalDAV and Graph adapters still parse the stored spelling
//! (DESIGN-series-time-zone.md, "Risiken").
//!
//! Pinned row by row in `tests/fixtures/seriesClock.json`; the `contract`
//! module below reads it, and so do the phone's door tests in cal-ffi and the
//! TypeScript contract test through the WebAssembly door.

#[rustfmt::skip]
mod zone_names;

use std::cmp::Ordering;

use serde::Serialize;

/// What kind of tzdata name an entry of the table is: a zone, or a link and
/// the reason tzdata keeps it — the section of tzdata's `backward` file that
/// declares it, or `etcetera`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum NameKind {
    /// A zone of its own.
    Zone,
    /// A pre-1993 naming convention: `US/Eastern`, `EST5EDT`.
    Pre1993,
    /// A two-part name renamed to three parts in 1995: `America/Buenos_Aires`.
    Renamed1995,
    /// A place tzdata merged into another whose clocks have agreed since 1970,
    /// and which had a zone.tab line of its own: `Europe/Oslo`.
    MergedZoneTab,
    /// A merged place without a zone.tab line: `America/Montreal`.
    MergedNonZoneTab,
    /// Another name for the same place: `Europe/Kiev`, `Asia/Calcutta`.
    Alternate,
    /// A UTC alias from `etcetera`: `GMT`.
    Etcetera,
}

/// The IANA tzdata release the zone names were generated from.
pub const TZDATA_VERSION: &str = zone_names::TZDATA_VERSION;

/// The zone a tzdata name resolves to, spelled as tzdata spells it, or `None`
/// when tzdata does not know the name.
///
/// ASCII letters match in either case. Nothing is trimmed, and no other
/// character folds: a dotless `ı` is not an `i`. A link resolves to its
/// target — `Asia/Calcutta` is `Asia/Kolkata`, `Europe/Oslo` is
/// `Europe/Berlin`, `UTC` is `Etc/UTC` — and `Etc/GMT+8` is a zone of its own.
pub fn canonical_zone(name: &str) -> Option<&'static str> {
    zone_names::NAMES
        .binary_search_by(|(entry, _, _)| ascii_folded(entry, name))
        .ok()
        .map(|at| zone_names::NAMES[at].1)
}

/// Every tzdata name with the zone it resolves to and its kind, sorted by the
/// name with ASCII case folded.
pub(crate) fn names() -> &'static [(&'static str, &'static str, NameKind)] {
    zone_names::NAMES
}

/// A listed zone's position in [`listed_zones`], for its id in any ASCII case.
pub(crate) fn listed_position(zone: &str) -> Option<usize> {
    zone_names::LISTED
        .binary_search_by(|entry| ascii_folded(entry, zone))
        .ok()
}

/// The zone a series repeats on, as the series stores it, or `None` when it
/// repeats on UTC: no zone, one of the names tzdata gives UTC, or a name
/// tzdata does not know.
///
/// The stored spelling comes back unchanged, so a surface keeps handing Intl
/// the name it has always handed it; only the question "is this a zone" is
/// answered here. A caller that parses the zone with chrono-tz, which knows
/// names in their exact case only, resolves it through [`canonical_zone`]
/// first.
pub fn series_clock_zone(tzid: Option<&str>) -> Option<&str> {
    let tzid = tzid?;
    match canonical_zone(tzid)? {
        "Etc/UTC" | "Etc/GMT" => None,
        _ => Some(tzid),
    }
}

/// The zone a series hands a provider when it is written: none for an all-day
/// series, otherwise its stored zone as stored, for the adapter's own rule
/// (such as [`series_clock_zone`]) to read.
///
/// An all-day series has no zone of its own (DESIGN-series-time-zone.md, 13b),
/// and a zone does harm on the wire: Exchange moves an all-day series to that
/// zone's midnights and stretches it over more days (live test round 2,
/// decision 46a). Every adapter that writes a series' zone asks this —
/// Exchange, Microsoft 365, Google and CalDAV — so the rule lives here once.
pub fn written_series_zone(tzid: Option<&str>, all_day: bool) -> Option<&str> {
    if all_day {
        None
    } else {
        tzid
    }
}

/// The zones outside `Etc/` with a region part, in tzdata's spelling and in
/// the order the names sort with ASCII case folded — the zones a series' zone
/// is chosen from. The region part keeps out the POSIX-style names such as
/// `EST5EDT`, which tzdata 2026d turns from links back into zones.
pub fn listed_zones() -> &'static [&'static str] {
    zone_names::LISTED
}

/// `a` against `b` with ASCII letters lowercased, byte by byte.
fn ascii_folded(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// The generated table holds what the lookups above assume of it.
#[cfg(test)]
mod table {
    use super::*;

    #[test]
    fn the_names_are_sorted_folded_and_unique_when_folded() {
        for pair in zone_names::NAMES.windows(2) {
            assert_eq!(
                ascii_folded(pair[0].0, pair[1].0),
                Ordering::Less,
                "{:?} does not sort before {:?}",
                pair[0].0,
                pair[1].0,
            );
        }
    }

    #[test]
    fn every_name_resolves_to_a_zone_that_resolves_to_itself() {
        for (name, target, _) in zone_names::NAMES {
            assert_eq!(canonical_zone(name), Some(*target), "{name}");
            assert_eq!(canonical_zone(target), Some(*target), "{name} -> {target}");
        }
    }

    #[test]
    fn the_listed_zones_are_the_zones_outside_etc() {
        for pair in zone_names::LISTED.windows(2) {
            assert_eq!(ascii_folded(pair[0], pair[1]), Ordering::Less);
        }
        let zones: Vec<&str> = zone_names::NAMES
            .iter()
            .filter(|(name, target, _)| {
                name == target && name.contains('/') && !name.starts_with("Etc/")
            })
            .map(|(name, _, _)| *name)
            .collect();
        assert_eq!(listed_zones(), zones.as_slice());
        // Named rather than counted: a tzdata release that adds a zone must not
        // turn this red, one that loses these must.
        for zone in [
            "Europe/Berlin",
            "Asia/Kolkata",
            "Europe/Kyiv",
            "America/Argentina/Buenos_Aires",
            "Pacific/Kanton",
        ] {
            assert!(listed_zones().contains(&zone), "{zone} is not listed");
        }
    }

    #[test]
    fn exactly_the_eighteen_utc_names_repeat_on_utc() {
        let mut utc: Vec<&str> = zone_names::NAMES
            .iter()
            .filter(|(_, target, _)| matches!(*target, "Etc/UTC" | "Etc/GMT"))
            .map(|(name, _, _)| *name)
            .collect();
        utc.sort_unstable();
        assert_eq!(
            utc,
            [
                "Etc/GMT",
                "Etc/GMT+0",
                "Etc/GMT-0",
                "Etc/GMT0",
                "Etc/Greenwich",
                "Etc/UCT",
                "Etc/UTC",
                "Etc/Universal",
                "Etc/Zulu",
                "GMT",
                "GMT+0",
                "GMT-0",
                "GMT0",
                "Greenwich",
                "UCT",
                "UTC",
                "Universal",
                "Zulu",
            ],
        );
    }

    #[test]
    fn the_posix_style_names_are_never_listed() {
        // tzdata 2026d turns these from links back into zones; the list keeps
        // the zones with a region part only.
        for name in ["CST6CDT", "EST5EDT", "MST7MDT", "PST8PDT"] {
            assert!(!listed_zones().contains(&name), "{name} is listed");
            assert!(canonical_zone(name).is_some(), "{name} no longer resolves");
        }
    }

    #[test]
    fn every_link_has_a_reason_and_every_zone_is_itself() {
        for (name, target, kind) in zone_names::NAMES {
            assert_eq!(
                name == target,
                *kind == NameKind::Zone,
                "{name} -> {target} ({kind:?})"
            );
        }
        // Named, one link per kind: a reader that lost a section fails here.
        for (name, kind) in [
            ("US/Eastern", NameKind::Pre1993),
            ("America/Buenos_Aires", NameKind::Renamed1995),
            ("Europe/Oslo", NameKind::MergedZoneTab),
            ("America/Montreal", NameKind::MergedNonZoneTab),
            ("Europe/Kiev", NameKind::Alternate),
            ("GMT", NameKind::Etcetera),
        ] {
            let found = zone_names::NAMES
                .iter()
                .find(|(entry, _, _)| *entry == name)
                .map(|(_, _, kind)| *kind);
            assert_eq!(found, Some(kind), "{name}");
        }
    }

    /// Decision 46a: an all-day series writes no zone, whatever it stores; any
    /// other series hands its stored zone on unchanged, in its own spelling.
    #[test]
    fn an_all_day_series_writes_no_zone() {
        for tzid in [
            Some("Europe/Berlin"),
            Some("asia/calcutta"),
            Some("UTC"),
            None,
        ] {
            assert_eq!(written_series_zone(tzid, true), None, "all-day {tzid:?}");
            assert_eq!(written_series_zone(tzid, false), tzid, "timed {tzid:?}");
        }
    }

    #[test]
    fn the_tzdata_release_is_recorded() {
        assert!(!TZDATA_VERSION.is_empty());
    }
}

/// The rule against `tests/fixtures/seriesClock.json`.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seriesClock.json"
    ));

    fn section(name: &str) -> Vec<Value> {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        doc[name]
            .as_array()
            .unwrap_or_else(|| panic!("{name} is an array"))
            .clone()
    }

    #[test]
    fn every_series_clock_row_holds() {
        let rows = section("seriesClockZone");
        // Anti-silence: the rows the rule turns on, by input.
        for tzid in [
            "etc/utc",
            "GMT+0",
            " UTC",
            "Etc/Unıversal",
            "Asia/Calcutta",
            "Europe/Oslo",
            "Etc/GMT+8",
            "+05:30",
            "europe/berlin",
        ] {
            assert!(
                rows.iter().any(|r| r["tzid"].as_str() == Some(tzid)),
                "the contract lost the {tzid:?} row",
            );
        }
        for row in &rows {
            let tzid = row["tzid"].as_str();
            assert_eq!(
                series_clock_zone(tzid),
                row["zone"].as_str(),
                "{tzid:?}: {}",
                row["note"].as_str().unwrap_or(""),
            );
        }
    }

    #[test]
    fn every_canonical_zone_row_holds() {
        let rows = section("canonicalZone");
        for name in [
            "asia/calcutta",
            "Europe/Oslo",
            "Etc/GMT+8",
            "UTC",
            "Etc/Unıversal",
            "",
        ] {
            assert!(
                rows.iter().any(|r| r["name"].as_str() == Some(name)),
                "the contract lost the {name:?} row",
            );
        }
        for row in &rows {
            let name = row["name"].as_str().expect("every row names a zone");
            assert_eq!(
                canonical_zone(name),
                row["canonical"].as_str(),
                "{name:?}: {}",
                row["note"].as_str().unwrap_or(""),
            );
        }
    }
}
