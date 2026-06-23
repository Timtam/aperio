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
];

/// Translate a Windows zone id (as EWS reports it) to its primary IANA name,
/// or `None` when the name isn't in the table.
pub fn windows_to_iana(windows: &str) -> Option<&'static str> {
    WINDOWS_IANA
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(windows))
        .map(|(_, iana)| *iana)
}

/// Translate an IANA zone to a Windows zone id for writing back to EWS, or
/// `None` when there's no mapping (then we omit the zone rather than guess).
pub fn iana_to_windows(iana: &str) -> Option<&'static str> {
    WINDOWS_IANA
        .iter()
        .find(|(_, i)| i.eq_ignore_ascii_case(iana))
        .map(|(w, _)| *w)
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
