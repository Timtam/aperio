//! Reading an `RRULE` into its parts, once for every rule that asks.
//!
//! [`series_shift`](crate::series_shift) has read rules this way since it was
//! written; [`recurrence_summary`](crate::recurrence_summary) asks the same
//! questions of the same text. Two readers would drift: one would accept a
//! part given twice, or a lowercase rule, or `INTERVAL=0`, where the other
//! refuses, and the sentence a user hears would describe a rule that is not
//! the one that recurs. So the reading lives here, and both use it.
//!
//! Only the reading. What each part *means* for moving a series, or for a
//! sentence, stays in the module that answers that question.

use crate::types::Weekday;

/// The rule could not be read at all: no `FREQ`, a part without `=`, or a part
/// given twice (RFC 5545 allows each once, and readers disagree on which copy
/// wins, so there is no one rule to follow).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Unreadable;

/// One `KEY=value` part: the key uppercased for matching, the text as written.
pub(crate) struct Part {
    pub key: String,
    pub value: String,
    pub raw: String,
}

/// The two-letter weekday tokens of `BYDAY` and `WKST`, in the order RFC 5545
/// counts a week from Monday.
pub(crate) const WEEKDAY_TOKENS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// The days in that order, to read a token as the day the app names.
pub(crate) const WEEKDAYS_FROM_MONDAY: [Weekday; 7] = [
    Weekday::Monday,
    Weekday::Tuesday,
    Weekday::Wednesday,
    Weekday::Thursday,
    Weekday::Friday,
    Weekday::Saturday,
    Weekday::Sunday,
];

/// How often a rule recurs. The four the app itself writes are
/// [`crate::RecurrenceFrequency`]; a provider's rule may also recur by the
/// hour or faster, which this reading keeps apart instead of dropping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Freq {
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// The rule's parts, and the `RRULE:` prefix as it was written (empty when the
/// rule came without one). Whitespace around a value is dropped, keys are
/// uppercased, and empty parts (a trailing `;`) are skipped — the way
/// `series_shift` has always read a rule.
pub(crate) fn parse_parts(rrule: &str) -> Result<(&str, Vec<Part>), Unreadable> {
    let trimmed = rrule.trim();
    let (prefix, body) = match trimmed.get(..6) {
        Some(p) if p.eq_ignore_ascii_case("RRULE:") => trimmed.split_at(6),
        _ => ("", trimmed),
    };
    let mut parts: Vec<Part> = Vec::new();
    for raw in body.split(';').filter(|p| !p.trim().is_empty()) {
        let (key, value) = raw.split_once('=').ok_or(Unreadable)?;
        let key = key.trim().to_ascii_uppercase();
        if parts.iter().any(|p| p.key == key) {
            return Err(Unreadable);
        }
        parts.push(Part {
            key,
            value: value.trim().to_string(),
            raw: raw.to_string(),
        });
    }
    Ok((prefix, parts))
}

/// The value of `key`, if the rule has that part.
pub(crate) fn part<'a>(parts: &'a [Part], key: &str) -> Option<&'a str> {
    parts
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.value.as_str())
}

/// The `FREQ` value, read without regard to case. `None` when the rule names
/// none of the seven.
pub(crate) fn parse_freq(value: &str) -> Option<Freq> {
    Some(match value.trim().to_ascii_uppercase().as_str() {
        "SECONDLY" => Freq::Secondly,
        "MINUTELY" => Freq::Minutely,
        "HOURLY" => Freq::Hourly,
        "DAILY" => Freq::Daily,
        "WEEKLY" => Freq::Weekly,
        "MONTHLY" => Freq::Monthly,
        "YEARLY" => Freq::Yearly,
        _ => return None,
    })
}

/// `INTERVAL`, which is 1 when the rule leaves it out. `None` when it is not a
/// whole number of at least 1 — `INTERVAL=0` recurs never, and no reader
/// agrees on what it means.
pub(crate) fn parse_interval(value: Option<&str>) -> Option<u32> {
    match value {
        Some(v) => v.parse::<u32>().ok().filter(|n| *n >= 1),
        None => Some(1),
    }
}

/// One `BYDAY` token: its ordinal, if it has one (`2SU` is the second Sunday,
/// `-1FR` the last Friday), and the weekday. `None` when it is neither.
pub(crate) fn parse_weekday_token(token: &str) -> Option<(Option<i8>, Weekday)> {
    let token = token.trim().to_ascii_uppercase();
    if !token.is_ascii() || token.len() < 2 {
        return None;
    }
    let (ordinal, day) = token.split_at(token.len() - 2);
    let day = WEEKDAYS_FROM_MONDAY[WEEKDAY_TOKENS.iter().position(|w| *w == day)?];
    if ordinal.is_empty() {
        return Some((None, day));
    }
    let ordinal: i8 = ordinal.parse().ok()?;
    if ordinal == 0 {
        return None;
    }
    Some((Some(ordinal), day))
}

/// Whether an `UNTIL` value is a UTC date-time (`YYYYMMDDTHHMMSSZ`) rather
/// than a date or a floating time.
pub(crate) fn is_utc_date_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 16
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8].eq_ignore_ascii_case(&b'T')
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15].eq_ignore_ascii_case(&b'Z')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_is_read_the_way_both_readers_need_it() {
        let (prefix, parts) = parse_parts("rrule:FREQ=weekly;byday=mo,we;INTERVAL=2;").unwrap();
        assert_eq!(prefix, "rrule:");
        assert_eq!(parts.len(), 3);
        assert_eq!(part(&parts, "BYDAY"), Some("mo,we"));
        assert_eq!(
            parse_freq(part(&parts, "FREQ").unwrap()),
            Some(Freq::Weekly)
        );
        assert_eq!(parse_interval(part(&parts, "INTERVAL")), Some(2));
        assert_eq!(parse_interval(None), Some(1));
        assert_eq!(parse_interval(Some("0")), None);
        assert_eq!(parse_interval(Some("two")), None);
        // A part given twice, and a part without a value, are not read.
        assert!(parse_parts("FREQ=WEEKLY;INTERVAL=2;interval=3").is_err());
        assert!(parse_parts("FREQ=WEEKLY;BYDAY").is_err());
        assert_eq!(parse_freq("Monthly"), Some(Freq::Monthly));
        assert_eq!(parse_freq("FORTNIGHTLY"), None);
    }

    #[test]
    fn a_weekday_token_carries_its_ordinal() {
        assert_eq!(parse_weekday_token("we"), Some((None, Weekday::Wednesday)));
        assert_eq!(parse_weekday_token("2SU"), Some((Some(2), Weekday::Sunday)));
        assert_eq!(
            parse_weekday_token("-1FR"),
            Some((Some(-1), Weekday::Friday))
        );
        assert_eq!(parse_weekday_token("0MO"), None);
        assert_eq!(parse_weekday_token("XX"), None);
        assert_eq!(parse_weekday_token("M"), None);
    }

    #[test]
    fn only_a_utc_date_time_until_is_one() {
        assert!(is_utc_date_time("20260630T235959Z"));
        assert!(is_utc_date_time("20260630T235959z"));
        assert!(!is_utc_date_time("20260630"));
        assert!(!is_utc_date_time("20260630T235959"));
        assert!(!is_utc_date_time("2026-06-30T23:59:59Z"));
    }
}
