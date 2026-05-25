//! Shared types + trait for Aperio's video-conference adapter
//! plugins (DESIGN.md §11).
//!
//! The host calls into a `VcAdapter` whenever the user clicks
//! "Generate meeting link" on an event, deletes one, or asks
//! for the current join URL. Each provider — Zoom, Microsoft
//! Teams, Google Meet, Cisco WebEx — implements this trait
//! once; the implementation is then packaged as a
//! `videoconference-adapter` plugin (see `vc-adapter-*-plugin`
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
//!     provider side; called when the event is deleted or the
//!     user removes the link via the event editor.
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
/// id, …). The host stores it as part of the calendar event's
/// `vc_meeting_id` column and threads it back into
/// `get_meeting` / `delete_meeting` calls.
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
    /// about it (treat as a soft delete + clear the cached
    /// `vc_meeting_id` on the event).
    async fn get_meeting(&self, id: &MeetingId) -> VcResult<Option<Meeting>>;

    /// Drop the meeting on the provider side. Called when the
    /// user explicitly removes the link from an event or
    /// deletes the event itself with "also delete provider-
    /// side meeting" confirmed.
    async fn delete_meeting(&self, id: &MeetingId) -> VcResult<()>;
}
