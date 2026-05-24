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
//!  - `PHOTO` (Phase 10g): inline base64 bodies in both the vCard
//!    3.0 (`PHOTO;ENCODING=b;TYPE=JPEG:<b64>`) and vCard 4.0
//!    (`PHOTO:data:image/jpeg;base64,<b64>`) shapes. URI-only
//!    PHOTOs (`PHOTO:http://…`) flip `has_photo` to `true` so the
//!    flag stays honest, but `parse_vcard_photo` returns `None`
//!    for them — Aperio doesn't fetch remote-URL avatars on the
//!    user's behalf.
//!
//! Not covered (intentionally; can grow when a real use-case shows up):
//!
//!  - Logo / sound binary blobs.
//!  - Categories, other X-* extensions.
//!  - Addresses (`ADR`) — planned for Phase 10h.
//!  - Property parameters beyond `TYPE`, `CN`, `ENCODING`, `VALUE`
//!    — we round-trip the value without trying to preserve
//!    `LANGUAGE`, `LABEL`, etc.
//!
//! Tolerance: line folding (continuation lines starting with space
//! / tab) and the three vCard escape sequences (`\\`, `\,`, `\;`,
//! `\n`) are honoured both directions.

use base64::Engine;
use cal_core::{Contact, ContactPhoto, GroupMember, NewContact};
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
    // PHOTO presence flag. We don't carry the bytes through the
    // `Contact` shape (avatars travel via `get_contact_photo`
    // instead), but we do need to remember whether the vCard had
    // one so the listing exposes the right `has_photo` value. Any
    // non-empty PHOTO property — inline base64 or URI — sets this
    // to true; the byte-extraction path (`parse_vcard_photo`)
    // applies stricter rules.
    let mut has_photo = false;
    // Group / distribution-list state. vCard 4.0 signals a group via
    // `KIND:group` + `MEMBER:mailto:foo@example.com`. Apple's
    // CardDAV servers (and older clients still on 3.0) ship the
    // same data under `X-ADDRESSBOOKSERVER-KIND` /
    // `X-ADDRESSBOOKSERVER-MEMBER` — we accept either and emit
    // both on write so round-trips survive between server kinds.
    let mut is_group = false;
    let mut members: Vec<GroupMember> = Vec::new();
    // Postal addresses (Phase 10l). vCard ADR has 7 semicolon-
    // separated components: po-box; extended; street; locality;
    // region; postal-code; country-name. We fold po-box +
    // extended into the street line so the cal-core model stays
    // one-string-per-field. Multiple ADR properties become
    // multiple `ContactAddress` entries.
    let mut addresses: Vec<cal_core::ContactAddress> = Vec::new();

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
            "PHOTO" => {
                // Any non-empty PHOTO body flips the listing flag.
                // We don't decode here; the actual bytes are
                // extracted on-demand by `parse_vcard_photo` so a
                // 1000-contact PROPFIND doesn't decode a megabyte
                // of base64 the user might never look at.
                if !value.trim().is_empty() {
                    has_photo = true;
                }
            }
            "ADR" => {
                // Seven components per RFC 6350 §6.3.1:
                //   0: po-box, 1: extended-address, 2: street,
                //   3: locality, 4: region, 5: postal-code,
                //   6: country-name.
                // We collapse po-box + extended + street into a
                // single street line because Aperio's UI surfaces
                // one multi-line text field per address — splitting
                // them out would force a confusing per-line decision
                // on the user.
                let parts = split_structured(value);
                let pobox = parts.first().map(String::as_str).unwrap_or("");
                let extended =
                    parts.get(1).map(String::as_str).unwrap_or("");
                let street = parts.get(2).map(String::as_str).unwrap_or("");
                let combined_street = [pobox, extended, street]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                let address = cal_core::ContactAddress {
                    label: extract_type_param(head).map(normalise_address_label),
                    street: Some(combined_street).filter(|s| !s.is_empty()),
                    city: parts.get(3).cloned().filter(|s| !s.is_empty()),
                    region: parts.get(4).cloned().filter(|s| !s.is_empty()),
                    postal_code: parts.get(5).cloned().filter(|s| !s.is_empty()),
                    country: parts.get(6).cloned().filter(|s| !s.is_empty()),
                };
                // Skip ADR lines that are all-empty — some vCards
                // emit `ADR:;;;;;;` as a placeholder; pulling those
                // into the model would clutter the UI with blank
                // rows.
                if address.street.is_some()
                    || address.city.is_some()
                    || address.region.is_some()
                    || address.postal_code.is_some()
                    || address.country.is_some()
                {
                    addresses.push(address);
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
        addresses,
        members: if is_group { Some(members) } else { None },
        has_photo,
        created_at: now,
        updated_at: updated_at.unwrap_or(now),
        etag,
    })
}

/// Pull the `TYPE` parameter off a property head like
/// `ADR;TYPE=home`. Returns the first TYPE value (vCard 4.0 allows
/// comma-separated lists like `TYPE=home,pref`; we keep "home" and
/// drop the modifiers). Case-insensitive on the parameter name —
/// the parameter value itself is preserved as-is so an unusual
/// custom type (`TYPE=Postfach`) round-trips back out unchanged.
fn extract_type_param(head: &str) -> Option<String> {
    for part in head.split(';').skip(1) {
        let (k, v) = part.split_once('=')?;
        if k.eq_ignore_ascii_case("TYPE") {
            // vCard 4.0 multi-value TYPE: take the first entry
            // (the "primary" semantic; the rest are usually
            // modifiers like "pref" we don't act on).
            return v.split(',').next().map(|s| s.trim().to_string());
        }
    }
    None
}

/// Fold a vCard TYPE value onto one of the canonical labels
/// (`"home"` / `"work"` / `"other"`) so the round-trip across
/// every adapter agrees on the slot. Unknown values pass through
/// lower-cased — adapters that need a slot pick "other" for them.
fn normalise_address_label(raw: String) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "home" => "home".into(),
        "work" | "business" => "work".into(),
        "other" => "other".into(),
        // Pass through anything else lowercased so we don't drop
        // user-defined types but also don't introduce case noise.
        _ => raw.to_ascii_lowercase(),
    }
}

/// Extract the inline photo from a vCard body.
///
/// Returns `Some(ContactPhoto)` if the body carries a base64-encoded
/// PHOTO property in either of the two shapes Aperio handles:
///
///   - vCard 3.0: `PHOTO;ENCODING=b;TYPE=JPEG:<base64>` (TYPE may
///     be JPEG / PNG / GIF; the MIME type is inferred from it).
///   - vCard 4.0: `PHOTO:data:image/jpeg;base64,<base64>` (the
///     URI's media-type prefix gives us the MIME directly).
///
/// Returns `None` when the vCard has no PHOTO, when the PHOTO is a
/// remote URL (we don't fetch URL avatars on the user's behalf),
/// or when the base64 doesn't decode. The CardDAV `get_contact_photo`
/// path treats `None` as "no avatar" — the frontend renders the
/// initials placeholder.
pub fn parse_vcard_photo(raw: &str) -> Option<ContactPhoto> {
    let unfolded = unfold(raw);
    for line in unfolded.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((head, value)) = split_property(line) else {
            continue;
        };
        let name = head.split(';').next().unwrap_or("").to_ascii_uppercase();
        if name != "PHOTO" {
            continue;
        }
        if let Some(photo) = decode_inline_photo(head, value) {
            return Some(photo);
        }
    }
    None
}

/// Decode a single PHOTO property line into a `ContactPhoto`,
/// rejecting URI-only photos and unrecognised encodings. Shared
/// by `parse_vcard_photo`.
fn decode_inline_photo(head: &str, value: &str) -> Option<ContactPhoto> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    // vCard 4.0 data URI: `PHOTO:data:image/jpeg;base64,<b64>` —
    // no parameters on the head, the encoding hint lives inside
    // the value itself.
    if let Some(rest) = value.strip_prefix("data:").or_else(|| value.strip_prefix("DATA:")) {
        let mut split = rest.splitn(2, ',');
        let header = split.next()?;
        let body = split.next()?;
        // header looks like `image/jpeg;base64`. Pull out the mime
        // up to the first `;` and verify the encoding is base64.
        let (mime, params) = header.split_once(';').unwrap_or((header, ""));
        if !params
            .split(';')
            .any(|p| p.trim().eq_ignore_ascii_case("base64"))
        {
            return None;
        }
        let cleaned: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(cleaned.as_bytes())
            .ok()?;
        if bytes.is_empty() {
            return None;
        }
        return Some(ContactPhoto {
            content_type: mime.trim().to_string(),
            data: bytes,
        });
    }

    // vCard 3.0 base64 form: `PHOTO;ENCODING=b;TYPE=JPEG:<b64>`.
    // The encoding parameter is mandatory in this branch — without
    // it we can't tell base64 bytes apart from a URI, so we abstain.
    let params: Vec<(&str, &str)> = head
        .split(';')
        .skip(1)
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    let encoded = params
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("ENCODING") && (*v == "b" || v.eq_ignore_ascii_case("BASE64")));
    if !encoded {
        // PHOTO without ENCODING and not a data: URI ⇒ this is a
        // bare URL like `PHOTO:http://example.org/photo.jpg`. We
        // don't fetch external avatars, so treat it as "no photo
        // we can return".
        return None;
    }
    let type_param = params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("TYPE"))
        .map(|(_, v)| {
            // Strip surrounding quotes vCard params allow on
            // multi-valued TYPE entries (`TYPE="JPEG,PHOTO"` shows
            // up in some Apple exports).
            let trimmed = v.trim_matches('"');
            trimmed
                .split(',')
                .next()
                .unwrap_or(trimmed)
                .to_ascii_uppercase()
        });
    let content_type = match type_param.as_deref() {
        Some("JPEG") | Some("JPG") | None => "image/jpeg",
        Some("PNG") => "image/png",
        Some("GIF") => "image/gif",
        Some("BMP") => "image/bmp",
        Some("WEBP") => "image/webp",
        Some(other) => {
            // Pass an already-shaped MIME (`image/jpeg`) through
            // unchanged; otherwise default to JPEG so the frontend
            // can still render the bytes.
            if other.starts_with("IMAGE/") {
                return Some(ContactPhoto {
                    content_type: other.to_ascii_lowercase(),
                    data: decode_b64_stripped(value)?,
                });
            }
            "image/jpeg"
        }
    };
    let bytes = decode_b64_stripped(value)?;
    if bytes.is_empty() {
        return None;
    }
    Some(ContactPhoto {
        content_type: content_type.to_string(),
        data: bytes,
    })
}

/// Strip whitespace (line-folding leftovers, indentation) before
/// base64-decoding. The `unfold` pass joins continuation lines
/// without trimming embedded whitespace, so we have to do it here.
fn decode_b64_stripped(value: &str) -> Option<Vec<u8>> {
    let cleaned: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .ok()
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
    // Postal addresses (Phase 10l). One ADR property per address.
    // We pack the (possibly multi-line) street into the third
    // component, leaving po-box (0) and extended-address (1) empty
    // — vCard parsers treat consecutive `;;` as "absent" so this
    // matches what every reasonable server emits. Label rides on
    // the `TYPE=` parameter; unrecognised labels still emit as a
    // TYPE so the round-trip preserves user intent.
    for address in &contact.addresses {
        if address.street.is_none()
            && address.city.is_none()
            && address.region.is_none()
            && address.postal_code.is_none()
            && address.country.is_none()
        {
            continue;
        }
        let type_param = address
            .label
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(";TYPE={}", escape_param(s)))
            .unwrap_or_default();
        // Street can hold an embedded newline from the parse-side
        // fold of po-box / extended into street. vCard escapes `\n`
        // as `\\n` (literal backslash-n) — the `escape` helper
        // already does that.
        out.push_str(&format!(
            "ADR{type_param}:;;{};{};{};{};{}\r\n",
            escape(address.street.as_deref().unwrap_or("")),
            escape(address.city.as_deref().unwrap_or("")),
            escape(address.region.as_deref().unwrap_or("")),
            escape(address.postal_code.as_deref().unwrap_or("")),
            escape(address.country.as_deref().unwrap_or("")),
        ));
    }
    // PHOTO (Phase 10g): emit the vCard 3.0 base64 form because
    // every CardDAV server in the wild — iCloud, Nextcloud,
    // Radicale, Baikal, Fastmail — round-trips it cleanly. The
    // data: URI variant requires the server to be on vCard 4.0,
    // which isn't a safe assumption. TYPE is derived from the
    // MIME so Apple Contacts' UI picks the right thumbnail
    // shape; long base64 lines are folded at 75 chars to stay
    // inside RFC 6350 §3.2 even though most servers tolerate
    // longer lines.
    if let Some(photo) = contact.photo.as_ref() {
        if !photo.data.is_empty() {
            push_vcard_photo(&mut out, photo);
        }
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
///
/// `preserved_photo` is the photo body we want to re-emit on the
/// updated resource. The Contact struct doesn't carry the bytes
/// (the listing flag is enough for every other code path), so
/// the CardDAV `update_contact` does a quick GET + parse to
/// recover them when `contact.has_photo` is true — passing
/// `None` here on an update of a contact that has a photo would
/// silently wipe the avatar on the next PUT.
pub fn rebuild_vcard(
    uid: &str,
    contact: &Contact,
    preserved_photo: Option<ContactPhoto>,
) -> String {
    // Reuse the create builder; the only delta would be REV
    // (rebuilt unconditionally above) and we don't try to preserve
    // properties we don't model (categories, X-*, etc.). The photo
    // travels through `NewContact.photo` so the existing emitter
    // path handles it without a second branch.
    let payload = NewContact {
        display_name: contact.display_name.clone(),
        given_name: contact.given_name.clone(),
        family_name: contact.family_name.clone(),
        organization: contact.organization.clone(),
        emails: contact.emails.clone(),
        phone_numbers: contact.phone_numbers.clone(),
        birthday: contact.birthday,
        notes: contact.notes.clone(),
        addresses: contact.addresses.clone(),
        members: contact.members.clone(),
        photo: preserved_photo,
    };
    build_vcard(uid, &payload)
}

/// Emit a `PHOTO` line in the vCard 3.0 base64 form, line-folded
/// per RFC 6350 §3.2 so older parsers don't reject the entry as
/// too-long. The MIME type is mapped onto the `TYPE=` parameter
/// using the inverse of the lookup `decode_inline_photo` uses;
/// unknown MIMEs fall back to `JPEG` because that's what every
/// CardDAV reference impl treats as the safe default.
fn push_vcard_photo(out: &mut String, photo: &ContactPhoto) {
    let mime_lower = photo.content_type.to_ascii_lowercase();
    let type_param = match mime_lower.as_str() {
        "image/jpeg" | "image/jpg" => "JPEG",
        "image/png" => "PNG",
        "image/gif" => "GIF",
        "image/bmp" => "BMP",
        "image/webp" => "WEBP",
        _ => "JPEG",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&photo.data);
    // RFC 6350 §3.2: lines longer than 75 octets SHOULD be folded.
    // We fold by writing the prefix, then a CRLF + single space
    // (the continuation marker) every 72 chars of base64 so the
    // total line length stays well under the limit.
    let header = format!("PHOTO;ENCODING=b;TYPE={type_param}:");
    out.push_str(&header);
    let mut written = header.len();
    for chunk in b64.as_bytes().chunks(72) {
        let s = std::str::from_utf8(chunk).expect("base64 is ASCII");
        if written + s.len() > 75 {
            out.push_str("\r\n ");
            written = 1;
        }
        out.push_str(s);
        written += s.len();
    }
    out.push_str("\r\n");
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
            addresses: Vec::new(),
            members: None,
            photo: None,
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
        // A URI-shaped PHOTO still flips the listing flag — the
        // server claims this row has an avatar — even though
        // `parse_vcard_photo` won't fetch the remote URL on the
        // user's behalf. The frontend collapses to the initials
        // placeholder when the photo fetch returns None.
        assert!(parsed.has_photo);
        assert!(parse_vcard_photo(body).is_none());
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
            addresses: Vec::new(),
            members: None,
            photo: None,
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
            addresses: Vec::new(),
            members: None,
            photo: None,
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
            has_photo: false,
            addresses: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: Some("etag-1".into()),
        };
        let body = rebuild_vcard("uid-2", &original, None);
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

    /// Sample PNG bytes shared across the PHOTO tests. Real PNG
    /// (signature + IHDR + IDAT + IEND) so the round-trip exercises
    /// the same decoder path a server would feed us.
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
        0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
        0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
        0x9c, 0x63, 0xfa, 0xcf, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xe5, 0x27, 0xde, 0xfc,
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn build_and_parse_inline_photo_round_trip() {
        let photo = ContactPhoto {
            content_type: "image/png".into(),
            data: PNG_1X1.to_vec(),
        };
        let nc = NewContact {
            display_name: "Pic Person".into(),
            given_name: None,
            family_name: None,
            organization: None,
            emails: Vec::new(),
            phone_numbers: Vec::new(),
            birthday: None,
            notes: None,
            addresses: Vec::new(),
            members: None,
            photo: Some(photo.clone()),
        };
        let body = build_vcard("uid-photo", &nc);
        // PHOTO header is present in the vCard 3.0 base64 form.
        assert!(body.contains("PHOTO;ENCODING=b;TYPE=PNG:"));
        // Listing flag flips on, bytes round-trip.
        let parsed = parse(&body);
        assert!(parsed.has_photo);
        let extracted = parse_vcard_photo(&body).expect("inline photo decodes");
        assert_eq!(extracted.content_type, photo.content_type);
        assert_eq!(extracted.data, photo.data);
    }

    #[test]
    fn parses_vcard_4_data_uri_photo() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(PNG_1X1);
        let body = format!(
            "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Test\r\nPHOTO:data:image/png;base64,{b64}\r\nEND:VCARD\r\n",
        );
        let parsed = parse(&body);
        assert!(parsed.has_photo);
        let extracted = parse_vcard_photo(&body).expect("data URI decodes");
        assert_eq!(extracted.content_type, "image/png");
        assert_eq!(extracted.data, PNG_1X1.to_vec());
    }

    #[test]
    fn folded_photo_lines_reassemble_before_decode() {
        // Force the builder to fold (PNG_1X1 is ~75 bytes base64).
        let photo = ContactPhoto {
            content_type: "image/png".into(),
            data: PNG_1X1.to_vec(),
        };
        let mut s = String::new();
        push_vcard_photo(&mut s, &photo);
        // The emitter MUST fold long base64 onto continuation
        // lines — confirm we actually produced one so this test
        // covers the fold-handling path through `unfold`.
        assert!(s.contains("\r\n "));
        let body =
            format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:T\r\n{s}END:VCARD\r\n");
        let extracted = parse_vcard_photo(&body).expect("folded photo decodes");
        assert_eq!(extracted.data, PNG_1X1.to_vec());
    }

    #[test]
    fn rebuild_carries_preserved_photo_back_into_vcard() {
        let contact = Contact {
            id: "id".into(),
            list_id: "list".into(),
            display_name: "Has Photo".into(),
            given_name: None,
            family_name: None,
            organization: None,
            emails: Vec::new(),
            phone_numbers: Vec::new(),
            birthday: None,
            notes: None,
            members: None,
            has_photo: true,
            addresses: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
        };
        let photo = ContactPhoto {
            content_type: "image/png".into(),
            data: PNG_1X1.to_vec(),
        };
        let body = rebuild_vcard("uid", &contact, Some(photo.clone()));
        let reparsed = parse(&body);
        assert!(reparsed.has_photo);
        let extracted = parse_vcard_photo(&body).expect("photo present");
        assert_eq!(extracted.data, photo.data);
    }

    #[test]
    fn parse_vcard_photo_rejects_bare_url() {
        let body =
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:T\r\nPHOTO:http://example.org/p.jpg\r\nEND:VCARD\r\n";
        // has_photo flips on (it's a real PHOTO line) but we
        // refuse to chase the URL — `parse_vcard_photo` returns
        // None so the caller knows there's no avatar to display.
        let parsed = parse(body);
        assert!(parsed.has_photo);
        assert!(parse_vcard_photo(body).is_none());
    }
}
