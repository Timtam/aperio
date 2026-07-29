//! The Webex Meetings endpoints, and the two ways Aperio can give an event a
//! join link.
//!
//! **A meeting per event** is the default: `POST /meetings` mints its own link,
//! password and dial-in details for that one event, and deleting the event can
//! delete the meeting. It needs `meeting:schedules_write`, and Webex caps
//! regular users at 100 creations per 24 hours.
//!
//! **The Personal Meeting Room** is the alternative: one permanent link the
//! account already owns, read with `meeting:preferences_read`. No creation, no
//! cap, no lifecycle, and no write scope — which also makes it the mode most
//! likely to work on an account whose licence cannot schedule at all. Every
//! event then shares the same room, which is the trade.
//!
//! Both produce a [`Meeting`]; the personal room simply has no start or end,
//! the case `vc_core` already describes as "instant / always-on".

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use vc_core::{Meeting, MeetingId, MeetingInvitee, NewMeeting, VcError, VcResult};

use crate::api::{wire_time, ApiState};

/// Webex refuses a meeting shorter than this.
const MIN_DURATION: Duration = Duration::minutes(10);
/// …or longer than this.
const MAX_DURATION: Duration = Duration::seconds(23 * 3600 + 59 * 60);
/// What a meeting gets when the caller names no window at all.
const DEFAULT_DURATION: Duration = Duration::hours(1);
/// Head room between "now" and the earliest start Webex will accept.
///
/// Webex refuses a meeting that starts in the past, and by the time it reads
/// the clock the request has already spent a token refresh and a round trip —
/// so "now" is not far enough, and an ordinarily-skewed client clock makes it
/// worse.
const START_FLOOR: Duration = Duration::minutes(1);
/// Webex caps a meeting title at 128 characters and answers 400 above it.
const MAX_TITLE: usize = 128;
/// …and an agenda at 1300.
const MAX_AGENDA: usize = 1300;
/// …and one integration tag at 64.
const MAX_TAG: usize = 64;

// ── wire types ───────────────────────────────────────────────────────────

/// One address in `CreateMeetingBody::invitees`.
#[derive(Debug, Serialize)]
struct InviteeBody<'a> {
    email: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateMeetingBody<'a> {
    title: &'a str,
    start: String,
    end: String,
    /// Webex wants a zone name alongside the instants. Everything Aperio holds
    /// is UTC, so it says so rather than guessing the user's.
    timezone: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    agenda: Option<&'a str>,
    #[serde(rename = "siteUrl", skip_serializing_if = "Option::is_none")]
    site_url: Option<&'a str>,
    /// Aperio sends its own invitations through the calendar event. Letting
    /// Webex also email everyone would put TWO invitations in each attendee's
    /// mailbox, each with its own iCalendar attachment, and a duplicate entry
    /// in their calendar. Default is `true` on Webex's side, so this is not
    /// optional politeness — it is the difference between one invite and two.
    #[serde(rename = "sendEmail")]
    send_email: bool,
    /// Aperio's own id for the event this meeting belongs to. Webex hands it
    /// back on read and can filter by it, which makes it a durable back
    /// reference: a meeting whose stored id was lost can still be found.
    #[serde(rename = "integrationTags", skip_serializing_if = "Vec::is_empty")]
    integration_tags: Vec<String>,
    /// Who Webex should consider invited. Omitted entirely when the event has
    /// no attendees, so a solo meeting sends no address anywhere.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    invitees: Vec<InviteeBody<'a>>,
}

#[derive(Debug, Deserialize)]
pub struct MeetingResponse {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(rename = "webLink", default)]
    pub web_link: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(rename = "meetingNumber", default)]
    pub meeting_number: Option<String>,
    #[serde(rename = "sipAddress", default)]
    pub sip_address: Option<String>,
    #[serde(default)]
    pub start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub end: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct SitesResponse {
    #[serde(default)]
    sites: Vec<Site>,
}

#[derive(Debug, Deserialize)]
struct Site {
    #[serde(rename = "siteUrl")]
    site_url: String,
    #[serde(default)]
    default: bool,
}

/// One page of `GET /meetings`.
#[derive(Debug, Deserialize)]
struct MeetingListResponse {
    #[serde(default)]
    items: Vec<MeetingResponse>,
}

#[derive(Debug, Deserialize)]
struct PersonalRoomResponse {
    #[serde(rename = "personalMeetingRoomLink", default)]
    link: Option<String>,
    /// Webex calls this `topic` on the preferences object even though the
    /// Meetings object calls the same thing `title`. Reading it as `title`
    /// deserialises silently to `None` on every call, so the room's real name —
    /// usually the host's, which is what an attendee recognises — never
    /// arrives.
    #[serde(default)]
    topic: Option<String>,
    /// An attendee join code, if a site variant ever returns one.
    ///
    /// Deliberately NOT falling back to `hostPin`, which this endpoint does
    /// return: the host PIN claims HOST of the room and starts it from a phone.
    /// `Meeting.password` is attendee-facing — the host renders it beside the
    /// join link in an event that syncs to every attendee — so putting the PIN
    /// there would hand every invitee the ability to take over the room.
    #[serde(default)]
    password: Option<String>,
    #[serde(rename = "sipAddress", default)]
    sip_address: Option<String>,
}

// ── operations ───────────────────────────────────────────────────────────

/// Confirm the credentials AND that this account can host meetings at all.
///
/// `GET /meetingPreferences/sites` rather than `/people/me`: both are cheap,
/// but only this one answers the question that is actually likely to be wrong.
/// A Webex account without a Meetings site is a perfectly valid Webex account
/// that cannot schedule anything, and finding that out at "test connection"
/// time is far kinder than finding it out when a meeting fails to appear.
/// It needs only the read scope, so it also works on a personal-room-only
/// account. Returns the default site, which `create_meeting` then targets.
pub async fn test_connection(state: &ApiState) -> VcResult<Option<String>> {
    let sites: SitesResponse = state.get_json("/meetingPreferences/sites").await?;
    if sites.sites.is_empty() {
        return Err(VcError::Unsupported(
            "This Webex account has no meeting site, so it cannot host meetings. That usually \
             means the account has no Webex Meetings subscription — ask whoever administers \
             your Webex organisation."
                .into(),
        ));
    }
    let default = sites
        .sites
        .iter()
        .find(|s| s.default)
        .or_else(|| sites.sites.first())
        .map(|s| s.site_url.clone());
    info!(
        sites = sites.sites.len(),
        default = default.as_deref().unwrap_or("(none)"),
        "Webex credentials are good"
    );
    Ok(default)
}

/// Create a meeting for `spec`, or hand back the personal room when that mode
/// is on.
pub async fn create_meeting(
    state: &ApiState,
    spec: &NewMeeting,
    site_url: Option<&str>,
    use_personal_room: bool,
    send_webex_emails: bool,
    event_tag: Option<&str>,
) -> VcResult<Meeting> {
    // The request decides; the account setting is only the fallback for a
    // caller that expresses no preference. A meeting is one or the other, and
    // that is not something a setup checkbox can know in advance.
    if use_personal_room || spec.use_personal_room {
        return personal_room(state).await;
    }
    if spec.title.trim().is_empty() {
        return Err(VcError::InvalidInput(
            "a Webex meeting needs a title".into(),
        ));
    }
    let title = clamp(spec.title.trim(), MAX_TITLE, "title");
    let agenda = spec
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| clamp(d, MAX_AGENDA, "agenda"));
    let (start, end) = window_for(spec, Utc::now());

    let body = CreateMeetingBody {
        title: &title,
        start: wire_time(start),
        end: wire_time(end),
        timezone: "UTC",
        agenda: agenda.as_deref(),
        site_url,
        // The account setting is a ceiling, not a command: it can switch the
        // provider's mail off entirely, but switching it ON per meeting is the
        // host's call, made from whether the calendar can invite by itself.
        send_email: send_webex_emails || spec.notify_attendees,
        integration_tags: event_tag
            .map(|t| vec![clamp(t, MAX_TAG, "integrationTag")])
            .unwrap_or_default(),
        invitees: spec
            .attendees
            .iter()
            .map(|email| email.trim())
            .filter(|email| !email.is_empty())
            .map(|email| InviteeBody { email })
            .collect(),
    };
    let created: MeetingResponse = state.post_json("/meetings", &body).await?;
    to_meeting(created)
}

/// Re-fetch a meeting. `None` means Webex no longer knows it — deleted on their
/// side, or never ours — which the trait defines as a soft delete.
pub async fn get_meeting(state: &ApiState, id: &MeetingId) -> VcResult<Option<Meeting>> {
    let path = format!("/meetings/{}", urlencode(id));
    match state.get_json_opt::<MeetingResponse>(&path).await? {
        Some(found) => {
            let mut meeting = to_meeting(found)?;
            meeting.invitees = invitees_for(state, &meeting.id).await;
            Ok(Some(meeting))
        }
        None => Ok(None),
    }
}

/// The meeting a join link belongs to.
///
/// `GET /meetings?webLink=…` is Webex's own reverse lookup, and it is the
/// reason the host can manage a meeting it did not create: the link is what
/// travels in a calendar event, the meeting id is not. Webex documents that
/// `webLink` makes `from`, `to`, `meetingType`, `state` and `siteUrl`
/// irrelevant, and that it cannot be combined with `meetingNumber` or `roomId`
/// — so this sends the link and nothing else.
///
/// `None` when Webex knows no meeting for the link, which is the normal answer
/// for a colleague's meeting on a site this account cannot see.
pub async fn resolve_meeting(state: &ApiState, join_url: &str) -> VcResult<Option<Meeting>> {
    let join_url = join_url.trim();
    if join_url.is_empty() {
        return Ok(None);
    }
    let path = format!("/meetings?webLink={}", urlencode(join_url));
    let page: MeetingListResponse = match state.get_json_opt(&path).await? {
        Some(page) => page,
        None => return Ok(None),
    };
    // An array is documented even for a single link. Take the first that
    // converts — a row without a web link is not a joinable meeting and
    // `to_meeting` rejects it.
    for raw in page.items {
        if let Ok(mut meeting) = to_meeting(raw) {
            meeting.invitees = invitees_for(state, &meeting.id).await;
            return Ok(Some(meeting));
        }
    }
    Ok(None)
}

/// The account's scheduled meetings between `start` and `end`.
///
/// What makes a meeting created in Webex's own web UI visible in a calendar at
/// all: it has no calendar entry anywhere, so nothing else would ever surface
/// it.
///
/// Both bounds are always sent. Webex's default when `from` is omitted is
/// "`to` minus seven days", which is a different window than the caller asked
/// for and would silently under-report.
///
/// Paging follows the `Link` header (RFC 5988), which Webex uses across its
/// API. The page count is bounded — a runaway `next` chain would otherwise turn
/// one calendar scroll into an unbounded walk, and a plugin call has no
/// cancellation.
pub async fn list_meetings(
    state: &ApiState,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> VcResult<Vec<Meeting>> {
    if end < start {
        return Err(VcError::InvalidInput(
            "the end of the window is before its start".into(),
        ));
    }
    let mut path = format!(
        "/meetings?from={}&to={}&max={MAX_PER_PAGE}",
        urlencode(&wire_time(start)),
        urlencode(&wire_time(end)),
    );
    let mut out = Vec::new();
    for _ in 0..MAX_PAGES {
        let (page, next): (MeetingListResponse, Option<String>) =
            state.get_json_paged(&path).await?;
        // A row Webex returns that carries no join link is not something a user
        // can be sent to, so it is dropped rather than surfaced as an event
        // that does nothing when activated.
        out.extend(
            page.items
                .into_iter()
                .filter_map(|raw| to_meeting(raw).ok()),
        );
        match next {
            Some(next) => path = next,
            None => return Ok(out),
        }
    }
    // Stopping is better than walking forever, but saying nothing about it
    // would make a truncated calendar look like an empty one.
    tracing::warn!(
        pages = MAX_PAGES,
        "stopped paging Webex meetings at the page cap; the window may be incomplete"
    );
    Ok(out)
}

/// Webex's per-page ceiling for meeting listings.
const MAX_PER_PAGE: u32 = 100;
/// How many pages one listing may walk. 100 × 100 meetings is far past any
/// real calendar window; beyond it something is wrong with the cursor.
const MAX_PAGES: usize = 100;

/// Drop a meeting on Webex's side.
///
/// `sendEmail` is threaded through for the same reason as on create: Webex's
/// default is to email everyone, and a cancellation notice on top of the one
/// Aperio's own event already sent is a second, contradictory message.
pub async fn delete_meeting(
    state: &ApiState,
    id: &MeetingId,
    send_webex_emails: bool,
) -> VcResult<()> {
    let path = format!(
        "/meetings/{}?sendEmail={}",
        urlencode(id),
        if send_webex_emails { "true" } else { "false" }
    );
    state.delete(&path).await
}

/// The account's permanent Personal Meeting Room.
async fn personal_room(state: &ApiState) -> VcResult<Meeting> {
    let room: PersonalRoomResponse = state
        .get_json("/meetingPreferences/personalMeetingRoom")
        .await?;
    let Some(link) = room.link.filter(|l| !l.trim().is_empty()) else {
        return Err(VcError::Unsupported(
            "This Webex account has no Personal Meeting Room link. Switch the account to \
             creating a meeting per event, or ask your Webex administrator to enable the \
             personal room."
                .into(),
        ));
    };
    Ok(Meeting {
        // The personal room has no per-meeting id. Its link IS its identity,
        // and it must never be handed to `delete_meeting` — the id is prefixed
        // so that a stray delete is refused loudly instead of trying to DELETE
        // a path that is not a meeting.
        id: format!("{PERSONAL_ROOM_ID_PREFIX}{link}"),
        join_url: link,
        title: room
            .topic
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Personal Room".to_string()),
        // No window: the room is always on. `vc_core` documents exactly this
        // case for `start_time` / `end_time` being None.
        start_time: None,
        end_time: None,
        // Only a genuine attendee join code — never the host PIN. See the
        // struct field above for why that distinction is not cosmetic.
        password: room.password.filter(|p| !p.trim().is_empty()),
        // A room has no invitee list — it is a door, not an appointment.
        invitees: Vec::new(),
    })
    .inspect(|_| {
        if let Some(sip) = room.sip_address.as_deref() {
            info!(sip, "personal room resolved");
        }
    })
}

/// Marks an id that is a personal-room link rather than a real meeting id.
pub const PERSONAL_ROOM_ID_PREFIX: &str = "webex-personal-room:";

/// True when this id names the permanent personal room, which has no lifecycle
/// on Webex's side and must not be deleted.
pub fn is_personal_room(id: &str) -> bool {
    id.starts_with(PERSONAL_ROOM_ID_PREFIX)
}

// ── helpers ──────────────────────────────────────────────────────────────

/// Decide the meeting's window from what the caller asked for.
///
/// Webex requires BOTH ends and refuses anything under 10 minutes or over
/// 23 h 59 m, while `NewMeeting` allows either end to be absent and an event
/// can legitimately be five minutes long or all day. Rejecting those would mean
/// "this event cannot have a meeting", which is a worse answer than a window
/// that differs slightly from the event's: the join link is what the user
/// wanted, and the times on the Webex side are advisory — the room does not
/// lock. So the window is clamped, and any adjustment is logged.
fn window_for(spec: &NewMeeting, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let requested_start = spec.start_time.unwrap_or(now);
    let requested_end = spec.end_time.unwrap_or(requested_start + DEFAULT_DURATION);

    // Webex refuses a start that has already passed, which an event routinely
    // has by the time somebody asks it for a link. Move the WHOLE window
    // forward rather than only the start, so the duration the user asked for
    // survives — flooring the start alone would make the span negative and
    // collapse an hour-long event to the ten-minute minimum.
    let floor = now + START_FLOOR;
    let shift = if requested_start < floor {
        floor - requested_start
    } else {
        Duration::zero()
    };
    if shift > Duration::zero() {
        warn!(
            shift_seconds = shift.num_seconds(),
            "the event has already started and Webex refuses a meeting that starts in the              past; the meeting window is moved forward at both ends (the event itself is              unchanged)"
        );
    }
    let start = requested_start + shift;
    let end = requested_end + shift;
    let requested = end - start;

    if requested < MIN_DURATION {
        warn!(
            requested_seconds = requested.num_seconds(),
            "the event is shorter than Webex's 10-minute minimum; the meeting window is              stretched to fit (the event itself is unchanged)"
        );
        return (start, start + MIN_DURATION);
    }
    if requested > MAX_DURATION {
        warn!(
            requested_seconds = requested.num_seconds(),
            "the event is longer than Webex's 23h59m maximum; the meeting window is              shortened to fit (the event itself is unchanged)"
        );
        return (start, start + MAX_DURATION);
    }
    (start, end)
}

/// Clamp a string Webex caps, rather than letting it 400.
///
/// Same trade as the window: a meeting whose agenda is cut short is a far
/// better answer than no join link at all, and the event itself is untouched
/// either way. One helper for all three caps so they cannot drift apart.
fn clamp(value: &str, max: usize, field: &'static str) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    warn!(
        field,
        max,
        length = value.chars().count(),
        "Webex caps this field; the value sent is shortened (the event itself is unchanged)"
    );
    value.chars().take(max).collect()
}

/// Percent-encode a path segment. Meeting ids are opaque and long, and Webex
/// has been seen to use base64-ish forms, so `/` and `+` must not slip through
/// and reshape the path.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn to_meeting(raw: MeetingResponse) -> VcResult<Meeting> {
    let Some(join_url) = raw.web_link.filter(|l| !l.trim().is_empty()) else {
        return Err(VcError::Protocol(format!(
            "Webex returned meeting {} without a join link, so there is nothing to join",
            raw.id
        )));
    };
    Ok(Meeting {
        id: raw.id,
        join_url,
        title: raw.title.unwrap_or_default(),
        start_time: raw.start,
        end_time: raw.end,
        password: raw.password.filter(|p| !p.trim().is_empty()),
        // Filled by the callers that read ONE meeting; a listing does not, since
        // it would turn one calendar scroll into a request per meeting.
        invitees: Vec::new(),
    })
}

/// Who Webex has invited to `meeting_id`.
///
/// Best effort on purpose. `GET /meetingInvitees` needs a scope the integration
/// may not hold, and reading the invitee list of a meeting one is merely
/// invited to may be refused outright — neither is a reason to fail the meeting
/// read that asked for it. A refusal degrades to an empty list and one log
/// line, because "we could not ask" and "nobody is invited" look the same to a
/// user and only one of them is worth interrupting them about.
async fn invitees_for(state: &ApiState, meeting_id: &MeetingId) -> Vec<MeetingInvitee> {
    let path = format!("/meetingInvitees?meetingId={}", urlencode(meeting_id));
    match state.get_json_opt::<InviteeListResponse>(&path).await {
        Ok(Some(page)) => page
            .items
            .into_iter()
            .filter(|raw| !raw.email.trim().is_empty())
            .map(|raw| MeetingInvitee {
                email: raw.email,
                display_name: raw.display_name.filter(|n| !n.trim().is_empty()),
                co_host: raw.co_host,
            })
            .collect(),
        Ok(None) => Vec::new(),
        Err(err) => {
            tracing::debug!(
                ?err,
                "Webex did not return the invitee list; showing the event's own attendees only"
            );
            Vec::new()
        }
    }
}

/// One page of `GET /meetingInvitees`.
#[derive(Debug, Deserialize)]
struct InviteeListResponse {
    #[serde(default)]
    items: Vec<InviteeResponse>,
}

#[derive(Debug, Deserialize)]
struct InviteeResponse {
    #[serde(default)]
    email: String,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
    #[serde(rename = "coHost", default)]
    co_host: bool,
}

/// Everything Webex tells us about a meeting that does not fit [`Meeting`], in
/// the order an invitation presents it.
///
/// Kept separate because `vc_core::Meeting` is the shared shape across four
/// providers and must not grow Webex-specific fields. The host stores this
/// beside the event so a screen reader can read out "meeting number, password,
/// dial-in" as labelled items rather than as one long string.
pub fn join_details(raw: &MeetingResponse) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(n) = raw.meeting_number.as_deref().filter(|n| !n.is_empty()) {
        out.push(("meeting_number", n.to_string()));
    }
    if let Some(p) = raw.password.as_deref().filter(|p| !p.is_empty()) {
        out.push(("password", p.to_string()));
    }
    if let Some(s) = raw.sip_address.as_deref().filter(|s| !s.is_empty()) {
        out.push(("sip_address", s.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn spec(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> NewMeeting {
        NewMeeting {
            title: "Weekly".into(),
            start_time: start,
            end_time: end,
            description: None,
            use_personal_room: false,
            attendees: Vec::new(),
            notify_attendees: false,
        }
    }

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 28, h, m, 0).unwrap()
    }

    /// A fixed clock, so the window tests stay deterministic instead of
    /// depending on when they happen to run.
    fn early() -> DateTime<Utc> {
        at(8, 0)
    }

    #[test]
    fn a_normal_window_is_passed_through_untouched() {
        let (s, e) = window_for(&spec(Some(at(9, 0)), Some(at(10, 0))), early());
        assert_eq!((s, e), (at(9, 0), at(10, 0)));
    }

    #[test]
    fn a_five_minute_event_still_gets_a_meeting() {
        // Webex refuses under 10 minutes. Refusing to mint a link at all would
        // be a worse answer than a window that runs five minutes past the
        // event — the room does not lock, and the link is what was wanted.
        let (s, e) = window_for(&spec(Some(at(9, 0)), Some(at(9, 5))), early());
        assert_eq!(s, at(9, 0));
        assert_eq!(e - s, MIN_DURATION);
    }

    #[test]
    fn an_all_day_event_is_clamped_rather_than_rejected() {
        let (s, e) = window_for(
            &spec(Some(at(0, 0)), Some(at(0, 0) + Duration::days(3))),
            at(0, 0) - Duration::hours(1),
        );
        assert_eq!(s, at(0, 0));
        assert_eq!(e - s, MAX_DURATION);
    }

    #[test]
    fn a_missing_end_becomes_an_hour() {
        let (s, e) = window_for(&spec(Some(at(9, 0)), None), early());
        assert_eq!(e - s, DEFAULT_DURATION);
    }

    #[test]
    fn an_event_that_already_started_is_moved_forward_whole() {
        // Webex refuses a start in the past, which an event routinely has by
        // the time somebody asks it for a link. Moving only the start would
        // make the span negative and collapse the hour to the 10-minute floor,
        // so the whole window shifts and the duration survives.
        let now = at(9, 15);
        let (s, e) = window_for(&spec(Some(at(9, 0)), Some(at(10, 0))), now);
        assert!(s > now, "the start must be in the future, got {s}");
        assert_eq!(
            e - s,
            Duration::hours(1),
            "the duration must survive the shift"
        );
    }

    #[test]
    fn a_meeting_with_no_start_still_lands_in_the_future() {
        // `now` is stamped before a token refresh and a round trip, so a start
        // of exactly now would already be past by the time Webex reads it.
        let now = at(9, 0);
        let (s, _e) = window_for(&spec(None, None), now);
        assert!(s >= now + START_FLOOR, "needs head room, got {s}");
    }

    #[test]
    fn a_future_event_is_not_moved_at_all() {
        let (s, e) = window_for(&spec(Some(at(15, 0)), Some(at(16, 0))), at(9, 0));
        assert_eq!((s, e), (at(15, 0), at(16, 0)));
    }

    #[test]
    fn the_fields_webex_caps_are_shortened_rather_than_rejected() {
        // Same trade as the window: a shortened agenda beats no join link.
        assert_eq!(
            clamp(&"x".repeat(200), MAX_TITLE, "title").chars().count(),
            128
        );
        assert_eq!(
            clamp(&"y".repeat(2000), MAX_AGENDA, "agenda")
                .chars()
                .count(),
            1300
        );
        assert_eq!(clamp(&"z".repeat(100), MAX_TAG, "tag").chars().count(), 64);
        assert_eq!(clamp("short", MAX_TITLE, "title"), "short");
        // Character-counted, not byte-counted: a description full of umlauts
        // must not be cut mid-character.
        assert_eq!(
            clamp(&"ü".repeat(200), MAX_TITLE, "title").chars().count(),
            128
        );
    }

    #[test]
    fn the_personal_room_never_publishes_the_host_pin() {
        // hostPin claims HOST of the room. Meeting.password is attendee-facing
        // and syncs to every invitee, so putting the PIN there would hand each
        // of them the ability to take the room over.
        let raw: PersonalRoomResponse = serde_json::from_value(serde_json::json!({
            "personalMeetingRoomLink": "https://x.webex.com/meet/toni",
            "topic": "Tonis Raum",
            "hostPin": "1234",
            "sipAddress": "toni@x.webex.com",
        }))
        .expect("personal room");
        assert!(raw.password.is_none(), "no attendee code was returned");
        assert_eq!(raw.topic.as_deref(), Some("Tonis Raum"));
    }

    #[test]
    fn the_personal_room_id_is_recognisable_and_never_a_meeting_id() {
        // Handing a personal-room id to delete_meeting would DELETE a path that
        // is not a meeting; the prefix is what lets the adapter refuse.
        assert!(is_personal_room(&format!(
            "{PERSONAL_ROOM_ID_PREFIX}https://x.webex.com/meet/toni"
        )));
        assert!(!is_personal_room("1234567890abcdef"));
    }

    #[test]
    fn meeting_ids_are_percent_encoded_into_the_path() {
        assert_eq!(urlencode("abc-123_x.y~z"), "abc-123_x.y~z");
        assert_eq!(urlencode("a/b+c"), "a%2Fb%2Bc");
        assert_eq!(urlencode("a b"), "a%20b");
    }

    #[test]
    fn a_meeting_without_a_join_link_is_an_error_not_an_empty_link() {
        // A Meeting whose join_url is "" would render a Join button that goes
        // nowhere, which is worse than saying the create did not work.
        let raw = MeetingResponse {
            id: "m1".into(),
            title: Some("t".into()),
            web_link: None,
            password: None,
            meeting_number: None,
            sip_address: None,
            start: None,
            end: None,
        };
        assert!(matches!(to_meeting(raw), Err(VcError::Protocol(_))));
    }

    #[test]
    fn join_details_are_labelled_items_not_one_blob() {
        // A screen reader reads these as separate facts; concatenating them
        // into one string is exactly what this avoids.
        let raw = MeetingResponse {
            id: "m1".into(),
            title: None,
            web_link: Some("https://x/j".into()),
            password: Some("pw".into()),
            meeting_number: Some("123 456".into()),
            sip_address: Some("123@x.webex.com".into()),
            start: None,
            end: None,
        };
        let details = join_details(&raw);
        assert_eq!(details.len(), 3);
        assert_eq!(details[0], ("meeting_number", "123 456".to_string()));
        assert!(details.iter().any(|(k, _)| *k == "sip_address"));
    }

    #[tokio::test]
    async fn creating_a_meeting_suppresses_webex_emails_and_tags_the_event() {
        // Webex's sendEmail defaults to TRUE and its mails carry an iCalendar
        // attachment, so leaving it alone puts a SECOND invitation and a
        // duplicate calendar entry in every attendee's mailbox.
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/meetings")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"sendEmail":false,"timezone":"UTC","integrationTags":["event-uid-1"],
                    "siteUrl":"site.webex.com"}"#
                    .to_string(),
            ))
            .with_status(200)
            .with_body(
                r#"{"id":"m1","title":"Weekly","webLink":"https://x.webex.com/j.php?MTID=a",
                    "password":"pw","meetingNumber":"123","start":"2026-07-28T09:00:00Z",
                    "end":"2026-07-28T10:00:00Z"}"#,
            )
            .create_async()
            .await;
        let state = crate::api::tests_support::state(&server.url());
        let meeting = create_meeting(
            &state,
            &spec(Some(at(9, 0)), Some(at(10, 0))),
            Some("site.webex.com"),
            false,
            false,
            Some("event-uid-1"),
        )
        .await
        .expect("create");
        assert_eq!(meeting.join_url, "https://x.webex.com/j.php?MTID=a");
        assert_eq!(meeting.password.as_deref(), Some("pw"));
        m.assert_async().await;
    }

    #[tokio::test]
    async fn the_event_s_attendees_become_webex_invitees_and_can_be_mailed() {
        // The other half of the mail question: before this, `sendEmail` had
        // barely anything to act on, because the meeting carried no invitees at
        // all. Whitespace is not an address and must not become one.
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/meetings")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"sendEmail":true,
                    "invitees":[{"email":"a@example.test"},{"email":"b@example.test"}]}"#
                    .to_string(),
            ))
            .with_status(200)
            .with_body(
                r#"{"id":"m1","title":"Weekly","webLink":"https://x.webex.com/j.php?MTID=a",
                    "start":"2026-07-28T09:00:00Z","end":"2026-07-28T10:00:00Z"}"#,
            )
            .create_async()
            .await;
        let state = crate::api::tests_support::state(&server.url());
        let mut spec = spec(Some(at(9, 0)), Some(at(10, 0)));
        spec.attendees = vec![
            "a@example.test".into(),
            "   ".into(),
            " b@example.test ".into(),
        ];
        spec.notify_attendees = true;
        create_meeting(&state, &spec, Some("site.webex.com"), false, false, None)
            .await
            .expect("create");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn an_account_without_a_site_is_told_so_plainly() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/meetingPreferences/sites")
            .with_status(200)
            .with_body(r#"{"sites":[]}"#)
            .create_async()
            .await;
        let state = crate::api::tests_support::state(&server.url());
        let err = test_connection(&state).await.expect_err("must fail");
        // Unsupported, not Forbidden: retrying will never help, and the user
        // needs to hear "subscription", not "permission".
        assert!(matches!(err, VcError::Unsupported(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn the_default_site_is_the_one_reported() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/meetingPreferences/sites")
            .with_status(200)
            .with_body(
                r#"{"sites":[{"siteUrl":"other.webex.com","default":false},
                            {"siteUrl":"mine.webex.com","default":true}]}"#,
            )
            .create_async()
            .await;
        let state = crate::api::tests_support::state(&server.url());
        assert_eq!(
            test_connection(&state).await.unwrap().as_deref(),
            Some("mine.webex.com")
        );
    }

    #[tokio::test]
    async fn the_personal_room_becomes_an_always_on_meeting() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/meetingPreferences/personalMeetingRoom")
            .with_status(200)
            .with_body(
                r#"{"personalMeetingRoomLink":"https://x.webex.com/meet/toni",
                    "topic":"Tonis Raum","hostPin":"1234"}"#,
            )
            .create_async()
            .await;
        let state = crate::api::tests_support::state(&server.url());
        let meeting = create_meeting(&state, &spec(None, None), None, true, false, None)
            .await
            .expect("personal room");
        assert_eq!(meeting.join_url, "https://x.webex.com/meet/toni");
        assert!(meeting.start_time.is_none() && meeting.end_time.is_none());
        assert!(is_personal_room(&meeting.id));
    }

    #[tokio::test]
    async fn a_deleted_meeting_reads_as_absent_not_as_an_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/meetings/m1")
            .with_status(404)
            .with_body(r#"{"message":"gone"}"#)
            .create_async()
            .await;
        let state = crate::api::tests_support::state(&server.url());
        assert!(get_meeting(&state, &"m1".to_string())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn deleting_asks_webex_not_to_email_anyone() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("DELETE", "/meetings/m1?sendEmail=false")
            .with_status(204)
            .create_async()
            .await;
        let state = crate::api::tests_support::state(&server.url());
        delete_meeting(&state, &"m1".to_string(), false)
            .await
            .expect("delete");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn a_cancellation_the_calendar_cannot_send_goes_out_from_webex() {
        // The other side of the same rule: on a calendar that cannot cancel
        // server-side, Webex's mail is the only word the attendees get that the
        // meeting is off. Silence there is not tidiness, it is nobody knowing.
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("DELETE", "/meetings/m1?sendEmail=true")
            .with_status(204)
            .create_async()
            .await;
        let state = crate::api::tests_support::state(&server.url());
        delete_meeting(&state, &"m1".to_string(), true)
            .await
            .expect("delete");
        m.assert_async().await;
    }
}
