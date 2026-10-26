//! An `.ics` file read for an import (DESIGN-file-import.md).
//!
//! The same reader as the feeds and CalDAV (`ical_mapping::mapping`), so a
//! file reads the way a server's copy of the same event does: zones (tzdata,
//! Windows ids, the file's own `VTIMEZONE`), all-day dates, rules and their
//! deletions, alarms, the organizer and the invitees. On top of it, what only
//! an import has to decide:
//!
//! - A changed occurrence of a series becomes an appointment of its own, and
//!   the series gets a gap where it was (Toni, 2026-10-25). It has no UID: the
//!   series' UID names the series, and a second resource under it would be
//!   read as the same appointment.
//! - A cancelled appointment (`STATUS:CANCELLED`, or a file whose `METHOD` is
//!   `CANCEL`) adds nothing and is named as left out; a cancelled occurrence
//!   is only the gap it leaves.
//! - An answer to an invitation (`METHOD:REPLY`, `COUNTER`) adds nothing.
//! - Tasks, notes and busy times are left out, each named.

use std::collections::HashMap;

use cal_core::{Event, FileImport, ImportedEvent, SkippedKind, SkippedPart};
use chrono::{DateTime, Utc};
use ical_mapping::mapping;

/// Read `content`, the bytes of a calendar file. The `Err` is a sentence for
/// a file that is no calendar at all.
pub fn import_ics(content: &[u8]) -> Result<FileImport, String> {
    let content = content.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(content);
    let text = String::from_utf8_lossy(content);
    if !text.to_ascii_uppercase().contains("BEGIN:VCALENDAR") {
        return Err("the file holds no calendar (no BEGIN:VCALENDAR)".to_string());
    }
    let file = mapping::read_calendar_file(&text)
        .map_err(|err| format!("the calendar in the file could not be read: {err}"))?;

    let mut skipped: Vec<SkippedPart> = Vec::new();
    let answers = matches!(
        file.method.as_deref(),
        Some("REPLY" | "COUNTER" | "DECLINECOUNTER" | "REFRESH")
    );
    let cancels = file.method.as_deref() == Some("CANCEL");

    // The slots each series' changed occurrences stand in for.
    let mut gaps: HashMap<String, Vec<DateTime<Utc>>> = HashMap::new();
    for event in &file.events {
        if let Ok(Some((series, slot))) = cal_core::split_override_id(&event.id) {
            gaps.entry(series.to_string()).or_default().push(slot);
        }
    }

    let mut events = Vec::new();
    for event in file.events {
        let title = (!event.title.trim().is_empty()).then(|| event.title.clone());
        if answers {
            skipped.push(SkippedPart {
                kind: SkippedKind::Reply,
                title,
            });
            continue;
        }
        match cal_core::split_override_id(&event.id) {
            Ok(Some(_)) => {
                // A changed occurrence: its own appointment, unless it was
                // cancelled, which leaves only the gap.
                if event.cancelled || cancels {
                    continue;
                }
                events.push(imported(event, None));
            }
            _ => {
                if event.cancelled || cancels {
                    skipped.push(SkippedPart {
                        kind: SkippedKind::Cancelled,
                        title,
                    });
                    continue;
                }
                let uid = event.id.clone();
                let mut event = event;
                if let (Some(recurrence), Some(slots)) = (event.recurrence.as_mut(), gaps.get(&uid))
                {
                    for slot in slots {
                        if !recurrence.exceptions.contains(slot) {
                            recurrence.exceptions.push(*slot);
                        }
                    }
                    recurrence.exceptions.sort();
                }
                events.push(imported(event, Some(uid)));
            }
        }
    }

    for title in file.unreadable {
        skipped.push(SkippedPart {
            kind: SkippedKind::Unreadable,
            title,
        });
    }
    for (kind, title) in file.others {
        let kind = match kind.as_str() {
            "VTODO" => SkippedKind::Task,
            "VJOURNAL" => SkippedKind::Journal,
            "VFREEBUSY" => SkippedKind::FreeBusy,
            _ => SkippedKind::Unreadable,
        };
        skipped.push(SkippedPart { kind, title });
    }
    Ok(FileImport { events, skipped })
}

/// The appointment the editor is filled with.
fn imported(event: Event, uid: Option<String>) -> ImportedEvent {
    ImportedEvent {
        uid,
        title: event.title,
        description: event.description.filter(|d| !d.trim().is_empty()),
        location: event.location.filter(|l| !l.trim().is_empty()),
        start: event.start,
        end: event.end,
        all_day: event.all_day,
        recurrence: event.recurrence,
        reminders: event.reminders,
        attendees: event.attendees,
        organizer: event.organizer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::{ReminderKind, SkippedKind};

    fn fixture(name: &str) -> FileImport {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        import_ics(&std::fs::read(&path).unwrap()).unwrap()
    }

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    /// An Outlook invitation: its Windows zone, folded lines, the organizer,
    /// the invitees with their names, the alarm.
    #[test]
    fn an_outlook_invitation_reads_whole() {
        let import = fixture("outlook-invitation.ics");
        assert!(import.skipped.is_empty(), "{:?}", import.skipped);
        let [event] = import.events.as_slice() else {
            panic!("one appointment, got {:?}", import.events)
        };
        assert_eq!(event.title, "Projektbesprechung");
        assert_eq!(event.start, at("2026-11-18T13:00:00Z"));
        assert_eq!(event.end, at("2026-11-18T14:00:00Z"));
        assert!(!event.all_day);
        assert_eq!(event.location.as_deref(), Some("Raum 3.14"));
        assert!(
            event
                .description
                .as_deref()
                .unwrap()
                .starts_with("Hallo zusammen,\n\nwir besprechen den Stand des Projekts."),
            "{:?}",
            event.description
        );
        assert_eq!(
            event.uid.as_deref(),
            Some(
                "040000008200E00074C5B7101A82E00800000000A0C4B1F2D8C5DC01000000000000000\
                 010000000F3A1B7C6D5E4F3A2B1C0D9E8F7A6B5C4"
            )
        );
        assert_eq!(
            event.organizer.as_deref(),
            Some("erika.mustermann@example.org")
        );
        assert_eq!(
            event.attendees,
            [
                "Max Mustermann <max.mustermann@example.org>",
                "Lea Beispiel <lea.beispiel@example.org>",
            ]
        );
        assert_eq!(event.reminders.len(), 1);
        assert!(
            matches!(
                event.reminders[0].kind,
                ReminderKind::Relative { minutes_before: 15 }
            ),
            "{:?}",
            event.reminders
        );
    }

    /// A Teams series: deleted Monday, a Monday moved to Tuesday and a
    /// cancelled one. The moved one is an appointment of its own without the
    /// series' UID, and the series has a gap at all three.
    #[test]
    fn a_series_keeps_its_gaps_and_its_moved_occurrence_stands_alone() {
        let import = fixture("teams-series.ics");
        assert!(import.skipped.is_empty(), "{:?}", import.skipped);
        assert_eq!(import.events.len(), 2, "{:?}", import.events);
        let series = &import.events[0];
        assert_eq!(series.uid.as_deref(), Some("teams-series-0001@example.org"));
        assert_eq!(series.start, at("2026-11-02T09:00:00Z"));
        let recurrence = series.recurrence.as_ref().unwrap();
        assert_eq!(recurrence.tzid.as_deref(), Some("Europe/Berlin"));
        assert_eq!(
            recurrence.exceptions,
            [
                at("2026-11-09T09:00:00Z"),
                at("2026-11-16T09:00:00Z"),
                at("2026-11-23T09:00:00Z"),
            ]
        );
        let moved = &import.events[1];
        assert_eq!(moved.title, "Jour fixe (verschoben)");
        assert_eq!(moved.uid, None);
        assert!(moved.recurrence.is_none());
        assert_eq!(moved.start, at("2026-11-17T13:30:00Z"));
    }

    /// A Google invitation: UTC times, the organizer's own row is no invitee,
    /// an empty location is none.
    #[test]
    fn a_google_invitation_names_its_invitees_without_the_organizer() {
        let import = fixture("google-invitation.ics");
        let [event] = import.events.as_slice() else {
            panic!("one appointment, got {:?}", import.events)
        };
        assert_eq!(event.title, "Abstimmung Budget");
        assert_eq!(event.start, at("2026-11-20T09:00:00Z"));
        assert_eq!(event.organizer.as_deref(), Some("organizer@example.com"));
        assert_eq!(event.attendees, ["guest@example.com"]);
        assert_eq!(event.location, None);
        assert_eq!(
            event.description.as_deref(),
            Some("Bitte vorher die Unterlagen lesen.")
        );
    }

    /// An Apple export: an all-day stay, a zoned weekly series, and a task and
    /// a note that are left out by name.
    #[test]
    fn an_export_reads_its_appointments_and_names_what_it_leaves_out() {
        let import = fixture("apple-export.ics");
        assert_eq!(import.events.len(), 2, "{:?}", import.events);
        let stay = &import.events[0];
        assert_eq!(stay.title, "Weihnachten bei den Eltern");
        assert!(stay.all_day);
        assert_eq!((stay.end - stay.start).num_days(), 3);
        let choir = &import.events[1];
        assert_eq!(choir.start, at("2026-07-01T16:30:00Z"));
        assert_eq!(
            choir.location.as_deref(),
            Some("Gemeindehaus, Kirchstraße 1")
        );
        assert_eq!(
            choir.recurrence.as_ref().unwrap().tzid.as_deref(),
            Some("Europe/Berlin")
        );
        assert_eq!(
            import.skipped,
            [
                SkippedPart {
                    kind: SkippedKind::Task,
                    title: Some("Geschenke besorgen".to_string())
                },
                SkippedPart {
                    kind: SkippedKind::Journal,
                    title: Some("Notiz zum Fest".to_string())
                },
            ]
        );
    }

    /// A cancellation and an answer add no appointment; both are named.
    #[test]
    fn a_cancellation_and_an_answer_add_nothing() {
        let cancel = fixture("cancellation.ics");
        assert!(cancel.events.is_empty());
        assert_eq!(
            cancel.skipped,
            [SkippedPart {
                kind: SkippedKind::Cancelled,
                title: Some("Abgesagt: Projektbesprechung".to_string())
            }]
        );
        let reply = fixture("reply.ics");
        assert!(reply.events.is_empty());
        assert_eq!(
            reply.skipped,
            [SkippedPart {
                kind: SkippedKind::Reply,
                title: Some("Angenommen: Abstimmung Budget".to_string())
            }]
        );
    }

    #[test]
    fn a_file_without_a_calendar_says_so() {
        let err = import_ics(b"BEGIN:VCARD\r\nFN:Max\r\nEND:VCARD\r\n").unwrap_err();
        assert!(err.contains("no calendar"), "{err}");
        // A byte-order mark in front is no reason to refuse.
        let with_bom = b"\xEF\xBB\xBFBEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
            BEGIN:VEVENT\r\nUID:bom\r\nSUMMARY:Probe\r\n\
            DTSTART:20261116T090000Z\r\nDTEND:20261116T100000Z\r\n\
            END:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(import_ics(with_bom).unwrap().events.len(), 1);
    }
}
