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
    AttendeeResponse, AttendeeStatus, Calendar, ColorSource, ContainerColor, Event,
    EventRecurrence, NewEvent, Reminder, ReminderKind,
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
        color_label: None,
        id: entry.id,
        name: entry.name,
        color,
        read_only: !entry.can_edit.unwrap_or(true),
        default_sound: None,
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
    /// before parsing; for anything else we route through
    /// `chrono_tz` (Graph speaks the IANA names natively).
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
        let tz: chrono_tz::Tz = self.time_zone.parse().map_err(|e| {
            GraphError::Protocol(format!("unknown timezone '{}': {e:?}", self.time_zone))
        })?;
        let naive = chrono::NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%dT%H:%M:%S")
            .map_err(|e| GraphError::Protocol(format!("graph naive datetime: {e}")))?;
        tz.from_local_datetime(&naive)
            .single()
            .ok_or_else(|| {
                GraphError::Protocol(format!(
                    "ambiguous local time {naive} in {}",
                    self.time_zone
                ))
            })
            .map(|local| local.with_timezone(&Utc))
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
    let recurrence = match entry.recurrence {
        Some(r) => recurrence_to_rrule(&r).map(|rrule| EventRecurrence {
            rrule,
            exceptions: Vec::new(), // Graph models exceptions separately
            // Graph attaches the IANA zone to start/end (it speaks IANA
            // natively); carry it for a timed recurring master so the frontend
            // expands DST-correctly. All-day + plain UTC stay on the UTC path.
            tzid: if entry.is_all_day {
                None
            } else {
                Some(entry.start.time_zone.clone()).filter(|t| !t.is_empty() && t != "UTC")
            },
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

    // Attendees: the editable flat list ("Name <email>" / bare email)
    // plus per-attendee RSVP state.
    let mut attendees = Vec::new();
    let mut attendee_responses = Vec::new();
    for a in entry.attendees {
        let Some(email) = a
            .email_address
            .as_ref()
            .and_then(|e| e.address.clone())
            .filter(|s| !s.trim().is_empty())
        else {
            continue;
        };
        let name = a
            .email_address
            .and_then(|e| e.name)
            .filter(|n| !n.trim().is_empty());
        attendees.push(format_attendee(name.as_deref(), &email));
        attendee_responses.push(AttendeeResponse {
            status: a
                .status
                .and_then(|s| s.response)
                .as_deref()
                .map(graph_status)
                .unwrap_or_default(),
            name,
            email,
        });
    }
    let organizer = entry
        .organizer
        .and_then(|o| o.email_address)
        .and_then(|e| e.address)
        .filter(|s| !s.trim().is_empty());

    Ok(Some(Event {
        send_invitations: false,
        truncate_tail_overrides: false,
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
        attendees,
        created_at: created,
        updated_at: updated,
        etag: entry.etag,
        organizer,
        attendee_responses,
        cancelled,
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

/// Render an attendee for the editable flat list — `"Name <email>"`
/// when a distinct display name exists, else the bare email.
fn format_attendee(name: Option<&str>, email: &str) -> String {
    match name {
        Some(n) if n.trim() != email => format!("{} <{}>", n.trim(), email),
        _ => email.to_string(),
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

/// Inverse: parse an RRULE body into Graph's structured form.
/// Returns `Err(Protocol)` when the rule uses features Graph can't
/// represent — e.g. BYSETPOS without a known BYDAY pattern.
pub fn rrule_to_recurrence(rrule: &str, start: DateTime<Utc>) -> GraphResult<RecurrenceObject> {
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
                // Default to the weekday of `start`.
                pattern.days_of_week.push(weekday_to_name(start.weekday()));
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
                pattern.day_of_month = Some(start.day());
            }
        }
        "YEARLY" => {
            pattern.kind = "absoluteYearly".into();
            pattern.month = parts
                .get("BYMONTH")
                .and_then(|s| s.parse().ok())
                .or(Some(start.month()));
            pattern.day_of_month = parts
                .get("BYMONTHDAY")
                .and_then(|s| s.parse().ok())
                .or(Some(start.day()));
        }
        other => {
            return Err(GraphError::Protocol(format!("unsupported FREQ: {other}")));
        }
    }

    let mut range = RecurrenceRange {
        kind: "noEnd".into(),
        start_date: Some(start.format("%Y-%m-%d").to_string()),
        end_date: None,
        number_of_occurrences: None,
    };
    if let Some(until) = parts.get("UNTIL") {
        // RFC 5545: compact UTC date-time. Graph wants YYYY-MM-DD.
        if let Some(date_part) = until.get(..8) {
            if let Ok(d) = NaiveDate::parse_from_str(date_part, "%Y%m%d") {
                range.kind = "endDate".into();
                range.end_date = Some(d.format("%Y-%m-%d").to_string());
            }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<RecurrenceObject>,
    /// Microsoft Graph QUIRK: putting attendees in the body makes Graph
    /// EMAIL them on create/update — there is no per-request suppress. So we
    /// write this ONLY when the user opted to notify; declining means Graph
    /// stores no attendee list (documented limitation).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<GraphAttendeeWrite>,
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
    let tzid = new.recurrence.as_ref().and_then(|r| r.tzid.as_deref());
    let body = EventWriteBody {
        subject: Some(new.title.clone()),
        body: new.description.clone().map(|c| EventBodyWrite {
            content_type: "text".into(),
            content: c,
        }),
        start: write_datetime(new.start, new.all_day, tzid),
        end: write_datetime(new.end, new.all_day, tzid),
        is_all_day: new.all_day,
        location: new
            .location
            .clone()
            .map(|l| EventLocationWrite { display_name: l }),
        is_reminder_on: first_reminder_minutes(&new.reminders).is_some(),
        reminder_minutes_before_start: first_reminder_minutes(&new.reminders),
        recurrence: match new.recurrence.as_ref() {
            Some(r) => Some(rrule_to_recurrence(&r.rrule, new.start)?),
            None => None,
        },
        attendees: attendees_to_write(&new.attendees, new.send_invitations),
    };
    Ok(body)
}

pub fn event_to_body(ev: &Event) -> GraphResult<EventWriteBody> {
    let tzid = ev.recurrence.as_ref().and_then(|r| r.tzid.as_deref());
    Ok(EventWriteBody {
        subject: Some(ev.title.clone()),
        body: ev.description.clone().map(|c| EventBodyWrite {
            content_type: "text".into(),
            content: c,
        }),
        start: write_datetime(ev.start, ev.all_day, tzid),
        end: write_datetime(ev.end, ev.all_day, tzid),
        is_all_day: ev.all_day,
        location: ev
            .location
            .clone()
            .map(|l| EventLocationWrite { display_name: l }),
        is_reminder_on: first_reminder_minutes(&ev.reminders).is_some(),
        reminder_minutes_before_start: first_reminder_minutes(&ev.reminders),
        recurrence: match ev.recurrence.as_ref() {
            Some(r) => Some(rrule_to_recurrence(&r.rrule, ev.start)?),
            None => None,
        },
        attendees: attendees_to_write(&ev.attendees, ev.send_invitations),
    })
}

fn write_datetime(when: DateTime<Utc>, all_day: bool, tzid: Option<&str>) -> GraphDateTimeWrite {
    if all_day {
        // For all-day events Graph still expects a `dateTime` (not a
        // `date`) at 00:00:00 in the given zone. The DAY must be the
        // user's LOCAL calendar day — formatting the UTC instant would
        // emit the UTC day, one early for users east of UTC (the
        // boundary instants are local midnights expressed in UTC). The
        // internal end is already exclusive, as Graph requires.
        GraphDateTimeWrite {
            date_time: when
                .with_timezone(&Local)
                .format("%Y-%m-%dT00:00:00")
                .to_string(),
            time_zone: "UTC".into(),
        }
    } else if let Some((name, tz)) =
        tzid.and_then(|t| t.parse::<chrono_tz::Tz>().ok().map(|tz| (t, tz)))
    {
        // A zoned recurring master: send the LOCAL wall-clock + its IANA zone so
        // Graph expands the series DST-correctly on its side (Graph accepts IANA
        // names). A one-off is an exact instant, so it stays on the UTC branch.
        GraphDateTimeWrite {
            date_time: when
                .with_timezone(&tz)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            time_zone: name.to_string(),
        }
    } else {
        GraphDateTimeWrite {
            date_time: when.format("%Y-%m-%dT%H:%M:%S").to_string(),
            time_zone: "UTC".into(),
        }
    }
}

fn first_reminder_minutes(reminders: &[Reminder]) -> Option<i64> {
    // Graph models per-event reminders as a single (minutes,
    // boolean) pair — there's no overrides array like Google's.
    // We take the first Relative reminder; everything else
    // (Absolute, AppStart, Email) stays local-only because Graph
    // has nowhere to put it.
    reminders.iter().find_map(|r| match r.kind {
        ReminderKind::Relative { minutes_before } => Some(minutes_before),
        _ => None,
    })
}

// ────────────────────────────────────────────────────────────────────────────
// Microsoft To Do — Phase 6e.2
// ────────────────────────────────────────────────────────────────────────────
//
// Microsoft consolidated all task scenarios under To Do; the legacy
// Outlook-tasks endpoint was deprecated in 2020. We hit
// `/me/todo/lists` (list listing) and `/me/todo/lists/{id}/tasks`
// (task CRUD).
//
// Two model mismatches worth flagging:
//
//   - `delete_task(task_id)` in cal-core takes only the task id, but
//     Graph's DELETE URL needs both list id and task id. We encode
//     the list id into the task id with a `|` separator
//     (`{listId}|{taskId}`). Graph ids are URL-safe base64-ish and
//     never contain `|`, so the split is unambiguous.
//
//   - Graph carries `startDateTime` and `dueDateTime` separately,
//     which map cleanly onto Aperio's `scheduled_date` /
//     `scheduled_time` and `deadline_date` / `deadline_time` after
//     the migration 0006 task-time refactor. The old on/by enum is
//     gone — every deadline is "by" semantics — so no wire changes
//     are needed on top of the column rename in this file.

use cal_core::{
    apply_task_extras, decode_payload, encode_payload, extras_for_task, recurrence_needs_extras,
    NewTask, Task, TaskList, TaskPriority, TaskStatus,
};

/// DESIGN §9.12: Graph strips HTML from the task body, so the visible-block
/// channel can't survive — the Aperio-Extras blob rides an **open
/// extension** instead. The name is the conventional reverse-DNS form.
pub const APERIO_EXTENSION_NAME: &str = "com.aperio.extras";
const OPEN_EXTENSION_ODATA_TYPE: &str = "microsoft.graph.openTypeExtension";

/// One entry in a todoTask's `extensions` collection (read side). The
/// custom `aperioExtras` property sits at the top level of an open
/// extension object alongside the well-known `extensionName`.
#[derive(Debug, Deserialize)]
pub struct OpenExtensionEntry {
    #[serde(default, rename = "extensionName")]
    pub extension_name: Option<String>,
    #[serde(default, rename = "aperioExtras")]
    pub aperio_extras: Option<String>,
}

/// Open-extension body for a write (create or the update reconcile). Graph
/// requires the `@odata.type` discriminator on each extension object.
#[derive(Debug, Serialize)]
pub struct OpenExtensionWrite {
    #[serde(rename = "@odata.type")]
    pub odata_type: &'static str,
    #[serde(rename = "extensionName")]
    pub extension_name: &'static str,
    #[serde(rename = "aperioExtras")]
    pub aperio_extras: String,
}

/// Build the Aperio open-extension for a task, or `None` when there's
/// nothing non-native to carry.
pub fn aperio_extension_write(
    recurrence: Option<&cal_core::TaskRecurrence>,
    resurface_date: Option<NaiveDate>,
    series_id: Option<&str>,
    effort: cal_core::TaskEffort,
    deadline_reminder_days: Option<i64>,
) -> Option<OpenExtensionWrite> {
    let extras = extras_for_task(
        recurrence,
        resurface_date,
        series_id,
        effort,
        deadline_reminder_days,
    );
    let payload = encode_payload(&extras)?;
    Some(OpenExtensionWrite {
        odata_type: OPEN_EXTENSION_ODATA_TYPE,
        extension_name: APERIO_EXTENSION_NAME,
        aperio_extras: payload,
    })
}

// ── Task list listing ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TodoListResponse {
    #[serde(default)]
    pub value: Vec<TodoListEntry>,
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TodoListEntry {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// `true` when the signed-in user owns this list (i.e. it lives
    /// in their mailbox). `false` for shared lists where the user
    /// is just a guest — we surface those as `read_only` so the UI
    /// doesn't expose write affordances we can't fulfil.
    #[serde(default, rename = "isOwner")]
    pub is_owner: Option<bool>,
}

pub fn map_task_list(entry: TodoListEntry) -> TaskList {
    TaskList {
        color_label: None,
        id: entry.id,
        name: entry.display_name,
        color: None,
        default_sound: None,
        // To Do task lists are independent — no embedded-in-calendar
        // semantics like CalDAV's VTODO-in-VCALENDAR pattern.
        embedded_in_calendar: None,
        parent_id: None,
        read_only: !entry.is_owner.unwrap_or(true),
    }
}

// ── Task reads ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TodoTaskResponse {
    #[serde(default)]
    pub value: Vec<TodoTaskEntry>,
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// One page of a `todo/lists/{id}/tasks/delta` response. Like the event
/// delta, `value` is kept as raw JSON so a tombstone
/// (`{ "id": "…", "@removed": { … } }`) can be told apart from a live
/// task before deserialising. Intermediate pages carry `@odata.nextLink`;
/// the final page carries `@odata.deltaLink`, the cursor for next round.
#[derive(Debug, Deserialize)]
pub struct TodoTaskDeltaResponse {
    #[serde(default)]
    pub value: Vec<serde_json::Value>,
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    #[serde(default, rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TodoTaskEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub body: Option<TodoTaskBody>,
    /// `low` / `normal` / `high` — Aperio's `Low` / `Medium` / `High`.
    #[serde(default)]
    pub importance: Option<String>,
    /// `notStarted` / `inProgress` / `completed` / `waitingOnOthers`
    /// / `deferred`. Mapped onto Aperio's narrower set; rare values
    /// fall through to Open.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "dueDateTime")]
    pub due_date_time: Option<GraphDateTime>,
    #[serde(default, rename = "startDateTime")]
    pub start_date_time: Option<GraphDateTime>,
    #[serde(default, rename = "completedDateTime")]
    pub completed_date_time: Option<GraphDateTime>,
    #[serde(default, rename = "reminderDateTime")]
    pub reminder_date_time: Option<GraphDateTime>,
    #[serde(default, rename = "isReminderOn")]
    pub is_reminder_on: bool,
    #[serde(default)]
    pub recurrence: Option<RecurrenceObject>,
    #[serde(default, rename = "createdDateTime")]
    pub created_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "lastModifiedDateTime")]
    pub last_modified_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "@odata.etag")]
    pub etag: Option<String>,
    /// Open extensions, present when the read expanded them (DESIGN §9.12).
    /// Absent in a plain delta page ⇒ "no extras", which degrades cleanly.
    #[serde(default)]
    pub extensions: Vec<OpenExtensionEntry>,
}

#[derive(Debug, Deserialize)]
pub struct TodoTaskBody {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default, rename = "contentType")]
    pub content_type: Option<String>,
}

pub fn map_task(entry: TodoTaskEntry, list_id: &str) -> GraphResult<Task> {
    let priority = match entry.importance.as_deref() {
        Some("low") => TaskPriority::Low,
        Some("high") => TaskPriority::High,
        _ => TaskPriority::Medium,
    };
    let status = match entry.status.as_deref() {
        Some("completed") => TaskStatus::Completed,
        Some("inProgress") => TaskStatus::InProgress,
        Some("deferred") => TaskStatus::Cancelled,
        // `waitingOnOthers` and `notStarted` (and missing) → Open.
        // `waitingOnOthers` doesn't have a clean Aperio counterpart
        // and conflating it with Open at least keeps the task on
        // the user's radar; promoting it to Cancelled would have
        // been wrong.
        _ => TaskStatus::Open,
    };
    let description = entry.body.and_then(|b| {
        let raw = b.content?;
        if raw.is_empty() {
            return None;
        }
        // `contentType: html` is the Graph default. The UI side
        // treats description as plain text — stripping HTML tags
        // server-side keeps the round-trip lossy but readable.
        // Aperio's TaskDialog doesn't render HTML, so a `<p>x</p>`
        // showing up verbatim would be worse than a plain "x".
        if matches!(b.content_type.as_deref(), Some("html") | Some("HTML")) {
            Some(strip_html_tags(&raw))
        } else {
            Some(raw)
        }
    });

    let (deadline_date, deadline_time) = entry
        .due_date_time
        .as_ref()
        .map(|d| d.to_utc())
        .transpose()?
        .map(|dt| (Some(dt.date_naive()), Some(dt.time())))
        .unwrap_or((None, None));
    let (scheduled_date, scheduled_time) = entry
        .start_date_time
        .as_ref()
        .map(|d| d.to_utc())
        .transpose()?
        .map(|dt| (Some(dt.date_naive()), Some(dt.time())))
        .unwrap_or((None, None));

    let reminders = if entry.is_reminder_on {
        match &entry.reminder_date_time {
            Some(rd) => vec![Reminder {
                kind: ReminderKind::Absolute { at: rd.to_utc()? },
                sound: None,
            }],
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Native patternedRecurrence covers plain scheduled rules; the open
    // extension (DESIGN §9.12) overlays the on-demand axes / resurface_date /
    // series_id, overriding the native recurrence when it carries one.
    let mut recurrence = entry
        .recurrence
        .as_ref()
        .and_then(graph_recurrence_to_task_recurrence);
    let mut resurface_date = None;
    let mut series_id = None;
    let mut effort = cal_core::TaskEffort::default();
    let mut deadline_reminder_days = None;
    if let Some(extras) = entry
        .extensions
        .iter()
        .find(|e| e.extension_name.as_deref() == Some(APERIO_EXTENSION_NAME))
        .and_then(|e| e.aperio_extras.as_deref())
        .and_then(decode_payload)
    {
        apply_task_extras(
            &extras,
            &mut recurrence,
            &mut resurface_date,
            &mut series_id,
            &mut effort,
            &mut deadline_reminder_days,
        );
    }

    let completed_at = entry
        .completed_date_time
        .as_ref()
        .map(|d| d.to_utc())
        .transpose()?;
    let created = entry.created_date_time.unwrap_or_else(Utc::now);
    let updated = entry.last_modified_date_time.unwrap_or(created);

    // Pack the list id into the task id so `delete_task(task_id)`
    // — which doesn't carry the list id in its signature — can find
    // its way back to the right DELETE URL.
    let composite_id = format!("{}|{}", list_id, entry.id);

    Ok(Task {
        assignees: Vec::new(),
        id: composite_id,
        list_id: list_id.to_string(),
        title: entry.title,
        description,
        status,
        priority,
        effort,
        scheduled_date,
        scheduled_time,
        deadline_date,
        deadline_time,
        deadline_reminder_days,
        resurface_date,
        series_id,
        recurrence,
        // Microsoft To Do has no task→task parent link: its
        // "checklistItems" are read-only strings without a write API, so
        // subtasks can't round-trip. The plugin manifest declares
        // `subtasks: false` so the UI never offers them on Graph lists.
        parent_id: None,
        section_id: None,
        color_label: None,
        reminders,
        sound: None,
        created_at: created,
        updated_at: updated,
        completed_at,
        etag: entry.etag,
    })
}

/// Split the composite `{list_id}|{task_id}` back into its parts.
/// Used by `update_task` / `delete_task` in the api layer.
pub fn split_task_id(composite: &str) -> (String, String) {
    match composite.split_once('|') {
        Some((list, task)) => (list.to_string(), task.to_string()),
        None => {
            // Compat: legacy task id without a packed list id. We
            // can't route without it; the api layer surfaces this
            // as Protocol.
            (String::new(), composite.to_string())
        }
    }
}

/// Very small HTML tag stripper for description bodies. Replaces
/// `<br>` and `</p>` with newlines, strips everything else between
/// `<` and `>`. Not a full HTML parser — but Graph emits tame
/// markup for tasks (mostly `<p>` and `<br>`), and the alternative
/// of pulling in an html-parser dependency for one field is overkill.
fn strip_html_tags(s: &str) -> String {
    let normalized = s
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n");
    let mut out = String::with_capacity(normalized.len());
    let mut in_tag = false;
    for c in normalized.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

// ── Task writes ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct TodoTaskWriteBody {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<TodoTaskBodyWrite>,
    pub importance: &'static str,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none", rename = "dueDateTime")]
    pub due_date_time: Option<GraphDateTimeWrite>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "startDateTime")]
    pub start_date_time: Option<GraphDateTimeWrite>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "reminderDateTime")]
    pub reminder_date_time: Option<GraphDateTimeWrite>,
    #[serde(rename = "isReminderOn")]
    pub is_reminder_on: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<RecurrenceObject>,
    /// Inline open extensions — honoured by Graph on **create** (POST). On
    /// update the extension is reconciled with a separate request, so the
    /// update body leaves this empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<OpenExtensionWrite>,
}

#[derive(Debug, Serialize)]
pub struct TodoTaskBodyWrite {
    pub content: String,
    #[serde(rename = "contentType")]
    pub content_type: &'static str,
}

pub fn new_task_to_body(new: &NewTask) -> GraphResult<TodoTaskWriteBody> {
    Ok(TodoTaskWriteBody {
        title: new.title.clone(),
        body: new
            .description
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| TodoTaskBodyWrite {
                content: s.to_string(),
                content_type: "text",
            }),
        importance: priority_to_importance(new.priority),
        status: task_status_to_graph(new.status),
        due_date_time: build_due_datetime(new.deadline_date, new.deadline_time),
        start_date_time: build_start_datetime(new.scheduled_date, new.scheduled_time),
        reminder_date_time: first_absolute_reminder_at(&new.reminders).map(|at| {
            GraphDateTimeWrite {
                date_time: at.format("%Y-%m-%dT%H:%M:%S").to_string(),
                time_zone: "UTC".into(),
            }
        }),
        is_reminder_on: first_absolute_reminder_at(&new.reminders).is_some(),
        // Only a plain scheduled rule rides Graph's native recurrence; a
        // backlog/on-demand rule rides the open extension below.
        recurrence: new
            .recurrence
            .as_ref()
            .filter(|r| !recurrence_needs_extras(r))
            .map(task_recurrence_to_graph)
            .transpose()?,
        // Create honours inline extensions.
        extensions: aperio_extension_write(
            new.recurrence.as_ref(),
            new.resurface_date,
            new.series_id.as_deref(),
            new.effort,
            new.deadline_reminder_days,
        )
        .into_iter()
        .collect(),
    })
}

pub fn task_to_body(task: &Task) -> GraphResult<TodoTaskWriteBody> {
    Ok(TodoTaskWriteBody {
        title: task.title.clone(),
        body: task
            .description
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| TodoTaskBodyWrite {
                content: s.to_string(),
                content_type: "text",
            }),
        importance: priority_to_importance(task.priority),
        status: task_status_to_graph(task.status),
        due_date_time: build_due_datetime(task.deadline_date, task.deadline_time),
        start_date_time: build_start_datetime(task.scheduled_date, task.scheduled_time),
        reminder_date_time: first_absolute_reminder_at(&task.reminders).map(|at| {
            GraphDateTimeWrite {
                date_time: at.format("%Y-%m-%dT%H:%M:%S").to_string(),
                time_zone: "UTC".into(),
            }
        }),
        is_reminder_on: first_absolute_reminder_at(&task.reminders).is_some(),
        recurrence: task
            .recurrence
            .as_ref()
            .filter(|r| !recurrence_needs_extras(r))
            .map(task_recurrence_to_graph)
            .transpose()?,
        // Update leaves extensions empty — Graph ignores inline extensions on
        // PATCH, so the api layer reconciles the extension separately.
        extensions: Vec::new(),
    })
}

fn priority_to_importance(p: TaskPriority) -> &'static str {
    match p {
        TaskPriority::Low => "low",
        TaskPriority::Medium => "normal",
        TaskPriority::High => "high",
    }
}

fn task_status_to_graph(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Open => "notStarted",
        TaskStatus::InProgress => "inProgress",
        TaskStatus::Completed => "completed",
        TaskStatus::Cancelled => "deferred",
    }
}

fn build_due_datetime(
    date: Option<NaiveDate>,
    time: Option<chrono::NaiveTime>,
) -> Option<GraphDateTimeWrite> {
    let date = date?;
    let t = time.unwrap_or_else(|| chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    let naive = date.and_time(t);
    Some(GraphDateTimeWrite {
        date_time: naive.format("%Y-%m-%dT%H:%M:%S").to_string(),
        time_zone: "UTC".into(),
    })
}

fn build_start_datetime(
    date: Option<NaiveDate>,
    time: Option<chrono::NaiveTime>,
) -> Option<GraphDateTimeWrite> {
    let date = date?;
    let t = time.unwrap_or_else(|| chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    let naive = date.and_time(t);
    Some(GraphDateTimeWrite {
        date_time: naive.format("%Y-%m-%dT%H:%M:%S").to_string(),
        time_zone: "UTC".into(),
    })
}

fn first_absolute_reminder_at(reminders: &[Reminder]) -> Option<DateTime<Utc>> {
    // Graph models a single absolute reminder per todoTask
    // (`reminderDateTime` + `isReminderOn`). We pull the first
    // Absolute entry. Relative reminders don't map — To Do's UI
    // doesn't expose "remind X minutes before" for tasks.
    reminders.iter().find_map(|r| match &r.kind {
        ReminderKind::Absolute { at } => Some(*at),
        _ => None,
    })
}

// ── Task recurrence ⇄ Graph ─────────────────────────────────────────────
//
// To Do uses the same PatternedRecurrence shape as events — pattern
// + range. We map cal-core's `TaskRecurrence` (a simple frequency +
// interval + optional BYDAY / BYMONTHDAY + end) to that.

pub fn task_recurrence_to_graph(rec: &cal_core::TaskRecurrence) -> GraphResult<RecurrenceObject> {
    use cal_core::{RecurrenceEnd, RecurrenceFrequency};
    let interval = rec.interval.max(1);
    let pattern = match rec.frequency {
        RecurrenceFrequency::Daily => RecurrencePattern {
            kind: "daily".into(),
            interval,
            days_of_week: Vec::new(),
            day_of_month: None,
            month: None,
            index: None,
            first_day_of_week: None,
        },
        RecurrenceFrequency::Weekly => RecurrencePattern {
            kind: "weekly".into(),
            interval,
            days_of_week: rec
                .day_of_week
                .as_ref()
                .map(|days| {
                    days.iter()
                        .map(|d| weekday_full_name(*d).to_string())
                        .collect()
                })
                .unwrap_or_default(),
            day_of_month: None,
            month: None,
            index: None,
            first_day_of_week: None,
        },
        RecurrenceFrequency::Monthly => RecurrencePattern {
            kind: "absoluteMonthly".into(),
            interval,
            days_of_week: Vec::new(),
            day_of_month: rec.day_of_month.map(|d| d as u32),
            month: None,
            index: None,
            first_day_of_week: None,
        },
        RecurrenceFrequency::Yearly => {
            // To Do's absoluteYearly pattern needs both month and
            // dayOfMonth. Aperio's TaskRecurrence model doesn't
            // carry a month for yearly — we lean on `day_of_month`
            // alone, which surfaces as "12 of January" by default.
            // A future Aperio TaskRecurrence v2 should add month.
            RecurrencePattern {
                kind: "absoluteYearly".into(),
                interval,
                days_of_week: Vec::new(),
                day_of_month: rec.day_of_month.map(|d| d as u32),
                month: Some(1),
                index: None,
                first_day_of_week: None,
            }
        }
    };
    let range = match &rec.end {
        Some(RecurrenceEnd::OnDate { date }) => RecurrenceRange {
            kind: "endDate".into(),
            start_date: Some(Utc::now().date_naive().format("%Y-%m-%d").to_string()),
            end_date: Some(date.format("%Y-%m-%d").to_string()),
            number_of_occurrences: None,
        },
        Some(RecurrenceEnd::After { occurrences }) => RecurrenceRange {
            kind: "numbered".into(),
            start_date: Some(Utc::now().date_naive().format("%Y-%m-%d").to_string()),
            end_date: None,
            number_of_occurrences: Some(*occurrences),
        },
        Some(RecurrenceEnd::Never) | None => RecurrenceRange {
            kind: "noEnd".into(),
            start_date: Some(Utc::now().date_naive().format("%Y-%m-%d").to_string()),
            end_date: None,
            number_of_occurrences: None,
        },
    };
    Ok(RecurrenceObject { pattern, range })
}

pub fn graph_recurrence_to_task_recurrence(
    rec: &RecurrenceObject,
) -> Option<cal_core::TaskRecurrence> {
    use cal_core::{RecurrenceEnd, RecurrenceFrequency, TaskRecurrence};
    let frequency = match rec.pattern.kind.as_str() {
        "daily" => RecurrenceFrequency::Daily,
        "weekly" => RecurrenceFrequency::Weekly,
        "absoluteMonthly" => RecurrenceFrequency::Monthly,
        "absoluteYearly" => RecurrenceFrequency::Yearly,
        // Relative monthly/yearly aren't in cal-core's TaskRecurrence
        // model — same caveat as for events. Read-as-no-recurrence.
        _ => return None,
    };
    let day_of_week = if matches!(frequency, RecurrenceFrequency::Weekly)
        && !rec.pattern.days_of_week.is_empty()
    {
        Some(
            rec.pattern
                .days_of_week
                .iter()
                .filter_map(|s| day_name_to_weekday(s))
                .collect::<Vec<_>>(),
        )
        .filter(|v: &Vec<_>| !v.is_empty())
    } else {
        None
    };
    let day_of_month = if matches!(
        frequency,
        RecurrenceFrequency::Monthly | RecurrenceFrequency::Yearly
    ) {
        rec.pattern.day_of_month.and_then(|d| u8::try_from(d).ok())
    } else {
        None
    };
    let end = match rec.range.kind.as_str() {
        "numbered" => rec
            .range
            .number_of_occurrences
            .map(|n| RecurrenceEnd::After { occurrences: n }),
        "endDate" => rec
            .range
            .end_date
            .as_ref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .map(|date| RecurrenceEnd::OnDate { date }),
        _ => Some(RecurrenceEnd::Never),
    };
    Some(TaskRecurrence {
        frequency,
        interval: rec.pattern.interval.max(1),
        day_of_week,
        day_of_month,
        anchor: Default::default(),
        placement: Default::default(),
        fixed_dates: None,
        end,
    })
}

fn weekday_full_name(w: cal_core::Weekday) -> &'static str {
    match w {
        cal_core::Weekday::Monday => "monday",
        cal_core::Weekday::Tuesday => "tuesday",
        cal_core::Weekday::Wednesday => "wednesday",
        cal_core::Weekday::Thursday => "thursday",
        cal_core::Weekday::Friday => "friday",
        cal_core::Weekday::Saturday => "saturday",
        cal_core::Weekday::Sunday => "sunday",
    }
}

fn day_name_to_weekday(s: &str) -> Option<cal_core::Weekday> {
    Some(match s.to_ascii_lowercase().as_str() {
        "monday" => cal_core::Weekday::Monday,
        "tuesday" => cal_core::Weekday::Tuesday,
        "wednesday" => cal_core::Weekday::Wednesday,
        "thursday" => cal_core::Weekday::Thursday,
        "friday" => cal_core::Weekday::Friday,
        "saturday" => cal_core::Weekday::Saturday,
        "sunday" => cal_core::Weekday::Sunday,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let w = write_datetime(local_midnight(2026, 6, 10), true, None);
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
        let back = write_datetime(instant, true, None);
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
        assert_eq!(ev.attendees[0], "The Boss <boss@example.com>");
        assert_eq!(ev.attendees[2], "nobody@example.com");
        assert_eq!(ev.attendee_responses.len(), 3);
        // organizer → implicit accept; tentativelyAccepted → Tentative;
        // none → NeedsAction.
        assert_eq!(ev.attendee_responses[0].status, AttendeeStatus::Accepted);
        assert_eq!(ev.attendee_responses[1].status, AttendeeStatus::Tentative);
        assert_eq!(ev.attendee_responses[2].status, AttendeeStatus::NeedsAction);
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
        let recur = rrule_to_recurrence(&rule, ev.start).unwrap();
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
        let recur = rrule_to_recurrence(&rule, ev.start).unwrap();
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
        };
        let json = serde_json::to_value(new_event_to_body(&new).unwrap()).unwrap();
        assert_eq!(json["start"]["timeZone"], "America/New_York");
        assert_eq!(json["start"]["dateTime"], "2025-12-14T19:00:00");
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
            Utc.with_ymd_and_hms(2026, 5, 27, 10, 0, 0).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, GraphError::Protocol(_)));
    }

    #[test]
    fn new_event_to_body_carries_reminder_and_recurrence() {
        let new = NewEvent {
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

    // ── Microsoft To Do tests ─────────────────────────────────────────

    #[test]
    fn task_list_is_owner_false_maps_to_read_only() {
        let entry: TodoListEntry =
            serde_json::from_str(r##"{"id":"L1","displayName":"Shared list","isOwner":false}"##)
                .unwrap();
        let list = map_task_list(entry);
        assert!(list.read_only);
        assert_eq!(list.name, "Shared list");
        assert!(list.embedded_in_calendar.is_none());
    }

    #[test]
    fn task_list_missing_is_owner_defaults_to_writable() {
        let entry: TodoListEntry =
            serde_json::from_str(r##"{"id":"L1","displayName":"My"}"##).unwrap();
        let list = map_task_list(entry);
        assert!(!list.read_only);
    }

    #[test]
    fn map_task_packs_list_id_into_task_id() {
        let entry: TodoTaskEntry = serde_json::from_str(
            r##"{
                "id": "T1",
                "title": "Buy milk",
                "importance": "high",
                "status": "notStarted"
            }"##,
        )
        .unwrap();
        let task = map_task(entry, "LIST-A").unwrap();
        assert_eq!(task.id, "LIST-A|T1");
        assert_eq!(task.list_id, "LIST-A");
        assert_eq!(task.title, "Buy milk");
        assert_eq!(task.priority, TaskPriority::High);
        assert_eq!(task.status, TaskStatus::Open);
    }

    #[test]
    fn map_task_translates_due_date_and_html_body() {
        let entry: TodoTaskEntry = serde_json::from_str(
            r##"{
                "id": "T2",
                "title": "Write report",
                "body": { "content": "<p>line 1</p><p>line 2</p>", "contentType": "html" },
                "importance": "normal",
                "status": "inProgress",
                "dueDateTime": { "dateTime": "2026-06-15T17:00:00", "timeZone": "UTC" }
            }"##,
        )
        .unwrap();
        let task = map_task(entry, "L").unwrap();
        assert_eq!(task.status, TaskStatus::InProgress);
        let body = task.description.unwrap();
        assert!(body.contains("line 1"));
        assert!(body.contains("line 2"));
        assert_eq!(task.deadline_date.unwrap().to_string(), "2026-06-15");
        assert_eq!(task.scheduled_date, None);
    }

    #[test]
    fn map_task_absolute_reminder_becomes_reminder_at() {
        let entry: TodoTaskEntry = serde_json::from_str(
            r##"{
                "id": "T3",
                "title": "Call back",
                "isReminderOn": true,
                "reminderDateTime": { "dateTime": "2026-05-21T09:30:00", "timeZone": "UTC" }
            }"##,
        )
        .unwrap();
        let task = map_task(entry, "L").unwrap();
        assert_eq!(task.reminders.len(), 1);
        match &task.reminders[0].kind {
            ReminderKind::Absolute { at } => {
                assert_eq!(at.to_rfc3339(), "2026-05-21T09:30:00+00:00");
            }
            other => panic!("expected Absolute reminder, got {other:?}"),
        }
    }

    #[test]
    fn split_task_id_separates_list_and_task() {
        let (list, task) = split_task_id("LIST-A|TASK-1");
        assert_eq!(list, "LIST-A");
        assert_eq!(task, "TASK-1");
    }

    #[test]
    fn split_task_id_legacy_unprefixed_returns_empty_list() {
        // Defensive path — an id that lost its list prefix bubbles
        // out as an empty list id so the api layer can surface a
        // clear Protocol error instead of generating a 404 URL.
        let (list, task) = split_task_id("BARE-TASK");
        assert!(list.is_empty());
        assert_eq!(task, "BARE-TASK");
    }

    #[test]
    fn new_task_to_body_serialises_required_fields() {
        let new = NewTask {
            assignees: Vec::new(),
            title: "Inbox zero".into(),
            description: Some("clear out".into()),
            status: TaskStatus::Open,
            priority: TaskPriority::Low,
            effort: cal_core::TaskEffort::Medium,
            deadline_reminder_days: None,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            resurface_date: None,
            series_id: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
        };
        let body = new_task_to_body(&new).unwrap();
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["title"], "Inbox zero");
        assert_eq!(json["importance"], "low");
        assert_eq!(json["status"], "notStarted");
        assert_eq!(json["body"]["content"], "clear out");
        assert_eq!(json["isReminderOn"], false);
        // Missing optional fields must not be serialised — Graph
        // accepts a PATCH that only changes a subset and treats
        // `null` and "missing" differently for some fields.
        assert!(json.get("dueDateTime").is_none());
        assert!(json.get("startDateTime").is_none());
        // No extras ⇒ no inline extensions on the wire.
        assert!(json.get("extensions").is_none());
    }

    #[test]
    fn open_extension_round_trips_backlog_recurrence() {
        use cal_core::{
            MonthDay, RecurrenceAnchor, RecurrenceEnd, RecurrenceFrequency, RecurrencePlacement,
            TaskRecurrence,
        };
        let rec = TaskRecurrence {
            frequency: RecurrenceFrequency::Yearly,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: Some(RecurrenceEnd::Never),
            anchor: RecurrenceAnchor::FromCompletion,
            placement: RecurrencePlacement::Backlog,
            fixed_dates: Some(vec![MonthDay { month: 4, day: 1 }]),
        };
        let new = NewTask {
            assignees: Vec::new(),
            title: "Swap shoes".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            effort: cal_core::TaskEffort::Medium,
            deadline_reminder_days: None,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            resurface_date: Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
            series_id: Some("series-shoes".into()),
            recurrence: Some(rec.clone()),
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
        };
        let body = new_task_to_body(&new).unwrap();
        // Backlog rule ⇒ no native recurrence; the open extension carries it.
        assert!(body.recurrence.is_none());
        assert_eq!(body.extensions.len(), 1);
        let payload = body.extensions[0].aperio_extras.clone();
        assert!(payload.starts_with("aperio:1:"));

        // Read it back via a todoTask whose extensions were expanded.
        let entry: TodoTaskEntry = serde_json::from_value(serde_json::json!({
            "id": "T9",
            "title": "Swap shoes",
            "status": "notStarted",
            "extensions": [{
                "@odata.type": "microsoft.graph.openTypeExtension",
                "extensionName": "com.aperio.extras",
                "aperioExtras": payload,
            }],
        }))
        .unwrap();
        let task = map_task(entry, "L").unwrap();
        assert_eq!(task.recurrence, Some(rec));
        assert_eq!(
            task.resurface_date,
            Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
        );
        assert_eq!(task.series_id.as_deref(), Some("series-shoes"));
    }

    #[test]
    fn task_to_body_emits_due_date_and_recurrence() {
        use cal_core::{RecurrenceEnd, RecurrenceFrequency, TaskRecurrence, Weekday};
        let task = Task {
            assignees: Vec::new(),
            id: "L|T".into(),
            list_id: "L".into(),
            title: "Standup".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            effort: cal_core::TaskEffort::Medium,
            deadline_reminder_days: None,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: Some(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()),
            deadline_time: Some(chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap()),
            resurface_date: None,
            series_id: None,
            recurrence: Some(TaskRecurrence {
                frequency: RecurrenceFrequency::Weekly,
                interval: 1,
                day_of_week: Some(vec![Weekday::Monday, Weekday::Wednesday]),
                day_of_month: None,
                anchor: Default::default(),
                placement: Default::default(),
                fixed_dates: None,
                end: Some(RecurrenceEnd::After { occurrences: 6 }),
            }),
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            etag: None,
        };
        let body = task_to_body(&task).unwrap();
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["dueDateTime"]["dateTime"], "2026-07-01T10:00:00");
        assert_eq!(json["recurrence"]["pattern"]["type"], "weekly");
        assert_eq!(json["recurrence"]["pattern"]["daysOfWeek"][0], "monday");
        assert_eq!(json["recurrence"]["range"]["type"], "numbered");
        assert_eq!(json["recurrence"]["range"]["numberOfOccurrences"], 6);
    }

    #[test]
    fn task_status_round_trips() {
        // Round-trip every status pair we own. `Cancelled` <→
        // `deferred` is the lossy edge — Aperio doesn't have
        // "deferred" as a concept but the round-trip restores the
        // original value, which is what matters for sync.
        let cases = [
            (TaskStatus::Open, "notStarted"),
            (TaskStatus::InProgress, "inProgress"),
            (TaskStatus::Completed, "completed"),
            (TaskStatus::Cancelled, "deferred"),
        ];
        for (status, graph) in cases {
            assert_eq!(task_status_to_graph(status), graph);
        }
    }

    #[test]
    fn html_strip_keeps_text_and_normalises_newlines() {
        let stripped = strip_html_tags("<p>hello</p><p>world</p>");
        assert!(stripped.contains("hello"));
        assert!(stripped.contains("world"));
        let with_br = strip_html_tags("line a<br/>line b<br>line c");
        assert!(with_br.contains("line a"));
        assert!(with_br.contains("line b"));
        assert!(with_br.contains("line c"));
    }
}
