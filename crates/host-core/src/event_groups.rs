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

use cal_core::{EventGroup, EventGroupMember};

use crate::db::SharedConn;

/// What a caller hands over to put an event in a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMember {
    pub calendar_id: String,
    pub event_id: String,
    pub title: String,
    pub starts_at: String,
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
        tx.commit()?;
        drop(conn);
        Ok(self.get(&group_id)?.expect("just written"))
    }

    /// One group with its members, or `None` when the id is unknown.
    pub fn get(&self, group_id: &str) -> Result<Option<EventGroup>, EventGroupsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
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
        Ok(Some(EventGroup {
            id: group_id.to_string(),
            created_at,
            updated_at,
            members,
        }))
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
        Ok(out)
    }

    /// Take one event out of its group.
    ///
    /// A group of one is not a group, so removing the second-to-last member
    /// dissolves it rather than leaving a single event claiming to be grouped
    /// with nothing. Returns the group as it stands afterwards, or `None` when
    /// it was dissolved or the event was not grouped at all.
    pub fn ungroup(
        &self,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<Option<EventGroup>, EventGroupsError> {
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
            tx.commit()?;
            return Ok(None);
        }
        tx.execute(
            "UPDATE event_groups SET updated_at = ? WHERE id = ?",
            params![now, group_id],
        )?;
        tx.commit()?;
        drop(conn);
        self.get(&group_id)
    }

    /// Dissolve a whole group. The events themselves are untouched.
    pub fn dissolve(&self, group_id: &str) -> Result<bool, EventGroupsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let removed = conn.execute("DELETE FROM event_groups WHERE id = ?", params![group_id])?;
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
    fn a_group_of_one_dissolves_itself() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        let group = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();

        // Removing one of two leaves a single event that would otherwise claim
        // to be grouped with nothing.
        assert_eq!(repo.ungroup("work", "ev-a").unwrap(), None);
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

        let left = repo
            .ungroup("colleague", "ev-c")
            .unwrap()
            .expect("still a group");
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
