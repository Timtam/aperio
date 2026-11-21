//! Minimal `VTIMEZONE` generation for zoned events written to CalDAV.
//!
//! RFC 5545 §3.6.5 requires that any `TZID` a `VEVENT` references is DEFINED by a
//! matching `VTIMEZONE` component in the same `VCALENDAR`. Aperio previously wrote
//! `DTSTART;TZID=<zone>` WITHOUT the `VTIMEZONE` — technically invalid. iCloud
//! tolerates that for open-ended rules but drops the recurrence of a zoned
//! recurring event with a `COUNT` bound (it can't resolve the zone to compute the
//! Nth occurrence), so a "repeat 2×" event silently lost its rule.
//!
//! We emit a `VTIMEZONE` derived from the zone's ACTUAL transitions — not a
//! hand-rolled table — as cal_core::Zone reads them from the tzdata bundled
//! with jiff-tzdb: every change of the total offset, with tzdata's
//! daylight-saving flag and abbreviation for it. A zone whose onsets follow a
//! fixed yearly Gregorian rule (Berlin, New York, Sydney, …) gets the compact
//! DAYLIGHT + STANDARD yearly RRULEs Apple/Thunderbird emit. A zone whose
//! onsets DON'T (Ramadan-tracking Asia/Gaza, whose transitions drift ~11
//! days/year; America/Santiago's varying week; Africa/Casablanca, which tracked
//! Ramadan until it stayed on +00 from 2026-09-20) gets each real transition
//! emitted explicitly across the event's window — a single yearly rule would
//! place the onset weeks off and shift events by an hour on strict clients.
//! Zones without DST get a single fixed STANDARD.
//!
//! The bundled data is tzdata's rearguard form, so the labels are the
//! conventional ones: Europe/Dublin's winter GMT is its STANDARD and its summer
//! IST its DAYLIGHT. Numeric abbreviations are written as tzdata writes them
//! (`TZNAME:+04`).

use cal_core::Zone;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};

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

/// The zone a `TZID` is written with, or `None` where none may be: "UTC" in
/// any case (a bare-UTC value needs no definition), a name that is not
/// tzdata's own in exact case, or an event so far out that the definition's
/// years (the one before the event to [`IRREGULAR_HORIZON_YEARS`] after it,
/// and the next) leave the years the bundled data covers. ONE predicate for
/// `zoned_datetime` (mapping.rs) and [`vtimezone_for`]: a `TZID` written
/// without its `VTIMEZONE` breaks RFC 5545 §3.6.5, and iCloud then drops a
/// `COUNT` rule.
pub(crate) fn writable_zone(tzid: &str, ref_year: i32) -> Option<Zone> {
    if tzid.eq_ignore_ascii_case("UTC") {
        return None;
    }
    let years = cal_core::COVERED_YEARS;
    let first = ref_year.checked_sub(1)?;
    let last = ref_year.checked_add(IRREGULAR_HORIZON_YEARS + 1)?;
    if !years.contains(&first) || !years.contains(&last) {
        return None;
    }
    Zone::named(tzid)
}

/// A `VTIMEZONE` block (CRLF-terminated iCalendar lines, no trailing CRLF) for
/// `tzid`, or `None` where [`writable_zone`] refuses it (a bare-UTC `DTSTART`
/// needs none). `ref_year` is the event's start year.
pub fn vtimezone_for(tzid: &str, ref_year: i32) -> Option<String> {
    let tz = writable_zone(tzid, ref_year)?;

    // Anchor the observance onsets to the year BEFORE the event. A VTIMEZONE
    // observance applies from its `DTSTART` onward, so a strict client (iCloud)
    // can only resolve an instant that has an onset at or before it. Anchoring
    // to the event's OWN year would leave a Jan–March event (before that year's
    // spring transition) with no preceding onset — the same unresolvable-zone
    // failure we're fixing. The prior year's onsets precede every instant in the
    // event's year, and (for a fixed-rule zone) the yearly RRULE still extends
    // forward. (Apple anchors Europe/Berlin at 19810329 for the same reason.)
    let base_year = ref_year - 1;
    let transitions = year_transitions(tz, base_year)?;

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
            if rule_is_stable(tz, base_year) {
                out.push_str(&sub_component("DAYLIGHT", dst_t));
                out.push_str(&sub_component("STANDARD", std_t));
            } else {
                out.push_str(&explicit_observances(
                    tz,
                    base_year,
                    ref_year + IRREGULAR_HORIZON_YEARS,
                ));
            }
        }
        // The event's own year changes the clock, though the year before had
        // no pair of changes: no single offset is right, so every real
        // transition is written.
        _ if !year_transitions(tz, ref_year)?.is_empty() => {
            out.push_str(&explicit_observances(
                tz,
                base_year,
                ref_year + IRREGULAR_HORIZON_YEARS,
            ));
        }
        // No clock change in the event's year → a single fixed STANDARD, read
        // in the event's OWN year: the year before may still have changed the
        // clock once (America/Vancouver into its last daylight saving in
        // March 2026), and its midyear reading would then name the wrong
        // offset or abbreviation for the event.
        _ => {
            let info = tz.offset_info(midyear(ref_year)?);
            let off = info.offset.local_minus_utc();
            let name = info.abbreviation;
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
fn explicit_observances(tz: Zone, from_year: i32, to_year: i32) -> String {
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
/// zones whose onsets drift (Ramadan-tracking Asia/Gaza, Africa/Casablanca
/// until 2026),
/// vary by a week (America/Santiago), or key off a fixed day-of-month (whose
/// weekday ordinal moves year to year) — all of which then take the explicit
/// path.
fn rule_is_stable(tz: Zone, base_year: i32) -> bool {
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

/// Every change of the total offset inside `year` (UTC), earliest first: the
/// window is (Jan 1 00:00Z, next Jan 1 00:00Z]. A transition that changes only
/// the abbreviation or the daylight-saving flag is no clock change and is not
/// listed (cal_core::Zone::offset_changes).
fn year_transitions(tz: Zone, year: i32) -> Option<Vec<Transition>> {
    let start = utc_midnight(year)?;
    let end = utc_midnight(year.checked_add(1)?)?;
    Some(
        tz.offset_changes(start)
            .take_while(|c| c.at <= end)
            .map(|c| Transition {
                at_utc: c.at.naive_utc(),
                from_total: c.from.local_minus_utc(),
                to_total: c.to.local_minus_utc(),
                to_is_dst: c.dst,
                to_name: c.abbreviation,
            })
            .collect(),
    )
}

fn utc_midnight(year: i32) -> Option<DateTime<Utc>> {
    Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single()
}

fn midyear(year: i32) -> Option<DateTime<Utc>> {
    Utc.with_ymd_and_hms(year, 7, 1, 0, 0, 0).single()
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
        assert!(rule_is_stable("Europe/Berlin".parse().unwrap(), 2025));
        assert!(rule_is_stable("America/New_York".parse().unwrap(), 2025));
        assert!(rule_is_stable("Australia/Sydney".parse().unwrap(), 2025));
    }

    #[test]
    fn ramadan_zone_uses_explicit_observances_not_a_yearly_rule() {
        // Africa/Casablanca's clock tracked Ramadan until 2026 (onsets drift ~11
        // days/year) and stays on +00 from 2026-09-20, so a single
        // FREQ=YEARLY;BYDAY rule would place the transition weeks off and shift
        // events by an hour. It must be detected as unstable and emitted as
        // explicit per-transition observances (no RRULE) across the event window.
        let tz: Zone = "Africa/Casablanca".parse().unwrap();
        assert!(
            !rule_is_stable(tz, 2025),
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
    fn dublins_daylight_is_its_summer_from_rearguard_data() {
        // tzdata's main form calls Ireland's winter GMT the daylight-saving
        // time (a negative save); the bundled rearguard data calls it
        // standard. Offsets and instants are the same either way.
        let vtz = vtimezone_for("Europe/Dublin", 2026).expect("Dublin resolves");
        let daylight = sub(&vtz, "DAYLIGHT").expect("has DAYLIGHT");
        assert_eq!(line(daylight, "TZOFFSETFROM:"), Some("+0000"));
        assert_eq!(line(daylight, "TZOFFSETTO:"), Some("+0100"));
        assert_eq!(line(daylight, "TZNAME:"), Some("IST"));
        assert_eq!(
            line(daylight, "RRULE:"),
            Some("FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU")
        );
        let standard = sub(&vtz, "STANDARD").expect("has STANDARD");
        assert_eq!(line(standard, "TZOFFSETTO:"), Some("+0000"));
        assert_eq!(line(standard, "TZNAME:"), Some("GMT"));
        assert_eq!(
            line(standard, "RRULE:"),
            Some("FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU")
        );
    }

    #[test]
    fn a_numeric_abbreviation_is_written_as_tzdata_writes_it() {
        // chrono-tz had no abbreviation for Dubai and an empty TZNAME was
        // written; the bundled data says +04.
        let vtz = vtimezone_for("Asia/Dubai", 2026).expect("Dubai resolves");
        let std = sub(&vtz, "STANDARD").expect("has STANDARD");
        assert_eq!(line(std, "TZOFFSETTO:"), Some("+0400"));
        assert_eq!(line(std, "TZNAME:"), Some("+04"));
        assert!(vtz.lines().all(|l| l.trim_end() != "TZNAME:"), "{vtz}");
    }

    #[test]
    fn vancouver_after_its_last_daylight_saving_reads_its_own_year() {
        // tzdata 2026b: British Columbia sprang forward on 2026-03-08 and stays
        // on -07 (MST) from 2026-11-01. The year before a 2027 event changed
        // the clock once, into PDT; the definition is read in 2027 itself.
        let vtz = vtimezone_for("America/Vancouver", 2027).expect("Vancouver resolves");
        assert!(sub(&vtz, "DAYLIGHT").is_none(), "{vtz}");
        let std = sub(&vtz, "STANDARD").expect("has STANDARD");
        assert_eq!(line(std, "TZOFFSETFROM:"), Some("-0700"));
        assert_eq!(line(std, "TZOFFSETTO:"), Some("-0700"));
        assert_eq!(line(std, "TZNAME:"), Some("MST"));
        assert_eq!(vtz.matches("BEGIN:STANDARD").count(), 1, "{vtz}");
        // A 2026 event still sees 2026's spring change.
        let vtz = vtimezone_for("America/Vancouver", 2026).expect("Vancouver resolves");
        assert!(vtz.contains("TZOFFSETTO:-0700"), "{vtz}");
        assert!(vtz.contains("TZOFFSETTO:-0800"), "{vtz}");
        assert!(!vtz.contains("RRULE"), "{vtz}");
    }

    #[test]
    fn a_zone_is_written_with_its_definition_or_not_at_all() {
        // One predicate decides both: a TZID never goes out without its
        // VTIMEZONE, at the edge of the years the bundled data covers too.
        let (first, last) = (
            *cal_core::COVERED_YEARS.start(),
            *cal_core::COVERED_YEARS.end(),
        );
        for year in [
            first - 1,
            first,
            first + 1,
            2026,
            last - 11,
            last - 10,
            last,
        ] {
            for tzid in [
                "Europe/Berlin",
                "Etc/UTC",
                "UTC",
                "europe/berlin",
                "Factory",
            ] {
                assert_eq!(
                    writable_zone(tzid, year).is_some(),
                    vtimezone_for(tzid, year).is_some(),
                    "{tzid} {year}"
                );
            }
        }
        assert!(writable_zone("Europe/Berlin", last - 11).is_some());
        assert!(writable_zone("Europe/Berlin", last - 10).is_none());
        assert!(writable_zone("Europe/Berlin", first + 1).is_some());
        assert!(writable_zone("Europe/Berlin", first).is_none());
        assert!(writable_zone("UTC", 2026).is_none());
        assert!(writable_zone("Etc/UTC", 2026).is_some());
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
