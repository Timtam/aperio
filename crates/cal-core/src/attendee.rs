//! Shared parsing for the attendee strings the UI collects.
//!
//! Aperio models attendees as a flat `Vec<String>` where each entry is
//! either `"Display Name <email@host>"` (the form the contact picker emits)
//! or a bare `"email@host"` (free-form entry). Every calendar adapter has to
//! turn that into its own wire shape — EWS `<t:Mailbox>` (Name + EmailAddress),
//! CalDAV `ATTENDEE;CN=..:mailto:..`, Google `{email, displayName}`, Graph
//! `emailAddress.{address,name}`. Centralising the split here keeps those four
//! mappings consistent instead of each re-inventing a parser.

/// Split an attendee entry into an optional display name and an email.
///
/// Recognises `"Display Name <email@host>"` (the inner address is taken
/// verbatim, the leading text becomes the trimmed, dequoted name) and bare
/// `"email@host"` (yields `(None, entry)`). A bracketed form with an empty
/// address falls back to treating the whole entry as the address. No
/// validation beyond the bracket split — callers treat the returned address
/// as authoritative.
pub fn parse(entry: &str) -> (Option<String>, String) {
    let entry = entry.trim();
    if let Some(open) = entry.rfind('<') {
        if let Some(rel_close) = entry[open + 1..].find('>') {
            let email = entry[open + 1..open + 1 + rel_close].trim();
            if !email.is_empty() {
                let name = entry[..open].trim().trim_matches('"').trim();
                let display = (!name.is_empty()).then(|| name.to_string());
                return (display, email.to_string());
            }
        }
    }
    (None, entry.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn bare_email_has_no_display_name() {
        assert_eq!(
            parse("alice@example.com"),
            (None, "alice@example.com".into())
        );
    }

    #[test]
    fn name_and_angle_bracket_email_split() {
        assert_eq!(
            parse("Alice Smith <alice@example.com>"),
            (Some("Alice Smith".into()), "alice@example.com".into())
        );
    }

    #[test]
    fn surrounding_and_inner_whitespace_trimmed() {
        assert_eq!(
            parse("  Bob   <  bob@example.com  > "),
            (Some("Bob".into()), "bob@example.com".into())
        );
    }

    #[test]
    fn quoted_display_name_is_dequoted() {
        assert_eq!(
            parse("\"Doe, John\" <john@example.com>"),
            (Some("Doe, John".into()), "john@example.com".into())
        );
    }

    #[test]
    fn angle_brackets_without_a_name() {
        assert_eq!(
            parse("<just@brackets.com>"),
            (None, "just@brackets.com".into())
        );
    }

    #[test]
    fn empty_angle_brackets_fall_back_to_whole_entry() {
        // Degenerate input — keep the raw string rather than inventing data.
        assert_eq!(parse("weird <>"), (None, "weird <>".into()));
    }
}
