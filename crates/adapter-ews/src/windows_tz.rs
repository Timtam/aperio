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
//! ## Reading a series' zone
//!
//! A series created without a zone comes back from Exchange with the start zone
//! `Greenwich Standard Time` and the end zone `tzone://Microsoft/Utc` (live
//! test, DESIGN-series-time-zone.md stage 4). That end zone means no zone
//! (decision 43b). Otherwise the start zone's id reads as the zone of its
//! default ("001") row, in tzdata's canonical spelling: `India Standard Time`
//! is `Asia/Kolkata`, not CLDR's `Asia/Calcutta`. Exchange keeps one id for a
//! group of cities on one clock, so a Vienna series reads back as Berlin. The
//! id `UTC`, and an id the table does not know — a custom definition, an id
//! only a server's registry has — mean no zone: the series repeats in UTC.
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
//! for it, CLDR's id runs another clock than the zone in the five years after
//! the pinned release, or the server does not know the id (41a). A server
//! refuses an id it does not know with `ErrorTimeZone`, and the whole save
//! fails; Exchange 2019 does not know `Sao Tome Standard Time`.
//!
//! ## The translation id
//!
//! Events the host caches were translated with one table and one reading rule.
//! [`translation_id`] names both: the generated table's `TABLE_ID`, which the
//! generator derives from the table's rows, and [`READ_RULE`], which is bumped
//! by hand whenever the way an id becomes a series' zone changes outside the
//! table — here in [`read_series_zone`], or in the read path of `mapping.rs`.
//! The EWS delta sync compares it with the one in the host's token and emits
//! every cached item again when they differ.

#[rustfmt::skip]
mod windows_zones;

pub use windows_zones::{CLDR_RELEASE, TZDATA_VERSION};

use std::cmp::Ordering;
use std::collections::BTreeSet;

use windows_zones::{OTHER_CLOCK, TABLE_ID, UNMAPPED, WINDOWS_ZONES, ZONE_WINDOWS};

/// The reading rule's version. Bump it when an id read from Exchange becomes
/// a series' zone differently without the generated table changing, so every
/// cached event is translated again.
///
/// 1: stage 4 — an id reads as its 001 zone, canonical; `UTC` and unknown ids
/// are no zone.
/// 2: stage 4 review — the end zone `tzone://Microsoft/Utc` means no zone.
pub const READ_RULE: u32 = 2;

/// The end zone Exchange stores for a series created without a zone.
const NO_ZONE_END: &str = "tzone://Microsoft/Utc";

/// Which translation cached events were made with: the table's rows and the
/// reading rule.
pub fn translation_id() -> String {
    format!("{TABLE_ID}.{READ_RULE}")
}

/// What a Windows zone id read from Exchange means for a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsZoneRead {
    /// A zone, in tzdata's canonical spelling.
    Zone(&'static str),
    /// No zone, so the series repeats in UTC: the id `UTC`, or a series
    /// Exchange marks as created without a zone.
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
    /// The server does not know CLDR's id for it, and would refuse the save.
    NotOnServer { windows: &'static str },
}

/// The Windows zone ids one Exchange server knows, from `GetServerTimeZones`,
/// compared in ASCII case folded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerTimeZones(BTreeSet<String>);

impl ServerTimeZones {
    pub fn new<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self(
            ids.into_iter()
                .map(|id| id.as_ref().to_ascii_lowercase())
                .collect(),
        )
    }

    /// Whether the server knows `windows`.
    pub fn knows(&self, windows: &str) -> bool {
        self.0.contains(&windows.to_ascii_lowercase())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A Windows zone id as Exchange reports it, in any ASCII case.
pub fn read_windows_zone(id: &str) -> WindowsZoneRead {
    match lookup(WINDOWS_ZONES, id) {
        Some(zone) if cal_core::series_clock_zone(Some(zone)).is_none() => WindowsZoneRead::Utc,
        Some(zone) => WindowsZoneRead::Zone(zone),
        None => WindowsZoneRead::Unknown,
    }
}

/// A series' zone from the StartTimeZone and EndTimeZone ids Exchange reports.
/// The end zone `tzone://Microsoft/Utc`, in any ASCII case, means no zone
/// whatever the start zone is (43b); otherwise the start id is read, and
/// `None` means Exchange reported no start zone.
pub fn read_series_zone(start: Option<&str>, end: Option<&str>) -> Option<WindowsZoneRead> {
    if end.is_some_and(|end| end.eq_ignore_ascii_case(NO_ZONE_END)) {
        return Some(WindowsZoneRead::Utc);
    }
    start.map(read_windows_zone)
}

/// The Windows id Exchange gets for a series' stored zone. `server` is the
/// list the server knows; `None`, or an empty list, when it could not be asked.
pub fn windows_zone_for(tzid: Option<&str>, server: Option<&ServerTimeZones>) -> WindowsZoneWrite {
    let Some(zone) = cal_core::series_clock_zone(tzid).and_then(cal_core::canonical_zone) else {
        return WindowsZoneWrite::NoZone;
    };
    if let Some(windows) = lookup(ZONE_WINDOWS, zone) {
        return match server {
            Some(server) if !server.is_empty() && !server.knows(windows) => {
                WindowsZoneWrite::NotStorable {
                    zone,
                    reason: NotStorable::NotOnServer { windows },
                }
            }
            _ => WindowsZoneWrite::Id(windows),
        };
    }
    let reason = if let Some(windows) = lookup(OTHER_CLOCK, zone) {
        NotStorable::OtherClock { windows }
    } else {
        if UNMAPPED.binary_search_by(|z| folded(z, zone)).is_err() {
            // The generator places every zone of its tzdata in one of the three
            // lists, and CI's `windows-zones --check` holds the committed table
            // to that. A zone in none of them means a table generated from
            // another tzdata release than the core's.
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
        "greenwich-created-without-zone",
        "utc-end-zone-in-any-case",
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
            WindowsZoneWrite::NotStorable {
                reason: NotStorable::NotOnServer { windows },
                ..
            } => json!({ "not_storable": "not_on_server", "windows": windows }),
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
                written(windows_zone_for(row["tzid"].as_str(), None)),
                row["expect"],
                "{name}: {}",
                row["note"]
            );
        }
        for row in doc["read"].as_array().expect("read rows") {
            let name = row["name"].as_str().expect("a row name");
            seen.push(name);
            let id = row["windows"].as_str().expect("a Windows id");
            let zone = read_series_zone(Some(id), row["end"].as_str()).expect("a start zone");
            assert_eq!(read(zone), row["expect"], "{name}: {}", row["note"]);
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
    fn a_series_without_a_start_zone_has_none() {
        assert_eq!(read_series_zone(None, None), None);
        // The end marker alone still says: no zone.
        assert_eq!(
            read_series_zone(None, Some(NO_ZONE_END)),
            Some(WindowsZoneRead::Utc)
        );
    }

    #[test]
    fn an_id_the_server_does_not_know_is_not_written() {
        // Exchange 2019 (build 15.2.2562) knows 140 ids but not São Tomé's.
        let server = ServerTimeZones::new(["W. Europe Standard Time", "UTC"]);
        assert_eq!(
            windows_zone_for(Some("Europe/Berlin"), Some(&server)),
            WindowsZoneWrite::Id("W. Europe Standard Time")
        );
        assert_eq!(
            windows_zone_for(Some("Africa/Sao_Tome"), Some(&server)),
            WindowsZoneWrite::NotStorable {
                zone: "Africa/Sao_Tome",
                reason: NotStorable::NotOnServer {
                    windows: "Sao Tome Standard Time"
                },
            }
        );
        // In any ASCII case.
        let lower = ServerTimeZones::new(["w. europe standard time"]);
        assert!(lower.knows("W. Europe Standard Time"));
        // A server that could not be asked, or answered with nothing, does not
        // stop a zone: the CLDR id is written as before.
        assert_eq!(
            windows_zone_for(Some("Africa/Sao_Tome"), Some(&ServerTimeZones::default())),
            WindowsZoneWrite::Id("Sao Tome Standard Time")
        );
        assert_eq!(
            windows_zone_for(Some("Africa/Sao_Tome"), None),
            WindowsZoneWrite::Id("Sao Tome Standard Time")
        );
    }

    #[test]
    fn the_table_was_resolved_with_the_cores_tzdata() {
        assert_eq!(TZDATA_VERSION, cal_core::TZDATA_VERSION);
    }

    #[test]
    fn the_translation_id_names_the_table_and_the_reading_rule() {
        assert_eq!(translation_id(), format!("{TABLE_ID}.{READ_RULE}"));
        assert_eq!(TABLE_ID.len(), 16);
        assert!(TABLE_ID.bytes().all(|b| b.is_ascii_hexdigit()));
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
            match windows_zone_for(Some(listed), None) {
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
    fn every_fixed_offset_zone_is_written() {
        // The 26 zones outside the list that are not UTC names: Etc/GMT-14 to
        // Etc/GMT-1 and Etc/GMT+1 to Etc/GMT+12. Each follows its CLDR row.
        let names = (1..=14)
            .map(|n| format!("Etc/GMT-{n}"))
            .chain((1..=12).map(|n| format!("Etc/GMT+{n}")));
        let mut count = 0;
        for name in names {
            assert_eq!(
                cal_core::canonical_zone(&name),
                Some(name.as_str()),
                "{name} is a zone"
            );
            assert!(
                matches!(windows_zone_for(Some(&name), None), WindowsZoneWrite::Id(_)),
                "{name}: {:?}",
                windows_zone_for(Some(&name), None)
            );
            count += 1;
        }
        assert_eq!(count, 26);
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
                windows_zone_for(Some(zone), None),
                want,
                "{windows} reads as {zone}"
            );
        }
    }
}
