//! Why a write was refused, in a word both surfaces can translate.
//!
//! A refusal reaches the user through several layers — an adapter error, the
//! host's error type, a Tauri command or the phone's native module, a catch in
//! the editor — and each of them only carries text. A sentence written in the
//! adapter would reach a blind user in English, whatever language the app runs
//! in. So the message starts with a token from this enum, and the surfaces
//! look the sentence up in their own locale files:
//!
//! ```text
//! reply-only-invitation: title
//! server-refused: need-privileges
//! identity-unknown
//! ```
//!
//! The token is the rule; the detail behind the colon is a field name
//! ([`crate::event_diff::EventField::token`]) or whatever the server named, and
//! a surface that does not know the detail still has a sentence for the token.

use serde::{Deserialize, Serialize};

/// The reason a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum WriteRefusal {
    /// The provider takes only the attendee's own changes to a meeting someone
    /// else organizes (RFC 6638 §3.2.2.1, decision 77a). The detail names the
    /// field that was changed, or the precondition the server named.
    ReplyOnlyInvitation,
    /// The server said no for a reason of its own; the detail is its
    /// `DAV:error` condition.
    ServerRefused,
    /// The account's own addresses on this server could not be read, so
    /// nothing can be decided about a meeting and nothing was written.
    IdentityUnknown,
    /// One occurrence could not be kept inside its series as an exception, so
    /// nothing was written (decision 79b). The detail is a machine token for
    /// the log — `no-master`, `not-recurring`, `already-skipped`,
    /// `ambiguous-local-time`, … — because to the user every one of them means
    /// the same thing: this occurrence could not be saved on its own. Carving
    /// it out instead is never done behind their back (decision 92).
    OccurrenceNotWritable,
    /// Aperio did not write, because the event could not be written without
    /// risking what else its resource holds: the resource cannot be read
    /// block by block, it holds no such event, or its components name
    /// different organizers. The detail is a machine token for the log, as
    /// for [`Self::OccurrenceNotWritable`].
    UnsafeToWrite,
    /// The operating system has not granted the device's own calendars or
    /// reminders ([`crate::Error::AccessNotGranted`]). Reads carry it too: it
    /// is the same sentence whichever way the account was touched. The
    /// detail names the store and its state for the log.
    AccessNotGranted,
    /// An all-day appointment's day could not be changed, because the provider
    /// stores it in a time zone Aperio cannot read, so its midnights are
    /// unknown (decision 237); or a timed series could not be cut, because the
    /// day it ends on is counted on that zone (decision 256). Its title and
    /// the rest still save. The detail is a machine token for the log:
    /// `no-zone` (no zone, or an id with no definition), `no-midnight` (the
    /// zone has no midnight that day), `no-wall` (the zone cannot place the
    /// series' time of day).
    DayZoneUnreadable,
    /// The provider's current copy of the item could not be read, and the
    /// write needs it, so nothing was sent: a retry may well work. The detail
    /// is the read's own error, for the log.
    CopyUnreadable,
    /// The write would make the provider drop occurrences of a series the user
    /// changed on their own — and deleted ones the adapter cannot delete again
    /// afterwards —, and the user has not agreed ([`crate::Event`]'s
    /// `accepts_exception_loss`), so nothing was sent (decisions 245-253).
    /// Exchange drops them whenever a series' start and end, the clock of its
    /// zone or its pattern are written again. The detail is
    /// [`SeriesRewrite::detail`]: what the update rewrites, and how many
    /// changed and how many deleted occurrences would be lost, `slot:2:1`;
    /// the surfaces ask with it and send the write again with the consent.
    ExceptionsWouldBeLost,
    /// A new series was written, its deleted occurrences could not all be
    /// removed from it again one by one, and it was deleted itself, so nothing
    /// stands and the old series is as it was (decisions 202, 203). Unlike the
    /// other refusals something was written for a moment: the provider deleted
    /// the series like any other, and where the detail says
    /// [`crate::step_write::INVITED`] its invitees may have heard of it and got
    /// its cancellation too (212). The detail is
    /// [`crate::step_write::step_detail`]: a machine token for the log, marked
    /// when the invitees may have heard of it — `no-occurrence` (no occurrence
    /// found where the provider's rule makes one), `foreign-deletion`,
    /// `unreadable`, `control` and `after-control` (the series read before and
    /// after), `time`, `invitation`, `network`, `http-<status>` or a code of the
    /// provider's own, `<provider>-<code>`.
    DeletionsNotKept,
    /// The provider turned the request away because it is throttling, and the
    /// request wrote nothing (decision 270): try again later. Reads carry it
    /// too, as for [`Self::AccessNotGranted`]. The detail is a
    /// [`BusyDetail`] word for the log: never a status or a word an older
    /// host's text heuristic takes for a sign-in failure. A throttle after a
    /// request of the same save had landed is no refusal but
    /// [`crate::WriteLeftover::PartlySaved`] (277).
    ProviderBusy,
}

/// What a provider answered when it turned a request away for throttling, as
/// the detail of [`WriteRefusal::ProviderBusy`]: a closed set of neutral
/// words, so that none of them, on any host, reads as a sign-in failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyDetail {
    /// HTTP 429, too many requests.
    TooManyRequests,
    /// HTTP 503 with a wait to keep: a provider that throttles that way.
    Unavailable,
    /// The provider's own answer that a rate limit was reached.
    RateLimit,
    /// The provider's own answer that this user's rate limit was reached.
    UserRateLimit,
    /// The provider's own answer that its server is busy.
    ServerBusy,
}

impl BusyDetail {
    /// Every detail, for the tests that keep them neutral.
    pub const ALL: [Self; 5] = [
        Self::TooManyRequests,
        Self::Unavailable,
        Self::RateLimit,
        Self::UserRateLimit,
        Self::ServerBusy,
    ];

    /// The word the detail is written as.
    pub const fn word(self) -> &'static str {
        match self {
            Self::TooManyRequests => "http-429",
            Self::Unavailable => "http-503",
            Self::RateLimit => "rate-limit",
            Self::UserRateLimit => "user-rate-limit",
            Self::ServerBusy => "server-busy",
        }
    }
}

/// What an update writes again that makes a provider drop a series' changed
/// and deleted occurrences (decisions 243-253): its start and end — a move, a
/// new length, all-day on or off —, only the clock its zone names (245), or
/// its pattern: its frequency, the days, the interval it repeats on (the
/// days and the interval measured in live round 6). Its range, how many times
/// or until when it runs, keeps them, also where it cuts occurrences off (the
/// zone-first live test, M5; round 8, U1-U3; round 9, V1, V2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum SeriesRewrite {
    /// Start and end: a move, a new length, all-day on or off.
    Slot,
    /// Only the zone, whose clock is another than the stored one.
    Zone,
    /// The pattern: its frequency, the days, the interval it repeats on.
    Pattern,
}

impl SeriesRewrite {
    /// The token the detail starts with.
    pub const fn token(self) -> &'static str {
        match self {
            Self::Slot => "slot",
            Self::Zone => "zone",
            Self::Pattern => "pattern",
        }
    }

    /// The detail of [`WriteRefusal::ExceptionsWouldBeLost`]: this rewrite,
    /// and how many occurrences changed on their own and how many deleted ones
    /// it would lose — a changed one takes the series' details again, a
    /// deleted one comes back: `slot:2:1` (decision 252).
    pub fn detail(self, changed: usize, deleted: usize) -> String {
        format!("{}:{changed}:{deleted}", self.token())
    }

    /// The rewrite and the counts a detail names, as [`Self::detail`] writes
    /// it; `None` for anything else.
    pub fn parse_detail(detail: &str) -> Option<(Self, usize, usize)> {
        let mut parts = detail.trim().split(':');
        let token = parts.next()?;
        let rewrite = [Self::Slot, Self::Zone, Self::Pattern]
            .into_iter()
            .find(|r| r.token() == token)?;
        let changed = parts.next()?.parse().ok()?;
        let deleted = parts.next()?.parse().ok()?;
        parts
            .next()
            .is_none()
            .then_some((rewrite, changed, deleted))
    }
}

impl WriteRefusal {
    /// The token that starts the message.
    pub const fn token(self) -> &'static str {
        match self {
            Self::ReplyOnlyInvitation => "reply-only-invitation",
            Self::ServerRefused => "server-refused",
            Self::IdentityUnknown => "identity-unknown",
            Self::OccurrenceNotWritable => "occurrence-not-writable",
            Self::UnsafeToWrite => "unsafe-to-write",
            Self::AccessNotGranted => "access-not-granted",
            Self::DayZoneUnreadable => "day-zone-unreadable",
            Self::CopyUnreadable => "copy-unreadable",
            Self::ExceptionsWouldBeLost => "exceptions-would-be-lost",
            Self::DeletionsNotKept => "deletions-not-kept",
            Self::ProviderBusy => "provider-busy",
        }
    }

    /// Whether a server that answered a write with this HTTP status turned the
    /// write down whole: it read the request and wrote nothing (bad request,
    /// too large, wrong media type, unprocessable, too many requests, no
    /// storage). The statuses the adapters already name — 401, 403, 404, 409,
    /// 412 — carry their own error; a 5xx may come after a part was written
    /// and says nothing either way.
    ///
    /// Where a write is refused this way, an adapter reports
    /// [`Self::ServerRefused`] rather than a protocol error, so that a caller
    /// deciding whether the write may have landed (splitting a series undoes
    /// its new part only when the cut certainly did not, decision 144) is told
    /// the truth.
    pub const fn refused_status(status: u16) -> bool {
        matches!(status, 400 | 413 | 415 | 422 | 429 | 507)
    }

    /// The refusal a write turned down whole with this HTTP status names: a
    /// busy provider for 429 (decision 270), the server's own refusal for the
    /// other statuses [`Self::refused_status`] knows, `None` otherwise. The one
    /// place that says which comes first, for every adapter.
    pub const fn turned_down_by_status(status: u16) -> Option<Self> {
        match status {
            429 => Some(Self::ProviderBusy),
            400 | 413 | 415 | 422 | 507 => Some(Self::ServerRefused),
            _ => None,
        }
    }

    /// The message: the token, and the detail behind a colon when there is one.
    pub fn message(self, detail: &str) -> String {
        let detail = detail.trim();
        if detail.is_empty() {
            self.token().to_string()
        } else {
            format!("{}: {detail}", self.token())
        }
    }

    /// The refusal a message names, and its detail. The surfaces do the same
    /// on their side (`shared/eventWriteError.ts`), which is why the token is
    /// read from the start of the message and never from the error's code: a
    /// refusal travels as whichever error its layer already used.
    pub fn parse(message: &str) -> Option<(Self, &str)> {
        let message = message.trim();
        for refusal in [
            Self::ReplyOnlyInvitation,
            Self::ServerRefused,
            Self::IdentityUnknown,
            Self::OccurrenceNotWritable,
            Self::UnsafeToWrite,
            Self::AccessNotGranted,
            Self::DayZoneUnreadable,
            Self::CopyUnreadable,
            Self::ExceptionsWouldBeLost,
            Self::DeletionsNotKept,
            Self::ProviderBusy,
        ] {
            let token = refusal.token();
            let rest = match message.strip_prefix(token) {
                Some(rest) => rest,
                None => continue,
            };
            return match rest.strip_prefix(':') {
                Some(detail) => Some((refusal, detail.trim())),
                None if rest.is_empty() => Some((refusal, "")),
                // "server-refused-by-proxy" names no refusal of ours.
                None => continue,
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_refusal_comes_back_from_its_message() {
        let msg = WriteRefusal::ProviderBusy.message(BusyDetail::RateLimit.word());
        assert_eq!(msg, "provider-busy: rate-limit");
        assert_eq!(
            WriteRefusal::parse(&msg),
            Some((WriteRefusal::ProviderBusy, "rate-limit"))
        );
        assert_eq!(
            WriteRefusal::parse("provider-busy"),
            Some((WriteRefusal::ProviderBusy, ""))
        );
        assert_eq!(WriteRefusal::parse("provider-busy-elsewhere: x"), None);
        assert_eq!(
            serde_json::to_value(WriteRefusal::ProviderBusy).unwrap(),
            serde_json::json!("provider-busy")
        );
    }

    #[test]
    fn a_status_turned_down_whole_names_busy_first() {
        assert_eq!(
            WriteRefusal::turned_down_by_status(429),
            Some(WriteRefusal::ProviderBusy)
        );
        for status in [400, 413, 415, 422, 507] {
            assert_eq!(
                WriteRefusal::turned_down_by_status(status),
                Some(WriteRefusal::ServerRefused),
                "{status}"
            );
        }
        for status in [200, 401, 403, 404, 409, 412, 500, 502, 503] {
            assert_eq!(
                WriteRefusal::turned_down_by_status(status),
                None,
                "{status}"
            );
        }
        // Every status it names is one turned down whole.
        for status in 0..1000u16 {
            if WriteRefusal::turned_down_by_status(status).is_some() {
                assert!(WriteRefusal::refused_status(status), "{status}");
            }
        }
    }

    #[test]
    fn a_busy_detail_never_looks_like_a_sign_in_failure() {
        // The needles an older host's heuristic reads a login problem by
        // (host-core `is_auth_shaped`); host-core's own test reads the same
        // messages with the real heuristic.
        let needles = [
            "401",
            "403",
            "unauthorized",
            "unauthorised",
            "forbidden",
            "authentication",
            "invalid credentials",
            "password",
            "invalid_grant",
            "invalid_client",
            "expired or revoked",
        ];
        for detail in BusyDetail::ALL {
            let msg = WriteRefusal::ProviderBusy.message(detail.word());
            for needle in needles {
                assert!(!msg.to_lowercase().contains(needle), "{msg} {needle}");
            }
        }
    }

    #[test]
    fn the_access_refusal_comes_back_from_its_message() {
        let msg = WriteRefusal::AccessNotGranted.message("calendars: Denied");
        assert_eq!(msg, "access-not-granted: calendars: Denied");
        assert_eq!(
            WriteRefusal::parse(&msg),
            Some((WriteRefusal::AccessNotGranted, "calendars: Denied"))
        );
    }

    #[test]
    fn a_message_carries_its_token_and_comes_back() {
        let msg = WriteRefusal::ReplyOnlyInvitation.message("title");
        assert_eq!(msg, "reply-only-invitation: title");
        assert_eq!(
            WriteRefusal::parse(&msg),
            Some((WriteRefusal::ReplyOnlyInvitation, "title"))
        );
        assert_eq!(
            WriteRefusal::IdentityUnknown.message(""),
            "identity-unknown"
        );
        assert_eq!(
            WriteRefusal::parse("identity-unknown"),
            Some((WriteRefusal::IdentityUnknown, ""))
        );
        assert_eq!(
            WriteRefusal::parse("server-refused: need-privileges"),
            Some((WriteRefusal::ServerRefused, "need-privileges"))
        );
    }

    #[test]
    fn every_refusal_is_read_back() {
        for refusal in [
            WriteRefusal::ReplyOnlyInvitation,
            WriteRefusal::ServerRefused,
            WriteRefusal::IdentityUnknown,
            WriteRefusal::OccurrenceNotWritable,
            WriteRefusal::UnsafeToWrite,
            WriteRefusal::AccessNotGranted,
            WriteRefusal::DayZoneUnreadable,
            WriteRefusal::CopyUnreadable,
            WriteRefusal::ExceptionsWouldBeLost,
            WriteRefusal::DeletionsNotKept,
        ] {
            let msg = refusal.message("detail");
            assert_eq!(
                WriteRefusal::parse(&msg),
                Some((refusal, "detail")),
                "{msg}"
            );
            // The token is the serialized name the surfaces look up.
            assert_eq!(
                serde_json::to_value(refusal).unwrap(),
                serde_json::json!(refusal.token()),
            );
        }
    }

    #[test]
    fn a_refusing_status_is_one_that_wrote_nothing() {
        for status in [400, 413, 415, 422, 429, 507] {
            assert!(WriteRefusal::refused_status(status), "{status}");
        }
        // Named elsewhere (401, 403, 404, 409, 412), or unknown either way.
        for status in [200, 401, 403, 404, 409, 412, 500, 502, 503, 504] {
            assert!(!WriteRefusal::refused_status(status), "{status}");
        }
    }

    #[test]
    fn a_series_rewrite_detail_comes_back() {
        for rewrite in [
            SeriesRewrite::Slot,
            SeriesRewrite::Zone,
            SeriesRewrite::Pattern,
        ] {
            let detail = rewrite.detail(2, 1);
            assert_eq!(SeriesRewrite::parse_detail(&detail), Some((rewrite, 2, 1)));
            // The token is the serialized name the surfaces read.
            assert_eq!(
                serde_json::to_value(rewrite).unwrap(),
                serde_json::json!(rewrite.token()),
            );
        }
        // The wire shape the surfaces parse (shared/exceptionsLoss.ts).
        assert_eq!(SeriesRewrite::Slot.detail(2, 1), "slot:2:1");
        assert_eq!(
            WriteRefusal::ExceptionsWouldBeLost.message(&SeriesRewrite::Zone.detail(1, 0)),
            "exceptions-would-be-lost: zone:1:0"
        );
        assert_eq!(SeriesRewrite::parse_detail("slot"), None);
        assert_eq!(SeriesRewrite::parse_detail("slot:3"), None);
        assert_eq!(SeriesRewrite::parse_detail("slot:many:0"), None);
        assert_eq!(SeriesRewrite::parse_detail("slot:1:0:9"), None);
        assert_eq!(SeriesRewrite::parse_detail("moved:3:0"), None);
    }

    #[test]
    fn a_message_that_only_starts_like_one_is_not_a_refusal() {
        assert_eq!(WriteRefusal::parse("server-refused-by-proxy: x"), None);
        assert_eq!(WriteRefusal::parse("Forbidden: nope"), None);
        assert_eq!(WriteRefusal::parse(""), None);
    }
}
