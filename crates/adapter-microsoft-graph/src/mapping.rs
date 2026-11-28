//! Microsoft Graph JSON ⇄ cal_core conversion.
//!
//! Reference: <https://learn.microsoft.com/en-us/graph/api/resources/event>.
//!
//! Recurrence is the substantive piece. Graph models recurrence as
//! a structured object (`pattern: { type, interval, daysOfWeek, … }`
//! + `range: { type, endDate, … }`) rather than the RFC 5545 RRULE
//! string the rest of Aperio uses. We translate bidirectionally
//! for the common shapes (daily, weekly+BYDAY, absoluteMonthly,
//! absoluteYearly + COUNT/UNTIL). Relative-monthly / relative-
//! yearly patterns ("last Wednesday of every month") parse on the
//! read side but raise a `GraphError::Protocol` on write — the
//! frontend's EventDialog isn't equipped to edit them today, and
//! losing the rule by silently dropping it would be worse than
//! refusing the write.

use cal_core::{
    AttendeeStatus, Calendar, ColorSource, ContainerColor, Event, EventRecurrence, NewEvent,
    Reminder, ReminderKind,
};
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{GraphError, GraphResult};

// ── Calendar list ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CalendarListResponse {
    #[serde(default)]
    pub value: Vec<CalendarListEntry>,
    /// Graph uses `@odata.nextLink` instead of Google's
    /// `nextPageToken`; we follow it verbatim.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CalendarListEntry {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "hexColor")]
    pub hex_color: Option<String>,
    /// `false` ⇒ calendar is shared in read-only mode. Aperio maps
    /// that to `Calendar.read_only`.
    #[serde(default, rename = "canEdit")]
    pub can_edit: Option<bool>,
}

pub fn map_calendar(entry: CalendarListEntry) -> Calendar {
    let color = entry.hex_color.and_then(parse_hex_color);
    Calendar {
        // Graph always sends invitations when attendees are present in the
        // body (no per-request suppress) — see EventWriteBody::attendees.
        supports_scheduling: true,
        // No RFC 7986 per-event COLOR round-trip on Graph; per-event colors
        // stay host-local overrides.
        supports_event_color: false,
        // Graph mails the attendees of a meeting the account organizes about
        // every change and about its deletion; no request can stay silent
        // (decision 82b; per Microsoft's documentation, not measured live).
        always_notifies_attendees: true,
        // Graph mails the guests by itself, but it takes an invitee's edits
        // and keeps them local, so an invitation is not read-only here (77a).
        invitations_reply_only: false,
        stores_occurrence_exceptions: false,
        notifier_name: Some("Microsoft 365".into()),
        color_label: None,
        id: entry.id,
        name: entry.name,
        color,
        read_only: !entry.can_edit.unwrap_or(true),
        default_sound: None,
        invitation_import: Default::default(),
        // Graph mints its own `uid` on a create: a file's is not kept.
        keeps_uid: false,
    }
}

fn parse_hex_color(raw: String) -> Option<ContainerColor> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('#') || trimmed.len() != 7 {
        return None;
    }
    if !trimmed[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(ContainerColor {
        hex: trimmed.to_ascii_lowercase(),
        source: ColorSource::Native,
    })
}

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EventListResponse {
    #[serde(default)]
    pub value: Vec<EventEntry>,
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// One page of a `calendarView/delta` response. Each `value` element is
/// kept as raw JSON because the page mixes two shapes: a normal event
/// object, and a tombstone `{ "id": "…", "@removed": { "reason": "…" } }`
/// that lacks the `start`/`end` a full [`EventEntry`] requires. The
/// caller branches on the `@removed` marker before deserialising the
/// rest. Intermediate pages carry `@odata.nextLink`; the final page
/// carries `@odata.deltaLink` — the opaque cursor for the next round.
#[derive(Debug, Deserialize)]
pub struct EventDeltaResponse {
    #[serde(default)]
    pub value: Vec<serde_json::Value>,
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    #[serde(default, rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EventEntry {
    pub id: String,
    #[serde(default)]
    pub subject: Option<String>,
    /// The FULL body. Graph returns it on every event read (the adapter sets
    /// no `$select`, so the default property set applies) — reading only
    /// `bodyPreview` and then writing that back as `body` truncated every
    /// description to its ~255-character preview, server-side.
    #[serde(default)]
    pub body: Option<EventBodyRead>,
    /// Graph's truncated plain-text preview. Kept as a FALLBACK for a shape
    /// that carries no `body`, so the field is not silently dropped if a
    /// future `$select` narrows the read.
    #[serde(default, rename = "bodyPreview")]
    pub body_preview: Option<String>,
    #[serde(default)]
    pub location: Option<EventLocation>,
    #[serde(rename = "isAllDay", default)]
    pub is_all_day: bool,
    #[serde(rename = "isCancelled", default)]
    pub is_cancelled: bool,
    #[serde(rename = "isReminderOn", default)]
    pub is_reminder_on: bool,
    #[serde(rename = "reminderMinutesBeforeStart", default)]
    pub reminder_minutes_before_start: Option<i64>,
    pub start: GraphDateTime,
    pub end: GraphDateTime,
    #[serde(default)]
    pub recurrence: Option<RecurrenceObject>,
    #[serde(default, rename = "createdDateTime")]
    pub created_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "lastModifiedDateTime")]
    pub last_modified_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "@odata.etag")]
    pub etag: Option<String>,
    /// Invitees + their RSVP state (`event.attendees[]`). Empty on a
    /// non-meeting event.
    #[serde(default)]
    pub attendees: Vec<GraphAttendeeRead>,
    /// The meeting organizer.
    #[serde(default)]
    pub organizer: Option<GraphRecipient>,
    /// Whether the signed-in user organizes the event (decision 70a). Part of
    /// the default property set.
    #[serde(default, rename = "isOrganizer")]
    pub is_organizer: Option<bool>,
    /// `singleInstance`, `occurrence`, `exception` or `seriesMaster`.
    #[serde(default, rename = "type")]
    pub event_type: Option<String>,
    /// The iCalendar UID of the event — but of each OCCURRENCE on its own, so
    /// `calendarView` rows of a series carry one each (Graph docs). Taken for
    /// a single appointment only.
    #[serde(default, rename = "iCalUId")]
    pub ical_uid: Option<String>,
    /// The UID a series' master and all its occurrences share (Graph docs).
    /// Unmeasured whether the default property set carries it; when it does,
    /// it is the UID for every row.
    #[serde(default)]
    pub uid: Option<String>,
}

/// An event body as Graph returns it: the content plus which of the two
/// content types it is in.
#[derive(Debug, Clone, Deserialize)]
pub struct EventBodyRead {
    #[serde(default, rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

impl EventBodyRead {
    /// True when Graph labelled this body as HTML.
    pub fn is_html(&self) -> bool {
        self.content_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("html"))
    }

    /// The body as plain text for Aperio's `Event.description`, which is a
    /// plain-text field rendered as such on both platforms.
    pub fn as_plain_text(&self) -> Option<String> {
        let content = self.content.as_deref()?;
        Some(if self.is_html() {
            html_to_text(content)
        } else {
            content.to_string()
        })
    }
}

/// Flatten an HTML body to readable plain text.
///
/// Deliberately small: Graph event bodies are Outlook-authored HTML and the
/// goal is a description a human can read in a plain-text field, not fidelity.
/// Anything richer would be a lie anyway, because the editor cannot round-trip
/// it — which is exactly why an UNCHANGED body is never written back (see
/// `api::update_event`).
pub fn html_to_text(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    // Where the current `<a>` started in `out`, and the URL it points at.
    let mut anchor: Option<(usize, String)> = None;
    while i < html.len() {
        if bytes[i] != b'<' {
            // Copy one whole char — indexing by byte would split a multi-byte
            // one, and Outlook bodies are full of umlauts and dashes.
            let ch = html[i..].chars().next().expect("i is a char boundary");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let Some(close_rel) = html[i..].find('>') else {
            // Unterminated tag: the rest is not markup, so keep it as text
            // rather than swallowing the tail of the description.
            out.push_str(&html[i..]);
            break;
        };
        let tag = &html[i + 1..i + close_rel];
        i += close_rel + 1;

        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|ch: char| ch.is_whitespace() || ch == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        match name.as_str() {
            // Script and style contents are not text — skip past the close tag.
            "script" | "style" if !closing => {
                let needle = format!("</{name}");
                let rest = &html[i..];
                i += match rest.to_ascii_lowercase().find(&needle) {
                    Some(pos) => rest[pos..]
                        .find('>')
                        .map(|e| pos + e + 1)
                        .unwrap_or(rest.len()),
                    None => rest.len(),
                };
            }
            // Links are the one tag whose ATTRIBUTE is content. An Outlook
            // invitation writes its join link as `<a href="…">Join meeting</a>`
            // — dropping the tag would leave the words and lose the URL, so the
            // description would carry no link at all and neither the user nor
            // DescriptionLinks could reach the meeting.
            "a" if !closing => {
                anchor = attr_value(tag, "href").map(|href| (out.len(), href));
            }
            "a" => {
                if let Some((start, href)) = anchor.take() {
                    // Usually the anchor text IS the URL; only append when it
                    // would otherwise be lost, so the common case reads cleanly.
                    let text = &out[start.min(out.len())..];
                    if !text.contains(href.as_str()) {
                        let sep = if text.trim().is_empty() { "" } else { " " };
                        out.push_str(&format!("{sep}{href}"));
                    }
                }
            }
            // Block-level elements and breaks become newlines so the text keeps
            // a shape; every other tag simply disappears.
            "br" | "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "blockquote" | "table" => out.push('\n'),
            _ => {}
        }
    }

    let decoded = decode_entities(&out);
    // Drop the blank lines the block tags leave behind. Both the opening and
    // the closing tag emit a newline — the opening one so a block starts on its
    // own line, the closing one so text that follows the block does not run
    // into it — which leaves an empty line between every pair of blocks. Outlook
    // wraps nearly every line in its own `<div>`, so keeping them would
    // double-space the whole description. Dropping them gives one line per
    // block, which is what a plain-text field and a screen reader both want.
    decoded
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Read one attribute out of a start tag, handling `x="…"`, `x='…'` and a bare
/// unquoted value. Entities are decoded, because a URL in an HTML attribute
/// routinely arrives with `&amp;` where the real URL has `&` — and a join link
/// with a mangled query string is a dead link.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0usize;
    loop {
        let rel = lower[from..].find(name)?;
        let at = from + rel;
        // Must be preceded by whitespace, so `href` does not match `xhref`.
        let preceded_ok = at > 0
            && lower[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after = lower[at + name.len()..].trim_start();
        if preceded_ok && after.starts_with('=') {
            let eq = tag[at + name.len()..].find('=')? + at + name.len() + 1;
            let rest = tag[eq..].trim_start();
            let value = match rest.chars().next() {
                Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
                _ => rest.split_whitespace().next().unwrap_or(""),
            };
            let value = decode_entities(value);
            return (!value.is_empty()).then_some(value);
        }
        from = at + name.len();
    }
}

/// Decode the entities that actually appear in Outlook bodies, plus numeric
/// references. An unknown named entity is left verbatim — showing `&hearts;`
/// beats swallowing text.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let replacement = match entity {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" | "#39" => Some("'".to_string()),
            "nbsp" => Some("\u{a0}".to_string()),
            other => other
                .strip_prefix('#')
                .and_then(|n| {
                    n.strip_prefix(['x', 'X'])
                        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                        .or_else(|| n.parse::<u32>().ok())
                })
                .and_then(char::from_u32)
                .map(|c| c.to_string()),
        };
        match replacement {
            Some(r) => {
                out.push_str(&r);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Deserialize)]
pub struct EventLocation {
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
}

/// One attendee row on a read Graph event.
#[derive(Debug, Deserialize)]
pub struct GraphAttendeeRead {
    #[serde(default, rename = "emailAddress")]
    pub email_address: Option<GraphEmailAddressRead>,
    #[serde(default)]
    pub status: Option<GraphResponseStatus>,
}

#[derive(Debug, Deserialize)]
pub struct GraphRecipient {
    #[serde(default, rename = "emailAddress")]
    pub email_address: Option<GraphEmailAddressRead>,
}

/// Read-side counterpart of the write [`GraphEmailAddress`] — fields are
/// optional because Graph may omit `name` (or, rarely, `address`).
#[derive(Debug, Deserialize)]
pub struct GraphEmailAddressRead {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GraphResponseStatus {
    /// `none` | `organizer` | `tentativelyAccepted` | `accepted` |
    /// `declined` | `notResponded`.
    #[serde(default)]
    pub response: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GraphDateTime {
    /// Graph's `dateTime` field is a naive ISO-8601-ish string
    /// (`2026-05-25T10:00:00.0000000`) without a `Z` suffix even
    /// when `timeZone` is `"UTC"`. We parse it together with the
    /// timezone field.
    #[serde(rename = "dateTime")]
    pub date_time: String,
    #[serde(rename = "timeZone")]
    pub time_zone: String,
}

impl GraphDateTime {
    /// Convert to UTC. For `timeZone == "UTC"` we just append `Z`
    /// before parsing; anything else must be tzdata's own name in exact
    /// case (Graph speaks the IANA names natively), read with
    /// `cal_core::Zone`. A wall time the zone skips or shows twice is an
    /// error (`WallPolicy::Strict`).
    pub(crate) fn to_utc(&self) -> GraphResult<DateTime<Utc>> {
        // Strip the seven-digit fractional second Graph likes to
        // emit but chrono rejects past three digits.
        let trimmed = trim_fractional_seconds(&self.date_time);
        if self.time_zone.eq_ignore_ascii_case("UTC")
            || self.time_zone.eq_ignore_ascii_case("Etc/UTC")
        {
            let with_z = format!("{trimmed}Z");
            return with_z
                .parse::<DateTime<Utc>>()
                .map_err(|e| GraphError::Protocol(format!("graph datetime: {e}: {with_z}")));
        }
        let tz = cal_core::Zone::named(&self.time_zone).ok_or_else(|| {
            GraphError::Protocol(format!("unknown timezone '{}'", self.time_zone))
        })?;
        let naive = chrono::NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%dT%H:%M:%S")
            .map_err(|e| GraphError::Protocol(format!("graph naive datetime: {e}")))?;
        tz.to_utc(naive, cal_core::WallPolicy::Strict)
            .ok_or_else(|| {
                GraphError::Protocol(format!(
                    "ambiguous local time {naive} in {}",
                    self.time_zone
                ))
            })
    }

    /// All-day boundary: the calendar DAY named in the wire string,
    /// anchored at LOCAL midnight (the app-internal all-day convention).
    /// For all-day events the date component IS the intended day — Graph
    /// pins the boundaries to midnight of that day in whatever zone the
    /// event carries — so we read the date verbatim instead of converting
    /// the instant (which could slip a day across timezones). DST edge:
    /// fall forward to the first valid local time when midnight is
    /// skipped.
    pub(crate) fn to_all_day_boundary(&self) -> GraphResult<DateTime<Utc>> {
        let trimmed = trim_fractional_seconds(&self.date_time);
        let naive = chrono::NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%dT%H:%M:%S")
            .map_err(|e| GraphError::Protocol(format!("graph naive datetime: {e}")))?;
        let midnight = naive
            .date()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        Ok(Local
            .from_local_datetime(&midnight)
            .earliest()
            .map(|l| l.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&midnight)))
    }
}

fn trim_fractional_seconds(raw: &str) -> String {
    // Graph: "2026-05-25T10:00:00.0000000" — too many fractional
    // digits for chrono. Truncate to three. If there's no fraction
    // at all, return the input unchanged.
    if let Some((head, tail)) = raw.split_once('.') {
        // Take at most three fractional digits, drop the rest.
        let mut frac = tail.chars().take(3).collect::<String>();
        // Strip trailing zeros for cleanliness.
        while frac.ends_with('0') {
            frac.pop();
        }
        if frac.is_empty() {
            head.to_string()
        } else {
            format!("{head}.{frac}")
        }
    } else {
        raw.to_string()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RecurrenceObject {
    pub pattern: RecurrencePattern,
    pub range: RecurrenceRange,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RecurrencePattern {
    #[serde(rename = "type")]
    pub kind: String, // daily | weekly | absoluteMonthly | relativeMonthly | absoluteYearly | relativeYearly
    pub interval: u32,
    #[serde(default, rename = "daysOfWeek")]
    pub days_of_week: Vec<String>,
    #[serde(default, rename = "dayOfMonth")]
    pub day_of_month: Option<u32>,
    #[serde(default)]
    pub month: Option<u32>,
    #[serde(default)]
    pub index: Option<String>,
    #[serde(default, rename = "firstDayOfWeek")]
    pub first_day_of_week: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RecurrenceRange {
    #[serde(rename = "type")]
    pub kind: String, // endDate | noEnd | numbered
    #[serde(default, rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(default, rename = "endDate")]
    pub end_date: Option<String>,
    #[serde(default, rename = "numberOfOccurrences")]
    pub number_of_occurrences: Option<u32>,
}

pub fn map_event(entry: EventEntry, calendar_id: &str) -> GraphResult<Option<Event>> {
    // A cancelled meeting stays VISIBLE (flagged `cancelled`, reminder-free,
    // hidden only by the user's show-cancelled setting) rather than being
    // dropped. `/calendarView` returns pre-expanded occurrences
    // (recurrence=None → no frontend re-expansion), so a lingering cancelled
    // instance never doubles up against a master. Genuine removals arrive as
    // `@removed` tombstones, handled by the caller before map_event.
    let cancelled = entry.is_cancelled;
    // All-day boundaries carry the intended calendar day in the wire
    // string — anchor those at LOCAL midnight (the app-internal all-day
    // convention) instead of converting the instant.
    let (start, end) = if entry.is_all_day {
        (
            entry.start.to_all_day_boundary()?,
            entry.end.to_all_day_boundary()?,
        )
    } else {
        (entry.start.to_utc()?, entry.end.to_utc()?)
    };
    // Graph attaches the IANA zone to start/end (it speaks IANA natively);
    // carry it for a timed recurring master so the frontend expands
    // DST-correctly. All-day + plain UTC stay on the UTC path. A zone
    // `Zone::named` does not know in exact case was already refused by
    // `start.to_utc()` above (as before); only POST/PATCH responses carry
    // masters.
    let tzid = (!entry.is_all_day)
        .then(|| entry.start.time_zone.clone())
        .filter(|t| !t.is_empty() && t != "UTC");
    let recurrence = match entry.recurrence {
        Some(r) => read_back_rrule(&r, start, entry.is_all_day, tzid.as_deref()).map(|rrule| {
            EventRecurrence {
                rrule,
                exceptions: Vec::new(), // Graph models exceptions separately
                tzid,
            }
        }),
        None => None,
    };

    let reminders = if entry.is_reminder_on {
        match entry.reminder_minutes_before_start {
            Some(minutes) => vec![Reminder {
                kind: ReminderKind::Relative {
                    minutes_before: minutes,
                },
                sound: None,
            }],
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let created = entry.created_date_time.unwrap_or_else(Utc::now);
    let updated = entry.last_modified_date_time.unwrap_or(created);

    // A series' rows share `uid`; `iCalUId` names one occurrence only, so it
    // stands for the appointment only when it is a single one.
    let single = matches!(entry.event_type.as_deref(), None | Some("singleInstance"));
    let ical_uid = entry.uid.filter(|uid| !uid.trim().is_empty()).or_else(|| {
        entry
            .ical_uid
            .filter(|uid| single && !uid.trim().is_empty())
    });

    // Attendees: the editable flat list ("Name <email>" / bare email) plus
    // per-attendee RSVP state, without the organizer (decision 67a), whose row
    // answers "organizer". `isOrganizer` says whether the signed-in user
    // organizes the event (decision 70a).
    let people = cal_core::attendee::people_from_read(
        entry
            .organizer
            .and_then(|o| o.email_address)
            .and_then(|e| e.address),
        entry.is_organizer,
        entry.attendees.into_iter().filter_map(|a| {
            let response = a.status.and_then(|s| s.response);
            let email_address = a.email_address?;
            Some(cal_core::attendee::ReadAttendee {
                email: email_address.address?,
                name: email_address.name,
                status: response.as_deref().map(graph_status).unwrap_or_default(),
                is_organizer: response.as_deref() == Some("organizer"),
            })
        }),
    );

    Ok(Some(Event {
        original_left_in: None,
        keep_attendees: false,
        keep_fields: Vec::new(),
        clear_attendees: false,
        clear_recurrence: false,
        organized_elsewhere: people.organized_elsewhere,
        send_invitations: false,
        truncate_tail_overrides: false,
        accepts_exception_loss: false,
        occurrence_shift_minutes: None,
        deletions_not_restored: Vec::new(),
        id: entry.id,
        calendar_id: calendar_id.to_string(),
        title: entry.subject.unwrap_or_default(),
        description: entry
            .body
            .as_ref()
            .and_then(EventBodyRead::as_plain_text)
            .filter(|s| !s.is_empty())
            .or(entry.body_preview),
        location: entry.location.and_then(|l| l.display_name),
        start,
        end,
        all_day: entry.is_all_day,
        recurrence,
        color_label: None,
        // Graph's per-event color isn't mapped; colors are host-local overrides.
        color_hex: None,
        reminders,
        sound: None,
        attendees: people.attendees,
        created_at: created,
        updated_at: updated,
        etag: entry.etag,
        organizer: people.organizer,
        attendee_responses: people.attendee_responses,
        cancelled,
        scheduling_silenced: false,
        ical_uid,
    }))
}

/// Map Graph's attendee `status.response` to the normalised RSVP enum.
/// `organizer` (the response on the organizer's own row) reads as an
/// implicit acceptance.
fn graph_status(s: &str) -> AttendeeStatus {
    match s {
        "accepted" | "organizer" => AttendeeStatus::Accepted,
        "declined" => AttendeeStatus::Declined,
        "tentativelyAccepted" => AttendeeStatus::Tentative,
        _ => AttendeeStatus::NeedsAction,
    }
}

// ── Recurrence: Graph ⇄ RRULE ───────────────────────────────────────────

/// Convert Graph's structured recurrence into an RFC 5545 RRULE
/// body (no `RRULE:` prefix — rest of Aperio stores it bare).
/// Returns `None` for relative-monthly / relative-yearly patterns
/// that we can't represent with the simple BYDAY model — the event
/// still surfaces, just without an editable recurrence in the UI.
pub fn recurrence_to_rrule(r: &RecurrenceObject) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match r.pattern.kind.as_str() {
        "daily" => parts.push("FREQ=DAILY".into()),
        "weekly" => {
            parts.push("FREQ=WEEKLY".into());
            if !r.pattern.days_of_week.is_empty() {
                let by = r
                    .pattern
                    .days_of_week
                    .iter()
                    .filter_map(|d| day_name_to_rrule(d))
                    .collect::<Vec<_>>()
                    .join(",");
                if !by.is_empty() {
                    parts.push(format!("BYDAY={by}"));
                }
            }
        }
        "absoluteMonthly" => {
            parts.push("FREQ=MONTHLY".into());
            if let Some(dom) = r.pattern.day_of_month {
                parts.push(format!("BYMONTHDAY={dom}"));
            }
        }
        "absoluteYearly" => {
            parts.push("FREQ=YEARLY".into());
            if let Some(m) = r.pattern.month {
                parts.push(format!("BYMONTH={m}"));
            }
            if let Some(dom) = r.pattern.day_of_month {
                parts.push(format!("BYMONTHDAY={dom}"));
            }
        }
        // Relative monthly / yearly skip — we'd need BYSETPOS +
        // BYDAY plus careful index→position translation. Not in
        // the 6e.1 scope.
        _ => return None,
    }
    if r.pattern.interval > 1 {
        parts.push(format!("INTERVAL={}", r.pattern.interval));
    }
    match r.range.kind.as_str() {
        "endDate" => {
            if let Some(end) = r.range.end_date.as_deref() {
                if let Ok(d) = NaiveDate::parse_from_str(end, "%Y-%m-%d") {
                    parts.push(format!("UNTIL={}", d.format("%Y%m%dT235959Z")));
                }
            }
        }
        "numbered" => {
            if let Some(n) = r.range.number_of_occurrences {
                parts.push(format!("COUNT={n}"));
            }
        }
        // "noEnd" → no UNTIL/COUNT — open-ended series.
        _ => {}
    }
    Some(parts.join(";"))
}

/// A series' rule as Microsoft 365 sends it back, its `endDate` D spelled as
/// "through D" on the series' clock (`cal_core::until_through`, 107b), as
/// the Exchange adapter reads an `EndDate` (READ_RULE 7): no reader needs the
/// 105a guard for it, and [`series_recurrence`] writes it as D again. The
/// clock is the views' (`until_through` reads the zone as they do), with no
/// exact-case gate (D7): a Windows name, UTC, an all-day series and a series
/// without a zone keep `DT235959Z`.
pub(crate) fn read_back_rrule(
    r: &RecurrenceObject,
    start: DateTime<Utc>,
    all_day: bool,
    tzid: Option<&str>,
) -> Option<String> {
    let rule = recurrence_to_rrule(r)?;
    if all_day || r.range.kind != "endDate" {
        return Some(rule);
    }
    let Some(day) = r
        .range
        .end_date
        .as_deref()
        .and_then(|end| NaiveDate::parse_from_str(end, "%Y-%m-%d").ok())
    else {
        return Some(rule);
    };
    Some(cal_core::with_until(
        &rule,
        &cal_core::rule_until_text(cal_core::until_through(day, &rule, start, false, tzid)),
    ))
}

/// Inverse: parse an RRULE body into Graph's structured form.
/// Returns `Err(Protocol)` when the rule uses features Graph can't
/// represent — e.g. BYSETPOS without a known BYDAY pattern.
///
/// `first_day` is the day the series starts on, on the clock Graph reads the
/// range in ([`series_recurrence`]): the range's `startDate`, and the
/// weekday, day of the month and month a rule without BYDAY, BYMONTHDAY or
/// BYMONTH takes. `end_day` is the day the end is counted through on the
/// series' clock, where the rule has an `UNTIL` ([`series_recurrence`]);
/// without one (an all-day series) the `endDate` is the `UNTIL`'s own date.
pub fn rrule_to_recurrence(
    rrule: &str,
    first_day: NaiveDate,
    end_day: Option<NaiveDate>,
) -> GraphResult<RecurrenceObject> {
    let parts: std::collections::HashMap<String, String> = rrule
        .split(';')
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.to_ascii_uppercase(), v.to_string()))
        })
        .collect();
    let freq = parts
        .get("FREQ")
        .ok_or_else(|| GraphError::Protocol("RRULE missing FREQ".into()))?
        .as_str();
    let interval = parts
        .get("INTERVAL")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1);

    let mut pattern = RecurrencePattern {
        kind: String::new(),
        interval,
        days_of_week: Vec::new(),
        day_of_month: None,
        month: None,
        index: None,
        first_day_of_week: Some("monday".into()),
    };
    match freq {
        "DAILY" => pattern.kind = "daily".into(),
        "WEEKLY" => {
            pattern.kind = "weekly".into();
            if let Some(byday) = parts.get("BYDAY") {
                pattern.days_of_week = byday.split(',').filter_map(rrule_day_to_name).collect();
            }
            if pattern.days_of_week.is_empty() {
                // Default to the weekday the series starts on.
                pattern
                    .days_of_week
                    .push(weekday_to_name(first_day.weekday()));
            }
        }
        "MONTHLY" => {
            if let Some(dom) = parts.get("BYMONTHDAY").and_then(|s| s.parse().ok()) {
                pattern.kind = "absoluteMonthly".into();
                pattern.day_of_month = Some(dom);
            } else if parts.contains_key("BYDAY") && parts.contains_key("BYSETPOS") {
                // Relative monthly — beyond 6e.1 scope.
                return Err(GraphError::Protocol(
                    "relative monthly recurrence not supported on write yet".into(),
                ));
            } else {
                pattern.kind = "absoluteMonthly".into();
                pattern.day_of_month = Some(first_day.day());
            }
        }
        "YEARLY" => {
            pattern.kind = "absoluteYearly".into();
            pattern.month = parts
                .get("BYMONTH")
                .and_then(|s| s.parse().ok())
                .or(Some(first_day.month()));
            pattern.day_of_month = parts
                .get("BYMONTHDAY")
                .and_then(|s| s.parse().ok())
                .or(Some(first_day.day()));
        }
        other => {
            return Err(GraphError::Protocol(format!("unsupported FREQ: {other}")));
        }
    }

    let mut range = RecurrenceRange {
        kind: "noEnd".into(),
        start_date: Some(first_day.format("%Y-%m-%d").to_string()),
        end_date: None,
        number_of_occurrences: None,
    };
    if let Some(until) = parts.get("UNTIL") {
        // RFC 5545: compact UTC date-time. Graph wants YYYY-MM-DD: a cut's
        // day where there is one, else the UNTIL's own digits, the day the
        // repeat field wrote (`…T235959Z`).
        let own_day = until
            .get(..8)
            .and_then(|digits| NaiveDate::parse_from_str(digits, "%Y%m%d").ok());
        if let Some(day) = end_day.or(own_day) {
            range.kind = "endDate".into();
            range.end_date = Some(day.format("%Y-%m-%d").to_string());
        }
    } else if let Some(count) = parts.get("COUNT").and_then(|s| s.parse().ok()) {
        range.kind = "numbered".into();
        range.number_of_occurrences = Some(count);
    }

    Ok(RecurrenceObject { pattern, range })
}

fn day_name_to_rrule(name: &str) -> Option<&'static str> {
    // Graph uses lowercase full names. RRULE uses two-letter caps.
    match name.to_ascii_lowercase().as_str() {
        "monday" => Some("MO"),
        "tuesday" => Some("TU"),
        "wednesday" => Some("WE"),
        "thursday" => Some("TH"),
        "friday" => Some("FR"),
        "saturday" => Some("SA"),
        "sunday" => Some("SU"),
        _ => None,
    }
}

fn rrule_day_to_name(code: &str) -> Option<String> {
    Some(match code.to_ascii_uppercase().as_str() {
        "MO" => "monday".into(),
        "TU" => "tuesday".into(),
        "WE" => "wednesday".into(),
        "TH" => "thursday".into(),
        "FR" => "friday".into(),
        "SA" => "saturday".into(),
        "SU" => "sunday".into(),
        _ => return None,
    })
}

fn weekday_to_name(w: chrono::Weekday) -> String {
    use chrono::Weekday::*;
    match w {
        Mon => "monday",
        Tue => "tuesday",
        Wed => "wednesday",
        Thu => "thursday",
        Fri => "friday",
        Sat => "saturday",
        Sun => "sunday",
    }
    .to_string()
}

// ── Reverse mapping: cal_core → Graph JSON ──────────────────────────────

#[derive(Debug, Serialize)]
pub struct EventWriteBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<EventBodyWrite>,
    pub start: GraphDateTimeWrite,
    pub end: GraphDateTimeWrite,
    #[serde(rename = "isAllDay")]
    pub is_all_day: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<EventLocationWrite>,
    #[serde(rename = "isReminderOn")]
    pub is_reminder_on: bool,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "reminderMinutesBeforeStart"
    )]
    pub reminder_minutes_before_start: Option<i64>,
    /// `None` leaves the field out, so Graph keeps its pattern; `Some(Some)`
    /// writes this one; `Some(None)` is JSON `null`, which Graph documents
    /// for taking the pattern away. That one only on a series the host says
    /// the edit made single ([`Event::clear_recurrence`], decision 280).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Option<RecurrenceObject>>,
    /// Microsoft Graph QUIRK: putting attendees in the body makes Graph
    /// EMAIL them on create/update — there is no per-request suppress. So we
    /// write this ONLY when the user opted to notify; declining means Graph
    /// stores no attendee list (documented limitation). `None` leaves Graph's
    /// list alone; `Some` of an empty list clears it, which only a notified,
    /// host-confirmed removal of every invitee asks for (decision 74a).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attendees: Option<Vec<GraphAttendeeWrite>>,
}

#[derive(Debug, Serialize)]
pub struct GraphAttendeeWrite {
    #[serde(rename = "emailAddress")]
    pub email_address: GraphEmailAddress,
    #[serde(rename = "type")]
    pub attendee_type: String,
}

#[derive(Debug, Serialize)]
pub struct GraphEmailAddress {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Map the flat attendee list to Graph attendee objects — but only when
/// `notify` is set (see the quirk on [`EventWriteBody::attendees`]).
fn attendees_to_write(attendees: &[String], notify: bool) -> Vec<GraphAttendeeWrite> {
    if !notify {
        return Vec::new();
    }
    attendees
        .iter()
        .filter_map(|entry| {
            let (name, address) = cal_core::attendee::parse(entry);
            (!address.is_empty()).then_some(GraphAttendeeWrite {
                email_address: GraphEmailAddress { address, name },
                attendee_type: "required".into(),
            })
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct EventBodyWrite {
    #[serde(rename = "contentType")]
    pub content_type: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct EventLocationWrite {
    #[serde(rename = "displayName")]
    pub display_name: String,
}

#[derive(Debug, Serialize)]
pub struct GraphDateTimeWrite {
    #[serde(rename = "dateTime")]
    pub date_time: String,
    #[serde(rename = "timeZone")]
    pub time_zone: String,
}

pub fn new_event_to_body(new: &NewEvent) -> GraphResult<EventWriteBody> {
    new_event_to_body_in(new, &Local)
}

/// [`new_event_to_body`] on a given device clock, so tests can pin one.
fn new_event_to_body_in<D: TimeZone>(new: &NewEvent, device: &D) -> GraphResult<EventWriteBody> {
    // The core's rule: an all-day series hands Graph no zone (decision 46a).
    let tzid = cal_core::written_series_zone(
        new.recurrence.as_ref().and_then(|r| r.tzid.as_deref()),
        new.all_day,
    );
    let reminder_minutes = first_reminder_minutes(&new.reminders);
    let body = EventWriteBody {
        subject: Some(new.title.clone()),
        body: new.description.clone().map(|c| EventBodyWrite {
            content_type: "text".into(),
            content: c,
        }),
        start: write_datetime_in(new.start, new.all_day, tzid, device),
        end: write_datetime_in(new.end, new.all_day, tzid, device),
        is_all_day: new.all_day,
        location: new
            .location
            .clone()
            .map(|l| EventLocationWrite { display_name: l }),
        is_reminder_on: reminder_minutes.is_some(),
        reminder_minutes_before_start: reminder_minutes,
        recurrence: match new.recurrence.as_ref() {
            Some(r) => Some(Some(series_recurrence(
                &r.rrule,
                new.start,
                new.all_day,
                tzid,
                false,
                device,
            )?)),
            None => None,
        },
        attendees: Some(attendees_to_write(&new.attendees, new.send_invitations))
            .filter(|list| !list.is_empty()),
    };
    Ok(body)
}

pub fn event_to_body(ev: &Event) -> GraphResult<EventWriteBody> {
    event_to_body_in(ev, &Local)
}

/// [`event_to_body`] on a given device clock, so tests can pin one.
fn event_to_body_in<D: TimeZone>(ev: &Event, device: &D) -> GraphResult<EventWriteBody> {
    // The core's rule: an all-day series hands Graph no zone (decision 46a).
    let tzid = cal_core::written_series_zone(
        ev.recurrence.as_ref().and_then(|r| r.tzid.as_deref()),
        ev.all_day,
    );
    let reminder_minutes = first_reminder_minutes(&ev.reminders);
    Ok(EventWriteBody {
        subject: Some(ev.title.clone()),
        body: ev.description.clone().map(|c| EventBodyWrite {
            content_type: "text".into(),
            content: c,
        }),
        start: write_datetime_in(ev.start, ev.all_day, tzid, device),
        end: write_datetime_in(ev.end, ev.all_day, tzid, device),
        is_all_day: ev.all_day,
        location: ev
            .location
            .clone()
            .map(|l| EventLocationWrite { display_name: l }),
        is_reminder_on: reminder_minutes.is_some(),
        reminder_minutes_before_start: reminder_minutes,
        recurrence: match ev.recurrence.as_ref() {
            Some(r) => Some(Some(series_recurrence(
                &r.rrule,
                ev.start,
                ev.all_day,
                tzid,
                ev.truncate_tail_overrides,
                device,
            )?)),
            // The repeat removed (decision 280): Graph's delete value. A
            // missing pattern alone says nothing — a series whose pattern
            // Aperio could not read has none either.
            None if ev.clear_recurrence => Some(None),
            None => None,
        },
        // Not when the edit left the invitees alone (decision 71a): the list
        // on the server keeps the organizer's row, which Aperio does not show.
        attendees: if !ev.send_invitations || ev.keep_attendees {
            None
        } else if ev.clear_attendees {
            Some(Vec::new())
        } else {
            Some(attendees_to_write(&ev.attendees, true)).filter(|list| !list.is_empty())
        },
    })
}

/// The zone a timed series' boundaries and repeat range are written in: its
/// stored zone where tzdata names it in exact case, sent as the IANA name
/// (Graph accepts those). `None` for an all-day series (decision 46a; its
/// `tzid` is `None` already) and where there is no such zone: that series is
/// written in UTC.
fn written_zone(all_day: bool, tzid: Option<&str>) -> Option<(&str, cal_core::Zone)> {
    if all_day {
        return None;
    }
    tzid.and_then(|name| cal_core::Zone::named(name).map(|zone| (name, zone)))
}

/// The calendar day an all-day boundary names: Aperio keeps it as the
/// device's midnight of that day.
fn device_day<D: TimeZone>(when: DateTime<Utc>, device: &D) -> NaiveDate {
    when.with_timezone(device).date_naive()
}

/// A boundary as Graph gets it, on a given device clock.
fn write_datetime_in<D: TimeZone>(
    when: DateTime<Utc>,
    all_day: bool,
    tzid: Option<&str>,
    device: &D,
) -> GraphDateTimeWrite {
    if all_day {
        // For all-day events Graph still expects a `dateTime` (not a
        // `date`) at 00:00:00 in the given zone. The DAY must be the
        // user's LOCAL calendar day — formatting the UTC instant would
        // emit the UTC day, one early for users east of UTC (the
        // boundary instants are local midnights expressed in UTC). The
        // internal end is already exclusive, as Graph requires.
        GraphDateTimeWrite {
            date_time: device_day(when, device)
                .format("%Y-%m-%dT00:00:00")
                .to_string(),
            time_zone: "UTC".into(),
        }
    } else if let Some((name, tz)) = written_zone(all_day, tzid) {
        // A zoned recurring master: send the LOCAL wall-clock + its IANA zone so
        // Graph expands the series DST-correctly on its side (Graph accepts IANA
        // names). A one-off is an exact instant, so it stays on the UTC branch.
        GraphDateTimeWrite {
            date_time: tz.wall(when).format("%Y-%m-%dT%H:%M:%S").to_string(),
            time_zone: name.to_string(),
        }
    } else {
        GraphDateTimeWrite {
            date_time: when.format("%Y-%m-%dT%H:%M:%S").to_string(),
            time_zone: "UTC".into(),
        }
    }
}

/// The repeat Graph gets for a series, its dates read on the clock its start
/// is written on.
///
/// Aperio writes no `recurrenceTimeZone`, so Graph reads the range's dates on
/// the clock of the event's start, and its `startDate` must be the start's
/// own date. The range therefore starts on the day [`write_datetime_in`]
/// writes the start on: an all-day series on the device's day (48a), a zoned
/// one on its zone's, anything else on UTC's; a rule without BYDAY,
/// BYMONTHDAY or BYMONTH takes its weekday, day and month from that day. Read
/// off the UTC instant, as before, it was the day before for a series east of
/// UTC that starts up to the zone's distance from UTC after midnight (00:30 in
/// Berlin) and for every all-day series there, and the day after for an
/// evening series west of UTC (20:00 in New York). The same date error
/// Exchange had before PR 8a (DESIGN-series-time-zone.md, stage 5).
///
/// The end, too, is a day on that clock (decisions 235, 140a): every end of
/// a timed series is written as the day `cal_core::written_end_day` gives on
/// the clock the start is written on, the last day whose occurrence, at the
/// master's wall time, starts at or before the `UNTIL`. Graph keeps a
/// series' end as a date (the manifest's `recurrence.ends_by_date`), and
/// Exchange, the store behind it, counts that date whole (live round 10a), so
/// the `UNTIL`'s UTC date mostly kept a cut's day in both series. An
/// unflagged `…T235959Z` counts as its own date, the day the repeat field has
/// always written it for; a cut ("this and all following", which sets
/// `truncate_tail_overrides`, `cut`) is counted as the instant it is. Whether
/// Graph counts the end date the same way, and which zone it reads it on, is
/// not measured (TODO.md). An all-day series keeps the date it names (an
/// all-day cut already names the day before). A cut the clock cannot place
/// is refused.
fn series_recurrence<D: TimeZone>(
    rrule: &str,
    start: DateTime<Utc>,
    all_day: bool,
    tzid: Option<&str>,
    cut: bool,
    device: &D,
) -> GraphResult<RecurrenceObject> {
    let zone = written_zone(all_day, tzid).map(|(_, zone)| zone);
    let first_day = match zone {
        _ if all_day => device_day(start, device),
        Some(zone) => zone.wall(start).date(),
        None => start.date_naive(),
    };
    let clock = zone.unwrap_or(cal_core::Zone::UTC);
    let end_day = if all_day {
        None
    } else {
        match cal_core::written_end_day(
            rrule,
            clock.wall(start).time(),
            cut,
            |when| Some(clock.wall(when)),
            |wall| clock.to_utc(wall, cal_core::WallPolicy::EarlierElseHourLater),
        ) {
            Some(day) => Some(day),
            None if cut && cal_core::utc_until(rrule).is_some() => {
                return Err(GraphError::Protocol(
                    cal_core::WriteRefusal::DayZoneUnreadable.message("no-wall"),
                ));
            }
            None => None,
        }
    };
    rrule_to_recurrence(rrule, first_day, end_day)
}

fn first_reminder_minutes(reminders: &[Reminder]) -> Option<i64> {
    // Graph models per-event reminders as a single (minutes,
    // boolean) pair — there's no overrides array like Google's.
    // We take the first Relative reminder; everything else
    // (Absolute, AppStart, Email) stays local-only because Graph
    // has nowhere to put it.
    //
    // `reminderMinutesBeforeStart` counts minutes BEFORE the start, on the
    // same Exchange store whose schema allows nothing below 0. A reminder
    // after the start (negative) is left out and named in the log, as decided
    // 2026-11-07 (TODO B7 222); the next relative reminder takes the slot.
    reminders.iter().find_map(|r| match r.kind {
        ReminderKind::Relative { minutes_before } if minutes_before < 0 => {
            tracing::warn!(
                minutes_after_start = -minutes_before,
                "reminder after the start left out: Outlook keeps only reminders before it"
            );
            None
        }
        ReminderKind::Relative { minutes_before } => Some(minutes_before),
        ReminderKind::RelativeToEnd { minutes_before } => {
            tracing::warn!(
                minutes_before_end = minutes_before,
                "reminder relative to the end left out: Outlook counts only from the start"
            );
            None
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── reminders after the start (TODO B7 222) ───────────────────────────
    fn after_start_rel(minutes_before: i64) -> Reminder {
        Reminder {
            kind: ReminderKind::Relative { minutes_before },
            sound: None,
        }
    }

    #[test]
    fn the_one_outlook_reminder_is_never_after_the_start() {
        assert_eq!(
            first_reminder_minutes(&[after_start_rel(-10), after_start_rel(15)]),
            Some(15)
        );
        assert_eq!(first_reminder_minutes(&[after_start_rel(0)]), Some(0));
        assert_eq!(first_reminder_minutes(&[after_start_rel(-10)]), None);
    }

    // ── description round-trip ────────────────────────────────────────────
    //
    // Regression cover for a live data-loss bug: the adapter read
    // `bodyPreview` — Graph's ~255-character plain-text preview — into
    // `Event.description`, and the write path sent that back as the full
    // `body`. Saving an unrelated field on an Outlook event therefore replaced
    // its description with a truncated, unformatted stump, server-side.

    #[test]
    fn description_comes_from_the_full_body_not_the_preview() {
        let entry: EventEntry = serde_json::from_value(serde_json::json!({
            "id": "evt-1",
            "subject": "Weekly",
            "bodyPreview": "Truncated preview…",
            "body": { "contentType": "text", "content": "The whole description." },
            "start": { "dateTime": "2026-07-28T09:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2026-07-28T10:00:00.0000000", "timeZone": "UTC" },
        }))
        .expect("event entry");
        let ev = map_event(entry, "cal-1").unwrap().expect("event");
        assert_eq!(ev.description.as_deref(), Some("The whole description."));
    }

    #[test]
    fn description_falls_back_to_the_preview_when_there_is_no_body() {
        let entry: EventEntry = serde_json::from_value(serde_json::json!({
            "id": "evt-2",
            "subject": "Weekly",
            "bodyPreview": "Only a preview",
            "start": { "dateTime": "2026-07-28T09:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2026-07-28T10:00:00.0000000", "timeZone": "UTC" },
        }))
        .expect("event entry");
        let ev = map_event(entry, "cal-1").unwrap().expect("event");
        assert_eq!(ev.description.as_deref(), Some("Only a preview"));
    }

    #[test]
    fn an_html_body_is_flattened_for_the_plain_text_description() {
        let entry: EventEntry = serde_json::from_value(serde_json::json!({
            "id": "evt-3",
            "subject": "Weekly",
            "bodyPreview": "Join here",
            "body": {
                "contentType": "html",
                "content": "<html><body><p>Join here:</p><p><a href=\"https://x/j\">https://x/j</a></p><div>Code&nbsp;1234</div></body></html>"
            },
            "start": { "dateTime": "2026-07-28T09:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2026-07-28T10:00:00.0000000", "timeZone": "UTC" },
        }))
        .expect("event entry");
        let ev = map_event(entry, "cal-1").unwrap().expect("event");
        let text = ev.description.expect("description");
        assert!(text.contains("Join here:"), "text was {text:?}");
        assert!(
            text.contains("https://x/j"),
            "the join URL must survive: {text:?}"
        );
        assert!(
            text.contains('\u{a0}') || text.contains("Code"),
            "entities decode: {text:?}"
        );
        assert!(!text.contains('<'), "no markup may remain: {text:?}");
    }

    #[test]
    fn html_to_text_keeps_the_url_behind_a_link() {
        // An Outlook / Webex Scheduler invitation writes its join link as an
        // anchor whose TEXT is a label. Dropping the tag would leave the words
        // and lose the meeting entirely.
        assert_eq!(
            html_to_text(r#"<a href="https://x.webex.com/j.php?MTID=abc">Join meeting</a>"#),
            "Join meeting https://x.webex.com/j.php?MTID=abc"
        );
    }

    #[test]
    fn html_to_text_does_not_repeat_a_url_that_is_already_the_link_text() {
        assert_eq!(
            html_to_text(r#"<a href="https://x/j">https://x/j</a>"#),
            "https://x/j"
        );
    }

    #[test]
    fn html_to_text_unescapes_ampersands_inside_an_href() {
        // `&amp;` in the attribute is `&` in the real URL — a join link whose
        // query string is mangled is a dead link.
        assert_eq!(
            html_to_text(r#"<a href="https://x/j?a=1&amp;b=2">Join</a>"#),
            "Join https://x/j?a=1&b=2"
        );
    }

    #[test]
    fn html_to_text_handles_single_quoted_and_extra_attributes() {
        assert_eq!(
            html_to_text("<a class='btn' href='https://x/j' target='_blank'>Go</a>"),
            "Go https://x/j"
        );
        // A tag whose name merely CONTAINS the attribute name must not match.
        assert_eq!(html_to_text(r#"<a data-href="nope">Go</a>"#), "Go");
    }

    #[test]
    fn html_to_text_keeps_line_structure_and_drops_scripts() {
        let html = "<p>One</p><script>var x = '<p>not text</p>';</script><p>Two</p>";
        assert_eq!(html_to_text(html), "One\nTwo");
    }

    #[test]
    fn html_to_text_survives_multibyte_and_unterminated_tags() {
        // A byte-indexed scanner would split "ü"; an unterminated tag must not
        // swallow the rest of a description.
        assert_eq!(html_to_text("Grüße <b>Welt</b>"), "Grüße Welt");
        assert_eq!(html_to_text("tail <notclosed"), "tail <notclosed");
    }

    #[test]
    fn html_to_text_decodes_numeric_and_named_entities() {
        assert_eq!(
            html_to_text("a &amp; b &lt;c&gt; &#65; &#x42;"),
            "a & b <c> A B"
        );
        // An unknown entity is left verbatim rather than swallowed.
        assert_eq!(html_to_text("&hearts;"), "&hearts;");
    }

    /// All-day instants the way the frontend produces them: LOCAL
    /// midnights (end exclusive), expressed in UTC. Keeps the asserted
    /// wire dates timezone-agnostic.
    fn local_midnight(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(y, m, d)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }

    /// The off-by-one guard: all-day boundaries serialise as midnight of
    /// the LOCAL calendar day — formatting the raw UTC instant would emit
    /// the previous day for users east of UTC.
    #[test]
    fn write_datetime_all_day_uses_local_day_at_midnight() {
        let w = write_datetime_in(local_midnight(2026, 6, 10), true, None, &Local);
        assert_eq!(w.date_time, "2026-06-10T00:00:00");
        assert_eq!(w.time_zone, "UTC");
    }

    /// Read → write round-trip for all-day boundaries: the day named in
    /// Graph's wire string survives unchanged in any timezone.
    #[test]
    fn all_day_boundary_round_trips_through_write() {
        let wire = GraphDateTime {
            date_time: "2026-06-10T00:00:00.0000000".into(),
            time_zone: "UTC".into(),
        };
        let instant = wire.to_all_day_boundary().unwrap();
        // The instant lands on the named LOCAL calendar day…
        assert_eq!(
            instant.with_timezone(&Local).date_naive(),
            NaiveDate::from_ymd_opt(2026, 6, 10).unwrap(),
        );
        // …and writing it back emits the same wire day.
        let back = write_datetime_in(instant, true, None, &Local);
        assert_eq!(back.date_time, "2026-06-10T00:00:00");
    }

    #[test]
    fn calendar_can_edit_false_maps_to_read_only() {
        let entry: CalendarListEntry = serde_json::from_str(
            r##"{"id":"abc","name":"Shared","hexColor":"#0078d4","canEdit":false}"##,
        )
        .unwrap();
        let cal = map_calendar(entry);
        assert!(cal.read_only);
        assert_eq!(cal.color.unwrap().hex, "#0078d4");
    }

    #[test]
    fn trim_fractional_seconds_drops_excess() {
        assert_eq!(
            trim_fractional_seconds("2026-05-25T10:00:00.0000000"),
            "2026-05-25T10:00:00"
        );
        assert_eq!(
            trim_fractional_seconds("2026-05-25T10:00:00.500"),
            "2026-05-25T10:00:00.5"
        );
        assert_eq!(
            trim_fractional_seconds("2026-05-25T10:00:00"),
            "2026-05-25T10:00:00"
        );
    }

    #[test]
    fn map_event_timed_utc() {
        let raw = r#"{
            "id": "ev1",
            "subject": "Standup",
            "start": { "dateTime": "2026-05-25T10:00:00.0000000", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T10:30:00.0000000", "timeZone": "UTC" },
            "isAllDay": false,
            "isReminderOn": true,
            "reminderMinutesBeforeStart": 15
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.title, "Standup");
        assert_eq!(
            ev.start,
            Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap()
        );
        assert_eq!(ev.reminders.len(), 1);
        match ev.reminders[0].kind {
            ReminderKind::Relative { minutes_before } => assert_eq!(minutes_before, 15),
            ref other => panic!("expected Relative, got {other:?}"),
        }
    }

    /// The UID by which an import finds what is already here (I4). `uid` is
    /// the series' and wins; `iCalUId` names one occurrence, so it stands for
    /// a single appointment only.
    #[test]
    fn map_event_reads_the_uid_a_series_shares() {
        let row = |extra: &str| {
            let raw = format!(
                r#"{{
                "id": "ev1",
                "subject": "Standup",
                "start": {{ "dateTime": "2026-05-25T10:00:00.0000000", "timeZone": "UTC" }},
                "end":   {{ "dateTime": "2026-05-25T10:30:00.0000000", "timeZone": "UTC" }}
                {extra}
            }}"#
            );
            let entry: EventEntry = serde_json::from_str(&raw).unwrap();
            map_event(entry, "primary").unwrap().unwrap().ical_uid
        };
        assert_eq!(
            row(r#", "type": "singleInstance", "iCalUId": "single-1""#).as_deref(),
            Some("single-1")
        );
        assert_eq!(
            row(r#", "type": "occurrence", "iCalUId": "occurrence-only""#),
            None
        );
        assert_eq!(
            row(r#", "type": "occurrence", "iCalUId": "occurrence-only", "uid": "series-1""#)
                .as_deref(),
            Some("series-1")
        );
        assert_eq!(row(""), None);
    }

    #[test]
    fn map_event_reads_attendees_and_organizer() {
        let raw = r#"{
            "id": "ev-mtg",
            "subject": "Planning",
            "start": { "dateTime": "2026-05-25T10:00:00.0000000", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T11:00:00.0000000", "timeZone": "UTC" },
            "isAllDay": false,
            "organizer": { "emailAddress": { "name": "The Boss", "address": "boss@example.com" } },
            "attendees": [
              { "type": "required", "status": { "response": "organizer" },
                "emailAddress": { "name": "The Boss", "address": "boss@example.com" } },
              { "type": "required", "status": { "response": "tentativelyAccepted" },
                "emailAddress": { "name": "Me", "address": "me@example.com" } },
              { "type": "required", "status": { "response": "none" },
                "emailAddress": { "address": "nobody@example.com" } }
            ]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.organizer.as_deref(), Some("boss@example.com"));
        // Never the organizer among the invitees (decision 67a).
        assert_eq!(ev.attendees, ["Me <me@example.com>", "nobody@example.com"]);
        assert_eq!(ev.attendee_responses.len(), 2);
        // tentativelyAccepted → Tentative; none → NeedsAction.
        assert_eq!(ev.attendee_responses[0].status, AttendeeStatus::Tentative);
        assert_eq!(ev.attendee_responses[1].status, AttendeeStatus::NeedsAction);
        // No isOrganizer: it cannot be confirmed the user organizes it.
        assert!(ev.organized_elsewhere);
    }

    /// Graph marks the organizer's row with the response "organizer", and the
    /// row goes by that mark even when its address is spelled otherwise than
    /// the organizer's. `isOrganizer` says the user organizes the event.
    #[test]
    fn the_organizer_row_goes_by_its_response() {
        let raw = r#"{
            "id": "ev-own",
            "start": { "dateTime": "2026-05-25T10:00:00.0000000", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T11:00:00.0000000", "timeZone": "UTC" },
            "isAllDay": false,
            "isOrganizer": true,
            "organizer": { "emailAddress": { "name": "Toni", "address": "Toni@Example.com" } },
            "attendees": [
              { "type": "required", "status": { "response": "organizer" },
                "emailAddress": { "name": "Toni", "address": "toni.alias@example.com" } },
              { "type": "required", "status": { "response": "accepted" },
                "emailAddress": { "address": "bob@example.com" } }
            ]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.attendees, ["bob@example.com"]);
        assert!(!ev.organized_elsewhere);
    }

    /// An update that left the invitees as they were read writes no attendee
    /// list, even when it notifies: the list on the server keeps the
    /// organizer's row, which a written list would drop (decision 71a).
    #[test]
    fn an_unchanged_attendee_list_is_not_written() {
        let raw = r#"{
            "id": "ev-own",
            "start": { "dateTime": "2026-05-25T10:00:00.0000000", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T11:00:00.0000000", "timeZone": "UTC" },
            "isAllDay": false,
            "isOrganizer": true,
            "organizer": { "emailAddress": { "address": "toni@example.com" } },
            "attendees": [
              { "type": "required", "status": { "response": "accepted" },
                "emailAddress": { "address": "bob@example.com" } }
            ]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let mut ev = map_event(entry, "primary").unwrap().unwrap();
        ev.send_invitations = true;

        ev.keep_attendees = true;
        let json = serde_json::to_value(event_to_body(&ev).unwrap()).unwrap();
        assert!(json.get("attendees").is_none());

        ev.keep_attendees = false;
        let json = serde_json::to_value(event_to_body(&ev).unwrap()).unwrap();
        assert_eq!(
            json["attendees"][0]["emailAddress"]["address"],
            "bob@example.com"
        );

        // Every invitee removed, and the removed told (decision 74a): an
        // empty list clears Graph's. Not without notifying, which Graph
        // cannot do silently.
        ev.attendees.clear();
        let json = serde_json::to_value(event_to_body(&ev).unwrap()).unwrap();
        assert!(json.get("attendees").is_none(), "{json}");
        ev.clear_attendees = true;
        let json = serde_json::to_value(event_to_body(&ev).unwrap()).unwrap();
        assert_eq!(json["attendees"], serde_json::json!([]));
        ev.send_invitations = false;
        let json = serde_json::to_value(event_to_body(&ev).unwrap()).unwrap();
        assert!(json.get("attendees").is_none(), "{json}");
    }

    /// A series made single sends `null`, the value Graph documents for
    /// taking the pattern away, but only with the host's signal (decision
    /// 280): a missing pattern alone leaves the field out.
    #[test]
    fn a_pattern_is_removed_only_with_the_hosts_signal() {
        let raw = r#"{
            "id": "series-1",
            "start": { "dateTime": "2026-05-25T10:00:00.0000000", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T11:00:00.0000000", "timeZone": "UTC" },
            "isAllDay": false
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let mut ev = map_event(entry, "primary").unwrap().unwrap();
        let json = serde_json::to_value(event_to_body(&ev).unwrap()).unwrap();
        assert!(json.get("recurrence").is_none(), "{json}");

        ev.clear_recurrence = true;
        let json = serde_json::to_value(event_to_body(&ev).unwrap()).unwrap();
        assert_eq!(
            json.get("recurrence"),
            Some(&serde_json::Value::Null),
            "{json}"
        );

        // A pattern still there is written, the signal or not.
        ev.recurrence = Some(EventRecurrence {
            rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
            exceptions: vec![],
            tzid: None,
        });
        let json = serde_json::to_value(event_to_body(&ev).unwrap()).unwrap();
        assert_eq!(json["recurrence"]["pattern"]["type"], "weekly", "{json}");
    }

    #[test]
    fn map_event_with_berlin_timezone_converts_to_utc() {
        let raw = r#"{
            "id": "ev1",
            "subject": "Termin",
            "start": { "dateTime": "2026-05-25T12:00:00.0000000", "timeZone": "Europe/Berlin" },
            "end":   { "dateTime": "2026-05-25T13:00:00.0000000", "timeZone": "Europe/Berlin" },
            "isAllDay": false
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        // Berlin is CEST (UTC+2) on 25 May.
        assert_eq!(
            ev.start,
            Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap()
        );
    }

    #[test]
    fn cancelled_event_stays_visible_and_flagged() {
        // A cancelled meeting stays VISIBLE, flagged `cancelled` — the host
        // suppresses its reminders and the user setting can hide it, but it is
        // not silently deleted (genuine removals arrive as `@removed`).
        // calendarView returns pre-expanded occurrences, so this is safe for
        // both a cancelled single and a cancelled series' instances.
        let raw = r#"{
            "id": "ev-x",
            "subject": "Canceled: Sync",
            "isCancelled": true,
            "isAllDay": false,
            "start": { "dateTime": "2026-05-25T10:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T11:00:00", "timeZone": "UTC" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert!(ev.cancelled);
    }

    #[test]
    fn recurrence_daily_round_trip() {
        let raw = r#"{
            "id": "ev",
            "subject": "Daily",
            "start": { "dateTime": "2026-05-25T10:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T11:00:00", "timeZone": "UTC" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "daily", "interval": 2, "daysOfWeek": []},
                "range": {"type": "noEnd", "startDate": "2026-05-25"}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        let rule = ev.recurrence.unwrap().rrule;
        assert!(rule.contains("FREQ=DAILY"));
        assert!(rule.contains("INTERVAL=2"));
        // Round-trip: parse back.
        let recur = rrule_to_recurrence(&rule, ev.start.date_naive(), None).unwrap();
        assert_eq!(recur.pattern.kind, "daily");
        assert_eq!(recur.pattern.interval, 2);
        assert_eq!(recur.range.kind, "noEnd");
    }

    #[test]
    fn recurrence_weekly_byday_round_trip() {
        let raw = r#"{
            "id": "ev",
            "subject": "Weekly",
            "start": { "dateTime": "2026-05-25T18:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T19:00:00", "timeZone": "UTC" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["monday", "wednesday"]},
                "range": {"type": "numbered", "startDate": "2026-05-25", "numberOfOccurrences": 10}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        let rule = ev.recurrence.unwrap().rrule;
        assert!(rule.contains("FREQ=WEEKLY"));
        assert!(rule.contains("BYDAY=MO,WE"));
        assert!(rule.contains("COUNT=10"));
        let recur = rrule_to_recurrence(&rule, ev.start.date_naive(), None).unwrap();
        assert_eq!(recur.pattern.kind, "weekly");
        assert_eq!(recur.pattern.days_of_week, vec!["monday", "wednesday"]);
        assert_eq!(recur.range.number_of_occurrences, Some(10));
    }

    #[test]
    fn map_event_carries_recurrence_timezone() {
        // Graph attaches the IANA zone to start/end; a timed recurring master
        // must carry it so the frontend expands it DST-correctly.
        let raw = r#"{
            "id": "ev-z",
            "subject": "OAGDU",
            "start": { "dateTime": "2025-12-14T19:00:00", "timeZone": "America/New_York" },
            "end":   { "dateTime": "2025-12-14T20:00:00", "timeZone": "America/New_York" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["sunday"]},
                "range": {"type": "numbered", "startDate": "2025-12-14", "numberOfOccurrences": 12}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(
            ev.recurrence.unwrap().tzid.as_deref(),
            Some("America/New_York")
        );
    }

    #[test]
    fn new_event_to_body_sends_recurrence_timezone() {
        // A zoned recurring master writes its LOCAL wall-clock + IANA zone so
        // Graph expands it DST-correctly on its side.
        let new = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "OAGDU".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2025, 12, 15, 0, 0, 0).unwrap(), // 19:00 EST
            end: Utc.with_ymd_and_hms(2025, 12, 15, 1, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=MONTHLY;BYDAY=2SU".into(),
                exceptions: Vec::new(),
                tzid: Some("America/New_York".into()),
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            uid_to_keep: None,
            keeps_invitation: false,
        };
        let json = serde_json::to_value(new_event_to_body(&new).unwrap()).unwrap();
        assert_eq!(json["start"]["timeZone"], "America/New_York");
        assert_eq!(json["start"]["dateTime"], "2025-12-14T19:00:00");
    }

    /// Decision 46a: an all-day series hands Graph no zone, whatever it stores;
    /// its days go out as midnights in UTC.
    #[test]
    fn new_event_to_body_sends_no_zone_for_an_all_day_series() {
        let midnight = Local.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap();
        let new = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "All-day".into(),
            description: None,
            location: None,
            start: midnight.with_timezone(&Utc),
            end: (midnight + chrono::Duration::days(1)).with_timezone(&Utc),
            all_day: true,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
                exceptions: Vec::new(),
                tzid: Some("America/New_York".into()),
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            uid_to_keep: None,
            keeps_invitation: false,
        };
        let json = serde_json::to_value(new_event_to_body(&new).unwrap()).unwrap();
        for side in ["start", "end"] {
            assert_eq!(json[side]["timeZone"], "UTC", "{side}: {json}");
        }
        assert_eq!(json["start"]["dateTime"], "2026-10-19T00:00:00");
    }

    #[test]
    fn recurrence_absolute_monthly_with_until_round_trip() {
        let raw = r#"{
            "id": "ev",
            "subject": "Bill",
            "start": { "dateTime": "2026-05-15T10:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-15T11:00:00", "timeZone": "UTC" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "absoluteMonthly", "interval": 1, "dayOfMonth": 15},
                "range": {"type": "endDate", "startDate": "2026-05-15", "endDate": "2027-05-15"}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        let rule = ev.recurrence.unwrap().rrule;
        assert!(rule.contains("FREQ=MONTHLY"));
        assert!(rule.contains("BYMONTHDAY=15"));
        assert!(rule.contains("UNTIL=20270515T235959Z"));
    }

    #[test]
    fn recurrence_relative_monthly_drops_to_none_on_read() {
        // Read side: we don't surface a recurrence we can't
        // represent, so the event still appears (just as a
        // standalone). Better than failing the whole listing.
        let raw = r#"{
            "id": "ev",
            "subject": "Last Wed",
            "start": { "dateTime": "2026-05-27T10:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-27T11:00:00", "timeZone": "UTC" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "relativeMonthly", "interval": 1, "daysOfWeek": ["wednesday"], "index": "last"},
                "range": {"type": "noEnd", "startDate": "2026-05-27"}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert!(ev.recurrence.is_none());
        assert_eq!(ev.title, "Last Wed");
    }

    #[test]
    fn rrule_to_recurrence_relative_monthly_errors() {
        // Write side: explicitly refuse so the user notices instead
        // of silently dropping the rule.
        let err = rrule_to_recurrence(
            "FREQ=MONTHLY;BYDAY=WE;BYSETPOS=-1",
            NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, GraphError::Protocol(_)));
    }

    #[test]
    fn new_event_to_body_carries_reminder_and_recurrence() {
        let new = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Yoga".into(),
            description: None,
            location: Some("Studio".into()),
            start: Utc.with_ymd_and_hms(2026, 5, 25, 18, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 25, 19, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
                exceptions: Vec::new(),
                tzid: None,
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![Reminder {
                kind: ReminderKind::Relative { minutes_before: 10 },
                sound: None,
            }],
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            uid_to_keep: None,
            keeps_invitation: false,
        };
        let body = new_event_to_body(&new).unwrap();
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["subject"], "Yoga");
        assert_eq!(json["isReminderOn"], true);
        assert_eq!(json["reminderMinutesBeforeStart"], 10);
        assert_eq!(json["recurrence"]["pattern"]["type"], "weekly");
        assert_eq!(json["recurrence"]["pattern"]["daysOfWeek"][0], "monday");
        assert_eq!(json["location"]["displayName"], "Studio");
        // No attendees written when not notifying.
        assert!(json.get("attendees").is_none());
    }

    #[test]
    fn attendees_written_only_when_notifying() {
        let mut new = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Review".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 25, 11, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: vec!["Alice <alice@example.com>".into()],
            send_invitations: true,
            uid_to_keep: None,
            keeps_invitation: false,
        };
        let json = serde_json::to_value(new_event_to_body(&new).unwrap()).unwrap();
        assert_eq!(
            json["attendees"][0]["emailAddress"]["address"],
            "alice@example.com"
        );
        assert_eq!(json["attendees"][0]["emailAddress"]["name"], "Alice");
        assert_eq!(json["attendees"][0]["type"], "required");

        // Same attendees, notify OFF → Graph would email them, so we omit.
        new.send_invitations = false;
        let json = serde_json::to_value(new_event_to_body(&new).unwrap()).unwrap();
        assert!(json.get("attendees").is_none());
    }
}

/// Stage 5 of DESIGN-series-time-zone.md: a series' repeat range and its
/// default days are read on the clock its start is written on, and a cut ends
/// on the series' clock.
#[cfg(test)]
mod series_clock_tests {
    use super::*;
    use chrono::{Duration, FixedOffset};

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().expect("an instant")
    }

    fn device(hours: i32) -> FixedOffset {
        FixedOffset::east_opt(hours * 3600).expect("an offset")
    }

    fn series(start: &str, all_day: bool, rrule: &str, tzid: Option<&str>) -> NewEvent {
        let start = at(start);
        NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Series".into(),
            description: None,
            location: None,
            start,
            end: start
                + if all_day {
                    Duration::days(1)
                } else {
                    Duration::hours(1)
                },
            all_day,
            recurrence: Some(EventRecurrence {
                rrule: rrule.into(),
                exceptions: Vec::new(),
                tzid: tzid.map(str::to_string),
            }),
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            uid_to_keep: None,
            keeps_invitation: false,
        }
    }

    /// The body Graph gets for a new series, on a device `hours` from UTC.
    /// Graph requires the range to start on the date of the start; every case
    /// holds that.
    fn written(new: &NewEvent, hours: i32) -> serde_json::Value {
        let json =
            serde_json::to_value(new_event_to_body_in(new, &device(hours)).unwrap()).unwrap();
        let start = json["start"]["dateTime"].as_str().expect("a start");
        assert_eq!(
            json["recurrence"]["range"]["startDate"].as_str(),
            start.get(..10),
            "the range starts on the date of the start"
        );
        json
    }

    /// An existing series cut at an occurrence: what "this and all
    /// following" writes for the old one.
    fn cut(start: &str, rrule: &str, tzid: Option<&str>) -> Event {
        let raw = r#"{
            "id": "ev",
            "start": { "dateTime": "2026-01-01T00:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-01-01T01:00:00", "timeZone": "UTC" },
            "isAllDay": false
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let mut ev = map_event(entry, "primary").unwrap().unwrap();
        ev.start = at(start);
        ev.end = ev.start + Duration::hours(1);
        ev.recurrence = Some(EventRecurrence {
            rrule: rrule.into(),
            exceptions: Vec::new(),
            tzid: tzid.map(str::to_string),
        });
        ev.truncate_tail_overrides = true;
        ev
    }

    fn end_date(ev: &Event) -> serde_json::Value {
        let json = serde_json::to_value(event_to_body_in(ev, &device(0)).unwrap()).unwrap();
        json["recurrence"]["range"]["endDate"].clone()
    }

    /// Monday 2 November 2026, 00:30 in Berlin, is Sunday 23:30 UTC: the UTC
    /// date started the range on the Sunday and repeated on Sundays.
    #[test]
    fn a_series_after_midnight_east_of_utc_starts_on_its_own_day() {
        let json = written(
            &series(
                "2026-11-01T23:30:00Z",
                false,
                "FREQ=WEEKLY",
                Some("Europe/Berlin"),
            ),
            0,
        );
        assert_eq!(json["start"]["dateTime"], "2026-11-02T00:30:00");
        assert_eq!(json["recurrence"]["range"]["startDate"], "2026-11-02");
        assert_eq!(json["recurrence"]["pattern"]["daysOfWeek"][0], "monday");
    }

    /// Monday 2 November 2026, 20:00 in New York, is Tuesday 01:00 UTC.
    #[test]
    fn an_evening_series_west_of_utc_starts_on_its_own_day() {
        let json = written(
            &series(
                "2026-11-03T01:00:00Z",
                false,
                "FREQ=WEEKLY",
                Some("America/New_York"),
            ),
            0,
        );
        assert_eq!(json["start"]["dateTime"], "2026-11-02T20:00:00");
        assert_eq!(json["recurrence"]["range"]["startDate"], "2026-11-02");
        assert_eq!(json["recurrence"]["pattern"]["daysOfWeek"][0], "monday");
    }

    /// 31 December 2026, 20:00 in New York, is 1 January 2027 in UTC: the day
    /// of the month and the month come from the series' own day.
    #[test]
    fn monthly_and_yearly_take_their_day_on_the_series_clock() {
        let start = "2027-01-01T01:00:00Z";
        let json = written(
            &series(start, false, "FREQ=MONTHLY", Some("America/New_York")),
            0,
        );
        assert_eq!(json["recurrence"]["range"]["startDate"], "2026-12-31");
        assert_eq!(json["recurrence"]["pattern"]["dayOfMonth"], 31);

        let json = written(
            &series(start, false, "FREQ=YEARLY", Some("America/New_York")),
            0,
        );
        assert_eq!(json["recurrence"]["pattern"]["month"], 12);
        assert_eq!(json["recurrence"]["pattern"]["dayOfMonth"], 31);
    }

    /// An all-day series writes its days as the device names them (48a) and
    /// no zone (46a), whatever zone it stores: its range starts on that day.
    /// The device's midnight of Monday 2 November two hours east of UTC is
    /// Sunday 22:00 UTC.
    #[test]
    fn an_all_day_series_starts_on_the_device_day() {
        for tzid in [None, Some("Europe/Berlin")] {
            let json = written(
                &series("2026-11-01T22:00:00Z", true, "FREQ=WEEKLY", tzid),
                2,
            );
            assert_eq!(json["start"]["dateTime"], "2026-11-02T00:00:00");
            assert_eq!(json["start"]["timeZone"], "UTC");
            assert_eq!(json["recurrence"]["range"]["startDate"], "2026-11-02");
            assert_eq!(json["recurrence"]["pattern"]["daysOfWeek"][0], "monday");
        }
    }

    /// A timed series without a zone tzdata names in exact case is written in
    /// UTC, and its range reads the UTC day, as before.
    #[test]
    fn a_series_without_a_zone_stays_on_utc() {
        for tzid in [None, Some("UTC"), Some("europe/berlin")] {
            let json = written(
                &series("2026-11-01T23:30:00Z", false, "FREQ=WEEKLY", tzid),
                2,
            );
            assert_eq!(json["start"]["timeZone"], "UTC");
            assert_eq!(json["recurrence"]["range"]["startDate"], "2026-11-01");
            assert_eq!(json["recurrence"]["pattern"]["daysOfWeek"][0], "sunday");
        }
    }

    /// A cut at Monday 16 November, 10:00 in Berlin (09:00 UTC), ends the old
    /// series on the Sunday before. The date of the UNTIL, the Monday, kept
    /// the day of the cut in both series.
    #[test]
    fn a_cut_ends_on_the_day_before_on_the_series_clock() {
        let ev = cut(
            "2026-11-02T09:00:00Z",
            "FREQ=WEEKLY;UNTIL=20261116T085959Z",
            Some("Europe/Berlin"),
        );
        assert_eq!(end_date(&ev), "2026-11-15");
    }

    /// West of UTC the UNTIL of an evening cut is already the next UTC day:
    /// Monday 16 November, 20:00 in New York, is Tuesday 01:00 UTC. Its date
    /// kept two days twice.
    #[test]
    fn an_evening_cut_west_of_utc_ends_on_the_day_before() {
        let ev = cut(
            "2026-11-03T01:00:00Z",
            "FREQ=DAILY;UNTIL=20261117T005959Z",
            Some("America/New_York"),
        );
        assert_eq!(end_date(&ev), "2026-11-15");
    }

    /// A daily 02:30 series in Berlin cut on 28 March 2027, when the clock
    /// skips from 02:00 to 03:00: that day's occurrence stands at 03:30, past
    /// the gap (01:30 UTC), so the UNTIL a second before it ends on the 27th.
    #[test]
    fn a_cut_on_a_day_the_clock_skips_the_time_ends_the_day_before() {
        let ev = cut(
            "2027-03-20T01:30:00Z",
            "FREQ=DAILY;UNTIL=20270328T012959Z",
            Some("Europe/Berlin"),
        );
        assert_eq!(end_date(&ev), "2027-03-27");
    }

    /// A series without a zone is cut on the days of UTC.
    #[test]
    fn a_cut_without_a_zone_ends_on_the_utc_day_before() {
        let ev = cut(
            "2026-11-02T09:00:00Z",
            "FREQ=WEEKLY;UNTIL=20261116T085959Z",
            None,
        );
        assert_eq!(end_date(&ev), "2026-11-15");
    }

    /// A typed end at the hour the 105a guard reads as a cut (19:00 New York
    /// in winter, 20:00 in summer) keeps its date, in either spelling: the
    /// repeat field's `…T235959Z` (its own date, 140a) and the read-back end
    /// of the day (107b).
    #[test]
    fn a_typed_end_keeps_its_date_at_the_blind_spot() {
        for (start, until, day) in [
            ("2026-11-03T00:00:00Z", "20261110T235959Z", "2026-11-10"),
            ("2026-11-03T00:00:00Z", "20261111T045959Z", "2026-11-10"),
            ("2026-10-20T00:00:00Z", "20261027T235959Z", "2026-10-27"),
            ("2026-10-20T00:00:00Z", "20261028T035959Z", "2026-10-27"),
        ] {
            let mut ev = cut(
                start,
                &format!("FREQ=DAILY;UNTIL={until}"),
                Some("America/New_York"),
            );
            ev.truncate_tail_overrides = false;
            assert_eq!(end_date(&ev), day, "{start} {until}");
        }
    }

    /// The 106a cut before 8 November's 19:00 in New York is two seconds
    /// before it, and ends the series on the 6th.
    #[test]
    fn a_cut_two_seconds_before_the_blind_spot_ends_the_day_before() {
        let ev = cut(
            "2026-11-03T00:00:00Z",
            "FREQ=DAILY;UNTIL=20261107T235958Z",
            Some("America/New_York"),
        );
        assert_eq!(end_date(&ev), "2026-11-06");
    }

    /// A copy of a cut series is a create without the flag: its end is still
    /// the day it reaches on the series' clock, never the UNTIL's UTC date.
    #[test]
    fn a_copy_of_a_cut_series_writes_the_day_before_the_cut() {
        for (start, until, tzid) in [
            ("2026-11-02T09:00:00Z", "20261116T085959Z", "Europe/Berlin"),
            (
                "2026-11-03T01:00:00Z",
                "20261117T005959Z",
                "America/New_York",
            ),
        ] {
            let json = written(
                &series(
                    start,
                    false,
                    &format!("FREQ=DAILY;UNTIL={until}"),
                    Some(tzid),
                ),
                0,
            );
            assert_eq!(
                json["recurrence"]["range"]["endDate"], "2026-11-15",
                "{tzid}"
            );
        }
    }

    /// A master as a POST or PATCH response carries it: daily 19:00 in New
    /// York in winter, ending on 10 November.
    fn created_master() -> EventEntry {
        serde_json::from_str(
            r#"{
                "id": "ev",
                "start": { "dateTime": "2026-11-02T19:00:00", "timeZone": "America/New_York" },
                "end":   { "dateTime": "2026-11-02T20:00:00", "timeZone": "America/New_York" },
                "isAllDay": false,
                "recurrence": {
                    "pattern": { "type": "daily", "interval": 1 },
                    "range": { "type": "endDate", "startDate": "2026-11-02", "endDate": "2026-11-10" }
                }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn a_create_response_reads_its_end_back_as_sent() {
        let ev = map_event(created_master(), "primary").unwrap().unwrap();
        let rec = ev.recurrence.expect("a master keeps its rule");
        assert_eq!(rec.rrule, "FREQ=DAILY;UNTIL=20261111T045959Z");
        assert_eq!(rec.tzid.as_deref(), Some("America/New_York"));
    }

    #[test]
    fn an_untouched_update_after_a_read_back_keeps_end_date() {
        let ev = map_event(created_master(), "primary").unwrap().unwrap();
        assert!(!ev.truncate_tail_overrides);
        assert_eq!(end_date(&ev), "2026-11-10");
    }

    fn ending(day: &str) -> RecurrenceObject {
        serde_json::from_value(serde_json::json!({
            "pattern": { "type": "daily", "interval": 1 },
            "range": { "type": "endDate", "startDate": "2026-11-02", "endDate": day }
        }))
        .unwrap()
    }

    /// A zone tzdata does not name (a Windows name) reads on UTC, as the
    /// views read it: the digits stay.
    #[test]
    fn a_windows_zone_name_keeps_the_digits() {
        assert_eq!(
            read_back_rrule(
                &ending("2026-11-10"),
                at("2026-11-03T00:00:00Z"),
                false,
                Some("Eastern Standard Time"),
            )
            .as_deref(),
            Some("FREQ=DAILY;UNTIL=20261110T235959Z")
        );
    }

    /// D7: no exact-case gate. The views read `america/new_york` as New
    /// York, and so does the read-back.
    #[test]
    fn a_lowercase_zone_name_reads_its_end_on_the_zone() {
        assert_eq!(
            read_back_rrule(
                &ending("2026-11-10"),
                at("2026-11-03T00:00:00Z"),
                false,
                Some("america/new_york"),
            )
            .as_deref(),
            Some("FREQ=DAILY;UNTIL=20261111T045959Z")
        );
    }

    #[test]
    fn an_all_day_read_back_keeps_the_digits() {
        assert_eq!(
            read_back_rrule(
                &ending("2026-11-10"),
                at("2026-11-03T00:00:00Z"),
                true,
                None,
            )
            .as_deref(),
            Some("FREQ=DAILY;UNTIL=20261110T235959Z")
        );
    }

    /// An end the user typed keeps its day, and so does an all-day cut, which
    /// already names the day before as a date.
    #[test]
    fn an_end_that_is_no_cut_keeps_its_own_date() {
        let mut ev = cut(
            "2026-11-03T01:00:00Z",
            "FREQ=WEEKLY;UNTIL=20261130T235959Z",
            Some("America/New_York"),
        );
        ev.truncate_tail_overrides = false;
        assert_eq!(end_date(&ev), "2026-11-30");

        let mut ev = cut("2026-11-02T00:00:00Z", "FREQ=DAILY;UNTIL=20261115", None);
        ev.all_day = true;
        assert_eq!(end_date(&ev), "2026-11-15");
    }
}
