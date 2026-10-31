//! The rows a meeting names its people by, as Aperio writes them new: the
//! CalDAV adapter for a meeting it creates or an invitee it adds, and the
//! request Exchange takes an imported invitation in by (I6).

use cal_core::attendee::parse;

use crate::ical_raw::{fold, param_value};

/// A new invitee's row: required, not yet answered, asked to answer.
pub fn attendee_line(entry: &str, ending: &str) -> String {
    let (name, address) = parse(entry);
    let mut line = String::from("ATTENDEE");
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        line.push_str(";CN=");
        line.push_str(&param_value(name.trim()));
    }
    line.push_str(";ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:");
    line.push_str(address.trim());
    fold(&line, ending)
}

/// The account's own row on someone else's meeting, kept from a file (I6):
/// not yet answered, and the server told to stay out of it (RFC 6638 §7.1
/// `SCHEDULE-AGENT=NONE`), as Apple marks an invitation it imports.
pub fn unscheduled_attendee_line(entry: &str, ending: &str) -> String {
    let (name, address) = parse(entry);
    let mut line = String::from("ATTENDEE");
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        line.push_str(";CN=");
        line.push_str(&param_value(name.trim()));
    }
    line.push_str(
        ";ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;SCHEDULE-AGENT=NONE:mailto:",
    );
    line.push_str(address.trim());
    fold(&line, ending)
}
