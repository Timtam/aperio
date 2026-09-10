//! Recognising a copy: "these two rows look like one appointment."
//!
//! Aperio never groups events by itself on a resemblance. The design says why
//! in one sentence — an office full of "Team meeting" at 10:00 would have two
//! different meetings declared one appointment, and a wrong group hides a real
//! commitment behind a copy of something else. So what this module produces is
//! an OFFER, and only the user's confirmation makes a group
//! (`DESIGN-event-groups.md`, Stufe 3).
//!
//! Two questions live here, and they are deliberately not the same one:
//!
//! - [`suggest_group_mate`] answers "which of these is a copy of THAT one" for
//!   someone who has already opened the grouping dialog.
//! - [`find_group_suggestions`] answers the question nobody asked — are there
//!   copies in this day at all? — and therefore has to be far more careful:
//!   it speaks unprompted, so a near miss offered every morning is worse than
//!   no offer.
//!
//! # Why the caller resolves the series id
//!
//! Both take rows that already carry a `series_id`. The core cannot work it
//! out: a frontend renders a recurring appointment as one synthetic row per
//! occurrence, and only that row knows which series it belongs to
//! ([`crate::series_master_id`] answers for a provider-sent override and
//! nothing else). Rather than guess, the rule asks for the answer.

use serde::{Deserialize, Serialize};

use crate::{normalized_title, EventGroup, SuggestionDecline};

/// The suffix that turns an account id into its meetings-calendar id.
///
/// Aperio's own convention, not a provider's. It lives here because two
/// separate rules read it — the host mints and routes these calendars, and the
/// suggestion rule below refuses to offer their rows — and a suffix understood
/// differently in two places would mean a row one of them drops and the other
/// never pairs.
pub const MEETINGS_CALENDAR_SUFFIX: &str = "::meetings";

/// Whether this calendar is the read-only calendar of an account's meetings.
pub fn is_meeting_calendar(calendar_id: &str) -> bool {
    calendar_id.ends_with(MEETINGS_CALENDAR_SUFFIX)
}

/// One row, reduced to what recognising a copy needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct SuggestibleEvent {
    pub calendar_id: String,
    /// The id this row would be GROUPED under — the series master. Resolved by
    /// the caller; see the module doc.
    pub series_id: String,
    pub title: String,
    /// RFC 3339, or a bare `YYYY-MM-DD` for an all-day row.
    pub start: String,
    #[serde(default)]
    pub all_day: bool,
}

/// Two rows that look like one appointment, as positions in the input.
///
/// Positions rather than the rows themselves: the caller holds the full events
/// and needs them back to render a title and a calendar name, and echoing them
/// across the boundary would double the payload to say nothing new.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct GroupSuggestion {
    pub first: usize,
    pub second: usize,
}

/// What "the same time" means for one row.
///
/// An all-day row agrees on the DAY, a timed one on the instant. The two are
/// different kinds of answer on purpose, and mixing them is what the separate
/// variants prevent: a timed row must never read as equal to an all-day row
/// that merely falls on its day.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WhenKey {
    Day(String),
    At(i64),
    /// A start nothing can be made of. It matches NOTHING, including another
    /// unreadable one — two rows Aperio cannot place are not evidence that
    /// they are the same appointment.
    Unreadable,
}

fn when_key(event: &SuggestibleEvent) -> WhenKey {
    if event.all_day {
        // The first ten characters, which is the date however the rest is
        // written. The frontend rule this replaces sliced the same way.
        let day: String = event.start.chars().take(10).collect();
        if day.len() < 10 {
            return WhenKey::Unreadable;
        }
        return WhenKey::Day(day);
    }
    match event.start.parse::<chrono::DateTime<chrono::Utc>>() {
        Ok(at) => WhenKey::At(at.timestamp_millis()),
        Err(_) => WhenKey::Unreadable,
    }
}

fn same_time(a: &WhenKey, b: &WhenKey) -> bool {
    match (a, b) {
        (WhenKey::Unreadable, _) | (_, WhenKey::Unreadable) => false,
        _ => a == b,
    }
}

/// The pair, in the canonical order a decline record uses.
fn pair_key<'a>(
    a: (&'a str, &'a str),
    b: (&'a str, &'a str),
) -> ((&'a str, &'a str), (&'a str, &'a str)) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The row that most looks like a copy of `anchor`, or `None`.
///
/// Three conditions, all required:
///
/// - the SAME title, ignoring case, padding and the width of the gaps between
///   words — a copy is made by copying, so a near-match is far more often two
///   different things than one;
/// - the SAME start (the same day for all-day rows) — "overlapping" would
///   catch the meeting before this one, which is exactly the wrong answer;
/// - a DIFFERENT calendar — two rows in one calendar are a duplicate to clean
///   up, not an appointment that lives in several places.
///
/// Ties go to the first candidate in the order the caller supplied, which is
/// the order the user sees. Nothing here writes anything: a suggestion is an
/// offer, and the user's confirmation is what makes a group.
pub fn suggest_group_mate(
    anchor: &SuggestibleEvent,
    candidates: &[SuggestibleEvent],
) -> Option<usize> {
    let title = normalized_title(&anchor.title);
    if title.is_empty() {
        return None;
    }
    let when = when_key(anchor);
    candidates.iter().position(|candidate| {
        candidate.calendar_id != anchor.calendar_id
            && normalized_title(&candidate.title) == title
            && same_time(&when_key(candidate), &when)
    })
}

/// Copies worth offering, among the rows of ONE day.
///
/// One day's rows, like the folding rule and for the same reason: a recurring
/// appointment renders a row per day, and across a range its own days would
/// pair up with each other.
///
/// Three rules keep an unprompted offer from becoming noise:
///
/// - the same strict match [`suggest_group_mate`] uses;
/// - never about rows that are already grouped, which is the answer to the
///   question already given;
/// - never about a pair the user has DECLINED. That record is what turns a
///   suggestion into a question asked once instead of a daily interruption,
///   and it is why migration 0037 exists.
///
/// At most one pair per row, and the caller decides how many to show — a day
/// that somehow produces six suggestions is a day where something is wrong
/// with the matching, and six offers is not the way to find that out.
///
/// # A meeting row is never offered on a resemblance
///
/// This whole function guesses from a name and a time, which is why its answer
/// is an offer rather than a group. A videoconference meeting is the one row
/// that does not need guessing: it carries the join URL its provider issued,
/// and the meeting-link grouping pairs it on that identity. Offering it here
/// put the two mechanisms in each other's way — Aperio writes the event's own
/// title into the meeting it creates, so "same name, same time" is nearly
/// guaranteed for the wrong reasons.
///
/// Answering that offer wrote a refusal NAMING the meeting, and a refusal is
/// forever. The user was asked the wrong question and their answer was kept.
pub fn find_group_suggestions(
    events: &[SuggestibleEvent],
    groups: &[EventGroup],
    declines: &[SuggestionDecline],
) -> Vec<GroupSuggestion> {
    let mut grouped: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
    for group in groups {
        for member in &group.members {
            grouped.insert((member.calendar_id.as_str(), member.event_id.as_str()));
        }
    }
    let declined: std::collections::HashSet<((&str, &str), (&str, &str))> = declines
        .iter()
        .filter(|d| d.is_declined())
        .map(|d| {
            pair_key(
                (d.calendar_a.as_str(), d.event_a.as_str()),
                (d.calendar_b.as_str(), d.event_b.as_str()),
            )
        })
        .collect();

    let offerable = |ev: &SuggestibleEvent| !is_meeting_calendar(&ev.calendar_id);
    let key_of = |ev: &SuggestibleEvent| (ev.calendar_id.clone(), ev.series_id.clone());

    let mut out = Vec::new();
    let mut spoken_for: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for (i, first) in events.iter().enumerate() {
        let first_key = key_of(first);
        if spoken_for.contains(&first_key)
            || grouped.contains(&(first.calendar_id.as_str(), first.series_id.as_str()))
        {
            continue;
        }
        if !offerable(first) {
            continue;
        }
        let title = normalized_title(&first.title);
        if title.is_empty() {
            continue;
        }
        let when = when_key(first);

        for (j, second) in events.iter().enumerate().skip(i + 1) {
            let second_key = key_of(second);
            if spoken_for.contains(&second_key)
                || grouped.contains(&(second.calendar_id.as_str(), second.series_id.as_str()))
            {
                continue;
            }
            if !offerable(second) {
                continue;
            }
            if second.calendar_id == first.calendar_id {
                continue;
            }
            if normalized_title(&second.title) != title {
                continue;
            }
            if !same_time(&when_key(second), &when) {
                continue;
            }
            let pair = pair_key(
                (first.calendar_id.as_str(), first.series_id.as_str()),
                (second.calendar_id.as_str(), second.series_id.as_str()),
            );
            if declined.contains(&pair) {
                continue;
            }
            out.push(GroupSuggestion {
                first: i,
                second: j,
            });
            spoken_for.insert(first_key);
            spoken_for.insert(second_key);
            break;
        }
    }
    out
}

/// Everything [`find_group_suggestions`] needs, in one JSON value.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionInput {
    pub events: Vec<SuggestibleEvent>,
    #[serde(default)]
    pub groups: Vec<EventGroup>,
    #[serde(default)]
    pub declines: Vec<SuggestionDecline>,
}

/// Everything [`suggest_group_mate`] needs, in one JSON value.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MateInput {
    pub anchor: SuggestibleEvent,
    pub candidates: Vec<SuggestibleEvent>,
}

/// Offer copies from JSON, and answer with JSON.
///
/// The door both frontends come through — the desktop compiled into its webview
/// as WebAssembly, mobile over the UniFFI bridge. It lives HERE rather than in
/// each binding because two copies of a marshalling step is how a rule comes to
/// exist twice, which is the thing this move exists to stop.
///
/// The answer is an array of index PAIRS into `events`, so the caller can hand
/// back the rows it already holds; see [`GroupSuggestion`].
///
/// A malformed input is an `Err`, not an empty answer: the caller built that
/// JSON, so it is a bug in the binding rather than a fact about the day.
pub fn find_group_suggestions_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: SuggestionInput = serde_json::from_str(input_json)?;
    let found = find_group_suggestions(&input.events, &input.groups, &input.declines);
    serde_json::to_string(&found)
}

/// Recognise a copy from JSON, and answer with JSON.
///
/// `"null"` when nothing there is a copy, which is the ordinary answer; a
/// number otherwise, being the position in `candidates`.
pub fn suggest_group_mate_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: MateInput = serde_json::from_str(input_json)?;
    let found = suggest_group_mate(&input.anchor, &input.candidates);
    serde_json::to_string(&found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventGroupMember;

    fn ev(calendar_id: &str, series_id: &str, title: &str, start: &str) -> SuggestibleEvent {
        SuggestibleEvent {
            calendar_id: calendar_id.into(),
            series_id: series_id.into(),
            title: title.into(),
            start: start.into(),
            all_day: false,
        }
    }

    fn all_day(calendar_id: &str, series_id: &str, title: &str, start: &str) -> SuggestibleEvent {
        SuggestibleEvent {
            all_day: true,
            ..ev(calendar_id, series_id, title, start)
        }
    }

    fn decline(a: (&str, &str), b: (&str, &str)) -> SuggestionDecline {
        SuggestionDecline::new(a, b, "2026-08-09T12:00:00Z")
    }

    fn day() -> Vec<SuggestibleEvent> {
        vec![
            ev("work", "ev-a", "Wochenplanung", "2026-08-10T08:00:00Z"),
            ev("private", "ev-b", "Wochenplanung", "2026-08-10T08:00:00Z"),
            ev("work", "ev-x", "Zahnarzt", "2026-08-10T11:00:00Z"),
        ]
    }

    fn anchor() -> SuggestibleEvent {
        ev("work", "ev-a", "Wochenplanung", "2026-08-10T08:00:00Z")
    }

    #[test]
    fn it_finds_the_same_appointment_in_another_calendar() {
        let candidates = [
            ev("private", "ev-x", "Standup", "2026-08-10T08:00:00Z"),
            ev(
                "private",
                "ev-b",
                "  wochenplanung ",
                "2026-08-10T08:00:00Z",
            ),
        ];
        assert_eq!(suggest_group_mate(&anchor(), &candidates), Some(1));
    }

    /// "Overlapping" would happily offer the meeting before this one, which is
    /// the wrong answer in the most ordinary calendar there is.
    #[test]
    fn a_meeting_that_merely_overlaps_is_refused() {
        let candidates = [ev(
            "private",
            "ev-b",
            "Wochenplanung",
            "2026-08-10T07:30:00Z",
        )];
        assert_eq!(suggest_group_mate(&anchor(), &candidates), None);
    }

    /// Two rows in one calendar are a duplicate to clean up, not an
    /// appointment that lives in several places.
    #[test]
    fn a_second_row_in_the_same_calendar_is_refused() {
        let candidates = [ev("work", "ev-b", "Wochenplanung", "2026-08-10T08:00:00Z")];
        assert_eq!(suggest_group_mate(&anchor(), &candidates), None);
    }

    #[test]
    fn a_different_name_at_the_same_time_is_refused() {
        let candidates = [ev("private", "ev-b", "Zahnarzt", "2026-08-10T08:00:00Z")];
        assert_eq!(suggest_group_mate(&anchor(), &candidates), None);
    }

    /// Every untitled row would otherwise look like a copy of every other one.
    #[test]
    fn a_nameless_event_is_never_a_copy() {
        let nameless = SuggestibleEvent {
            title: "   ".into(),
            ..anchor()
        };
        let candidates = [ev("private", "ev-b", "", "2026-08-10T08:00:00Z")];
        assert_eq!(suggest_group_mate(&nameless, &candidates), None);
    }

    #[test]
    fn all_day_rows_agree_on_the_day_not_the_instant() {
        let anchor = all_day("work", "ev-a", "Urlaub", "2026-08-10");
        let candidates = [all_day("private", "ev-b", "Urlaub", "2026-08-10T00:00:00Z")];
        assert_eq!(suggest_group_mate(&anchor, &candidates), Some(0));
    }

    /// The day rule belongs to all-day rows alone. A timed row that happens to
    /// fall on the same day is a different appointment, and reading the two as
    /// one is exactly the wrong group this module exists to avoid.
    #[test]
    fn a_timed_row_never_matches_an_all_day_one_on_its_day() {
        let anchor = all_day("work", "ev-a", "Urlaub", "2026-08-10");
        let candidates = [ev("private", "ev-b", "Urlaub", "2026-08-10T09:00:00Z")];
        assert_eq!(suggest_group_mate(&anchor, &candidates), None);
    }

    /// Ties go to the order the user sees.
    #[test]
    fn the_first_of_several_wins() {
        let candidates = [
            ev("private", "ev-b", "Wochenplanung", "2026-08-10T08:00:00Z"),
            ev("colleague", "ev-c", "Wochenplanung", "2026-08-10T08:00:00Z"),
        ];
        assert_eq!(suggest_group_mate(&anchor(), &candidates), Some(0));
    }

    /// A start nothing can be made of matches NOTHING — including another one
    /// just like it. Two rows Aperio cannot place are not evidence that they
    /// are the same appointment.
    ///
    /// The frontend rule this replaces had no answer here at all: its key was
    /// `new Date(value).toISOString()`, which THROWS on an unreadable start.
    #[test]
    fn an_unreadable_start_matches_nothing() {
        let broken = ev("work", "ev-a", "Wochenplanung", "irgendwann");
        let same_but_broken = [ev("private", "ev-b", "Wochenplanung", "irgendwann")];
        assert_eq!(suggest_group_mate(&broken, &same_but_broken), None);
        let sound = [ev(
            "private",
            "ev-b",
            "Wochenplanung",
            "2026-08-10T08:00:00Z",
        )];
        assert_eq!(suggest_group_mate(&broken, &sound), None);
    }

    #[test]
    fn it_spots_the_copy_in_another_calendar() {
        assert_eq!(
            find_group_suggestions(&day(), &[], &[]),
            vec![GroupSuggestion {
                first: 0,
                second: 1
            }],
        );
    }

    /// A meeting carries the join URL its provider issued — an identity — and
    /// the meeting-link grouping pairs it on that. Offering it here asks the
    /// user the wrong question, and their answer is kept forever.
    #[test]
    fn a_videoconference_meeting_is_never_offered_on_a_resemblance() {
        let with_meeting = [
            ev(
                "acc::meetings",
                "vc::m1",
                "Team meeting",
                "2026-08-10T10:00:00Z",
            ),
            ev(
                "shared-team",
                "ev-strange",
                "Team meeting",
                "2026-08-10T10:00:00Z",
            ),
        ];
        assert!(find_group_suggestions(&with_meeting, &[], &[]).is_empty());
    }

    #[test]
    fn events_that_are_already_grouped_are_not_offered() {
        let member = |calendar_id: &str, event_id: &str| EventGroupMember {
            calendar_id: calendar_id.into(),
            event_id: event_id.into(),
            title: "Wochenplanung".into(),
            starts_at: "2026-08-10T08:00:00Z".into(),
            added_at: "2026-08-09T12:00:00Z".into(),
        };
        let group = EventGroup {
            id: "g1".into(),
            created_at: "2026-08-09T12:00:00Z".into(),
            updated_at: "2026-08-09T12:00:00Z".into(),
            members: vec![member("work", "ev-a"), member("private", "ev-b")],
        };
        assert!(find_group_suggestions(&day(), &[group], &[]).is_empty());
    }

    /// The whole reason the decline is stored: told once, Aperio has to stop.
    #[test]
    fn a_declined_pair_is_never_asked_about_again() {
        let declines = [decline(("work", "ev-a"), ("private", "ev-b"))];
        assert!(find_group_suggestions(&day(), &[], &declines).is_empty());
    }

    /// Declined from B's side; offering it again from A's would be the same
    /// question asked twice.
    #[test]
    fn a_decline_counts_whichever_way_round_it_was_made() {
        let declines = [decline(("private", "ev-b"), ("work", "ev-a"))];
        assert!(find_group_suggestions(&day(), &[], &declines).is_empty());
    }

    /// A refusal the user has since taken back by grouping the pair BY HAND
    /// does not silence anything.
    #[test]
    fn a_cleared_decline_stops_silencing_the_offer() {
        let mut taken_back = decline(("work", "ev-a"), ("private", "ev-b"));
        taken_back.cleared_at = Some("2026-08-09T13:00:00Z".into());
        assert!(!taken_back.is_declined());
        assert_eq!(find_group_suggestions(&day(), &[], &[taken_back]).len(), 1);
    }

    /// One offer, not three pairings of the same appointment — answering it
    /// groups a+b, and the next round offers c against that group's member.
    #[test]
    fn each_event_is_offered_at_most_once() {
        let three = [
            ev("work", "ev-a", "Wochenplanung", "2026-08-10T08:00:00Z"),
            ev("private", "ev-b", "Wochenplanung", "2026-08-10T08:00:00Z"),
            ev("colleague", "ev-c", "Wochenplanung", "2026-08-10T08:00:00Z"),
        ];
        assert_eq!(find_group_suggestions(&three, &[], &[]).len(), 1);
    }

    #[test]
    fn the_strict_match_holds_here_too() {
        let rows = [
            ev("work", "ev-a", "Wochenplanung", "2026-08-10T08:00:00Z"),
            ev("work", "ev-dup", "Wochenplanung", "2026-08-10T08:00:00Z"),
            ev(
                "private",
                "ev-late",
                "Wochenplanung",
                "2026-08-10T09:00:00Z",
            ),
        ];
        assert!(find_group_suggestions(&rows, &[], &[]).is_empty());
    }

    #[test]
    fn the_meetings_suffix_is_the_one_the_host_mints() {
        assert!(is_meeting_calendar(&format!(
            "acc-1{MEETINGS_CALENDAR_SUFFIX}"
        )));
        assert!(!is_meeting_calendar("acc-1"));
        assert!(!is_meeting_calendar("meetings::acc-1"));
    }
}
