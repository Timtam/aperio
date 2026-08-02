//! Shared types + trait for Aperio's video-conference adapter
//! plugins (DESIGN.md §11).
//!
//! The host calls into a `VcAdapter` whenever the user clicks
//! "Generate meeting link" on an event, deletes one, or asks
//! for the current join URL. Each provider — Zoom, Microsoft
//! Teams, Google Meet, Cisco WebEx — implements this trait
//! once; the implementation is then packaged as a
//! `videoconference-adapter` plugin (see `adapter-*-plugin`
//! crates) so the host loads + invokes it through the same
//! C-ABI surface that cal- and sync-adapters use.
//!
//! ## Minimum viable surface
//!
//! v1 focuses on the four verbs the UI needs to drive the
//! "Meeting beitreten" affordance described in §11.2:
//!
//!   - [`VcAdapter::test_connection`] — credential / network
//!     smoke-test surfaced by the AccountsDialog's "Test
//!     connection" button.
//!   - [`VcAdapter::create_meeting`] — generate a fresh
//!     meeting + return its join URL so the calendar layer can
//!     embed it in the event description.
//!   - [`VcAdapter::delete_meeting`] — drop the meeting on the
//!     provider side; called when the user removes the meeting
//!     via the event editor. Deleting the event does NOT cascade.
//!   - [`VcAdapter::get_meeting`] — re-fetch a previously-
//!     created meeting (status, join URL, password) so the
//!     event-detail view can verify the meeting is still valid
//!     before showing the "Join" button.
//!
//! Room booking (§11.2's "Auswahl verfügbarer Konferenzräume")
//! lands later as an optional capability — Zoom Rooms and
//! Microsoft Teams have it natively; Meet and basic WebEx
//! don't. Capability discovery follows the cal-core
//! `Capability` pattern.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Per-provider identifier for a created meeting. The string
/// is opaque to the host — each provider uses its own format
/// (Zoom's numeric meeting id, Teams's GUID, Meet's URL-safe
/// id, …).
///
/// The host keeps it in a HOST-LOCAL binding table alongside the event id and
/// the account that minted it (`host_core::meetings`), not on the event itself
/// and not in anything that syncs: it is the provider's private handle, useless
/// on another device, while the join URL in the event body is what every other
/// client actually reads. Threaded back into `get_meeting` and `delete_meeting`.
pub type MeetingId = String;

/// A meeting that already exists on the provider side. The
/// host displays `join_url` as the "Meeting beitreten" link in
/// the event detail view; `password` (when present) is shown
/// alongside so the user can paste it into the provider's
/// native client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meeting {
    /// Provider-side identifier. Opaque to the host.
    pub id: MeetingId,
    /// URL the user clicks (or the host opens in the system
    /// browser) to join the meeting.
    pub join_url: String,
    /// Provider's display title for the meeting. Echoes back
    /// what was passed in [`NewMeeting::title`].
    pub title: String,
    /// Scheduled start time. `None` for instant / always-on
    /// meetings (Meet's default, Zoom's Personal Meeting Room).
    pub start_time: Option<DateTime<Utc>>,
    /// Scheduled end time. `None` follows the same logic as
    /// [`Self::start_time`].
    pub end_time: Option<DateTime<Utc>>,
    /// Numeric / alphanumeric join code the user has to enter
    /// when they click into the meeting via the provider's
    /// native client. `None` when the meeting doesn't require
    /// one (Meet's default behaviour).
    pub password: Option<String>,

    /// Who the PROVIDER has invited, when it can say.
    ///
    /// Not the same thing as the calendar event's attendees, and worth keeping
    /// apart from them. An event auto-created from a provider's invitation mail
    /// lists whatever that mail addressed — often just the recipient and the
    /// provider's own sending address (`messenger@webex.com`), while the
    /// people actually in the meeting are known only to the provider. Showing
    /// the meeting's own list next to the event's is the difference between
    /// "two attendees, one of them a robot" and the truth.
    ///
    /// Empty when the provider cannot say, or will not for this caller: reading
    /// a meeting one is merely invited to is not something every provider
    /// permits, and an empty list is the honest answer rather than a failure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invitees: Vec<MeetingInvitee>,

    /// Everything else somebody needs to get in, in the order an invitation
    /// presents it: meeting number, the password a keypad can actually take,
    /// dial-in numbers, the SIP address, a link to more numbers.
    ///
    /// This is what makes the block Aperio writes into an event useful to
    /// somebody joining by PHONE. Without it the event carries a link and a
    /// password and nothing to dial, which is not an invitation for anyone
    /// without a browser to hand.
    ///
    /// The first entry SHOULD be the join link itself, so the whole block is
    /// one uniform list; a host that finds it missing prepends
    /// [`Self::join_url`] under a default label rather than writing a block
    /// with no way in. Empty is a valid answer — a provider that offers only a
    /// link says so by saying nothing, and the block degrades to that one line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub join_details: Vec<JoinDetail>,
}

/// What a value in a [`JoinDetail`] actually is, so the host can render it
/// without knowing the provider.
///
/// Deliberately about the VALUE, not about the meaning: the core has no
/// business knowing what a "meeting number" is, but it does need to know that
/// this string is a phone number and that one is a link — to offer the right
/// affordance, to allow the right URL scheme, and later to emit the right
/// `FEATURE=` on an RFC 7986 `CONFERENCE` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinDetailKind {
    /// Plain text. The safe default for anything unrecognised.
    #[default]
    Text,
    /// An `http(s)` link — the join link itself, or a page listing more
    /// dial-in numbers.
    Url,
    /// A dialable phone number. Written in the block as the provider gives it,
    /// digit grouping and all: screen readers chunk digits at whitespace, and
    /// somebody reading a number aloud needs it grouped the way it is printed.
    Tel,
    /// A SIP address for a video system.
    Sip,
    /// Something typed rather than dialled or clicked — a meeting number, an
    /// access code, a numeric password.
    Code,
}

/// One labelled fact about how to join a meeting.
///
/// The core owns the SHAPE — an ordered list of labelled lines, rendered the
/// same way for every provider — and the adapter owns the words and the values.
/// That is what keeps four providers looking alike in an invitation without the
/// core learning what any of them calls anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinDetail {
    /// Key into the plugin's own string catalogue (`plugin.json` → `strings`),
    /// resolved by the host in the language the caller asks for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_key: Option<String>,

    /// The label to use when the catalogue cannot answer — no entry, no
    /// catalogue at all, or a third-party plugin that ships none. Written in
    /// whatever language the adapter author chose; it is better than a bare
    /// value with nothing naming it.
    pub label: String,

    /// The value itself, verbatim from the provider.
    pub value: String,

    #[serde(default)]
    pub kind: JoinDetailKind,
}

/// One person the provider lists on a meeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingInvitee {
    pub email: String,
    /// The provider's display name, when it has one. Falls back to the address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Whether this person can host in the organizer's absence.
    #[serde(default)]
    pub co_host: bool,
}

/// Inputs the host hands to [`VcAdapter::create_meeting`].
/// Mirrors what the event editor knows when the user clicks
/// "Generate meeting link": the event's title + scheduled time
/// window + an optional description the provider can embed in
/// its own meeting metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMeeting {
    /// Display title for the meeting on the provider's side.
    /// The host passes the event's title verbatim — providers
    /// that have separate "topic" + "description" fields
    /// usually put this in the topic.
    pub title: String,
    /// Scheduled start time. `None` requests an instant
    /// meeting where supported (Meet) or falls back to a
    /// provider default (Zoom's Personal Meeting Room).
    pub start_time: Option<DateTime<Utc>>,
    /// Scheduled end time. `None` paired with `start_time:
    /// Some(_)` requests an open-ended meeting where the
    /// provider supports it; otherwise the provider picks a
    /// default duration.
    pub end_time: Option<DateTime<Utc>>,
    /// Optional richer description the provider can attach to
    /// the meeting metadata. Most providers surface this in
    /// their calendar invite alongside the join URL.
    #[serde(default)]
    pub description: Option<String>,

    /// Use the account's permanent room instead of minting a meeting.
    ///
    /// Per REQUEST, not per account. Which of the two a meeting should be is a
    /// property of the meeting — a quick one-to-one wants the room, something
    /// scheduled with outsiders wants its own — and asking once at setup time
    /// forces one answer onto everything that follows.
    ///
    /// Providers without a permanent room ignore it.
    #[serde(default)]
    pub use_personal_room: bool,

    /// The event's attendees, so the provider knows who the meeting is for.
    ///
    /// **Bare email addresses.** Aperio's own attendee entries are display
    /// strings — `"Alice Smith <alice@example.test>"` from the contact picker,
    /// and the same shape read back from Google, Graph, EWS and CalDAV — and a
    /// provider validates this field as an address, so the host splits before
    /// filling it (`host_core::meetings::attendee_addresses`). An entry that
    /// yields no address is dropped rather than sent: a display name in an
    /// address field reaches nobody, and on a strict provider it fails the
    /// whole meeting.
    ///
    /// Without this a provider has no idea who is coming: it can neither list
    /// them back (the "who is actually invited" question) nor notify them.
    /// These addresses LEAVE the device — that is the point of handing them to
    /// a meeting service — so the host passes them only when it is creating a
    /// meeting for an event that has them.
    #[serde(default)]
    pub attendees: Vec<String>,

    /// Whether the provider should email [`Self::attendees`] itself.
    ///
    /// The host sets this only when it has no other way to reach them. A
    /// calendar that can invite server-side already will, and a provider mail
    /// on top of that puts a SECOND iCalendar attachment in every attendee's
    /// mailbox — and so a duplicate entry in their calendar. When the calendar
    /// cannot invite (a local calendar, a subscribed feed, a CalDAV server
    /// without RFC 6638), the provider's mail is the only invitation there is,
    /// and suppressing it means nobody is told at all.
    #[serde(default)]
    pub notify_attendees: bool,
}

/// Inputs the host hands to [`VcAdapter::delete_meeting`].
///
/// A struct rather than a bare id, for the same reason [`NewMeeting`] is one:
/// taking a meeting down is not only "which", it is also "and what do the
/// people who were invited hear about it". A provider that can email them is
/// the only channel some calendars have, and the answer belongs to the call,
/// not to a setting somewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingRemoval {
    /// Which meeting. The provider's own identifier, opaque to the host,
    /// exactly as it came back from [`VcAdapter::create_meeting`].
    pub id: MeetingId,

    /// Whether the provider should tell the attendees the meeting is off.
    ///
    /// The mirror of [`NewMeeting::notify_attendees`], and answered from the
    /// same fact: a calendar that can invite server-side also cancels
    /// server-side, so a provider cancellation on top of it is a second,
    /// contradictory message. A calendar that cannot invite cannot cancel
    /// either, and there the provider's mail is the only word the attendees
    /// will ever get that the meeting is not happening.
    #[serde(default)]
    pub notify_attendees: bool,
}

impl MeetingRemoval {
    /// Take a meeting down and let the provider tell the attendees, or not.
    pub fn new(id: impl Into<MeetingId>, notify_attendees: bool) -> Self {
        Self {
            id: id.into(),
            notify_attendees,
        }
    }

    /// Take a meeting down without telling anybody.
    ///
    /// The honest answer whenever nobody was ever told the meeting existed —
    /// rolling one back because the event it belonged to could not be saved —
    /// and the safe default for a caller that has no event to answer the
    /// question from. Silence cannot produce a contradictory message; the
    /// worst it does is leave someone to find out from the calendar.
    pub fn silent(id: impl Into<MeetingId>) -> Self {
        Self::new(id, false)
    }
}

/// Error variants every provider-specific adapter has to map
/// its underlying API errors onto. Mirrors the shape of
/// `cal_core::Error` + `sync_core::SyncError` so the command
/// layer can pattern-match consistently across all three.
#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VcError {
    /// Provider rejected the credentials (expired access
    /// token, revoked OAuth grant, …). The host surfaces this
    /// to the UI as "Sign in to the provider again" rather
    /// than a generic error.
    #[error("authentication: {0}")]
    Authentication(String),

    /// Authenticated but the user lacks the necessary scope
    /// (Zoom basic-tier accounts can't create meetings, Teams
    /// users without a Teams licence, …).
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// The meeting id passed to `get_meeting` / `delete_meeting`
    /// doesn't exist (already deleted, never created, wrong
    /// account, …).
    #[error("not found: {0}")]
    NotFound(String),

    /// Network problem (DNS, TLS handshake, connection
    /// refused). Treated as transient by the UI — the user can
    /// retry.
    #[error("network: {0}")]
    Network(String),

    /// Provider responded but the response shape didn't match
    /// what we expect (API version drift, unsupported
    /// regional variant). Tends to be a bug we need to fix on
    /// the adapter side rather than something the user can
    /// recover from.
    #[error("protocol: {0}")]
    Protocol(String),

    /// The supplied [`NewMeeting`] was rejected before the
    /// provider was even reached (empty title, end before
    /// start, …).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Provider returned a response indicating the requested
    /// operation isn't supported on the current plan / region
    /// (e.g. recording on free Zoom). Distinct from
    /// `Forbidden` because retrying with the same account
    /// won't help — the user has to upgrade or pick a
    /// different provider.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Catch-all for anything else — the adapter couldn't put
    /// it in any of the more specific buckets. The message
    /// gets surfaced to the user verbatim.
    #[error("internal: {0}")]
    Internal(String),
}

/// Convenience alias to match the cal-core / sync-core
/// pattern.
pub type VcResult<T> = Result<T, VcError>;

/// What a single provider-specific adapter has to implement.
/// One instance per configured account (a user might have two
/// Zoom accounts; the host opens two plugin instances against
/// the same shared library).
///
/// All methods are async — every provider's API is HTTP-bound,
/// so the implementations end up `.await`-ing reqwest calls.
/// The host's plugin runtime (`tokio` current-thread, set up
/// by `plugin-sdk`) drives them inside the FFI fn bodies.
#[async_trait]
pub trait VcAdapter: Send + Sync {
    /// Smoke-test the configured credentials + network reach.
    /// Drives the AccountsDialog's "Test connection" button.
    /// Implementations typically issue a lightweight
    /// `GET /users/me` or equivalent so the round-trip stays
    /// cheap.
    async fn test_connection(&self) -> VcResult<()>;

    /// Create a fresh meeting on the provider side and return
    /// the populated [`Meeting`]. The host stores the returned
    /// `id` against the event so a later `delete_meeting` /
    /// `get_meeting` can address it.
    async fn create_meeting(&self, spec: NewMeeting) -> VcResult<Meeting>;

    /// Re-fetch an existing meeting by its provider-side id.
    /// `None` means the meeting was deleted on the provider
    /// side between this call and whenever the host last knew
    /// about it (treat as a soft delete + drop the host's
    /// binding for that event).
    async fn get_meeting(&self, id: &MeetingId) -> VcResult<Option<Meeting>>;

    /// Drop the meeting on the provider side. Called when the user removes the
    /// meeting from an event — there is no event-delete cascade; deleting an
    /// event leaves its meeting standing, because the binding that names it is
    /// host-local and the event may be deleted on a device that never had it.
    ///
    /// [`MeetingRemoval::notify_attendees`] says whether the provider should
    /// email the people who were invited. The host answers it from the event's
    /// own calendar — see [`NewMeeting::notify_attendees`] for the same
    /// reasoning in the other direction — and an adapter whose provider cannot
    /// notify simply ignores it.
    async fn delete_meeting(&self, removal: MeetingRemoval) -> VcResult<()>;

    /// The meeting a join link belongs to, or `None` when the provider has
    /// none for it.
    ///
    /// The link is the only identifier that reaches a calendar. It travels in
    /// the event, where Outlook, a phone and a colleague's client all read it;
    /// the provider's own meeting id travels nowhere. Without this the host can
    /// manage only meetings it created *itself* and still has a local record
    /// of — not one made in the provider's own web UI, not one made on the
    /// user's other device, not one an invitation brought in.
    ///
    /// Defaults to [`VcError::Unsupported`]: not every provider offers a lookup
    /// by link, and one that does not is not broken.
    async fn resolve_meeting(&self, join_url: &str) -> VcResult<Option<Meeting>> {
        let _ = join_url;
        Err(VcError::Unsupported(
            "this provider cannot look a meeting up by its join link".into(),
        ))
    }

    /// Whether [`Self::list_meetings`] is actually wired.
    ///
    /// Synchronous and free, because the caller needs it while DECIDING
    /// whether to offer a meetings calendar at all — a decision made on a
    /// registration path that cannot await, and one that must not be made by
    /// calling the provider.
    ///
    /// Behind the plugin ABI this is exactly "is the vtable slot non-NULL",
    /// which is the ABI's own answer to the same question. A manifest flag
    /// beside it would be a second source of one truth, free to disagree.
    fn can_list_meetings(&self) -> bool {
        false
    }

    /// The account's scheduled meetings between `start` and `end`.
    ///
    /// What makes meetings *without* a calendar entry visible at all — the ones
    /// created straight in the provider's web UI, which otherwise exist only
    /// there. The host surfaces them as read-only events.
    ///
    /// Defaults to [`VcError::Unsupported`]; a provider that cannot enumerate
    /// simply gets no such view.
    async fn list_meetings(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> VcResult<Vec<Meeting>> {
        let _ = (start, end);
        Err(VcError::Unsupported(
            "this provider cannot list meetings".into(),
        ))
    }
}
