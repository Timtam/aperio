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
        .binary_search_by(|(entry, _)| ascii_folded(entry, name))
        .ok()
        .map(|at| zone_names::NAMES[at].1)
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

/// The zones outside `Etc/`, in tzdata's spelling and in the order the names
/// sort with ASCII case folded — the zones a series' zone is chosen from.
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
        for (name, target) in zone_names::NAMES {
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
            .filter(|(name, target)| name == target && !name.starts_with("Etc/"))
            .map(|(name, _)| *name)
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
            .filter(|(_, target)| matches!(*target, "Etc/UTC" | "Etc/GMT"))
            .map(|(name, _)| *name)
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
