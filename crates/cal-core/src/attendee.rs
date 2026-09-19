//! Shared parsing for the attendee strings the UI collects.
//!
//! Aperio models attendees as a flat `Vec<String>` where each entry is
//! either `"Display Name <email@host>"` (the form the contact picker emits)
//! or a bare `"email@host"` (free-form entry). Every calendar adapter has to
//! turn that into its own wire shape — EWS `<t:Mailbox>` (Name + EmailAddress),
//! CalDAV `ATTENDEE;CN=..:mailto:..`, Google `{email, displayName}`, Graph
//! `emailAddress.{address,name}`. Centralising the split here keeps those four
//! mappings consistent instead of each re-inventing a parser.
//!
//! # The organizer is never an invitee (decision 67a)
//!
//! Providers list the organizer among the attendees: Exchange as a row with
//! the response "Organizer", Google with `organizer: true`, Graph with the
//! response "organizer", CalDAV as an `ATTENDEE` equal to `ORGANIZER`. Read
//! as an invitee, an appointment the user made alone looked like a meeting:
//! the editors offered to notify, and Exchange refused the save because the
//! only recipient was the sender. So the rule lives here, once:
//!
//! - On read, [`people_from_read`] drops the organizer's row from both the
//!   editable list and the responses. Each adapter hands it the provider's
//!   explicit per-row flag; without one, an exact match of the normalised
//!   address with the organizer counts. An unknown organizer drops nothing.
//! - Only an event the connected account organizes may notify anyone
//!   (decision 70a). The adapter says whether it does, from the provider's
//!   own statement; [`people_from_read`] turns that into
//!   [`Event::organized_elsewhere`].
//! - On write, [`guard_update`] and [`guard_create`] keep the organizer out of
//!   what goes to the provider, clear a send intent nobody may act on, and
//!   (decision 71a) tell the adapter to leave the provider's list alone when
//!   the edit did not change it.
//!
//! [`Event::organized_elsewhere`]: crate::Event::organized_elsewhere

use crate::{AttendeeResponse, AttendeeStatus, Event, NewEvent};

/// An address in the one spelling comparisons use: trimmed, without a
/// leading `mailto:` in any case, and lower-cased.
pub fn normalize_address(address: &str) -> String {
    let trimmed = address.trim();
    let bare = match trimmed.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => &trimmed[7..],
        _ => trimmed,
    };
    bare.trim().to_lowercase()
}

/// The editable form of one attendee: `"Name <email>"` when the trimmed name
/// says something the address does not, otherwise the bare address. The
/// inverse of [`parse`].
pub fn format(name: Option<&str>, email: &str) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty() && *n != email) {
        Some(name) => format!("{name} <{email}>"),
        None => email.to_string(),
    }
}

/// One attendee row as a provider reports it, in core terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadAttendee {
    pub email: String,
    pub name: Option<String>,
    pub status: AttendeeStatus,
    /// The provider marks this row as the organizer's own.
    pub is_organizer: bool,
}

/// Who an event involves, as the rules above read it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventPeople {
    pub organizer: Option<String>,
    /// The invitees, in the editable form: never the organizer.
    pub attendees: Vec<String>,
    /// The invitees' responses, in the same order: never the organizer's.
    pub attendee_responses: Vec<AttendeeResponse>,
    /// Someone other than the connected account organizes the event, or an
    /// organizer is named and the provider does not say it is the account
    /// (see [`organized_elsewhere`]).
    pub organized_elsewhere: bool,
}

/// The read rule: the event's people without the organizer among them.
///
/// `organizer` is the provider's organizer address, if it names one. A row is
/// the organizer's when the provider flags it, or when its normalised address
/// equals the organizer's. When the provider names no organizer, the address
/// of the one flagged row stands in for it; with several flagged rows it stays
/// unknown. Rows without an address are skipped; the others keep their order.
///
/// `organized_by_me` is the provider's own statement that the connected
/// account organizes the event: `Some(true)` or `Some(false)` when it says,
/// `None` when it cannot. See [`organized_elsewhere`] for how it counts.
pub fn people_from_read(
    organizer: Option<String>,
    organized_by_me: Option<bool>,
    rows: impl IntoIterator<Item = ReadAttendee>,
) -> EventPeople {
    let rows: Vec<ReadAttendee> = rows
        .into_iter()
        .filter(|r| !r.email.trim().is_empty())
        .collect();
    let organizer = organizer.filter(|o| !o.trim().is_empty()).or_else(|| {
        let mut flagged = rows.iter().filter(|r| r.is_organizer);
        match (flagged.next(), flagged.next()) {
            (Some(only), None) => Some(only.email.clone()),
            _ => None,
        }
    });
    let organizer_address = organizer.as_deref().map(normalize_address);
    let mut people = EventPeople {
        organized_elsewhere: organized_elsewhere(organizer.as_deref(), organized_by_me),
        organizer,
        ..EventPeople::default()
    };
    for row in rows {
        let is_organizer = row.is_organizer
            || organizer_address.as_deref() == Some(normalize_address(&row.email).as_str());
        if is_organizer {
            continue;
        }
        let name = row.name.filter(|n| !n.trim().is_empty());
        people.attendees.push(format(name.as_deref(), &row.email));
        people.attendee_responses.push(AttendeeResponse {
            email: row.email,
            name,
            status: row.status,
        });
    }
    people
}

/// Whether someone other than the connected account organizes an event.
///
/// The provider's answer counts first: `Some(true)` is the account's own,
/// `Some(false)` is someone else's, even when the provider names no organizer
/// address. Without an answer, an event with an organizer counts as organized
/// elsewhere, because only the organizer may notify anyone and an unconfirmed
/// claim notifies nobody; an event with no organizer is the account's own (a
/// plain appointment).
pub fn organized_elsewhere(organizer: Option<&str>, organized_by_me: Option<bool>) -> bool {
    match organized_by_me {
        Some(mine) => !mine,
        None => organizer.is_some_and(|o| !o.trim().is_empty()),
    }
}

/// Whether two addresses name the same mailbox, as the rules above compare
/// them (see [`normalize_address`]). Empty addresses name nobody.
pub fn same_address(a: &str, b: &str) -> bool {
    let a = normalize_address(a);
    !a.is_empty() && a == normalize_address(b)
}

/// `attendees` without the entries whose address is the organizer's.
/// Unchanged when the organizer is unknown.
pub fn without_organizer(attendees: &[String], organizer: Option<&str>) -> Vec<String> {
    let Some(organizer) = organizer.map(normalize_address).filter(|o| !o.is_empty()) else {
        return attendees.to_vec();
    };
    attendees
        .iter()
        .filter(|entry| normalize_address(&parse(entry).1) != organizer)
        .cloned()
        .collect()
}

/// Whether two attendee lists invite the same people: the same normalised
/// addresses, in any order. Display names and spelling do not count.
pub fn same_invitees(a: &[String], b: &[String]) -> bool {
    let set = |list: &[String]| {
        let mut addresses: Vec<String> = list
            .iter()
            .map(|entry| normalize_address(&parse(entry).1))
            .filter(|address| !address.is_empty())
            .collect();
        addresses.sort();
        addresses.dedup();
        addresses
    };
    set(a) == set(b)
}

/// The write rule for an update, run by the host before the adapter sees the
/// event. `read` is the event as it was last read, when the host has it.
///
/// - The organizer leaves the attendee list, and the responses.
/// - The send intent survives only for an event the account organizes, with
///   someone else invited, or with invitees just removed, who may be told
///   (decision 74a).
/// - When `read` shows the same invitees, [`Event::keep_attendees`] tells the
///   adapter to leave the provider's list as it is (decision 71a), so a title
///   or time change never rewrites who is invited.
/// - When the edit removed every invitee `read` shows,
///   [`Event::clear_attendees`] tells the adapter to write the list empty,
///   which it never does for an empty list alone.
pub fn guard_update(event: &mut Event, read: Option<&Event>) {
    let organizer = event.organizer.clone();
    event.attendees = without_organizer(&event.attendees, organizer.as_deref());
    if let Some(organizer) = organizer.as_deref().map(normalize_address) {
        event
            .attendee_responses
            .retain(|r| normalize_address(&r.email) != organizer);
    }
    let before = read.map(|read| without_organizer(&read.attendees, read.organizer.as_deref()));
    event.keep_attendees = before
        .as_deref()
        .is_some_and(|before| same_invitees(before, &event.attendees));
    event.clear_attendees =
        event.attendees.is_empty() && before.as_deref().is_some_and(|before| !before.is_empty());
    event.send_invitations &=
        !event.organized_elsewhere && (!event.attendees.is_empty() || event.clear_attendees);
}

/// The write rule for a create, run by the host before the adapter sees it.
/// A create derived from an existing event (a single occurrence carved out, a
/// copy, a carried or detached one) names that event's organizer in
/// [`NewEvent::organizer`], so the organizer leaves the list here too; and it
/// may notify only if that event was the account's own (decision 72a).
pub fn guard_create(event: &mut NewEvent) {
    event.attendees = without_organizer(&event.attendees, event.organizer.as_deref());
    event.send_invitations &= !event.organized_elsewhere && !event.attendees.is_empty();
}

/// Split an attendee entry into an optional display name and an email.
///
/// Recognises `"Display Name <email@host>"` (the inner address is taken
/// verbatim, the leading text becomes the trimmed, dequoted name) and bare
/// `"email@host"` (yields `(None, entry)`). A bracketed form with an empty
/// address falls back to treating the whole entry as the address. No
/// validation beyond the bracket split — callers treat the returned address
/// as authoritative.
pub fn parse(entry: &str) -> (Option<String>, String) {
    let entry = entry.trim();
    if let Some(open) = entry.rfind('<') {
        if let Some(rel_close) = entry[open + 1..].find('>') {
            let email = entry[open + 1..open + 1 + rel_close].trim();
            if !email.is_empty() {
                let name = entry[..open].trim().trim_matches('"').trim();
                let display = (!name.is_empty()).then(|| name.to_string());
                return (display, email.to_string());
            }
        }
    }
    (None, entry.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn bare_email_has_no_display_name() {
        assert_eq!(
            parse("alice@example.com"),
            (None, "alice@example.com".into())
        );
    }

    #[test]
    fn name_and_angle_bracket_email_split() {
        assert_eq!(
            parse("Alice Smith <alice@example.com>"),
            (Some("Alice Smith".into()), "alice@example.com".into())
        );
    }

    #[test]
    fn surrounding_and_inner_whitespace_trimmed() {
        assert_eq!(
            parse("  Bob   <  bob@example.com  > "),
            (Some("Bob".into()), "bob@example.com".into())
        );
    }

    #[test]
    fn quoted_display_name_is_dequoted() {
        assert_eq!(
            parse("\"Doe, John\" <john@example.com>"),
            (Some("Doe, John".into()), "john@example.com".into())
        );
    }

    #[test]
    fn angle_brackets_without_a_name() {
        assert_eq!(
            parse("<just@brackets.com>"),
            (None, "just@brackets.com".into())
        );
    }

    #[test]
    fn empty_angle_brackets_fall_back_to_whole_entry() {
        // Degenerate input — keep the raw string rather than inventing data.
        assert_eq!(parse("weird <>"), (None, "weird <>".into()));
    }

    mod organizer {
        use super::super::*;
        use chrono::{TimeZone, Utc};

        fn row(email: &str, is_organizer: bool) -> ReadAttendee {
            ReadAttendee {
                email: email.into(),
                name: None,
                status: AttendeeStatus::Accepted,
                is_organizer,
            }
        }

        fn event(organizer: Option<&str>, attendees: &[&str]) -> Event {
            let at = Utc.with_ymd_and_hms(2026, 10, 6, 7, 0, 0).unwrap();
            Event {
                id: "ev".into(),
                calendar_id: "cal".into(),
                title: "Standup".into(),
                description: None,
                location: None,
                start: at,
                end: at,
                all_day: false,
                recurrence: None,
                color_label: None,
                color_hex: None,
                reminders: Vec::new(),
                sound: None,
                attendees: attendees.iter().map(|a| a.to_string()).collect(),
                send_invitations: true,
                truncate_tail_overrides: false,
                keep_attendees: false,
                clear_attendees: false,
                created_at: at,
                updated_at: at,
                etag: None,
                organizer: organizer.map(str::to_string),
                organized_elsewhere: false,
                attendee_responses: Vec::new(),
                cancelled: false,
            }
        }

        #[test]
        fn an_address_is_compared_in_one_spelling() {
            assert_eq!(
                normalize_address("  MailTo:Boss@Example.COM "),
                "boss@example.com"
            );
            assert_eq!(normalize_address(" bob@example.com "), "bob@example.com");
            assert!(same_address("mailto:A@x", "a@x"));
            assert!(!same_address("", ""), "an empty address names nobody");
        }

        #[test]
        fn format_is_the_inverse_of_parse() {
            assert_eq!(format(Some(" Alice "), "a@x"), "Alice <a@x>");
            assert_eq!(format(Some("a@x"), "a@x"), "a@x");
            assert_eq!(format(None, "a@x"), "a@x");
            assert_eq!(
                parse(&format(Some("Alice"), "a@x")),
                (Some("Alice".into()), "a@x".into())
            );
        }

        /// Live round 4: an appointment made in Outlook lists its organizer, the
        /// account itself, as the only attendee.
        #[test]
        fn an_organizer_alone_leaves_no_invitees() {
            let people = people_from_read(Some("toni@x".into()), Some(true), [row("toni@x", true)]);
            assert!(people.attendees.is_empty());
            assert!(people.attendee_responses.is_empty());
            assert_eq!(people.organizer.as_deref(), Some("toni@x"));
            assert!(!people.organized_elsewhere);
        }

        /// The flag wins where the addresses differ (an Exchange-internal
        /// organizer address, an alias), and the address wins where there is no
        /// flag (CalDAV).
        #[test]
        fn the_organizer_row_is_found_by_flag_or_by_address() {
            let by_flag = people_from_read(
                Some("/o=Org/cn=boss".into()),
                None,
                [row("boss@x", true), row("bob@x", false)],
            );
            assert_eq!(by_flag.attendees, ["bob@x"]);
            assert_eq!(by_flag.organizer.as_deref(), Some("/o=Org/cn=boss"));
            let by_address = people_from_read(
                Some("mailto:Boss@X".into()),
                None,
                [row("boss@x", false), row("bob@x", false)],
            );
            assert_eq!(by_address.attendees, ["bob@x"]);
        }

        #[test]
        fn nothing_is_guessed() {
            // No organizer, no flag: nobody is the organizer.
            let unknown = people_from_read(None, None, [row("a@x", false), row("b@x", false)]);
            assert_eq!(unknown.attendees, ["a@x", "b@x"]);
            assert!(
                !unknown.organized_elsewhere,
                "no organizer: the account's own"
            );
            // An organizer that matches no row drops nothing.
            let absent = people_from_read(Some("c@x".into()), None, [row("a@x", false)]);
            assert_eq!(absent.attendees, ["a@x"]);
        }

        #[test]
        fn one_flagged_row_names_a_missing_organizer() {
            let one = people_from_read(None, None, [row("boss@x", true), row("bob@x", false)]);
            assert_eq!(one.organizer.as_deref(), Some("boss@x"));
            assert_eq!(one.attendees, ["bob@x"]);
            let two = people_from_read(
                None,
                None,
                [
                    row("a@x", true),
                    row("b@x", true),
                    row("", false),
                    row("c@x", false),
                ],
            );
            assert_eq!(two.organizer, None, "two flags name nobody");
            assert_eq!(
                two.attendees,
                ["c@x"],
                "flagged and empty rows go, order stays"
            );
            assert_eq!(two.attendee_responses.len(), 1);
        }

        /// Only the organizer may notify; an unconfirmed claim notifies nobody.
        #[test]
        fn only_a_confirmed_organizer_is_the_accounts() {
            assert!(!organized_elsewhere(Some("me@x"), Some(true)));
            assert!(organized_elsewhere(Some("boss@x"), Some(false)));
            assert!(organized_elsewhere(Some("boss@x"), None));
            assert!(!organized_elsewhere(None, None));
            assert!(!organized_elsewhere(Some("  "), None));
            // The provider's answer counts even without an organizer address.
            assert!(organized_elsewhere(None, Some(false)));
            assert!(!organized_elsewhere(None, Some(true)));
            let unnamed = people_from_read(
                None,
                Some(false),
                [row("a@x", true), row("b@x", true), row("c@x", false)],
            );
            assert_eq!(unnamed.organizer, None);
            assert!(unnamed.organized_elsewhere);
        }

        #[test]
        fn the_update_guard_drops_the_organizer_and_the_send_nobody_needs() {
            let mut alone = event(Some("toni@x"), &["Toni <TONI@x>"]);
            guard_update(&mut alone, None);
            assert!(alone.attendees.is_empty());
            assert!(!alone.send_invitations, "nobody else to notify");

            let mut meeting = event(Some("toni@x"), &["Toni <toni@x>", "bob@x"]);
            guard_update(&mut meeting, None);
            assert_eq!(meeting.attendees, ["bob@x"]);
            assert!(meeting.send_invitations);

            let mut invitation = event(Some("boss@x"), &["me@x", "bob@x"]);
            invitation.organized_elsewhere = true;
            guard_update(&mut invitation, None);
            assert!(!invitation.send_invitations, "only the organizer notifies");
        }

        /// Decision 71a: an edit that invites the same people leaves the
        /// provider's list alone, whatever the spelling or order.
        #[test]
        fn the_update_guard_keeps_an_unchanged_list() {
            let read = event(Some("toni@x"), &["Toni <toni@x>", "Bob <bob@x>", "carol@x"]);
            let mut same = event(Some("toni@x"), &["CAROL@x", "bob@x"]);
            guard_update(&mut same, Some(&read));
            assert!(same.keep_attendees);

            let mut changed = event(Some("toni@x"), &["bob@x"]);
            guard_update(&mut changed, Some(&read));
            assert!(!changed.keep_attendees, "carol was removed");

            let mut unknown = event(Some("toni@x"), &["bob@x"]);
            guard_update(&mut unknown, None);
            assert!(!unknown.keep_attendees, "nothing to compare with: write");
        }

        /// Decision 74a: removing the last invitee clears the provider's
        /// list, and the one removed may still be told. Only a read that
        /// showed invitees says so; an empty list alone clears nothing.
        #[test]
        fn removing_the_last_invitee_clears_the_list() {
            let read = event(Some("toni@x"), &["Toni <toni@x>", "bob@x"]);
            let mut removed = event(Some("toni@x"), &[]);
            guard_update(&mut removed, Some(&read));
            assert!(removed.clear_attendees);
            assert!(!removed.keep_attendees);
            assert!(removed.send_invitations, "bob may get a cancellation");

            let mut elsewhere = event(Some("boss@x"), &[]);
            elsewhere.organized_elsewhere = true;
            guard_update(&mut elsewhere, Some(&event(Some("boss@x"), &["bob@x"])));
            assert!(elsewhere.clear_attendees);
            assert!(!elsewhere.send_invitations, "only the organizer notifies");

            let mut alone = event(Some("toni@x"), &[]);
            guard_update(&mut alone, Some(&event(Some("toni@x"), &["Toni <toni@x>"])));
            assert!(!alone.clear_attendees, "the organizer was never a guest");
            assert!(alone.keep_attendees);
            assert!(!alone.send_invitations);

            let mut unknown = event(Some("toni@x"), &[]);
            guard_update(&mut unknown, None);
            assert!(!unknown.clear_attendees, "nothing read: nothing cleared");
            assert!(!unknown.send_invitations);
        }

        #[test]
        fn the_create_guard_drops_the_source_organizer() {
            let at = Utc.with_ymd_and_hms(2026, 10, 6, 7, 0, 0).unwrap();
            let mut new = NewEvent {
                title: "Standup".into(),
                description: None,
                location: None,
                start: at,
                end: at,
                all_day: false,
                recurrence: None,
                color_label: None,
                color_hex: None,
                reminders: Vec::new(),
                sound: None,
                attendees: vec!["Toni <toni@x>".into()],
                send_invitations: true,
                organizer: Some("mailto:toni@x".into()),
                organized_elsewhere: false,
            };
            guard_create(&mut new);
            assert!(new.attendees.is_empty());
            assert!(!new.send_invitations);

            new.attendees = vec!["bob@x".into()];
            new.send_invitations = true;
            new.organized_elsewhere = true;
            guard_create(&mut new);
            assert_eq!(new.attendees, ["bob@x"]);
            assert!(
                !new.send_invitations,
                "a copy of someone else's meeting notifies nobody"
            );
        }
    }
}
