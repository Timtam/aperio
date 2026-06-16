//! UniFFI surface for Aperio's mobile clients.
//!
//! This crate is the *engine-reuse* boundary. The Rust domain logic in
//! [`cal_core`] stays the single source of truth; this thin wrapper re-exports
//! selected pieces of it across an FFI boundary so a Swift (iOS) or Kotlin
//! (Android) UI can call them. The UI is rebuilt per platform — the engine is
//! not.
//!
//! Kept deliberately small for now: one pure, value-in / value-out function, so
//! the binding-generation toolchain is proven before anything that crosses a
//! `DateTime` (which needs a custom UniFFI type converter) or a large composite
//! type.

uniffi::setup_scaffolding!();

/// A parsed attendee entry: an optional display name plus the email address.
///
/// Mirrors the `(Option<String>, String)` tuple [`cal_core::attendee::parse`]
/// returns — UniFFI needs a named record rather than a bare tuple.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttendee {
    /// Display name, if the entry carried one (`"Jane Doe <jane@host>"`).
    pub name: Option<String>,
    /// The email address (authoritative; taken verbatim from the entry).
    pub email: String,
}

/// Parse a calendar attendee entry into its display name and email.
///
/// Accepts `"Display Name <email@host>"` or a bare `"email@host"`, delegating
/// the split to [`cal_core::attendee::parse`] so the mobile UI and every
/// desktop adapter share one parser.
#[uniffi::export]
pub fn parse_attendee(entry: String) -> ParsedAttendee {
    let (name, email) = cal_core::attendee::parse(&entry);
    ParsedAttendee { name, email }
}

#[cfg(test)]
mod tests {
    use super::{parse_attendee, ParsedAttendee};

    #[test]
    fn wraps_core_parser_for_named_entry() {
        assert_eq!(
            parse_attendee("Alice Smith <alice@example.com>".to_string()),
            ParsedAttendee {
                name: Some("Alice Smith".to_string()),
                email: "alice@example.com".to_string(),
            }
        );
    }

    #[test]
    fn bare_email_has_no_name() {
        assert_eq!(
            parse_attendee("bob@example.com".to_string()),
            ParsedAttendee {
                name: None,
                email: "bob@example.com".to_string(),
            }
        );
    }
}
