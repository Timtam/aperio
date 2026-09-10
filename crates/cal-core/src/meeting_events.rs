//! Hiding a provider-side meeting that already has a calendar entry.
//!
//! A videoconference account contributes a read-only calendar of its own
//! meetings, so a meeting created in the provider's web UI — which has no
//! calendar entry anywhere — still shows up. But most meetings DO have one:
//! Aperio's own event, or the invitation Outlook wrote. Left alone, those
//! appear twice.
//!
//! The filter is the **join URL**, and that choice is the whole point. It is
//! what the provider issued, what the event carries, and what identifies the
//! meeting to everyone involved — an exact key, not a resemblance. Matching on
//! title and time instead would be worse than it looks: Aperio writes the
//! event's own title into the meeting it creates, so title equality carries
//! almost no evidence, and the times drift apart precisely when an event is
//! moved, which is when a user most wants the two seen as one.
//!
//! # Why the whole day crosses at once
//!
//! This decides about a SET: which rows a window holds is the question, and an
//! adapter asked for one calendar cannot know what the others hold. The rule
//! therefore takes the window.
//!
//! It used to sit in `shared/meetingEvents.ts` and reach the detection through
//! the door once PER ROW — twice, in fact, since the filter reads each row's
//! link on the way in and again on the way out. A day view crossed the
//! boundary forty times to answer one question about forty rows. Now it
//! crosses once, with the same bytes.

use serde::{Deserialize, Serialize};

use crate::conferencing::{detect_conference, ConferenceSources};
use crate::group_suggestion::is_meeting_calendar;

/// The least a row needs for the duplicate filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct MeetingFilterEvent {
    pub calendar_id: String,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Whether this row is already a member of a group.
    ///
    /// A grouped meeting row must NOT be dropped: the folding is what hides it
    /// then, and it hides it while COUNTING it — the row says "2×" and the
    /// group can be opened. Dropped first, the count would be a lie about a
    /// row that is not there.
    ///
    /// This is also the transition. As automatic grouping spreads, the filter
    /// quietly stops applying to the pairs it covers, and what is left is the
    /// case it was written for: a meeting whose partner is not in view.
    #[serde(default)]
    pub grouped: bool,
}

/// The join URL a row carries, or `None`.
///
/// The same reading the detection gives, because it IS the detection: two
/// readers that disagreed would mean a row this filter drops and the
/// meeting-link grouping never pairs — the duplicate gone with nothing to say
/// where.
pub fn meeting_join_url(event: &MeetingFilterEvent) -> Option<String> {
    detect_conference(&ConferenceSources {
        provider_field: None,
        icalendar_conference: &[],
        vendor_properties: &[],
        location: event.location.as_deref(),
        description: event.description.as_deref(),
    })
    .map(|link| link.join_url)
}

/// Which rows survive the duplicate filter, as positions in the input.
///
/// Order-independent and stable: real events are never dropped, only the
/// synthesized ones, and only when something else in view already shows that
/// exact meeting.
pub fn without_duplicate_meetings(events: &[MeetingFilterEvent]) -> Vec<usize> {
    // The links carried by REAL events first — a second synthesized event for
    // the same meeting (two accounts on one site, say) must not suppress the
    // first.
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut links: Vec<Option<String>> = Vec::with_capacity(events.len());
    for event in events {
        let url = meeting_join_url(event);
        if !is_meeting_calendar(&event.calendar_id) {
            if let Some(url) = &url {
                claimed.insert(url.clone());
            }
        }
        links.push(url);
    }
    if claimed.is_empty() {
        return (0..events.len()).collect();
    }
    events
        .iter()
        .enumerate()
        .filter(|(i, event)| {
            if !is_meeting_calendar(&event.calendar_id) {
                return true;
            }
            if event.grouped {
                return true;
            }
            match &links[*i] {
                None => true,
                Some(url) => !claimed.contains(url),
            }
        })
        .map(|(i, _)| i)
        .collect()
}

/// Filter a window from JSON, and answer with JSON.
///
/// The door both frontends come through; the marshalling lives here so the two
/// cannot drift in what they accept. The answer is the POSITIONS that survive,
/// because the caller is holding the rows already.
pub fn without_duplicate_meetings_json(events_json: &str) -> Result<String, serde_json::Error> {
    let events: Vec<MeetingFilterEvent> = serde_json::from_str(events_json)?;
    serde_json::to_string(&without_duplicate_meetings(&events))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOIN: &str = "https://example.webex.com/meet/j.doe";
    const OTHER: &str = "https://example.webex.com/meet/a.other";

    fn row(calendar_id: &str, location: Option<&str>) -> MeetingFilterEvent {
        MeetingFilterEvent {
            calendar_id: calendar_id.into(),
            location: location.map(str::to_string),
            description: None,
            grouped: false,
        }
    }

    #[test]
    fn a_meeting_whose_entry_is_in_view_is_dropped() {
        let rows = [
            row("work", Some(JOIN)),
            row("acc::meetings", Some(JOIN)),
            row("work", None),
        ];
        assert_eq!(without_duplicate_meetings(&rows), vec![0, 2]);
    }

    /// A meeting created in the provider's web UI has no calendar entry
    /// anywhere. Dropping it would lose an appointment the user really has.
    #[test]
    fn a_meeting_with_no_entry_in_view_stays() {
        let rows = [row("work", Some(OTHER)), row("acc::meetings", Some(JOIN))];
        assert_eq!(without_duplicate_meetings(&rows), vec![0, 1]);
    }

    /// The folding is what hides a GROUPED meeting, and it hides it while
    /// counting it. Dropped first, the "2×" mark would describe a row that is
    /// not there.
    #[test]
    fn a_grouped_meeting_is_never_dropped() {
        let mut meeting = row("acc::meetings", Some(JOIN));
        meeting.grouped = true;
        let rows = [row("work", Some(JOIN)), meeting];
        assert_eq!(without_duplicate_meetings(&rows), vec![0, 1]);
    }

    /// Real events are never dropped, whatever else is in view.
    #[test]
    fn two_real_events_sharing_a_link_both_stay() {
        let rows = [row("work", Some(JOIN)), row("private", Some(JOIN))];
        assert_eq!(without_duplicate_meetings(&rows), vec![0, 1]);
    }

    /// A second synthesized row for the same meeting — two accounts on one
    /// site — must not suppress the first. Only a REAL event claims a link.
    #[test]
    fn one_meeting_row_never_suppresses_another() {
        let rows = [
            row("acc-a::meetings", Some(JOIN)),
            row("acc-b::meetings", Some(JOIN)),
        ];
        assert_eq!(without_duplicate_meetings(&rows), vec![0, 1]);
    }

    #[test]
    fn a_window_with_no_links_at_all_is_untouched() {
        let rows = [row("work", None), row("acc::meetings", None)];
        assert_eq!(without_duplicate_meetings(&rows), vec![0, 1]);
    }
}
