//! A meeting someone else organizes, on a provider that takes only the
//! attendee's own changes (decision 77a).
//!
//! RFC 6638 §3.2.2.1: on an attendee's copy of a meeting, a scheduling server
//! accepts the attendee's own reply and its own alarms, and refuses everything
//! else with `allowed-attendee-scheduling-object-change`. Aperio therefore
//! shows such an invitation read-only apart from those two, instead of
//! offering edits the server would throw away. Deleting stays possible: the
//! server then tells the organizer (83b).
//!
//! What the core owns here is the WRITE rule: which field an attendee may
//! change, and what a difference against an unseen copy means
//! ([`reply_only_change`], [`reply_only_verdict`]). The adapter asks it
//! against the server's fresh copy, before any PUT.
//!
//! Whether to show an editor read-only at all is two booleans the adapter
//! already answers — the calendar's `invitations_reply_only` and the event's
//! `organized_elsewhere` — and the surfaces read them through
//! `invitationLocked` in `shared/notifyAttendees.ts`, beside the notice rules
//! they belong with. A Rust twin of `a && b` would be a second place to keep
//! in step for nothing.

use crate::event_diff::{changed_fields, EventField};
use crate::Event;

/// The first field of `edit` that an attendee may not change, compared against
/// `before`, in [`EventField`] order. `None` when the edit changes nothing but
/// the attendee's own reminders.
///
/// Reminders are the one field an attendee writes. A sound, a colour label and
/// the answer itself are not fields at all ([`changed_fields`]).
pub fn reply_only_change(edit: &Event, before: &Event) -> Option<EventField> {
    changed_fields(edit, before)
        .into_iter()
        .find(|field| *field != EventField::Reminders)
}

/// What a write of an invitation may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyOnlyVerdict {
    /// Nothing protected changed: write it.
    Allowed,
    /// Something protected differs from a copy the caller never saw. That is
    /// the organizer's change, not the user's, so the save is a conflict and
    /// not a refusal: the surfaces ask the user to look at the new copy.
    Stale,
    /// The user changed a field only the organizer may change.
    Refused(EventField),
}

/// The verdict for saving `edit` over the copy `before`.
///
/// `same_version` says whether the caller edited *that* copy: its ETag was
/// known and equals the one just read. Without that proof a protected
/// difference is [`ReplyOnlyVerdict::Stale`], never a refusal — blaming the
/// user for the organizer's edit would be a guess, and the user cannot act on
/// it.
pub fn reply_only_verdict(edit: &Event, before: &Event, same_version: bool) -> ReplyOnlyVerdict {
    match reply_only_change(edit, before) {
        None => ReplyOnlyVerdict::Allowed,
        Some(field) if same_version => ReplyOnlyVerdict::Refused(field),
        Some(_) => ReplyOnlyVerdict::Stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Reminder, ReminderKind};
    use chrono::{TimeZone, Utc};

    fn invitation() -> Event {
        Event {
            keep_attendees: false,
            clear_attendees: false,
            organized_elsewhere: true,
            id: "ev-1".into(),
            calendar_id: "cal".into(),
            title: "Planning".into(),
            description: None,
            location: Some("Room 3".into()),
            start: Utc.with_ymd_and_hms(2026, 6, 15, 7, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 6, 15, 8, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: vec![Reminder {
                kind: ReminderKind::Relative { minutes_before: 10 },
                sound: None,
            }],
            sound: None,
            attendees: vec!["bob@example.com".into()],
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap(),
            etag: Some("\"e1\"".into()),
            organizer: Some("boss@example.com".into()),
            attendee_responses: Vec::new(),
            cancelled: false,
        }
    }

    #[test]
    fn the_attendees_own_reminders_are_allowed_and_the_rest_is_not() {
        let before = invitation();

        let mut edit = before.clone();
        edit.reminders.push(Reminder {
            kind: ReminderKind::Relative { minutes_before: 60 },
            sound: None,
        });
        edit.color_label = Some(crate::ColorLabelId("label".into()));
        edit.sound = None;
        assert_eq!(reply_only_change(&edit, &before), None);
        assert_eq!(
            reply_only_verdict(&edit, &before, true),
            ReplyOnlyVerdict::Allowed
        );

        // The first protected field is named, in EventField order.
        let mut edit = before.clone();
        edit.title = "Planning, moved".into();
        edit.location = Some("Room 4".into());
        edit.reminders.clear();
        assert_eq!(reply_only_change(&edit, &before), Some(EventField::Title));
        assert_eq!(
            reply_only_verdict(&edit, &before, true),
            ReplyOnlyVerdict::Refused(EventField::Title)
        );
    }

    #[test]
    fn a_difference_against_a_copy_the_caller_never_saw_is_the_organizers() {
        let before = invitation();
        let mut edit = before.clone();
        edit.start = before.start + chrono::Duration::hours(1);
        // The organizer moved the meeting between the read and this save: the
        // user is not told they changed something they did not.
        assert_eq!(
            reply_only_verdict(&edit, &before, false),
            ReplyOnlyVerdict::Stale
        );
    }
}
