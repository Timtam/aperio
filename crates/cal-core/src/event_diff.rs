//! Which fields an edit changed against a copy of the same event.
//!
//! An adapter that holds the provider's current copy can tell whether a save
//! would change anything there at all. A calendar server that mails the
//! invitees about every saved change to a meeting (RFC 6638 implicit
//! scheduling, decision 76a) must not get a write that changes nothing: a
//! colour, a sound or a private reminder lives on this device, and saving one
//! would otherwise mail every guest.
//!
//! The comparison is exact, with the one equivalence the editors produce: no
//! description (or location) and an empty one are the same. Reminders compare
//! as a set, invitees by address ([`crate::attendee::same_invitees`]).

use crate::attendee::same_invitees;
use crate::{Event, EventRecurrence, Reminder};

/// A field an edit can change and a provider stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventField {
    Title,
    Description,
    Location,
    Start,
    End,
    AllDay,
    Recurrence,
    Reminders,
    Attendees,
    ColorHex,
}

/// The fields of `edit` that differ from `before`, in the order of
/// [`EventField`]. Empty when saving `edit` over `before` changes nothing a
/// provider stores. Host-local parts (colour label, sound) are not fields.
pub fn changed_fields(edit: &Event, before: &Event) -> Vec<EventField> {
    let mut changed = Vec::new();
    if edit.title != before.title {
        changed.push(EventField::Title);
    }
    if !same_text(edit.description.as_deref(), before.description.as_deref()) {
        changed.push(EventField::Description);
    }
    if !same_text(edit.location.as_deref(), before.location.as_deref()) {
        changed.push(EventField::Location);
    }
    if edit.start != before.start {
        changed.push(EventField::Start);
    }
    if edit.end != before.end {
        changed.push(EventField::End);
    }
    if edit.all_day != before.all_day {
        changed.push(EventField::AllDay);
    }
    if !same_recurrence(edit.recurrence.as_ref(), before.recurrence.as_ref()) {
        changed.push(EventField::Recurrence);
    }
    if !same_reminders(&edit.reminders, &before.reminders) {
        changed.push(EventField::Reminders);
    }
    if !same_invitees(&edit.attendees, &before.attendees) {
        changed.push(EventField::Attendees);
    }
    if edit.color_hex != before.color_hex {
        changed.push(EventField::ColorHex);
    }
    changed
}

fn same_text(a: Option<&str>, b: Option<&str>) -> bool {
    a.unwrap_or("") == b.unwrap_or("")
}

fn same_recurrence(a: Option<&EventRecurrence>, b: Option<&EventRecurrence>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let mut ea = a.exceptions.clone();
            let mut eb = b.exceptions.clone();
            ea.sort();
            ea.dedup();
            eb.sort();
            eb.dedup();
            a.rrule == b.rrule && a.tzid == b.tzid && ea == eb
        }
        _ => false,
    }
}

/// The same reminders, in any order, each as often.
fn same_reminders(a: &[Reminder], b: &[Reminder]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut left: Vec<&Reminder> = b.iter().collect();
    a.iter().all(|r| match left.iter().position(|l| *l == r) {
        Some(i) => {
            left.swap_remove(i);
            true
        }
        None => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReminderKind;
    use chrono::{TimeZone, Utc};

    fn event() -> Event {
        let at = Utc.with_ymd_and_hms(2026, 11, 9, 15, 0, 0).unwrap();
        Event {
            keep_attendees: false,
            clear_attendees: false,
            organized_elsewhere: false,
            id: "ev".into(),
            calendar_id: "cal".into(),
            title: "Sync".into(),
            description: None,
            location: None,
            start: at,
            end: at + chrono::Duration::minutes(30),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY".into(),
                exceptions: vec![at + chrono::Duration::weeks(1)],
                tzid: Some("Europe/Berlin".into()),
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![
                Reminder {
                    kind: ReminderKind::Relative { minutes_before: 10 },
                    sound: None,
                },
                Reminder {
                    kind: ReminderKind::Relative { minutes_before: 60 },
                    sound: None,
                },
            ],
            sound: None,
            attendees: vec!["Bob <bob@x>".into()],
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: at,
            updated_at: at,
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
        }
    }

    #[test]
    fn an_unchanged_copy_has_no_changed_field() {
        let before = event();
        let mut edit = before.clone();
        // Host-local parts, spelling and order do not count.
        edit.color_label = Some(crate::ColorLabelId("label".into()));
        edit.description = Some(String::new());
        edit.reminders.reverse();
        edit.attendees = vec!["BOB@x".into()];
        edit.recurrence
            .as_mut()
            .unwrap()
            .exceptions
            .push(before.start + chrono::Duration::weeks(1));
        assert_eq!(changed_fields(&edit, &before), []);
    }

    #[test]
    fn each_stored_field_is_named() {
        let before = event();
        let mut edit = before.clone();
        edit.title = "Sync 2".into();
        edit.location = Some("Room 3".into());
        edit.end += chrono::Duration::minutes(15);
        edit.recurrence.as_mut().unwrap().rrule = "FREQ=DAILY".into();
        edit.reminders.pop();
        edit.attendees.push("carol@x".into());
        assert_eq!(
            changed_fields(&edit, &before),
            [
                EventField::Title,
                EventField::Location,
                EventField::End,
                EventField::Recurrence,
                EventField::Reminders,
                EventField::Attendees,
            ]
        );
    }
}
