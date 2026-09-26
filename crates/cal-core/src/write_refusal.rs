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
    fn a_message_that_only_starts_like_one_is_not_a_refusal() {
        assert_eq!(WriteRefusal::parse("server-refused-by-proxy: x"), None);
        assert_eq!(WriteRefusal::parse("Forbidden: nope"), None);
        assert_eq!(WriteRefusal::parse(""), None);
    }
}
