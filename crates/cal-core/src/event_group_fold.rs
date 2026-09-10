//! Folding a group into one row (`DESIGN-event-groups.md`, Stufe 1).
//!
//! A group says several events mean the same appointment. Here that changes
//! what a day looks like: four rows that are one commitment become one row that
//! names the calendars it spans. It is the largest everyday gain of the
//! feature, and for a screen-reader user not a cosmetic one — three fewer
//! things to walk past, every time.
//!
//! A day that read differently on the phone than on the desktop would be worse
//! than not folding at all, which is why the decision is here rather than in
//! either surface.
//!
//! # Call it with ONE DAY's rows
//!
//! Not a whole week, and the reason is the divergence check. A recurring
//! appointment renders one row per day, so a group of two series over a
//! five-day range hands in ten rows with five different start times — read
//! across the range that looks exactly like "the copies have drifted apart",
//! and nothing would ever fold. Within one day the question is the right one
//! again: two copies of the same appointment, today, at different times.
//!
//! Every view already buckets by day before it renders, so this costs nothing.

use serde::{Deserialize, Serialize};

use crate::group_suggestion::is_meeting_calendar;
use crate::EventGroup;

/// The minimum a row has to carry to be foldable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct FoldableEvent {
    pub calendar_id: String,
    /// The id membership is keyed by — the series master, resolved by the
    /// caller, because only a rendered row knows which series it belongs to.
    pub series_id: String,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub all_day: bool,
    /// Whether the user can ACT on this row — see [`actionable`].
    ///
    /// Absent means "the core decides", which is the answer every caller
    /// wants: the only rows Aperio has that cannot be acted on are the
    /// read-only videoconference copies, and the core recognises those. It is
    /// an option rather than a plain bool so a caller that knows more — which
    /// calendars are read-only, say — can still say so without the rule having
    /// to guess.
    #[serde(default)]
    pub actionable: Option<bool>,
}

/// One rendered row after folding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct CollapsedRow {
    /// The row to draw, as a POSITION in the input. For a group, the member
    /// that was chosen to stand for it — which is not always the row whose
    /// slot this is.
    pub event: usize,
    /// The id of the group this row stands for, when it stands for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// How many OTHER events the group holds — from the group itself, not from
    /// what happened to be on screen. A copy in a calendar the user has
    /// switched off is still a copy, and saying "with 2 others" is what makes
    /// the count match their own memory of how many they keep.
    pub other_members: usize,
    /// The calendars the group spans, this row's own first.
    pub calendar_ids: Vec<String>,
    /// The members in view disagree about when the appointment is.
    ///
    /// Then the group is a claim that has stopped being true — one copy was
    /// moved and the others were not. Folding it silently would hide exactly
    /// the problem the user needs to see, so the row says so and the divergent
    /// members are NOT folded away.
    pub diverged: bool,
}

/// Everything [`collapse_event_groups`] needs, in one JSON value.
#[derive(Debug, Clone, Deserialize)]
pub struct FoldInput {
    pub events: Vec<FoldableEvent>,
    #[serde(default)]
    pub groups: Vec<EventGroup>,
}

/// The moment, as an INSTANT rather than as the string it arrived in.
///
/// The same instant reaches this rule spelled two ways: a frontend rewrites
/// every expanded occurrence through its own ISO formatter
/// (`…T08:00:00.000Z`) while a one-off passes through with the backend's
/// serialisation (`…T08:00:00Z`), and some providers add sub-second precision.
/// Comparing the raw strings made a series grouped with a single event
/// permanently "diverged": it never folded, and BOTH copies announced "which is
/// now at a different time" — every day, for two events at the identical
/// instant.
fn start_key(event: &FoldableEvent) -> String {
    let raw = event.start.as_deref().unwrap_or_default();
    if event.all_day {
        return format!("day:{}", raw.chars().take(10).collect::<String>());
    }
    match raw.parse::<chrono::DateTime<chrono::Utc>>() {
        Ok(at) => format!("at:{}", at.timestamp_millis()),
        // Unreadable: compared as it stands rather than folded onto every
        // other unreadable one.
        Err(_) => format!("at:{raw}"),
    }
}

/// Whether this row is one the user can ACT on — the tie-breaker for which
/// member a folded group shows.
///
/// Position alone decided it before, and position is not a property of the
/// data: the members of a folded group are at the identical instant (a
/// difference marks the group diverged and nothing folds), so the sort is a tie
/// and the order falls through to whatever the calendar fan-out happened to
/// produce. The row could therefore be the read-only videoconference copy: no
/// editor, no delete, no move, and on a phone not even a button. Which one won
/// could differ between two launches with the same data.
///
/// Worse for a screen reader, it changed UNDER the user: at first paint the
/// meeting row is filtered out and the appointment is the row; a beat later the
/// groups arrive, the meeting is re-admitted, and the row at the same index
/// silently becomes a different event.
///
/// This answers for the only rows Aperio has that cannot be acted on.
fn actionable(event: &FoldableEvent) -> bool {
    event
        .actionable
        .unwrap_or_else(|| !is_meeting_calendar(&event.calendar_id))
}

/// Fold each group's members into a single row, keeping the input order.
///
/// The representative is chosen over the whole window before anything is
/// emitted — the first ACTIONABLE member, else the first at all — because the
/// row that stands for the group has to be picked from all of its members and
/// not from the one that happened to come first.
///
/// The SLOT, though, is the first member's: folding removes rows, it never
/// moves one. So the day's sorting stays exactly as the caller decided it.
pub fn collapse_event_groups(events: &[FoldableEvent], groups: &[EventGroup]) -> Vec<CollapsedRow> {
    let alone = |i: usize, event: &FoldableEvent| CollapsedRow {
        event: i,
        group_id: None,
        other_members: 0,
        calendar_ids: vec![event.calendar_id.clone()],
        diverged: false,
    };
    if groups.is_empty() {
        return events
            .iter()
            .enumerate()
            .map(|(i, e)| alone(i, e))
            .collect();
    }

    let mut by_member: std::collections::HashMap<(&str, &str), &EventGroup> =
        std::collections::HashMap::new();
    for group in groups {
        for member in &group.members {
            by_member.insert(
                (member.calendar_id.as_str(), member.event_id.as_str()),
                group,
            );
        }
    }
    let group_of = |event: &FoldableEvent| {
        by_member
            .get(&(event.calendar_id.as_str(), event.series_id.as_str()))
            .copied()
    };

    // First pass: which groups are represented here, and do their members
    // agree about when the appointment is. Divergence has to be known BEFORE
    // the first member is folded, because it decides whether to fold at all.
    let mut starts_by_group: std::collections::HashMap<&str, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for event in events {
        if let Some(group) = group_of(event) {
            starts_by_group
                .entry(group.id.as_str())
                .or_default()
                .insert(start_key(event));
        }
    }

    // Which member each group SHOWS.
    let mut show_for: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, event) in events.iter().enumerate() {
        let Some(group) = group_of(event) else {
            continue;
        };
        match show_for.get(group.id.as_str()) {
            Some(&current) if actionable(&events[current]) || !actionable(event) => {}
            _ => {
                show_for.insert(group.id.as_str(), i);
            }
        }
    }

    let mut represented: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (i, event) in events.iter().enumerate() {
        let Some(group) = group_of(event) else {
            out.push(alone(i, event));
            continue;
        };
        let diverged = starts_by_group
            .get(group.id.as_str())
            .map_or(1, std::collections::HashSet::len)
            > 1;
        // A group whose members have drifted apart is not folded: every copy
        // stays visible, each marked, because that disagreement is the thing to
        // act on.
        if !diverged && represented.contains(group.id.as_str()) {
            continue;
        }
        represented.insert(group.id.as_str());
        let shown_at = if diverged {
            i
        } else {
            show_for.get(group.id.as_str()).copied().unwrap_or(i)
        };
        let shown = &events[shown_at];
        let mut calendar_ids = vec![shown.calendar_id.clone()];
        for member in &group.members {
            if !calendar_ids.contains(&member.calendar_id) {
                calendar_ids.push(member.calendar_id.clone());
            }
        }
        out.push(CollapsedRow {
            event: shown_at,
            group_id: Some(group.id.clone()),
            other_members: group.members.len().saturating_sub(1),
            calendar_ids,
            diverged,
        });
    }
    out
}

/// Fold from JSON, and answer with JSON.
///
/// The door both frontends come through; the marshalling lives here so the two
/// cannot drift in what they accept.
pub fn collapse_event_groups_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: FoldInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&collapse_event_groups(&input.events, &input.groups))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventGroupMember;

    fn row(calendar_id: &str, series_id: &str, start: &str) -> FoldableEvent {
        FoldableEvent {
            calendar_id: calendar_id.into(),
            series_id: series_id.into(),
            start: Some(start.into()),
            all_day: false,
            actionable: None,
        }
    }

    fn group(id: &str, members: &[(&str, &str)]) -> EventGroup {
        EventGroup {
            id: id.into(),
            created_at: "2026-08-09T12:00:00Z".into(),
            updated_at: "2026-08-09T12:00:00Z".into(),
            members: members
                .iter()
                .map(|(cal, ev)| EventGroupMember {
                    calendar_id: (*cal).into(),
                    event_id: (*ev).into(),
                    title: "Wochenplanung".into(),
                    starts_at: "2026-08-10T08:00:00Z".into(),
                    added_at: "2026-08-09T12:00:00Z".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_group_becomes_one_row_that_names_its_calendars() {
        let events = [
            row("work", "ev-a", "2026-08-10T08:00:00Z"),
            row("private", "ev-b", "2026-08-10T08:00:00Z"),
        ];
        let folded = collapse_event_groups(
            &events,
            &[group("g1", &[("work", "ev-a"), ("private", "ev-b")])],
        );
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].event, 0);
        assert_eq!(folded[0].group_id.as_deref(), Some("g1"));
        assert_eq!(folded[0].other_members, 1);
        assert_eq!(folded[0].calendar_ids, vec!["work", "private"]);
        assert!(!folded[0].diverged);
    }

    /// The count comes from the GROUP, not from what is on screen. A copy in a
    /// calendar the user switched off is still a copy.
    #[test]
    fn the_count_is_the_groups_and_not_the_windows() {
        let events = [row("work", "ev-a", "2026-08-10T08:00:00Z")];
        let folded = collapse_event_groups(
            &events,
            &[group(
                "g1",
                &[("work", "ev-a"), ("private", "ev-b"), ("coll", "ev-c")],
            )],
        );
        assert_eq!(folded[0].other_members, 2);
        assert_eq!(folded[0].calendar_ids, vec!["work", "private", "coll"]);
    }

    /// A read-only videoconference copy must not be the row that stands for the
    /// group: no editor, no delete, and on a phone not even a button. Position
    /// alone used to decide it, and position is not a property of the data.
    #[test]
    fn an_actionable_member_stands_for_the_group_even_when_a_meeting_came_first() {
        let events = [
            row("acc::meetings", "vc::1", "2026-08-10T08:00:00Z"),
            row("work", "ev-a", "2026-08-10T08:00:00Z"),
        ];
        let folded = collapse_event_groups(
            &events,
            &[group("g1", &[("acc::meetings", "vc::1"), ("work", "ev-a")])],
        );
        assert_eq!(folded.len(), 1);
        assert_eq!(
            folded[0].event, 1,
            "the appointment stands, not the meeting"
        );
        // The SLOT is still the first member's — folding removes rows, it
        // never moves one — so the row's own calendar leads.
        assert_eq!(folded[0].calendar_ids[0], "work");
    }

    /// With nothing else to show, the meeting row stands for the group rather
    /// than the group disappearing.
    #[test]
    fn a_meeting_alone_still_stands_for_its_group() {
        let events = [row("acc::meetings", "vc::1", "2026-08-10T08:00:00Z")];
        let folded = collapse_event_groups(
            &events,
            &[group("g1", &[("acc::meetings", "vc::1"), ("work", "ev-a")])],
        );
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].event, 0);
    }

    /// A group whose copies no longer agree about the time is NOT folded. That
    /// disagreement is the thing to act on, and hiding it is what the whole
    /// feature is against.
    #[test]
    fn a_diverged_group_keeps_every_copy_visible() {
        let events = [
            row("work", "ev-a", "2026-08-10T08:00:00Z"),
            row("private", "ev-b", "2026-08-10T10:00:00Z"),
        ];
        let folded = collapse_event_groups(
            &events,
            &[group("g1", &[("work", "ev-a"), ("private", "ev-b")])],
        );
        assert_eq!(folded.len(), 2);
        assert!(folded.iter().all(|r| r.diverged));
        assert_eq!(folded[0].event, 0);
        assert_eq!(folded[1].event, 1);
    }

    /// The same instant, spelled two ways, is one instant.
    ///
    /// A frontend rewrites every expanded occurrence through its own ISO
    /// formatter while a one-off keeps the backend's. Comparing the strings
    /// made such a group permanently "diverged": it never folded, and BOTH
    /// copies announced a disagreement that did not exist.
    #[test]
    fn two_spellings_of_one_instant_do_not_read_as_a_divergence() {
        let events = [
            row("work", "ev-a", "2026-08-10T08:00:00.000Z"),
            row("private", "ev-b", "2026-08-10T08:00:00Z"),
        ];
        let folded = collapse_event_groups(
            &events,
            &[group("g1", &[("work", "ev-a"), ("private", "ev-b")])],
        );
        assert_eq!(folded.len(), 1);
        assert!(!folded[0].diverged);
    }

    /// Folding removes rows; it never moves one.
    #[test]
    fn the_days_order_survives_the_fold() {
        let events = [
            row("work", "ev-early", "2026-08-10T07:00:00Z"),
            row("work", "ev-a", "2026-08-10T08:00:00Z"),
            row("private", "ev-b", "2026-08-10T08:00:00Z"),
            row("work", "ev-late", "2026-08-10T09:00:00Z"),
        ];
        let folded = collapse_event_groups(
            &events,
            &[group("g1", &[("work", "ev-a"), ("private", "ev-b")])],
        );
        assert_eq!(
            folded.iter().map(|r| r.event).collect::<Vec<_>>(),
            vec![0, 1, 3],
        );
    }

    #[test]
    fn without_groups_every_row_stands_for_itself() {
        let events = [
            row("work", "ev-a", "2026-08-10T08:00:00Z"),
            row("private", "ev-b", "2026-08-10T09:00:00Z"),
        ];
        let folded = collapse_event_groups(&events, &[]);
        assert_eq!(folded.len(), 2);
        assert!(folded
            .iter()
            .all(|r| r.group_id.is_none() && r.other_members == 0 && !r.diverged));
    }

    /// An all-day copy agrees on the DAY, so two spellings of the same day do
    /// not read as a divergence either.
    #[test]
    fn all_day_copies_agree_on_the_day() {
        let mut a = row("work", "ev-a", "2026-08-10");
        let mut b = row("private", "ev-b", "2026-08-10T00:00:00Z");
        a.all_day = true;
        b.all_day = true;
        let folded = collapse_event_groups(
            &[a, b],
            &[group("g1", &[("work", "ev-a"), ("private", "ev-b")])],
        );
        assert_eq!(folded.len(), 1);
        assert!(!folded[0].diverged);
    }
}
