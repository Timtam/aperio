//! RFC 5545 `RRULE` ⇄ structured [`TaskRecurrence`] conversion.
//!
//! Adapters that speak iCalendar (CalDAV VTODO) — or, through the `RRULE`
//! intermediate, EWS — convert Aperio's structured task recurrence to and
//! from an `RRULE` string here, so the (lossy) mapping lives in one tested
//! place rather than being re-derived per adapter.
//!
//! Scope is the axes the task model and its editor actually carry: `FREQ`,
//! `INTERVAL`, `BYDAY` (the weekly weekday picker), `BYMONTHDAY` (a monthly
//! day-of-month) and the `COUNT` / `UNTIL` end modes. Relative patterns
//! (`BYDAY=2MO`, "third Wednesday") and other RRULE parts (`BYSETPOS`,
//! `BYHOUR`, `BYMONTH`, …) have no slot in the structured model and are
//! dropped on read.

use chrono::NaiveDate;

use crate::types::{RecurrenceEnd, RecurrenceFrequency, TaskRecurrence, Weekday};

fn freq_token(f: RecurrenceFrequency) -> &'static str {
    match f {
        RecurrenceFrequency::Daily => "DAILY",
        RecurrenceFrequency::Weekly => "WEEKLY",
        RecurrenceFrequency::Monthly => "MONTHLY",
        RecurrenceFrequency::Yearly => "YEARLY",
    }
}

fn weekday_token(w: Weekday) -> &'static str {
    match w {
        Weekday::Monday => "MO",
        Weekday::Tuesday => "TU",
        Weekday::Wednesday => "WE",
        Weekday::Thursday => "TH",
        Weekday::Friday => "FR",
        Weekday::Saturday => "SA",
        Weekday::Sunday => "SU",
    }
}

/// Parse one `BYDAY` token, tolerating (and discarding) an ordinal prefix
/// like `2MO` / `-1FR` — relative weekdays aren't representable, so we keep
/// only the weekday itself (the last two letters).
fn parse_weekday(token: &str) -> Option<Weekday> {
    let t = token.trim().to_ascii_uppercase();
    match t.get(t.len().saturating_sub(2)..)? {
        "MO" => Some(Weekday::Monday),
        "TU" => Some(Weekday::Tuesday),
        "WE" => Some(Weekday::Wednesday),
        "TH" => Some(Weekday::Thursday),
        "FR" => Some(Weekday::Friday),
        "SA" => Some(Weekday::Saturday),
        "SU" => Some(Weekday::Sunday),
        _ => None,
    }
}

/// Serialize a [`TaskRecurrence`] into an RFC 5545 `RRULE` value (without
/// the `RRULE:` prefix), e.g. `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE`.
pub fn task_recurrence_to_rrule(rec: &TaskRecurrence) -> String {
    let mut parts = vec![format!("FREQ={}", freq_token(rec.frequency))];
    if rec.interval > 1 {
        parts.push(format!("INTERVAL={}", rec.interval));
    }
    // A weekday list only carries meaning for the weekly frequency (the
    // model has no relative monthly/yearly weekday).
    if rec.frequency == RecurrenceFrequency::Weekly {
        if let Some(days) = rec.day_of_week.as_ref().filter(|d| !d.is_empty()) {
            let byday = days
                .iter()
                .map(|d| weekday_token(*d))
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!("BYDAY={byday}"));
        }
    }
    if let Some(dom) = rec.day_of_month {
        if matches!(
            rec.frequency,
            RecurrenceFrequency::Monthly | RecurrenceFrequency::Yearly
        ) {
            parts.push(format!("BYMONTHDAY={dom}"));
        }
    }
    match &rec.end {
        Some(RecurrenceEnd::After { occurrences }) => parts.push(format!("COUNT={occurrences}")),
        Some(RecurrenceEnd::OnDate { date }) => {
            parts.push(format!("UNTIL={}", date.format("%Y%m%d")));
        }
        Some(RecurrenceEnd::Never) | None => {}
    }
    parts.join(";")
}

/// Parse an RFC 5545 `RRULE` value into a [`TaskRecurrence`]. Tolerates a
/// leading `RRULE:` and surrounding whitespace. Returns `None` when there's
/// no usable `FREQ`. Unmodelled parts are ignored.
pub fn rrule_to_task_recurrence(rrule: &str) -> Option<TaskRecurrence> {
    let trimmed = rrule.trim();
    let body = trimmed.strip_prefix("RRULE:").unwrap_or(trimmed);

    let mut frequency: Option<RecurrenceFrequency> = None;
    let mut interval: u32 = 1;
    let mut day_of_week: Option<Vec<Weekday>> = None;
    let mut day_of_month: Option<u8> = None;
    let mut end: Option<RecurrenceEnd> = None;

    for part in body.split(';') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("").trim().to_ascii_uppercase();
        let val = kv.next().unwrap_or("").trim();
        match key.as_str() {
            "FREQ" => {
                frequency = match val.to_ascii_uppercase().as_str() {
                    "DAILY" => Some(RecurrenceFrequency::Daily),
                    "WEEKLY" => Some(RecurrenceFrequency::Weekly),
                    "MONTHLY" => Some(RecurrenceFrequency::Monthly),
                    "YEARLY" => Some(RecurrenceFrequency::Yearly),
                    _ => None,
                };
            }
            "INTERVAL" => {
                if let Ok(n) = val.parse::<u32>() {
                    interval = n.max(1);
                }
            }
            "BYDAY" => {
                let days: Vec<Weekday> = val.split(',').filter_map(parse_weekday).collect();
                if !days.is_empty() {
                    day_of_week = Some(days);
                }
            }
            "BYMONTHDAY" => {
                if let Some(d) = val
                    .split(',')
                    .next()
                    .and_then(|s| s.trim().parse::<u8>().ok())
                    .filter(|d| (1..=31).contains(d))
                {
                    day_of_month = Some(d);
                }
            }
            "COUNT" => {
                if let Ok(n) = val.parse::<u32>() {
                    end = Some(RecurrenceEnd::After { occurrences: n });
                }
            }
            "UNTIL" => {
                if let Some(date) = parse_until(val) {
                    end = Some(RecurrenceEnd::OnDate { date });
                }
            }
            _ => {} // BYSETPOS, BYHOUR, BYMONTH, WKST, … — not modelled.
        }
    }

    let frequency = frequency?;
    // Drop a weekday list on non-weekly frequencies (a server might carry a
    // relative `BYDAY=2MO` on a monthly rule, which we can't represent).
    if frequency != RecurrenceFrequency::Weekly {
        day_of_week = None;
    }
    Some(TaskRecurrence {
        frequency,
        interval,
        day_of_week,
        day_of_month,
        end,
    })
}

/// Parse an `UNTIL` value — accepts the DATE form (`YYYYMMDD`) and the
/// DATE-TIME form (`YYYYMMDDTHHMMSSZ`), keeping the date.
fn parse_until(val: &str) -> Option<NaiveDate> {
    let v = val.trim();
    let date_part = v.split('T').next().unwrap_or(v);
    NaiveDate::parse_from_str(date_part, "%Y%m%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        frequency: RecurrenceFrequency,
        interval: u32,
        day_of_week: Option<Vec<Weekday>>,
        day_of_month: Option<u8>,
        end: Option<RecurrenceEnd>,
    ) -> TaskRecurrence {
        TaskRecurrence {
            frequency,
            interval,
            day_of_week,
            day_of_month,
            end,
        }
    }

    #[test]
    fn serializes_the_modelled_axes() {
        assert_eq!(
            task_recurrence_to_rrule(&rule(RecurrenceFrequency::Daily, 1, None, None, None)),
            "FREQ=DAILY",
        );
        assert_eq!(
            task_recurrence_to_rrule(&rule(
                RecurrenceFrequency::Weekly,
                2,
                Some(vec![Weekday::Monday, Weekday::Wednesday]),
                None,
                None,
            )),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE",
        );
        assert_eq!(
            task_recurrence_to_rrule(&rule(
                RecurrenceFrequency::Monthly,
                1,
                None,
                Some(15),
                Some(RecurrenceEnd::OnDate {
                    date: NaiveDate::from_ymd_opt(2026, 6, 30).unwrap(),
                }),
            )),
            "FREQ=MONTHLY;BYMONTHDAY=15;UNTIL=20260630",
        );
        assert_eq!(
            task_recurrence_to_rrule(&rule(
                RecurrenceFrequency::Daily,
                3,
                None,
                None,
                Some(RecurrenceEnd::After { occurrences: 10 }),
            )),
            "FREQ=DAILY;INTERVAL=3;COUNT=10",
        );
    }

    #[test]
    fn round_trips_through_rrule() {
        let cases = [
            rule(RecurrenceFrequency::Daily, 1, None, None, None),
            rule(
                RecurrenceFrequency::Weekly,
                2,
                Some(vec![Weekday::Monday, Weekday::Friday]),
                None,
                Some(RecurrenceEnd::After { occurrences: 5 }),
            ),
            rule(
                RecurrenceFrequency::Monthly,
                1,
                None,
                Some(15),
                Some(RecurrenceEnd::OnDate {
                    date: NaiveDate::from_ymd_opt(2027, 1, 1).unwrap(),
                }),
            ),
            rule(RecurrenceFrequency::Yearly, 1, None, None, None),
        ];
        for original in cases {
            let back = rrule_to_task_recurrence(&task_recurrence_to_rrule(&original));
            assert_eq!(back.as_ref(), Some(&original));
        }
    }

    #[test]
    fn parses_tolerantly_and_drops_unmodelled_parts() {
        // RRULE: prefix, lowercase keys, an ordinal BYDAY on a monthly rule
        // (relative — dropped), and an unknown token.
        let parsed =
            rrule_to_task_recurrence("RRULE:freq=MONTHLY;byday=2MO;bysetpos=1;bymonthday=10")
                .expect("has FREQ");
        assert_eq!(parsed.frequency, RecurrenceFrequency::Monthly);
        assert_eq!(parsed.day_of_week, None); // relative weekday dropped
        assert_eq!(parsed.day_of_month, Some(10));

        // A DATE-TIME UNTIL keeps the date.
        let until = rrule_to_task_recurrence("FREQ=DAILY;UNTIL=20260630T235959Z").unwrap();
        assert_eq!(
            until.end,
            Some(RecurrenceEnd::OnDate {
                date: NaiveDate::from_ymd_opt(2026, 6, 30).unwrap(),
            }),
        );

        // No FREQ → not a usable recurrence.
        assert!(rrule_to_task_recurrence("INTERVAL=2").is_none());
    }
}
