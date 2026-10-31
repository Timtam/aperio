//! Windows ↔ tzdata time-zone names.
//!
//! Exchange names a series' zone by its WINDOWS id ("W. Europe Standard
//! Time"), and so do the calendar files Outlook and Teams write; the rest of
//! Aperio speaks tzdata ("Europe/Berlin"). The translation is the Unicode CLDR
//! `windowsZones` mapping, generated into `windows_zone/table.rs` by
//! `cargo xtask windows-zones` from the file pinned in `cldr/` (release tag,
//! sha256 and licence beside it). Nothing here is kept by hand: a hand-kept
//! copy had drifted to three wrong rows and wrote 174 of the 312 listed zones
//! without a zone.
//!
//! The tables live in the core because more than one reader needs them: the
//! Exchange adapter, which reads and writes Windows ids (its rules, which id
//! means no zone and which zone Exchange cannot store, stay in its
//! `windows_tz`), and the iCalendar reader, which meets them in a `TZID`.

#[rustfmt::skip]
mod table;

pub use table::{
    CLDR_RELEASE, OTHER_CLOCK, TABLE_ID, TZDATA_VERSION, UNMAPPED, WINDOWS_ZONES, ZONE_WINDOWS,
};

use std::cmp::Ordering;

/// ASCII case folded, the order the generated tables are sorted in.
pub fn folded(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// The value `key` maps to in one of the generated tables, in any ASCII case.
pub fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .binary_search_by(|(k, _)| folded(k, key))
        .ok()
        .map(|at| table[at].1)
}

/// The tzdata zone a Windows zone id names — its CLDR default ("001") row, in
/// tzdata's canonical spelling — in any ASCII case. `None` for an id the table
/// does not know: a custom definition, an id only one server's registry has.
pub fn tzdata_for_windows(id: &str) -> Option<&'static str> {
    lookup(WINDOWS_ZONES, id.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_id_reads_as_its_zone_in_any_case() {
        assert_eq!(
            tzdata_for_windows("W. Europe Standard Time"),
            Some("Europe/Berlin")
        );
        assert_eq!(
            tzdata_for_windows("w. europe standard time"),
            Some("Europe/Berlin")
        );
        assert_eq!(
            tzdata_for_windows(" India Standard Time "),
            Some("Asia/Kolkata")
        );
        assert_eq!(tzdata_for_windows("Customized Time Zone"), None);
        assert_eq!(tzdata_for_windows("Europe/Berlin"), None);
    }

    #[test]
    fn the_table_was_resolved_with_the_cores_tzdata() {
        assert_eq!(TZDATA_VERSION, crate::TZDATA_VERSION);
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
            assert_eq!(crate::canonical_zone(zone), Some(zone), "{zone}");
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
}
