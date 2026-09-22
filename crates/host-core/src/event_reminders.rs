//! Reminders Aperio keeps for one event and tells no provider about
//! (migration `0043_event_local_reminders.sql`).
//!
//! A reminder normally rides ON the event, where every client of the calendar
//! can see it. These stay here: they fire in Aperio, travel to the user's
//! other devices through Aperio's own sync, and reach nobody else on a shared
//! calendar. The scheduler folds them in beside the event's own reminders
//! (see [`crate::reminders::effective_reminders`]).
//!
//! Writes travel — the command layer emits `SyncEvent::EventLocalRemindersSet`
//! after calling [`EventRemindersRepo::set`], the way the event-group commands
//! do. Repairs do NOT: see [`EventRemindersRepo::heal`].

use cal_core::{EventLocalReminders, Reminder};
use rusqlite::{params, OptionalExtension};
use thiserror::Error;

use crate::db::SharedConn;

#[derive(Debug, Error)]
pub enum EventRemindersError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("stored reminders are not readable: {0}")]
    Decode(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, EventRemindersError>;

/// Read/write access to the per-event Aperio-only reminders.
pub struct EventRemindersRepo<'a> {
    db: &'a SharedConn,
}

const SELECT: &str = "SELECT calendar_id, event_id, reminders, title, starts_at, updated_at \
     FROM event_local_reminders";

fn row_to_value(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, String, String)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn decode(
    (calendar_id, event_id, reminders, title, starts_at, updated_at): (
        String,
        String,
        String,
        String,
        String,
        String,
    ),
) -> EventLocalReminders {
    EventLocalReminders {
        calendar_id,
        event_id,
        // A row we cannot read is a row that asks for nothing. Dropping the
        // whole row instead would hide it from the editor, so the user could
        // not repair it either.
        reminders: serde_json::from_str::<Vec<Reminder>>(&reminders).unwrap_or_default(),
        title,
        starts_at,
        updated_at,
    }
}

impl<'a> EventRemindersRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// Every row. There is one per event the user gave a private reminder, so
    /// the whole set is small — the same call shape `list_color_overrides`
    /// has, and the scheduler needs them all at once anyway.
    pub fn list(&self) -> Result<Vec<EventLocalReminders>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(SELECT)?;
        let rows = stmt.query_map([], row_to_value)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(decode(row?));
        }
        Ok(out)
    }

    /// One event's row, or `None` when it never had one.
    pub fn get(&self, calendar_id: &str, event_id: &str) -> Result<Option<EventLocalReminders>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(&format!("{SELECT} WHERE calendar_id = ? AND event_id = ?"))?;
        let mut rows = stmt.query_map(params![calendar_id, event_id], row_to_value)?;
        match rows.next() {
            Some(row) => Ok(Some(decode(row?))),
            None => Ok(None),
        }
    }

    /// Write one event's private reminders, signature and all.
    ///
    /// An empty list is stored, not deleted: it is the record of a decision,
    /// and the last writer needs something to win against (see the migration).
    /// The caller emits the sync event afterwards.
    ///
    /// The signature (`title`, `starts_at`) is that of the event the key NAMES.
    /// A list the user emptied keeps it, so the decision follows the event when
    /// another device finds its id reminted. A key its event LEFT — for a new
    /// id, or another calendar — names nothing any more and has none: see
    /// [`Self::retire`].
    pub fn set(
        &self,
        calendar_id: &str,
        event_id: &str,
        reminders: &[Reminder],
        title: &str,
        starts_at: &str,
        updated_at: &str,
    ) -> Result<EventLocalReminders> {
        let encoded = serde_json::to_string(reminders)?;
        {
            let conn = self.db.lock().expect("db mutex poisoned");
            conn.execute(
                "INSERT INTO event_local_reminders
                     (calendar_id, event_id, reminders, title, starts_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(calendar_id, event_id) DO UPDATE SET
                     reminders  = excluded.reminders,
                     title      = excluded.title,
                     starts_at  = excluded.starts_at,
                     updated_at = excluded.updated_at",
                params![calendar_id, event_id, encoded, title, starts_at, updated_at],
            )?;
        }
        Ok(EventLocalReminders {
            calendar_id: calendar_id.to_string(),
            event_id: event_id.to_string(),
            reminders: reminders.to_vec(),
            title: title.to_string(),
            starts_at: starts_at.to_string(),
            updated_at: updated_at.to_string(),
        })
    }

    /// Point a row at the id its event carries now.
    ///
    /// Ids belong to the provider and change underneath us — a re-bootstrap
    /// remints them, moving an event between calendars remints it, Exchange
    /// does it unprompted. The SIGNATURE stored with each row exists so the
    /// event can be found again; this writes down what was found.
    ///
    /// `updated_at` deliberately does NOT move, and nothing is emitted — the
    /// same rule [`crate::event_groups::EventGroupsRepo::heal_member`] follows,
    /// for the same two reasons: a repair stamped "now" would outrank another
    /// device's real decision, and two devices whose caches disagree about an
    /// id would heal each other back and forth without end. Every device sees
    /// the same events and repairs its own copy.
    ///
    /// Returns whether a row moved. A row already gone is not an error: two
    /// views healing the same thing at once is ordinary.
    pub fn heal(&self, calendar_id: &str, old_event_id: &str, new_event_id: &str) -> Result<bool> {
        if old_event_id == new_event_id {
            return Ok(false);
        }
        let mut conn = self.db.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        // Both ids name the SAME event — the caller only heals after matching
        // the signature to exactly one — so exactly one row may survive. A row
        // already sitting on the new id is the decision made under the id the
        // event carries NOW, so it wins on equal footing and the later write
        // wins outright; either way the stale row goes, or a snapshot that
        // still carries it would resurrect a duplicate the repair then refuses
        // to touch.
        let incumbent: Option<String> = tx
            .query_row(
                "SELECT updated_at FROM event_local_reminders
                  WHERE calendar_id = ? AND event_id = ?",
                params![calendar_id, new_event_id],
                |row| row.get(0),
            )
            .optional()?;
        let old: Option<String> = tx
            .query_row(
                "SELECT updated_at FROM event_local_reminders
                  WHERE calendar_id = ? AND event_id = ?",
                params![calendar_id, old_event_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(old_updated) = old else {
            tx.commit()?;
            return Ok(false);
        };
        let moved = match incumbent {
            // Nothing in the way: the row simply follows its event.
            None => {
                tx.execute(
                    "UPDATE event_local_reminders
                        SET event_id = ?
                      WHERE calendar_id = ? AND event_id = ?",
                    params![new_event_id, calendar_id, old_event_id],
                )?;
                true
            }
            // Two rows for one event: keep the later decision, drop the other.
            Some(new_updated) => {
                if old_updated > new_updated {
                    tx.execute(
                        "DELETE FROM event_local_reminders
                          WHERE calendar_id = ? AND event_id = ?",
                        params![calendar_id, new_event_id],
                    )?;
                    tx.execute(
                        "UPDATE event_local_reminders
                            SET event_id = ?
                          WHERE calendar_id = ? AND event_id = ?",
                        params![new_event_id, calendar_id, old_event_id],
                    )?;
                    true
                } else {
                    tx.execute(
                        "DELETE FROM event_local_reminders
                          WHERE calendar_id = ? AND event_id = ?",
                        params![calendar_id, old_event_id],
                    )?;
                    false
                }
            }
        };
        tx.commit()?;
        Ok(moved)
    }

    /// Write down what the event looks like NOW, so the signature keeps
    /// matching after the user moves or renames the appointment.
    ///
    /// Local and silent for the same reason as [`Self::heal`]: every device
    /// sees the same event and refreshes its own copy.
    pub fn refresh_signature(
        &self,
        calendar_id: &str,
        event_id: &str,
        title: &str,
        starts_at: &str,
    ) -> Result<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "UPDATE event_local_reminders
                SET title = ?, starts_at = ?
              WHERE calendar_id = ? AND event_id = ?",
            params![title, starts_at, calendar_id, event_id],
        )?;
        Ok(())
    }

    /// Carry a row across a calendar MOVE.
    ///
    /// Unlike [`Self::heal`] this is not a repair every device could derive
    /// for itself: only the device that made the move knows both ends, and
    /// the repair in the reminder scan cannot help — it never looks outside
    /// one calendar. So the caller EMITS the result, and `updated_at` moves.
    ///
    /// The old row is EMPTIED rather than deleted, for the reason the
    /// migration gives: a peer that still holds the old list has to have
    /// something to lose against, or it would go on ringing for an
    /// appointment that is no longer there. It is retired without a signature
    /// ([`Self::retire`]). A list the user had emptied moves like any other.
    ///
    /// Returns both rows to emit, or `None` when the key did not change or the
    /// event had no row.
    pub fn relocate(
        &self,
        old_calendar_id: &str,
        old_event_id: &str,
        new_calendar_id: &str,
        new_event_id: &str,
        updated_at: &str,
    ) -> Result<Option<(EventLocalReminders, EventLocalReminders)>> {
        if (old_calendar_id, old_event_id) == (new_calendar_id, new_event_id) {
            return Ok(None);
        }
        let Some(row) = self.get(old_calendar_id, old_event_id)? else {
            return Ok(None);
        };
        // An emptied list moves too: it is the user's decision about this
        // event, and it must not stay behind signed in the old calendar, where
        // the repair would carry it onto a copy with the same title and start.
        let moved = self.set(
            new_calendar_id,
            new_event_id,
            &row.reminders,
            &row.title,
            &row.starts_at,
            updated_at,
        )?;
        let emptied = self.retire(old_calendar_id, old_event_id, updated_at)?;
        Ok(Some((moved, emptied)))
    }

    /// Empty the row of a key its event left — the provider minted a new id
    /// (Exchange does on every save), or the event moved to another calendar
    /// — and give it NO signature.
    ///
    /// The emptied list is still the record of a decision: a peer that holds
    /// the old list under this key loses to it and stops firing. But it must
    /// never be found again. With a signature, the reminder scan's repair
    /// (`cal_core::plan_repairs`) would move the row onto whatever event in
    /// that calendar carries the title and start — the very event that left
    /// the key, under its new id, or a twin once that event changes or goes —
    /// where, as the later write, it would replace that event's reminders with
    /// nothing. A row without a signature is never repaired.
    ///
    /// The row stays, inert: nothing fires from an empty list, and the scan
    /// skips it.
    pub fn retire(
        &self,
        calendar_id: &str,
        event_id: &str,
        updated_at: &str,
    ) -> Result<EventLocalReminders> {
        self.set(calendar_id, event_id, &[], "", "", updated_at)
    }

    /// Retire the row of an event that was deleted, for the caller to emit.
    ///
    /// An orphan here is not inert, unlike a leftover colour override: the
    /// repair in the reminder scan repoints a row whose id it cannot find onto
    /// the one event of that calendar sharing its title and start. A row left
    /// behind by a delete could therefore start ringing on a DIFFERENT
    /// appointment — a copy of a deleted meeting, say. Dropping it here did
    /// not reach the other devices: each kept the row, signed, for its own
    /// scan to repair. So it is retired ([`Self::retire`]) and the caller
    /// emits it — the later write on every device that holds this key, and
    /// never repaired.
    ///
    /// Only THIS key. An Exchange id changes with every edit, made anywhere,
    /// and each device's scan moves its row to the new id silently; a device
    /// that still holds the event under an older id keeps that row, signed.
    /// Retiring by signature instead would hit a twin's own row just as well.
    /// See TODO.md, 119.
    ///
    /// Returns the row to emit, or `None` when the event had none, or only a
    /// retired one: that has already been said.
    pub fn forget_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        updated_at: &str,
    ) -> Result<Option<EventLocalReminders>> {
        match self.get(calendar_id, event_id)? {
            None => Ok(None),
            Some(row) if row.is_empty() && row.title.is_empty() && row.starts_at.is_empty() => {
                Ok(None)
            }
            Some(_) => self.retire(calendar_id, event_id, updated_at).map(Some),
        }
    }

    /// Drop the rows of a calendar that is gone. Called when a calendar is
    /// deleted, so private reminders don't outlive the events they name.
    pub fn forget_calendar(&self, calendar_id: &str) -> Result<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "DELETE FROM event_local_reminders WHERE calendar_id = ?",
            params![calendar_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use cal_core::ReminderKind;

    fn db() -> SharedConn {
        DbHandle::open_in_memory().expect("in-memory db").shared()
    }

    fn rel(minutes: i64) -> Reminder {
        Reminder {
            kind: ReminderKind::Relative {
                minutes_before: minutes,
            },
            sound: None,
        }
    }

    #[test]
    fn a_row_round_trips_with_its_signature() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set(
            "cal",
            "ev",
            &[rel(60)],
            "Zahnarzt",
            "2026-06-15T09:00:00Z",
            "2026-06-01T10:00:00Z",
        )
        .unwrap();
        let row = repo.get("cal", "ev").unwrap().expect("row is there");
        assert_eq!(row.reminders, vec![rel(60)]);
        assert_eq!(row.title, "Zahnarzt");
        assert_eq!(row.starts_at, "2026-06-15T09:00:00Z");
        assert_eq!(repo.list().unwrap().len(), 1);
    }

    /// An emptied list stays as a row: it is the record of a decision, and a
    /// peer that has not heard yet must have something to lose against.
    #[test]
    fn an_emptied_list_is_stored_not_deleted() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("cal", "ev", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        repo.set("cal", "ev", &[], "T", "S", "2026-06-02T10:00:00Z")
            .unwrap();
        let row = repo.get("cal", "ev").unwrap().expect("row survives");
        assert!(row.is_empty());
        assert_eq!(row.updated_at, "2026-06-02T10:00:00Z");
    }

    /// A key that names no event any more is emptied without a signature, so
    /// the repair never finds it again; a list the user emptied for the event
    /// the key still names keeps its own.
    #[test]
    fn a_retired_key_carries_no_signature() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("cal", "old", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        let retired = repo.retire("cal", "old", "2026-06-02T10:00:00Z").unwrap();
        assert!(retired.is_empty());
        assert_eq!(
            (retired.title.as_str(), retired.starts_at.as_str()),
            ("", "")
        );
        let row = repo.get("cal", "old").unwrap().expect("the record stays");
        assert_eq!((row.title.as_str(), row.starts_at.as_str()), ("", ""));
        assert_eq!(row.updated_at, "2026-06-02T10:00:00Z");

        repo.set("cal", "kept", &[], "T", "S", "2026-06-02T10:00:00Z")
            .unwrap();
        let kept = repo.get("cal", "kept").unwrap().unwrap();
        assert_eq!((kept.title.as_str(), kept.starts_at.as_str()), ("T", "S"));
    }

    /// A list the user emptied moves with its event, and the old key is
    /// retired like any other.
    #[test]
    fn relocating_carries_an_emptied_list() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("home", "ev", &[], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        let (moved, emptied) = repo
            .relocate("home", "ev", "work", "ev2", "2026-06-02T10:00:00Z")
            .unwrap()
            .expect("an emptied list is carried too");
        assert!(moved.is_empty());
        assert_eq!((moved.title.as_str(), moved.starts_at.as_str()), ("T", "S"));
        assert_eq!(
            (emptied.title.as_str(), emptied.starts_at.as_str()),
            ("", "")
        );
    }

    /// A move to another calendar leaves the old key retired: emptied, and
    /// without the signature that would let the repair find an event there
    /// again — a copy left in the old calendar with the same title and start
    /// would otherwise have its own reminders emptied.
    #[test]
    fn relocating_retires_the_old_key() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("home", "ev", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        let (moved, emptied) = repo
            .relocate("home", "ev", "work", "ev2", "2026-06-02T10:00:00Z")
            .unwrap()
            .expect("there was a row to move");
        assert_eq!(moved.reminders, vec![rel(60)]);
        assert_eq!((moved.title.as_str(), moved.starts_at.as_str()), ("T", "S"));
        assert!(emptied.is_empty());
        assert_eq!(
            (emptied.title.as_str(), emptied.starts_at.as_str()),
            ("", "")
        );
    }

    /// The whole point of the signature: the provider reminted the id and the
    /// row follows, without its timestamp moving.
    #[test]
    fn healing_repoints_a_row_without_touching_its_timestamp() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("cal", "old", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        assert!(repo.heal("cal", "old", "new").unwrap());
        assert!(repo.get("cal", "old").unwrap().is_none());
        let row = repo.get("cal", "new").unwrap().expect("moved");
        assert_eq!(row.reminders, vec![rel(60)]);
        assert_eq!(row.updated_at, "2026-06-01T10:00:00Z");
        // Nothing to move twice, and no error for trying.
        assert!(!repo.heal("cal", "old", "new").unwrap());
    }

    /// Two rows for ONE event: the later decision wins and the other GOES.
    /// Leaving it behind would let a snapshot that still carries it resurrect
    /// a duplicate the repair then refuses to touch.
    #[test]
    fn healing_keeps_the_later_decision_and_drops_the_other() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        // The row under the id the event carries NOW is the newer one.
        repo.set("cal", "old", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        repo.set("cal", "new", &[rel(5)], "T", "S", "2026-06-03T10:00:00Z")
            .unwrap();
        assert!(!repo.heal("cal", "old", "new").unwrap());
        assert_eq!(
            repo.get("cal", "new").unwrap().unwrap().reminders,
            vec![rel(5)]
        );
        assert!(repo.get("cal", "old").unwrap().is_none());
    }

    /// The other way round: the row that was waiting to be found is the newer
    /// decision, so it takes the id and the stale one goes.
    #[test]
    fn healing_lets_the_waiting_row_win_when_it_is_newer() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("cal", "old", &[rel(60)], "T", "S", "2026-06-05T10:00:00Z")
            .unwrap();
        repo.set("cal", "new", &[rel(5)], "T", "S", "2026-06-03T10:00:00Z")
            .unwrap();
        assert!(repo.heal("cal", "old", "new").unwrap());
        assert_eq!(
            repo.get("cal", "new").unwrap().unwrap().reminders,
            vec![rel(60)]
        );
        assert!(repo.get("cal", "old").unwrap().is_none());
    }

    /// A row that is gone is not an error: two views healing at once is
    /// ordinary.
    #[test]
    fn healing_a_row_that_is_not_there_says_so() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        assert!(!repo.heal("cal", "nothing", "new").unwrap());
    }

    /// A deleted event's row is retired, not dropped: emptied, so it wins over
    /// a peer's copy, and unsigned, so no scan anywhere re-points it at a
    /// different appointment.
    #[test]
    fn a_deleted_event_retires_its_row() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("cal", "gone", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        repo.set("cal", "kept", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        let retired = repo
            .forget_event("cal", "gone", "2026-06-02T10:00:00Z")
            .unwrap()
            .expect("a row to emit");
        assert!(retired.is_empty());
        assert_eq!(
            (retired.title.as_str(), retired.starts_at.as_str()),
            ("", "")
        );
        assert_eq!(retired.updated_at, "2026-06-02T10:00:00Z");
        assert_eq!(repo.get("cal", "gone").unwrap(), Some(retired));
        assert_eq!(
            repo.get("cal", "kept").unwrap().unwrap().reminders,
            vec![rel(60)]
        );
    }

    /// Nothing to say, nothing written: a delete of an event without private
    /// reminders — most of them — leaves no row behind, and a key already
    /// retired is not retired again.
    #[test]
    fn forgetting_an_event_without_a_live_row_writes_nothing() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        assert_eq!(
            repo.forget_event("cal", "never", "2026-06-02T10:00:00Z")
                .unwrap(),
            None
        );
        assert!(repo.get("cal", "never").unwrap().is_none());

        repo.retire("cal", "moved", "2026-06-01T10:00:00Z").unwrap();
        assert_eq!(
            repo.forget_event("cal", "moved", "2026-06-02T10:00:00Z")
                .unwrap(),
            None
        );
        assert_eq!(
            repo.get("cal", "moved").unwrap().unwrap().updated_at,
            "2026-06-01T10:00:00Z"
        );
    }

    /// A list the user emptied is still a live decision about the event: its
    /// delete retires it like any other.
    #[test]
    fn a_deleted_event_retires_an_emptied_list_too() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("cal", "ev", &[], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        let retired = repo
            .forget_event("cal", "ev", "2026-06-02T10:00:00Z")
            .unwrap()
            .expect("an emptied list is signed, so it is retired");
        assert_eq!(
            (retired.title.as_str(), retired.starts_at.as_str()),
            ("", "")
        );
    }

    #[test]
    fn a_refreshed_signature_follows_the_appointment() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set(
            "cal",
            "ev",
            &[rel(60)],
            "Alt",
            "2026-06-15T09:00:00Z",
            "2026-06-01T10:00:00Z",
        )
        .unwrap();
        repo.refresh_signature("cal", "ev", "Neu", "2026-06-16T09:00:00Z")
            .unwrap();
        let row = repo.get("cal", "ev").unwrap().unwrap();
        assert_eq!(row.title, "Neu");
        assert_eq!(row.starts_at, "2026-06-16T09:00:00Z");
        assert_eq!(row.updated_at, "2026-06-01T10:00:00Z");
    }

    #[test]
    fn a_deleted_calendar_takes_its_rows_with_it() {
        let db = db();
        let repo = EventRemindersRepo::new(&db);
        repo.set("gone", "ev", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        repo.set("kept", "ev", &[rel(60)], "T", "S", "2026-06-01T10:00:00Z")
            .unwrap();
        repo.forget_calendar("gone").unwrap();
        assert!(repo.get("gone", "ev").unwrap().is_none());
        assert!(repo.get("kept", "ev").unwrap().is_some());
    }
}
