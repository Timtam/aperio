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
//! The same rule decides in three places, so they cannot disagree: the
//! editors lock their fields by [`invitation_locked`], the adapter refuses a
//! protected change by [`reply_only_verdict`] against the server's fresh copy,
//! and `shared/invitationLocked.ts` is the surfaces' twin of the first.

use crate::event_diff::{changed_fields, EventField};
use crate::Event;

/// Whether an event is such an invitation: the calendar's provider takes only
/// the attendee's changes, and the account does not organize this meeting.
///
/// Both answers come from the adapter — `Calendar::invitations_reply_only` at
/// discovery, `Event::organized_elsewhere` from the event's own ORGANIZER line
/// (decision 70a) — so nothing is guessed from addresses here.
pub const fn invitation_locked(calendar_reply_only: bool, organized_elsewhere: bool) -> bool {
    calendar_reply_only && organized_elsewhere
}

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
    fn only_someone_elses_meeting_on_such_a_provider_is_locked() {
        assert!(invitation_locked(true, true));
        assert!(!invitation_locked(true, false));
        assert!(!invitation_locked(false, true));
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
