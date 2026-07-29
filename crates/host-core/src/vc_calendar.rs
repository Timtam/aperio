//! A read-only calendar made of a videoconference account's own meetings.
//!
//! A meeting created in the provider's web UI exists only there. It has no
//! calendar entry anywhere, so no calendar app — this one included — has ever
//! shown it, and the first reminder its owner gets is the meeting starting.
//!
//! Any videoconference adapter that can enumerate its meetings
//! ([`VcAdapter::list_meetings`]) gets one of these: a calendar whose events
//! *are* the meetings. Nothing here knows a provider; an adapter added later
//! gets the same treatment by filling the same slot, and one that cannot
//! enumerate simply has no such calendar.
//!
//! ## Read-only, and why that is not a limitation
//!
//! Every write refuses. The point of this calendar is to make visible what
//! exists elsewhere; letting someone type a new event into it would ask the
//! provider to invent a meeting from a calendar entry, which is the opposite of
//! the direction that works. Creating a meeting is what the event editor's own
//! control does, against a real calendar.
//!
//! ## The duplicate problem
//!
//! Most meetings DO have a calendar entry — Aperio's own, or one Outlook wrote.
//! Listing those here as well would show every meeting twice.
//!
//! This calendar lists everything; the suppression happens where all of a
//! window's events are actually in hand, which is the view layer
//! (`shared/meetingEvents.ts`, one copy for both platforms). It cannot happen
//! here: an adapter is asked for ONE calendar's events and has no idea what the
//! other calendars hold.
//!
//! The filter there is the join URL, which is exact — it is what the provider
//! issued, what the event carries, and what identifies the meeting to everybody
//! involved. Not the title and not the time window: those look like evidence
//! and are not, especially since Aperio writes the event's own title into the
//! meeting it creates.

use async_trait::async_trait;
use cal_core::{
    Calendar, CalendarFeature, ContainerColor, DateRange, Error, Event, FreeBusy, NewEvent, Result,
};
use std::sync::Arc;
use vc_core::{Meeting, VcAdapter};

/// Suffix that turns an account id into its meetings-calendar id.
///
/// A calendar id has to be stable and has to be recognisable as belonging to
/// this account, because that is how the registry routes a read back.
const CALENDAR_SUFFIX: &str = "::meetings";

/// The calendar id for `account_id`'s meetings.
pub fn calendar_id_for(account_id: &str) -> String {
    format!("{account_id}{CALENDAR_SUFFIX}")
}

/// The account a meetings-calendar id belongs to, or `None` when the id is not
/// one of ours.
pub fn account_for_calendar(calendar_id: &str) -> Option<&str> {
    calendar_id.strip_suffix(CALENDAR_SUFFIX)
}

/// Turns a videoconference account into a read-only calendar of its meetings.
pub struct VcCalendar {
    account_id: String,
    /// What the calendar is called in the sidebar. The account's own display
    /// name, so two Webex accounts stay tellable apart.
    display_name: String,
    vc: Arc<dyn VcAdapter>,
}

impl VcCalendar {
    pub fn new(
        account_id: impl Into<String>,
        display_name: impl Into<String>,
        vc: Arc<dyn VcAdapter>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            display_name: display_name.into(),
            vc,
        }
    }

    /// A meeting as an event.
    ///
    /// Read-only by construction: the id is the provider's meeting id, prefixed
    /// so it can never be mistaken for a calendar event's id, and the join URL
    /// goes in `location` so the ordinary conference detection finds it and the
    /// Join affordance works exactly as it does everywhere else.
    fn to_event(&self, meeting: Meeting) -> Option<Event> {
        // A meeting with no time cannot be placed in a calendar. The personal
        // room is the case — it is always on, which is precisely not an
        // appointment.
        let (start, end) = (meeting.start_time?, meeting.end_time?);
        Some(Event {
            id: format!("vc::{}", meeting.id),
            calendar_id: calendar_id_for(&self.account_id),
            title: meeting.title,
            description: meeting
                .password
                .filter(|p| !p.trim().is_empty())
                .map(|password| format!("Meeting password: {password}")),
            // The join URL goes here so the ordinary conference detection finds
            // it and Join works exactly as it does on any other event.
            location: Some(meeting.join_url),
            start,
            end,
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: start,
            updated_at: start,
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
        })
    }
}

#[async_trait]
impl cal_core::Adapter for VcCalendar {
    async fn authenticate(
        &self,
        _credentials: cal_core::Credentials,
    ) -> Result<cal_core::AuthToken> {
        // The videoconference account behind this calendar is already
        // authenticated; there is no second credential to present.
        Err(read_only())
    }

    fn capabilities(&self) -> &[cal_core::Capability] {
        &[cal_core::Capability::Calendar]
    }
}

#[async_trait]
impl CalendarFeature for VcCalendar {
    async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        Ok(vec![Calendar {
            id: calendar_id_for(&self.account_id),
            name: self.display_name.clone(),
            color: None,
            color_label: None,
            // Every write refuses, so say so before anyone tries.
            read_only: true,
            default_sound: None,
            supports_scheduling: false,
            supports_event_color: false,
        }])
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> Result<Vec<Event>> {
        if account_for_calendar(calendar_id) != Some(self.account_id.as_str()) {
            return Ok(Vec::new());
        }
        let meetings = self
            .vc
            .list_meetings(range.start, range.end)
            .await
            .map_err(|err| Error::Internal(err.to_string()))?;
        Ok(meetings
            .into_iter()
            .filter_map(|m| self.to_event(m))
            .collect())
    }

    async fn create_event(&self, _calendar_id: &str, _event: NewEvent) -> Result<Event> {
        Err(read_only())
    }

    async fn update_event(&self, _event: Event) -> Result<Event> {
        Err(read_only())
    }

    async fn delete_event(&self, _event_id: &str, _send_cancellations: bool) -> Result<()> {
        Err(read_only())
    }

    async fn get_free_busy(&self, _emails: &[&str], _range: DateRange) -> Result<Vec<FreeBusy>> {
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        None
    }
}

/// The refusal every write gets.
///
/// Says what to do instead, because "read-only" on its own leaves someone
/// wondering how a meeting is supposed to be made at all.
fn read_only() -> Error {
    Error::Unsupported(
        "This calendar shows the meetings that exist at the provider; it cannot be edited. \
         To create a meeting, add it to one of your own calendars and use \"Create a meeting\" \
         on the event."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::sync::Mutex;

    struct FakeVc {
        meetings: Vec<Meeting>,
        listed: Mutex<Vec<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)>>,
    }

    #[async_trait]
    impl VcAdapter for FakeVc {
        async fn test_connection(&self) -> vc_core::VcResult<()> {
            Ok(())
        }
        async fn create_meeting(&self, _spec: vc_core::NewMeeting) -> vc_core::VcResult<Meeting> {
            unreachable!("read path only")
        }
        async fn get_meeting(
            &self,
            _id: &vc_core::MeetingId,
        ) -> vc_core::VcResult<Option<Meeting>> {
            Ok(None)
        }
        async fn delete_meeting(&self, _removal: vc_core::MeetingRemoval) -> vc_core::VcResult<()> {
            unreachable!("read path only")
        }
        async fn list_meetings(
            &self,
            start: chrono::DateTime<Utc>,
            end: chrono::DateTime<Utc>,
        ) -> vc_core::VcResult<Vec<Meeting>> {
            self.listed.lock().unwrap().push((start, end));
            Ok(self.meetings.clone())
        }
    }

    fn meeting(id: &str, title: &str, link: &str, timed: bool) -> Meeting {
        Meeting {
            id: id.into(),
            join_url: link.into(),
            title: title.into(),
            start_time: timed.then(|| Utc.with_ymd_and_hms(2026, 7, 29, 9, 0, 0).unwrap()),
            end_time: timed.then(|| Utc.with_ymd_and_hms(2026, 7, 29, 9, 30, 0).unwrap()),
            password: Some("s3cr3t".into()),
            invitees: Vec::new(),
        }
    }

    fn calendar_for(meetings: Vec<Meeting>) -> VcCalendar {
        VcCalendar::new(
            "acc-1",
            "Webex (work)",
            Arc::new(FakeVc {
                meetings,
                listed: Mutex::new(Vec::new()),
            }),
        )
    }

    /// An event shaped like one this calendar would emit.
    fn sample_event() -> Event {
        let start = Utc.with_ymd_and_hms(2026, 7, 29, 9, 0, 0).unwrap();
        Event {
            id: "vc::m1".into(),
            calendar_id: calendar_id_for("acc-1"),
            title: "Standup".into(),
            description: None,
            location: None,
            start,
            end: start,
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: start,
            updated_at: start,
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
        }
    }

    fn window() -> DateRange {
        DateRange::new(
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(),
        )
    }

    #[tokio::test]
    async fn a_meeting_with_no_calendar_entry_becomes_an_event() {
        let cal = calendar_for(vec![meeting(
            "m1",
            "Standup",
            "https://x.webex.com/j/1",
            true,
        )]);
        let events = cal
            .get_events(&calendar_id_for("acc-1"), window())
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Standup");
        // The link goes in `location`, which is what the ordinary conference
        // detection reads — so Join works here exactly as it does elsewhere.
        assert_eq!(
            events[0].location.as_deref(),
            Some("https://x.webex.com/j/1")
        );
    }

    #[tokio::test]
    async fn an_always_on_room_is_not_an_appointment() {
        // The personal room has no start or end. Placing it in a calendar would
        // mean inventing a time it does not have.
        let cal = calendar_for(vec![meeting(
            "room",
            "Personal Room",
            "https://x.webex.com/meet/t",
            false,
        )]);
        let events = cal
            .get_events(&calendar_id_for("acc-1"), window())
            .await
            .unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn the_calendar_is_read_only_and_says_what_to_do_instead() {
        let cal = calendar_for(vec![]);
        let err = cal
            .delete_event("vc::m1", false)
            .await
            .expect_err("must refuse");
        let message = err.to_string();
        assert!(
            message.contains("Create a meeting"),
            "the refusal has to point somewhere: {message}"
        );
        let events = cal
            .get_events(&calendar_id_for("acc-1"), window())
            .await
            .unwrap();
        assert!(events.is_empty(), "the fake has no meetings");
        // update_event refuses too, and for the same reason.
        assert!(cal.update_event(sample_event()).await.is_err());
    }

    #[tokio::test]
    async fn another_accounts_calendar_id_reads_empty_rather_than_wrong() {
        let cal = calendar_for(vec![meeting(
            "m1",
            "Standup",
            "https://x.webex.com/j/1",
            true,
        )]);
        let events = cal
            .get_events(&calendar_id_for("some-other-account"), window())
            .await
            .unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn the_calendar_reports_itself_as_read_only() {
        let cal = calendar_for(vec![]);
        let listed = cal.list_calendars().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].read_only,
            "writes are refused, so say so up front"
        );
        assert_eq!(listed[0].name, "Webex (work)");
    }
}
