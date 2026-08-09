//! Meetings Aperio itself created, and the events they belong to.
//!
//! Joining works for any meeting, from any tool, because the join URL travels
//! in the event where every calendar client can read it (`cal_core`'s
//! conference detection does the reading). *Owning* one is different: to look a
//! meeting up or delete it, the provider wants its own id, and a join URL does
//! not carry one — Webex's link has an `MTID` and nothing else.
//!
//! So this module keeps the one fact that cannot be recovered from the event:
//! which provider-side meeting Aperio minted for which event, and through which
//! account. It is host-local (see migration 0034 for why) and it exists so that
//! deleting an event can also take its meeting down, rather than leaving one
//! standing on the provider for every event anyone ever created.
//!
//! Nothing here knows a provider. The account id routes to whatever
//! videoconference adapter is registered for it, and the meeting id is opaque.

use chrono::Utc;
use rusqlite::{params, OptionalExtension};

use crate::db::SharedConn;

/// The meeting Aperio created for an event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EventMeeting {
    /// Series master id — one meeting serves a whole recurring series, exactly
    /// as a recurring meeting does on the provider side.
    pub event_id: String,
    /// The videoconference account that minted it. Routes the later lookup or
    /// delete back to the same adapter.
    pub account_id: String,
    /// Provider-side id. Opaque here.
    pub meeting_id: String,
    /// What the user actually clicks. Kept alongside so the UI can show the
    /// binding without a network call.
    pub join_url: String,
    pub created_at: String,
}

/// The bare email addresses in an event's attendee list.
///
/// Aperio's attendees are display strings — `"Alice Smith <alice@example.test>"`
/// from the contact picker, and the same shape read back from Google, Graph,
/// EWS and CalDAV. A meeting provider wants an address and nothing else, so the
/// split has to happen before the list leaves the host: handing Webex the whole
/// display string puts `"Alice Smith <alice@example.test>"` in a field the API
/// validates as an address, and the meeting is refused outright.
///
/// Uses the same [`cal_core::attendee::parse`] every calendar adapter uses, so
/// there is one parser rather than five. Entries that yield no address at all —
/// a contact with only a name — are dropped: an address is what the provider
/// can act on, and a name in an address field reaches nobody.
pub fn attendee_addresses(attendees: &[String]) -> Vec<String> {
    attendees
        .iter()
        .map(|entry| cal_core::attendee::parse(entry).1)
        .map(|address| address.trim().to_string())
        .filter(|address| address.contains('@'))
        .collect()
}

/// Whether the videoconference provider should email the people invited to a
/// NEW meeting. Takes the bare addresses from [`attendee_addresses`].
///
/// Only when nothing else will. A calendar that can invite server-side is the
/// channel that should carry the invitation — it is the one whose replies come
/// back as RSVPs, and a provider mail alongside it delivers a SECOND iCalendar
/// attachment, which lands as a duplicate entry in every attendee's calendar.
///
/// When the calendar cannot invite — a local calendar, a subscribed feed, a
/// CalDAV server without RFC 6638 — the provider's mail is the only invitation
/// that exists, and suppressing it means nobody is told at all. That was the
/// state before this: the flag was an account checkbox, defaulted off for the
/// duplicate risk, and so the case where it was the ONLY channel was off too.
///
/// Nobody to notify means no mail, whatever the calendar can do. The caller
/// owes the complement: when this answers `false` because the calendar CAN
/// invite, the calendar write has to actually carry the invitation
/// (`Event::send_invitations`), or the two channels agree on silence and the
/// join link reaches nobody at all.
pub fn should_provider_notify(addresses: &[String], calendar_supports_scheduling: bool) -> bool {
    !addresses.is_empty() && !calendar_supports_scheduling
}

/// Whether the provider should email that a meeting is OFF.
///
/// The mirror of [`should_provider_notify`], and deliberately NOT conditioned
/// on the event's attendees — because on the way out they are the wrong list to
/// ask. Three reasons, each of which happens:
///
/// - The provider mails *its own* invitees, not the event's. A meeting adopted
///   from the provider's own web interface has invitees the event never heard
///   of, and telling them is the entire point.
/// - By removal time the event may be gone, or unreadable. An empty answer from
///   a failed cache read is indistinguishable from "nobody was invited", and
///   guessing "nobody" there means silence for people who were told.
/// - An empty invitee list costs nothing. Asking a provider to notify a meeting
///   nobody was invited to sends no mail, so the risk is one-sided.
///
/// What remains is the one fact that matters: can the calendar carry the
/// cancellation itself? If it can, it does, and a provider mail on top is the
/// contradictory second message. If it cannot, the provider's mail is the only
/// word the attendees will get, and without it they simply turn up.
pub fn should_provider_announce_removal(calendar_supports_scheduling: bool) -> bool {
    !calendar_supports_scheduling
}

/// The lines of the block Aperio writes into an event, with every label already
/// resolved into `lang`.
///
/// The adapter names each line and supplies the value; the plugin's own
/// catalogue supplies the words; this turns the pair into what
/// [`cal_core::conferencing::meeting_block`] renders. Nothing here knows a
/// provider — it knows that a meeting has labelled facts and that somebody has
/// to pick the language before they are frozen into somebody else's calendar.
///
/// `join_url` is the safety net: an adapter is *supposed* to list the link
/// first, and one that forgets would otherwise produce a block with no way in.
/// If no line carries it, it is prepended under a plain English label — worse
/// than the adapter doing it properly, far better than an invitation nobody can
/// accept.
pub fn block_lines(
    details: &[vc_core::JoinDetail],
    join_url: &str,
    catalogue: Option<&plugin_core::StringCatalogue>,
    lang: &str,
) -> Vec<(String, String)> {
    let mut lines: Vec<(String, String)> = details
        .iter()
        .filter(|detail| !detail.value.trim().is_empty())
        .map(|detail| {
            let label = plugin_core::resolve_label(
                catalogue,
                detail.label_key.as_deref(),
                &detail.label,
                lang,
            );
            (label.to_string(), detail.value.trim().to_string())
        })
        .collect();
    if !lines.iter().any(|(_, value)| value == join_url.trim()) {
        lines.insert(
            0,
            ("Join the meeting".to_string(), join_url.trim().to_string()),
        );
    }
    lines
}

#[derive(Debug, thiserror::Error)]
pub enum MeetingsError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Read/write access to the event↔meeting bindings.
pub struct MeetingsRepo<'a> {
    db: &'a SharedConn,
}

impl<'a> MeetingsRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// Record that `meeting_id` was created for `event_id`.
    ///
    /// An upsert: creating a second meeting for an event replaces the binding.
    /// The caller is responsible for deleting the previous meeting first if it
    /// wants to — dropping the row on its own would orphan the old meeting,
    /// which is exactly the failure this table exists to prevent.
    pub fn bind(
        &self,
        event_id: &str,
        account_id: &str,
        meeting_id: &str,
        join_url: &str,
    ) -> Result<EventMeeting, MeetingsError> {
        let created_at = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO event_meetings (event_id, account_id, meeting_id, join_url, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(event_id) DO UPDATE SET
                account_id = excluded.account_id,
                meeting_id = excluded.meeting_id,
                join_url   = excluded.join_url,
                created_at = excluded.created_at",
            params![event_id, account_id, meeting_id, join_url, created_at],
        )?;
        Ok(EventMeeting {
            event_id: event_id.to_string(),
            account_id: account_id.to_string(),
            meeting_id: meeting_id.to_string(),
            join_url: join_url.to_string(),
            created_at,
        })
    }

    /// The meeting bound to `event_id`, if Aperio created one.
    pub fn get(&self, event_id: &str) -> Result<Option<EventMeeting>, MeetingsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT event_id, account_id, meeting_id, join_url, created_at
               FROM event_meetings WHERE event_id = ?",
        )?;
        let row = stmt
            .query_row(params![event_id], |row| {
                Ok(EventMeeting {
                    event_id: row.get(0)?,
                    account_id: row.get(1)?,
                    meeting_id: row.get(2)?,
                    join_url: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    /// Forget the binding. Returns what was there, so a caller that is about to
    /// delete the event can take the meeting down with it in one step and know
    /// exactly which meeting it removed.
    /// The meeting bound to this event — or to any COPY of it.
    ///
    /// A meeting hangs on exactly one event, and which one is a coincidence of
    /// the moment it was attached. Before groups that was simply how it was:
    /// open the private copy and the Join button was missing, because the link
    /// had been made from the work copy. A group says those are one
    /// appointment, so its meeting is the appointment's, and it is reachable
    /// from whichever copy the user happens to be looking at.
    ///
    /// The binding still LIVES on one event. Moving the row to the group would
    /// mean a second table and a migration for something the lookup can answer
    /// — and it would have to decide what happens to the meeting when the
    /// group is dissolved, which nobody has asked for yet.
    ///
    /// Members are tried in the group's own order, so the answer is the same
    /// on every device. A group lookup that fails is treated as "not grouped":
    /// this is a read on the way to a Join button, and degrading to the plain
    /// binding is exactly what the app did before.
    pub fn get_including_copies(
        &self,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<Option<EventMeeting>, MeetingsError> {
        if let Some(own) = self.get(event_id)? {
            return Ok(Some(own));
        }
        let groups = crate::event_groups::EventGroupsRepo::new(self.db);
        let found = groups
            .groups_for_events(&[(calendar_id.to_string(), event_id.to_string())])
            .unwrap_or_default();
        for group in found {
            for member in &group.members {
                if member.event_id == event_id {
                    continue;
                }
                if let Some(meeting) = self.get(&member.event_id)? {
                    return Ok(Some(meeting));
                }
            }
        }
        Ok(None)
    }

    pub fn unbind(&self, event_id: &str) -> Result<Option<EventMeeting>, MeetingsError> {
        let existing = self.get(event_id)?;
        if existing.is_some() {
            let conn = self.db.lock().expect("db mutex poisoned");
            conn.execute(
                "DELETE FROM event_meetings WHERE event_id = ?",
                params![event_id],
            )?;
        }
        Ok(existing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    fn fresh() -> (tempfile::TempDir, DbHandle) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = DbHandle::open(dir.path().join("test.sqlite")).expect("open");
        (dir, db)
    }

    #[test]
    fn a_binding_round_trips() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = MeetingsRepo::new(&shared);
        assert_eq!(repo.get("ev-1").unwrap(), None);

        let bound = repo
            .bind(
                "ev-1",
                "acc-1",
                "m-123",
                "https://example.webex.com/j.php?MTID=m1",
            )
            .unwrap();
        let read = repo.get("ev-1").unwrap().expect("bound");
        assert_eq!(read, bound);
        assert_eq!(read.meeting_id, "m-123");
    }

    #[test]
    fn binding_again_replaces_rather_than_duplicating() {
        // One event, one meeting. A second create for the same event means the
        // first was replaced — the caller is expected to have deleted it, and
        // keeping both rows would only hide that it did not.
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = MeetingsRepo::new(&shared);
        repo.bind("ev-1", "acc-1", "m-1", "https://a.test/1")
            .unwrap();
        repo.bind("ev-1", "acc-2", "m-2", "https://a.test/2")
            .unwrap();
        let read = repo.get("ev-1").unwrap().unwrap();
        assert_eq!(read.meeting_id, "m-2");
        assert_eq!(read.account_id, "acc-2");
    }

    #[test]
    fn unbind_reports_what_it_removed_so_the_meeting_can_be_taken_down() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = MeetingsRepo::new(&shared);
        repo.bind("ev-1", "acc-1", "m-1", "https://a.test/1")
            .unwrap();

        let removed = repo.unbind("ev-1").unwrap().expect("was bound");
        assert_eq!(removed.meeting_id, "m-1");
        assert_eq!(repo.get("ev-1").unwrap(), None);
        // Idempotent: deleting an event twice must not look like a meeting to
        // delete twice.
        assert_eq!(repo.unbind("ev-1").unwrap(), None);
    }

    #[test]
    fn an_event_without_a_meeting_is_simply_absent() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = MeetingsRepo::new(&shared);
        assert_eq!(repo.get("never-had-one").unwrap(), None);
        assert_eq!(repo.unbind("never-had-one").unwrap(), None);
    }

    // ── should_provider_notify ──────────────────────────────────────────────

    #[test]
    fn the_provider_mails_only_when_the_calendar_cannot() {
        let guests = vec!["a@example.test".to_string()];
        // The calendar will invite: a provider mail on top is the duplicate.
        assert!(!should_provider_notify(&guests, true));
        // The calendar cannot: the provider's mail is the only invitation.
        assert!(should_provider_notify(&guests, false));
    }

    #[test]
    fn nobody_to_notify_means_no_mail_either_way() {
        assert!(!should_provider_notify(&[], false));
        assert!(!should_provider_notify(&[], true));
    }

    // ── attendee_addresses ──────────────────────────────────────────────────

    #[test]
    fn a_display_name_never_reaches_the_provider_as_an_address() {
        // The contact picker's own output. Handing this to a meeting provider
        // verbatim puts a display string in a field the API validates as an
        // address — Webex refuses the whole meeting.
        let picked = vec![
            "Alice Smith <alice@example.test>".to_string(),
            "  bob@example.test  ".to_string(),
            "\"Quoted Name\" <carol@example.test>".to_string(),
        ];
        assert_eq!(
            attendee_addresses(&picked),
            vec![
                "alice@example.test".to_string(),
                "bob@example.test".to_string(),
                "carol@example.test".to_string(),
            ]
        );
    }

    #[test]
    fn an_entry_with_no_address_is_dropped_rather_than_mailed_into_the_void() {
        // A contact with a name and no address, and plain whitespace. Neither
        // reaches anybody, so neither should count as a guest either.
        let entries = vec!["Alice Smith".to_string(), "   ".to_string(), String::new()];
        assert!(attendee_addresses(&entries).is_empty());
        assert!(!should_provider_notify(
            &attendee_addresses(&entries),
            false
        ));
    }

    // ── should_provider_announce_removal ────────────────────────────────────

    #[test]
    fn removal_asks_only_whether_the_calendar_can_cancel() {
        // Deliberately not conditioned on the event's attendees: an adopted
        // meeting has invitees the event never knew, and by removal time the
        // event may be gone. The provider mails only who it holds, so asking
        // costs nothing when it holds nobody.
        assert!(should_provider_announce_removal(false));
        assert!(!should_provider_announce_removal(true));
    }
}
