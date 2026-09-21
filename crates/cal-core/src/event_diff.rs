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
    /// Every field, in [`changed_fields`] order.
    pub const ALL: [EventField; 10] = [
        Self::Title,
        Self::Description,
        Self::Location,
        Self::Start,
        Self::End,
        Self::AllDay,
        Self::Recurrence,
        Self::Reminders,
        Self::Attendees,
        Self::ColorHex,
    ];

    /// The field a [`token`](Self::token) names; `None` for one this build
    /// does not know.
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.token() == token)
    }

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

/// The fields `edit` leaves exactly as the copy the editor opened has them
/// (decision 106): what an adapter should leave as the PROVIDER has it, even
/// where the provider's value differs from this device's, because then the
/// difference is somebody else's change and not the user's.
///
/// A two-way comparison against the provider (`changed_fields(edit, server)`)
/// cannot tell those apart: a field this device holds stale differs from the
/// server and would be written back. This is the third side.
///
/// `opened` counts only when it is provably the copy the edit was made from.
/// Its ETag must be known and equal the edit's, and `refreshed_since_last_write`
/// must hold: the caller's copy has been read from the provider again since
/// this app last wrote there. The second half is not a nicety. A host that only
/// marks its copy stale after a write still hands out the pre-write row, with
/// the pre-write ETag, to every surface that asks — and an edit built from it
/// (a split's restore, a revert right after a save, a meeting link removed
/// just after it was added) would compare equal to it field by field, keep
/// every field, and write nothing. Without that proof nothing is kept, and the
/// adapter compares with the provider alone, as before.
///
/// The invitees are never named: [`Event::keep_attendees`] carries decision
/// 71a's own rule for them. Nor is the colour, which the host resolves.
pub fn kept_fields(
    edit: &Event,
    opened: Option<&Event>,
    refreshed_since_last_write: bool,
) -> Vec<EventField> {
    let Some(opened) = opened else {
        return Vec::new();
    };
    if !refreshed_since_last_write || opened.etag.is_none() || opened.etag != edit.etag {
        return Vec::new();
    }
    let changed = changed_fields(edit, opened);
    EventField::ALL
        .into_iter()
        .filter(|field| !matches!(field, EventField::Attendees | EventField::ColorHex))
        .filter(|field| !changed.contains(field))
        .collect()
}

/// `edit`, with the content it left alone taken from `provider`: the title,
/// description, location and reminders named in [`Event::keep_fields`], and
/// the invitees when [`Event::keep_attendees`] says the edit did not change
/// them.
///
/// For a write that CREATES what the edit describes rather than updating a
/// field at a time — an Exchange exception Exchange will not move is created
/// again as a single — so what the user did not touch is the provider's, not
/// whatever this device's copy said. The slot is always the edit's: it is one
/// fact, and a create exists to put it somewhere new. A reminder only this
/// device keeps (app start) stays with the edit, since the provider never had
/// it.
pub fn take_kept_content(edit: &Event, provider: &Event) -> Event {
    let kept = |field: EventField| edit.keep_fields.contains(&field);
    let mut out = edit.clone();
    if kept(EventField::Title) {
        out.title = provider.title.clone();
    }
    if kept(EventField::Description) {
        out.description = provider.description.clone();
    }
    if kept(EventField::Location) {
        out.location = provider.location.clone();
    }
    if kept(EventField::Reminders) {
        out.reminders = provider
            .reminders
            .iter()
            .filter(|r| !matches!(r.kind, ReminderKind::AppStart))
            .chain(
                edit.reminders
                    .iter()
                    .filter(|r| matches!(r.kind, ReminderKind::AppStart)),
            )
            .cloned()
            .collect();
    }
    if edit.keep_attendees {
        out.attendees = provider.attendees.clone();
    }
    out
}

/// [`Event::keep_fields`] on the wire: each field by its [`EventField::token`],
/// the one spelling the refusal messages already use.
///
/// Read leniently. The list reaches every adapter plugin, and a plugin built
/// before a field existed must not fail the whole update over a token it does
/// not know: that token is dropped, the field counts as not kept, and it is
/// written — which is what that plugin always did.
pub(crate) mod keep_fields_wire {
    use super::EventField;
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        fields: &[EventField],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(fields.len()))?;
        for field in fields {
            seq.serialize_element(field.token())?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<EventField>, D::Error> {
        let tokens = Vec::<String>::deserialize(deserializer)?;
        Ok(tokens
            .iter()
            .filter_map(|token| EventField::from_token(token))
            .collect())
    }
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
            keep_fields: Vec::new(),
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
        let tokens: Vec<&str> = EventField::ALL.iter().map(|f| f.token()).collect();
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

    /// The copy the editor opened, at a known version.
    fn opened() -> Event {
        Event {
            etag: Some("v1".into()),
            ..event()
        }
    }

    /// Decision 106: every field the edit left as the opened copy has it is
    /// kept — the invitees and the colour never, they have their own rules.
    #[test]
    fn the_fields_the_edit_left_alone_are_kept() {
        let edit = Event {
            title: "Sync, moved room".into(),
            ..opened()
        };
        assert_eq!(
            kept_fields(&edit, Some(&opened()), true),
            [
                EventField::Description,
                EventField::Location,
                EventField::Start,
                EventField::End,
                EventField::AllDay,
                EventField::Recurrence,
                EventField::Reminders,
            ]
        );
    }

    #[test]
    fn the_invitees_and_the_colour_are_never_kept_here() {
        let kept = kept_fields(&opened(), Some(&opened()), true);
        assert!(!kept.contains(&EventField::Attendees), "{kept:?}");
        assert!(!kept.contains(&EventField::ColorHex), "{kept:?}");
        assert_eq!(kept.len(), 8, "everything else: {kept:?}");
    }

    /// Only a copy that is provably the one the edit was made from counts.
    /// Without that proof nothing is kept, and the adapter compares with the
    /// provider alone.
    #[test]
    fn without_proof_of_the_opened_copy_nothing_is_kept() {
        let edit = opened();
        // Another version: the edit was made from another copy.
        let newer = Event {
            etag: Some("v2".into()),
            title: "Renamed elsewhere".into(),
            ..opened()
        };
        assert_eq!(kept_fields(&edit, Some(&newer), true), []);
        // No version at all: two unknowns are not the same version.
        let unknown = Event {
            etag: None,
            ..opened()
        };
        let unknown_edit = Event {
            etag: None,
            ..opened()
        };
        assert_eq!(kept_fields(&unknown_edit, Some(&unknown), true), []);
        // A copy from before this app's last write there: the split's
        // restore, a revert right after a save.
        assert_eq!(kept_fields(&edit, Some(&opened()), false), []);
        // Nothing read at all.
        assert_eq!(kept_fields(&edit, None, true), []);
    }

    /// The list travels to every adapter plugin. A token a plugin's build does
    /// not know is dropped rather than failing the whole event: that field is
    /// then written, as that plugin always did.
    #[test]
    fn an_unknown_kept_field_is_dropped_not_fatal() {
        let mut json = serde_json::to_value(Event {
            keep_fields: vec![EventField::Title, EventField::Location],
            ..opened()
        })
        .unwrap();
        assert_eq!(
            json["keep_fields"],
            serde_json::json!(["title", "location"])
        );
        json["keep_fields"] = serde_json::json!(["title", "zone", "color"]);
        let read: Event = serde_json::from_value(json).unwrap();
        assert_eq!(read.keep_fields, [EventField::Title, EventField::ColorHex]);
        // Nothing kept: nothing on the wire, and an older payload reads as none.
        let json = serde_json::to_value(opened()).unwrap();
        assert!(json.get("keep_fields").is_none(), "{json}");
        let read: Event = serde_json::from_value(json).unwrap();
        assert!(read.keep_fields.is_empty());
    }

    #[test]
    fn every_token_names_its_field_back() {
        for field in EventField::ALL {
            assert_eq!(EventField::from_token(field.token()), Some(field));
        }
        assert_eq!(EventField::from_token("color-hex"), None);
    }

    /// A create that stands in for an update takes what the user did not
    /// touch from the provider: the text, the place, the reminders and — when
    /// the edit left them alone — the invitees. The slot is always the edit's,
    /// and a reminder only this device keeps stays with it.
    #[test]
    fn a_create_in_place_of_an_update_takes_the_untouched_content_from_the_provider() {
        let provider = Event {
            title: "Kickoff".into(),
            description: Some("Own agenda".into()),
            location: Some("Room 7".into()),
            reminders: vec![Reminder {
                kind: ReminderKind::Relative { minutes_before: 30 },
                sound: None,
            }],
            attendees: vec!["carol@x".into()],
            ..event()
        };
        let moved = Event {
            start: event().start + chrono::Duration::days(7),
            end: event().end + chrono::Duration::days(7),
            reminders: vec![
                Reminder {
                    kind: ReminderKind::Relative { minutes_before: 10 },
                    sound: None,
                },
                Reminder {
                    kind: ReminderKind::AppStart,
                    sound: None,
                },
            ],
            keep_fields: vec![
                EventField::Title,
                EventField::Description,
                EventField::Location,
                EventField::Reminders,
            ],
            keep_attendees: true,
            ..event()
        };
        let created = take_kept_content(&moved, &provider);
        assert_eq!(created.title, "Kickoff");
        assert_eq!(created.description.as_deref(), Some("Own agenda"));
        assert_eq!(created.location.as_deref(), Some("Room 7"));
        assert_eq!(created.attendees, ["carol@x"]);
        assert_eq!(
            created
                .reminders
                .iter()
                .map(|r| r.kind.clone())
                .collect::<Vec<_>>(),
            [
                ReminderKind::Relative { minutes_before: 30 },
                ReminderKind::AppStart,
            ]
        );
        assert_eq!(created.start, moved.start, "the slot is the edit's");
        assert_eq!(created.end, moved.end);

        // What the user changed stays the user's.
        let retitled = Event {
            keep_fields: Vec::new(),
            keep_attendees: false,
            ..moved.clone()
        };
        let created = take_kept_content(&retitled, &provider);
        assert_eq!(created.title, moved.title);
        assert_eq!(created.location, moved.location);
        assert_eq!(created.attendees, moved.attendees);
    }
}
