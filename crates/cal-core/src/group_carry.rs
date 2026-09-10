//! Carrying a change to the other copies (`DESIGN-event-groups.md`, Stufe 2).
//!
//! A group says several events mean the same appointment. When the appointment
//! moves, all of them have to move — that is the second thing groups are for,
//! after not being read out four times. Doing it by hand is what the feature
//! exists to end: whoever forgets a copy has calendars that contradict each
//! other and finds out when somebody turns up at the wrong time.
//!
//! Two things are deliberately narrow.
//!
//! WHICH FIELDS travel: what the appointment IS (title, when, where, what it
//! says), and nothing that is a property of the copy. Reminders above all — the
//! private copy usually exists precisely because it has a reminder the work one
//! does not, and carrying those across would delete the reason for the copy.
//! Colour, calendar and attendees are per-copy for the same kind of reason.
//!
//! WHICH MEMBERS travel: only those Aperio may write. A colleague's calendar is
//! read-only, and the design is explicit that "carry to all" must SAY which
//! members it could not do rather than skip them quietly — otherwise it
//! produces exactly the contradiction it set out to prevent.
//!
//! # What this answers with
//!
//! FIELD VALUES, not rows. A caller's own row carries far more than the six
//! fields here — its reminders, its colour, its calendar — and the core has no
//! business knowing about them. So the row-shaped rules answer with the six
//! fields to write, and the caller lays them over what it already holds. That
//! is also what keeps a copy's reminder, the reason it exists, untouched.

use serde::{Deserialize, Serialize};

/// The fields a change is carried in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct CarryableFields {
    pub title: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub location: Option<String>,
    pub description: Option<String>,
}

/// One of the fields that travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum CarryField {
    Title,
    Start,
    End,
    AllDay,
    Location,
    Description,
}

/// The order they are reported in, which is the order they are declared in.
const CARRIED: [CarryField; 6] = [
    CarryField::Title,
    CarryField::Start,
    CarryField::End,
    CarryField::AllDay,
    CarryField::Location,
    CarryField::Description,
];

/// One member of the group, as the caller knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct CarryTarget {
    pub calendar_id: String,
    pub event_id: String,
    /// The name to show when reporting what happened to it.
    pub title: String,
    /// Whether Aperio may write to the calendar it lives in.
    pub writable: bool,
}

/// What carrying would do, decided before anything is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct CarryPlan {
    /// The fields that actually differ from the saved event.
    pub changed: Vec<CarryField>,
    /// Members Aperio can and will write.
    pub targets: Vec<CarryTarget>,
    /// Members it must leave alone, and the user has to be told about.
    pub skipped: Vec<CarryTarget>,
    /// Whether the plan is worth asking the user about at all.
    ///
    /// Part of the plan rather than a separate call: it is two comparisons, and
    /// crossing a door for them would be silly — but writing them out on the
    /// other side would be a twin, which is the thing this whole move is
    /// against.
    pub worth_carrying: bool,
}

/// Treat empty and absent as the same thing.
///
/// Providers disagree about which they return for a field the user never filled
/// in, and a change from absent to empty is not a change anybody made.
fn norm(value: Option<&str>) -> Option<&str> {
    match value {
        None | Some("") => None,
        some => some,
    }
}

fn millis(iso: &str) -> Option<i64> {
    iso.parse::<chrono::DateTime<chrono::Utc>>()
        .ok()
        .map(|at| at.timestamp_millis())
}

/// The same comparison, for the two fields that are INSTANTS.
///
/// The editor rebuilds start and end through its own ISO formatter, which is
/// not the spelling the backend sent. Compared as strings, every save looked
/// like it had moved the appointment — so a reminder-only edit asked to carry,
/// and carrying rewrote start and end onto every copy.
fn same_instant(a: &str, b: &str) -> bool {
    match (millis(a), millis(b)) {
        (Some(a), Some(b)) => a == b,
        _ => norm(Some(a)) == norm(Some(b)),
    }
}

fn field_of(fields: &CarryableFields, field: CarryField) -> Option<&str> {
    match field {
        CarryField::Title => Some(fields.title.as_str()),
        CarryField::Start => Some(fields.start.as_str()),
        CarryField::End => Some(fields.end.as_str()),
        CarryField::AllDay => None,
        CarryField::Location => fields.location.as_deref(),
        CarryField::Description => fields.description.as_deref(),
    }
}

fn differs(before: &CarryableFields, after: &CarryableFields, field: CarryField) -> bool {
    match field {
        CarryField::Start => !same_instant(&before.start, &after.start),
        CarryField::End => !same_instant(&before.end, &after.end),
        CarryField::AllDay => before.all_day != after.all_day,
        other => norm(field_of(before, other)) != norm(field_of(after, other)),
    }
}

/// What carrying this edit to the group's other copies would do.
///
/// Decided from the data, before the question is even asked: with nothing
/// changed there is nothing to carry and no reason to ask, and with every other
/// member read-only the honest answer is "this cannot be carried" rather than a
/// dialog that does nothing.
///
/// `members` is the group's membership, already annotated by the caller with the
/// name to show and whether Aperio may write there — two things only a surface
/// knows. The `anchor` is the copy that was edited, and is excluded, having been
/// saved already.
pub fn plan_carry(
    members: &[CarryTarget],
    anchor: (&str, &str),
    before: &CarryableFields,
    after: &CarryableFields,
) -> CarryPlan {
    let changed = CARRIED
        .into_iter()
        .filter(|&field| differs(before, after, field))
        .collect();
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for member in members {
        if (member.calendar_id.as_str(), member.event_id.as_str()) == anchor {
            continue;
        }
        if member.writable {
            targets.push(member.clone());
        } else {
            skipped.push(member.clone());
        }
    }
    let changed: Vec<CarryField> = changed;
    let worth_carrying = !changed.is_empty() && !targets.is_empty();
    CarryPlan {
        changed,
        targets,
        skipped,
        worth_carrying,
    }
}

/// Apply the carried fields onto one member's own current values.
///
/// A member keeps everything else it has — its calendar, its colour, and above
/// all its reminders. Only what the appointment IS travels.
pub fn carry_onto(
    member: &CarryableFields,
    after: &CarryableFields,
    changed: &[CarryField],
) -> CarryableFields {
    let mut next = member.clone();
    for &field in changed {
        match field {
            CarryField::Title => next.title = after.title.clone(),
            CarryField::Start => next.start = after.start.clone(),
            CarryField::End => next.end = after.end.clone(),
            CarryField::AllDay => next.all_day = after.all_day,
            CarryField::Location => next.location = after.location.clone(),
            CarryField::Description => next.description = after.description.clone(),
        }
    }
    next
}

/// Format an instant the way every caller's rows are spelled.
fn iso(ms: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|at| at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// The fields of the standalone row a carried OCCURRENCE edit creates.
///
/// An occurrence edit is not an update: the series gets an EXDATE and a single
/// event is created in its place. Carrying it means doing that on each copy, and
/// the row created there is NOT the anchor's — it is the member's own occurrence
/// with the carried fields laid over it. So a private copy keeps its own
/// reminder, its own colour and its own calendar; what travels is what the
/// appointment IS.
///
/// Start and end come from the member's own occurrence unless the edit MOVED it:
/// a title-only edit must not drag the copy to the master's start, and a moved
/// occurrence must land where the user put it. The copies are aligned by the
/// premise of the group, so the anchor's new instants are the right ones.
///
/// `None` when the occurrence instant cannot be read. See
/// [`future_carry_fields`] for why that is an answer rather than a panic.
pub fn occurrence_carry_fields(
    master: &CarryableFields,
    occurrence_iso: &str,
    after: &CarryableFields,
    changed: &[CarryField],
) -> Option<CarryableFields> {
    let occurrence_ms = millis(occurrence_iso)?;
    let duration = match (millis(&master.start), millis(&master.end)) {
        (Some(start), Some(end)) => (end - start).max(0),
        _ => 0,
    };
    let row = CarryableFields {
        start: occurrence_iso.to_string(),
        end: iso(occurrence_ms + duration)?,
        ..master.clone()
    };
    Some(carry_onto(&row, after, changed))
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// How many whole days a shift of `ms` is, rounding a tie towards the FUTURE.
///
/// This is JavaScript's `Math.round`, which is NOT Rust's. Both round to the
/// nearest whole number; they break a tie differently. `Math.round` goes
/// towards +∞ — `-0.5` is `-0` — while `f64::round` goes away from zero, so
/// `-0.5` is `-1`. For an all-day copy that is a whole day: an anchor moved
/// back by exactly twelve hours must not move the copy at all, and `f64::round`
/// would move it a day.
///
/// Done in INTEGERS, and that is not only tidiness. Adding half a day and
/// dividing with a floor is exactly `Math.round`'s tie rule, it is exact where
/// a float divide is approximate — and the float version compiled to
/// `i64.trunc_sat_f64_s`, a WebAssembly instruction from a feature `wasm-opt`
/// is not run with here, so the desktop's module would not build at all.
///
/// `div_euclid` is floor division for a positive divisor, which `/` is not:
/// `/` truncates towards zero, and that would break the tie the other way for
/// every backward move.
///
/// Pinned by `src/state/groupCarry.test.ts`, whose expectations were measured
/// from the TypeScript this replaces.
fn whole_days(ms: i64) -> i64 {
    (ms + DAY_MS / 2).div_euclid(DAY_MS)
}

/// The fields of the row a carried "this and all following" edit creates.
///
/// Unlike [`occurrence_carry_fields`] this one carries the MOVE, not the
/// instant. The two scopes differ exactly there: an occurrence edit lands on one
/// instant that every copy shares, while "and all following" cuts each copy at
/// ITS own next occurrence — which need not be the anchor's, because a copy may
/// run to a different pattern, or have that one occurrence deleted.
///
/// Writing the anchor's new instant onto such a copy was wrong twice over: the
/// head was truncated before the copy's own occurrence while the tail began at
/// the anchor's, so the two halves did not meet — the copy lost a real
/// appointment, gained one on a day it never had, and every occurrence after it
/// fell out of phase. So what travels is the SHIFT the user made (start moved by
/// so much, duration is now so long), applied to the copy's own cut point.
///
/// Two deliberate narrowings:
///   - an ALL-DAY copy moves in whole days, whatever the anchor's shift was to
///     the minute; a start that is not local midnight is not an all-day event;
///   - the anchor's new DURATION is adopted only when both agree about being
///     all-day, so an hour-long edit cannot shrink an all-day copy to an hour.
///
/// The duration is always derived, never an instant taken from the anchor: an
/// end lifted verbatim onto a copy cut at a different point could precede its
/// own start, which no provider will accept and some will accept and mangle.
///
/// # `None` means "this copy could not be carried"
///
/// A cut point that cannot be read has no answer, and inventing one would put a
/// row at an instant nobody chose. The TypeScript this replaces THREW here —
/// `new Date(NaN).toISOString()` raises — which abandoned the rest of the carry
/// mid-loop rather than reporting the one copy. The caller already has the right
/// shape for this: it keeps a list of members it could not write and reports
/// them, because a copy that silently kept its old shape is the contradiction
/// the group exists to prevent.
pub fn future_carry_fields(
    master: &CarryableFields,
    anchor_iso: &str,
    before: &CarryableFields,
    after: &CarryableFields,
    changed: &[CarryField],
) -> Option<CarryableFields> {
    let anchor_ms = millis(anchor_iso)?;
    let own_duration = match (millis(&master.start), millis(&master.end)) {
        (Some(start), Some(end)) => (end - start).max(0),
        _ => 0,
    };
    let moved_by = if changed.contains(&CarryField::Start) {
        match (millis(&after.start), millis(&before.start)) {
            (Some(a), Some(b)) => a - b,
            _ => 0,
        }
    } else {
        0
    };
    let shift = if master.all_day {
        whole_days(moved_by) * DAY_MS
    } else {
        moved_by
    };
    let same_kind = master.all_day == after.all_day;
    let duration = if changed.contains(&CarryField::End) && same_kind {
        match (millis(&after.end), millis(&after.start)) {
            (Some(end), Some(start)) => (end - start).max(0),
            _ => own_duration,
        }
    } else {
        own_duration
    };
    let start = anchor_ms + shift;
    let row = CarryableFields {
        start: iso(start)?,
        end: iso(start + duration)?,
        ..master.clone()
    };
    // Everything else travels as it does everywhere else; start and end were
    // just decided above and must not be overwritten with the anchor's
    // instants.
    let rest: Vec<CarryField> = changed
        .iter()
        .copied()
        .filter(|&f| f != CarryField::Start && f != CarryField::End)
        .collect();
    Some(carry_onto(&row, after, &rest))
}

/// Everything [`plan_carry`] needs, in one JSON value.
#[derive(Debug, Clone, Deserialize)]
pub struct PlanCarryInput {
    /// The group's membership, annotated with what only a surface knows.
    pub members: Vec<CarryTarget>,
    pub anchor_calendar_id: String,
    pub anchor_event_id: String,
    pub before: CarryableFields,
    pub after: CarryableFields,
}

/// Everything the two row-shaped rules need, in one JSON value.
#[derive(Debug, Clone, Deserialize)]
pub struct CarryRowInput {
    /// The member's own current values.
    pub master: CarryableFields,
    /// The instant this copy is cut at, or the occurrence being replaced.
    pub at: String,
    pub before: CarryableFields,
    pub after: CarryableFields,
    pub changed: Vec<CarryField>,
}

/// Plan from JSON, and answer with JSON.
pub fn plan_carry_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: PlanCarryInput = serde_json::from_str(input_json)?;
    let plan = plan_carry(
        &input.members,
        (&input.anchor_calendar_id, &input.anchor_event_id),
        &input.before,
        &input.after,
    );
    serde_json::to_string(&plan)
}

/// The occurrence row's fields, from JSON. `"null"` when the instant cannot be
/// read, which means this copy could not be carried.
pub fn occurrence_carry_fields_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: CarryRowInput = serde_json::from_str(input_json)?;
    let fields = occurrence_carry_fields(&input.master, &input.at, &input.after, &input.changed);
    serde_json::to_string(&fields)
}

/// The "and all following" row's fields, from JSON. `"null"` when the cut point
/// cannot be read.
pub fn future_carry_fields_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: CarryRowInput = serde_json::from_str(input_json)?;
    let fields = future_carry_fields(
        &input.master,
        &input.at,
        &input.before,
        &input.after,
        &input.changed,
    );
    serde_json::to_string(&fields)
}

/// Lay the carried fields onto a member's own values, from JSON.
pub fn carry_onto_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: CarryRowInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&carry_onto(&input.master, &input.after, &input.changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(start: &str, end: &str) -> CarryableFields {
        CarryableFields {
            title: "Wochenplanung".into(),
            start: start.into(),
            end: end.into(),
            all_day: false,
            location: None,
            description: None,
        }
    }

    fn base() -> CarryableFields {
        fields("2026-08-10T08:00:00Z", "2026-08-10T09:00:00Z")
    }

    fn member(calendar_id: &str, event_id: &str, writable: bool) -> CarryTarget {
        CarryTarget {
            calendar_id: calendar_id.into(),
            event_id: event_id.into(),
            title: format!("Kopie {event_id}"),
            writable,
        }
    }

    fn members() -> Vec<CarryTarget> {
        vec![
            member("work", "ev-a", true),
            member("private", "ev-b", true),
            // The colleague's calendar is the read-only one, as it usually is.
            member("colleague", "ev-c", false),
        ]
    }

    #[test]
    fn it_names_what_changed_who_gets_it_and_who_cannot() {
        let plan = plan_carry(
            &members(),
            ("work", "ev-a"),
            &base(),
            &fields("2026-08-10T10:00:00Z", "2026-08-10T11:00:00Z"),
        );
        assert_eq!(plan.changed, vec![CarryField::Start, CarryField::End]);
        assert_eq!(
            plan.targets.iter().map(|t| &t.event_id).collect::<Vec<_>>(),
            vec!["ev-b"],
        );
        // The one it must not write is reported, never quietly dropped — that
        // silence is what produces the contradiction groups exist to prevent.
        assert_eq!(
            plan.skipped.iter().map(|t| &t.event_id).collect::<Vec<_>>(),
            vec!["ev-c"],
        );
        assert!(plan.worth_carrying);
    }

    #[test]
    fn the_edited_copy_is_left_out_of_its_own_carry() {
        let mut after = base();
        after.title = "Neu".into();
        let plan = plan_carry(&members(), ("work", "ev-a"), &base(), &after);
        assert!(plan.targets.iter().all(|t| t.event_id != "ev-a"));
        assert!(plan.skipped.iter().all(|t| t.event_id != "ev-a"));
    }

    /// The user only changed the reminder — a property of THIS copy, and very
    /// often the whole reason the copy exists.
    #[test]
    fn nothing_carried_is_not_worth_asking_about() {
        let plan = plan_carry(&members(), ("work", "ev-a"), &base(), &base());
        assert!(plan.changed.is_empty());
        assert!(!plan.worth_carrying);
    }

    #[test]
    fn every_other_copy_read_only_is_not_worth_asking_about() {
        let read_only = vec![
            member("work", "ev-a", true),
            member("private", "ev-b", false),
            member("colleague", "ev-c", false),
        ];
        let mut after = base();
        after.title = "Neu".into();
        let plan = plan_carry(&read_only, ("work", "ev-a"), &base(), &after);
        assert!(plan.targets.is_empty());
        assert_eq!(plan.skipped.len(), 2);
        assert!(!plan.worth_carrying);
    }

    /// Compared as strings, EVERY save looked like a move: a reminder-only
    /// edit asked to carry, and carrying rewrote start and end onto every copy.
    #[test]
    fn a_reserialised_start_is_not_a_change() {
        let plan = plan_carry(
            &members(),
            ("work", "ev-a"),
            &base(),
            &fields("2026-08-10T08:00:00.000Z", "2026-08-10T09:00:00.000Z"),
        );
        assert!(plan.changed.is_empty());
    }

    /// Providers disagree about which they return for a field nobody filled
    /// in; an absent-to-empty flip is not a change anyone made.
    #[test]
    fn empty_and_absent_are_the_same_value() {
        let mut after = base();
        after.location = Some(String::new());
        let plan = plan_carry(&members(), ("work", "ev-a"), &base(), &after);
        assert!(plan.changed.is_empty());
    }

    #[test]
    fn only_what_changed_travels() {
        let mut mine = base();
        mine.location = Some("Raum 3".into());
        let mut after = base();
        after.title = "Wochenplanung neu".into();
        after.location = Some("Raum 4".into());
        let carried = carry_onto(&mine, &after, &[CarryField::Title]);
        assert_eq!(carried.title, "Wochenplanung neu");
        // Not among the changed fields, so it stays.
        assert_eq!(carried.location.as_deref(), Some("Raum 3"));
    }

    #[test]
    fn an_occurrence_lands_on_the_copys_own_day() {
        let master = fields("2026-08-03T08:00:00.000Z", "2026-08-03T09:00:00.000Z");
        let mut after = base();
        after.title = "Wochenplanung kurz".into();
        let row = occurrence_carry_fields(
            &master,
            "2026-08-24T08:00:00.000Z",
            &after,
            &[CarryField::Title],
        )
        .expect("a readable occurrence");
        assert_eq!(row.start, "2026-08-24T08:00:00.000Z");
        assert_eq!(row.end, "2026-08-24T09:00:00.000Z");
        assert_eq!(row.title, "Wochenplanung kurz");
    }

    #[test]
    fn a_moved_occurrence_lands_where_the_user_put_it() {
        let master = fields("2026-08-03T08:00:00.000Z", "2026-08-03T09:00:00.000Z");
        let after = fields("2026-08-24T10:00:00.000Z", "2026-08-24T11:00:00.000Z");
        let row = occurrence_carry_fields(
            &master,
            "2026-08-24T08:00:00.000Z",
            &after,
            &[CarryField::Start, CarryField::End],
        )
        .expect("a readable occurrence");
        assert_eq!(row.start, "2026-08-24T10:00:00.000Z");
        assert_eq!(row.end, "2026-08-24T11:00:00.000Z");
    }

    // ── "this and all following" ────────────────────────────────────────
    //
    // Every expectation below was MEASURED from the TypeScript this replaces,
    // and is pinned there too (`src/state/groupCarry.test.ts`).

    /// The copy's own master: a week later than the anchor's, an hour long.
    fn copy_master() -> CarryableFields {
        fields("2026-08-12T14:00:00Z", "2026-08-12T15:00:00Z")
    }
    const COPY_CUT: &str = "2026-08-12T14:00:00Z";

    #[test]
    fn a_title_only_edit_leaves_the_copys_instants_alone() {
        let mut after = base();
        after.title = "Neuer Name".into();
        let row = future_carry_fields(
            &copy_master(),
            COPY_CUT,
            &base(),
            &after,
            &[CarryField::Title],
        )
        .expect("a readable cut point");
        assert_eq!(row.start, "2026-08-12T14:00:00.000Z");
        assert_eq!(row.end, "2026-08-12T15:00:00.000Z");
        assert_eq!(row.title, "Neuer Name");
    }

    /// The SHIFT travels, applied to the copy's own cut point. Writing the
    /// anchor's new instant here would put the tail three days before the head.
    #[test]
    fn the_shift_travels_not_the_instant() {
        let row = future_carry_fields(
            &copy_master(),
            COPY_CUT,
            &base(),
            &fields("2026-08-10T09:00:00Z", "2026-08-10T10:00:00Z"),
            &[CarryField::Start, CarryField::End],
        )
        .expect("a readable cut point");
        assert_eq!(row.start, "2026-08-12T15:00:00.000Z");
        assert_eq!(row.end, "2026-08-12T16:00:00.000Z");
    }

    #[test]
    fn a_backward_move_travels_the_same_way() {
        let row = future_carry_fields(
            &copy_master(),
            COPY_CUT,
            &base(),
            &fields("2026-08-09T08:00:00Z", "2026-08-09T09:00:00Z"),
            &[CarryField::Start, CarryField::End],
        )
        .expect("a readable cut point");
        assert_eq!(row.start, "2026-08-11T14:00:00.000Z");
    }

    #[test]
    fn a_new_duration_without_a_move() {
        let mut after = base();
        after.end = "2026-08-10T11:00:00Z".into();
        let row = future_carry_fields(
            &copy_master(),
            COPY_CUT,
            &base(),
            &after,
            &[CarryField::End],
        )
        .expect("a readable cut point");
        assert_eq!(row.start, "2026-08-12T14:00:00.000Z");
        assert_eq!(row.end, "2026-08-12T17:00:00.000Z");
    }

    fn all_day_copy() -> CarryableFields {
        CarryableFields {
            all_day: true,
            ..fields("2026-08-12T00:00:00Z", "2026-08-13T00:00:00Z")
        }
    }

    #[test]
    fn an_all_day_copy_moves_in_whole_days() {
        let row = future_carry_fields(
            &all_day_copy(),
            "2026-08-12T00:00:00Z",
            &base(),
            &fields("2026-08-10T20:00:00Z", "2026-08-10T21:00:00Z"),
            &[CarryField::Start, CarryField::End],
        )
        .expect("a readable cut point");
        // Twelve hours, rounded to a day.
        assert_eq!(row.start, "2026-08-13T00:00:00.000Z");
        // And an hour-long edit must not shrink it: the two disagree about
        // being all-day, so the copy keeps its own duration.
        assert_eq!(row.end, "2026-08-14T00:00:00.000Z");
    }

    /// THE TIE, and the reason [`round_half_up`] exists.
    ///
    /// `Math.round` breaks it towards +∞, so exactly twelve hours forward is a
    /// day and exactly twelve hours back is nothing. `f64::round` breaks it
    /// away from zero, which would move an all-day copy a whole day further
    /// back on every exact half-day rewind — silently.
    #[test]
    fn an_exact_half_day_tie_breaks_towards_the_future() {
        let back = future_carry_fields(
            &all_day_copy(),
            "2026-08-12T00:00:00Z",
            &base(),
            &fields("2026-08-09T20:00:00Z", "2026-08-09T21:00:00Z"),
            &[CarryField::Start, CarryField::End],
        )
        .expect("a readable cut point");
        assert_eq!(
            back.start, "2026-08-12T00:00:00.000Z",
            "exactly twelve hours back must not move the copy",
        );

        let further = future_carry_fields(
            &all_day_copy(),
            "2026-08-12T00:00:00Z",
            &base(),
            &fields("2026-08-08T20:00:00Z", "2026-08-08T21:00:00Z"),
            &[CarryField::Start, CarryField::End],
        )
        .expect("a readable cut point");
        assert_eq!(further.start, "2026-08-11T00:00:00.000Z");
    }

    #[test]
    fn both_all_day_adopts_the_new_duration() {
        let mut before = base();
        before.all_day = true;
        let after = CarryableFields {
            all_day: true,
            ..fields("2026-08-10T00:00:00Z", "2026-08-13T00:00:00Z")
        };
        let row = future_carry_fields(
            &all_day_copy(),
            "2026-08-12T00:00:00Z",
            &before,
            &after,
            &[CarryField::Start, CarryField::End],
        )
        .expect("a readable cut point");
        assert_eq!(row.start, "2026-08-12T00:00:00.000Z");
        assert_eq!(row.end, "2026-08-15T00:00:00.000Z");
    }

    /// An unreadable cut point has no answer, and inventing one would put a row
    /// at an instant nobody chose. The TypeScript this replaces THREW here,
    /// abandoning the rest of the carry mid-loop rather than reporting the one
    /// copy.
    #[test]
    fn an_unreadable_cut_point_carries_nothing() {
        assert!(future_carry_fields(
            &copy_master(),
            "irgendwann",
            &base(),
            &fields("2026-08-10T09:00:00Z", "2026-08-10T10:00:00Z"),
            &[CarryField::Start, CarryField::End],
        )
        .is_none());
        assert!(occurrence_carry_fields(
            &copy_master(),
            "irgendwann",
            &base(),
            &[CarryField::Title],
        )
        .is_none());
    }
}
