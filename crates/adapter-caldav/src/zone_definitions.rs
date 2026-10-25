//! The time zones a calendar body defines itself (`VTIMEZONE`, RFC 5545
//! §3.6.5), for a `TZID` neither tzdata nor the Windows table knows.
//!
//! A `TZID` is usually a tzdata name (Apple, Google, Nextcloud) or a Windows id
//! (Outlook, Teams, Exchange): both resolve without the body
//! (`cal_core::windows_zone`). A file may also name a zone of its own —
//! Outlook's "Customized Time Zone", a server's private label — and then only
//! its `VTIMEZONE` says what clock it runs. This reads that definition, enough
//! for the shapes calendars write: observances with a `DTSTART`, the offsets
//! before and after, and a yearly rule on the nth weekday of a month or a list
//! of `RDATE`s. Anything else in a definition is ignored, and a zone it cannot
//! read stays unresolved, as before.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};

use crate::ical_raw::all_lines;

/// The zones one body defines, by `TZID`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZoneDefinitions(HashMap<String, Vec<Observance>>);

/// One `STANDARD` or `DAYLIGHT` block: from each onset on, the clock runs at
/// `offset_to`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observance {
    /// The first onset, in the local time before it.
    start: NaiveDateTime,
    offset_from: i64,
    offset_to: i64,
    rule: Option<YearlyRule>,
    rdates: Vec<NaiveDateTime>,
}

/// `FREQ=YEARLY;BYMONTH=m;BYDAY=nWD`, optionally bounded by `UNTIL`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct YearlyRule {
    month: u32,
    weekday: Weekday,
    /// 1 to 5 from the start of the month, -1 to -5 from its end.
    nth: i64,
    until: Option<NaiveDateTime>,
}

impl ZoneDefinitions {
    /// The `VTIMEZONE`s of `body`. A definition without a `TZID`, or without
    /// an observance it can read, is left out.
    pub fn from_body(body: &str) -> Self {
        let mut zones = HashMap::new();
        let mut tzid: Option<String> = None;
        let mut observances: Vec<Observance> = Vec::new();
        let mut current: Option<Draft> = None;
        let mut in_zone = false;
        for (name, params, value) in all_lines(body) {
            match name.as_str() {
                "BEGIN" if value.eq_ignore_ascii_case("VTIMEZONE") => {
                    in_zone = true;
                    tzid = None;
                    observances.clear();
                }
                "BEGIN"
                    if in_zone
                        && (value.eq_ignore_ascii_case("STANDARD")
                            || value.eq_ignore_ascii_case("DAYLIGHT")) =>
                {
                    current = Some(Draft::default());
                }
                "END"
                    if in_zone
                        && (value.eq_ignore_ascii_case("STANDARD")
                            || value.eq_ignore_ascii_case("DAYLIGHT")) =>
                {
                    if let Some(observance) = current.take().and_then(Draft::finish) {
                        observances.push(observance);
                    }
                }
                "END" if value.eq_ignore_ascii_case("VTIMEZONE") => {
                    if let Some(id) = tzid.take() {
                        if !observances.is_empty() {
                            zones.insert(id, std::mem::take(&mut observances));
                        }
                    }
                    in_zone = false;
                }
                _ if !in_zone => {}
                "TZID" if current.is_none() => tzid = Some(value.trim().to_string()),
                _ => {
                    if let Some(draft) = current.as_mut() {
                        draft.take(&name, &params, &value);
                    }
                }
            }
        }
        Self(zones)
    }

    /// Whether `tzid` is defined here.
    pub fn defines(&self, tzid: &str) -> bool {
        self.0.contains_key(tzid)
    }

    /// The instant the wall-clock time `local` names in the zone `tzid`, by
    /// its definition here. `None` when the body does not define it.
    pub fn resolve(&self, tzid: &str, local: NaiveDateTime) -> Option<DateTime<Utc>> {
        let observances = self.0.get(tzid)?;
        // The latest onset at or before `local` decides the clock.
        let mut best: Option<(NaiveDateTime, i64)> = None;
        for observance in observances {
            for onset in observance.onsets_through(local) {
                if best.is_none_or(|(at, _)| onset > at) {
                    best = Some((onset, observance.offset_to));
                }
            }
        }
        // Before every onset: the clock the earliest one changes from.
        let offset = match best {
            Some((_, offset)) => offset,
            None => observances.iter().min_by_key(|o| o.start)?.offset_from,
        };
        Some(Utc.from_utc_datetime(&(local - Duration::seconds(offset))))
    }
}

impl Observance {
    /// Every onset at or before `local`, as far back as the year before.
    fn onsets_through(&self, local: NaiveDateTime) -> Vec<NaiveDateTime> {
        let mut onsets = Vec::new();
        if self.start <= local {
            onsets.push(self.start);
        }
        onsets.extend(self.rdates.iter().copied().filter(|d| *d <= local));
        if let Some(rule) = &self.rule {
            for year in [local.year() - 1, local.year()] {
                let Some(day) = rule.day_in(year) else {
                    continue;
                };
                let onset = day.and_time(self.start.time());
                if onset >= self.start
                    && onset <= local
                    && rule.until.is_none_or(|until| onset <= until)
                {
                    onsets.push(onset);
                }
            }
        }
        onsets
    }
}

impl YearlyRule {
    /// The rule's day in `year`.
    fn day_in(&self, year: i32) -> Option<NaiveDate> {
        if self.nth > 0 {
            NaiveDate::from_weekday_of_month_opt(year, self.month, self.weekday, self.nth as u8)
        } else {
            let first_of_next = if self.month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)?
            } else {
                NaiveDate::from_ymd_opt(year, self.month + 1, 1)?
            };
            let last = first_of_next.pred_opt()?;
            let back = (7 + last.weekday().num_days_from_monday() as i64
                - self.weekday.num_days_from_monday() as i64)
                % 7;
            let day = last - Duration::days(back + 7 * (-self.nth - 1));
            (day.month() == self.month).then_some(day)
        }
    }
}

/// An observance as its lines arrive.
#[derive(Default)]
struct Draft {
    start: Option<NaiveDateTime>,
    offset_from: Option<i64>,
    offset_to: Option<i64>,
    rule: Option<YearlyRule>,
    unreadable_rule: bool,
    rdates: Vec<NaiveDateTime>,
}

impl Draft {
    fn take(&mut self, name: &str, _params: &[(String, String)], value: &str) {
        match name {
            "DTSTART" => self.start = local_time(value),
            "TZOFFSETFROM" => self.offset_from = offset_seconds(value),
            "TZOFFSETTO" => self.offset_to = offset_seconds(value),
            "RRULE" => match yearly_rule(value) {
                Some(rule) => self.rule = Some(rule),
                None => self.unreadable_rule = true,
            },
            "RDATE" => self
                .rdates
                .extend(value.split(',').filter_map(|v| local_time(v.trim()))),
            _ => {}
        }
    }

    /// The observance, when every part it needs was read. A rule it cannot
    /// read would place the onsets wrong, so the observance goes with it.
    fn finish(self) -> Option<Observance> {
        if self.unreadable_rule {
            return None;
        }
        Some(Observance {
            start: self.start?,
            offset_from: self.offset_from?,
            offset_to: self.offset_to?,
            rule: self.rule,
            rdates: self.rdates,
        })
    }
}

/// A local date-time, `YYYYMMDDTHHMMSS` (a trailing `Z` is read as local too:
/// some writers put one there by mistake).
fn local_time(value: &str) -> Option<NaiveDateTime> {
    let value = value.trim().trim_end_matches(['Z', 'z']);
    NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()
}

/// `+0100`, `-0530`, `+053000` in seconds east of UTC.
fn offset_seconds(value: &str) -> Option<i64> {
    let value = value.trim();
    let (sign, digits) = match value.as_bytes().first()? {
        b'+' => (1, &value[1..]),
        b'-' => (-1, &value[1..]),
        _ => return None,
    };
    if !(digits.len() == 4 || digits.len() == 6) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[0..2].parse().ok()?;
    let minutes: i64 = digits[2..4].parse().ok()?;
    let seconds: i64 = if digits.len() == 6 {
        digits[4..6].parse().ok()?
    } else {
        0
    };
    Some(sign * (hours * 3600 + minutes * 60 + seconds))
}

/// A yearly rule on the nth weekday of one month — the only shape zone
/// definitions use in practice. `None` for any other.
fn yearly_rule(value: &str) -> Option<YearlyRule> {
    let mut freq_yearly = false;
    let mut month = None;
    let mut by_day = None;
    let mut until = None;
    for part in value.split(';') {
        let (key, val) = part.split_once('=')?;
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => freq_yearly = val.trim().eq_ignore_ascii_case("YEARLY"),
            "BYMONTH" => {
                month = val
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|m| (1..=12).contains(m))
            }
            "BYDAY" => by_day = nth_weekday(val.trim()),
            "UNTIL" => {
                until = local_time(val).or_else(|| {
                    NaiveDate::parse_from_str(val.trim(), "%Y%m%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(23, 59, 59))
                })
            }
            "INTERVAL" if val.trim() != "1" => return None,
            "INTERVAL" | "WKST" => {}
            _ => return None,
        }
    }
    let (nth, weekday) = by_day?;
    freq_yearly.then_some(YearlyRule {
        month: month?,
        weekday,
        nth,
        until,
    })
}

/// `-1SU`, `2SU`, `+1MO`.
fn nth_weekday(value: &str) -> Option<(i64, Weekday)> {
    if value.len() < 3 {
        return None;
    }
    let (count, day) = value.split_at(value.len() - 2);
    let nth: i64 = count.trim_start_matches('+').parse().ok()?;
    if nth == 0 || nth.abs() > 5 {
        return None;
    }
    let weekday = match day.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    };
    Some((nth, weekday))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Outlook writes for a zone it has no id for: Berlin's clock under
    /// a name of its own.
    const CUSTOM: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\nTZID:Customized Time Zone\r\n\
BEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\nEND:STANDARD\r\n\
BEGIN:DAYLIGHT\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\nEND:DAYLIGHT\r\n\
END:VTIMEZONE\r\nEND:VCALENDAR\r\n";

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    fn utc(text: &str) -> DateTime<Utc> {
        Utc.from_utc_datetime(&at(text))
    }

    #[test]
    fn a_defined_zone_runs_its_own_clock_through_the_year() {
        let zones = ZoneDefinitions::from_body(CUSTOM);
        assert!(zones.defines("Customized Time Zone"));
        // Winter: +01:00.
        assert_eq!(
            zones.resolve("Customized Time Zone", at("2026-11-16 10:00")),
            Some(utc("2026-11-16 09:00"))
        );
        // Summer: +02:00, from the last Sunday of March 2026 (29th).
        assert_eq!(
            zones.resolve("Customized Time Zone", at("2026-07-01 10:00")),
            Some(utc("2026-07-01 08:00"))
        );
        assert_eq!(
            zones.resolve("Customized Time Zone", at("2026-03-29 03:30")),
            Some(utc("2026-03-29 01:30"))
        );
        assert_eq!(
            zones.resolve("Customized Time Zone", at("2026-03-28 10:00")),
            Some(utc("2026-03-28 09:00"))
        );
        // Back to winter on the last Sunday of October 2026 (25th).
        assert_eq!(
            zones.resolve("Customized Time Zone", at("2026-10-25 04:00")),
            Some(utc("2026-10-25 03:00"))
        );
        // Early January reads the change of the October before.
        assert_eq!(
            zones.resolve("Customized Time Zone", at("2027-01-02 10:00")),
            Some(utc("2027-01-02 09:00"))
        );
        assert_eq!(zones.resolve("Unbekannt", at("2026-11-16 10:00")), None);
    }

    #[test]
    fn a_zone_without_a_rule_and_one_with_dates_resolve_too() {
        let body = "BEGIN:VTIMEZONE\r\nTZID:Fixed\r\n\
BEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0530\r\nTZOFFSETTO:+0530\r\nEND:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VTIMEZONE\r\nTZID:Dated\r\n\
BEGIN:STANDARD\r\nDTSTART:20261025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\n\
BEGIN:DAYLIGHT\r\nDTSTART:20260329T020000\r\nRDATE:20270328T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nEND:DAYLIGHT\r\n\
END:VTIMEZONE\r\n";
        let zones = ZoneDefinitions::from_body(body);
        assert_eq!(
            zones.resolve("Fixed", at("2026-11-16 10:00")),
            Some(utc("2026-11-16 04:30"))
        );
        assert_eq!(
            zones.resolve("Dated", at("2026-07-01 10:00")),
            Some(utc("2026-07-01 08:00"))
        );
        assert_eq!(
            zones.resolve("Dated", at("2026-12-01 10:00")),
            Some(utc("2026-12-01 09:00"))
        );
        assert_eq!(
            zones.resolve("Dated", at("2027-04-01 10:00")),
            Some(utc("2027-04-01 08:00"))
        );
        // Before its first onset: the clock the earliest one changes from.
        assert_eq!(
            zones.resolve("Dated", at("2026-01-15 10:00")),
            Some(utc("2026-01-15 09:00"))
        );
    }

    #[test]
    fn a_rule_it_cannot_read_leaves_the_zone_unresolved() {
        let body = "BEGIN:VTIMEZONE\r\nTZID:Odd\r\n\
BEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
RRULE:FREQ=MONTHLY;BYDAY=-1SU\r\nEND:STANDARD\r\n\
END:VTIMEZONE\r\n";
        assert!(!ZoneDefinitions::from_body(body).defines("Odd"));
    }

    #[test]
    fn the_nth_weekday_counts_from_either_end_of_the_month() {
        let rule = |nth, month| YearlyRule {
            month,
            weekday: Weekday::Sun,
            nth,
            until: None,
        };
        assert_eq!(
            rule(-1, 3).day_in(2026),
            NaiveDate::from_ymd_opt(2026, 3, 29)
        );
        assert_eq!(rule(2, 3).day_in(2026), NaiveDate::from_ymd_opt(2026, 3, 8));
        assert_eq!(
            rule(1, 11).day_in(2026),
            NaiveDate::from_ymd_opt(2026, 11, 1)
        );
        assert_eq!(
            rule(-1, 12).day_in(2026),
            NaiveDate::from_ymd_opt(2026, 12, 27)
        );
        assert_eq!(rule(-5, 2).day_in(2026), None);
        assert_eq!(offset_seconds("-0530"), Some(-19800));
        assert_eq!(offset_seconds("+053000"), Some(19800));
        assert_eq!(offset_seconds("0100"), None);
    }
}
