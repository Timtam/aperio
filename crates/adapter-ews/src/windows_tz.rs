//! Windows ↔ IANA time-zone name translation.
//!
//! Exchange/EWS identifies time zones by WINDOWS names ("Eastern Standard
//! Time"), not IANA names ("America/New_York"). The rest of Aperio — and
//! `Intl` in the frontend recurrence expander — speaks IANA, so a recurring
//! EWS master's zone must be translated to expand DST-correctly.
//!
//! This is the CLDR `windowsZones` "001" (default-territory) mapping for the
//! commonly-used zones. It is intentionally not exhaustive: an unmapped Windows
//! name resolves to `None`, and the event then expands in UTC (the prior
//! behaviour), never worse. Extend the table as needed.
//!
//! ## The two directions are not the same mapping
//!
//! Windows to IANA is one-to-one: each Windows id has one default territory.
//! IANA to Windows is MANY-to-one, and that asymmetry was a real bug. The
//! reverse lookup used to search the table below, which stores only the
//! default member, so Vienna, Zurich, Amsterdam, Stockholm, Madrid, Brussels,
//! Rome, Copenhagen, Oslo, Lisbon and Dublin all resolved to `None` — and
//! `mapping.rs` then wrote the appointment to Exchange with NO time zone at
//! all. Half of Europe, silently, on every create and update.
//!
//! So the write direction has its own table of the CLDR MEMBERS that share a
//! Windows id. It only needs the names a device is likely to report.

/// (Windows zone id, primary IANA zone) pairs, grouped loosely by region.
const WINDOWS_IANA: &[(&str, &str)] = &[
    ("UTC", "Etc/UTC"),
    // Europe / Africa
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("GMT Standard Time", "Europe/London"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("Romance Standard Time", "Europe/Paris"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("Belarus Standard Time", "Europe/Minsk"),
    // Middle East / Asia
    ("Arabic Standard Time", "Asia/Baghdad"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("India Standard Time", "Asia/Kolkata"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("China Standard Time", "Asia/Shanghai"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("Korea Standard Time", "Asia/Seoul"),
    // Australia / Pacific
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    // Americas
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time", "America/Denver"),
    ("Mountain Standard Time (Mexico)", "America/Chihuahua"),
    ("Central Standard Time", "America/Chicago"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Canada Central Standard Time", "America/Regina"),
    ("Eastern Standard Time", "America/New_York"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("US Eastern Standard Time", "America/Indiana/Indianapolis"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("Argentina Standard Time", "America/Argentina/Buenos_Aires"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Brazilian Standard Time", "America/Cuiaba"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Paraguay Standard Time", "America/Asuncion"),
    ("Montevideo Standard Time", "America/Montevideo"),
    ("Cuba Standard Time", "America/Havana"),
    ("Haiti Standard Time", "America/Port-au-Prince"),
    ("Greenland Standard Time", "America/Nuuk"),
    ("Aleutian Standard Time", "America/Adak"),
    ("Yukon Standard Time", "America/Whitehorse"),
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC-11", "Etc/GMT+11"),
    ("UTC-02", "Etc/GMT+2"),
    ("UTC+12", "Etc/GMT-12"),
    ("UTC+13", "Etc/GMT-13"),
    // Extended coverage (less common but real populated zones).
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("Libya Standard Time", "Africa/Tripoli"),
    ("Namibia Standard Time", "Africa/Windhoek"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Sudan Standard Time", "Africa/Khartoum"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Jordan Standard Time", "Asia/Amman"),
    ("Syria Standard Time", "Asia/Damascus"),
    ("Lebanon Standard Time", "Asia/Beirut"),
    ("West Bank Standard Time", "Asia/Hebron"),
    ("Georgian Standard Time", "Asia/Tbilisi"),
    ("Caucasus Standard Time", "Asia/Yerevan"),
    ("Azerbaijan Standard Time", "Asia/Baku"),
    ("Afghanistan Standard Time", "Asia/Kabul"),
    ("West Asia Standard Time", "Asia/Tashkent"),
    ("Central Asia Standard Time", "Asia/Almaty"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Kathmandu"),
    ("Myanmar Standard Time", "Asia/Yangon"),
    ("North Asia Standard Time", "Asia/Krasnoyarsk"),
    ("North Asia East Standard Time", "Asia/Irkutsk"),
    ("Yakutsk Standard Time", "Asia/Yakutsk"),
    ("Vladivostok Standard Time", "Asia/Vladivostok"),
    ("Magadan Standard Time", "Asia/Magadan"),
    ("Ulaanbaatar Standard Time", "Asia/Ulaanbaatar"),
    ("North Korea Standard Time", "Asia/Pyongyang"),
    ("Mauritius Standard Time", "Indian/Mauritius"),
    ("Lord Howe Standard Time", "Australia/Lord_Howe"),
    ("Central Pacific Standard Time", "Pacific/Guadalcanal"),
    ("Fiji Standard Time", "Pacific/Fiji"),
    ("Tonga Standard Time", "Pacific/Tongatapu"),
    ("Samoa Standard Time", "Pacific/Apia"),
    ("Chatham Islands Standard Time", "Pacific/Chatham"),
    ("Norfolk Standard Time", "Pacific/Norfolk"),
];

/// Translate a Windows zone id (as EWS reports it) to its primary IANA name,
/// or `None` when the name isn't in the table.
pub fn windows_to_iana(windows: &str) -> Option<&'static str> {
    WINDOWS_IANA
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(windows))
        .map(|(_, iana)| *iana)
}

/// The OTHER CLDR members of a Windows zone — the ones the table above does
/// not store, because it keeps one default territory each.
///
/// Only the write direction needs these, and only for names a device actually
/// reports: `Intl.DateTimeFormat().resolvedOptions().timeZone` on a machine in
/// Vienna says `Europe/Vienna`, never `Europe/Berlin`.
const IANA_MEMBERS: &[(&str, &str)] = &[
    // UTC spellings that are not the canonical one.
    ("Etc/GMT", "UTC"),
    ("Etc/Universal", "UTC"),
    ("Etc/Zulu", "UTC"),
    ("UTC", "UTC"),
    // GMT Standard Time — the British Isles, Portugal, the Atlantic isles.
    ("Europe/Dublin", "GMT Standard Time"),
    ("Europe/Lisbon", "GMT Standard Time"),
    ("Europe/Guernsey", "GMT Standard Time"),
    ("Europe/Isle_of_Man", "GMT Standard Time"),
    ("Europe/Jersey", "GMT Standard Time"),
    ("Atlantic/Canary", "GMT Standard Time"),
    ("Atlantic/Faroe", "GMT Standard Time"),
    ("Atlantic/Madeira", "GMT Standard Time"),
    // W. Europe Standard Time — Germany's neighbours and the Alpine states.
    ("Europe/Amsterdam", "W. Europe Standard Time"),
    ("Europe/Andorra", "W. Europe Standard Time"),
    ("Europe/Gibraltar", "W. Europe Standard Time"),
    ("Europe/Luxembourg", "W. Europe Standard Time"),
    ("Europe/Malta", "W. Europe Standard Time"),
    ("Europe/Monaco", "W. Europe Standard Time"),
    ("Europe/Oslo", "W. Europe Standard Time"),
    ("Europe/Rome", "W. Europe Standard Time"),
    ("Europe/San_Marino", "W. Europe Standard Time"),
    ("Europe/Stockholm", "W. Europe Standard Time"),
    ("Europe/Vaduz", "W. Europe Standard Time"),
    ("Europe/Vatican", "W. Europe Standard Time"),
    ("Europe/Vienna", "W. Europe Standard Time"),
    ("Europe/Zurich", "W. Europe Standard Time"),
    ("Europe/Busingen", "W. Europe Standard Time"),
    ("Arctic/Longyearbyen", "W. Europe Standard Time"),
    // Romance Standard Time.
    ("Europe/Brussels", "Romance Standard Time"),
    ("Europe/Copenhagen", "Romance Standard Time"),
    ("Europe/Madrid", "Romance Standard Time"),
    ("Africa/Ceuta", "Romance Standard Time"),
    // Central Europe Standard Time.
    ("Europe/Bratislava", "Central Europe Standard Time"),
    ("Europe/Ljubljana", "Central Europe Standard Time"),
    ("Europe/Podgorica", "Central Europe Standard Time"),
    ("Europe/Prague", "Central Europe Standard Time"),
    ("Europe/Tirane", "Central Europe Standard Time"),
    ("Europe/Belgrade", "Central Europe Standard Time"),
    // Central European Standard Time.
    ("Europe/Sarajevo", "Central European Standard Time"),
    ("Europe/Skopje", "Central European Standard Time"),
    ("Europe/Zagreb", "Central European Standard Time"),
    // FLE Standard Time — the Baltics, Finland, Ukraine, Bulgaria.
    ("Europe/Helsinki", "FLE Standard Time"),
    ("Europe/Mariehamn", "FLE Standard Time"),
    ("Europe/Riga", "FLE Standard Time"),
    ("Europe/Sofia", "FLE Standard Time"),
    ("Europe/Tallinn", "FLE Standard Time"),
    ("Europe/Vilnius", "FLE Standard Time"),
    // `Europe/Kiev` became `Europe/Kyiv` in tzdata 2022b. The table above
    // stores the old spelling because every runtime still resolves it; a
    // device on current data reports the new one.
    ("Europe/Kyiv", "FLE Standard Time"),
    // GTB Standard Time.
    ("Europe/Athens", "GTB Standard Time"),
    ("Asia/Nicosia", "GTB Standard Time"),
    ("Europe/Nicosia", "GTB Standard Time"),
    // Greenwich Standard Time — West Africa, on UTC without DST.
    ("Africa/Abidjan", "Greenwich Standard Time"),
    ("Africa/Accra", "Greenwich Standard Time"),
    ("Africa/Dakar", "Greenwich Standard Time"),
    ("Atlantic/St_Helena", "Greenwich Standard Time"),
    // North America.
    ("America/Detroit", "Eastern Standard Time"),
    ("America/Toronto", "Eastern Standard Time"),
    ("America/Nassau", "Eastern Standard Time"),
    ("America/Iqaluit", "Eastern Standard Time"),
    ("America/Winnipeg", "Central Standard Time"),
    ("America/Matamoros", "Central Standard Time"),
    ("America/Boise", "Mountain Standard Time"),
    ("America/Edmonton", "Mountain Standard Time"),
    ("America/Vancouver", "Pacific Standard Time"),
    ("America/Tijuana", "Pacific Standard Time"),
    // Asia and the Pacific.
    ("Asia/Hong_Kong", "China Standard Time"),
    ("Asia/Macau", "China Standard Time"),
    ("Australia/Melbourne", "AUS Eastern Standard Time"),
];

/// Translate an IANA zone to a Windows zone id for writing back to EWS, or
/// `None` when there's no mapping (then we omit the zone rather than guess).
///
/// Checks the default-territory table first, then the other CLDR members —
/// see the module docs for why the write direction needs its own table.
pub fn iana_to_windows(iana: &str) -> Option<&'static str> {
    WINDOWS_IANA
        .iter()
        .find(|(_, i)| i.eq_ignore_ascii_case(iana))
        .map(|(w, _)| *w)
        .or_else(|| {
            IANA_MEMBERS
                .iter()
                .find(|(i, _)| i.eq_ignore_ascii_case(iana))
                .map(|(_, w)| *w)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_zones_both_ways() {
        assert_eq!(
            windows_to_iana("Eastern Standard Time"),
            Some("America/New_York")
        );
        assert_eq!(
            windows_to_iana("W. Europe Standard Time"),
            Some("Europe/Berlin")
        );
        assert_eq!(windows_to_iana("Tokyo Standard Time"), Some("Asia/Tokyo"));
        assert_eq!(
            iana_to_windows("America/New_York"),
            Some("Eastern Standard Time")
        );
        assert_eq!(
            iana_to_windows("Europe/Berlin"),
            Some("W. Europe Standard Time")
        );
    }

    #[test]
    fn the_write_direction_covers_the_other_members_of_a_windows_zone() {
        // The bug this pins down: each of these is a CLDR member of a Windows
        // zone whose DEFAULT member is another city, so the reverse lookup over
        // the one-to-one table returned None — and `mapping.rs` then wrote the
        // appointment to Exchange with no time zone at all.
        for (iana, windows) in [
            ("Europe/Vienna", "W. Europe Standard Time"),
            ("Europe/Zurich", "W. Europe Standard Time"),
            ("Europe/Amsterdam", "W. Europe Standard Time"),
            ("Europe/Stockholm", "W. Europe Standard Time"),
            ("Europe/Rome", "W. Europe Standard Time"),
            ("Europe/Oslo", "W. Europe Standard Time"),
            ("Europe/Madrid", "Romance Standard Time"),
            ("Europe/Brussels", "Romance Standard Time"),
            ("Europe/Copenhagen", "Romance Standard Time"),
            ("Europe/Lisbon", "GMT Standard Time"),
            ("Europe/Dublin", "GMT Standard Time"),
            ("Europe/Prague", "Central Europe Standard Time"),
            ("Europe/Helsinki", "FLE Standard Time"),
            ("Europe/Athens", "GTB Standard Time"),
            ("America/Toronto", "Eastern Standard Time"),
            ("America/Vancouver", "Pacific Standard Time"),
        ] {
            assert_eq!(iana_to_windows(iana), Some(windows), "{iana}");
        }
    }

    #[test]
    fn the_renamed_ukrainian_zone_resolves_under_both_spellings() {
        // tzdata 2022b renamed Europe/Kiev to Europe/Kyiv; a current runtime
        // reports the new name, the table stores the old one.
        assert_eq!(iana_to_windows("Europe/Kiev"), Some("FLE Standard Time"));
        assert_eq!(iana_to_windows("Europe/Kyiv"), Some("FLE Standard Time"));
    }

    #[test]
    fn a_default_member_still_wins_over_the_member_table() {
        // Both tables are consulted; the default-territory one answers first,
        // so a zone that is in both keeps its own Windows id.
        assert_eq!(
            iana_to_windows("Europe/Berlin"),
            Some("W. Europe Standard Time")
        );
        assert_eq!(
            iana_to_windows("Europe/Paris"),
            Some("Romance Standard Time")
        );
    }

    #[test]
    fn unknown_zone_is_none() {
        assert_eq!(windows_to_iana("Totally Made Up Time"), None);
        assert_eq!(iana_to_windows("Mars/Olympus_Mons"), None);
    }

    #[test]
    fn windows_lookup_is_case_insensitive() {
        assert_eq!(
            windows_to_iana("eastern standard time"),
            Some("America/New_York")
        );
    }
}
