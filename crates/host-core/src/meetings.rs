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
}
