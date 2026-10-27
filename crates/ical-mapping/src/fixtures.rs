//! Anonymised samples of what providers send, for the tests of this crate and
//! of the crates that write through it (the CalDAV adapter). Compiled for tests
//! only (`test-fixtures`).

use crate::identity::OwnIdentity;

/// A meeting the account organizes, the way iCloud stores it (live
/// measurement M1, anonymised): ORGANIZER and the account's own CHAIR row
/// name a principal path, with the address in EMAIL. Aperio shows the
/// organizer by that address, drops the CHAIR row from the invitees, and
/// recognises the account by its principal path.
pub const ICLOUD_MEETING: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Apple Inc.//iPhone OS 26.0//EN\r\n\
BEGIN:VEVENT\r\n\
ATTENDEE;CN=Toni Barth;CUTYPE=INDIVIDUAL;PARTSTAT=ACCEPTED;EMAIL=toni@example\r\n \
.org;ROLE=CHAIR:/aB1/principal/\r\n\
ATTENDEE;CN=Bob Guest;CUTYPE=INDIVIDUAL;EMAIL=bob@example.net;SCHEDULE-STATUS=\r\n \
1.1:mailto:bob@example.net\r\n\
DTEND;TZID=Europe/Berlin:20261109T163000\r\n\
DTSTAMP:20260919T184800Z\r\n\
DTSTART;TZID=Europe/Berlin:20261109T160000\r\n\
ORGANIZER;CN=Toni Barth;EMAIL=toni@example.org:/aB1/principal/\r\n\
SEQUENCE:0\r\n\
SUMMARY:Aperio R6 eigene\r\n\
UID:EBFB90D1-F3C9-4E8C-B18E-211595E15C22\r\n\
X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Reminder\r\n\
TRIGGER:-PT15M\r\n\
UID:5D446DBE-3179-48F1-AC45-96C95B46851B\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

/// An invitation as iCloud stores it on the attendee's side: someone
/// else's ORGANIZER, the account's own row spelled as a principal path
/// with its EMAIL, a zone, Apple's own properties, and alarms of every
/// kind the read shows — a display alarm carrying Apple's default mark,
/// an audio alarm, an e-mail alarm with an ATTENDEE line of its own, one
/// triggered from the end, and one whose trigger Aperio does not model.
/// A weekly series with one changed occurrence.
pub const ICLOUD_INVITATION: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Apple Inc.//iPhone OS 26.0//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Europe/Berlin\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
ATTENDEE;CN=Boss;CUTYPE=INDIVIDUAL;EMAIL=boss@example.net;PARTSTAT=ACCEPTED;RO\r\n \
LE=CHAIR:mailto:boss@example.net\r\n\
ATTENDEE;CN=Toni Barth;CUTYPE=INDIVIDUAL;EMAIL=toni@example.org;PARTSTAT=NEEDS\r\n \
-ACTION;ROLE=REQ-PARTICIPANT:/aB1/principal/\r\n\
CREATED:20260901T090000Z\r\n\
DTEND;TZID=Europe/Berlin:20261109T163000\r\n\
DTSTAMP:20260919T184800Z\r\n\
DTSTART;TZID=Europe/Berlin:20261109T160000\r\n\
LAST-MODIFIED:20260918T101500Z\r\n\
ORGANIZER;CN=Boss;EMAIL=boss@example.net:mailto:boss@example.net\r\n\
RRULE:FREQ=WEEKLY;COUNT=4\r\n\
SEQUENCE:2\r\n\
STATUS:CONFIRMED\r\n\
SUMMARY:Aperio R6 fremde\r\n\
TRANSP:OPAQUE\r\n\
UID:9C1F6A4E-0B77-4E1E-9F6E-51D2A0C9B7A1\r\n\
X-APPLE-DEFAULT-ALARM:TRUE\r\n\
X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Aperio R6 fremde\r\n\
TRIGGER:-PT15M\r\n\
UID:11111111-1111-4111-8111-111111111111\r\n\
X-APPLE-DEFAULT-ALARM:TRUE\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:AUDIO\r\n\
ATTACH;VALUE=URI:Basso\r\n\
TRIGGER:-PT30M\r\n\
UID:22222222-2222-4222-8222-222222222222\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:EMAIL\r\n\
ATTENDEE:mailto:toni@example.org\r\n\
DESCRIPTION:By mail\r\n\
SUMMARY:By mail\r\n\
TRIGGER:-P1D\r\n\
UID:33333333-3333-4333-8333-333333333333\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:After the end\r\n\
TRIGGER;RELATED=END:-PT5M\r\n\
UID:44444444-4444-4444-8444-444444444444\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:A week before\r\n\
TRIGGER:-P1W\r\n\
UID:55555555-5555-4555-8555-555555555555\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
ATTENDEE;CN=Toni Barth;EMAIL=toni@example.org;PARTSTAT=NEEDS-ACTION:/aB1/princ\r\n \
ipal/\r\n\
DTEND;TZID=Europe/Berlin:20261116T173000\r\n\
DTSTAMP:20260919T184800Z\r\n\
DTSTART;TZID=Europe/Berlin:20261116T170000\r\n\
ORGANIZER;CN=Boss;EMAIL=boss@example.net:mailto:boss@example.net\r\n\
RECURRENCE-ID;TZID=Europe/Berlin:20261116T160000\r\n\
SEQUENCE:3\r\n\
SUMMARY:Aperio R6 fremde, verschoben\r\n\
UID:9C1F6A4E-0B77-4E1E-9F6E-51D2A0C9B7A1\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Aperio R6 fremde\r\n\
TRIGGER:-PT15M\r\n\
UID:66666666-6666-4666-8666-666666666666\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

/// The same invitation as an all-day series, the way a provider writes
/// one: `DTSTART;VALUE=DATE`.
pub const ICLOUD_ALL_DAY_INVITATION: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
ATTENDEE;CN=Toni Barth;EMAIL=toni@example.org;PARTSTAT=ACCEPTED:/aB1/principal\r\n \
/\r\n\
DTEND;VALUE=DATE:20261110\r\n\
DTSTAMP:20260919T184800Z\r\n\
DTSTART;VALUE=DATE:20261109\r\n\
ORGANIZER;CN=Boss;EMAIL=boss@example.net:mailto:boss@example.net\r\n\
RRULE:FREQ=WEEKLY;COUNT=4\r\n\
SUMMARY:Ganztags fremd\r\n\
UID:9C1F6A4E-0B77-4E1E-9F6E-51D2A0C9B7A1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

/// The account's addresses as iCloud lists them (M1, anonymised).
pub fn icloud_identity() -> OwnIdentity {
    OwnIdentity::from_hrefs(
        &[
            "/1974/principal".into(),
            "/aB1/principal/".into(),
            "urn:uuid:1974".into(),
            "mailto:toni@example.org".into(),
        ],
        &url::Url::parse("https://p42-caldav.icloud.com/1974/principal/").unwrap(),
    )
}
