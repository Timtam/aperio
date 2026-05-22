//! Minimal vCard 3.0 / 4.0 parser and serialiser.
//!
//! Hand-rolled — the well-maintained Rust vcard libraries either
//! drag in a kitchen-sink of dependencies (icalendar-style heavy
//! crates) or stop at 3.0. Aperio only needs the eight properties
//! the `cal_core::Contact` model exposes, and the file format is
//! simple enough that doing it ourselves is shorter than wiring up
//! a third-party parser and translating its types.
//!
//! Covered:
//!
//!  - `BEGIN:VCARD` / `END:VCARD` wrapper, `VERSION` (3.0 on write,
//!    accepts 2.1 / 3.0 / 4.0 on read).
//!  - `FN` → `display_name`. Required by the spec; if absent the
//!    parser falls back to the assembled `N` components.
//!  - `N` (structured `family;given;additional;prefix;suffix`) →
//!    `family_name`, `given_name`.
//!  - `ORG` (first component) → `organization`.
//!  - `EMAIL` (multi-valued) → `emails`.
//!  - `TEL` (multi-valued) → `phone_numbers`.
//!  - `BDAY` (ISO 8601 date or vCard `YYYYMMDD`) → `birthday`.
//!  - `NOTE` → `notes`.
//!  - `UID` → reused as the resource UID on write; parsed but the
//!    caller does its own id juggling.
//!  - `REV` → reused as `updated_at` when present.
//!  - `KIND` / `X-ADDRESSBOOKSERVER-KIND` (Phase 10f) → group flag.
//!  - `MEMBER` / `X-ADDRESSBOOKSERVER-MEMBER` (Phase 10f) → the
//!    distribution-list member list, both vCard 4.0 spec form and
//!    Apple CardDAV variant.
//!
//! Not covered (intentionally; can grow when a real use-case shows up):
//!
//!  - Photo / logo / sound binary blobs (planned for Phase 10g).
//!  - Categories, other X-* extensions.
//!  - Addresses (`ADR`) — planned for Phase 10h.
//!  - Property parameters beyond `TYPE` and `CN` — we round-trip
//!    the value without trying to preserve `LANGUAGE`, `LABEL`, etc.
//!
//! Tolerance: line folding (continuation lines starting with space
//! / tab) and the three vCard escape sequences (`\\`, `\,`, `\;`,
//! `\n`) are honoured both directions.

use cal_core::{Contact, GroupMember, NewContact};
use chrono::{DateTime, NaiveDate, Utc};

use crate::error::{CaldavError, CaldavResult};

/// Parse a vCard text body into a `Contact`. The caller supplies
/// `list_id` so the returned struct can carry its container — the
/// vCard format itself has no concept of an address book.
///
/// `id` is the URL the server gave us for this resource (encoded
/// the same way as task ids — `{href}|{uid}` — so a follow-up
/// PUT or DELETE can reach the same path). Callers that don't
/// have the href can pass the bare UID and accept that updates
/// will round-trip back through the `{list}/{uid}.vcf` fallback.
pub fn parse_vcard(
    raw: &str,
    list_id: &str,
    id: String,
    etag: Option<String>,
) -> CaldavResult<Contact> {
    let unfolded = unfold(raw);
    let mut display_name: Option<String> = None;
    let mut family_name: Option<String> = None;
    let mut given_name: Option<String> = None;
    let mut organization: Option<String> = None;
    let mut emails: Vec<String> = Vec::new();
    let mut phone_numbers: Vec<String> = Vec::new();
    let mut birthday: Option<NaiveDate> = None;
    let mut notes: Option<String> = None;
    let mut updated_at: Option<DateTime<Utc>> = None;
    let mut saw_vcard = false;
    // Group / distribution-list state. vCard 4.0 signals a group via
    // `KIND:group` + `MEMBER:mailto:foo@example.com`. Apple's
    // CardDAV servers (and older clients still on 3.0) ship the
    // same data under `X-ADDRESSBOOKSERVER-KIND` /
    // `X-ADDRESSBOOKSERVER-MEMBER` — we accept either and emit
    // both on write so round-trips survive between server kinds.
    let mut is_group = false;
    let mut members: Vec<GroupMember> = Vec::new();

    for raw_line in unfolded.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        // `BEGIN:VCARD` / `END:VCARD` envelopes are checked
        // case-insensitively per the spec.
        if line.eq_ignore_ascii_case("BEGIN:VCARD") {
            saw_vcard = true;
            continue;
        }
        if line.eq_ignore_ascii_case("END:VCARD") {
            break;
        }
        let Some((head, value)) = split_property(line) else {
            continue;
        };
        let name = head.split(';').next().unwrap_or("").to_ascii_uppercase();
        match name.as_str() {
            "VERSION" => { /* discard */ }
            "FN" => display_name = Some(unescape(value)),
            "N" => {
                // `family;given;additional;prefix;suffix`. We only
                // pull the first two components — Aperio's model
                // doesn't carry additional / prefix / suffix.
                let parts = split_structured(value);
                if let Some(p) = parts.first() {
                    if !p.is_empty() {
                        family_name = Some(p.clone());
                    }
                }
                if let Some(p) = parts.get(1) {
                    if !p.is_empty() {
                        given_name = Some(p.clone());
                    }
                }
            }
            "ORG" => {
                // First semicolon-separated component is the org;
                // the rest are units within it. Aperio shows just
                // the org name today.
                let parts = split_structured(value);
                if let Some(p) = parts.first() {
                    if !p.is_empty() {
                        organization = Some(p.clone());
                    }
                }
            }
            "EMAIL" => {
                let trimmed = unescape(value);
                if !trimmed.is_empty() {
                    emails.push(trimmed);
                }
            }
            "TEL" => {
                let trimmed = unescape(value);
                if !trimmed.is_empty() {
                    phone_numbers.push(trimmed);
                }
            }
            "BDAY" => {
                birthday = parse_bday(value);
            }
            "NOTE" => notes = Some(unescape(value)),
            "REV" => {
                updated_at = DateTime::parse_from_rfc3339(value)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
                    .or_else(|| {
                        // vCard 3.0 sometimes emits `20240519T120000Z`
                        // — try the compact form too.
                        chrono::NaiveDateTime::parse_from_str(
                            value,
                            "%Y%m%dT%H%M%SZ",
                        )
                        .ok()
                        .map(|n| Utc.from_utc_datetime(&n))
                    });
            }
            "UID" | "PRODID" => { /* discard — adapter owns the id mapping */ }
            "KIND" | "X-ADDRESSBOOKSERVER-KIND" => {
                // The only `KIND` value Aperio acts on is `group`.
                // vCard 4.0 also defines `individual`, `org`,
                // `location`, `application` — none of which change
                // our wire mapping today, so we treat anything
                // non-group as a regular contact.
                if value.trim().eq_ignore_ascii_case("group") {
                    is_group = true;
                }
            }
            "MEMBER" | "X-ADDRESSBOOKSERVER-MEMBER" => {
                // Spec form: `MEMBER;CN=Alice:mailto:alice@example.com`.
                // We extract the email (after `mailto:`) and the
                // optional CN parameter from the head. urn:uuid:
                // references — used when a server links groups by
                // their underlying contact UID — are accepted and
                // surfaced as members with no resolvable email; we
                // skip those because the picker needs an email.
                let trimmed = unescape(value);
                let email = trimmed
                    .strip_prefix("mailto:")
                    .or_else(|| trimmed.strip_prefix("MAILTO:"))
                    .map(|s| s.to_string());
                if let Some(email) = email.filter(|s| !s.is_empty()) {
                    // CN parameter lives on the head as
                    // `MEMBER;CN=Alice` (case-insensitive). Other
                    // parameters (e.g. `MEMBER;TYPE=…`) are not
                    // semantically used by groups, so we just look
                    // for CN.
                    let name = extract_cn_param(head);
                    members.push(GroupMember { name, email });
                }
            }
            _ => { /* unknown / unsupported property; skip silently */ }
        }
    }

    if !saw_vcard {
        return Err(CaldavError::Protocol(
            "vCard body missing BEGIN:VCARD".into(),
        ));
    }

    // `FN` is required by the spec but we defend in depth: fall
    // back to assembling `N` components, then to a literal
    // placeholder. The Contact model needs a non-empty
    // display_name to render.
    let display_name = display_name
        .filter(|s| !s.trim().is_empty())
        .or_else(|| assemble_display_name(given_name.as_deref(), family_name.as_deref()))
        .unwrap_or_else(|| "Unnamed contact".to_string());

    let now = Utc::now();
    Ok(Contact {
        id,
        list_id: list_id.to_string(),
        display_name,
        given_name,
        family_name,
        organization,
        emails,
        phone_numbers,
        birthday,
        notes,
        members: if is_group { Some(members) } else { None },
        created_at: now,
        updated_at: updated_at.unwrap_or(now),
        etag,
    })
}

/// Extract the `CN=…` parameter from a vCard property head like
/// `MEMBER;CN=Alice` or `MEMBER;TYPE=work;CN="Alice Doe"`. Returns
/// `None` if no CN parameter is present. Quoted values get their
/// surrounding double quotes stripped — vCard params may quote
/// values that contain commas or semicolons.
fn extract_cn_param(head: &str) -> Option<String> {
    for part in head.split(';').skip(1) {
        let (key, val) = part.split_once('=')?;
        if key.trim().eq_ignore_ascii_case("CN") {
            let v = val.trim();
            let stripped = v.strip_prefix('"').and_then(|s| s.strip_suffix('"'));
            return Some(stripped.unwrap_or(v).to_string()).filter(|s| !s.is_empty());
        }
    }
    None
}

/// Build a vCard 3.0 body for a new contact. The UID is supplied so
/// the caller can use the same value as the resource filename. We
/// emit 3.0 (not 4.0) because every major CardDAV server — iCloud,
/// Nextcloud, Radicale, Baikal, Fastmail — accepts it; 4.0 is
/// patchier in the wild.
pub fn build_vcard(uid: &str, contact: &NewContact) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VCARD\r\n");
    out.push_str("VERSION:3.0\r\n");
    out.push_str("PRODID:-//Aperio//Contacts//EN\r\n");
    out.push_str(&format!("UID:{uid}\r\n"));
    // KIND signals "this row is a distribution list" — emit both
    // the vCard 4.0 form (`KIND:group`) and the Apple
    // CardDAV / vCard 3.0 form (`X-ADDRESSBOOKSERVER-KIND:group`)
    // so a server reading either dialect picks up the group flag.
    // Apple Contacts and Nextcloud both honour the X- variant on
    // 3.0; vCard 4.0 clients honour `KIND`.
    if contact.members.is_some() {
        out.push_str("KIND:group\r\n");
        out.push_str("X-ADDRESSBOOKSERVER-KIND:group\r\n");
    }
    out.push_str(&format!("FN:{}\r\n", escape(&contact.display_name)));
    // N is always present, even when one half is blank — clients
    // (Apple Contacts, Nextcloud's UI) sort by N if FN is missing.
    out.push_str(&format!(
        "N:{};{};;;\r\n",
        escape(contact.family_name.as_deref().unwrap_or("")),
        escape(contact.given_name.as_deref().unwrap_or(""))
    ));
    if let Some(org) = contact.organization.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!("ORG:{}\r\n", escape(org)));
    }
    for email in &contact.emails {
        if !email.is_empty() {
            // TYPE=INTERNET is the historical baseline; some
            // servers (older Radicale) reject EMAIL without a TYPE.
            out.push_str(&format!(
                "EMAIL;TYPE=INTERNET:{}\r\n",
                escape(email)
            ));
        }
    }
    for phone in &contact.phone_numbers {
        if !phone.is_empty() {
            out.push_str(&format!("TEL:{}\r\n", escape(phone)));
        }
    }
    if let Some(bday) = contact.birthday {
        out.push_str(&format!("BDAY:{}\r\n", bday.format("%Y-%m-%d")));
    }
    if let Some(note) = contact.notes.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!("NOTE:{}\r\n", escape(note)));
    }
    // Group members: vCard 4.0 uses MEMBER (URI value), Apple
    // CardDAV uses X-ADDRESSBOOKSERVER-MEMBER. Emit both so each
    // server kind can read its native form. CN holds the optional
    // display name so the picker round-trips human-readable
    // labels rather than collapsing groups to bare email lists.
    if let Some(members) = contact.members.as_ref() {
        for m in members {
            if m.email.is_empty() {
                continue;
            }
            let cn = m
                .name
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!(";CN={}", escape_param(s)))
                .unwrap_or_default();
            out.push_str(&format!(
                "MEMBER{cn}:mailto:{}\r\n",
                escape(&m.email),
            ));
            out.push_str(&format!(
                "X-ADDRESSBOOKSERVER-MEMBER{cn}:mailto:{}\r\n",
                escape(&m.email),
            ));
        }
    }
    out.push_str(&format!(
        "REV:{}\r\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
    out.push_str("END:VCARD\r\n");
    out
}

/// Escape a vCard parameter value for use inside a property head
/// (e.g. `MEMBER;CN=…:value`). Parameters tolerate fewer escapes
/// than property values — semicolons, commas, and colons break the
/// parser, double quotes wrap a value to preserve the unsafe chars.
fn escape_param(s: &str) -> String {
    if s.chars()
        .any(|c| c == ';' || c == ',' || c == ':' || c == '"')
    {
        // Surround in double quotes and replace any inner quotes
        // with a single quote (vCard params don't define a quote
        // escape, so the cleanest fallback is substitution).
        let cleaned = s.replace('"', "'");
        format!("\"{cleaned}\"")
    } else {
        s.to_string()
    }
}

/// Variant of `build_vcard` for an existing `Contact`. Used by the
/// PUT-update path — keeps the contact's UID stable so the
/// resource URL doesn't shift, and emits the same property set as
/// the create version.
pub fn rebuild_vcard(uid: &str, contact: &Contact) -> String {
    // Reuse the create builder; the only delta would be REV
    // (rebuilt unconditionally above) and we don't try to preserve
    // properties we don't model (categories, X-*, etc.).
    let payload = NewContact {
        display_name: contact.display_name.clone(),
        given_name: contact.given_name.clone(),
        family_name: contact.family_name.clone(),
        organization: contact.organization.clone(),
        emails: contact.emails.clone(),
        phone_numbers: contact.phone_numbers.clone(),
        birthday: contact.birthday,
        notes: contact.notes.clone(),
        members: contact.members.clone(),
    };
    build_vcard(uid, &payload)
}

use chrono::TimeZone;

// ── Internal helpers ───────────────────────────────────────────────────

/// Reverse line folding (RFC 6350 §3.2 / RFC 2426 §2.6): a line
/// continuation is signalled by a leading SPACE or TAB on the next
/// line. We join those into the previous logical line.
fn unfold(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut iter = raw.split_inclusive('\n').peekable();
    while let Some(line) = iter.next() {
        // Strip the trailing CR / LF for the join — we'll re-add a
        // single `\n` between logical lines.
        let trimmed = line.trim_end_matches(['\r', '\n']);
        out.push_str(trimmed);
        // Peek: if the next line starts with SP or HT, append it
        // here (after stripping the leading whitespace char) and
        // skip the normal write.
        while let Some(next) = iter.peek() {
            if let Some(rest) = next.strip_prefix(|c: char| c == ' ' || c == '\t') {
                out.push_str(rest.trim_end_matches(['\r', '\n']));
                iter.next();
            } else {
                break;
            }
        }
        out.push('\n');
    }
    out
}

/// Split a property line into the part before the first `:` (name +
/// parameters) and the value after. We DO have to ignore `:` inside
/// quoted parameter values per the grammar, but in practice no
/// commonly-used vCard property uses that — and supporting it
/// would mean shipping a real grammar walker. Skip until needed.
fn split_property(line: &str) -> Option<(&str, &str)> {
    let idx = line.find(':')?;
    Some((&line[..idx], &line[idx + 1..]))
}

/// Split a structured property value on raw (un-escaped) `;`. The
/// vCard escape rules say `\;` produces a literal semicolon that
/// the structure must NOT split on. We do that splitter-step here
/// then unescape each component independently.
fn split_structured(value: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Escape sequence — copy the next character literally.
            match chars.next() {
                Some('n') | Some('N') => current.push('\n'),
                Some(other) => current.push(other),
                None => current.push('\\'),
            }
        } else if c == ';' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    parts.push(current);
    parts
}

/// Unescape a non-structured value: `\\` → `\`, `\,` → `,`, `\;` →
/// `;`, `\n` / `\N` → LF. Anything else after a backslash is
/// passed through verbatim — tolerant against bad input.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Inverse of `unescape`: emit `\,`, `\;`, `\n`, `\\` for the four
/// special characters the format requires.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            // Carriage returns in single-line properties are
            // illegal in vCard; drop them. Real multi-line text
            // (NOTE) keeps its embedded newlines via the \n escape
            // above.
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

/// vCard birthday is either ISO-8601 date (`2026-05-22`) or the
/// compact form (`20260522`). We accept both and ignore time
/// suffixes — Aperio's model is date-only.
fn parse_bday(raw: &str) -> Option<NaiveDate> {
    // Take only the leading date portion; some vCards include time.
    let head = raw
        .split('T')
        .next()
        .unwrap_or("")
        .trim_end_matches(['Z', 'z']);
    NaiveDate::parse_from_str(head, "%Y-%m-%d")
        .ok()
        .or_else(|| NaiveDate::parse_from_str(head, "%Y%m%d").ok())
}

fn assemble_display_name(given: Option<&str>, family: Option<&str>) -> Option<String> {
    match (given, family) {
        (Some(g), Some(f)) if !g.is_empty() && !f.is_empty() => Some(format!("{g} {f}")),
        (Some(g), _) if !g.is_empty() => Some(g.to_string()),
        (_, Some(f)) if !f.is_empty() => Some(f.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Contact {
        parse_vcard(body, "list-x", "id-x".into(), Some("etag-x".into())).unwrap()
    }

    #[test]
    fn round_trips_basic_fields() {
        let nc = NewContact {
            display_name: "Max Mustermann".into(),
            given_name: Some("Max".into()),
            family_name: Some("Mustermann".into()),
            organization: Some("Example GmbH".into()),
            emails: vec![
                "max@example.com".into(),
                "m.muster@example.org".into(),
            ],
            phone_numbers: vec!["+49 30 1234567".into()],
            birthday: Some(NaiveDate::from_ymd_opt(1985, 4, 17).unwrap()),
            notes: Some("Met at conf 2024".into()),
            members: None,
        };
        let body = build_vcard("uid-1", &nc);
        let parsed = parse(&body);
        assert_eq!(parsed.display_name, "Max Mustermann");
        assert_eq!(parsed.given_name.as_deref(), Some("Max"));
        assert_eq!(parsed.family_name.as_deref(), Some("Mustermann"));
        assert_eq!(parsed.organization.as_deref(), Some("Example GmbH"));
        assert_eq!(parsed.emails.len(), 2);
        assert_eq!(parsed.emails[0], "max@example.com");
        assert_eq!(parsed.phone_numbers, vec!["+49 30 1234567".to_string()]);
        assert_eq!(
            parsed.birthday,
            Some(NaiveDate::from_ymd_opt(1985, 4, 17).unwrap()),
        );
        assert_eq!(parsed.notes.as_deref(), Some("Met at conf 2024"));
    }

    #[test]
    fn parses_minimal_vcard() {
        // Just FN — no N, no other props. Real-world vCards from
        // legacy systems look like this.
        let body = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Jane Doe\r\nEND:VCARD\r\n";
        let parsed = parse(body);
        assert_eq!(parsed.display_name, "Jane Doe");
        assert!(parsed.given_name.is_none());
        assert!(parsed.emails.is_empty());
    }

    #[test]
    fn handles_line_folding() {
        // RFC line continuation: the leading space on the next line
        // must be stripped and the lines joined.
        let body = concat!(
            "BEGIN:VCARD\r\n",
            "VERSION:3.0\r\n",
            "FN:Hans Wurst\r\n",
            "NOTE:Eine lange Notiz die\r\n",
            " auf mehrere Zeilen verteilt ist\r\n",
            "END:VCARD\r\n",
        );
        let parsed = parse(body);
        assert_eq!(
            parsed.notes.as_deref(),
            Some("Eine lange Notiz dieauf mehrere Zeilen verteilt ist"),
        );
    }

    #[test]
    fn handles_escapes_in_note() {
        // \, \; \n inside NOTE must come back as the literal chars.
        let body = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Max\r\nNOTE:Line A\\nLine B\\, comma\\; semi\r\nEND:VCARD\r\n";
        let parsed = parse(body);
        assert_eq!(
            parsed.notes.as_deref(),
            Some("Line A\nLine B, comma; semi"),
        );
    }

    #[test]
    fn structured_n_splits_on_unescaped_semicolons() {
        let body = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Anna von Beispiel\r\nN:von Beispiel;Anna;;Dr.;\r\nEND:VCARD\r\n";
        let parsed = parse(body);
        assert_eq!(parsed.family_name.as_deref(), Some("von Beispiel"));
        assert_eq!(parsed.given_name.as_deref(), Some("Anna"));
    }

    #[test]
    fn falls_back_to_n_when_fn_missing() {
        // No FN line — assemble from N parts.
        let body = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Mustermann;Max;;;\r\nEND:VCARD\r\n";
        let parsed = parse(body);
        assert_eq!(parsed.display_name, "Max Mustermann");
    }

    #[test]
    fn falls_back_to_placeholder_when_everything_missing() {
        // No FN, no N — last-ditch fallback so the contact still
        // renders rather than panicking.
        let body = "BEGIN:VCARD\r\nVERSION:3.0\r\nEND:VCARD\r\n";
        let parsed = parse(body);
        assert_eq!(parsed.display_name, "Unnamed contact");
    }

    #[test]
    fn parses_compact_bday() {
        let body = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:T\r\nBDAY:19850417\r\nEND:VCARD\r\n";
        let parsed = parse(body);
        assert_eq!(
            parsed.birthday,
            Some(NaiveDate::from_ymd_opt(1985, 4, 17).unwrap()),
        );
    }

    #[test]
    fn ignores_unknown_properties() {
        let body = concat!(
            "BEGIN:VCARD\r\n",
            "VERSION:4.0\r\n",
            "FN:Test\r\n",
            "X-CUSTOM:wild value\r\n",
            "PHOTO:http://example.org/photo.jpg\r\n",
            "CATEGORIES:friends,work\r\n",
            "END:VCARD\r\n",
        );
        let parsed = parse(body);
        assert_eq!(parsed.display_name, "Test");
    }

    #[test]
    fn parses_email_with_type_parameter() {
        let body = concat!(
            "BEGIN:VCARD\r\n",
            "VERSION:3.0\r\n",
            "FN:T\r\n",
            "EMAIL;TYPE=WORK:work@example.com\r\n",
            "EMAIL;TYPE=HOME,PREF:home@example.com\r\n",
            "END:VCARD\r\n",
        );
        let parsed = parse(body);
        assert_eq!(
            parsed.emails,
            vec!["work@example.com".to_string(), "home@example.com".to_string()],
        );
    }

    #[test]
    fn missing_begin_yields_protocol_error() {
        let body = "VERSION:3.0\r\nFN:Test\r\n";
        let err = parse_vcard(body, "list", "id".into(), None).unwrap_err();
        assert!(matches!(err, CaldavError::Protocol(_)));
    }

    #[test]
    fn build_escapes_commas_and_semicolons() {
        let nc = NewContact {
            display_name: "Smith, Inc.; LTD".into(),
            given_name: None,
            family_name: None,
            organization: None,
            emails: Vec::new(),
            phone_numbers: Vec::new(),
            birthday: None,
            notes: None,
            members: None,
        };
        let body = build_vcard("uid", &nc);
        assert!(body.contains("FN:Smith\\, Inc.\\; LTD"));
        // Round-trip recovers the original.
        let parsed = parse(&body);
        assert_eq!(parsed.display_name, "Smith, Inc.; LTD");
    }

    #[test]
    fn build_escapes_newlines_in_notes() {
        let nc = NewContact {
            display_name: "T".into(),
            given_name: None,
            family_name: None,
            organization: None,
            emails: Vec::new(),
            phone_numbers: Vec::new(),
            birthday: None,
            notes: Some("Line one\nLine two".into()),
            members: None,
        };
        let body = build_vcard("uid", &nc);
        assert!(body.contains("NOTE:Line one\\nLine two"));
        let parsed = parse(&body);
        assert_eq!(parsed.notes.as_deref(), Some("Line one\nLine two"));
    }

    #[test]
    fn rebuild_round_trips_contact() {
        let original = Contact {
            id: "id-x".into(),
            list_id: "list".into(),
            display_name: "Jane Doe".into(),
            given_name: Some("Jane".into()),
            family_name: Some("Doe".into()),
            organization: Some("Beispiel AG".into()),
            emails: vec!["jane@example.com".into()],
            phone_numbers: vec!["+49 170 1234567".into()],
            birthday: Some(NaiveDate::from_ymd_opt(1990, 3, 15).unwrap()),
            notes: None,
            members: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: Some("etag-1".into()),
        };
        let body = rebuild_vcard("uid-2", &original);
        let reparsed = parse(&body);
        assert_eq!(reparsed.display_name, "Jane Doe");
        assert_eq!(reparsed.given_name.as_deref(), Some("Jane"));
        assert_eq!(reparsed.family_name.as_deref(), Some("Doe"));
        assert_eq!(reparsed.organization.as_deref(), Some("Beispiel AG"));
        assert_eq!(reparsed.emails, vec!["jane@example.com".to_string()]);
        assert_eq!(
            reparsed.phone_numbers,
            vec!["+49 170 1234567".to_string()],
        );
        assert_eq!(reparsed.birthday, original.birthday);
    }
}
