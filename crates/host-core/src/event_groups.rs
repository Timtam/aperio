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
use std::collections::{HashMap, HashSet};

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
///
/// Both carry the refusals the removal wrote (see [`Removal`]), because those
/// have to travel too: a device that hears only "the group is gone" will form
/// it again the moment it sees the same join URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ungrouped {
    /// The group stands, with the members left in it.
    Remains {
        group: EventGroup,
        declines: Vec<SuggestionDecline>,
    },
    /// Fewer than two were left, so the group is gone.
    Dissolved {
        group_id: String,
        declines: Vec<SuggestionDecline>,
    },
}

/// A group as it stands, plus the refusals that grouping took back.
///
/// The caller has to pass BOTH on: a clearing that never leaves the device it
/// happened on is a mark the other devices still hold, and they would use it
/// to break this very group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grouped {
    pub group: EventGroup,
    pub cleared: Vec<SuggestionDecline>,
}

/// A group after one of its members moved, plus the refusals that moved with it.
///
/// The marks have to reach the other devices: they are TOLD the new member id
/// rather than working it out, so nothing there would rewrite a mark of its
/// own — see `carry_declines`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocated {
    pub group: EventGroup,
    pub carried: Vec<SuggestionDecline>,
}

/// Why a member is leaving its group — and so whether it means anything.
///
/// Only [`Removal::ByUser`] is a statement. Someone taking an event out is
/// saying "this is not the same appointment as those", and Aperio writes that
/// down: automatic grouping (`DESIGN-event-groups.md`, Stufe 4) reads exactly
/// those marks, and without them a group formed from a meeting link would come
/// back on the next render, every day.
///
/// Everything else is bookkeeping and says nothing about anything. A deleted
/// event's membership no longer points at a row; a copy taken out mid-way
/// through a series split is on its way straight back into a new group. Neither
/// may leave a refusal behind — one that would outlive the reason for it and
/// quietly bind a pair the user never ruled on, on every device, forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// The user took this event out.
    ByUser,
    /// Aperio is tidying up its own records — a deleted event's membership, or
    /// a copy being moved from one group to another.
    Bookkeeping,
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
             (calendar_a, event_a, calendar_b, event_b, declined_at, cleared_at)
         VALUES (?, ?, ?, ?, ?, NULL)
         ON CONFLICT(calendar_a, event_a, calendar_b, event_b)
         DO UPDATE SET declined_at = MAX(declined_at, excluded.declined_at)",
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

/// Take a refusal back, because the user just said the opposite.
///
/// Grouping two events BY HAND is the statement that they ARE one appointment,
/// and it has to be able to win — an arriving mark is now allowed to break a
/// group that contradicts it, so without this a refusal from a device that was
/// offline for three weeks would tear apart a group made deliberately
/// yesterday.
///
/// It stamps rather than deletes. Deleting would break the union rule the
/// whole table rests on: one device's delete and another's surviving row merge
/// back to "refused" and the clearing is lost. Two timestamps that only move
/// forward merge to the same answer whatever order they arrive in.
///
/// Writes nothing when there is no row: a pair nobody ever refused needs no
/// record saying so.
fn clear_decline(
    conn: &rusqlite::Connection,
    first: (&str, &str),
    second: (&str, &str),
    now: &str,
) -> Result<Option<SuggestionDecline>, EventGroupsError> {
    let key = SuggestionDecline::new(first, second, now.to_string());
    // At least as late as the refusal it takes back, never merely "now".
    //
    // Clocks differ between devices, and the refusal may carry a stamp from one
    // running ahead. Taking this device's wall clock would then write a
    // clearing OLDER than what it contradicts — counted nowhere, yet reported
    // as done and broadcast to everyone. The user's group would stand beside a
    // mark saying it should not, which is exactly the contradiction this column
    // exists to end.
    let changed = conn.execute(
        "UPDATE event_group_suggestion_declines
            SET cleared_at = MAX(COALESCE(cleared_at, ''), declined_at, ?)
          WHERE calendar_a = ? AND event_a = ? AND calendar_b = ? AND event_b = ?",
        params![
            now,
            key.calendar_a,
            key.event_a,
            key.calendar_b,
            key.event_b
        ],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    read_decline(conn, &key)
}

/// One pair's row, whatever it currently says.
fn read_decline(
    conn: &rusqlite::Connection,
    key: &SuggestionDecline,
) -> Result<Option<SuggestionDecline>, EventGroupsError> {
    let row = conn
        .query_row(
            "SELECT declined_at, cleared_at FROM event_group_suggestion_declines
              WHERE calendar_a = ? AND event_a = ? AND calendar_b = ? AND event_b = ?",
            params![key.calendar_a, key.event_a, key.calendar_b, key.event_b],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    Ok(row.map(|(declined_at, cleared_at)| SuggestionDecline {
        declined_at,
        cleared_at,
        ..key.clone()
    }))
}

fn read_declines(conn: &rusqlite::Connection) -> Result<Vec<SuggestionDecline>, EventGroupsError> {
    // Only the pairs currently refused. A row whose clearing is the newer of
    // the two statements is a pair the user has since grouped by hand, and it
    // stays on disk only so the clearing can travel and merge.
    let mut stmt = conn.prepare(
        "SELECT calendar_a, event_a, calendar_b, event_b, declined_at, cleared_at
           FROM event_group_suggestion_declines
          WHERE cleared_at IS NULL OR declined_at > cleared_at",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SuggestionDecline {
                cleared_at: row.get(5)?,
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

/// Drop members until no refusal contradicts the ones that remain.
///
/// A group claims its members are one appointment. A mark says two of them are
/// not — so the claim as a whole cannot stand, and one of the two has to go.
///
/// Which one, and in which order, has to be decided from the DATA alone or two
/// devices holding identical rows would show different groups. So: of all the
/// marks whose two sides are both still present, take the smallest in the
/// canonical order the marks are already stored in and drop its second half.
/// Repeat until nothing contradicts. A total order in gives a total order out —
/// the storage order, the arrival order and the group's size are all
/// irrelevant.
///
/// Doing this when a mark ARRIVES instead, by deleting the membership row, made
/// the answer a function of what the database happened to hold at that instant:
/// a mark landing before its group broke nothing and one landing after broke
/// it, the snapshot path and the log path applied the two in opposite orders,
/// and two marks sharing a member were not commutative because the first
/// changed what the second could still find. Every one of those is arrival
/// order deciding what the data means.
///
/// The membership itself is never touched. The rows stay as they are, and a
/// clearing — grouping the pair by hand — simply makes the mark stop counting,
/// which brings the member straight back.
fn without_refused_pairs(
    conn: &rusqlite::Connection,
    members: Vec<EventGroupMember>,
) -> Result<Vec<EventGroupMember>, EventGroupsError> {
    if members.len() < 2 {
        return Ok(members);
    }
    let present: HashSet<(String, String)> = members
        .iter()
        .map(|m| (m.calendar_id.clone(), m.event_id.clone()))
        .collect();
    // Only the marks that could speak about THESE members. The set is small —
    // it grows only when someone says no — so one read beats a query per pair.
    let mut refusals: Vec<(String, String, String, String)> = read_declines(conn)?
        .into_iter()
        .filter(|d| {
            present.contains(&(d.calendar_a.clone(), d.event_a.clone()))
                && present.contains(&(d.calendar_b.clone(), d.event_b.clone()))
        })
        .map(|d| (d.calendar_a, d.event_a, d.calendar_b, d.event_b))
        .collect();
    if refusals.is_empty() {
        return Ok(members);
    }
    refusals.sort();

    let mut dropped: HashSet<(String, String)> = HashSet::new();
    while let Some((_, _, cb, eb)) = refusals.iter().find(|(ca, ea, cb, eb)| {
        !dropped.contains(&(ca.clone(), ea.clone())) && !dropped.contains(&(cb.clone(), eb.clone()))
    }) {
        // The canonically-second half goes. Either choice breaks the pair; this
        // one is a property of the mark, so every device makes it.
        dropped.insert((cb.clone(), eb.clone()));
    }
    Ok(members
        .into_iter()
        .filter(|m| !dropped.contains(&(m.calendar_id.clone(), m.event_id.clone())))
        .collect())
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
    // A refusal contradicting two of these takes one of them out — the rule
    // lives here, next to the one below and for the same reason.
    let members = without_refused_pairs(conn, members)?;
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

/// Point every refusal naming `old` at `new`.
///
/// Group members are repaired when a provider re-mints an id and followed when
/// an event moves calendars, because the design accepts that ids change. The
/// refusals were keyed by the same ids and were touched by neither — so months
/// after saying no, an id change would silently un-say it and the pair would be
/// grouped again with nothing to explain why.
///
/// The pair is stored in a canonical order, so a rewrite can change which side
/// a row belongs on: the row is rebuilt through `SuggestionDecline::new` rather
/// than updated in place, and merged into whatever is already there — the same
/// later-of-each rule the sync path uses, for the same reason.
///
/// Local and silent, exactly like the repairs it follows: every device holds
/// the same marks and sees the same id change, so each fixes its own.
fn carry_declines(
    conn: &rusqlite::Connection,
    old: (&str, &str),
    new: (&str, &str),
) -> Result<Vec<SuggestionDecline>, EventGroupsError> {
    if old == new {
        return Ok(Vec::new());
    }
    let affected: Vec<SuggestionDecline> = {
        let mut stmt = conn.prepare(
            "SELECT calendar_a, event_a, calendar_b, event_b, declined_at, cleared_at
               FROM event_group_suggestion_declines
              WHERE (calendar_a = ?1 AND event_a = ?2) OR (calendar_b = ?1 AND event_b = ?2)",
        )?;
        let rows = stmt.query_map(params![old.0, old.1], |row| {
            Ok(SuggestionDecline {
                calendar_a: row.get(0)?,
                event_a: row.get(1)?,
                calendar_b: row.get(2)?,
                event_b: row.get(3)?,
                declined_at: row.get(4)?,
                cleared_at: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut carried = Vec::new();
    for row in affected {
        let other = if (row.calendar_a.as_str(), row.event_a.as_str()) == old {
            (row.calendar_b.clone(), row.event_b.clone())
        } else {
            (row.calendar_a.clone(), row.event_a.clone())
        };
        // A pair cannot refuse itself: an event moved onto the very id it was
        // already refused against leaves nothing to say.
        if (other.0.as_str(), other.1.as_str()) == new {
            continue;
        }
        let mut moved = SuggestionDecline::new(
            new,
            (other.0.as_str(), other.1.as_str()),
            row.declined_at.clone(),
        );
        moved.cleared_at = row.cleared_at.clone();
        if let Some(existing) = read_decline(conn, &moved)? {
            moved.merge(&existing);
        }
        conn.execute(
            "INSERT INTO event_group_suggestion_declines
                 (calendar_a, event_a, calendar_b, event_b, declined_at, cleared_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(calendar_a, event_a, calendar_b, event_b) DO UPDATE SET
                 declined_at = excluded.declined_at,
                 cleared_at  = excluded.cleared_at",
            params![
                moved.calendar_a,
                moved.event_a,
                moved.calendar_b,
                moved.event_b,
                moved.declined_at,
                moved.cleared_at,
            ],
        )?;
        conn.execute(
            "DELETE FROM event_group_suggestion_declines
              WHERE calendar_a = ? AND event_a = ? AND calendar_b = ? AND event_b = ?",
            params![row.calendar_a, row.event_a, row.calendar_b, row.event_b],
        )?;
        carried.push(moved);
    }
    Ok(carried)
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
    pub fn group(&self, members: &[NewMember]) -> Result<Grouped, EventGroupsError> {
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
        // Saying "these ARE one appointment" takes back every refusal among
        // them — and this has to happen BEFORE the answer is read back.
        //
        // Reading first would ask `read_group`, which drops a member a refusal
        // contradicts; the loop would then walk the SURVIVORS and never reach
        // the pair whose mark is the reason one of them is missing. Grouping a
        // refused pair by hand would have cleared nothing, quietly done
        // nothing, and — for a pair — read back as a group of one, which is no
        // group at all.
        //
        // Over the stored membership, and over every PAIR of it rather than
        // only the ones this call named: they are all one appointment now, and
        // a leftover mark between two of them is a contradiction the read side
        // would act on.
        let stored: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT calendar_id, event_id FROM event_group_members
                  WHERE group_id = ? ORDER BY calendar_id, event_id",
            )?;
            let rows = stmt.query_map(params![group_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut cleared = Vec::new();
        for (i, first) in stored.iter().enumerate() {
            for second in stored.iter().skip(i + 1) {
                if let Some(row) = clear_decline(
                    &tx,
                    (first.0.as_str(), first.1.as_str()),
                    (second.0.as_str(), second.1.as_str()),
                    &now,
                )? {
                    cleared.push(row);
                }
            }
        }
        // Read the answer back INSIDE the transaction. Committing first and
        // re-reading afterwards means letting go of the write lock in between,
        // where another thread's dissolve can land — and the `expect` that
        // followed would then panic, poisoning the mutex and taking every
        // later database call in the process down with it. A row we just wrote
        // and have not yet released cannot be missing.
        let group = read_group(&tx, &group_id)?.ok_or(EventGroupsError::Vanished)?;
        tx.commit()?;
        Ok(Grouped { group, cleared })
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
    ) -> Result<Option<Relocated>, EventGroupsError> {
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
        // The refusals name the id the membership just left — see
        // `carry_declines`. Unlike the healing, these have to TRAVEL: the other
        // devices are told the new member id, they do not derive the move from
        // evidence of their own, so nothing there would ever rewrite the marks.
        // Left behind they would name an id that exists nowhere, and the pair
        // would be groupable again as if nothing had been said.
        let carried = carry_declines(
            &tx,
            (old_calendar_id, old_event_id),
            (new_calendar_id, new_event_id),
        )?;
        let group = read_group(&tx, &group_id)?;
        tx.commit()?;
        Ok(group.map(|group| Relocated { group, carried }))
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
        // The refusals name the same id the membership did — see
        // `carry_declines`.
        carry_declines(
            &tx,
            (calendar_id, old_event_id),
            (calendar_id, new_event_id),
        )?;
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
    ///
    /// `removal` decides whether this leaves a refusal behind; see [`Removal`].
    pub fn ungroup(
        &self,
        calendar_id: &str,
        event_id: &str,
        removal: Removal,
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
        // Who it is leaving. Read BEFORE the delete, and — when the user is the
        // one taking it out — written down as what that means: this event and
        // those are not one appointment.
        //
        // It binds the suggestion line too, and that is right rather than a
        // side effect: "I took this out" is the same statement as "no, not the
        // same thing". An explicit Group action is never blocked by it.
        let mut declines = Vec::new();
        if removal == Removal::ByUser {
            let others: Vec<(String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT calendar_id, event_id FROM event_group_members
                      WHERE group_id = ? AND NOT (calendar_id = ? AND event_id = ?)",
                )?;
                let rows = stmt.query_map(params![group_id, calendar_id, event_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            for (other_cal, other_id) in &others {
                let decline = SuggestionDecline::new(
                    (calendar_id, event_id),
                    (other_cal.as_str(), other_id.as_str()),
                    now.clone(),
                );
                write_decline(&tx, &decline)?;
                declines.push(decline);
            }
        }
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
            return Ok(Some(Ungrouped::Dissolved { group_id, declines }));
        }
        tx.execute(
            "UPDATE event_groups SET updated_at = ? WHERE id = ?",
            params![now, group_id],
        )?;
        let group = read_group(&tx, &group_id)?.ok_or(EventGroupsError::Vanished)?;
        tx.commit()?;
        Ok(Some(Ungrouped::Remains { group, declines }))
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
    ///
    /// Every pair in it is written down as declined, for the same reason
    /// [`Self::ungroup`] does it: "these are not one appointment" is what
    /// taking the group apart says, and Stage 4's automatic grouping has to
    /// hear it — otherwise a group built from meeting links comes back on the
    /// next render, every day.
    ///
    /// `None` means there was no such group and nothing happened; `Some` gives
    /// the refusals written, which the caller has to pass on.
    pub fn dissolve(
        &self,
        group_id: &str,
    ) -> Result<Option<Vec<SuggestionDecline>>, EventGroupsError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.db.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        // Read before the delete: the members go with the group (ON DELETE
        // CASCADE), so afterwards there is nothing left to say no about.
        let members: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT calendar_id, event_id FROM event_group_members WHERE group_id = ?",
            )?;
            let rows = stmt.query_map(params![group_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let removed = tx.execute("DELETE FROM event_groups WHERE id = ?", params![group_id])?;
        if removed == 0 {
            tx.commit()?;
            return Ok(None);
        }
        let mut declines = Vec::new();
        for (i, (cal_a, id_a)) in members.iter().enumerate() {
            for (cal_b, id_b) in members.iter().skip(i + 1) {
                let decline = SuggestionDecline::new(
                    (cal_a.as_str(), id_a.as_str()),
                    (cal_b.as_str(), id_b.as_str()),
                    now.clone(),
                );
                write_decline(&tx, &decline)?;
                declines.push(decline);
            }
        }
        mark_dissolved(&tx, group_id, &now)?;
        tx.commit()?;
        Ok(Some(declines))
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
            .unwrap()
            .group;
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
            .unwrap()
            .group;

        // "and this one too" — named alongside a member already in the group.
        let joined = repo
            .group(&[member("work", "ev-a"), member("colleague", "ev-c")])
            .unwrap()
            .group;
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
            .unwrap()
            .group;

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
            .unwrap()
            .group;

        // Removing one of two leaves a single event that would otherwise claim
        // to be grouped with nothing.
        let Some(Ungrouped::Dissolved { group_id, declines }) =
            repo.ungroup("work", "ev-a", Removal::ByUser).unwrap()
        else {
            panic!("one member left, so the group goes");
        };
        assert_eq!(group_id, group.id);
        assert_eq!(declines.len(), 1, "the pair it was in is now refused");
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

        let Some(Ungrouped::Remains {
            group: left,
            declines,
        }) = repo.ungroup("colleague", "ev-c", Removal::ByUser).unwrap()
        else {
            panic!("two members left, so the group stands");
        };
        assert_eq!(left.members.len(), 2);
        // One refusal per member it left, and none between the two that stay.
        assert_eq!(declines.len(), 2);
    }

    #[test]
    fn taking_a_group_apart_says_these_are_not_the_same_appointment() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        let group = repo
            .group(&[
                member("work", "ev-a"),
                member("private", "ev-b"),
                member("colleague", "ev-c"),
            ])
            .unwrap()
            .group;

        let declines = repo.dissolve(&group.id).unwrap().expect("it was there");
        // Every pair, not just the ones somebody looked at: after this the
        // group is gone, and each of the three could otherwise be offered
        // against each of the others again.
        assert_eq!(declines.len(), 3);
        assert_eq!(repo.declined_suggestions().unwrap().len(), 3);

        // And a second dissolve of the same id is not an event.
        assert_eq!(repo.dissolve(&group.id).unwrap(), None);
    }

    /// The series carry takes a copy out of the head group and puts it straight
    /// back into the new one. Recorded as a refusal, that would bind a pair
    /// nobody ruled on — and the very next step groups them, so the database
    /// would claim both things at once.
    #[test]
    fn regrouping_a_copy_is_not_a_refusal() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        repo.group(&[
            member("work", "series"),
            member("private", "copy"),
            member("colleague", "other"),
        ])
        .unwrap();

        let Some(Ungrouped::Remains { declines, .. }) = repo
            .ungroup("private", "copy", Removal::Bookkeeping)
            .unwrap()
        else {
            panic!("two members left, so the group stands");
        };
        assert!(declines.is_empty());
        assert!(repo.declined_suggestions().unwrap().is_empty());

        // …and the copy can be grouped again straight away, which is what the
        // carry does two steps later.
        repo.group(&[member("private", "copy"), member("work", "series")])
            .expect("regrouping is not blocked");
    }

    #[test]
    fn a_deleted_event_has_not_refused_anything() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        repo.group(&[
            member("work", "ev-a"),
            member("private", "ev-b"),
            member("colleague", "ev-c"),
        ])
        .unwrap();

        // The event is gone; its membership is bookkeeping, not a statement.
        // A refusal here would outlive the event and quietly bind an id the
        // provider may hand out again.
        let Some(Ungrouped::Remains { declines, .. }) = repo
            .ungroup("colleague", "ev-c", Removal::Bookkeeping)
            .unwrap()
        else {
            panic!("two members left, so the group stands");
        };
        assert!(declines.is_empty());
        assert!(repo.declined_suggestions().unwrap().is_empty());
    }

    #[test]
    fn grouping_by_hand_takes_the_refusal_back() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        repo.decline_suggestion(("work", "ev-a"), ("private", "ev-b"))
            .unwrap();
        assert_eq!(repo.declined_suggestions().unwrap().len(), 1);

        // The opposite statement, and the newer one.
        let grouped = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();
        assert_eq!(grouped.cleared.len(), 1, "the caller has to pass it on");
        assert!(
            repo.declined_suggestions().unwrap().is_empty(),
            "a pair that is grouped is not a pair that is refused",
        );

        // And refusing again wins again — the later statement, either way.
        repo.ungroup("private", "ev-b", Removal::ByUser).unwrap();
        assert_eq!(repo.declined_suggestions().unwrap().len(), 1);
    }

    #[test]
    fn a_refusal_follows_an_id_the_provider_reminted() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        // The pair is refused, and SEPARATELY there is a group holding the
        // event under its old id — which is what gives `heal_member` something
        // to repair.
        repo.decline_suggestion(("private", "old-b"), ("acc::meetings", "vc::1"))
            .unwrap();
        let group = repo
            .group(&[member("private", "old-b"), member("work", "ev-a")])
            .unwrap()
            .group;

        repo.heal_member(&group.id, "private", "old-b", "new-b")
            .unwrap()
            .expect("healed");

        let declines = repo.declined_suggestions().unwrap();
        assert_eq!(declines.len(), 1, "still exactly one refusal");
        let d = &declines[0];
        let names = [
            (d.calendar_a.as_str(), d.event_a.as_str()),
            (d.calendar_b.as_str(), d.event_b.as_str()),
        ];
        assert!(
            names.contains(&("private", "new-b")),
            "the refusal moved with the event: {names:?}",
        );
        assert!(names.contains(&("acc::meetings", "vc::1")));
    }

    /// A refusal contradicting two members takes one of them out — wherever the
    /// group is read, without touching a single stored row.
    #[test]
    fn a_refusal_takes_a_member_out_of_the_group_that_contradicts_it() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        let group = repo
            .group(&[
                member("work", "ev-a"),
                member("private", "ev-b"),
                member("colleague", "ev-c"),
            ])
            .unwrap()
            .group;
        assert_eq!(group.members.len(), 3);

        repo.decline_suggestion(("work", "ev-a"), ("private", "ev-b"))
            .unwrap();

        let after = repo.get(&group.id).unwrap().expect("still a group");
        assert_eq!(after.members.len(), 2, "one of the pair is gone");
        // The canonically-second half — a property of the mark, so every device
        // drops the same one.
        assert!(after.members.iter().any(|m| m.event_id == "ev-c"));

        // Nothing was deleted: taking the refusal back brings it straight back.
        repo.group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();
        assert_eq!(repo.get(&group.id).unwrap().unwrap().members.len(), 3);
    }

    /// Two refusals sharing a member are commutative — the old rule, which
    /// deleted memberships as marks arrived, was not.
    #[test]
    fn two_refusals_sharing_a_member_give_one_answer() {
        // The pairs are declined in both orders, in two fresh databases. If the
        // rule read anything but the finished set, the two would disagree.
        let mut answers = Vec::new();
        for reversed in [false, true] {
            let (_tmp, db) = fresh();
            let shared = db.shared();
            let repo = EventGroupsRepo::new(&shared);
            let group = repo
                .group(&[
                    member("work", "ev-a"),
                    member("private", "ev-b"),
                    member("colleague", "ev-c"),
                ])
                .unwrap()
                .group;
            let pairs: [((&str, &str), (&str, &str)); 2] = [
                (("work", "ev-a"), ("private", "ev-b")),
                (("private", "ev-b"), ("colleague", "ev-c")),
            ];
            let order: Vec<_> = if reversed {
                pairs.iter().rev().collect()
            } else {
                pairs.iter().collect()
            };
            for (first, second) in order {
                repo.decline_suggestion(*first, *second).unwrap();
            }
            let after = repo.get(&group.id).unwrap();
            answers.push(
                after
                    .map(|g| {
                        let mut ids: Vec<_> = g.members.into_iter().map(|m| m.event_id).collect();
                        ids.sort();
                        ids
                    })
                    .unwrap_or_default(),
            );
        }
        assert_eq!(answers[0], answers[1], "the order the marks were made in");
    }

    /// A clearing is never born already lost, however far the clocks differ.
    #[test]
    fn grouping_by_hand_wins_even_against_a_refusal_from_a_fast_clock() {
        let (_tmp, db) = fresh();
        let shared = db.shared();
        let repo = EventGroupsRepo::new(&shared);
        // A refusal stamped in the future, as a device with a fast clock would.
        let ahead = (Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        {
            let conn = shared.lock().unwrap();
            write_decline(
                &conn,
                &SuggestionDecline::new(("work", "ev-a"), ("private", "ev-b"), ahead),
            )
            .unwrap();
        }
        assert_eq!(repo.declined_suggestions().unwrap().len(), 1);

        let grouped = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap();
        assert_eq!(grouped.cleared.len(), 1);
        assert!(
            repo.declined_suggestions().unwrap().is_empty(),
            "the clearing is clamped above the refusal, not stamped from this clock",
        );
        assert_eq!(
            repo.get(&grouped.group.id).unwrap().unwrap().members.len(),
            2
        );
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
            .unwrap()
            .group;
        let again = repo
            .group(&[member("work", "ev-a"), member("private", "ev-b")])
            .unwrap()
            .group;
        assert_eq!(again.id, first.id);
        assert_eq!(again.members.len(), 2);
    }
}
