//! Minimal `VTIMEZONE` generation for zoned events written to CalDAV.
//!
//! RFC 5545 §3.6.5 requires that any `TZID` a `VEVENT` references is DEFINED by a
//! matching `VTIMEZONE` component in the same `VCALENDAR`. Aperio previously wrote
//! `DTSTART;TZID=<zone>` WITHOUT the `VTIMEZONE` — technically invalid. iCloud
//! tolerates that for open-ended rules but drops the recurrence of a zoned
//! recurring event with a `COUNT` bound (it can't resolve the zone to compute the
//! Nth occurrence), so a "repeat 2×" event silently lost its rule.
//!
//! We emit a `VTIMEZONE` derived from `chrono-tz`'s ACTUAL transitions — not a
//! hand-rolled table. `chrono-tz` doesn't expose its transition list publicly
//! (the `TimeSpans` trait is private), so we probe the offset across a year and
//! binary-search the exact transition instants. A zone whose onsets follow a
//! fixed yearly Gregorian rule (Berlin, New York, Sydney, …) gets the compact
//! DAYLIGHT + STANDARD yearly RRULEs Apple/Thunderbird emit. A zone whose onsets
//! DON'T (the Ramadan-tracking zones Africa/Casablanca, Asia/Gaza, … whose
//! transitions drift ~11 days/year; America/Santiago's varying week) gets each
//! real transition emitted explicitly across the event's window — a single
//! yearly rule would place the onset weeks off and shift events by an hour on
//! strict clients. Zones without DST get a single fixed STANDARD.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::{OffsetComponents, OffsetName, Tz};

/// A detected offset change: the first UTC instant on the new offset, plus the
/// offsets on either side and the new side's DST flag + abbreviation.
struct Transition {
    /// UTC instant of the change (first instant on `to_total`).
    at_utc: NaiveDateTime,
    from_total: i32,
    to_total: i32,
    to_is_dst: bool,
    to_name: String,
}

/// A `VTIMEZONE` block (CRLF-terminated iCalendar lines, no trailing CRLF) for
/// `tzid`, or `None` for UTC / an unresolvable zone (a bare-UTC `DTSTART` needs
/// none). `ref_year` is the event's start year.
pub fn vtimezone_for(tzid: &str, ref_year: i32) -> Option<String> {
    if tzid.eq_ignore_ascii_case("UTC") {
        return None;
    }
    let tz: Tz = tzid.parse().ok()?;

    // Anchor the observance onsets to the year BEFORE the event. A VTIMEZONE
    // observance applies from its `DTSTART` onward, so a strict client (iCloud)
    // can only resolve an instant that has an onset at or before it. Anchoring
    // to the event's OWN year would leave a Jan–March event (before that year's
    // spring transition) with no preceding onset — the same unresolvable-zone
    // failure we're fixing. The prior year's onsets precede every instant in the
    // event's year, and (for a fixed-rule zone) the yearly RRULE still extends
    // forward. (Apple anchors Europe/Berlin at 19810329 for the same reason.)
    let base_year = ref_year - 1;
    let transitions = year_transitions(&tz, base_year)?;

    // The current rule = the latest STANDARD-onset and DAYLIGHT-onset transitions
    // in the base year.
    let daylight = transitions.iter().rev().find(|t| t.to_is_dst);
    let standard = transitions.iter().rev().find(|t| !t.to_is_dst);

    let mut out = String::new();
    out.push_str("BEGIN:VTIMEZONE\r\n");
    out.push_str(&format!("TZID:{tzid}\r\n"));

    match (standard, daylight) {
        // A DST zone. If its onsets follow a fixed yearly Gregorian rule (the
        // overwhelming majority — Berlin, New York, Sydney, …) emit the compact,
        // infinite-correct DAYLIGHT + STANDARD yearly rules. If they DON'T — a
        // Ramadan-tracking zone (Africa/Casablanca, Asia/Gaza, …) whose onsets
        // drift ~11 days/year, or a zone whose week varies (America/Santiago) —
        // a single yearly rule would resolve the wrong offset, so emit each real
        // transition explicitly across the event's window instead.
        (Some(std_t), Some(dst_t)) => {
            if rule_is_stable(&tz, base_year) {
                out.push_str(&sub_component("DAYLIGHT", dst_t));
                out.push_str(&sub_component("STANDARD", std_t));
            } else {
                out.push_str(&explicit_observances(
                    &tz,
                    base_year,
                    ref_year + IRREGULAR_HORIZON_YEARS,
                ));
            }
        }
        // No DST change in the year → a single fixed STANDARD at the base offset.
        _ => {
            let off = total_offset(&tz, midyear(base_year)?);
            let name = abbreviation(&tz, midyear(base_year)?);
            out.push_str("BEGIN:STANDARD\r\n");
            out.push_str(&format!("TZOFFSETFROM:{}\r\n", fmt_offset(off)));
            out.push_str(&format!("TZOFFSETTO:{}\r\n", fmt_offset(off)));
            out.push_str(&format!("TZNAME:{name}\r\n"));
            out.push_str("DTSTART:19700101T000000\r\n");
            out.push_str("END:STANDARD\r\n");
        }
    }

    out.push_str("END:VTIMEZONE\r\n");
    Some(out)
}

/// How many years forward from the event an irregular zone's explicit
/// observances cover. Beyond it a strict client reuses the last observance's
/// offset — inherently approximate for an unbounded (NEVER) recurrence in a zone
/// whose onsets can't be expressed as a fixed rule, which is the same trade-off
/// real clients (Apple) make for these zones. A decade covers any COUNT/UNTIL
/// series and a long stretch of an open-ended one.
const IRREGULAR_HORIZON_YEARS: i32 = 10;

/// Emit the shared `BEGIN:<kind> … DTSTART:` head of an observance and return it
/// together with the transition's LOCAL wall-clock (read in the OFFSET-FROM —
/// RFC 5545: a VTIMEZONE `DTSTART` is local time in `TZOFFSETFROM`).
fn observance_open(kind: &str, t: &Transition) -> (String, NaiveDateTime) {
    let local = t.at_utc + Duration::seconds(t.from_total as i64);
    let mut s = String::new();
    s.push_str(&format!("BEGIN:{kind}\r\n"));
    s.push_str(&format!("TZOFFSETFROM:{}\r\n", fmt_offset(t.from_total)));
    s.push_str(&format!("TZOFFSETTO:{}\r\n", fmt_offset(t.to_total)));
    s.push_str(&format!("TZNAME:{}\r\n", t.to_name));
    s.push_str(&format!("DTSTART:{}\r\n", local.format("%Y%m%dT%H%M%S")));
    (s, local)
}

/// A STANDARD/DAYLIGHT sub-component for a transition, as a yearly RRULE anchored
/// on the transition's local wall-clock. Used for fixed-rule zones only.
fn sub_component(kind: &str, t: &Transition) -> String {
    let (mut s, local) = observance_open(kind, t);
    let (ord, weekday) = nth_weekday(local.date());
    s.push_str(&format!(
        "RRULE:FREQ=YEARLY;BYMONTH={};BYDAY={}{}\r\n",
        local.month(),
        ord,
        weekday
    ));
    s.push_str(&format!("END:{kind}\r\n"));
    s
}

/// One explicit observance PER real transition across `[from_year, to_year]`,
/// each with an explicit `DTSTART` and NO `RRULE` — the correct representation
/// for a zone whose onsets don't follow a fixed yearly rule (they'd be misplaced
/// by a single `RRULE`). Every transition in the window resolves to its true
/// offset; the anchor guarantee holds because the window starts in the year
/// before the event.
fn explicit_observances(tz: &Tz, from_year: i32, to_year: i32) -> String {
    let mut out = String::new();
    for year in from_year..=to_year {
        let Some(transitions) = year_transitions(tz, year) else {
            continue;
        };
        for t in &transitions {
            let kind = if t.to_is_dst { "DAYLIGHT" } else { "STANDARD" };
            let (head, _) = observance_open(kind, t);
            out.push_str(&head);
            out.push_str(&format!("END:{kind}\r\n"));
        }
    }
    out
}

/// Whether the zone's DST onsets follow a fixed yearly Gregorian rule that a
/// single `FREQ=YEARLY;BYMONTH;BYDAY` observance reproduces. Samples several
/// consecutive years from `base_year`; stable iff every sampled year has the
/// SAME set of `(is_dst, month, ordinal, weekday)` transition keys. Fails for
/// zones whose onsets drift (Ramadan-tracking Africa/Casablanca, Asia/Gaza),
/// vary by a week (America/Santiago), or key off a fixed day-of-month (whose
/// weekday ordinal moves year to year) — all of which then take the explicit
/// path.
fn rule_is_stable(tz: &Tz, base_year: i32) -> bool {
    const SAMPLE_YEARS: i32 = 6;
    let key_for = |year: i32| -> Option<Vec<(bool, u32, i32, &'static str)>> {
        let mut keys: Vec<_> = year_transitions(tz, year)?
            .iter()
            .map(|t| {
                let local = t.at_utc + Duration::seconds(t.from_total as i64);
                let (ord, wd) = nth_weekday(local.date());
                (t.to_is_dst, local.month(), ord, wd)
            })
            .collect();
        keys.sort();
        Some(keys)
    };
    let Some(reference) = key_for(base_year) else {
        return false;
    };
    if reference.is_empty() {
        return false;
    }
    ((base_year + 1)..=(base_year + SAMPLE_YEARS)).all(|y| key_for(y).as_ref() == Some(&reference))
}

/// Every offset transition inside `year` (UTC), earliest first. Probes day by day
/// and binary-searches the exact minute of each change.
fn year_transitions(tz: &Tz, year: i32) -> Option<Vec<Transition>> {
    let mut out = Vec::new();
    let mut day = NaiveDate::from_ymd_opt(year, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let end = NaiveDate::from_ymd_opt(year + 1, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let mut prev = total_offset(tz, day);
    while day < end {
        let next = day + Duration::days(1);
        let next_off = total_offset(tz, next);
        if next_off != prev {
            // Narrow [day, next] to the first instant carrying the new offset.
            // Bisect on WHOLE seconds (`Duration / 2` would land on fractional
            // seconds and leave `DTSTART` at e.g. `T020028`); IANA transitions
            // sit on a whole second, so this converges exactly onto it.
            let (mut lo, mut hi) = (day, next);
            while (hi - lo).num_seconds() > 1 {
                let mid = lo + Duration::seconds((hi - lo).num_seconds() / 2);
                if total_offset(tz, mid) == prev {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            out.push(Transition {
                at_utc: hi,
                from_total: prev,
                to_total: next_off,
                to_is_dst: is_dst(tz, hi),
                to_name: abbreviation(tz, hi),
            });
            prev = next_off;
        }
        day = next;
    }
    Some(out)
}

fn total_offset(tz: &Tz, utc: NaiveDateTime) -> i32 {
    let off = tz.offset_from_utc_datetime(&utc);
    (off.base_utc_offset() + off.dst_offset()).num_seconds() as i32
}

fn is_dst(tz: &Tz, utc: NaiveDateTime) -> bool {
    tz.offset_from_utc_datetime(&utc).dst_offset().num_seconds() != 0
}

fn abbreviation(tz: &Tz, utc: NaiveDateTime) -> String {
    tz.offset_from_utc_datetime(&utc).abbreviation().to_string()
}

fn midyear(year: i32) -> Option<NaiveDateTime> {
    NaiveDate::from_ymd_opt(year, 7, 1)?.and_hms_opt(0, 0, 0)
}

/// `+0100` / `-0500` from a whole-second offset (rounded to the minute).
fn fmt_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let mins = seconds.abs() / 60;
    format!("{sign}{:02}{:02}", mins / 60, mins % 60)
}

/// `(ordinal, weekday-code)` for a date's weekday within its month: `-1` (last)
/// when the date is in the final week, else the 1-based ordinal (`1`..`4`).
fn nth_weekday(date: NaiveDate) -> (i32, &'static str) {
    const CODES: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
    let code = CODES[date.weekday().num_days_from_monday() as usize];
    let last_day = NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1)
        .or_else(|| NaiveDate::from_ymd_opt(date.year() + 1, 1, 1))
        .map(|first_next| first_next.pred_opt().map_or(31, |d| d.day()))
        .unwrap_or(31);
    if date.day() > last_day.saturating_sub(7) {
        (-1, code)
    } else {
        (((date.day() - 1) / 7 + 1) as i32, code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the value of a single-occurrence line (`KEY:value`) out of a block.
    fn line<'a>(block: &'a str, key: &str) -> Option<&'a str> {
        block
            .lines()
            .find_map(|l| l.strip_prefix(key).map(|v| v.trim_end_matches('\r')))
    }

    /// Split a VTIMEZONE into its `(STANDARD, DAYLIGHT)` sub-component strings.
    fn sub<'a>(vtz: &'a str, kind: &str) -> Option<&'a str> {
        let begin = vtz.find(&format!("BEGIN:{kind}"))?;
        let end = vtz[begin..].find(&format!("END:{kind}"))? + begin;
        Some(&vtz[begin..end])
    }

    #[test]
    fn utc_and_unresolvable_zones_emit_nothing() {
        assert_eq!(vtimezone_for("UTC", 2026), None);
        assert_eq!(vtimezone_for("utc", 2026), None);
        assert_eq!(vtimezone_for("Not/AZone", 2026), None);
        assert_eq!(vtimezone_for("", 2026), None);
    }

    #[test]
    fn europe_berlin_has_last_sunday_dst_rules() {
        let vtz = vtimezone_for("Europe/Berlin", 2026).expect("Berlin resolves");
        assert!(vtz.starts_with("BEGIN:VTIMEZONE\r\n"));
        assert!(vtz.trim_end().ends_with("END:VTIMEZONE"));
        assert_eq!(line(&vtz, "TZID:"), Some("Europe/Berlin"));
        // Every line is CRLF-terminated (RFC 5545 §3.1) so the block can splice
        // straight into the icalendar-crate output without mixing endings.
        assert!(vtz.split("\r\n").all(|l| !l.contains('\n')));

        // Spring: CET(+0100) → CEST(+0200), last Sunday of March at 02:00 local.
        let daylight = sub(&vtz, "DAYLIGHT").expect("has DAYLIGHT");
        assert_eq!(line(daylight, "TZOFFSETFROM:"), Some("+0100"));
        assert_eq!(line(daylight, "TZOFFSETTO:"), Some("+0200"));
        assert_eq!(line(daylight, "TZNAME:"), Some("CEST"));
        assert_eq!(
            line(daylight, "RRULE:"),
            Some("FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU")
        );
        // Anchored to the year BEFORE the event (2025) so an onset precedes any
        // instant in 2026: last Sunday of March 2025 = the 30th.
        assert_eq!(line(daylight, "DTSTART:"), Some("20250330T020000"));

        // Autumn: CEST(+0200) → CET(+0100), last Sunday of October at 03:00 local.
        let standard = sub(&vtz, "STANDARD").expect("has STANDARD");
        assert_eq!(line(standard, "TZOFFSETFROM:"), Some("+0200"));
        assert_eq!(line(standard, "TZOFFSETTO:"), Some("+0100"));
        assert_eq!(line(standard, "TZNAME:"), Some("CET"));
        assert_eq!(
            line(standard, "RRULE:"),
            Some("FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU")
        );
        // Last Sunday of October 2025 = the 26th.
        assert_eq!(line(standard, "DTSTART:"), Some("20251026T030000"));
    }

    #[test]
    fn observance_onsets_precede_an_early_year_event() {
        // The failure this guards against: an event whose first occurrence is
        // before its year's spring transition (e.g. a January meal plan) must
        // still have a VTIMEZONE onset at or before it, or a strict client can't
        // resolve the zone. Both observance DTSTARTs must fall in the year
        // before the event (2025 for a 2026 event), not the event's own year.
        let vtz = vtimezone_for("Europe/Berlin", 2026).expect("Berlin resolves");
        for kind in ["DAYLIGHT", "STANDARD"] {
            let block = sub(&vtz, kind).unwrap();
            let dtstart = line(block, "DTSTART:").unwrap();
            assert!(
                dtstart.starts_with("2025"),
                "{kind} DTSTART must anchor in the prior year, got {dtstart}"
            );
        }
    }

    #[test]
    fn america_new_york_has_us_dst_rules() {
        let vtz = vtimezone_for("America/New_York", 2026).expect("NY resolves");
        assert_eq!(line(&vtz, "TZID:"), Some("America/New_York"));

        // Spring forward: 2nd Sunday of March, EST(-0500) → EDT(-0400).
        let daylight = sub(&vtz, "DAYLIGHT").expect("has DAYLIGHT");
        assert_eq!(line(daylight, "TZOFFSETFROM:"), Some("-0500"));
        assert_eq!(line(daylight, "TZOFFSETTO:"), Some("-0400"));
        assert_eq!(line(daylight, "TZNAME:"), Some("EDT"));
        assert_eq!(
            line(daylight, "RRULE:"),
            Some("FREQ=YEARLY;BYMONTH=3;BYDAY=2SU")
        );

        // Fall back: 1st Sunday of November, EDT(-0400) → EST(-0500).
        let standard = sub(&vtz, "STANDARD").expect("has STANDARD");
        assert_eq!(line(standard, "TZOFFSETFROM:"), Some("-0400"));
        assert_eq!(line(standard, "TZOFFSETTO:"), Some("-0500"));
        assert_eq!(line(standard, "TZNAME:"), Some("EST"));
        assert_eq!(
            line(standard, "RRULE:"),
            Some("FREQ=YEARLY;BYMONTH=11;BYDAY=1SU")
        );
    }

    #[test]
    fn no_dst_zones_emit_a_single_fixed_standard() {
        // Tokyo: fixed +0900, no DST since 1951.
        let tokyo = vtimezone_for("Asia/Tokyo", 2026).expect("Tokyo resolves");
        assert!(sub(&tokyo, "DAYLIGHT").is_none());
        let std = sub(&tokyo, "STANDARD").expect("has STANDARD");
        assert_eq!(line(std, "TZOFFSETFROM:"), Some("+0900"));
        assert_eq!(line(std, "TZOFFSETTO:"), Some("+0900"));
        assert_eq!(line(std, "TZNAME:"), Some("JST"));
        assert!(line(std, "RRULE:").is_none());

        // Arizona: fixed -0700, opts out of US DST.
        let phoenix = vtimezone_for("America/Phoenix", 2026).expect("Phoenix resolves");
        assert!(sub(&phoenix, "DAYLIGHT").is_none());
        let std = sub(&phoenix, "STANDARD").expect("has STANDARD");
        assert_eq!(line(std, "TZOFFSETTO:"), Some("-0700"));
        assert_eq!(line(std, "TZNAME:"), Some("MST"));
    }

    #[test]
    fn fixed_rule_zones_are_detected_as_stable() {
        // Ordinary DST zones follow a fixed yearly Gregorian rule.
        assert!(rule_is_stable(&"Europe/Berlin".parse().unwrap(), 2025));
        assert!(rule_is_stable(&"America/New_York".parse().unwrap(), 2025));
        assert!(rule_is_stable(&"Australia/Sydney".parse().unwrap(), 2025));
    }

    #[test]
    fn ramadan_zone_uses_explicit_observances_not_a_yearly_rule() {
        // Africa/Casablanca's DST tracks Ramadan (onsets drift ~11 days/year), so
        // a single FREQ=YEARLY;BYDAY rule would place the transition weeks off and
        // shift events by an hour. It must be detected as unstable and emitted as
        // explicit per-transition observances (no RRULE) across the event window.
        let tz: Tz = "Africa/Casablanca".parse().unwrap();
        assert!(
            !rule_is_stable(&tz, 2025),
            "Casablanca is not a fixed-rule zone"
        );

        let vtz = vtimezone_for("Africa/Casablanca", 2026).expect("Casablanca resolves");
        assert!(
            !vtz.contains("RRULE"),
            "an irregular zone must not use a yearly RRULE:\n{vtz}"
        );
        // Explicit coverage spans multiple years, including the event's own year,
        // so every occurrence resolves to its true offset.
        assert!(
            vtz.contains("DTSTART:2026"),
            "covers the event year:\n{vtz}"
        );
        let observances =
            vtz.matches("BEGIN:STANDARD").count() + vtz.matches("BEGIN:DAYLIGHT").count();
        assert!(
            observances >= 4,
            "expected explicit observances across several years, got {observances}:\n{vtz}"
        );
        // Sanity: the observances that flip Casablanca to +01 (Ramadan end) and to
        // +00 (Ramadan start) are both represented.
        assert!(vtz.contains("TZOFFSETTO:+0100"), "{vtz}");
        assert!(vtz.contains("TZOFFSETTO:+0000"), "{vtz}");
    }

    #[test]
    fn india_half_hour_offset_formats_correctly() {
        let vtz = vtimezone_for("Asia/Kolkata", 2026).expect("Kolkata resolves");
        let std = sub(&vtz, "STANDARD").expect("has STANDARD");
        assert_eq!(line(std, "TZOFFSETTO:"), Some("+0530"));
    }

    #[test]
    fn fmt_offset_covers_sign_and_half_hours() {
        assert_eq!(fmt_offset(0), "+0000");
        assert_eq!(fmt_offset(3600), "+0100");
        assert_eq!(fmt_offset(-5 * 3600), "-0500");
        assert_eq!(fmt_offset(5 * 3600 + 30 * 60), "+0530");
        assert_eq!(fmt_offset(-(3 * 3600 + 30 * 60)), "-0330");
    }

    #[test]
    fn nth_weekday_classifies_ordinals_and_last() {
        // 2nd Sunday of March 2026 = the 8th.
        assert_eq!(
            nth_weekday(NaiveDate::from_ymd_opt(2026, 3, 8).unwrap()),
            (2, "SU")
        );
        // 1st Sunday of November 2026 = the 1st.
        assert_eq!(
            nth_weekday(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()),
            (1, "SU")
        );
        // Last Sunday of March 2026 = the 29th → last, not 5th.
        assert_eq!(
            nth_weekday(NaiveDate::from_ymd_opt(2026, 3, 29).unwrap()),
            (-1, "SU")
        );
        // Last Sunday of October 2026 = the 25th (a 31-day month) → last.
        assert_eq!(
            nth_weekday(NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()),
            (-1, "SU")
        );
    }
}
