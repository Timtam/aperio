//! Windows ↔ tzdata time-zone names for Exchange.
//!
//! Exchange (EWS) names a series' zone by its WINDOWS id ("W. Europe Standard
//! Time"); the rest of Aperio speaks tzdata ("Europe/Berlin"). The translation
//! is the Unicode CLDR `windowsZones` mapping, generated into
//! `windows_tz/windows_zones.rs` by `cargo xtask windows-zones` from the file
//! pinned in `cldr/` (release tag, sha256 and licence beside it). Nothing here
//! is kept by hand: a hand-kept copy had drifted to three wrong rows and wrote
//! 174 of the 312 listed zones without a zone.
//!
//! ## Reading an id
//!
//! An id reads as the zone of its default ("001") row, in tzdata's canonical
//! spelling: `India Standard Time` is `Asia/Kolkata`, not CLDR's
//! `Asia/Calcutta`. Exchange keeps one id for a group of cities on one clock,
//! so a Vienna series reads back as Berlin. The id `UTC`, and an id the table
//! does not know — a custom definition, an id only a server's registry has —
//! mean no zone: the series repeats in UTC.
//!
//! ## Writing a zone
//!
//! A series' stored zone goes through the core's one rule for zones first
//! (`cal_core::series_clock_zone`, then `cal_core::canonical_zone`): no zone, a
//! UTC name or a name tzdata does not know writes no zone (decisions 14a, 26a);
//! any spelling of a zone writes that zone's id, `Asia/Calcutta` as
//! `India Standard Time`, and a merged place its target's (25b).
//!
//! A zone Exchange cannot store is written without one (22a): CLDR has no id
//! for it, or CLDR's id runs another clock than the zone in the five years
//! after the pinned release. See DESIGN-series-time-zone.md, stage 4.

#[rustfmt::skip]
mod windows_zones;

pub use windows_zones::{CLDR_RELEASE, TABLE_ID, TZDATA_VERSION};

use std::cmp::Ordering;

use windows_zones::{OTHER_CLOCK, UNMAPPED, WINDOWS_ZONES, ZONE_WINDOWS};

/// What a Windows zone id read from Exchange means for a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsZoneRead {
    /// A zone, in tzdata's canonical spelling.
    Zone(&'static str),
    /// The id `UTC`: no zone, so the series repeats in UTC.
    Utc,
    /// An id the table does not know: empty, a custom definition, an id only a
    /// server's registry has (`Kamchatka Standard Time`), or an invented one.
    /// No zone either.
    Unknown,
}

/// What Exchange gets for a series' stored zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsZoneWrite {
    /// StartTimeZone and EndTimeZone with this id.
    Id(&'static str),
    /// No zone: none is stored, a UTC name, or a name tzdata does not know. The
    /// series repeats in UTC.
    NoZone,
    /// A zone Exchange cannot store, written without one.
    NotStorable {
        zone: &'static str,
        reason: NotStorable,
    },
}

/// Why Exchange cannot store a zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotStorable {
    /// CLDR has no Windows id for it.
    NoWindowsZone,
    /// CLDR's id for it runs another clock in the years after the pinned
    /// release, so Exchange would store another zone than the one chosen.
    OtherClock { windows: &'static str },
}

/// A Windows zone id as Exchange reports it, in any ASCII case.
pub fn read_windows_zone(id: &str) -> WindowsZoneRead {
    match lookup(WINDOWS_ZONES, id) {
        Some(zone) if cal_core::series_clock_zone(Some(zone)).is_none() => WindowsZoneRead::Utc,
        Some(zone) => WindowsZoneRead::Zone(zone),
        None => WindowsZoneRead::Unknown,
    }
}

/// The Windows id Exchange gets for a series' stored zone.
pub fn windows_zone_for(tzid: Option<&str>) -> WindowsZoneWrite {
    let Some(zone) = cal_core::series_clock_zone(tzid).and_then(cal_core::canonical_zone) else {
        return WindowsZoneWrite::NoZone;
    };
    if let Some(windows) = lookup(ZONE_WINDOWS, zone) {
        return WindowsZoneWrite::Id(windows);
    }
    let reason = if let Some(windows) = lookup(OTHER_CLOCK, zone) {
        NotStorable::OtherClock { windows }
    } else {
        if UNMAPPED.binary_search_by(|z| folded(z, zone)).is_err() {
            // Every zone of the core's tzdata is in one of the three lists (a
            // test proves it), so this is a table from another release.
            tracing::warn!(
                target: "adapter_ews::zones",
                zone,
                table = TZDATA_VERSION,
                core = cal_core::TZDATA_VERSION,
                "the Exchange zone table has no row for this zone",
            );
        }
        NotStorable::NoWindowsZone
    };
    WindowsZoneWrite::NotStorable { zone, reason }
}

/// ASCII case folded, the order the generated tables are sorted in.
fn folded(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .binary_search_by(|(k, _)| folded(k, key))
        .ok()
        .map(|at| table[at].1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/windowsZones.json"
    ));

    /// The rows the rules turn on, by name: a row that goes missing from the
    /// fixture fails here by its name, and a row not named here fails too.
    const MUST_RUN: &[&str] = &[
        "berlin-default",
        "vienna-member",
        "lowercase-berlin",
        "device-spelling-calcutta",
        "merged-oslo",
        "merged-amsterdam-takes-brussels",
        "merged-copenhagen-takes-berlin",
        "kyiv-only-through-kiev",
        "kanton-through-enderbury",
        "chihuahua-cldr43",
        "almaty-cldr45",
        "beirut-middle-east",
        "coyhaique-cldr48",
        "etc-gmt-minus-1",
        "troll-no-windows-zone",
        "scoresbysund-other-clock",
        "utc-name-utc",
        "utc-name-gmt",
        "utc-name-etc-zulu",
        "unknown-name",
        "offset-string",
        "padded-berlin",
        "no-tzid",
        "w-europe-berlin",
        "lowercase-id",
        "india-kolkata",
        "fle-kyiv",
        "greenwich-abidjan",
        "utc-is-no-zone",
        "utc-minus-02-etc",
        "mountain-mexico-mazatlan",
        "central-asia-bishkek",
        "middle-east-beirut",
        "kamchatka-registry-only",
        "lebanon-not-an-id",
        "custom-definition",
        "empty-id",
    ];

    fn written(write: WindowsZoneWrite) -> Value {
        match write {
            WindowsZoneWrite::Id(id) => json!({ "id": id }),
            WindowsZoneWrite::NoZone => json!({ "no_zone": true }),
            WindowsZoneWrite::NotStorable {
                reason: NotStorable::NoWindowsZone,
                ..
            } => json!({ "not_storable": "no_windows_zone" }),
            WindowsZoneWrite::NotStorable {
                reason: NotStorable::OtherClock { windows },
                ..
            } => json!({ "not_storable": "other_clock", "windows": windows }),
        }
    }

    fn read(read: WindowsZoneRead) -> Value {
        match read {
            WindowsZoneRead::Zone(zone) => json!({ "zone": zone }),
            WindowsZoneRead::Utc => json!({ "utc": true }),
            WindowsZoneRead::Unknown => json!({ "unknown": true }),
        }
    }

    #[test]
    fn every_contract_row_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let mut seen = Vec::new();
        for row in doc["write"].as_array().expect("write rows") {
            let name = row["name"].as_str().expect("a row name");
            seen.push(name);
            assert_eq!(
                written(windows_zone_for(row["tzid"].as_str())),
                row["expect"],
                "{name}: {}",
                row["note"]
            );
        }
        for row in doc["read"].as_array().expect("read rows") {
            let name = row["name"].as_str().expect("a row name");
            seen.push(name);
            let id = row["windows"].as_str().expect("a Windows id");
            assert_eq!(
                read(read_windows_zone(id)),
                row["expect"],
                "{name}: {}",
                row["note"]
            );
        }
        for name in MUST_RUN {
            assert!(seen.contains(name), "the contract lost the {name} row");
        }
        for name in &seen {
            assert!(
                MUST_RUN.contains(name),
                "{name} is not in MUST_RUN: name it there, so losing it is loud"
            );
        }
    }

    #[test]
    fn the_table_was_resolved_with_the_cores_tzdata() {
        assert_eq!(TZDATA_VERSION, cal_core::TZDATA_VERSION);
    }

    #[test]
    fn every_zone_in_the_table_is_spelled_as_tzdata_spells_it() {
        let zones = WINDOWS_ZONES
            .iter()
            .map(|(_, zone)| *zone)
            .chain(ZONE_WINDOWS.iter().map(|(zone, _)| *zone))
            .chain(OTHER_CLOCK.iter().map(|(zone, _)| *zone))
            .chain(UNMAPPED.iter().copied());
        for zone in zones {
            assert_eq!(cal_core::canonical_zone(zone), Some(zone), "{zone}");
        }
    }

    #[test]
    fn every_id_written_is_one_the_table_reads() {
        for (zone, windows) in ZONE_WINDOWS.iter().chain(OTHER_CLOCK) {
            assert!(
                lookup(WINDOWS_ZONES, windows).is_some(),
                "{zone} -> {windows}"
            );
        }
    }

    #[test]
    fn the_tables_are_sorted_for_binary_search_and_unique() {
        fn sorted(name: &str, keys: &[&str]) {
            for pair in keys.windows(2) {
                assert_eq!(
                    folded(pair[0], pair[1]),
                    Ordering::Less,
                    "{name}: {} must sort before {}",
                    pair[0],
                    pair[1]
                );
            }
        }
        let keys = |table: &[(&'static str, &'static str)]| -> Vec<&'static str> {
            table.iter().map(|(key, _)| *key).collect()
        };
        sorted("WINDOWS_ZONES", &keys(WINDOWS_ZONES));
        sorted("ZONE_WINDOWS", &keys(ZONE_WINDOWS));
        sorted("OTHER_CLOCK", &keys(OTHER_CLOCK));
        sorted("UNMAPPED", UNMAPPED);
    }

    #[test]
    fn no_zone_is_in_two_of_the_lists() {
        for (zone, _) in ZONE_WINDOWS {
            assert!(lookup(OTHER_CLOCK, zone).is_none(), "{zone}");
            assert!(!UNMAPPED.contains(zone), "{zone}");
        }
        for (zone, _) in OTHER_CLOCK {
            assert!(!UNMAPPED.contains(zone), "{zone}");
        }
    }

    #[test]
    fn every_listed_zone_is_written_or_named_as_unstorable() {
        let mut unstorable = Vec::new();
        for listed in cal_core::listed_zones() {
            match windows_zone_for(Some(listed)) {
                WindowsZoneWrite::Id(_) => {}
                WindowsZoneWrite::NotStorable { zone, .. } => unstorable.push(zone),
                WindowsZoneWrite::NoZone => panic!("{listed} is a listed zone, not UTC"),
            }
        }
        // A change here is a CLDR or tzdata update, and asks decision 22a
        // again: these zones are marked in an Exchange calendar's list and
        // cannot be chosen there.
        assert_eq!(
            unstorable,
            [
                "America/Scoresbysund",
                "Antarctica/Casey",
                "Antarctica/Troll",
                "Antarctica/Vostok",
            ]
        );
    }

    #[test]
    fn every_windows_id_writes_back_as_itself() {
        for (windows, zone) in WINDOWS_ZONES {
            let want = if *windows == "UTC" {
                WindowsZoneWrite::NoZone
            } else {
                WindowsZoneWrite::Id(windows)
            };
            assert_eq!(
                windows_zone_for(Some(zone)),
                want,
                "{windows} reads as {zone}"
            );
        }
    }
}
