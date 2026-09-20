//! Which fields an edit changed against a copy of the same event.
//!
//! An adapter that holds the provider's current copy can tell whether a save
//! would change anything there at all. A calendar server that mails the
//! invitees about every saved change to a meeting (RFC 6638 implicit
//! scheduling, decision 76a) must not get a write that changes nothing: a
//! colour, a sound or a private reminder lives on this device, and saving one
//! would otherwise mail every guest.
//!
//! The comparison is exact apart from what the editors change on their own
//! and what no provider stores:
//!
//! - title, description and location compare as the editors save them:
//!   trimmed the JavaScript way, with an empty text the same as none;
//! - reminders compare as a set of their kinds: the sound of a reminder and
//!   an app-start reminder live on this device only;
//! - invitees compare by address ([`crate::attendee::same_invitees`]).

use crate::attendee::same_invitees;
use crate::signatures::js_trim;
use crate::{Event, EventRecurrence, Reminder, ReminderKind};

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

impl EventField {
    /// The name the refusal message carries, so a surface can say which field
    /// was refused ([`crate::WriteRefusal`]).
    pub const fn token(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::Location => "location",
            Self::Start => "start",
            Self::End => "end",
            Self::AllDay => "all-day",
            Self::Recurrence => "recurrence",
            Self::Reminders => "reminders",
            Self::Attendees => "attendees",
            Self::ColorHex => "color",
        }
    }
}

/// The fields of `edit` that differ from `before`, in the order of
/// [`EventField`]. Empty when saving `edit` over `before` changes nothing a
/// provider stores. Host-local parts (colour label, sound) are not fields.
pub fn changed_fields(edit: &Event, before: &Event) -> Vec<EventField> {
    let mut changed = Vec::new();
    if !same_text(Some(&edit.title), Some(&before.title)) {
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
    js_trim(a.unwrap_or("")) == js_trim(b.unwrap_or(""))
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

/// The same stored reminders, in any order, each as often: compared by
/// kind, without the app-start ones.
fn same_reminders(a: &[Reminder], b: &[Reminder]) -> bool {
    let stored = |list: &[Reminder]| -> Vec<ReminderKind> {
        list.iter()
            .map(|r| r.kind.clone())
            .filter(|k| !matches!(k, ReminderKind::AppStart))
            .collect()
    };
    let (a, b) = (stored(a), stored(b));
    if a.len() != b.len() {
        return false;
    }
    let mut left: Vec<&ReminderKind> = b.iter().collect();
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
            scheduling_silenced: false,
        }
    }

    #[test]
    fn an_unchanged_copy_has_no_changed_field() {
        let before = event();
        let mut edit = before.clone();
        // Host-local parts, spelling and order do not count, nor what the
        // editors trim or no provider stores.
        edit.color_label = Some(crate::ColorLabelId("label".into()));
        edit.description = Some(String::new());
        edit.title = " Sync\u{00A0}\n".into();
        edit.reminders.reverse();
        edit.reminders[0].sound = Some(crate::SoundConfig::default());
        edit.reminders.push(Reminder {
            kind: ReminderKind::AppStart,
            sound: None,
        });
        edit.attendees = vec!["BOB@x".into()];
        edit.recurrence
            .as_mut()
            .unwrap()
            .exceptions
            .push(before.start + chrono::Duration::weeks(1));
        assert_eq!(changed_fields(&edit, &before), []);
    }

    #[test]
    fn every_field_has_a_stable_token() {
        // The refusal message carries the token
        // (`reply-only-invitation: title`), which is what a reader of a log,
        // a bug report or a server trace sees; the surfaces say the sentence
        // for the REFUSAL, not for the field. Renaming one silently would
        // change what every such message says.
        let all = [
            EventField::Title,
            EventField::Description,
            EventField::Location,
            EventField::Start,
            EventField::End,
            EventField::AllDay,
            EventField::Recurrence,
            EventField::Reminders,
            EventField::Attendees,
            EventField::ColorHex,
        ];
        let tokens: Vec<&str> = all.iter().map(|f| f.token()).collect();
        assert_eq!(
            tokens,
            [
                "title",
                "description",
                "location",
                "start",
                "end",
                "all-day",
                "recurrence",
                "reminders",
                "attendees",
                "color"
            ]
        );
    }

    #[test]
    fn each_stored_field_is_named() {
        let before = event();
        let mut edit = before.clone();
        edit.title = "Sync 2".into();
        edit.description = Some("Agenda".into());
        edit.location = Some("Room 3".into());
        edit.start -= chrono::Duration::minutes(15);
        edit.end += chrono::Duration::minutes(15);
        edit.all_day = true;
        edit.recurrence.as_mut().unwrap().rrule = "FREQ=DAILY".into();
        edit.reminders.pop();
        edit.attendees.push("carol@x".into());
        edit.color_hex = Some("#336699".into());
        assert_eq!(
            changed_fields(&edit, &before),
            [
                EventField::Title,
                EventField::Description,
                EventField::Location,
                EventField::Start,
                EventField::End,
                EventField::AllDay,
                EventField::Recurrence,
                EventField::Reminders,
                EventField::Attendees,
                EventField::ColorHex,
            ]
        );
        // One more of the same kind is a change, the kind of one too.
        let mut edit = before.clone();
        edit.reminders.push(before.reminders[0].clone());
        assert_eq!(changed_fields(&edit, &before), [EventField::Reminders]);
        let mut edit = before.clone();
        edit.recurrence = None;
        assert_eq!(changed_fields(&edit, &before), [EventField::Recurrence]);
    }
}
