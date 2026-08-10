//! Which events mean the same appointment.
//!
//! The same commitment routinely exists several times over — in the work
//! calendar so colleagues see it, copied into a private calendar because that
//! is the one a voice assistant reads out, and again in each colleague's
//! calendar Aperio also reads. To every provider those are unrelated events.
//! A group is Aperio's statement that they are not. See
//! `DESIGN-event-groups.md`.
//!
//! Nothing here reaches a provider, and nothing here is a calendar operation:
//! grouping two events changes neither of them. The group is a fact kept beside
//! them, and removing it leaves both exactly as they were.
//!
//! Ids are the SERIES MASTER's, matching `event_meetings` and
//! `event_color_overrides`: a recurring appointment is grouped as a series.
//! Grouping single occurrences is a different feature and deliberately not this
//! one.

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;

use cal_core::{EventGroup, EventGroupMember, SuggestionDecline};

use crate::db::SharedConn;

/// What a caller hands over to put an event in a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMember {
    pub calendar_id: String,
    pub event_id: String,
    pub title: String,
    pub starts_at: String,
}

/// What taking an event out of its group did.
///
/// The caller needs to tell the two apart because they sync differently: a
/// group that still stands travels as its new membership, a dissolved one as
/// its id. Collapsing both into `Option<EventGroup>` loses exactly the id the
/// dissolve event needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ungrouped {
    /// The group stands, with the members left in it.
    Remains(EventGroup),
    /// Fewer than two were left, so the group is gone.
    Dissolved { group_id: String },
}

#[derive(Debug, thiserror::Error)]
pub enum EventGroupsError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Fewer than two events cannot mean "the same appointment" as each other.
    #[error("a group needs at least two events")]
    TooFewMembers,
    /// The events named already belong to different groups. Merging them is a
    /// decision with consequences the caller has to make deliberately, not
    /// something to infer from an ambiguous request.
    #[error("those events are already in different groups")]
    ConflictingGroups,
    /// A group that was written moments ago in the same transaction could not
    /// be read back. Not reachable as far as anyone knows — it exists so the
    /// code can say so instead of panicking, which would poison the database
    /// mutex and take the whole process's storage with it.
    #[error("the group vanished while it was being written")]
    Vanished,
}

/// Remember that this device dissolved a group, and when.
///
/// The mark has to outlive the row: another device may still be holding an
/// UPDATE it wrote before it heard about the dissolve, and when that arrives
/// to an empty table there is nothing left to compare it against — so it
/// re-creates the group. See migration 0036.
fn mark_dissolved(
    conn: &rusqlite::Connection,
    group_id: &str,
    dissolved_at: &str,
) -> Result<(), EventGroupsError> {
    conn.execute(
        "INSERT INTO event_group_tombstones (group_id, dissolved_at)
         VALUES (?, ?)
         ON CONFLICT(group_id) DO UPDATE SET dissolved_at = excluded.dissolved_at",
        params![group_id, dissolved_at],
    )?;
    Ok(())
}

/// Insert a decline, idempotently. A set that only ever grows: two devices
/// declining different pairs converge by union, and there is nothing for a
/// last-writer rule to decide.
fn write_decline(
    conn: &rusqlite::Connection,
    decline: &SuggestionDecline,
) -> Result<(), EventGroupsError> {
    conn.execute(
        "INSERT INTO event_group_suggestion_declines
             (calendar_a, event_a, calendar_b, event_b, declined_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(calendar_a, event_a, calendar_b, event_b) DO NOTHING",
        params![
            decline.calendar_a,
            decline.event_a,
            decline.calendar_b,
            decline.event_b,
            decline.declined_at
        ],
    )?;
    Ok(())
}

fn read_declines(conn: &rusqlite::Connection) -> Result<Vec<SuggestionDecline>, EventGroupsError> {
    let mut stmt = conn.prepare(
        "SELECT calendar_a, event_a, calendar_b, event_b, declined_at
           FROM event_group_suggestion_declines",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SuggestionDecline {
                calendar_a: row.get(0)?,
                event_a: row.get(1)?,
                calendar_b: row.get(2)?,
                event_b: row.get(3)?,
                declined_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read one group and its members.
///
/// Takes a plain `&Connection` so a transaction can use it too: a caller that
/// has just written the group reads it back before letting go of the lock,
/// rather than committing and hoping it is still there.
fn read_group(
    conn: &rusqlite::Connection,
    group_id: &str,
) -> Result<Option<EventGroup>, EventGroupsError> {
    let head: Option<(String, String)> = conn
        .query_row(
            "SELECT created_at, updated_at FROM event_groups WHERE id = ?",
            params![group_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((created_at, updated_at)) = head else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT calendar_id, event_id, title, starts_at, added_at
           FROM event_group_members WHERE group_id = ?
          ORDER BY added_at, event_id",
    )?;
    let members = stmt
        .query_map(params![group_id], |row| {
            Ok(EventGroupMember {
                calendar_id: row.get(0)?,
                event_id: row.get(1)?,
                title: row.get(2)?,
                starts_at: row.get(3)?,
                added_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // Fewer than two members is not a group, and is not shown.
    //
    // Not deleted, though — the sync applier keeps a group that lost members
    // to a newer claim, precisely so it does not forget what it had won.
    // Deleting it made the outcome depend on the order claims arrived in (see
    // `upsert_event_group_from_sync`). So the rule lives on the READ side: the
    // row stays as the record of a claim that lost, and nobody is told about a
    // group of one.
    if members.len() < 2 {
        return Ok(None);
    }
    Ok(Some(EventGroup {
        id: group_id.to_string(),
        created_at,
        updated_at,
        members,
    }))
}

/// Read/write access to event groups.
pub struct EventGroupsRepo<'a> {
    db: &'a SharedConn,
}

impl<'a> EventGroupsRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// Put these events in one group, and return it.
    ///
    /// Joins an existing group when exactly one of the members is already in
    /// one — the natural "and this one too" — and refuses when two of them are
    /// in DIFFERENT groups, because merging two claims about what an
    /// appointment is cannot be inferred from a request that did not ask for
    /// it.
    ///
    /// Idempotent for members already in the group it lands on: grouping the
    /// same pair twice is not an error, it is a no-op with the same answer.
    pub fn group(&self, members: &[NewMember]) -> Result<EventGroup, EventGroupsError> {
        if members.len() < 2 {
            return Err(EventGroupsError::TooFewMembers);
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.db.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;

        // Which groups the named events already belong to.
        let mut existing: Vec<String> = Vec::new();
        for m in members {
            let found: Option<String> = tx
                .query_row(
                    "SELECT group_id FROM event_group_members
                      WHERE calendar_id = ? AND event_id = ?",
                    params![m.calendar_id, m.event_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = found {
                if !existing.contains(&id) {
                    existing.push(id);
                }
            }
        }
        if existing.len() > 1 {
            return Err(EventGroupsError::ConflictingGroups);
        }

        let group_id = match existing.first() {
            Some(id) => {
                tx.execute(
                    "UPDATE event_groups SET updated_at = ? WHERE id = ?",
                    params![now, id],
                )?;
                id.clone()
            }
            None => {
                // A fresh id every time, so a group the user re-creates after
                // dissolving one is a new group and its predecessor's
                // tombstone (migration 0036) has nothing to say about it.
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO event_groups (id, created_at, updated_at) VALUES (?, ?, ?)",
                    params![id, now, now],
                )?;
                id
            }
        };

        for m in members {
            // The signature is refreshed on re-add rather than kept from the
            // first time: the point of it is to find the event again, and the
            // most recent title and start are the best chance of that.
            tx.execute(
                "INSERT INTO event_group_members
                     (group_id, calendar_id, event_id, title, starts_at, added_at)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(calendar_id, event_id) DO UPDATE SET
                     group_id  = excluded.group_id,
                     title     = excluded.title,
                     starts_at = excluded.starts_at",
                params![
                    group_id,
                    m.calendar_id,
                    m.event_id,
                    m.title,
                    m.starts_at,
                    now
                ],
            )?;
        }
        // Read the answer back INSIDE the transaction. Committing first and
        // re-reading afterwards means letting go of the write lock in between,
        // where another thread's dissolve can land — and the `expect` that
        // followed would then panic, poisoning the mutex and taking every
        // later database call in the process down with it. A row we just wrote
        // and have not yet released cannot be missing.
        let group = read_group(&tx, &group_id)?.ok_or(EventGroupsError::Vanished)?;
        tx.commit()?;
        Ok(group)
    }

    /// One group with its members, or `None` when the id is unknown.
    pub fn get(&self, group_id: &str) -> Result<Option<EventGroup>, EventGroupsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        read_group(&conn, group_id)
    }

    /// The group each of these events belongs to, keyed by `(calendar_id,
    /// event_id)`. Events in no group are simply absent.
    ///
    /// The lookup a calendar view makes once per rendered range, rather than
    /// once per event: a day with twenty events should cost one query.
    pub fn for_events(
        &self,
        events: &[(String, String)],
    ) -> Result<HashMap<(String, String), String>, EventGroupsError> {
        let mut out = HashMap::new();
        if events.is_empty() {
            return Ok(out);
        }
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT group_id FROM event_group_members WHERE calendar_id = ? AND event_id = ?",
        )?;
        for (calendar_id, event_id) in events {
            let found: Option<String> = stmt
                .query_row(params![calendar_id, event_id], |row| row.get(0))
                .optional()?;
            if let Some(group_id) = found {
                out.insert((calendar_id.clone(), event_id.clone()), group_id);
            }
        }
        drop(stmt);
        // Same rule as `get`: a row may still name a group that lost its other
        // members to a newer claim, and that is not a group anyone should be
        // told about.
        let mut sizes =
            conn.prepare("SELECT COUNT(*) FROM event_group_members WHERE group_id = ?")?;
        let mut lonely: Vec<(String, String)> = Vec::new();
        for (key, group_id) in &out {
            let count: i64 = sizes.query_row(params![group_id], |row| row.get(0))?;
            if count < 2 {
                lonely.push(key.clone());
            }
        }
        for key in lonely {
            out.remove(&key);
        }
        Ok(out)
    }

    /// Every group any of these events belongs to, each once.
    ///
    /// What a calendar view asks for: it holds a rendered range of events and
    /// needs the groups behind them, including the members that fall outside
    /// the range — a group is only readable as a whole ("this and three
    /// others"), so returning the touched groups entire is the answer, not the
    /// membership rows that happen to be on screen.
    pub fn groups_for_events(
        &self,
        events: &[(String, String)],
    ) -> Result<Vec<EventGroup>, EventGroupsError> {
        let mut ids: Vec<String> = Vec::new();
        for group_id in self.for_events(events)?.into_values() {
            if !ids.contains(&group_id) {
                ids.push(group_id);
            }
        }
        let mut out = Vec::new();
        for id in ids {
            if let Some(group) = self.get(&id)? {
                out.push(group);
            }
        }
        Ok(out)
    }

    /// Follow an event that moved to another calendar (or was re-created
    /// there under a new id).
    ///
    /// Moving a copy between calendars does not stop it meaning the same
    /// appointment — that is the whole point of the group. But membership is
    /// keyed by (calendar, event), and a cross-adapter move re-creates the
    /// event with a new id, so without this the row is left pointing at
    /// something that no longer exists and the group quietly shrinks.
    ///
    /// `None` when the event was not grouped. Returns the group afterwards so
    /// the caller can tell the other devices.
    pub fn relocate(
        &self,
        old_calendar_id: &str,
        old_event_id: &str,
        new_calendar_id: &str,
        new_event_id: &str,
    ) -> Result<Option<EventGroup>, EventGroupsError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.db.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        let group_id: Option<String> = tx
            .query_row(
                "SELECT group_id FROM event_group_members
                  WHERE calendar_id = ? AND event_id = ?",
                params![old_calendar_id, old_event_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(group_id) = group_id else {
            tx.commit()?;
            return Ok(None);
        };
        let signature: (String, String) = tx.query_row(
            "SELECT title, starts_at FROM event_group_members
              WHERE calendar_id = ? AND event_id = ?",
            params![old_calendar_id, old_event_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        // The target may already be in a group (the user moved an event onto
        // one it was grouped with). Its own membership wins; ours goes.
        tx.execute(
            "DELETE FROM event_group_members WHERE calendar_id = ? AND event_id = ?",
            params![old_calendar_id, old_event_id],
        )?;
        tx.execute(
            "INSERT INTO event_group_members
                 (group_id, calendar_id, event_id, title, starts_at, added_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(calendar_id, event_id) DO NOTHING",
            params![
                group_id,
                new_calendar_id,
                new_event_id,
                // ITS own signature, carried across the move. The row this
                // replaced was picked with `LIMIT 1` from the group, so a
                // moved member used to inherit whichever other member SQLite
                // happened to return — and the signature is exactly what has
                // to survive, since it is how the member is found again.
                signature.0,
                signature.1,
                now
            ],
        )?;
        tx.execute(
            "UPDATE event_groups SET updated_at = ? WHERE id = ?",
            params![now, group_id],
        )?;
        let group = read_group(&tx, &group_id)?;
        tx.commit()?;
        Ok(group)
    }

    /// Take every event of a calendar out of its groups.
    ///
    /// A deleted calendar takes its events with it, and a membership row that
    /// names one of them is a group counting a row that cannot be shown. The
    /// groups left standing are returned so the caller can tell the other
    /// devices; a group that falls under two members stops being shown by
    /// itself (see `get`), so nothing has to be deleted here.
    pub fn forget_calendar(&self, calendar_id: &str) -> Result<Vec<EventGroup>, EventGroupsError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.db.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        let mut stmt =
            tx.prepare("SELECT DISTINCT group_id FROM event_group_members WHERE calendar_id = ?")?;
        let touched = stmt
            .query_map(params![calendar_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        if touched.is_empty() {
            tx.commit()?;
            return Ok(Vec::new());
        }
        tx.execute(
            "DELETE FROM event_group_members WHERE calendar_id = ?",
            params![calendar_id],
        )?;
        let mut out = Vec::new();
        for id in touched {
            tx.execute(
                "UPDATE event_groups SET updated_at = ? WHERE id = ?",
                params![now, id],
            )?;
            if let Some(group) = read_group(&tx, &id)? {
                out.push(group);
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Write down what a member's event looks like NOW.
    ///
    /// The signature is how a member is found again once the provider remints
    /// its id — so it has to describe the event as it currently is. Written
    /// once at joining, it went stale the first time the appointment moved,
    /// and the healing that depends on it could never match again. Carrying an
    /// edit to the copies makes that the ordinary case rather than the rare
    /// one: moving the appointment is what the carry is for.
    ///
    /// Local only and silent, for the same reason as `heal_member`: every
    /// device sees the same events and refreshes its own copy.
    pub fn refresh_signature(
        &self,
        calendar_id: &str,
        event_id: &str,
        title: &str,
        starts_at: &str,
    ) -> Result<(), EventGroupsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "UPDATE event_group_members
                SET title = ?, starts_at = ?
              WHERE calendar_id = ? AND event_id = ?",
            params![title, starts_at, calendar_id, event_id],
        )?;
        Ok(())
    }

    /// Point a member at the id its event carries now.
    ///
    /// Ids belong to the provider and change underneath us — a re-bootstrap
    /// remints them, moving an event between calendars remints it, Exchange
    /// does it unprompted. The SIGNATURE stored with each member exists so the
    /// event can be found again; this writes down what was found.
    ///
    /// A repair of Aperio's own bookkeeping, not a change to the group: the
    /// same events mean the same appointment before and after.
    ///
    /// `updated_at` deliberately does NOT move, and nothing is emitted. A heal
    /// is derived from evidence every device has — its own view of the events
    /// — so each repairs itself when it next renders the range, and none of
    /// them has to be told. Broadcasting it was worse than useless twice over:
    /// a silent repair stamped "now" would outrank a DISSOLVE another device
    /// had just made, resurrecting a group the user got rid of; and two
    /// devices whose caches disagree about an id would heal each other back
    /// and forth without end.
    ///
    /// Returns the group afterwards, or `None` when the old member was already
    /// gone (two views healing the same thing at once is not an error).
    pub fn heal_member(
        &self,
        group_id: &str,
        calendar_id: &str,
        old_event_id: &str,
        new_event_id: &str,
    ) -> Result<Option<EventGroup>, EventGroupsError> {
        let mut conn = self.db.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE event_group_members
                SET event_id = ?
              WHERE group_id = ? AND calendar_id = ? AND event_id = ?",
            params![new_event_id, group_id, calendar_id, old_event_id],
        )?;
        if changed == 0 {
            tx.commit()?;
            return Ok(None);
        }
        let group = read_group(&tx, group_id)?;
        tx.commit()?;
        Ok(group)
    }

    /// Take one event out of its group.
    ///
    /// A group of one is not a group, so removing the second-to-last member
    /// dissolves it rather than leaving a single event claiming to be grouped
    /// with nothing. `None` means the event was not grouped in the first place
    /// — nothing happened, and nothing needs to be told to the other devices.
    pub fn ungroup(
        &self,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<Option<Ungrouped>, EventGroupsError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.db.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        let group_id: Option<String> = tx
            .query_row(
                "SELECT group_id FROM event_group_members
                  WHERE calendar_id = ? AND event_id = ?",
                params![calendar_id, event_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(group_id) = group_id else {
            tx.commit()?;
            return Ok(None);
        };
        tx.execute(
            "DELETE FROM event_group_members WHERE calendar_id = ? AND event_id = ?",
            params![calendar_id, event_id],
        )?;
        let remaining: i64 = tx.query_row(
            "SELECT COUNT(*) FROM event_group_members WHERE group_id = ?",
            params![group_id],
            |row| row.get(0),
        )?;
        if remaining < 2 {
            tx.execute("DELETE FROM event_groups WHERE id = ?", params![group_id])?;
            mark_dissolved(&tx, &group_id, &now)?;
            tx.commit()?;
            return Ok(Some(Ungrouped::Dissolved { group_id }));
        }
        tx.execute(
            "UPDATE event_groups SET updated_at = ? WHERE id = ?",
            params![now, group_id],
        )?;
        let group = read_group(&tx, &group_id)?.ok_or(EventGroupsError::Vanished)?;
        tx.commit()?;
        Ok(Some(Ungrouped::Remains(group)))
    }

    /// Record that these two events are NOT the same appointment.
    ///
    /// Silences the OFFER, nothing else — grouping them by hand still works
    /// and never consults this. Idempotent: declining twice is one decision,
    /// and the pair is canonicalised so declining "B and A" answers "A and B".
    pub fn decline_suggestion(
        &self,
        first: (&str, &str),
        second: (&str, &str),
    ) -> Result<SuggestionDecline, EventGroupsError> {
        let decline = SuggestionDecline::new(first, second, Utc::now().to_rfc3339());
        let conn = self.db.lock().expect("db mutex poisoned");
        write_decline(&conn, &decline)?;
        Ok(decline)
    }

    /// Every pair the user has said is not one appointment.
    ///
    /// Read whole rather than asked per pair: the set is small (it grows only
    /// when someone says no) and a view checks every candidate pair it has, so
    /// one read beats one query per row.
    pub fn declined_suggestions(&self) -> Result<Vec<SuggestionDecline>, EventGroupsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        read_declines(&conn)
    }

    /// Dissolve a whole group. The events themselves are untouched.
    pub fn dissolve(&self, group_id: &str) -> Result<bool, EventGroupsError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.db.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        let removed = tx.execute("DELETE FROM event_groups WHERE id = ?", params![group_id])?;
        if removed > 0 {
            mark_dissolved(&tx, group_id, &now)?;
        }
        tx.commit()?;
        Ok(removed > 0)
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

    fn member(calendar: &str, event: &str) -> NewMember {
        NewMember {
            calendar_id: calendar.into(),
            event_id: event.into(),
            title: "Wochenplanung".into(),
            starts_at: "2026-08-10T08:00:00Z".into(),
        }
    }

    #[test]
    fn two_events_become_one_group() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);

        let group = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();
        assert_eq!(group.members.len(), 2);

        let found = repo
            .for_events(&[
                ("work".into(), "ev-a".into()),
                ("private".into(), "ev-b".into()),
                ("work".into(), "unrelated".into()),
            ])
            .unwrap();
        assert_eq!(found.len(), 2, "the ungrouped event must not appear");
        assert_eq!(found[&("work".into(), "ev-a".into())], group.id);
    }

    #[test]
    fn a_third_event_joins_the_group_the_others_are_in() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        let first = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();

        // "and this one too" — named alongside a member already in the group.
        let joined = repo
            .group(&[member("work", "ev-a"), member("colleague", "ev-c")])
            .unwrap();
        assert_eq!(joined.id, first.id, "a second group would split the truth");
        assert_eq!(joined.members.len(), 3);
    }

    #[test]
    fn a_view_gets_whole_groups_including_members_it_cannot_see() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        repo.group(&[
            member("work", "ev-a"),
            member("private", "ev-b"),
            member("colleague", "ev-c"),
        ])
        .unwrap();
        repo.group(&[member("work", "ev-x"), member("private", "ev-y")])
            .unwrap();

        // The view holds one event of each group and nothing else.
        let groups = repo
            .groups_for_events(&[
                ("work".into(), "ev-a".into()),
                ("private".into(), "ev-y".into()),
                ("work".into(), "ungrouped".into()),
            ])
            .unwrap();
        assert_eq!(groups.len(), 2, "each group once, the loner not at all");
        let three = groups.iter().find(|g| g.members.len() == 3).unwrap();
        assert!(
            three.members.iter().any(|m| m.event_id == "ev-c"),
            "a member outside the range still belongs to the group",
        );
    }

    #[test]
    fn merging_two_existing_groups_is_refused_rather_than_guessed() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        repo.group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();
        repo.group(&[member("colleague", "ev-c"), member("other", "ev-d")])
            .unwrap();

        let err = repo
            .group(&[member("work", "ev-a"), member("colleague", "ev-c")])
            .unwrap_err();
        assert!(matches!(err, EventGroupsError::ConflictingGroups));
    }

    #[test]
    fn a_member_can_be_pointed_at_the_id_its_event_carries_now() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        let group = repo
            .group(&[member("work", "old-a"), member("private", "ev-b")])
            .unwrap();

        let healed = repo
            .heal_member(&group.id, "work", "old-a", "new-a")
            .unwrap()
            .expect("the group stands");
        assert!(healed.members.iter().any(|m| m.event_id == "new-a"));
        assert!(!healed.members.iter().any(|m| m.event_id == "old-a"));
        assert_eq!(
            healed.members.len(),
            2,
            "healing is not a membership change"
        );

        // Two views healing the same thing at once is not an error.
        assert_eq!(
            repo.heal_member(&group.id, "work", "old-a", "new-a")
                .unwrap(),
            None,
        );
    }

    #[test]
    fn a_group_of_one_dissolves_itself() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        let group = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();

        // Removing one of two leaves a single event that would otherwise claim
        // to be grouped with nothing.
        assert_eq!(
            repo.ungroup("work", "ev-a").unwrap(),
            Some(Ungrouped::Dissolved {
                group_id: group.id.clone()
            }),
        );
        assert_eq!(repo.get(&group.id).unwrap(), None);
        assert!(repo
            .for_events(&[("private".into(), "ev-b".into())])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn removing_one_of_three_keeps_the_rest() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        repo.group(&[
            member("work", "ev-a"),
            member("private", "ev-b"),
            member("colleague", "ev-c"),
        ])
        .unwrap();

        let Some(Ungrouped::Remains(left)) = repo.ungroup("colleague", "ev-c").unwrap() else {
            panic!("two members left, so the group stands");
        };
        assert_eq!(left.members.len(), 2);
    }

    #[test]
    fn one_event_is_not_a_group() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        let err = repo.group(&[member("work", "ev-a")]).unwrap_err();
        assert!(matches!(err, EventGroupsError::TooFewMembers));
    }

    #[test]
    fn grouping_the_same_pair_twice_changes_nothing() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        let first = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();
        let again = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.members.len(), 2);
    }
}
