//! Apple's silent alarm: how an iPhone tells itself not to add the account's
//! default alert to an appointment.
//!
//! Measured on 2026-11-08 with Toni's iPhone (iOS 27.2) and iCloud, by raw
//! PUTs (`icloud-reminders marker` in the local live-test tools):
//!
//! - The iPhone writes, into every appointment it makes, beside the alert the
//!   user chose, a VALARM with `ACTION:NONE` and an absolute trigger at
//!   1976-04-01 00:55:45 UTC — RFC 9074 §8's "commonly used" value for a
//!   trigger to be ignored. No UID, no X-APPLE property.
//! - It adds its default alert ("Default Alert Times") to every appointment it
//!   receives WITHOUT that alarm, however many alarms the appointment already
//!   has. The extra alert lives on the phone only; iCloud serves the same body
//!   to every client. With the silent alarm there is none, for timed and
//!   all-day appointments alike; with only the silent alarm the iPhone shows
//!   "None". Writing it into an appointment that lacked it removes the default
//!   the phone had already added.
//! - Rewritten as an alarm that SHOWS (`ACTION:DISPLAY`, same trigger), which
//!   is what Aperio's saves did before, the iPhone shows it as an alert "at
//!   time of event" and adds its default besides.
//!
//! An alarm Apple flags `X-APPLE-DEFAULT-ALARM:TRUE` is that default itself,
//! kept on the server; while one stands in an appointment, the silent alarm
//! has no business beside it.
//!
//! The read leaves every alarm at this trigger out (`mapping::parse_valarms`),
//! a rewrite restores the damaged form ([`restored`]), and the CalDAV adapter
//! writes the alarm into what it sends to iCloud ([`with_silent_alarm`]).

use crate::ical_raw::{component_lines, component_ranges, insert_before_end, line_ending};

/// The ignored trigger instant, as the iPhone spells it.
pub const IGNORED_TRIGGER: &str = "19760401T005545Z";

/// Properties that make an alarm at the ignored trigger a real one: it fires
/// on a location, not at a time (RFC 9074 §8, RFC 9073; Apple's own names).
/// Apple does not count those as silent either.
const LOCATION_PROPERTIES: [&str; 4] = [
    "PROXIMITY",
    "STRUCTURED-LOCATION",
    "X-APPLE-PROXIMITY",
    "X-APPLE-STRUCTURED-LOCATION",
];

/// The silent alarm exactly as the iPhone writes it, in `ending`.
pub fn silent_alarm(ending: &str) -> String {
    format!(
        "BEGIN:VALARM{ending}ACTION:NONE{ending}TRIGGER;VALUE=DATE-TIME:{IGNORED_TRIGGER}{ending}END:VALARM{ending}"
    )
}

/// Whether a TRIGGER value is the ignored instant. Such an alarm never fires
/// at a time, so it is never a reminder Aperio shows.
pub fn is_ignored_trigger(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case(IGNORED_TRIGGER)
}

/// What one VALARM's text is, as far as the silent alarm goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `ACTION:NONE`: the silent alarm.
    Silent,
    /// The ignored trigger on an alarm that would show, and no location: a
    /// silent alarm somebody rewrote — Aperio's saves, until 2026-11.
    Damaged,
    /// Apple's flagged default alert.
    AppleDefault,
    Other,
}

fn kind_of(alarm: &str) -> Kind {
    let lines = component_lines(alarm);
    let value_of = |name: &str| {
        lines
            .iter()
            .find(|line| line.name == name)
            .map(|line| line.value.trim().to_ascii_uppercase())
    };
    let action = value_of("ACTION").unwrap_or_default();
    if action == "NONE" {
        return Kind::Silent;
    }
    if value_of("X-APPLE-DEFAULT-ALARM").as_deref() == Some("TRUE") {
        return Kind::AppleDefault;
    }
    let ignored = lines
        .iter()
        .any(|line| line.name == "TRIGGER" && is_ignored_trigger(&line.value));
    let located = lines
        .iter()
        .any(|line| LOCATION_PROPERTIES.contains(&line.name.as_str()));
    if ignored && !located {
        Kind::Damaged
    } else {
        Kind::Other
    }
}

/// `alarm` (one VALARM's text), with a rewritten silent alarm put back as the
/// iPhone wrote it, in the alarm's own line ending. Anything else unchanged.
pub fn restored(alarm: &str) -> String {
    if kind_of(alarm) == Kind::Damaged {
        silent_alarm(line_ending(alarm))
    } else {
        alarm.to_string()
    }
}

/// `body` (a VCALENDAR) as Aperio sends it to iCloud: in every VEVENT, a
/// rewritten silent alarm put back, a second silent alarm dropped, and the
/// silent alarm added where neither it nor Apple's flagged default stands —
/// so the iPhone shows exactly the alerts the appointment has, and adds none
/// of its own. Everything else stays as it is, byte for byte.
pub fn with_silent_alarm(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 128);
    let mut from = 0;
    for range in component_ranges(body, "VEVENT") {
        out.push_str(&body[from..range.start]);
        out.push_str(&event_with_silent_alarm(&body[range.clone()]));
        from = range.end;
    }
    out.push_str(&body[from..]);
    out
}

fn event_with_silent_alarm(vevent: &str) -> String {
    let mut out = String::with_capacity(vevent.len() + 96);
    let mut from = 0;
    let mut silent = false;
    let mut apple_default = false;
    for range in component_ranges(vevent, "VALARM") {
        out.push_str(&vevent[from..range.start]);
        let alarm = &vevent[range.clone()];
        match kind_of(alarm) {
            Kind::Silent | Kind::Damaged if silent => {}
            Kind::Silent | Kind::Damaged => {
                silent = true;
                out.push_str(&restored(alarm));
            }
            Kind::AppleDefault => {
                apple_default = true;
                out.push_str(alarm);
            }
            Kind::Other => out.push_str(alarm),
        }
        from = range.end;
    }
    out.push_str(&vevent[from..]);
    if silent || apple_default {
        out
    } else {
        insert_before_end(&out, &silent_alarm(line_ending(vevent)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APERIO_ALARM: &str =
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Zahnarzt\r\nTRIGGER:-PT7M\r\nEND:VALARM\r\n";
    const DAMAGED: &str = "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Zahnarzt\r\nTRIGGER;VALUE=DATE-TIME:19760401T005545Z\r\nEND:VALARM\r\n";

    fn vcal(events: &[&str]) -> String {
        let mut body = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n");
        for (index, alarms) in events.iter().enumerate() {
            let rid = if index == 0 {
                String::new()
            } else {
                format!("RECURRENCE-ID:2026121{index}T090000Z\r\n")
            };
            body.push_str(&format!(
                "BEGIN:VEVENT\r\nUID:u\r\n{rid}DTSTART:20261214T090000Z\r\nSUMMARY:Zahnarzt\r\n{alarms}END:VEVENT\r\n"
            ));
        }
        body.push_str("END:VCALENDAR\r\n");
        body
    }

    #[test]
    fn the_silent_alarm_is_the_iphones_own_text() {
        assert_eq!(
            silent_alarm("\r\n"),
            "BEGIN:VALARM\r\nACTION:NONE\r\nTRIGGER;VALUE=DATE-TIME:19760401T005545Z\r\nEND:VALARM\r\n"
        );
    }

    #[test]
    fn every_vevent_gets_the_silent_alarm_after_its_own() {
        let body = vcal(&[APERIO_ALARM, ""]);
        let out = with_silent_alarm(&body);
        assert_eq!(out.matches("ACTION:NONE").count(), 2, "{out}");
        // Inside each VEVENT, after the alarm that was there.
        assert!(
            out.contains(&format!("{APERIO_ALARM}{}END:VEVENT", silent_alarm("\r\n"))),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "SUMMARY:Zahnarzt\r\n{}END:VEVENT",
                silent_alarm("\r\n")
            )),
            "{out}"
        );
        // Nothing else moved.
        assert_eq!(out.replace(&silent_alarm("\r\n"), ""), body);
    }

    #[test]
    fn a_body_that_has_it_stays_as_it_is() {
        let body = vcal(&[&format!("{APERIO_ALARM}{}", silent_alarm("\r\n"))]);
        assert_eq!(with_silent_alarm(&body), body);
    }

    #[test]
    fn apples_flagged_default_needs_no_silent_alarm_beside_it() {
        let flagged = "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT1H\r\nX-APPLE-DEFAULT-ALARM:TRUE\r\nEND:VALARM\r\n";
        let body = vcal(&[flagged]);
        assert_eq!(with_silent_alarm(&body), body);
    }

    #[test]
    fn a_rewritten_silent_alarm_is_put_back_and_never_doubled() {
        let body = vcal(&[&format!("{APERIO_ALARM}{DAMAGED}")]);
        let out = with_silent_alarm(&body);
        assert!(
            !out.contains("ACTION:DISPLAY\r\nDESCRIPTION:Zahnarzt\r\nTRIGGER;VALUE"),
            "{out}"
        );
        assert_eq!(out.matches("ACTION:NONE").count(), 1, "{out}");

        let both = vcal(&[&format!("{}{DAMAGED}", silent_alarm("\r\n"))]);
        let out = with_silent_alarm(&both);
        assert_eq!(out.matches("BEGIN:VALARM").count(), 1, "{out}");
    }

    #[test]
    fn a_location_alarm_at_the_ignored_trigger_is_a_real_one() {
        // RFC 9074 §8's own example: the ignored trigger on an alarm that
        // fires when the user leaves a place.
        let located = "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=DATE-TIME:19760401T005545Z\r\nPROXIMITY:DEPART\r\nEND:VALARM\r\n";
        assert_eq!(restored(located), located);
        let out = with_silent_alarm(&vcal(&[located]));
        assert!(out.contains(located), "{out}");
        assert_eq!(out.matches("ACTION:NONE").count(), 1, "{out}");
    }

    #[test]
    fn a_bare_lf_body_gets_a_bare_lf_alarm() {
        let body = vcal(&[APERIO_ALARM]).replace("\r\n", "\n");
        let out = with_silent_alarm(&body);
        assert!(out.contains(&silent_alarm("\n")), "{out}");
        assert!(!out.contains('\r'), "{out:?}");
        assert_eq!(restored(&DAMAGED.replace("\r\n", "\n")), silent_alarm("\n"));
    }
}
