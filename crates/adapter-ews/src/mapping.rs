//! XML response parsers for EWS.
//!
//! EWS streams its results in a fixed envelope shape:
//!
//! ```xml
//! <soap:Envelope>
//!   <soap:Body>
//!     <m:FindFolderResponse>
//!       <m:ResponseMessages>
//!         <m:FindFolderResponseMessage ResponseClass="Success">
//!           <m:ResponseCode>NoError</m:ResponseCode>
//!           <m:RootFolder>
//!             <t:Folders>
//!               <t:CalendarFolder> … </t:CalendarFolder>
//!               <t:CalendarFolder> … </t:CalendarFolder>
//!             </t:Folders>
//!           </m:RootFolder>
//!         </m:FindFolderResponseMessage>
//!       </m:ResponseMessages>
//!     </m:FindFolderResponse>
//!   </soap:Body>
//! </soap:Envelope>
//! ```
//!
//! Item responses follow the same skeleton with `m:FindItemResponse` /
//! `<t:Items>` / `<t:CalendarItem>`. We walk the stream with quick-xml
//! tracking element local-names (ignoring the `t:` / `m:` / `soap:`
//! prefixes since servers occasionally emit unbound default-namespace
//! versions of the same names).
//!
//! Fault detection happens in `soap.rs` *before* this module runs, so
//! by the time we get here the body is guaranteed to be a success.
//! That keeps the parsers below straightforward — they only need to
//! handle the happy-path schema.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};

use cal_core::event_diff::EventField;
use cal_core::{
    AttendeeStatus, Calendar, Event, EventRecurrence, FreeBusy, FreeBusySlot, Reminder,
    ReminderKind,
};

use crate::error::{EwsError, EwsResult};
use crate::windows_tz::ServerTimeZones;

/// One calendar folder pulled from a `FindFolder` response.
#[derive(Debug, Clone)]
pub struct ParsedFolder {
    pub folder_id: String,
    pub change_key: Option<String>,
    pub display_name: String,
}

/// Walk a `FindFolderResponse` body and yield one `ParsedFolder` per
/// `<t:CalendarFolder>` block. The caller wraps the result in
/// `cal_core::Calendar` via `to_calendar`.
pub fn parse_find_folder_response(xml: &str) -> EwsResult<Vec<ParsedFolder>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut folders = Vec::new();
    let mut inside_folder = false;
    let mut current = ParsedFolder {
        folder_id: String::new(),
        change_key: None,
        display_name: String::new(),
    };
    // Track which simple element we're collecting text for. EWS
    // surfaces `DisplayName` as a child element, not an attribute.
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                // Start vs Empty doesn't matter here — `FolderId` is
                // always an empty element (attribute-only), while
                // `DisplayName` is always non-empty with a text child.
                // Both arms match into the same handling below.

                if local == b"calendarfolder" {
                    inside_folder = true;
                    current = ParsedFolder {
                        folder_id: String::new(),
                        change_key: None,
                        display_name: String::new(),
                    };
                }
                if inside_folder && local == b"folderid" {
                    // FolderId is an empty element with `Id` + `ChangeKey`
                    // attributes — we read them off the start tag.
                    for a in e.attributes().flatten() {
                        let key = a.key.as_ref();
                        if key.eq_ignore_ascii_case(b"Id") {
                            current.folder_id = String::from_utf8_lossy(&a.value).into_owned();
                        } else if key.eq_ignore_ascii_case(b"ChangeKey") {
                            current.change_key =
                                Some(String::from_utf8_lossy(&a.value).into_owned());
                        }
                    }
                }
                if inside_folder && local == b"displayname" {
                    text_target = Some("name");
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"displayname" {
                    text_target = None;
                }
                if local == b"calendarfolder" {
                    if !current.folder_id.is_empty() {
                        folders.push(current.clone());
                    }
                    inside_folder = false;
                }
            }
            Ok(XmlEvent::Text(t)) if text_target == Some("name") => {
                let s = t.unescape().map(|c| c.to_string()).unwrap_or_default();
                current.display_name.push_str(&s);
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(EwsError::Protocol(format!("xml parse: {err}")));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(folders)
}

/// Translate a parsed folder into a cal-core `Calendar`. We encode the
/// folder id and (optional) change key into a single Aperio id so the
/// downstream commands can pass it back to us as a flat string. The
/// separator `|` is illegal in EWS-emitted base64 ids.
pub fn to_calendar(folder: ParsedFolder, read_only: bool) -> Calendar {
    // The calendar id is the STABLE folder EntryID only — NOT
    // `folder_id|change_key`. A folder's ChangeKey is volatile:
    // Exchange bumps it whenever the folder changes (including on item
    // add/remove, via properties like ItemCount). Embedding it would
    // rotate the calendar id on every change, orphaning the host
    // snapshot cache (and any per-calendar settings) keyed by that id
    // and forcing a synchronous cold EWS fetch on every app open.
    // Writes that genuinely need a ChangeKey (UpdateFolder, i.e.
    // rename) harvest a fresh one at write time instead — see
    // `api::rename_calendar`.
    Calendar {
        // EWS/Exchange always performs server-side meeting scheduling when
        // the CreateItem/UpdateItem send-disposition asks for it.
        supports_scheduling: true,
        // No RFC 7986 per-event COLOR round-trip on EWS; per-event colors
        // stay host-local overrides.
        supports_event_color: false,
        always_notifies_attendees: false,
        invitations_reply_only: false,
        // FALSE for now, and not because Exchange cannot: `UpdateItem` against
        // an `OccurrenceItemId` makes an exception of any occurrence. But
        // `resolve_override_target` looks a `::rid::` id up among the
        // `ModifiedOccurrences` ONLY, so an occurrence nobody has changed yet
        // is "no longer an exception in the series" and the write is refused.
        // Declaring the capability before that path exists would promise a
        // write this adapter cannot address (79b; the index machinery is next
        // door in `delete_series_occurrence`, verified per index against the
        // server, and TODO says so).
        stores_occurrence_exceptions: false,
        notifier_name: None,
        color_label: None,
        id: folder.folder_id,
        name: if folder.display_name.is_empty() {
            "Calendar".into()
        } else {
            folder.display_name
        },
        color: None,
        read_only,
        default_sound: None,
    }
}

/// Split a calendar id minted by `to_calendar` back into its
/// (folder_id, change_key) components.
pub fn split_calendar_id(id: &str) -> (String, Option<String>) {
    match id.split_once('|') {
        Some((fid, ck)) => (fid.to_string(), Some(ck.to_string())),
        None => (id.to_string(), None),
    }
}

// ── Event id encoding ───────────────────────────────────────────────────
//
// EWS surfaces four flavours of CalendarItem and Aperio's write side
// needs to tell them apart:
//
//   - **Single**: a standalone non-recurring event.
//   - **RecurringMaster**: the series template that owns the
//     recurrence rule.
//   - **Occurrence**: one expanded instance of a series, returned by
//     `CalendarView`. Has its own ItemId distinct from the master's.
//   - **Exception**: an occurrence that already carries an override
//     (someone moved its time, changed its subject, etc.). Also
//     distinct from the master.
//
// Aperio's event ids carry the type as a one-character prefix so the
// adapter knows where to route writes:
//
//   `S:id|ck` → Single (delete / update target the row directly)
//   `O:id|ck` → Occurrence (target the master for series-wide writes;
//                target the row for per-occurrence EXDATE)
//   `E:id|ck` → Exception (same routing as Occurrence)
//   `M:id|ck` → RecurringMaster (target the row directly; affects
//                the whole series by definition)
//
// Decoder is backwards-compatible: an unprefixed `id|ck` reads as
// Single. That keeps any persisted ids minted before this change
// (e.g. in the local override store) working without a migration.

/// Kind of EWS CalendarItem behind an Aperio event id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventIdKind {
    Single,
    Occurrence,
    Exception,
    RecurringMaster,
}

impl EventIdKind {
    fn prefix(self) -> char {
        match self {
            EventIdKind::Single => 'S',
            EventIdKind::Occurrence => 'O',
            EventIdKind::Exception => 'E',
            EventIdKind::RecurringMaster => 'M',
        }
    }

    fn from_prefix(c: char) -> Option<Self> {
        Some(match c {
            'S' => EventIdKind::Single,
            'O' => EventIdKind::Occurrence,
            'E' => EventIdKind::Exception,
            'M' => EventIdKind::RecurringMaster,
            _ => return None,
        })
    }

    /// Parse from the EWS `<t:CalendarItemType>` element value.
    pub fn from_calendar_item_type(s: &str) -> Self {
        match s {
            "Single" => EventIdKind::Single,
            "Occurrence" => EventIdKind::Occurrence,
            "Exception" => EventIdKind::Exception,
            "RecurringMaster" => EventIdKind::RecurringMaster,
            // EWS occasionally returns blank values for items the
            // server didn't fully expand. Fall through to Single as
            // the least-surprising default; non-recurring is the
            // common case.
            _ => EventIdKind::Single,
        }
    }

    pub fn is_occurrence_like(self) -> bool {
        matches!(self, EventIdKind::Occurrence | EventIdKind::Exception)
    }
}

/// Pack an EWS ItemId + ChangeKey + type into the Aperio-facing
/// event id string. Matches the decoder in [`decode_event_id`].
pub fn encode_event_id(kind: EventIdKind, id: &str, change_key: Option<&str>) -> String {
    let prefix = kind.prefix();
    match change_key {
        Some(ck) => format!("{prefix}:{id}|{ck}"),
        None => format!("{prefix}:{id}"),
    }
}

/// Separates a series id from the occurrence instant an override replaces.
///
/// The SAME string the CalDAV adapter uses (`mapping::RECURRENCE_ID_MARKER`
/// there) and the same one the shared frontend reads
/// (`RECURRENCE_ID_MARKER` in `shared/recurrence.ts`). That is the entire
/// point: an override id is only an override if every layer agrees on how to
/// spot one.
///
/// This adapter used to write `#override:` here — a marker written in one place
/// and read in none. The consequences were not cosmetic. The frontend saw a
/// plain standalone event, so opening an edited occurrence never offered the
/// "this one or the series?" prompt; and because the suffix was glued onto an
/// id that still carried the MASTER's change key, `decode_event_id` handed the
/// write path `ck#override:2026-05-30T08:00:00+00:00` as a ChangeKey — a
/// corrupt optimistic-concurrency token aimed at the series head.
pub const RECURRENCE_ID_MARKER: &str = "::rid::";

/// Pack an override's id: the MASTER's encoded id, then the marker, then the
/// occurrence it replaces.
///
/// The master id goes in front deliberately. The frontend derives the series
/// from everything before the marker (`overrideSeriesId`), and that is what
/// makes "edit the whole series" reachable from an occurrence the user opened.
/// The exception's own ItemId is NOT in here — it is resolved at write time
/// from the master's `ModifiedOccurrences`, keyed by this instant, because a
/// ChangeKey baked into an id goes stale the moment anything else touches the
/// item.
pub fn encode_override_event_id(master_event_id: &str, original_start: DateTime<Utc>) -> String {
    format!(
        "{master_event_id}{RECURRENCE_ID_MARKER}{}",
        original_start.to_rfc3339()
    )
}

/// Decoded Aperio event id: split apart into a CalendarItem kind,
/// the raw ItemId, and the optional ChangeKey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedEventId {
    pub kind: EventIdKind,
    pub item_id: String,
    pub change_key: Option<String>,
    /// The occurrence instant this id overrides, for a
    /// `…::rid::<rfc3339>` override id; `None` for a master, a plain
    /// occurrence or a single event.
    ///
    /// When set, `item_id`/`change_key` still address the SERIES — the
    /// exception's own item is resolved from it. Writers must branch on this
    /// before deciding what they are about to overwrite.
    pub recurrence_id: Option<DateTime<Utc>>,
}

/// Decode an Aperio event id. Falls back to `Single` if the string
/// has no prefix (compat path for ids minted before 6f.1c).
pub fn decode_event_id(s: &str) -> DecodedEventId {
    // The override suffix comes OFF FIRST, before anything looks for the
    // change-key separator. It sits at the very end of an id whose change key
    // is in the middle, so splitting on `|` first swallows the marker into the
    // ChangeKey — which is exactly the bug this function used to have, and it
    // aimed a malformed concurrency token at the series head.
    let (base, recurrence_id) = match s.split_once(RECURRENCE_ID_MARKER) {
        Some((base, iso)) => (
            base,
            DateTime::parse_from_rfc3339(iso)
                .ok()
                .map(|dt| dt.with_timezone(&Utc)),
        ),
        None => (s, None),
    };
    // An id that carries the marker but not a readable timestamp is NOT
    // silently treated as a master: the base is still the series, and
    // `recurrence_id: None` would tell a writer it may overwrite the whole
    // thing. Keeping the base and losing only the instant means the write path
    // refuses (it cannot find an occurrence to target) instead of guessing.

    // The prefix is one character followed by `:`. Anything else is
    // treated as an un-prefixed legacy id.
    let mut chars = base.chars();
    let first = chars.next();
    let second = chars.next();
    if let (Some(p), Some(':')) = (first, second) {
        if let Some(kind) = EventIdKind::from_prefix(p) {
            let rest = &base[2..];
            let (item_id, change_key) = match rest.split_once('|') {
                Some((id, ck)) => (id.to_string(), Some(ck.to_string())),
                None => (rest.to_string(), None),
            };
            return DecodedEventId {
                kind,
                item_id,
                change_key,
                recurrence_id,
            };
        }
    }
    // Legacy / unprefixed path.
    let (item_id, change_key) = match base.split_once('|') {
        Some((id, ck)) => (id.to_string(), Some(ck.to_string())),
        None => (base.to_string(), None),
    };
    DecodedEventId {
        kind: EventIdKind::Single,
        item_id,
        change_key,
        recurrence_id,
    }
}

/// Whether an id names one occurrence of a series rather than the series.
///
/// True for a `…::rid::…` override AND for the `O:`/`E:` ids the server hands
/// back from a `CalendarView`. Writers that must not touch a whole series ask
/// this, not the kind alone.
pub fn addresses_single_occurrence(decoded: &DecodedEventId) -> bool {
    decoded.recurrence_id.is_some() || decoded.kind.is_occurrence_like()
}

/// One calendar item pulled from a `FindItem` response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedItem {
    pub item_id: String,
    pub change_key: Option<String>,
    pub subject: String,
    pub body: Option<String>,
    pub location: Option<String>,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub is_all_day: bool,
    pub is_recurring: bool,
    /// `<t:IsCancelled>` — the meeting was cancelled by its organizer.
    /// Exchange keeps the item in the calendar (with a "Canceled:" subject)
    /// rather than deleting it, so Aperio surfaces it but never schedules
    /// reminders for it. `#[serde(default)]` so sync-state files written before
    /// this field existed load as `false` without forcing a re-sync.
    #[serde(default)]
    pub cancelled: bool,
    /// `<t:AppointmentState>` — a bitmask (asfMeeting=1, asfReceived=2,
    /// asfCanceled=4). Some Exchange configs leave `IsCancelled=false` on an
    /// attendee's copy of a cancelled meeting yet still flip the `asfCanceled`
    /// bit here, so `to_event` ORs it into the cancelled flag as a fallback
    /// signal. `None` when the server omits the property (older servers, or a
    /// read shape that doesn't request it). `#[serde(default)]` so persisted
    /// sync state written before this field existed loads as `None` without
    /// forcing a re-sync.
    #[serde(default)]
    pub appointment_state: Option<i32>,
    pub reminder_is_set: bool,
    pub reminder_minutes_before_start: Option<i64>,
    pub created: Option<DateTime<Utc>>,
    pub last_modified: Option<DateTime<Utc>>,
    /// `<t:CalendarItemType>` element value, normalised. Defaults to
    /// `Single` when EWS omits it (e.g. older servers that don't
    /// honour the property request).
    pub item_type: Option<String>,
    /// Windows zone id from `<t:StartTimeZone>` (e.g. "Eastern Standard Time"),
    /// when requested + present. Translated to IANA for a recurring master's
    /// `EventRecurrence.tzid` so the series expands DST-correctly. `#[serde(default)]`
    /// so older persisted sync state loads without forcing a full re-sync.
    #[serde(default)]
    pub start_time_zone: Option<String>,
    /// Windows zone id from `<t:EndTimeZone>`. Exchange writes
    /// `tzone://Microsoft/Utc` here for a series created without a zone, while
    /// its start zone reads `Greenwich Standard Time` (decision 43b).
    /// `#[serde(default)]` so older persisted state loads; that state is
    /// drained again anyway (`api::ITEM_PARSER`).
    #[serde(default)]
    pub end_time_zone: Option<String>,
    /// `<t:OriginalStart>` on an occurrence or exception read by itself (the
    /// occurrence probe): the slot of the series it fills, which stays put when
    /// an exception is moved. `None` elsewhere. `#[serde(default)]` so older
    /// persisted state loads.
    #[serde(default)]
    pub original_start: Option<DateTime<Utc>>,
    /// On a RecurringMaster row from `SyncFolderItems`, the
    /// `<t:Recurrence>` element parses to this. `None` on singles
    /// and on read paths that don't request the field (the legacy
    /// `FindItem + CalendarView` parser leaves this empty).
    pub recurrence: Option<EwsRecurrence>,
    /// `<t:DeletedOccurrences>` start datetimes on a RecurringMaster
    /// row — translates to EXDATE entries in
    /// `cal_core::EventRecurrence::exceptions` on the way down.
    /// `#[serde(default)]` so a future schema where this field
    /// is absent in older persisted state files loads cleanly
    /// instead of triggering a full re-sync.
    #[serde(default)]
    pub deleted_occurrence_starts: Vec<DateTime<Utc>>,
    /// `<t:ModifiedOccurrences>` on a RecurringMaster row — one
    /// entry per instance whose time was moved or content was
    /// edited server-side. The server inlines just the new
    /// time + original time + the override's item id; the
    /// override's actual subject/location would require a
    /// follow-up GetItem (deferred). The adapter currently
    /// EXDATEs out the original slot and emits a synthetic
    /// standalone event at the moved time, inheriting the
    /// master's content — gets the time right, may show stale
    /// title/location for the small minority of overrides that
    /// also edited the content fields.
    #[serde(default)]
    pub modified_occurrences: Vec<ModifiedOccurrence>,
    /// `<t:Organizer><t:Mailbox><t:EmailAddress>` — the meeting
    /// organizer's SMTP address. Populated only by the detail GetItem
    /// fan-out (the `SyncFolderItems`/`FindItem` shapes omit it).
    #[serde(default)]
    pub organizer: Option<String>,
    /// `<t:MyResponseType>` — the connected mailbox's own response, which is
    /// `Organizer` exactly when it organizes the item (decision 70a). Same
    /// detail-fetch caveat as `organizer`.
    #[serde(default)]
    pub my_response_type: Option<String>,
    /// `<t:RequiredAttendees>` + `<t:OptionalAttendees>` — the invitees
    /// with their `<t:ResponseType>`. Same detail-fetch caveat as
    /// `organizer`.
    #[serde(default)]
    pub attendees: Vec<EwsAttendee>,
    /// True once the per-item detail GetItem fan-out has populated
    /// this row's `body` (and, for masters, `recurrence`). The
    /// `SyncFolderItems` shape never carries `<t:Body>` or
    /// `<t:Recurrence>`, so a freshly Created/Updated row starts
    /// `false` and gets enriched once; the flag stops every
    /// subsequent sync from re-fetching the (often empty) body of
    /// every item. `#[serde(default)]` → persisted state files
    /// written before this field existed load as `false` and
    /// re-enrich once on the next launch.
    #[serde(default)]
    pub detail_fetched: bool,
}

/// One entry from a master row's `<t:ModifiedOccurrences>` list.
/// Carries the override's identity + new time slot + the original
/// time slot the override displaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifiedOccurrence {
    /// The override's own ItemId — addressable directly for a
    /// future per-override GetItem fan-out.
    pub item_id: String,
    pub change_key: Option<String>,
    /// Where the override actually appears on the calendar.
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Which RRULE-generated slot the override displaces; we use
    /// this as the master's EXDATE so the expander skips the
    /// vacated slot.
    pub original_start: DateTime<Utc>,
    /// Whether this override is a CANCELLED occurrence: the organizer
    /// withdrew just this instance of the series. The inline
    /// `<t:ModifiedOccurrences>` shape carries no cancelled flag — this
    /// is filled by the per-override GetItem enrichment from the
    /// exception item's own `IsCancelled`/`AppointmentState`/subject
    /// (`resolve_cancelled`). `#[serde(default)]` so persisted state
    /// written before this field existed loads as `false`.
    #[serde(default)]
    pub cancelled: bool,
    /// The exception's OWN item, as its per-occurrence `GetItem` answered.
    ///
    /// A changed occurrence is an item of its own on the server, with its own
    /// subject, body, location, reminder and times. The enrichment used to
    /// read all of that and keep one boolean; the row then showed the SERIES'
    /// content under the occurrence's slot (decision 58a, measured in live
    /// round 5).
    ///
    /// `None` when that GetItem has not run, failed, or answered for another
    /// slot: the emitted row then inherits the series' content exactly as it
    /// always did, because a row with a guessed subject would be worse than a
    /// row with an inherited one. `#[serde(default)]` keeps persisted state
    /// written before this field existed loadable.
    #[serde(default)]
    pub own: Option<Box<ParsedItem>>,
}

/// One invitee from a CalendarItem's `RequiredAttendees` /
/// `OptionalAttendees` list. `response_type` is the raw EWS value
/// (`Accept`, `Decline`, `Tentative`, `Organizer`, `NoResponseReceived`,
/// `Unknown`), normalised to [`cal_core::AttendeeStatus`] in `to_event`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EwsAttendee {
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub response_type: Option<String>,
}

/// Walk a `FindItemResponse` body and yield one `ParsedItem` per
/// `<t:CalendarItem>` block.
pub fn parse_find_item_response(xml: &str) -> EwsResult<Vec<ParsedItem>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut items = Vec::new();
    let mut inside_item = false;
    let mut current = ParsedItem::default();
    let mut text_target: Option<&'static str> = None;
    // EWS nests two distinct `Body` elements: the item's text body
    // (`<t:Body BodyType="HTML">…</t:Body>`) and the SOAP `<soap:Body>`
    // envelope. We only collect text when we're inside a CalendarItem,
    // which sidesteps the ambiguity.

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"calendaritem" {
                    inside_item = true;
                    current = ParsedItem::default();
                    continue;
                }
                if !inside_item {
                    continue;
                }
                match local.as_slice() {
                    b"itemid" => {
                        for a in e.attributes().flatten() {
                            let key = a.key.as_ref();
                            if key.eq_ignore_ascii_case(b"Id") {
                                current.item_id = String::from_utf8_lossy(&a.value).into_owned();
                            } else if key.eq_ignore_ascii_case(b"ChangeKey") {
                                current.change_key =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    b"subject" => text_target = Some("subject"),
                    b"body" => text_target = Some("body"),
                    b"location" => text_target = Some("location"),
                    b"start" => text_target = Some("start"),
                    b"end" => text_target = Some("end"),
                    b"isalldayevent" => text_target = Some("all_day"),
                    b"isrecurring" => text_target = Some("recurring"),
                    b"iscancelled" => text_target = Some("cancelled"),
                    b"appointmentstate" => text_target = Some("appointment_state"),
                    b"reminderisset" => text_target = Some("reminder_on"),
                    b"reminderminutesbeforestart" => text_target = Some("reminder_mins"),
                    b"datetimecreated" => text_target = Some("created"),
                    b"lastmodifiedtime" => text_target = Some("modified"),
                    b"calendaritemtype" => text_target = Some("item_type"),
                    _ => {}
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"calendaritem" {
                    if !current.item_id.is_empty() {
                        items.push(std::mem::take(&mut current));
                    }
                    inside_item = false;
                    continue;
                }
                text_target = None;
            }
            Ok(XmlEvent::Text(t)) if text_target.is_some() => {
                let raw = match t.unescape() {
                    Ok(c) => c.to_string(),
                    Err(_) => continue,
                };
                let s = raw.trim();
                if s.is_empty() {
                    continue;
                }
                match text_target {
                    Some("subject") => current.subject.push_str(s),
                    Some("body") => {
                        let acc = current.body.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    Some("location") => {
                        let acc = current.location.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    Some("start") => current.start = parse_ews_datetime(s),
                    Some("end") => current.end = parse_ews_datetime(s),
                    Some("all_day") => {
                        current.is_all_day = s.eq_ignore_ascii_case("true");
                    }
                    Some("recurring") => {
                        current.is_recurring = s.eq_ignore_ascii_case("true");
                    }
                    Some("cancelled") => {
                        current.cancelled = s.eq_ignore_ascii_case("true");
                    }
                    Some("appointment_state") => {
                        current.appointment_state = s.parse::<i32>().ok();
                    }
                    Some("reminder_on") => {
                        current.reminder_is_set = s.eq_ignore_ascii_case("true");
                    }
                    Some("reminder_mins") => {
                        current.reminder_minutes_before_start = s.parse::<i64>().ok();
                    }
                    Some("created") => current.created = parse_ews_datetime(s),
                    Some("modified") => current.last_modified = parse_ews_datetime(s),
                    Some("item_type") => {
                        let acc = current.item_type.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(EwsError::Protocol(format!("xml parse: {err}")));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(items)
}

// ── SyncFolderItems response ───────────────────────────────────────────────

/// One change reported by `SyncFolderItems`. The server groups
/// per-item notifications under `<m:Changes>`:
///
/// ```xml
/// <m:Changes>
///   <t:Create><t:CalendarItem>…</t:CalendarItem></t:Create>
///   <t:Update><t:CalendarItem>…</t:CalendarItem></t:Update>
///   <t:Delete><t:ItemId Id="…"/></t:Delete>
///   <t:ReadFlagChange>…</t:ReadFlagChange>  (calendar items don't emit this)
/// </m:Changes>
/// ```
///
/// We map Create/Update to the same `ParsedItem` shape the
/// FindItem path produces so the downstream cal-core conversion
/// stays single-source. Delete carries only the item id — the
/// caller drops the corresponding row from its local cache.
#[derive(Debug, Clone)]
pub enum SyncChange {
    Create(ParsedItem),
    Update(ParsedItem),
    Delete(String),
}

/// Result of one `SyncFolderItems` round-trip. The caller stashes
/// `new_sync_state` for the next call and uses `includes_last` to
/// decide whether to keep paging.
#[derive(Debug, Clone)]
pub struct SyncFolderItemsResult {
    pub changes: Vec<SyncChange>,
    pub new_sync_state: String,
    pub includes_last: bool,
}

/// Walk a `SyncFolderItemsResponse` body. Returns one
/// `SyncFolderItemsResult` per call — the caller loops on
/// `includes_last == false` to drain the rest of the deltas with
/// the freshly-returned `new_sync_state`.
///
/// Field plumbing inside each `<t:CalendarItem>` mirrors
/// [`parse_find_item_response`] so the data shape stays uniform
/// across read paths. (The duplication is intentional — extracting
/// a shared walker would couple the two responses' state machines
/// without removing meaningful logic.)
pub fn parse_sync_folder_items_response(xml: &str) -> EwsResult<SyncFolderItemsResult> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut changes: Vec<SyncChange> = Vec::new();
    let mut new_sync_state = String::new();
    let mut includes_last = false;

    // Where in the response tree are we?
    //   - inside_change_kind: Some("create"|"update"|"delete") while
    //     we're inside a t:Create/t:Update/t:Delete block.
    //   - inside_item: true while inside a <t:CalendarItem> (Create
    //     and Update wrap one; Delete just carries an ItemId).
    //   - recurrence_walker: Some(_) while inside a <t:Recurrence>
    //     subtree. The walker accumulates pattern/range state; the
    //     outer End triggers finish + assignment to ParsedItem.
    //   - inside_deleted_occurrence: bumped on each
    //     <t:DeletedOccurrence> Start so the inner <t:Start> text
    //     routes to the right collection.
    let mut inside_change_kind: Option<&'static str> = None;
    let mut inside_item = false;
    let mut current = ParsedItem::default();
    let mut text_target: Option<&'static str> = None;
    let mut recurrence_walker: Option<RecurrenceWalker> = None;
    // `<t:StartTimeZone>` carries the master's Windows zone id (directly, or on
    // a nested `<t:TimeZoneDefinition>`); track that we're inside it so the
    // nested-form id isn't picked up from EndTimeZone.
    let mut inside_start_timezone = false;
    let mut inside_deleted_occurrences = false;
    let mut inside_deleted_occurrence = false;
    // ModifiedOccurrences mirrors the DeletedOccurrences shape but
    // each child carries multiple fields (item_id, start, end,
    // original_start) — we accumulate into `current_override` and
    // push to the master on End.
    let mut inside_modified_occurrences = false;
    let mut inside_modified_occurrence = false;
    let mut current_override = ModifiedOccurrenceBuilder::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                // Recurrence subtree: route to the shared walker
                // instead of the outer item state machine, so the
                // pattern/range elements don't collide with
                // CalendarItem's `<t:Start>` / `<t:End>` text fields.
                if let Some(walker) = recurrence_walker.as_mut() {
                    walker.observe_start(local.as_slice());
                    continue;
                }
                match local.as_slice() {
                    b"create" => inside_change_kind = Some("create"),
                    b"update" => inside_change_kind = Some("update"),
                    b"delete" => inside_change_kind = Some("delete"),
                    b"syncstate" => text_target = Some("sync_state"),
                    b"includeslastiteminrange" => {
                        text_target = Some("includes_last");
                    }
                    b"calendaritem" if inside_change_kind.is_some() => {
                        inside_item = true;
                        current = ParsedItem::default();
                    }
                    // Delete carries the ItemId directly under the
                    // wrapping `t:Delete`. Capture it as the change
                    // payload and emit on the End of the Delete block.
                    b"itemid" if inside_change_kind == Some("delete") => {
                        for a in e.attributes().flatten() {
                            if a.key.as_ref().eq_ignore_ascii_case(b"Id") {
                                current.item_id = String::from_utf8_lossy(&a.value).into_owned();
                            }
                        }
                    }
                    // `inside_modified_occurrence` is technically a
                    // SUBSET of `inside_item` — both are true while
                    // we're walking a master's override list. Guard
                    // this arm explicitly so the override's nested
                    // ItemId doesn't silently overwrite the
                    // master's. The override-specific arm sits
                    // below (with the same `inside_modified_occurrence`
                    // condition) and captures it correctly.
                    b"itemid" if inside_item && !inside_modified_occurrence => {
                        for a in e.attributes().flatten() {
                            let key = a.key.as_ref();
                            if key.eq_ignore_ascii_case(b"Id") {
                                current.item_id = String::from_utf8_lossy(&a.value).into_owned();
                            } else if key.eq_ignore_ascii_case(b"ChangeKey") {
                                current.change_key =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    // Recurrence subtree start — hand off to the
                    // shared walker until matching End.
                    b"recurrence" if inside_item => {
                        recurrence_walker = Some(RecurrenceWalker::default());
                    }
                    // DeletedOccurrences block — each child carries
                    // a `<t:Start>` that we want to collect as an
                    // EXDATE. Track depth so the inner `<t:Start>`
                    // text doesn't accidentally rewrite the master's
                    // own start/end.
                    b"deletedoccurrences" if inside_item => {
                        inside_deleted_occurrences = true;
                    }
                    b"deletedoccurrence" if inside_deleted_occurrences => {
                        inside_deleted_occurrence = true;
                    }
                    b"modifiedoccurrences" if inside_item => {
                        inside_modified_occurrences = true;
                    }
                    b"occurrence" if inside_modified_occurrences => {
                        inside_modified_occurrence = true;
                        current_override = ModifiedOccurrenceBuilder::default();
                    }
                    // ItemId nested inside <t:Occurrence> carries the
                    // override's address. Capture it BEFORE the
                    // outer `b"itemid" if inside_item` arm — that
                    // arm would overwrite the master's id with the
                    // occurrence's id (catastrophic — every
                    // subsequent push targets the wrong row).
                    b"itemid" if inside_modified_occurrence => {
                        for a in e.attributes().flatten() {
                            let key = a.key.as_ref();
                            if key.eq_ignore_ascii_case(b"Id") {
                                current_override.item_id =
                                    String::from_utf8_lossy(&a.value).into_owned();
                            } else if key.eq_ignore_ascii_case(b"ChangeKey") {
                                current_override.change_key =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    // Per-field text targets — only honoured while
                    // we're inside a CalendarItem block.
                    b"subject" if inside_item => text_target = Some("subject"),
                    b"body" if inside_item => text_target = Some("body"),
                    b"location" if inside_item => text_target = Some("location"),
                    b"start" if inside_deleted_occurrence => {
                        text_target = Some("deleted_occurrence_start");
                    }
                    b"start" if inside_modified_occurrence => {
                        text_target = Some("override_start");
                    }
                    b"end" if inside_modified_occurrence => {
                        text_target = Some("override_end");
                    }
                    b"originalstart" if inside_modified_occurrence => {
                        text_target = Some("override_original_start");
                    }
                    // An occurrence or exception read on its own: the slot it
                    // fills (see `ParsedItem::original_start`).
                    b"originalstart" if inside_item => text_target = Some("original_start"),
                    b"start" if inside_item => text_target = Some("start"),
                    b"end" if inside_item => text_target = Some("end"),
                    b"isalldayevent" if inside_item => {
                        text_target = Some("all_day");
                    }
                    b"isrecurring" if inside_item => {
                        text_target = Some("recurring");
                    }
                    b"iscancelled" if inside_item => {
                        text_target = Some("cancelled");
                    }
                    b"appointmentstate" if inside_item => {
                        text_target = Some("appointment_state");
                    }
                    b"reminderisset" if inside_item => {
                        text_target = Some("reminder_on");
                    }
                    b"reminderminutesbeforestart" if inside_item => {
                        text_target = Some("reminder_mins");
                    }
                    b"datetimecreated" if inside_item => {
                        text_target = Some("created");
                    }
                    b"lastmodifiedtime" if inside_item => {
                        text_target = Some("modified");
                    }
                    b"calendaritemtype" if inside_item => {
                        text_target = Some("item_type");
                    }
                    b"starttimezone" if inside_item => {
                        inside_start_timezone = true;
                        // Simple form: <t:StartTimeZone Id="Eastern Standard Time" .../>.
                        for a in e.attributes().flatten() {
                            if a.key.as_ref().eq_ignore_ascii_case(b"Id") {
                                current.start_time_zone =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    // The end zone's own id, where Exchange marks a series
                    // created without a zone as `tzone://Microsoft/Utc`
                    // (decision 43b). Only the element's own id is read; a
                    // nested full definition belongs to the start zone above.
                    b"endtimezone" if inside_item => {
                        for a in e.attributes().flatten() {
                            if a.key.as_ref().eq_ignore_ascii_case(b"Id") {
                                current.end_time_zone =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    b"timezonedefinition"
                        if inside_start_timezone && current.start_time_zone.is_none() =>
                    {
                        // Full-definition form: the id sits one level down.
                        for a in e.attributes().flatten() {
                            if a.key.as_ref().eq_ignore_ascii_case(b"Id") {
                                current.start_time_zone =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                // Recurrence outer-end → finish walker, assign to
                // current ParsedItem (or drop with a Protocol error
                // if the subtree was malformed). We DON'T forward
                // this End to the walker — the walker handles
                // inner Ends only via observe_end_generic, which
                // we already routed in the bottom branch.
                if local.as_slice() == b"recurrence" {
                    if let Some(walker) = recurrence_walker.take() {
                        // Swallow `finish()` errors deliberately —
                        // unsupported shapes (Relative*) or malformed
                        // subtrees should only nuke THIS row's
                        // recurrence, not the whole sync drain.
                        // The row stays in the cache as a single
                        // event at its master start; the user can
                        // still see / dismiss it.
                        if let Ok(rec) = walker.finish() {
                            current.recurrence = Some(rec);
                        }
                    }
                    text_target = None;
                    continue;
                }
                // Inside the recurrence subtree all other Ends just
                // clear the walker's text target.
                if let Some(walker) = recurrence_walker.as_mut() {
                    walker.observe_end_generic();
                    continue;
                }
                match local.as_slice() {
                    b"deletedoccurrences" => {
                        inside_deleted_occurrences = false;
                    }
                    b"deletedoccurrence" => {
                        inside_deleted_occurrence = false;
                    }
                    b"modifiedoccurrences" => {
                        inside_modified_occurrences = false;
                    }
                    b"starttimezone" => inside_start_timezone = false,
                    b"occurrence" if inside_modified_occurrence => {
                        inside_modified_occurrence = false;
                        if let Some(o) = std::mem::take(&mut current_override).finish() {
                            current.modified_occurrences.push(o);
                        }
                    }
                    b"create" => {
                        if !current.item_id.is_empty() {
                            changes.push(SyncChange::Create(std::mem::take(&mut current)));
                        }
                        inside_change_kind = None;
                        inside_item = false;
                    }
                    b"update" => {
                        if !current.item_id.is_empty() {
                            changes.push(SyncChange::Update(std::mem::take(&mut current)));
                        }
                        inside_change_kind = None;
                        inside_item = false;
                    }
                    b"delete" => {
                        if !current.item_id.is_empty() {
                            let id = std::mem::take(&mut current.item_id);
                            current = ParsedItem::default();
                            changes.push(SyncChange::Delete(id));
                        }
                        inside_change_kind = None;
                    }
                    b"calendaritem" => {
                        // Item completes inside Create/Update — the
                        // wrapping End above is what actually emits
                        // the change.
                        inside_item = false;
                    }
                    _ => {}
                }
                text_target = None;
            }
            Ok(XmlEvent::Text(t)) => {
                let raw = match t.unescape() {
                    Ok(c) => c.to_string(),
                    Err(_) => continue,
                };
                let s = raw.trim();
                if s.is_empty() {
                    continue;
                }
                // Recurrence subtree text routes to the walker,
                // bypassing the outer item field map entirely.
                if let Some(walker) = recurrence_walker.as_mut() {
                    walker.observe_text(s);
                    continue;
                }
                if text_target.is_none() {
                    continue;
                }
                match text_target {
                    Some("sync_state") => new_sync_state.push_str(s),
                    Some("includes_last") => {
                        includes_last = s.eq_ignore_ascii_case("true");
                    }
                    Some("subject") => current.subject.push_str(s),
                    Some("body") => {
                        let acc = current.body.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    Some("location") => {
                        let acc = current.location.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    Some("start") => current.start = parse_ews_datetime(s),
                    Some("end") => current.end = parse_ews_datetime(s),
                    Some("original_start") => current.original_start = parse_ews_datetime(s),
                    Some("deleted_occurrence_start") => {
                        if let Some(dt) = parse_ews_datetime(s) {
                            current.deleted_occurrence_starts.push(dt);
                        }
                    }
                    Some("override_start") => {
                        current_override.start = parse_ews_datetime(s);
                    }
                    Some("override_end") => {
                        current_override.end = parse_ews_datetime(s);
                    }
                    Some("override_original_start") => {
                        current_override.original_start = parse_ews_datetime(s);
                    }
                    Some("all_day") => {
                        current.is_all_day = s.eq_ignore_ascii_case("true");
                    }
                    Some("recurring") => {
                        current.is_recurring = s.eq_ignore_ascii_case("true");
                    }
                    Some("cancelled") => {
                        current.cancelled = s.eq_ignore_ascii_case("true");
                    }
                    Some("appointment_state") => {
                        current.appointment_state = s.parse::<i32>().ok();
                    }
                    Some("reminder_on") => {
                        current.reminder_is_set = s.eq_ignore_ascii_case("true");
                    }
                    Some("reminder_mins") => {
                        current.reminder_minutes_before_start = s.parse::<i64>().ok();
                    }
                    Some("created") => current.created = parse_ews_datetime(s),
                    Some("modified") => current.last_modified = parse_ews_datetime(s),
                    Some("item_type") => {
                        let acc = current.item_type.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(EwsError::Protocol(format!(
                    "SyncFolderItems xml parse: {err}"
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    if new_sync_state.is_empty() {
        return Err(EwsError::Protocol(
            "SyncFolderItems response missing SyncState".into(),
        ));
    }

    Ok(SyncFolderItemsResult {
        changes,
        new_sync_state,
        includes_last,
    })
}

/// Result of one IdOnly `SyncFolderItems` probe page. The Tasks/Contacts
/// delta read only needs to know "did anything change?" and the fresh
/// cookie — not the item details — so this skips per-item parsing and
/// just counts the Create/Update/Delete wrappers.
#[derive(Debug, Clone)]
pub struct SyncProbe {
    /// Number of Create/Update/Delete changes on this page (ReadFlagChange
    /// — mail-only — is deliberately not counted).
    pub change_count: usize,
    pub new_sync_state: String,
    pub includes_last: bool,
}

/// Walk an IdOnly `SyncFolderItemsResponse` and report only the change
/// count + cookie + last-page flag. Item-type agnostic: it counts the
/// `<t:Create>` / `<t:Update>` / `<t:Delete>` wrappers without caring
/// whether they hold a Task, Contact or anything else, so the same probe
/// drives both the Tasks and Contacts delta paths.
pub fn parse_sync_folder_items_counts(xml: &str) -> EwsResult<SyncProbe> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut change_count = 0usize;
    let mut new_sync_state = String::new();
    let mut includes_last = false;
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                match local.as_slice() {
                    b"create" | b"update" | b"delete" => change_count += 1,
                    b"syncstate" => text_target = Some("sync_state"),
                    b"includeslastiteminrange" => text_target = Some("includes_last"),
                    _ => {}
                }
            }
            Ok(XmlEvent::End(_)) => text_target = None,
            Ok(XmlEvent::Text(t)) if text_target.is_some() => {
                let raw = match t.unescape() {
                    Ok(c) => c.to_string(),
                    Err(_) => continue,
                };
                let s = raw.trim();
                if s.is_empty() {
                    continue;
                }
                match text_target {
                    Some("sync_state") => new_sync_state.push_str(s),
                    Some("includes_last") => {
                        includes_last = s.eq_ignore_ascii_case("true");
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(EwsError::Protocol(format!(
                    "SyncFolderItems probe xml parse: {err}"
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    if new_sync_state.is_empty() {
        return Err(EwsError::Protocol(
            "SyncFolderItems probe response missing SyncState".into(),
        ));
    }

    Ok(SyncProbe {
        change_count,
        new_sync_state,
        includes_last,
    })
}

// ── GetItem (recurrence enrichment) ──────────────────────────────────────
//
// `SyncFolderItems` honours most of `AdditionalProperties` but *drops*
// the complex calendar properties (`Recurrence`, `ModifiedOccurrences`,
// `DeletedOccurrences`) from the response regardless of what we ask
// for — a well-known EWS quirk. To pick those up we follow Outlook's
// own playbook: after each sync drain, do a batched `GetItem` against
// every RecurringMaster id and merge the recurrence shape back into
// the cached state.
//
// The walker below mirrors `parse_sync_folder_items_response`'s
// inner-item state machine (recurrence subtree + modified/deleted
// occurrence collection) but skips the Create/Update/Delete framing
// since GetItem just streams `<m:Items>/<t:CalendarItem>` blocks
// directly under each `<m:GetItemResponseMessage>`.

/// Walk a `GetItemResponse` body and yield one `ParsedItem` per
/// `<t:CalendarItem>` block. Designed for the recurrence-enrichment
/// fan-out — populates `recurrence`, `modified_occurrences`, and
/// `deleted_occurrence_starts` on every master in the batch.
///
/// The base CalendarItem fields (subject, start, end, …) come back
/// populated too because we always re-request the small set in
/// `get_calendar_items_with_recurrence`'s ItemShape; the caller
/// typically discards them and only keeps the recurrence fields.
pub fn parse_get_calendar_items_response(xml: &str) -> EwsResult<Vec<ParsedItem>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut items: Vec<ParsedItem> = Vec::new();
    let mut inside_item = false;
    let mut current = ParsedItem::default();
    let mut text_target: Option<&'static str> = None;
    let mut recurrence_walker: Option<RecurrenceWalker> = None;
    // `<t:StartTimeZone>` carries the master's Windows zone id (directly, or on
    // a nested `<t:TimeZoneDefinition>`); track that we're inside it so the
    // nested-form id isn't picked up from EndTimeZone.
    let mut inside_start_timezone = false;
    let mut inside_deleted_occurrences = false;
    let mut inside_deleted_occurrence = false;
    let mut inside_modified_occurrences = false;
    let mut inside_modified_occurrence = false;
    let mut current_override = ModifiedOccurrenceBuilder::default();
    // Attendee / organizer subtree state.
    let mut inside_attendees = false;
    let mut inside_attendee = false;
    let mut inside_organizer = false;
    let mut inside_mailbox = false;
    let mut current_attendee = EwsAttendee::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                // Recurrence subtree gets routed to the shared walker
                // — same logic as in `parse_sync_folder_items_response`.
                if let Some(walker) = recurrence_walker.as_mut() {
                    walker.observe_start(local.as_slice());
                    continue;
                }
                match local.as_slice() {
                    b"calendaritem" => {
                        inside_item = true;
                        current = ParsedItem::default();
                    }
                    // Master's ItemId. The `!inside_modified_occurrence`
                    // guard mirrors the SyncFolderItems parser — the
                    // override's nested ItemId must not clobber it.
                    b"itemid" if inside_item && !inside_modified_occurrence => {
                        for a in e.attributes().flatten() {
                            let key = a.key.as_ref();
                            if key.eq_ignore_ascii_case(b"Id") {
                                current.item_id = String::from_utf8_lossy(&a.value).into_owned();
                            } else if key.eq_ignore_ascii_case(b"ChangeKey") {
                                current.change_key =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    b"recurrence" if inside_item => {
                        recurrence_walker = Some(RecurrenceWalker::default());
                    }
                    b"deletedoccurrences" if inside_item => {
                        inside_deleted_occurrences = true;
                    }
                    b"deletedoccurrence" if inside_deleted_occurrences => {
                        inside_deleted_occurrence = true;
                    }
                    b"modifiedoccurrences" if inside_item => {
                        inside_modified_occurrences = true;
                    }
                    b"occurrence" if inside_modified_occurrences => {
                        inside_modified_occurrence = true;
                        current_override = ModifiedOccurrenceBuilder::default();
                    }
                    b"itemid" if inside_modified_occurrence => {
                        for a in e.attributes().flatten() {
                            let key = a.key.as_ref();
                            if key.eq_ignore_ascii_case(b"Id") {
                                current_override.item_id =
                                    String::from_utf8_lossy(&a.value).into_owned();
                            } else if key.eq_ignore_ascii_case(b"ChangeKey") {
                                current_override.change_key =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    b"subject" if inside_item => text_target = Some("subject"),
                    b"body" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("body");
                    }
                    // What an OCCURRENCE owns beside its times. The shape only
                    // began asking for these with decision 58a: an exception
                    // read here used to keep the series' location, reminder and
                    // all-day flag because nothing ever parsed its own.
                    b"location" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("location");
                    }
                    b"isalldayevent" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("all_day");
                    }
                    b"reminderisset" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("reminder_on");
                    }
                    b"reminderminutesbeforestart" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("reminder_mins");
                    }
                    b"datetimecreated" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("created");
                    }
                    b"lastmodifiedtime" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("modified");
                    }
                    b"start" if inside_deleted_occurrence => {
                        text_target = Some("deleted_occurrence_start");
                    }
                    b"start" if inside_modified_occurrence => {
                        text_target = Some("override_start");
                    }
                    b"end" if inside_modified_occurrence => {
                        text_target = Some("override_end");
                    }
                    b"originalstart" if inside_modified_occurrence => {
                        text_target = Some("override_original_start");
                    }
                    // An occurrence or exception read on its own: the slot it
                    // fills (see `ParsedItem::original_start`).
                    b"originalstart" if inside_item => text_target = Some("original_start"),
                    b"start" if inside_item => text_target = Some("start"),
                    b"end" if inside_item => text_target = Some("end"),
                    b"isrecurring" if inside_item => text_target = Some("recurring"),
                    b"iscancelled" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("cancelled");
                    }
                    b"appointmentstate" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("appointment_state");
                    }
                    b"calendaritemtype" if inside_item => text_target = Some("item_type"),
                    // Organizer + attendee subtree. The `Mailbox`
                    // (EmailAddress/Name) is shared by both, so the
                    // text targets are scoped by the enclosing flag.
                    b"organizer" if inside_item => inside_organizer = true,
                    b"myresponsetype" if inside_item && !inside_modified_occurrence => {
                        text_target = Some("my_response_type");
                    }
                    b"requiredattendees" | b"optionalattendees" if inside_item => {
                        inside_attendees = true;
                    }
                    b"attendee" if inside_attendees => {
                        inside_attendee = true;
                        current_attendee = EwsAttendee::default();
                    }
                    b"mailbox" if inside_attendee || inside_organizer => {
                        inside_mailbox = true;
                    }
                    b"emailaddress" if inside_mailbox && inside_attendee => {
                        text_target = Some("attendee_email");
                    }
                    b"emailaddress" if inside_mailbox && inside_organizer => {
                        text_target = Some("organizer_email");
                    }
                    b"name" if inside_mailbox && inside_attendee => {
                        text_target = Some("attendee_name");
                    }
                    b"responsetype" if inside_attendee => {
                        text_target = Some("attendee_response");
                    }
                    b"starttimezone" if inside_item => {
                        inside_start_timezone = true;
                        for a in e.attributes().flatten() {
                            if a.key.as_ref().eq_ignore_ascii_case(b"Id") {
                                current.start_time_zone =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    // As in the sync parser: the end zone's own id only.
                    b"endtimezone" if inside_item => {
                        for a in e.attributes().flatten() {
                            if a.key.as_ref().eq_ignore_ascii_case(b"Id") {
                                current.end_time_zone =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    b"timezonedefinition"
                        if inside_start_timezone && current.start_time_zone.is_none() =>
                    {
                        for a in e.attributes().flatten() {
                            if a.key.as_ref().eq_ignore_ascii_case(b"Id") {
                                current.start_time_zone =
                                    Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local.as_slice() == b"recurrence" {
                    if let Some(walker) = recurrence_walker.take() {
                        // Server occasionally hands back a malformed
                        // recurrence (e.g. an unsupported Relative*
                        // shape). Swallow the error so one bad master
                        // doesn't blow up the whole batch — the
                        // caller will just see `recurrence=None` for
                        // that row and render it as a single event.
                        if let Ok(rec) = walker.finish() {
                            current.recurrence = Some(rec);
                        }
                    }
                    text_target = None;
                    continue;
                }
                if let Some(walker) = recurrence_walker.as_mut() {
                    walker.observe_end_generic();
                    continue;
                }
                match local.as_slice() {
                    b"deletedoccurrences" => inside_deleted_occurrences = false,
                    b"deletedoccurrence" => inside_deleted_occurrence = false,
                    b"modifiedoccurrences" => inside_modified_occurrences = false,
                    b"starttimezone" => inside_start_timezone = false,
                    b"occurrence" if inside_modified_occurrence => {
                        inside_modified_occurrence = false;
                        if let Some(o) = std::mem::take(&mut current_override).finish() {
                            current.modified_occurrences.push(o);
                        }
                    }
                    b"attendee" if inside_attendee => {
                        inside_attendee = false;
                        if !current_attendee.email.trim().is_empty() {
                            current
                                .attendees
                                .push(std::mem::take(&mut current_attendee));
                        }
                    }
                    b"mailbox" => inside_mailbox = false,
                    b"requiredattendees" | b"optionalattendees" => inside_attendees = false,
                    b"organizer" => inside_organizer = false,
                    b"calendaritem" => {
                        if !current.item_id.is_empty() {
                            items.push(std::mem::take(&mut current));
                        }
                        inside_item = false;
                    }
                    _ => {}
                }
                text_target = None;
            }
            Ok(XmlEvent::Text(t)) => {
                let raw = match t.unescape() {
                    Ok(c) => c.to_string(),
                    Err(_) => continue,
                };
                let s = raw.trim();
                if s.is_empty() {
                    continue;
                }
                if let Some(walker) = recurrence_walker.as_mut() {
                    walker.observe_text(s);
                    continue;
                }
                if text_target.is_none() {
                    continue;
                }
                match text_target {
                    Some("attendee_email") => current_attendee.email.push_str(s),
                    Some("attendee_name") => {
                        current_attendee
                            .name
                            .get_or_insert_with(String::new)
                            .push_str(s);
                    }
                    Some("attendee_response") => {
                        current_attendee
                            .response_type
                            .get_or_insert_with(String::new)
                            .push_str(s);
                    }
                    Some("organizer_email") => {
                        current
                            .organizer
                            .get_or_insert_with(String::new)
                            .push_str(s);
                    }
                    Some("my_response_type") => {
                        current
                            .my_response_type
                            .get_or_insert_with(String::new)
                            .push_str(s);
                    }
                    Some("subject") => current.subject.push_str(s),
                    Some("body") => {
                        let acc = current.body.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    Some("start") => current.start = parse_ews_datetime(s),
                    Some("end") => current.end = parse_ews_datetime(s),
                    Some("original_start") => current.original_start = parse_ews_datetime(s),
                    Some("deleted_occurrence_start") => {
                        if let Some(dt) = parse_ews_datetime(s) {
                            current.deleted_occurrence_starts.push(dt);
                        }
                    }
                    Some("override_start") => {
                        current_override.start = parse_ews_datetime(s);
                    }
                    Some("override_end") => {
                        current_override.end = parse_ews_datetime(s);
                    }
                    Some("override_original_start") => {
                        current_override.original_start = parse_ews_datetime(s);
                    }
                    Some("recurring") => {
                        current.is_recurring = s.eq_ignore_ascii_case("true");
                    }
                    Some("cancelled") => {
                        current.cancelled = s.eq_ignore_ascii_case("true");
                    }
                    Some("appointment_state") => {
                        current.appointment_state = s.parse::<i32>().ok();
                    }
                    Some("location") => {
                        let acc = current.location.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    Some("all_day") => {
                        current.is_all_day = s.eq_ignore_ascii_case("true");
                    }
                    Some("reminder_on") => {
                        current.reminder_is_set = s.eq_ignore_ascii_case("true");
                    }
                    Some("reminder_mins") => {
                        current.reminder_minutes_before_start = s.parse::<i64>().ok();
                    }
                    Some("created") => current.created = parse_ews_datetime(s),
                    Some("modified") => current.last_modified = parse_ews_datetime(s),
                    Some("item_type") => {
                        let acc = current.item_type.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(EwsError::Protocol(format!("GetItem xml parse: {err}")));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(items)
}

/// EWS serialises timestamps as `YYYY-MM-DDTHH:MM:SSZ` (or
/// `YYYY-MM-DDTHH:MM:SS.fffZ`). Both parse cleanly through
/// `DateTime::parse_from_rfc3339`.
fn parse_ews_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Parse a `GetUserAvailability` `CalendarEvent` timestamp.
///
/// Unlike the `Z`-suffixed timestamps the item read path sees,
/// availability times come back **naive** (no offset) — they're
/// expressed in the time zone the request supplied, which we pin to
/// UTC. We accept the rare `Z`-suffixed variant too, then fall back
/// to the naive `YYYY-MM-DDTHH:MM:SS` (optionally with fractional
/// seconds) form, treating it as UTC.
fn parse_availability_datetime(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
        }
    }
    None
}

/// Parse a `GetUserAvailabilityResponse` into one [`FreeBusy`] per
/// requested address.
///
/// EWS returns a `FreeBusyResponseArray` with one `FreeBusyResponse`
/// per mailbox **in request order** — the address itself is never
/// echoed back, so we map results to `emails` by position. Each
/// response carries a `FreeBusyView` whose `CalendarEventArray` lists
/// the mailbox's busy blocks (`StartTime`/`EndTime`/`BusyType`).
///
/// Tolerance is deliberate: a mailbox we aren't allowed to see (or
/// that doesn't resolve) comes back as a `ResponseMessage` tagged
/// `Error` with no `CalendarEventArray`. Rather than abort the whole
/// query, that mailbox simply yields an empty slot list — "availability
/// unknown" — matching the graceful-degradation contract the other
/// providers honour. Genuine transport faults are caught earlier by
/// the HTTP status check; this parser is fed the raw body without the
/// per-message fault check so partial results survive.
///
/// `BusyType` values `Free` and `NoData` are dropped; everything else
/// (`Busy`, `Tentative`, `OOF`, `WorkingElsewhere`) counts as a busy
/// slot.
pub fn parse_get_user_availability(xml: &str, emails: &[&str]) -> EwsResult<Vec<FreeBusy>> {
    // One slot list per requested mailbox, filled by position.
    let mut per_mailbox: Vec<Vec<FreeBusySlot>> = vec![Vec::new(); emails.len()];

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    // -1 until the first `<m:FreeBusyResponse>` bumps it to 0.
    let mut idx: isize = -1;
    let mut in_event = false;
    let mut start: Option<DateTime<Utc>> = None;
    let mut end: Option<DateTime<Utc>> = None;
    let mut busy_type = String::new();
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                match local.as_slice() {
                    b"freebusyresponse" => idx += 1,
                    b"calendarevent" => {
                        in_event = true;
                        start = None;
                        end = None;
                        busy_type.clear();
                    }
                    b"starttime" if in_event => text_target = Some("start"),
                    b"endtime" if in_event => text_target = Some("end"),
                    b"busytype" if in_event => text_target = Some("busy"),
                    _ => {}
                }
            }
            Ok(XmlEvent::Text(t)) => {
                if let Some(target) = text_target {
                    let s = t.unescape().map(|c| c.to_string()).unwrap_or_default();
                    match target {
                        "start" => start = parse_availability_datetime(&s),
                        "end" => end = parse_availability_datetime(&s),
                        "busy" => busy_type.push_str(s.trim()),
                        _ => {}
                    }
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                text_target = None;
                if local.as_slice() == b"calendarevent" {
                    in_event = false;
                    let busy = !matches!(busy_type.as_str(), "Free" | "NoData" | "");
                    if busy {
                        if let (Some(s), Some(en)) = (start, end) {
                            if idx >= 0 && (idx as usize) < per_mailbox.len() {
                                per_mailbox[idx as usize].push(FreeBusySlot { start: s, end: en });
                            }
                        }
                    }
                    start = None;
                    end = None;
                    busy_type.clear();
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => return Err(EwsError::Protocol(format!("xml parse: {err}"))),
            _ => {}
        }
        buf.clear();
    }

    Ok(emails
        .iter()
        .zip(per_mailbox)
        .map(|(email, slots)| FreeBusy {
            email: (*email).to_string(),
            slots,
        })
        .collect())
}

/// Translate a parsed item into a cal-core `Event`. The calendar id is
/// supplied by the caller (the API layer knows which folder we just
/// listed). Recurrence is left as `None` for now — EWS returns
/// `CalendarView` results already expanded, so each row is an
/// individual occurrence rather than a master + RRULE pair.
///
/// **Known gap (deferred):** every occurrence of a recurring series
/// currently arrives in cal-core as an independent single event
/// rather than as a master with an RRULE. The frontend's local
/// expander (`src/intl/recurrence.ts`) therefore never sees a
/// series and series-aware UX (chip, bulk edit, EXDATE skip) doesn't
/// fire on EWS.
///
/// Planned fix: switch the read path away from `FindItem` +
/// `CalendarView` (server-side expansion, no master visible) to
/// `SyncFolderItems` with a persisted sync-state cookie per folder
/// — what Outlook itself uses. A single delta-sync request returns
/// masters with `<t:Recurrence>` inline, plus their
/// `ModifiedOccurrences` (exception overrides) and
/// `DeletedOccurrences` (EXDATEs), with no GetItem fan-out. Local
/// expansion via the existing rrule.js path matches the
/// CalDAV/iCal behaviour. Tracked as its own iteration — needs a
/// sync-state cache, an EWS-Recurrence → RRULE parser (inverse of
/// `rrule_to_ews_recurrence`), and exception/EXDATE handling.
pub fn to_event(item: ParsedItem, calendar_id: &str) -> EwsResult<Event> {
    let start = item
        .start
        .ok_or_else(|| EwsError::Protocol("CalendarItem missing Start".into()))?;
    let end = item
        .end
        .ok_or_else(|| EwsError::Protocol("CalendarItem missing End".into()))?;
    // All-day boundaries re-anchor at LOCAL midnight of their local
    // calendar day (the app-internal convention; see all_day_local_anchor).
    let (start, end) = if item.is_all_day {
        (all_day_local_anchor(start), all_day_local_anchor(end))
    } else {
        (start, end)
    };

    // Resolve cancelled up front, before `item`'s Vec/Option fields are moved
    // out into the Event below (attendees/organizer takes leave `item` partially
    // moved, which would block a later `&item` borrow).
    let cancelled = resolve_cancelled(&item);

    // Prefix the id with the CalendarItemType so writes know how to
    // route — series-wide ops resolve the master from an Occurrence
    // id via a lazy GetItem; the EXDATE path stays on the raw row.
    let kind = item
        .item_type
        .as_deref()
        .map(EventIdKind::from_calendar_item_type)
        .unwrap_or(EventIdKind::Single);
    let id = encode_event_id(kind, &item.item_id, item.change_key.as_deref());

    let reminders = if item.reminder_is_set {
        let minutes = item.reminder_minutes_before_start.unwrap_or(15);
        vec![Reminder {
            kind: ReminderKind::Relative {
                minutes_before: minutes,
            },
            sound: None,
        }]
    } else {
        Vec::new()
    };

    // Two paths produce ParsedItem today:
    //
    //  - Legacy FindItem+CalendarView (parse_find_item_response):
    //    `is_recurring=true` means "this row is one expanded
    //    occurrence". The master's RRULE isn't visible here, so we
    //    fall through with `recurrence = None`.
    //  - SyncFolderItems (parse_sync_folder_items_response): rows
    //    with `is_recurring=true` are masters carrying their
    //    `<t:Recurrence>` element, which the parser already shaped
    //    into `item.recurrence`. We translate it to a cal_core
    //    RRULE + EXDATE list so the frontend expander handles the
    //    series exactly like CalDAV/iCal.
    let recurrence: Option<EventRecurrence> = item.recurrence.as_ref().map(|r| {
        // Each modified occurrence DISPLACES the RRULE slot at
        // `original_start`; the moved instance is emitted as a
        // standalone event by the caller (refresh_and_read_events).
        // We add the original slot to the EXDATE list so the
        // expander doesn't double-render — once at the original
        // (wrong) time and once at the moved time.
        let mut exceptions: Vec<DateTime<Utc>> = Vec::with_capacity(
            item.deleted_occurrence_starts.len() + item.modified_occurrences.len(),
        );
        // For an all-day master, `start`/`end` above are re-anchored to LOCAL
        // midnight, but `deleted_occurrence_starts` / `original_start` arrive as
        // raw "some-zone midnight" instants. The frontend expander anchors the
        // series on the re-anchored `start` and matches EXDATEs by exact instant,
        // so un-anchored exceptions miss on any non-UTC device → the vacated slot
        // isn't suppressed and renders alongside its override (a duplicate row).
        // Anchor the exceptions the same way so they line up with the grid.
        let anchor = |dt: DateTime<Utc>| {
            if item.is_all_day {
                all_day_local_anchor(dt)
            } else {
                dt
            }
        };
        exceptions.extend(item.deleted_occurrence_starts.iter().map(|d| anchor(*d)));
        for o in &item.modified_occurrences {
            exceptions.push(anchor(o.original_start));
        }
        EventRecurrence {
            rrule: r.to_rrule(),
            exceptions,
            // EWS reports the master's zone as Windows ids, read by `windows_tz`:
            // the end zone `tzone://Microsoft/Utc` marks a series created
            // without a zone (43b), and otherwise the start zone becomes
            // tzdata's zone. The id `UTC`, an id the table does not know, or none
            // at all: no zone, so the series repeats in UTC.
            tzid: match crate::windows_tz::read_series_zone(
                item.start_time_zone.as_deref(),
                item.end_time_zone.as_deref(),
            ) {
                Some(crate::windows_tz::WindowsZoneRead::Zone(zone)) => Some(zone.to_string()),
                Some(crate::windows_tz::WindowsZoneRead::Utc) | None => None,
                Some(crate::windows_tz::WindowsZoneRead::Unknown) => {
                    // Debug, not warn: this runs for every cached item on
                    // every reminder scan.
                    tracing::debug!(
                        target: "adapter_ews::zones",
                        item_id = %item.item_id,
                        windows_zone = ?item.start_time_zone,
                        "not a CLDR Windows zone id; the series repeats in UTC",
                    );
                    None
                }
            },
        }
    });

    // Attendees → editable flat list ("Name <email>" / bare) + RSVP state,
    // without the organizer (decision 67a). Exchange lists the organizer as a
    // row whose ResponseType is "Organizer" — for an appointment made in
    // Outlook, as the only row — and says through MyResponseType whether the
    // connected mailbox organizes the item (decision 70a). "Unknown" says
    // nothing, so it counts as no answer.
    let organized_by_me = item
        .my_response_type
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty() && *r != "Unknown")
        .map(|r| r == "Organizer");
    let people = cal_core::attendee::people_from_read(
        item.organizer,
        organized_by_me,
        item.attendees.into_iter().map(|a| {
            let status = a
                .response_type
                .as_deref()
                .map(ews_response_type)
                .unwrap_or_default();
            cal_core::attendee::ReadAttendee {
                is_organizer: a.response_type.as_deref().map(str::trim) == Some("Organizer"),
                email: a.email,
                name: a.name,
                status,
            }
        }),
    );

    Ok(Event {
        keep_attendees: false,
        keep_fields: Vec::new(),
        clear_attendees: false,
        organized_elsewhere: people.organized_elsewhere,
        send_invitations: false,
        truncate_tail_overrides: false,
        id,
        calendar_id: calendar_id.to_string(),
        title: item.subject,
        description: item.body,
        location: item.location,
        start,
        end,
        all_day: item.is_all_day,
        recurrence,
        color_label: None,
        // EWS has no native COLOR; per-event colors are host-local overrides.
        color_hex: None,
        reminders,
        sound: None,
        attendees: people.attendees,
        created_at: item.created.unwrap_or_else(Utc::now),
        updated_at: item.last_modified.unwrap_or_else(Utc::now),
        etag: item.change_key,
        organizer: people.organizer,
        attendee_responses: people.attendee_responses,
        cancelled,
        scheduling_silenced: false,
    })
}

/// Whether a parsed calendar item is cancelled, from the two authoritative EWS
/// signals: the `IsCancelled` flag and the `asfCanceled` (0x4) bit in
/// `AppointmentState`. Shared by `to_event` (whole meetings) and the
/// occurrence-exception enrichment (a single cancelled occurrence of a recurring
/// series — the organizer cancelled just that instance, which arrives as a
/// cancelled exception item; the mailbox auto-processing that records the
/// cancellation sets `IsCancelled` on that exception just as on a whole meeting).
///
/// We deliberately do NOT infer cancellation from a localized "Canceled:" /
/// "Abgesagt:" subject prefix. Exchange's auto-processing that prepends that
/// prefix is the same that flips `IsCancelled`, so the prefix never catches a
/// cancellation the flags miss — it would only ever FALSE-positive on a
/// user-authored title like "Abgesagt: Vertretung klären", wrongly dimming a
/// live meeting and suppressing its reminders.
pub(crate) fn resolve_cancelled(item: &ParsedItem) -> bool {
    let state_cancelled = item
        .appointment_state
        .map(|s| s & 0x4 != 0)
        .unwrap_or(false);
    item.cancelled || state_cancelled
}

/// Map EWS `<t:ResponseType>` to the normalised RSVP enum. `Organizer`
/// (the organizer's own row) reads as an implicit acceptance;
/// `Unknown` / `NoResponseReceived` are "no reply yet".
fn ews_response_type(s: &str) -> AttendeeStatus {
    match s {
        "Accept" => AttendeeStatus::Accepted,
        "Decline" => AttendeeStatus::Declined,
        "Tentative" => AttendeeStatus::Tentative,
        "Organizer" => AttendeeStatus::Accepted,
        _ => AttendeeStatus::NeedsAction,
    }
}

// ── Write side ──────────────────────────────────────────────────────────
//
// The write paths build SOAP request bodies field-by-field rather
// than serialising a single struct, because EWS's update-shape is
// asymmetric: each field changed becomes its own
// `<t:SetItemField>` block with the FieldURI repeated inside a
// stub `<t:CalendarItem>`. We render those blocks into raw strings
// here and let `soap::update_calendar_item` wrap them in an envelope.

use cal_core::NewEvent;

use crate::soap::escape_xml;

/// Build the `<t:CalendarItem>` body that goes inside a `CreateItem`
/// envelope. Mirrors the shape of a CalendarView response row but
/// only the fields Aperio actually supports on the write side.
///
/// Reminders: EWS models a single relative reminder per item, so
/// we pull the first `Relative` entry and ignore the rest. Aperio's
/// UI already enforces a single reminder per event today, so the
/// "ignore the rest" branch is mostly defensive.
///
/// Recurrence: translated by [`rrule_to_ews_recurrence`]; an error
/// in the RRULE bubbles out as a Protocol error so the user gets a
/// clear "this rule isn't supported by EWS" message rather than a
/// silently-non-recurring event.
pub fn new_event_to_calendar_item_xml(event: &NewEvent) -> EwsResult<String> {
    new_event_to_calendar_item_xml_on(event, None)
}

/// [`new_event_to_calendar_item_xml`] for a server whose known Windows zone
/// ids are `server_zones` (decision 41a); `None` when they are unknown.
pub fn new_event_to_calendar_item_xml_on(
    event: &NewEvent,
    server_zones: Option<&ServerTimeZones>,
) -> EwsResult<String> {
    let mut out = String::new();
    out.push_str("        <t:CalendarItem>\n");
    out.push_str(&format!(
        "          <t:Subject>{}</t:Subject>\n",
        escape_xml(&event.title)
    ));
    if let Some(desc) = event.description.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "          <t:Body BodyType=\"Text\">{}</t:Body>\n",
            escape_xml(desc)
        ));
    }
    // Single relative reminder, if any. EWS requires both fields
    // (`ReminderIsSet=true` *and* `ReminderMinutesBeforeStart`) or
    // it ignores the value silently.
    let reminder_minutes = first_relative_reminder_minutes(&event.reminders);
    if let Some(minutes) = reminder_minutes {
        out.push_str("          <t:ReminderIsSet>true</t:ReminderIsSet>\n");
        out.push_str(&format!(
            "          <t:ReminderMinutesBeforeStart>{minutes}</t:ReminderMinutesBeforeStart>\n",
        ));
    } else {
        out.push_str("          <t:ReminderIsSet>false</t:ReminderIsSet>\n");
    }
    // All-day events pin their boundaries to UTC midnight of the LOCAL
    // calendar day (see ews_all_day_boundary); timed events write the
    // instant verbatim.
    let (wire_start, wire_end) = if event.all_day {
        (
            ews_all_day_boundary(event.start),
            ews_all_day_boundary(event.end),
        )
    } else {
        (event.start, event.end)
    };
    out.push_str(&format!(
        "          <t:Start>{}</t:Start>\n",
        format_ews_datetime(wire_start)
    ));
    out.push_str(&format!(
        "          <t:End>{}</t:End>\n",
        format_ews_datetime(wire_end)
    ));
    if event.all_day {
        out.push_str("          <t:IsAllDayEvent>true</t:IsAllDayEvent>\n");
    }
    if let Some(location) = event.location.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "          <t:Location>{}</t:Location>\n",
            escape_xml(location)
        ));
    }
    // Attendees are written whenever present (turning the item into a
    // meeting) regardless of the send flag — whether Exchange EMAILS them is
    // controlled separately by the CreateItem `SendMeetingInvitations`
    // disposition. `RequiredAttendees` must precede `Recurrence` in the EWS
    // CalendarItem element order.
    out.push_str(&required_attendees_xml(&event.attendees));
    if let Some(rec) = &event.recurrence {
        let rec_xml = rrule_to_ews_recurrence(&rec.rrule, event.start)?;
        out.push_str("          ");
        out.push_str(&rec_xml);
        out.push('\n');
    }
    // A zoned recurring master: tell Exchange the series' zone as a Windows id,
    // so it expands the series on that clock server-side. Per the EWS
    // CalendarItemType element order, StartTimeZone/EndTimeZone follow
    // <t:Recurrence>.
    if let Some(windows) =
        series_windows_zone(event.all_day, event.recurrence.as_ref(), server_zones)
    {
        out.push_str(&format!(
            "          <t:StartTimeZone Id=\"{}\"/>\n",
            escape_xml(windows)
        ));
        out.push_str(&format!(
            "          <t:EndTimeZone Id=\"{}\"/>\n",
            escape_xml(windows)
        ));
    }
    out.push_str("        </t:CalendarItem>");
    Ok(out)
}

/// The Windows id a series is written with, by `windows_tz`'s rule; `None`
/// writes no zone. An all-day series writes none (the core's
/// `written_series_zone`, decision 46a): Exchange would move it to the zone's
/// midnights and stretch it over more days. A zone Exchange cannot store —
/// here, or on this server — is logged by name, once per write.
fn series_windows_zone(
    all_day: bool,
    recurrence: Option<&EventRecurrence>,
    server_zones: Option<&ServerTimeZones>,
) -> Option<&'static str> {
    use crate::windows_tz::{windows_zone_for, WindowsZoneWrite};
    let tzid = cal_core::written_series_zone(recurrence.and_then(|r| r.tzid.as_deref()), all_day);
    match windows_zone_for(tzid, server_zones) {
        WindowsZoneWrite::Id(windows) => Some(windows),
        WindowsZoneWrite::NoZone => None,
        WindowsZoneWrite::NotStorable { zone, reason } => {
            tracing::warn!(
                target: "adapter_ews::zones",
                zone,
                ?reason,
                "Exchange cannot store this series time zone; the series is written without one",
            );
            None
        }
    }
}

/// Render the `<t:RequiredAttendees>` block for a CalendarItem from Aperio's
/// flat attendee list (`"Name <email>"` or bare email). Returns an empty
/// string when there are no usable entries, so callers can splice it in
/// unconditionally. EWS is order-sensitive: inside `<t:CalendarItem>` this
/// belongs after `Location` and before `Recurrence`.
fn required_attendees_xml(attendees: &[String]) -> String {
    let mut inner = String::new();
    for entry in attendees {
        let (name, email) = cal_core::attendee::parse(entry);
        if email.is_empty() {
            continue;
        }
        inner.push_str("            <t:Attendee>\n              <t:Mailbox>\n");
        if let Some(name) = name {
            inner.push_str(&format!(
                "                <t:Name>{}</t:Name>\n",
                escape_xml(&name)
            ));
        }
        inner.push_str(&format!(
            "                <t:EmailAddress>{}</t:EmailAddress>\n",
            escape_xml(&email)
        ));
        inner.push_str("              </t:Mailbox>\n            </t:Attendee>\n");
    }
    if inner.is_empty() {
        return String::new();
    }
    format!("          <t:RequiredAttendees>\n{inner}          </t:RequiredAttendees>\n")
}

/// Build the `<t:Updates>` body that goes inside an `UpdateItem`
/// envelope's `<t:ItemChange>`. Returns `(set_fields, delete_fields)`
/// — every field that has a value becomes a `<t:SetItemField>`, and
/// every field that was set on the previous version but is now empty
/// becomes a `<t:DeleteItemField>` so EWS clears it server-side.
///
/// For a single item or a series head; an exception takes
/// [`event_to_update_field_xml_on`] with [`EventIdKind::Exception`].
pub fn event_to_update_field_xml(event: &Event) -> EwsResult<(String, String)> {
    event_to_update_field_xml_on(event, None, None, EventIdKind::Single)
}

/// [`event_to_update_field_xml`] for a server whose known Windows zone ids are
/// `server_zones` (decision 41a); `None` when they are unknown. `target` is the
/// kind of item the update is written to, as `resolve_write_target` found it.
///
/// `before` is the PROVIDER's current copy, read at write time
/// (`api::read_before`). With it, only the fields whose value differs from that
/// copy are written (decision 58a): an update used to set every field it had,
/// so a save that moved an occurrence by an hour also wrote back its title, its
/// body and its reminder, and a save that changed nothing still went out.
///
/// That comparison alone is TWO-way, edit against server: it cannot tell a
/// field the user changed from one this device holds stale. The third side is
/// the copy the editor opened (decision 106): the host marks the fields the
/// edit left as that copy has them ([`Event::keep_fields`]), and those are not
/// written, with or without `before` — the provider's value, whatever it is
/// now, stays. A kept rule that must go along with a moved start is the
/// server's rule, rebuilt on that start.
///
/// Without `before`, every field not kept is written. With it, what is emitted
/// is a SUBSET of what the same event emits without — never a superset, and
/// never another value — except that a kept rule and its zone are the server's
/// own. So a field the COMPARISON suppresses is one whose value the server
/// already has; a field `keep_fields` suppresses may differ from the server's,
/// on purpose.
pub fn event_to_update_field_xml_on(
    event: &Event,
    before: Option<&Event>,
    server_zones: Option<&ServerTimeZones>,
    target: EventIdKind,
) -> EwsResult<(String, String)> {
    let mut set = String::new();
    let mut del = String::new();

    // `None` means "no copy to compare with": then everything the edit did not
    // leave alone is written.
    let changed = before.map(|b| cal_core::event_diff::changed_fields(event, b));
    // Decision 106: a field the edit left exactly as the editor opened it is
    // the provider's. Where the provider's value differs from this device's,
    // that difference is somebody else's change — another device, or an
    // occurrence's own content under a row that inherited the series' — and
    // writing the stale copy back would undo it. The host marks these fields
    // (`Event::keep_fields`); an empty list means nothing is known.
    let may = |field: EventField| !event.keep_fields.contains(&field);
    let touches = |field: EventField| {
        may(field)
            && changed
                .as_ref()
                .is_none_or(|fields| fields.contains(&field))
    };

    if touches(EventField::Title) {
        push_set_string(&mut set, "item:Subject", "Subject", &event.title);
    }
    // Body is SET when present, but NEVER deleted. `SyncFolderItems`
    // doesn't return `<t:Body>`, so the description is loaded lazily
    // via the GetItem enrichment fan-out — and the grid drag-move
    // path edits the cached row directly. If enrichment hasn't run
    // (or the server genuinely has no body), `description` is None,
    // and emitting `DeleteItemField item:Body` would wipe the real
    // server-side description on every such edit. Only push a Set
    // when we actually have a body to write; a deliberate "clear the
    // description" therefore doesn't propagate to EWS (acceptable —
    // far better than silent data loss).
    if touches(EventField::Description) {
        if let Some(desc) = event.description.as_deref().filter(|s| !s.is_empty()) {
            push_set_body(&mut set, desc);
        }
    }
    // The location is the second half of the round-5 defect: an unchanged one
    // was written back on every save, and an absent one was DELETED on every
    // save — including the save of an occurrence that never had one of its own.
    if touches(EventField::Location) {
        match event.location.as_deref().filter(|s| !s.is_empty()) {
            Some(loc) => {
                push_set_string(&mut set, "calendar:Location", "Location", loc);
            }
            None => {
                del.push_str(delete_item_field_xml("calendar:Location").as_str());
            }
        }
    }
    // Whether the start this update would put on the server is not the one
    // already there. Without a copy to compare with, it may be.
    let start_moves = before.is_none_or(|b| b.start != event.start);
    // The slot is ONE fact, written as one group: `ews_all_day_boundary`
    // rewrites both boundaries from `all_day`, and Exchange validates a Start
    // against the End it has stored. Writing one of the three without the
    // others is how a whole update faults, or how a stored instant is silently
    // re-read. Predictable beats minimal.
    //
    // A rule the user changed is built from the edit's start (below), so it
    // brings the slot along when that start is not the server's — a kept start
    // another device has since moved. Otherwise the new rule's first day and
    // the server's start would name different days.
    let slot_changed = touches(EventField::Start)
        || touches(EventField::End)
        || touches(EventField::AllDay)
        || (touches(EventField::Recurrence) && event.recurrence.is_some() && start_moves);
    if slot_changed {
        // Same all-day boundary pinning as the create path.
        let (wire_start, wire_end) = if event.all_day {
            (
                ews_all_day_boundary(event.start),
                ews_all_day_boundary(event.end),
            )
        } else {
            (event.start, event.end)
        };
        push_set_datetime(&mut set, "calendar:Start", "Start", wire_start);
        push_set_datetime(&mut set, "calendar:End", "End", wire_end);
        push_set_bool(
            &mut set,
            "calendar:IsAllDayEvent",
            "IsAllDayEvent",
            event.all_day,
        );
    }

    if touches(EventField::Reminders) {
        let reminder_minutes = first_relative_reminder_minutes(&event.reminders);
        push_set_bool(
            &mut set,
            "item:ReminderIsSet",
            "ReminderIsSet",
            reminder_minutes.is_some(),
        );
        if let Some(minutes) = reminder_minutes {
            // ReminderMinutesBeforeStart is an integer field, not a string.
            push_set_raw(
                &mut set,
                "item:ReminderMinutesBeforeStart",
                "ReminderMinutesBeforeStart",
                &minutes.to_string(),
            );
        }
    }
    // NB: when there's no reminder we DON'T `DeleteItemField`
    // ReminderMinutesBeforeStart. EWS refuses that delete with
    // `ErrorInvalidPropertyDelete` — the property always carries a
    // value server-side (a default), so it isn't deletable. Setting
    // `ReminderIsSet=false` above is the canonical way to turn a
    // reminder off; the stale minutes value is then ignored by the
    // server and by Outlook. (Deleting it was the cause of the
    // "Die Löschaktion wird für diese Eigenschaft nicht unterstützt"
    // failure when editing a recurring series.)

    // Attendees: SET when present (stored as a meeting). An empty list alone
    // emits no DeleteItemField, so an edit that never touched the attendee
    // list cannot mass-uninvite. Whether attendees are EMAILED is governed by
    // the envelope's SendMeetingInvitationsOrCancellations.
    // Not at all when the edit left the invitees alone (decision 71a): the list
    // on the server keeps what Aperio does not show, the organizer's own row.
    // Cleared, both collections the read merges, only when the host says the
    // edit removed every invitee it had read (decision 74a).
    let attendees_xml = if event.keep_attendees {
        String::new()
    } else {
        required_attendees_xml(&event.attendees)
    };
    if !attendees_xml.is_empty() {
        set.push_str(&format!(
            "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:RequiredAttendees\"/>\n              <t:CalendarItem>\n{attendees_xml}              </t:CalendarItem>\n            </t:SetItemField>\n"
        ));
    } else if event.clear_attendees && !event.keep_attendees {
        del.push_str(delete_item_field_xml("calendar:RequiredAttendees").as_str());
        del.push_str(delete_item_field_xml("calendar:OptionalAttendees").as_str());
    }
    // The rule Exchange stores is the BUILT one, and that depends on the start
    // as well: the range's StartDate is `event.start`'s date, and a rule
    // without BYDAY, BYMONTHDAY or BYMONTH takes those from the start too. The
    // rrule text carries none of it, so a series dragged to another day
    // compares equal on the text alone and would keep its old StartDate on the
    // server. So the rule is asked twice: as text, which also catches a rule
    // that no longer builds, and as what would go on the wire.
    //
    // Who may cause the second: the rule itself when the edit did not leave
    // it alone; or a slot that moves the server's start, for an event that has
    // a rule — a kept rule is then rebuilt from the start it now stands on.
    // A kept rule under a start that stays is left as the server has it, so
    // another device's COUNT or UNTIL survives. And the slot never deletes a
    // rule: an edit without one says nothing about the server's.
    // A missing copy's rule is unknown, not equal: a blind write whose slot
    // moves writes the edit's rule with it.
    //
    // A KEPT rule is the server's (decision 106): when the slot takes it along,
    // the server's rule is rebuilt on the start being written, never this
    // device's copy of it, which may be stale — another device's COUNT or
    // UNTIL would go. Its zone with it: `same_recurrence` counts the zone as
    // part of the rule. And a server copy without a rule stays without one.
    let rule_of: &Event = match before {
        Some(server) if !may(EventField::Recurrence) => server,
        _ => event,
    };
    let built_rule = |ev: &Event| {
        ev.recurrence
            .as_ref()
            .map(|rec| rrule_to_ews_recurrence(&rec.rrule, ev.start).ok())
    };
    let written_rule = rule_of
        .recurrence
        .as_ref()
        .map(|rec| rrule_to_ews_recurrence(&rec.rrule, event.start).ok());
    let rule_follows_slot = slot_changed && start_moves && event.recurrence.is_some();
    let rule_changed = touches(EventField::Recurrence)
        || ((may(EventField::Recurrence) || rule_follows_slot)
            && written_rule != before.and_then(built_rule));
    // Two locks on the rule, on purpose. The diff is one: an exception has no
    // rule on either side, so nothing is emitted. The `target` check below is
    // the other, and it is the one that still holds when there is no `before`.
    // Together they also stop an ordinary single's save from carrying a
    // pointless `DeleteItemField calendar:Recurrence` every time.
    if rule_changed {
        if let Some(rec) = &rule_of.recurrence {
            let rec_xml = rrule_to_ews_recurrence(&rec.rrule, event.start)?;
            // Wrap the recurrence element in a SetItemField against
            // calendar:Recurrence. EWS expects the body's inner shape to
            // start with `<t:CalendarItem>` containing the recurrence.
            set.push_str(&format!(
                "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:Recurrence\"/>\n              <t:CalendarItem>\n                {rec_xml}\n              </t:CalendarItem>\n            </t:SetItemField>\n",
            ));
        } else if target != EventIdKind::Exception {
            // An exception has no rule of its own to clear. Exchange refuses to
            // delete one there (`ErrorInvalidPropertyDelete`, live test round 3),
            // and the whole update fails with it.
            del.push_str(delete_item_field_xml("calendar:Recurrence").as_str());
        }
    }
    // Keep the zone on a zoned recurring master so a server-side edit doesn't
    // drop it and re-expand the series in UTC, by the same rule as a create.
    //
    // `series_windows_zone` is a pure function of (all_day, recurrence), so
    // those two are the exact gate; the slot rides along as insurance for the
    // unmeasured claim above that a server-side edit can drop the zone. A
    // time-only gate would drop it on a rule-only change — weekly to daily
    // without moving the series.
    let zone_may_change = rule_changed || touches(EventField::AllDay) || slot_changed;
    if let Some(windows) =
        series_windows_zone(event.all_day, rule_of.recurrence.as_ref(), server_zones)
            .filter(|_| zone_may_change)
    {
        let win = escape_xml(windows);
        set.push_str(&format!(
            "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:StartTimeZone\"/>\n              <t:CalendarItem>\n                <t:StartTimeZone Id=\"{win}\"/>\n              </t:CalendarItem>\n            </t:SetItemField>\n            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:EndTimeZone\"/>\n              <t:CalendarItem>\n                <t:EndTimeZone Id=\"{win}\"/>\n              </t:CalendarItem>\n            </t:SetItemField>\n",
        ));
    }

    Ok((set, del))
}

fn first_relative_reminder_minutes(reminders: &[Reminder]) -> Option<i64> {
    reminders.iter().find_map(|r| match &r.kind {
        ReminderKind::Relative { minutes_before } => Some(*minutes_before),
        _ => None,
    })
}

/// Format a `DateTime<Utc>` as `YYYY-MM-DDTHH:MM:SSZ`. EWS accepts
/// the optional fractional-second form too, but the no-fraction form
/// is what Outlook itself sends, so we stay on the well-trodden path.
fn format_ews_datetime(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// All-day boundary for the wire: UTC midnight of the LOCAL calendar day.
/// Exchange normalises all-day Start/End to whole days in the request's
/// timezone context (UTC for us) — writing the raw boundary instant (a
/// local midnight, e.g. 22:00Z of the previous day for UTC+2) would pin
/// the event to the WRONG day. The internal end is already exclusive
/// (next day's midnight), which is the whole-day span EWS expects.
fn ews_all_day_boundary(when: DateTime<Utc>) -> DateTime<Utc> {
    let day = when.with_timezone(&Local).date_naive();
    Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
}

/// Re-anchor an all-day boundary read from EWS at LOCAL midnight of the
/// intended calendar day — the app-internal all-day convention.
///
/// The `Event` for ONE changed occurrence of a series.
///
/// With the occurrence's own item in hand (`ov.own`, read by the enrichment),
/// every field the OCCURRENCE owns comes from it — through the same
/// [`to_event`] every other row goes through, so the all-day anchor, the
/// organizer and attendee rules (67a, 70a) and the timestamp fallbacks are the
/// ones already written down, not a second copy of them. Only what the SERIES
/// owns, or what no provider stores, is taken from the master.
///
/// Without it the row is the master's content at the occurrence's own slot,
/// exactly as before decision 58a: a row with an INHERITED subject is wrong,
/// but a row with a GUESSED one would be worse.
pub fn override_event(
    master_ev: &Event,
    master_item: &ParsedItem,
    ov: &ModifiedOccurrence,
    calendar_id: &str,
) -> EwsResult<Event> {
    let Some(own) = ov.own.as_deref() else {
        return Ok(inherited_override_event(master_ev, master_item, ov));
    };
    let mut row = to_event(own.clone(), calendar_id)?;
    row.id = encode_override_event_id(&master_ev.id, ov.original_start);
    // An exception carries no rule of its own; the master keeps the series.
    row.recurrence = None;
    // The slot comes from the master's own list, which is what the expander
    // vacated. All-day is read off the OCCURRENCE now, so a row's flag and its
    // boundaries always agree — the master's flag decided this before, and a
    // series can hold an occurrence that is not all-day.
    if row.all_day {
        row.start = all_day_local_anchor(ov.start);
        row.end = all_day_local_anchor(ov.end);
    } else {
        row.start = ov.start;
        row.end = ov.end;
    }
    row.etag = ov.change_key.clone();
    // Cancelled from either side: the organizer withdrew this one instance, or
    // the whole series is gone.
    row.cancelled = master_ev.cancelled || ov.cancelled || row.cancelled;
    // Device-local or series-owned, and never stored per occurrence: EWS keeps
    // no colour at all, and the calendar is the master's.
    row.calendar_id = master_ev.calendar_id.clone();
    row.color_label = master_ev.color_label.clone();
    row.color_hex = master_ev.color_hex.clone();
    row.scheduling_silenced = master_ev.scheduling_silenced;
    // `to_event` falls back to "now" for a missing timestamp, which would make
    // an occurrence's `updated_at` churn on every read and beat the cache. A
    // timestamp the server did not give is the master's.
    if own.created.is_none() {
        row.created_at = master_ev.created_at;
    }
    if own.last_modified.is_none() {
        row.updated_at = master_ev.updated_at;
    }
    Ok(row)
}

/// The pre-58a row: the master's content at the occurrence's own slot. Used
/// where the occurrence's own item could not be read.
fn inherited_override_event(
    master_ev: &Event,
    master_item: &ParsedItem,
    ov: &ModifiedOccurrence,
) -> Event {
    let mut row = master_ev.clone();
    row.id = encode_override_event_id(&master_ev.id, ov.original_start);
    row.recurrence = None;
    if master_item.is_all_day {
        row.start = all_day_local_anchor(ov.start);
        row.end = all_day_local_anchor(ov.end);
    } else {
        row.start = ov.start;
        row.end = ov.end;
    }
    // Its content is the SERIES', so its version must be too. The occurrence's
    // own key alone would give this row and the row of the occurrence's own
    // copy the same ETag, though they say different things — and the host
    // takes an equal ETag as proof that the editor opened this very content
    // (decision 106). Marked, and naming the master's key, so a flip between
    // the two, or a change to the series, is never the same version. Nothing
    // sends a row's ETag to Exchange: a write reads the item first and uses
    // the key that read returns.
    row.etag = match (ov.change_key.as_deref(), master_ev.etag.as_deref()) {
        (Some(own), Some(series)) => Some(format!("inherited:{own}:{series}")),
        _ => None,
    };
    row.cancelled = master_ev.cancelled || ov.cancelled;
    row
}

/// EWS hands back a plain instant that is midnight of the intended day
/// in SOME zone (the mailbox timezone, or UTC for boundaries we wrote
/// ourselves) without saying which. Sampling 12 hours INTO the day lands
/// inside the intended day in UTC for any zone offset in (−12h, +12h],
/// so the sample's UTC date recovers the day without guessing the zone.
/// DST edge: fall forward when the local zone skips midnight.
pub(crate) fn all_day_local_anchor(when: DateTime<Utc>) -> DateTime<Utc> {
    let day = (when + chrono::Duration::hours(12)).date_naive();
    let midnight = day.and_hms_opt(0, 0, 0).unwrap();
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|l| l.with_timezone(&Utc))
        .unwrap_or(when)
}

fn push_set_string(out: &mut String, field_uri: &str, tag: &str, value: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n              <t:CalendarItem>\n                <t:{tag}>{value}</t:{tag}>\n              </t:CalendarItem>\n            </t:SetItemField>\n",
        value = escape_xml(value),
    ));
}

fn push_set_raw(out: &mut String, field_uri: &str, tag: &str, raw_inner: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n              <t:CalendarItem>\n                <t:{tag}>{raw_inner}</t:{tag}>\n              </t:CalendarItem>\n            </t:SetItemField>\n",
    ));
}

fn push_set_bool(out: &mut String, field_uri: &str, tag: &str, value: bool) {
    let raw = if value { "true" } else { "false" };
    push_set_raw(out, field_uri, tag, raw);
}

fn push_set_datetime(out: &mut String, field_uri: &str, tag: &str, value: DateTime<Utc>) {
    let raw = format_ews_datetime(value);
    push_set_raw(out, field_uri, tag, &raw);
}

fn push_set_body(out: &mut String, value: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"item:Body\"/>\n              <t:CalendarItem>\n                <t:Body BodyType=\"Text\">{value}</t:Body>\n              </t:CalendarItem>\n            </t:SetItemField>\n",
        value = escape_xml(value),
    ));
}

fn delete_item_field_xml(field_uri: &str) -> String {
    format!(
        "            <t:DeleteItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n            </t:DeleteItemField>\n",
    )
}

// ── RRULE ⇄ EWS recurrence ──────────────────────────────────────────────
//
// EWS models recurrence as a structured pattern + range, much like
// Microsoft Graph. The patterns we support:
//
//   - DAILY                       → DailyRecurrence
//   - WEEKLY [+ BYDAY=…]          → WeeklyRecurrence
//   - MONTHLY + BYMONTHDAY=15     → AbsoluteMonthlyRecurrence
//   - MONTHLY + BYDAY=3WE         → RelativeMonthlyRecurrence
//   - YEARLY  + BYMONTH=3 BYMONTHDAY=15 → AbsoluteYearlyRecurrence
//   - YEARLY  + BYMONTH=3 BYDAY=1FR      → RelativeYearlyRecurrence
//
// And the three ranges:
//
//   - default (no UNTIL/COUNT)    → NoEndRecurrence
//   - COUNT=N                     → NumberedRecurrence
//   - UNTIL=YYYYMMDD[THHMMSSZ]    → EndDateRecurrence
//
// Relative monthly / yearly ("third Wednesday of the month",
// "last weekday of the month") are covered too: a single ordinal
// BYDAY token (`3WE`, `-1FR`) maps straight to DayOfWeekIndex +
// DaysOfWeek, and a multi-day BYDAY + BYSETPOS collapses into one
// of EWS's composite tokens (Day / Weekday / WeekendDay). This is
// the exact inverse of the read path's `push_relative_byday`, so a
// series round-trips EWS → RRULE → EWS without drift.

/// Translate an RFC-5545 RRULE into an EWS `<t:Recurrence>` block.
/// `start` is the master event's start date, used as the
/// recurrence's StartDate (EWS requires it on every range type).
pub fn rrule_to_ews_recurrence(rrule: &str, start: DateTime<Utc>) -> EwsResult<String> {
    let parts = parse_rrule(rrule);
    let freq = parts
        .get("FREQ")
        .cloned()
        .ok_or_else(|| EwsError::Protocol(format!("RRULE missing FREQ: {rrule}")))?;
    let interval = parts
        .get("INTERVAL")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);

    let pattern_xml = match freq.as_str() {
        "DAILY" => {
            format!("<t:DailyRecurrence><t:Interval>{interval}</t:Interval></t:DailyRecurrence>",)
        }
        "WEEKLY" => {
            let days = parts
                .get("BYDAY")
                .map(|v| v.as_str())
                .map(rrule_byday_to_ews_days)
                .transpose()?
                .unwrap_or_else(|| weekday_for(start));
            // Pin the week-start explicitly (honouring WKST, default Monday) so
            // Exchange doesn't expand an INTERVAL>=2 series with the mailbox's
            // own FirstDayOfWeek — which would drift from the RRULE we stored.
            // EWS schema order: Interval, DaysOfWeek, FirstDayOfWeek.
            let first_day = parts
                .get("WKST")
                .map(|w| rrule_byday_to_ews_days(w.as_str()))
                .transpose()?
                .unwrap_or_else(|| "Monday".to_string());
            format!(
                "<t:WeeklyRecurrence><t:Interval>{interval}</t:Interval><t:DaysOfWeek>{days}</t:DaysOfWeek><t:FirstDayOfWeek>{first_day}</t:FirstDayOfWeek></t:WeeklyRecurrence>",
            )
        }
        "MONTHLY" => {
            // Two monthly shapes:
            //   - BYDAY present → relative ("third Wednesday")
            //   - else          → absolute ("the 15th")
            if let Some(byday) = parts.get("BYDAY") {
                let (days_token, index_word) =
                    rrule_relative_byday_to_ews(byday, parts.get("BYSETPOS").map(|s| s.as_str()))?;
                // EWS schema order: Interval, DaysOfWeek, DayOfWeekIndex.
                format!(
                    "<t:RelativeMonthlyRecurrence><t:Interval>{interval}</t:Interval><t:DaysOfWeek>{days_token}</t:DaysOfWeek><t:DayOfWeekIndex>{index_word}</t:DayOfWeekIndex></t:RelativeMonthlyRecurrence>",
                )
            } else {
                let day = parts
                    .get("BYMONTHDAY")
                    .and_then(|v| v.parse::<u8>().ok())
                    .unwrap_or_else(|| {
                        use chrono::Datelike;
                        start.day() as u8
                    });
                format!(
                    "<t:AbsoluteMonthlyRecurrence><t:Interval>{interval}</t:Interval><t:DayOfMonth>{day}</t:DayOfMonth></t:AbsoluteMonthlyRecurrence>",
                )
            }
        }
        "YEARLY" => {
            use chrono::Datelike;
            let month_num = parts
                .get("BYMONTH")
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or_else(|| start.month());
            let month_name = month_number_to_name(month_num).ok_or_else(|| {
                EwsError::Protocol(format!("RRULE BYMONTH out of range: {month_num}"))
            })?;
            if let Some(byday) = parts.get("BYDAY") {
                let (days_token, index_word) =
                    rrule_relative_byday_to_ews(byday, parts.get("BYSETPOS").map(|s| s.as_str()))?;
                // EWS schema order: DaysOfWeek, DayOfWeekIndex, Month.
                format!(
                    "<t:RelativeYearlyRecurrence><t:DaysOfWeek>{days_token}</t:DaysOfWeek><t:DayOfWeekIndex>{index_word}</t:DayOfWeekIndex><t:Month>{month_name}</t:Month></t:RelativeYearlyRecurrence>",
                )
            } else {
                let day = parts
                    .get("BYMONTHDAY")
                    .and_then(|v| v.parse::<u8>().ok())
                    .unwrap_or_else(|| start.day() as u8);
                format!(
                    "<t:AbsoluteYearlyRecurrence><t:DayOfMonth>{day}</t:DayOfMonth><t:Month>{month_name}</t:Month></t:AbsoluteYearlyRecurrence>",
                )
            }
        }
        other => {
            return Err(EwsError::Protocol(format!(
                "RRULE FREQ '{other}' is not supported by Aperio's EWS writer"
            )));
        }
    };

    let start_date = start.format("%Y-%m-%d").to_string();
    let range_xml = if let Some(count_str) = parts.get("COUNT") {
        let count = count_str
            .parse::<u32>()
            .map_err(|_| EwsError::Protocol(format!("RRULE COUNT not numeric: {count_str}")))?;
        format!(
            "<t:NumberedRecurrence><t:StartDate>{start_date}</t:StartDate><t:NumberOfOccurrences>{count}</t:NumberOfOccurrences></t:NumberedRecurrence>",
        )
    } else if let Some(until_str) = parts.get("UNTIL") {
        let end_date = parse_until_date(until_str)
            .ok_or_else(|| EwsError::Protocol(format!("RRULE UNTIL not parseable: {until_str}")))?;
        format!(
            "<t:EndDateRecurrence><t:StartDate>{start_date}</t:StartDate><t:EndDate>{end_date}</t:EndDate></t:EndDateRecurrence>",
        )
    } else {
        format!("<t:NoEndRecurrence><t:StartDate>{start_date}</t:StartDate></t:NoEndRecurrence>",)
    };

    Ok(format!(
        "<t:Recurrence>{pattern_xml}{range_xml}</t:Recurrence>"
    ))
}

/// Naive RFC-5545 RRULE tokeniser. Splits on `;`, then on `=`. We
/// only ever read a handful of keys (FREQ, INTERVAL, BYDAY, …) and
/// the values themselves don't contain `;` or `=`, so a full parser
/// would be overkill.
fn parse_rrule(rrule: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let trimmed = rrule
        .trim()
        .strip_prefix("RRULE:")
        .unwrap_or_else(|| rrule.trim());
    for part in trimmed.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            out.insert(k.trim().to_ascii_uppercase(), v.trim().to_string());
        }
    }
    out
}

/// Translate RRULE's BYDAY (MO,WE,FR) to EWS's space-separated
/// full day names (Monday Wednesday Friday).
fn rrule_byday_to_ews_days(byday: &str) -> EwsResult<String> {
    let mut out = Vec::new();
    for raw in byday.split(',') {
        let tok = raw.trim();
        // BYDAY can carry an ordinal prefix ("1MO", "-1FR"); the
        // weekly branch above doesn't accept those, but the early
        // bail lives in the caller — here we just strip the ordinal
        // off so a stray prefix doesn't crash the day-name lookup.
        let stripped: &str =
            tok.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '+');
        let name = match stripped {
            "MO" => "Monday",
            "TU" => "Tuesday",
            "WE" => "Wednesday",
            "TH" => "Thursday",
            "FR" => "Friday",
            "SA" => "Saturday",
            "SU" => "Sunday",
            other => {
                return Err(EwsError::Protocol(format!(
                    "RRULE BYDAY token not recognised: {other}"
                )));
            }
        };
        out.push(name);
    }
    if out.is_empty() {
        return Err(EwsError::Protocol("RRULE BYDAY is empty".into()));
    }
    Ok(out.join(" "))
}

/// Translate the BYDAY (+ optional BYSETPOS) of a *relative*
/// monthly/yearly RRULE into EWS's `(DaysOfWeek, DayOfWeekIndex)`
/// pair. Inverse of the read path's `push_relative_byday`.
///
/// Two input shapes, matching exactly what the read path emits:
///   - **Single ordinal token** (`3WE`, `-1FR`): the ordinal
///     prefix carries the position; DaysOfWeek is the single day.
///   - **Multi-day list + BYSETPOS** (`MO,TU,WE,TH,FR` +
///     `BYSETPOS=-1`): the day-set is collapsed back into a
///     composite token (`Weekday` / `WeekendDay` / `Day`) — EWS's
///     relative recurrence takes ONE `DaysOfWeekType` value, not a
///     list, so a multi-day set is only representable when it
///     matches a known composite. A non-composite multi-day set
///     (rare; not something Aperio's UI authors) surfaces Protocol.
///
/// Position mapping: 1→First … 4→Fourth; anything ≥5 or negative
/// (RRULE's `-1` "from the end") → Last, since EWS has no "Fifth".
fn rrule_relative_byday_to_ews(byday: &str, bysetpos: Option<&str>) -> EwsResult<(String, String)> {
    let tokens: Vec<&str> = byday
        .split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err(EwsError::Protocol("RRULE BYDAY is empty".into()));
    }

    // Single-token case: ordinal prefix lives on the token itself.
    if tokens.len() == 1 {
        let (ordinal, weekday2) = split_byday_ordinal(tokens[0]);
        let pos = ordinal
            .or_else(|| bysetpos.and_then(|s| s.parse::<i32>().ok()))
            .ok_or_else(|| {
                EwsError::Protocol(format!(
                    "relative recurrence BYDAY '{}' has no ordinal and no BYSETPOS",
                    tokens[0]
                ))
            })?;
        let day_name = byday_weekday_to_ews_name(weekday2)?;
        return Ok((day_name.to_string(), ordinal_to_index_word(pos).to_string()));
    }

    // Multi-token case: every token is a bare weekday; the position
    // comes from BYSETPOS. Collapse the day-set into a composite.
    let pos = bysetpos
        .and_then(|s| s.parse::<i32>().ok())
        .ok_or_else(|| {
            EwsError::Protocol("relative recurrence with a multi-day BYDAY needs BYSETPOS".into())
        })?;
    let mut days: Vec<EwsDay> = Vec::with_capacity(tokens.len());
    for tok in &tokens {
        let (_, weekday2) = split_byday_ordinal(tok);
        let name = byday_weekday_to_ews_name(weekday2)?;
        days.push(EwsDay::from_wire(name).expect("name came from the fixed lookup"));
    }
    let composite = ews_days_to_composite(&days).ok_or_else(|| {
        EwsError::Protocol(
            "relative recurrence day-set doesn't match an EWS composite (Day/Weekday/WeekendDay)"
                .into(),
        )
    })?;
    Ok((
        composite.to_string(),
        ordinal_to_index_word(pos).to_string(),
    ))
}

/// Split a BYDAY token into its optional leading ordinal and the
/// two-letter weekday. `"3WE"` → `(Some(3), "WE")`, `"-1FR"` →
/// `(Some(-1), "FR")`, `"WE"` → `(None, "WE")`.
fn split_byday_ordinal(tok: &str) -> (Option<i32>, &str) {
    let split_at = tok
        .char_indices()
        .find(|(_, c)| c.is_ascii_alphabetic())
        .map(|(i, _)| i)
        .unwrap_or(0);
    let (prefix, weekday) = tok.split_at(split_at);
    let ordinal = if prefix.is_empty() {
        None
    } else {
        prefix.parse::<i32>().ok()
    };
    (ordinal, weekday)
}

fn byday_weekday_to_ews_name(weekday2: &str) -> EwsResult<&'static str> {
    Ok(match weekday2 {
        "MO" => "Monday",
        "TU" => "Tuesday",
        "WE" => "Wednesday",
        "TH" => "Thursday",
        "FR" => "Friday",
        "SA" => "Saturday",
        "SU" => "Sunday",
        other => {
            return Err(EwsError::Protocol(format!(
                "RRULE BYDAY weekday not recognised: {other}"
            )));
        }
    })
}

/// EWS DayOfWeekIndex word for an RRULE ordinal. EWS tops out at
/// "Fourth" + "Last", so a fifth occurrence (`5`) or any negative
/// (`-1` = "from the end") maps to "Last".
fn ordinal_to_index_word(pos: i32) -> &'static str {
    match pos {
        1 => "First",
        2 => "Second",
        3 => "Third",
        4 => "Fourth",
        _ => "Last",
    }
}

/// Collapse a weekday set into an EWS composite token, or `None`
/// if it doesn't match one of the three EWS recognises. A single
/// day returns its own name so the caller can use one code path.
fn ews_days_to_composite(days: &[EwsDay]) -> Option<&'static str> {
    use EwsDay::*;
    // Dedup + sort into a discriminant bitmask so order/repeats in the
    // input don't matter. EwsDay is a fieldless enum (discriminants
    // 0..=6), so `1 << (d as u8)` gives a stable per-day bit.
    let mask: u8 = days.iter().fold(0u8, |acc, d| acc | (1u8 << (*d as u8)));
    let bit = |d: EwsDay| 1u8 << (d as u8);
    let weekday = bit(Monday) | bit(Tuesday) | bit(Wednesday) | bit(Thursday) | bit(Friday);
    let weekend = bit(Saturday) | bit(Sunday);
    if days.len() == 1 {
        return Some(ews_day_name(days[0]));
    }
    if mask == weekday {
        return Some("Weekday");
    }
    if mask == weekend {
        return Some("WeekendDay");
    }
    if mask == weekday | weekend {
        return Some("Day");
    }
    None
}

fn ews_day_name(d: EwsDay) -> &'static str {
    match d {
        EwsDay::Monday => "Monday",
        EwsDay::Tuesday => "Tuesday",
        EwsDay::Wednesday => "Wednesday",
        EwsDay::Thursday => "Thursday",
        EwsDay::Friday => "Friday",
        EwsDay::Saturday => "Saturday",
        EwsDay::Sunday => "Sunday",
    }
}

fn weekday_for(ts: DateTime<Utc>) -> String {
    use chrono::Datelike;
    match ts.weekday() {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    }
    .to_string()
}

fn month_number_to_name(n: u32) -> Option<&'static str> {
    Some(match n {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => return None,
    })
}

/// RRULE UNTIL comes in three shapes: `YYYYMMDD`, `YYYYMMDDTHHMMSS`,
/// `YYYYMMDDTHHMMSSZ`. EWS's EndDate wants `YYYY-MM-DD` — we trim
/// time/timezone and reformat.
fn parse_until_date(until: &str) -> Option<String> {
    let trimmed = until.trim().trim_end_matches('Z');
    let date_part = trimmed.split('T').next()?;
    if date_part.len() != 8 || !date_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "{}-{}-{}",
        &date_part[0..4],
        &date_part[4..6],
        &date_part[6..8]
    ))
}

// ── CreateItem / UpdateItem response parsers ─────────────────────────────

// ── EWS recurrence → RRULE (inverse of rrule_to_ews_recurrence) ───────────
//
// The Outlook-style read path (SyncFolderItems / FindItem without
// CalendarView) delivers recurring series as a single master with a
// `<t:Recurrence>` child carrying the structured pattern + range.
// cal-core's `Event.recurrence.rrule` holds RFC-5545 RRULE strings,
// matching what CalDAV/iCal produce — so the bridge below
// translates the structured EWS shape into an RRULE so the frontend
// expander (`src/intl/recurrence.ts`) renders EWS series the same
// way as the other adapters.
//
// Coverage tracks the writer (rrule_to_ews_recurrence) one-to-one:
//
//   - DailyRecurrence            ↔  FREQ=DAILY[;INTERVAL=n]
//   - WeeklyRecurrence           ↔  FREQ=WEEKLY[;INTERVAL=n][;BYDAY=...]
//   - AbsoluteMonthlyRecurrence  ↔  FREQ=MONTHLY[;INTERVAL=n];BYMONTHDAY=n
//   - RelativeMonthlyRecurrence  ↔  FREQ=MONTHLY[;INTERVAL=n];BYDAY=Nxx
//   - AbsoluteYearlyRecurrence   ↔  FREQ=YEARLY;BYMONTH=n;BYMONTHDAY=n
//   - RelativeYearlyRecurrence   ↔  FREQ=YEARLY;BYMONTH=n;BYDAY=Nxx
//   - NoEndRecurrence            ↔  (no UNTIL / COUNT)
//   - NumberedRecurrence         ↔  COUNT=n
//   - EndDateRecurrence          ↔  UNTIL=YYYYMMDDT235959Z (UTC, to
//                                    match the UTC DTSTART)
//
// Relative monthly/yearly ("first Monday", "last weekday") round-trip
// in both directions now — single-day rules via the BYDAY ordinal
// prefix, composites (Day/Weekday/WeekendDay) via BYDAY + BYSETPOS.

/// Structured `<t:Recurrence>` parsed straight out of EWS XML. Each
/// pattern + range variant carries only the fields RFC 5545 needs;
/// the conversion to RRULE happens via [`Self::to_rrule`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EwsRecurrence {
    pub pattern: EwsRecurrencePattern,
    pub range: EwsRecurrenceRange,
}

/// Pattern half of an EWS recurrence (the "how often" part).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EwsRecurrencePattern {
    Daily {
        interval: u32,
    },
    Weekly {
        interval: u32,
        days_of_week: Vec<EwsDay>,
        /// EWS `<t:FirstDayOfWeek>`. Governs which day starts the week for an
        /// `INTERVAL>=2` weekly rule, so it must reach the RRULE as `WKST=` or a
        /// series that straddles the week boundary expands (and indexes) on the
        /// wrong dates. Exchange defaults it per mailbox (Sunday for en-US);
        /// absent → Monday (the RFC-5545 default).
        first_day_of_week: EwsDay,
    },
    AbsoluteMonthly {
        interval: u32,
        day_of_month: u8,
    },
    AbsoluteYearly {
        day_of_month: u8,
        month: EwsMonth,
    },
    /// "Third Wednesday of every month", "last Friday of every
    /// other month", etc. EWS's RelativeMonthlyRecurrence
    /// element. `days_of_week` is *already* expanded if the wire
    /// carried one of the composite tokens (`Day`, `Weekday`,
    /// `WeekendDay`) — single-day rules end up with one entry,
    /// composites with the set they stand for. The RRULE
    /// translation branches on the length to emit `BYDAY=Nxx`
    /// (single) or `BYDAY=xx,yy,…+BYSETPOS=N` (multi).
    RelativeMonthly {
        interval: u32,
        days_of_week: Vec<EwsDay>,
        day_of_week_index: EwsDayOfWeekIndex,
    },
    /// "Third Wednesday of March every year", etc. Yearly twin of
    /// `RelativeMonthly` — adds the `Month` element.
    RelativeYearly {
        days_of_week: Vec<EwsDay>,
        day_of_week_index: EwsDayOfWeekIndex,
        month: EwsMonth,
    },
}

/// Range half (the "when does it stop" part).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EwsRecurrenceRange {
    NoEnd,
    Numbered { occurrences: u32 },
    EndDate { end: String },
}

/// Day-of-week as EWS spells it. Carrying the variant rather than
/// the wire string keeps the RRULE translation total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EwsDay {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl EwsDay {
    fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "Monday" => Self::Monday,
            "Tuesday" => Self::Tuesday,
            "Wednesday" => Self::Wednesday,
            "Thursday" => Self::Thursday,
            "Friday" => Self::Friday,
            "Saturday" => Self::Saturday,
            "Sunday" => Self::Sunday,
            _ => return None,
        })
    }
    fn to_rrule(self) -> &'static str {
        match self {
            Self::Monday => "MO",
            Self::Tuesday => "TU",
            Self::Wednesday => "WE",
            Self::Thursday => "TH",
            Self::Friday => "FR",
            Self::Saturday => "SA",
            Self::Sunday => "SU",
        }
    }
}

/// Parse a `<t:DaysOfWeek>` token list into a vector of concrete
/// weekdays. EWS allows three composite shortcuts in addition to
/// the seven specific days:
///
///   - `Day` — any day of the week (all 7)
///   - `Weekday` — Mon-Fri
///   - `WeekendDay` — Sat+Sun
///
/// We expand them in-place so downstream RRULE generation only
/// ever sees a flat list of concrete days. Unknown tokens are
/// dropped silently (no crash on a future composite we haven't
/// seen — the worst case is a less-specific recurrence than the
/// server intended).
fn parse_days_of_week(s: &str) -> Vec<EwsDay> {
    use EwsDay::*;
    let mut out: Vec<EwsDay> = Vec::new();
    for tok in s.split_whitespace() {
        match tok {
            "Day" => out.extend([
                Monday, Tuesday, Wednesday, Thursday, Friday, Saturday, Sunday,
            ]),
            "Weekday" => out.extend([Monday, Tuesday, Wednesday, Thursday, Friday]),
            "WeekendDay" => out.extend([Saturday, Sunday]),
            other => {
                if let Some(d) = EwsDay::from_wire(other) {
                    out.push(d);
                }
            }
        }
    }
    out
}

/// Position-within-month for relative recurrences. EWS spells
/// these out as words on the wire; the RRULE translation maps
/// to BYSETPOS / the BYDAY ordinal prefix:
///
///   - First → 1, Second → 2, Third → 3, Fourth → 4
///   - Last → -1 (RRULE convention for "from the end")
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EwsDayOfWeekIndex {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

impl EwsDayOfWeekIndex {
    fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "First" => Self::First,
            "Second" => Self::Second,
            "Third" => Self::Third,
            "Fourth" => Self::Fourth,
            "Last" => Self::Last,
            _ => return None,
        })
    }
    fn to_rrule_pos(self) -> i32 {
        match self {
            Self::First => 1,
            Self::Second => 2,
            Self::Third => 3,
            Self::Fourth => 4,
            Self::Last => -1,
        }
    }
}

/// Calendar month as EWS spells it (full English name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EwsMonth {
    January,
    February,
    March,
    April,
    May,
    June,
    July,
    August,
    September,
    October,
    November,
    December,
}

impl EwsMonth {
    fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "January" => Self::January,
            "February" => Self::February,
            "March" => Self::March,
            "April" => Self::April,
            "May" => Self::May,
            "June" => Self::June,
            "July" => Self::July,
            "August" => Self::August,
            "September" => Self::September,
            "October" => Self::October,
            "November" => Self::November,
            "December" => Self::December,
            _ => return None,
        })
    }
    fn to_rrule_number(self) -> u32 {
        match self {
            Self::January => 1,
            Self::February => 2,
            Self::March => 3,
            Self::April => 4,
            Self::May => 5,
            Self::June => 6,
            Self::July => 7,
            Self::August => 8,
            Self::September => 9,
            Self::October => 10,
            Self::November => 11,
            Self::December => 12,
        }
    }
}

impl EwsRecurrence {
    /// Translate to an RFC 5545 RRULE string. Inverse of
    /// [`rrule_to_ews_recurrence`]; roundtrip-checked in tests.
    pub fn to_rrule(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        match &self.pattern {
            EwsRecurrencePattern::Daily { interval } => {
                parts.push("FREQ=DAILY".into());
                if *interval > 1 {
                    parts.push(format!("INTERVAL={interval}"));
                }
            }
            EwsRecurrencePattern::Weekly {
                interval,
                days_of_week,
                first_day_of_week,
            } => {
                parts.push("FREQ=WEEKLY".into());
                if *interval > 1 {
                    parts.push(format!("INTERVAL={interval}"));
                }
                if !days_of_week.is_empty() {
                    let csv = days_of_week
                        .iter()
                        .map(|d| d.to_rrule())
                        .collect::<Vec<_>>()
                        .join(",");
                    parts.push(format!("BYDAY={csv}"));
                }
                // WKST only changes expansion for INTERVAL>=2, but the RFC-5545
                // default is Monday, so only emit it when it actually differs —
                // keeps the common rule byte-identical to what the frontend
                // builds and validates.
                if *first_day_of_week != EwsDay::Monday {
                    parts.push(format!("WKST={}", first_day_of_week.to_rrule()));
                }
            }
            EwsRecurrencePattern::AbsoluteMonthly {
                interval,
                day_of_month,
            } => {
                parts.push("FREQ=MONTHLY".into());
                if *interval > 1 {
                    parts.push(format!("INTERVAL={interval}"));
                }
                parts.push(format!("BYMONTHDAY={day_of_month}"));
            }
            EwsRecurrencePattern::AbsoluteYearly {
                day_of_month,
                month,
            } => {
                parts.push("FREQ=YEARLY".into());
                parts.push(format!("BYMONTH={}", month.to_rrule_number()));
                parts.push(format!("BYMONTHDAY={day_of_month}"));
            }
            EwsRecurrencePattern::RelativeMonthly {
                interval,
                days_of_week,
                day_of_week_index,
            } => {
                parts.push("FREQ=MONTHLY".into());
                if *interval > 1 {
                    parts.push(format!("INTERVAL={interval}"));
                }
                push_relative_byday(&mut parts, days_of_week, *day_of_week_index);
            }
            EwsRecurrencePattern::RelativeYearly {
                days_of_week,
                day_of_week_index,
                month,
            } => {
                parts.push("FREQ=YEARLY".into());
                parts.push(format!("BYMONTH={}", month.to_rrule_number()));
                push_relative_byday(&mut parts, days_of_week, *day_of_week_index);
            }
        }
        match &self.range {
            EwsRecurrenceRange::NoEnd => {}
            EwsRecurrenceRange::Numbered { occurrences } => {
                parts.push(format!("COUNT={occurrences}"));
            }
            EwsRecurrenceRange::EndDate { end } => {
                // EWS sends EndDate as YYYY-MM-DD. The series' DTSTART
                // is a UTC date-time, and RFC 5545 requires UNTIL to
                // share that value type — i.e. a UTC date-time too. A
                // bare date-only UNTIL is read as floating/local, and
                // the strict `rrule` crate used by the reminder
                // expander rejects the whole rule with
                // `DtStartUntilMismatchTimezone`, degrading the series
                // to just its master start. Emit an end-of-day UTC
                // instant so the rule validates AND the final day's
                // occurrences stay included (UNTIL is inclusive).
                // Matches the frontend's `buildRRule` (`…T235959Z`).
                let compact: String = end.chars().filter(|c| *c != '-').collect();
                parts.push(format!("UNTIL={compact}T235959Z"));
            }
        }
        parts.join(";")
    }
}

/// The 1-based EWS `OccurrenceItemId` InstanceIndex of the occurrence of a
/// recurring master (recurrence `rec`, anchored at `start`) that is nearest
/// `target`.
///
/// Critically, this expands the NOMINAL pattern with NO EXDATEs: EWS
/// InstanceIndex is the position in the ORIGINAL pattern and does NOT renumber
/// when an occurrence is deleted (a deleted occurrence leaves an index "hole").
/// So the ordinal here is stable regardless of prior per-occurrence deletions —
/// which is exactly why a date-based search over live occurrences was wrong.
///
/// Expansion is in UTC to match the UTC `UNTIL` that [`EwsRecurrence::to_rrule`]
/// emits (the `rrule` crate rejects a DTSTART/UNTIL timezone mismatch). A
/// master in a non-UTC zone can therefore land the "nearest" occurrence one
/// index off at a UTC day boundary — the caller GetItem-verifies index ± 1
/// against the server, which recovers that and confirms the real date before
/// deleting. Returns `None` if the rule can't be parsed/expanded.
pub(crate) fn nominal_occurrence_index(
    rec: &EwsRecurrence,
    start: DateTime<Utc>,
    target: DateTime<Utc>,
) -> Option<u32> {
    use rrule::{RRule, RRuleSet, Tz as RruleTz};
    let body = rec.to_rrule();
    let body = body.strip_prefix("RRULE:").unwrap_or(body.as_str());
    let unvalidated: RRule<rrule::Unvalidated> = body.parse().ok()?;
    let dt_start = start.with_timezone(&RruleTz::UTC);
    let validated = unvalidated.validate(dt_start).ok()?;
    // Bound the expansion just past the target so a long series doesn't
    // over-expand; 2 days of slack absorbs any zone/DST skew at the boundary.
    let bound = (target + chrono::Duration::days(2)).with_timezone(&RruleTz::UTC);
    let set = RRuleSet::new(dt_start).rrule(validated).before(bound);
    // The expansion is already date-bounded to just past the target, so this
    // count cap only matters for an absurdly long series; 50k daily occurrences
    // is ~136 years — well past any real meeting, while still bounding a
    // pathological sub-daily rule.
    let dates = set.all(50_000).dates;
    let (pos, _) = dates
        .iter()
        .enumerate()
        .min_by_key(|(_, d)| (d.with_timezone(&Utc) - target).num_seconds().abs())?;
    u32::try_from(pos + 1).ok()
}

/// Emit the BYDAY (+ optional BYSETPOS) parts for a relative
/// monthly / yearly recurrence. Branches on the day-list size:
///
///   - **Single day** (e.g. Wednesday + Third) → `BYDAY=3WE`.
///     The ordinal prefix is RRULE's compact form and rrule.js
///     handles it identically to BYDAY=WE + BYSETPOS=3 but with
///     fewer parts.
///   - **Multiple days** (e.g. Weekday-composite + First → all
///     five workdays) → `BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1`. The
///     BYSETPOS modifier picks the chosen ordinal among the
///     candidates the month yields; that's the only way to
///     express "first weekday of the month" in RRULE.
///   - **Empty** (parser saw an unknown DaysOfWeek token) →
///     nothing pushed. The recurrence won't actually expand on
///     the frontend; better that than a malformed RRULE.
fn push_relative_byday(parts: &mut Vec<String>, days_of_week: &[EwsDay], index: EwsDayOfWeekIndex) {
    let pos = index.to_rrule_pos();
    if days_of_week.len() == 1 {
        parts.push(format!("BYDAY={pos}{}", days_of_week[0].to_rrule()));
        return;
    }
    if days_of_week.is_empty() {
        return;
    }
    let csv = days_of_week
        .iter()
        .map(|d| d.to_rrule())
        .collect::<Vec<_>>()
        .join(",");
    parts.push(format!("BYDAY={csv}"));
    parts.push(format!("BYSETPOS={pos}"));
}

/// Parse a `<t:Recurrence>...</t:Recurrence>` block (the full XML
/// fragment including the wrapping element) into a structured
/// [`EwsRecurrence`]. Returns `Err(Protocol)` only on shapes that
/// genuinely can't be represented (e.g. the response is missing
/// the pattern or range half, or carries an incomplete pattern
/// like `RelativeMonthly` without a `DayOfWeekIndex`). All six
/// EWS recurrence-pattern variants — Daily, Weekly,
/// AbsoluteMonthly, AbsoluteYearly, RelativeMonthly, RelativeYearly
/// — translate to valid RRULE on the read side.
///
/// Shares its actual walking logic with [`RecurrenceWalker`] so the
/// `SyncFolderItems` parser can re-use it inline (no XML-slicing
/// gymnastics required to carve out the subtree).
pub fn parse_ews_recurrence(xml: &str) -> EwsResult<EwsRecurrence> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut walker = RecurrenceWalker::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                walker.observe_start(local.as_slice());
            }
            Ok(XmlEvent::Text(t)) => {
                let raw = match t.unescape() {
                    Ok(c) => c.to_string(),
                    Err(_) => continue,
                };
                let s = raw.trim();
                if !s.is_empty() {
                    walker.observe_text(s);
                }
            }
            Ok(XmlEvent::End(_)) => walker.observe_end_generic(),
            Ok(XmlEvent::Eof) => break,
            Ok(_) => {}
            Err(err) => {
                return Err(EwsError::Protocol(format!(
                    "Recurrence XML parse error: {err}"
                )));
            }
        }
        buf.clear();
    }
    walker.finish()
}

/// Convenience: parse + translate to RRULE in one step. Used by
/// callers that don't need the structured form (most read-path code).
pub fn parse_ews_recurrence_to_rrule(xml: &str) -> EwsResult<String> {
    Ok(parse_ews_recurrence(xml)?.to_rrule())
}

// ── builders (mutable scratch types used during XML walk) ─────────────────

/// Stateful walker over the contents of a `<t:Recurrence>` block.
/// Both the standalone [`parse_ews_recurrence`] and the
/// `SyncFolderItems` walker route events through this so the
/// pattern/range accumulation logic lives in exactly one place.
///
/// Usage: feed every Start/Text/End event you see while you're
/// inside the Recurrence subtree; on the matching outer End, call
/// [`Self::finish`] for the assembled value.
#[derive(Default)]
pub(crate) struct RecurrenceWalker {
    pattern: Option<PatternBuilder>,
    range: Option<RangeBuilder>,
    text_target: Option<&'static str>,
    /// Set when we encounter a recurrence shape Aperio can't
    /// translate (currently nothing — all six EWS pattern variants
    /// are supported on the read path). Kept as a hook so future
    /// EWS extensions can flag themselves here without changing
    /// the walker's signature: the walker keeps consuming the
    /// rest of the subtree quietly and surfaces the error from
    /// [`Self::finish`] instead, so one bad row doesn't abort the
    /// whole batch parse.
    unsupported: Option<&'static str>,
}

impl RecurrenceWalker {
    /// Handle a Start (or Empty) event's local-name. Infallible:
    /// unsupported recurrence shapes are recorded internally and
    /// surfaced at [`Self::finish`] time.
    ///
    /// **Why infallible matters**: a GetItem batch can carry dozens
    /// of masters and one Relative*Recurrence (e.g. "every third
    /// Wednesday") would otherwise propagate out of the parser and
    /// nuke the entire response — including all the singles and
    /// every other series in the same payload. Letting `finish`
    /// surface the error lets the caller localise the failure to
    /// the single offending row.
    pub(crate) fn observe_start(&mut self, local: &[u8]) {
        match local {
            b"dailyrecurrence" => {
                self.pattern = Some(PatternBuilder::Daily { interval: 1 });
            }
            b"weeklyrecurrence" => {
                self.pattern = Some(PatternBuilder::Weekly {
                    interval: 1,
                    days_of_week: Vec::new(),
                    // RFC-5545 default until an explicit <t:FirstDayOfWeek> arrives.
                    first_day_of_week: EwsDay::Monday,
                });
            }
            b"absolutemonthlyrecurrence" => {
                self.pattern = Some(PatternBuilder::AbsoluteMonthly {
                    interval: 1,
                    day_of_month: 1,
                });
            }
            b"absoluteyearlyrecurrence" => {
                self.pattern = Some(PatternBuilder::AbsoluteYearly {
                    day_of_month: 1,
                    month: None,
                });
            }
            b"relativemonthlyrecurrence" => {
                self.pattern = Some(PatternBuilder::RelativeMonthly {
                    interval: 1,
                    days_of_week: Vec::new(),
                    day_of_week_index: None,
                });
            }
            b"relativeyearlyrecurrence" => {
                self.pattern = Some(PatternBuilder::RelativeYearly {
                    days_of_week: Vec::new(),
                    day_of_week_index: None,
                    month: None,
                });
            }
            b"noendrecurrence" => self.range = Some(RangeBuilder::NoEnd),
            b"numberedrecurrence" => {
                self.range = Some(RangeBuilder::Numbered { occurrences: 0 });
            }
            b"enddaterecurrence" => {
                self.range = Some(RangeBuilder::EndDate { end: String::new() });
            }
            b"interval" => self.text_target = Some("interval"),
            b"daysofweek" => self.text_target = Some("days_of_week"),
            b"firstdayofweek" => self.text_target = Some("first_day_of_week"),
            b"dayofweekindex" => self.text_target = Some("day_of_week_index"),
            b"dayofmonth" => self.text_target = Some("day_of_month"),
            b"month" => self.text_target = Some("month"),
            b"numberofoccurrences" => {
                self.text_target = Some("number_of_occurrences");
            }
            b"enddate" => self.text_target = Some("end_date"),
            _ => {}
        }
    }

    /// Feed text content (already trimmed + non-empty). Routed to
    /// whichever element's child text is currently active.
    pub(crate) fn observe_text(&mut self, s: &str) {
        match self.text_target {
            Some("interval") => {
                let v = s.parse::<u32>().unwrap_or(1).max(1);
                match self.pattern.as_mut() {
                    Some(PatternBuilder::Daily { interval })
                    | Some(PatternBuilder::Weekly { interval, .. })
                    | Some(PatternBuilder::AbsoluteMonthly { interval, .. })
                    | Some(PatternBuilder::RelativeMonthly { interval, .. }) => {
                        *interval = v;
                    }
                    _ => {}
                }
            }
            Some("days_of_week") => {
                // `parse_days_of_week` honours the composite tokens
                // ("Day"/"Weekday"/"WeekendDay") and expands them
                // to the matching list of concrete weekdays — used
                // by Relative* recurrences and (in principle) by
                // any future Weekly composite shape too.
                let expanded = parse_days_of_week(s);
                match self.pattern.as_mut() {
                    Some(PatternBuilder::Weekly { days_of_week, .. })
                    | Some(PatternBuilder::RelativeMonthly { days_of_week, .. })
                    | Some(PatternBuilder::RelativeYearly { days_of_week, .. }) => {
                        days_of_week.extend(expanded);
                    }
                    _ => {}
                }
            }
            Some("first_day_of_week") => {
                if let (
                    Some(PatternBuilder::Weekly {
                        first_day_of_week, ..
                    }),
                    Some(day),
                ) = (self.pattern.as_mut(), EwsDay::from_wire(s))
                {
                    *first_day_of_week = day;
                }
            }
            Some("day_of_week_index") => {
                let idx = EwsDayOfWeekIndex::from_wire(s);
                match self.pattern.as_mut() {
                    Some(PatternBuilder::RelativeMonthly {
                        day_of_week_index, ..
                    })
                    | Some(PatternBuilder::RelativeYearly {
                        day_of_week_index, ..
                    }) => {
                        *day_of_week_index = idx;
                    }
                    _ => {}
                }
            }
            Some("day_of_month") => {
                let v = s.parse::<u8>().unwrap_or(1).clamp(1, 31);
                match self.pattern.as_mut() {
                    Some(PatternBuilder::AbsoluteMonthly { day_of_month, .. })
                    | Some(PatternBuilder::AbsoluteYearly { day_of_month, .. }) => {
                        *day_of_month = v;
                    }
                    _ => {}
                }
            }
            Some("month") => match self.pattern.as_mut() {
                Some(PatternBuilder::AbsoluteYearly { month, .. })
                | Some(PatternBuilder::RelativeYearly { month, .. }) => {
                    *month = EwsMonth::from_wire(s);
                }
                _ => {}
            },
            Some("number_of_occurrences") => {
                if let Some(RangeBuilder::Numbered { occurrences }) = self.range.as_mut() {
                    *occurrences = s.parse::<u32>().unwrap_or(0);
                }
            }
            Some("end_date") => {
                if let Some(RangeBuilder::EndDate { end }) = self.range.as_mut() {
                    // Some servers append a TZ suffix
                    // ("2026-12-31+02:00"); keep the date part only.
                    let trimmed = s.split(['T', '+']).next().unwrap_or(s);
                    *end = trimmed.to_string();
                }
            }
            _ => {}
        }
    }

    /// Handle any End event by clearing the active text target.
    /// The caller is responsible for tracking when the OUTER
    /// `</t:Recurrence>` arrives.
    pub(crate) fn observe_end_generic(&mut self) {
        self.text_target = None;
    }

    /// Assemble the parsed recurrence. Errors on:
    ///   - unsupported shapes recorded during the walk
    ///     (Relative* recurrences),
    ///   - missing pattern element (server didn't include one,
    ///     or only included an unsupported one),
    ///   - missing / incomplete range element.
    pub(crate) fn finish(self) -> EwsResult<EwsRecurrence> {
        if let Some(name) = self.unsupported {
            return Err(EwsError::Protocol(format!(
                "{name} is not supported by Aperio yet",
            )));
        }
        let pattern = self
            .pattern
            .ok_or_else(|| EwsError::Protocol("Recurrence missing pattern element".into()))?
            .finish()?;
        let range = self
            .range
            .ok_or_else(|| EwsError::Protocol("Recurrence missing range element".into()))?
            .finish()?;
        Ok(EwsRecurrence { pattern, range })
    }
}

enum PatternBuilder {
    Daily {
        interval: u32,
    },
    Weekly {
        interval: u32,
        days_of_week: Vec<EwsDay>,
        /// EWS `<t:FirstDayOfWeek>`. Governs which day starts the week for an
        /// `INTERVAL>=2` weekly rule, so it must reach the RRULE as `WKST=` or a
        /// series that straddles the week boundary expands (and indexes) on the
        /// wrong dates. Exchange defaults it per mailbox (Sunday for en-US);
        /// absent → Monday (the RFC-5545 default).
        first_day_of_week: EwsDay,
    },
    AbsoluteMonthly {
        interval: u32,
        day_of_month: u8,
    },
    AbsoluteYearly {
        day_of_month: u8,
        month: Option<EwsMonth>,
    },
    RelativeMonthly {
        interval: u32,
        days_of_week: Vec<EwsDay>,
        day_of_week_index: Option<EwsDayOfWeekIndex>,
    },
    RelativeYearly {
        days_of_week: Vec<EwsDay>,
        day_of_week_index: Option<EwsDayOfWeekIndex>,
        month: Option<EwsMonth>,
    },
}

impl PatternBuilder {
    fn finish(self) -> EwsResult<EwsRecurrencePattern> {
        match self {
            Self::Daily { interval } => Ok(EwsRecurrencePattern::Daily { interval }),
            Self::Weekly {
                interval,
                days_of_week,
                first_day_of_week,
            } => Ok(EwsRecurrencePattern::Weekly {
                interval,
                days_of_week,
                first_day_of_week,
            }),
            Self::AbsoluteMonthly {
                interval,
                day_of_month,
            } => Ok(EwsRecurrencePattern::AbsoluteMonthly {
                interval,
                day_of_month,
            }),
            Self::AbsoluteYearly {
                day_of_month,
                month,
            } => {
                let month = month.ok_or_else(|| {
                    EwsError::Protocol("AbsoluteYearlyRecurrence missing Month".into())
                })?;
                Ok(EwsRecurrencePattern::AbsoluteYearly {
                    day_of_month,
                    month,
                })
            }
            Self::RelativeMonthly {
                interval,
                days_of_week,
                day_of_week_index,
            } => {
                let day_of_week_index = day_of_week_index.ok_or_else(|| {
                    EwsError::Protocol("RelativeMonthlyRecurrence missing DayOfWeekIndex".into())
                })?;
                if days_of_week.is_empty() {
                    return Err(EwsError::Protocol(
                        "RelativeMonthlyRecurrence missing DaysOfWeek".into(),
                    ));
                }
                Ok(EwsRecurrencePattern::RelativeMonthly {
                    interval,
                    days_of_week,
                    day_of_week_index,
                })
            }
            Self::RelativeYearly {
                days_of_week,
                day_of_week_index,
                month,
            } => {
                let day_of_week_index = day_of_week_index.ok_or_else(|| {
                    EwsError::Protocol("RelativeYearlyRecurrence missing DayOfWeekIndex".into())
                })?;
                let month = month.ok_or_else(|| {
                    EwsError::Protocol("RelativeYearlyRecurrence missing Month".into())
                })?;
                if days_of_week.is_empty() {
                    return Err(EwsError::Protocol(
                        "RelativeYearlyRecurrence missing DaysOfWeek".into(),
                    ));
                }
                Ok(EwsRecurrencePattern::RelativeYearly {
                    days_of_week,
                    day_of_week_index,
                    month,
                })
            }
        }
    }
}

enum RangeBuilder {
    NoEnd,
    Numbered { occurrences: u32 },
    EndDate { end: String },
}

/// Scratch type used while walking a single `<t:Occurrence>`
/// child of `<t:ModifiedOccurrences>`. Becomes a
/// [`ModifiedOccurrence`] on completion.
#[derive(Default)]
struct ModifiedOccurrenceBuilder {
    item_id: String,
    change_key: Option<String>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    original_start: Option<DateTime<Utc>>,
}

impl ModifiedOccurrenceBuilder {
    fn finish(self) -> Option<ModifiedOccurrence> {
        // All three time fields + item_id are mandatory per the EWS
        // schema. A missing one means a malformed response — drop
        // the override rather than emitting a half-built event that
        // would surface at the wrong time.
        Some(ModifiedOccurrence {
            item_id: if self.item_id.is_empty() {
                return None;
            } else {
                self.item_id
            },
            change_key: self.change_key,
            start: self.start?,
            end: self.end?,
            original_start: self.original_start?,
            // Both filled later by the per-occurrence GetItem enrichment; the
            // inline ModifiedOccurrences shape carries neither the cancelled
            // flag nor anything the occurrence owns.
            cancelled: false,
            own: None,
        })
    }
}

impl RangeBuilder {
    fn finish(self) -> EwsResult<EwsRecurrenceRange> {
        match self {
            Self::NoEnd => Ok(EwsRecurrenceRange::NoEnd),
            Self::Numbered { occurrences } => {
                if occurrences == 0 {
                    return Err(EwsError::Protocol(
                        "NumberedRecurrence missing or zero NumberOfOccurrences".into(),
                    ));
                }
                Ok(EwsRecurrenceRange::Numbered { occurrences })
            }
            Self::EndDate { end } => {
                if end.is_empty() {
                    return Err(EwsError::Protocol(
                        "EndDateRecurrence missing EndDate".into(),
                    ));
                }
                Ok(EwsRecurrenceRange::EndDate { end })
            }
        }
    }
}

/// Pulled-out version of the ItemId attribute pair so the api layer
/// can hand the ChangeKey back to the caller as the new ETag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRef {
    pub id: String,
    pub change_key: Option<String>,
}

/// Parse a `CreateItemResponse` / `UpdateItemResponse` and return
/// the first item's `<t:ItemId>` attributes. Both responses share
/// the same envelope shape (`m:*Response` → `m:ResponseMessages` →
/// `m:*ResponseMessage` → `m:Items` → `t:CalendarItem` →
/// `t:ItemId`), so one parser covers both.
///
/// `check_for_fault` already ran by the time we get here, so the
/// `ResponseClass="Success"` invariant holds. We only have to walk
/// to the first ItemId, read attributes, and return.
pub fn parse_first_item_id(xml: &str) -> EwsResult<ItemRef> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"itemid" {
                    let mut id = String::new();
                    let mut ck: Option<String> = None;
                    for a in e.attributes().flatten() {
                        let key = a.key.as_ref();
                        if key.eq_ignore_ascii_case(b"Id") {
                            id = String::from_utf8_lossy(&a.value).into_owned();
                        } else if key.eq_ignore_ascii_case(b"ChangeKey") {
                            ck = Some(String::from_utf8_lossy(&a.value).into_owned());
                        }
                    }
                    if id.is_empty() {
                        return Err(EwsError::Protocol(
                            "ItemId element missing Id attribute".into(),
                        ));
                    }
                    return Ok(ItemRef { id, change_key: ck });
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(EwsError::Protocol(format!("xml parse: {err}")));
            }
            _ => {}
        }
        buf.clear();
    }
    Err(EwsError::Protocol(
        "response did not contain an ItemId".into(),
    ))
}

/// Parse a `GetServerTimeZonesResponse` into the Windows zone ids the server
/// knows: the `Id` of every `<t:TimeZoneDefinition>`. The display `Name` is in
/// the server's language and is not read. `check_for_fault` already ran.
pub fn parse_server_time_zones(xml: &str) -> EwsResult<ServerTimeZones> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut ids = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                if e.local_name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"TimeZoneDefinition")
                {
                    let id = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.as_ref().eq_ignore_ascii_case(b"Id"))
                        .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                    match id {
                        Some(id) if !id.is_empty() => ids.push(id),
                        _ => {
                            return Err(EwsError::Protocol(
                                "TimeZoneDefinition element missing Id attribute".into(),
                            ));
                        }
                    }
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(EwsError::Protocol(format!("xml parse: {err}")));
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(ServerTimeZones::new(ids))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_get_user_availability_busy_blocks_by_mailbox_order() {
        // Two mailboxes, in request order. The first has a busy and a
        // tentative block (both count) plus a Free block (dropped); the
        // second errored (no CalendarEventArray) and must degrade to an
        // empty slot list rather than abort the whole parse.
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:GetUserAvailabilityResponse>
      <m:FreeBusyResponseArray>
        <m:FreeBusyResponse>
          <m:ResponseMessage ResponseClass="Success">
            <m:ResponseCode>NoError</m:ResponseCode>
          </m:ResponseMessage>
          <m:FreeBusyView>
            <t:FreeBusyViewType>Detailed</t:FreeBusyViewType>
            <t:CalendarEventArray>
              <t:CalendarEvent>
                <t:StartTime>2026-06-01T09:00:00</t:StartTime>
                <t:EndTime>2026-06-01T10:00:00</t:EndTime>
                <t:BusyType>Busy</t:BusyType>
              </t:CalendarEvent>
              <t:CalendarEvent>
                <t:StartTime>2026-06-01T12:00:00</t:StartTime>
                <t:EndTime>2026-06-01T12:30:00</t:EndTime>
                <t:BusyType>Tentative</t:BusyType>
              </t:CalendarEvent>
              <t:CalendarEvent>
                <t:StartTime>2026-06-01T15:00:00</t:StartTime>
                <t:EndTime>2026-06-01T16:00:00</t:EndTime>
                <t:BusyType>Free</t:BusyType>
              </t:CalendarEvent>
            </t:CalendarEventArray>
          </m:FreeBusyView>
        </m:FreeBusyResponse>
        <m:FreeBusyResponse>
          <m:ResponseMessage ResponseClass="Error">
            <m:MessageText>Unable to resolve e-mail address.</m:MessageText>
            <m:ResponseCode>ErrorMailRecipientNotFound</m:ResponseCode>
          </m:ResponseMessage>
        </m:FreeBusyResponse>
      </m:FreeBusyResponseArray>
    </m:GetUserAvailabilityResponse>
  </s:Body>
</s:Envelope>"#;
        let emails = ["alice@example.com", "ghost@example.com"];
        let fb = parse_get_user_availability(xml, &emails).expect("parse availability");
        assert_eq!(fb.len(), 2);
        // First mailbox: Busy + Tentative kept, Free dropped → 2 slots,
        // mapped to the address by position.
        assert_eq!(fb[0].email, "alice@example.com");
        assert_eq!(fb[0].slots.len(), 2);
        assert_eq!(
            fb[0].slots[0].start,
            "2026-06-01T09:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(
            fb[0].slots[0].end,
            "2026-06-01T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(
            fb[0].slots[1].start,
            "2026-06-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        // Second mailbox errored → empty, but still present and labelled.
        assert_eq!(fb[1].email, "ghost@example.com");
        assert!(fb[1].slots.is_empty());
    }

    #[test]
    fn parses_find_folder_response_with_multiple_folders() {
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:FindFolderResponse>
      <m:ResponseMessages>
        <m:FindFolderResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:RootFolder>
            <t:Folders>
              <t:CalendarFolder>
                <t:FolderId Id="AAMkAGI2TH" ChangeKey="CQAAABYAAA"/>
                <t:DisplayName>Kalender</t:DisplayName>
              </t:CalendarFolder>
              <t:CalendarFolder>
                <t:FolderId Id="AAMkAGI2WORK"/>
                <t:DisplayName>Arbeit</t:DisplayName>
              </t:CalendarFolder>
            </t:Folders>
          </m:RootFolder>
        </m:FindFolderResponseMessage>
      </m:ResponseMessages>
    </m:FindFolderResponse>
  </s:Body>
</s:Envelope>"#;
        let parsed = parse_find_folder_response(xml).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].folder_id, "AAMkAGI2TH");
        assert_eq!(parsed[0].change_key.as_deref(), Some("CQAAABYAAA"));
        assert_eq!(parsed[0].display_name, "Kalender");
        assert_eq!(parsed[1].folder_id, "AAMkAGI2WORK");
        assert!(parsed[1].change_key.is_none());
        assert_eq!(parsed[1].display_name, "Arbeit");
    }

    #[test]
    fn to_calendar_uses_stable_folder_id_ignoring_change_key() {
        // Even when the folder reports a ChangeKey, the calendar id is
        // the bare folder EntryID — the volatile ChangeKey is kept out
        // of the identity so the id stays stable across sessions.
        let folder = ParsedFolder {
            folder_id: "FID".into(),
            change_key: Some("CK".into()),
            display_name: "Work".into(),
        };
        let cal = to_calendar(folder, false);
        assert_eq!(cal.id, "FID");
        assert_eq!(cal.name, "Work");
        assert!(!cal.read_only);
    }

    #[test]
    fn split_calendar_id_roundtrips() {
        let (fid, ck) = split_calendar_id("FID|CK");
        assert_eq!(fid, "FID");
        assert_eq!(ck.as_deref(), Some("CK"));
        let (fid, ck) = split_calendar_id("BAREFID");
        assert_eq!(fid, "BAREFID");
        assert!(ck.is_none());
    }

    #[test]
    fn parses_find_item_response_with_full_payload() {
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:FindItemResponse>
      <m:ResponseMessages>
        <m:FindItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:RootFolder TotalItemsInView="1">
            <t:Items>
              <t:CalendarItem>
                <t:ItemId Id="IID-1" ChangeKey="ICK-1"/>
                <t:Subject>Sync</t:Subject>
                <t:Body BodyType="Text">Standup notes</t:Body>
                <t:DateTimeCreated>2026-05-19T08:00:00Z</t:DateTimeCreated>
                <t:LastModifiedTime>2026-05-19T09:30:00Z</t:LastModifiedTime>
                <t:ReminderIsSet>true</t:ReminderIsSet>
                <t:ReminderMinutesBeforeStart>10</t:ReminderMinutesBeforeStart>
                <t:Start>2026-05-20T08:00:00Z</t:Start>
                <t:End>2026-05-20T08:30:00Z</t:End>
                <t:Location>Online</t:Location>
                <t:IsAllDayEvent>false</t:IsAllDayEvent>
                <t:IsRecurring>false</t:IsRecurring>
                <t:IsCancelled>true</t:IsCancelled>
                <t:AppointmentState>7</t:AppointmentState>
              </t:CalendarItem>
            </t:Items>
          </m:RootFolder>
        </m:FindItemResponseMessage>
      </m:ResponseMessages>
    </m:FindItemResponse>
  </s:Body>
</s:Envelope>"#;
        let items = parse_find_item_response(xml).unwrap();
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.item_id, "IID-1");
        assert_eq!(it.change_key.as_deref(), Some("ICK-1"));
        assert_eq!(it.subject, "Sync");
        assert_eq!(it.body.as_deref(), Some("Standup notes"));
        assert_eq!(it.location.as_deref(), Some("Online"));
        assert!(!it.is_all_day);
        assert!(!it.is_recurring);
        assert!(it.cancelled);
        assert_eq!(it.appointment_state, Some(7));
        assert!(it.reminder_is_set);
        assert_eq!(it.reminder_minutes_before_start, Some(10));
        assert_eq!(it.start.unwrap().to_rfc3339(), "2026-05-20T08:00:00+00:00");
        assert_eq!(it.end.unwrap().to_rfc3339(), "2026-05-20T08:30:00+00:00");
    }

    #[test]
    fn parses_find_item_response_with_zero_items() {
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:FindItemResponse>
      <m:ResponseMessages>
        <m:FindItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:RootFolder TotalItemsInView="0">
            <t:Items/>
          </m:RootFolder>
        </m:FindItemResponseMessage>
      </m:ResponseMessages>
    </m:FindItemResponse>
  </s:Body>
</s:Envelope>"#;
        let items = parse_find_item_response(xml).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn to_event_maps_reminder_and_etag() {
        let item = ParsedItem {
            my_response_type: None,
            item_id: "IID".into(),
            change_key: Some("ICK".into()),
            subject: "Lunch".into(),
            body: None,
            location: Some("Cafe".into()),
            start: Some("2026-05-20T11:30:00Z".parse().unwrap()),
            end: Some("2026-05-20T12:30:00Z".parse().unwrap()),
            is_all_day: false,
            is_recurring: false,
            reminder_is_set: true,
            reminder_minutes_before_start: Some(15),
            created: Some("2026-05-19T08:00:00Z".parse().unwrap()),
            last_modified: Some("2026-05-19T09:00:00Z".parse().unwrap()),
            item_type: None,
            start_time_zone: None,
            end_time_zone: None,
            original_start: None,
            recurrence: None,
            deleted_occurrence_starts: Vec::new(),
            modified_occurrences: Vec::new(),
            organizer: None,
            attendees: Vec::new(),
            detail_fetched: false,
            cancelled: false,
            appointment_state: None,
        };
        let ev = to_event(item, "FID|CK").unwrap();
        // No `<t:CalendarItemType>` element → defaults to Single,
        // which encodes with the `S:` prefix in the Aperio id.
        assert_eq!(ev.id, "S:IID|ICK");
        assert_eq!(ev.calendar_id, "FID|CK");
        assert_eq!(ev.title, "Lunch");
        assert_eq!(ev.reminders.len(), 1);
        match &ev.reminders[0].kind {
            ReminderKind::Relative { minutes_before } => assert_eq!(*minutes_before, 15),
            other => panic!("expected Relative reminder, got {other:?}"),
        }
        assert_eq!(ev.etag.as_deref(), Some("ICK"));
    }

    #[test]
    fn to_event_maps_cancelled() {
        // A cancelled EWS meeting (IsCancelled=true) carries the flag onto the
        // Event so the host suppresses its reminders + can hide it.
        let item = ParsedItem {
            item_id: "IID".into(),
            subject: "Canceled: Sync".into(),
            start: Some("2026-05-20T12:00:00Z".parse().unwrap()),
            end: Some("2026-05-20T12:30:00Z".parse().unwrap()),
            cancelled: true,
            ..Default::default()
        };
        let ev = to_event(item, "FID|CK").unwrap();
        assert!(ev.cancelled);
    }

    #[test]
    fn to_event_maps_cancelled_via_appointment_state() {
        // Some Exchange configs leave IsCancelled=false on the attendee's copy
        // but flip the asfCanceled (0x4) bit in AppointmentState. The 0x5 here
        // = asfMeeting(1) | asfCanceled(4).
        let item = ParsedItem {
            item_id: "IID".into(),
            subject: "Team Standup".into(),
            start: Some("2026-05-20T12:00:00Z".parse().unwrap()),
            end: Some("2026-05-20T12:30:00Z".parse().unwrap()),
            cancelled: false,
            appointment_state: Some(5),
            ..Default::default()
        };
        let ev = to_event(item, "FID|CK").unwrap();
        assert!(ev.cancelled);
    }

    #[test]
    fn to_event_subject_prefix_alone_is_not_cancelled() {
        // A localized "Abgesagt:" prefix WITHOUT IsCancelled or the asfCanceled
        // bit must NOT be treated as cancelled: Exchange's auto-processing that
        // prepends the prefix also sets IsCancelled, so a prefix-only subject is
        // a user-authored title ("Abgesagt: Vertretung klären"), not a real
        // cancellation. Flagging it would wrongly dim a live meeting and suppress
        // its reminders.
        let item = ParsedItem {
            item_id: "IID".into(),
            subject: "Abgesagt: Vertretung klären".into(),
            start: Some("2026-05-20T12:00:00Z".parse().unwrap()),
            end: Some("2026-05-20T12:30:00Z".parse().unwrap()),
            cancelled: false,
            appointment_state: Some(3), // asfMeeting|asfReceived, no cancel bit
            ..Default::default()
        };
        let ev = to_event(item, "FID|CK").unwrap();
        assert!(!ev.cancelled);
    }

    #[test]
    fn to_event_not_cancelled_for_ordinary_meeting() {
        // A normal received meeting: no cancel bit, ordinary subject.
        let item = ParsedItem {
            item_id: "IID".into(),
            subject: "Termin abgesagt? bitte klären".into(),
            start: Some("2026-05-20T12:00:00Z".parse().unwrap()),
            end: Some("2026-05-20T12:30:00Z".parse().unwrap()),
            cancelled: false,
            appointment_state: Some(3),
            ..Default::default()
        };
        let ev = to_event(item, "FID|CK").unwrap();
        assert!(!ev.cancelled);
    }

    #[test]
    fn to_event_errors_when_start_missing() {
        let item = ParsedItem {
            item_id: "IID".into(),
            subject: "Bad".into(),
            ..Default::default()
        };
        let err = to_event(item, "FID").unwrap_err();
        match err {
            EwsError::Protocol(m) => assert!(m.contains("Start")),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    // ── Write-side tests ─────────────────────────────────────────────────

    fn new_event_min(title: &str) -> NewEvent {
        NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: title.into(),
            description: None,
            location: None,
            start: "2026-05-20T08:00:00Z".parse().unwrap(),
            end: "2026-05-20T09:00:00Z".parse().unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        }
    }

    #[test]
    fn new_event_to_calendar_item_xml_renders_required_fields() {
        let ev = new_event_min("Standup");
        let xml = new_event_to_calendar_item_xml(&ev).unwrap();
        assert!(xml.contains("<t:Subject>Standup</t:Subject>"));
        assert!(xml.contains("<t:Start>2026-05-20T08:00:00Z</t:Start>"));
        assert!(xml.contains("<t:End>2026-05-20T09:00:00Z</t:End>"));
        // No description / location / reminder → those tags must
        // be absent so EWS doesn't choke on empty values.
        assert!(!xml.contains("<t:Body"));
        assert!(!xml.contains("<t:Location>"));
        assert!(xml.contains("<t:ReminderIsSet>false</t:ReminderIsSet>"));
    }

    #[test]
    fn new_event_to_calendar_item_xml_emits_reminder_minutes() {
        let mut ev = new_event_min("Sync");
        ev.reminders = vec![Reminder {
            kind: ReminderKind::Relative { minutes_before: 10 },
            sound: None,
        }];
        let xml = new_event_to_calendar_item_xml(&ev).unwrap();
        assert!(xml.contains("<t:ReminderIsSet>true</t:ReminderIsSet>"));
        assert!(xml.contains("<t:ReminderMinutesBeforeStart>10</t:ReminderMinutesBeforeStart>"));
    }

    #[test]
    fn new_event_to_calendar_item_xml_emits_start_time_zone_for_zoned_master() {
        let mut ev = new_event_min("OAGDU");
        ev.recurrence = Some(EventRecurrence {
            rrule: "FREQ=MONTHLY;BYDAY=2SU".into(),
            exceptions: Vec::new(),
            tzid: Some("America/New_York".into()),
        });
        let xml = new_event_to_calendar_item_xml(&ev).unwrap();
        // IANA → the Windows id Exchange expects, emitted AFTER <t:Recurrence>
        // (the CalendarItemType element order).
        assert!(
            xml.contains(r#"<t:StartTimeZone Id="Eastern Standard Time"/>"#),
            "{xml}"
        );
        assert!(xml.contains(r#"<t:EndTimeZone Id="Eastern Standard Time"/>"#));
        let rec_pos = xml.find("<t:Recurrence>").expect("recurrence present");
        let tz_pos = xml.find("<t:StartTimeZone").expect("StartTimeZone present");
        assert!(tz_pos > rec_pos, "StartTimeZone must follow Recurrence");
    }

    /// A weekly master with `tzid`, for the update tests.
    fn zoned_master(tzid: Option<&str>) -> Event {
        Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: "IID|CK".into(),
            calendar_id: "FID|FK".into(),
            title: "Weekly".into(),
            description: None,
            location: None,
            start: "2026-05-20T08:00:00Z".parse().unwrap(),
            end: "2026-05-20T09:00:00Z".parse().unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY".into(),
                exceptions: Vec::new(),
                tzid: tzid.map(str::to_string),
            }),
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("CK".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        }
    }

    /// A create and an update write the zone by `windows_tz`'s one rule: a
    /// device spelling resolves first, a merged place writes its target's id,
    /// the ids the hand table got wrong are CLDR's, and a zone Exchange cannot
    /// store or a UTC name writes none.
    #[test]
    fn create_and_update_write_the_zone_by_the_one_rule() {
        let cases = [
            ("Asia/Calcutta", Some("India Standard Time")),
            ("Asia/Beirut", Some("Middle East Standard Time")),
            ("Europe/Amsterdam", Some("Romance Standard Time")),
            ("Antarctica/Troll", None),
            ("America/Scoresbysund", None),
            ("UTC", None),
            ("Etc/UTC", None),
        ];
        for (tzid, windows) in cases {
            let mut create = new_event_min("Zoned");
            create.recurrence = Some(EventRecurrence {
                rrule: "FREQ=MONTHLY;BYDAY=2SU".into(),
                exceptions: Vec::new(),
                tzid: Some(tzid.into()),
            });
            let xml = new_event_to_calendar_item_xml(&create).unwrap();
            let (set, del) = event_to_update_field_xml(&zoned_master(Some(tzid))).unwrap();
            // Whatever the zone, an update never deletes the zone fields: what
            // Exchange keeps when none is sent is the live test's question.
            assert!(!del.contains("TimeZone"), "update {tzid} deletes: {del}");
            match windows {
                Some(windows) => {
                    let start = format!(r#"<t:StartTimeZone Id="{windows}"/>"#);
                    let end = format!(r#"<t:EndTimeZone Id="{windows}"/>"#);
                    assert!(
                        xml.contains(&start) && xml.contains(&end),
                        "create {tzid}: {xml}"
                    );
                    assert!(
                        set.contains(&start) && set.contains(&end),
                        "update {tzid}: {set}"
                    );
                }
                None => {
                    assert!(!xml.contains("TimeZone"), "create {tzid}: {xml}");
                    assert!(!set.contains("TimeZone"), "update {tzid}: {set}");
                }
            }
        }
    }

    /// Decision 46a: an all-day series writes no zone on create or update,
    /// whatever zone it stores (Outlook's, or one kept from a timed series).
    /// Live test round 2: a zone makes Exchange stretch it over more days.
    #[test]
    fn an_all_day_series_writes_no_zone() {
        for tzid in ["Europe/Berlin", "America/Los_Angeles", "Asia/Calcutta"] {
            let mut create = new_event_min("All-day");
            create.all_day = true;
            create.start = "2026-10-18T22:00:00Z".parse().unwrap();
            create.end = "2026-10-19T22:00:00Z".parse().unwrap();
            create.recurrence = Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
                exceptions: Vec::new(),
                tzid: Some(tzid.into()),
            });
            let xml = new_event_to_calendar_item_xml(&create).unwrap();
            assert!(
                xml.contains("<t:IsAllDayEvent>true</t:IsAllDayEvent>"),
                "{xml}"
            );
            assert!(!xml.contains("TimeZone"), "create all-day {tzid}: {xml}");

            let mut master = zoned_master(Some(tzid));
            master.all_day = true;
            let (set, del) = event_to_update_field_xml(&master).unwrap();
            assert!(!set.contains("TimeZone"), "update all-day {tzid}: {set}");
            assert!(
                !del.contains("TimeZone"),
                "update all-day {tzid} deletes: {del}"
            );

            // The same series with a time still writes its zone.
            master.all_day = false;
            let (set, _) = event_to_update_field_xml(&master).unwrap();
            assert!(
                set.contains("<t:StartTimeZone Id="),
                "update timed {tzid}: {set}"
            );
        }
    }

    /// Decision 41a: an id the server does not know goes out without a zone,
    /// because Exchange 2019 refuses the whole save over it (live test A9).
    /// The ids it knows are written, in the server's case or not; a server
    /// that could not be asked, or knows none, gets the CLDR ids.
    #[test]
    fn a_zone_the_server_does_not_know_is_written_without_a_zone() {
        let server = ServerTimeZones::new(["w. europe standard time", "UTC"]);
        let written = |tzid: &str, zones: Option<&ServerTimeZones>| {
            let mut create = new_event_min("Zoned");
            create.recurrence = Some(EventRecurrence {
                rrule: "FREQ=WEEKLY".into(),
                exceptions: Vec::new(),
                tzid: Some(tzid.into()),
            });
            let xml = new_event_to_calendar_item_xml_on(&create, zones).unwrap();
            let (set, _) = event_to_update_field_xml_on(
                &zoned_master(Some(tzid)),
                None,
                zones,
                EventIdKind::RecurringMaster,
            )
            .unwrap();
            (xml, set)
        };

        let (xml, set) = written("Europe/Berlin", Some(&server));
        let start = r#"<t:StartTimeZone Id="W. Europe Standard Time"/>"#;
        assert!(xml.contains(start), "create Berlin: {xml}");
        assert!(set.contains(start), "update Berlin: {set}");

        let (xml, set) = written("Africa/Sao_Tome", Some(&server));
        assert!(!xml.contains("TimeZone"), "create Sao Tome: {xml}");
        assert!(!set.contains("TimeZone"), "update Sao Tome: {set}");

        for zones in [None, Some(&ServerTimeZones::default())] {
            let (xml, _) = written("Africa/Sao_Tome", zones);
            assert!(
                xml.contains(r#"<t:StartTimeZone Id="Sao Tome Standard Time"/>"#),
                "create Sao Tome, server list {zones:?}: {xml}"
            );
        }
    }

    /// The shape Exchange 2019 answered in the live test: each definition
    /// carries (empty) child elements and a display name in the server's
    /// language. Only the ids are kept.
    #[test]
    fn parse_server_time_zones_reads_the_definition_ids() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Body>
    <m:GetServerTimeZonesResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
      <m:ResponseMessages>
        <m:GetServerTimeZonesResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:TimeZoneDefinitions>
            <t:TimeZoneDefinition Name="(UTC-12:00) Internationale Datumsgrenze West" Id="Dateline Standard Time"><t:Periods/><t:TransitionsGroups/></t:TimeZoneDefinition>
            <t:TimeZoneDefinition Name="(UTC+01:00) Amsterdam, Berlin" Id="W. Europe Standard Time"><t:Periods/><t:TransitionsGroups/></t:TimeZoneDefinition>
            <t:TimeZoneDefinition Name="(UTC) Koordinierte Weltzeit" Id="UTC"/>
          </m:TimeZoneDefinitions>
        </m:GetServerTimeZonesResponseMessage>
      </m:ResponseMessages>
    </m:GetServerTimeZonesResponse>
  </s:Body>
</s:Envelope>"#;
        let zones = parse_server_time_zones(xml).unwrap();
        assert_eq!(zones.len(), 3);
        for id in ["Dateline Standard Time", "W. Europe Standard Time", "UTC"] {
            assert!(zones.knows(id), "{id}");
        }
        assert!(!zones.knows("Sao Tome Standard Time"));
        assert!(
            !zones.knows("(UTC) Koordinierte Weltzeit"),
            "names are not ids"
        );

        let missing = xml.replace(r#" Id="UTC""#, "");
        assert!(parse_server_time_zones(&missing).is_err());
    }

    /// Writes the create and update requests of the live Exchange test
    /// (DESIGN-series-time-zone.md, stage 4, decision 38a) as Aperio builds
    /// them, into the directory `APERIO_LIVE_TEST_DIR` names:
    ///
    /// `APERIO_LIVE_TEST_DIR=<dir> cargo test -p adapter-ews --lib live_test_requests -- --ignored`
    ///
    /// Three changes from Aperio's bytes: the series go into the mailbox's own
    /// calendar (`DistinguishedFolderId calendar`) instead of a folder id, the
    /// updates carry `ITEM_ID` and `CHANGEKEY` placeholders, and each file
    /// starts with an XML comment naming its step. A9 is not Aperio's rule at
    /// all: the A1 request with an invented id, to see what an unknown id gets.
    #[test]
    #[ignore = "writes the live Exchange test requests; see the doc comment"]
    fn live_test_requests() {
        let dir = std::path::PathBuf::from(
            std::env::var("APERIO_LIVE_TEST_DIR")
                .expect("APERIO_LIVE_TEST_DIR names the output directory"),
        );
        std::fs::create_dir_all(&dir).unwrap();
        // Mondays from 19 October 2026, four of them: on a Berlin clock the
        // last three fall after the change on 25 October.
        let rule = |tzid: Option<&str>| EventRecurrence {
            rrule: "FREQ=WEEKLY;BYDAY=MO;COUNT=4".into(),
            exceptions: Vec::new(),
            tzid: tzid.map(str::to_string),
        };
        let series = |subject: &str, tzid: Option<&str>| NewEvent {
            start: "2026-10-19T08:00:00Z".parse().unwrap(),
            end: "2026-10-19T09:00:00Z".parse().unwrap(),
            recurrence: Some(rule(tzid)),
            ..new_event_min(subject)
        };
        let write = |name: &str, comment: String, envelope: String| {
            let prelude = "<?xml version=\"1.0\" encoding=\"utf-8\"?>";
            let envelope = envelope.replacen(prelude, &format!("{prelude}\n<!-- {comment} -->"), 1);
            std::fs::write(dir.join(name), envelope).unwrap();
        };
        let create = |name: &str, what: &str, event: NewEvent| {
            let tzid = event.recurrence.as_ref().and_then(|r| r.tzid.clone());
            let zone = crate::windows_tz::windows_zone_for(
                cal_core::written_series_zone(tzid.as_deref(), event.all_day),
                None,
            );
            let envelope = crate::soap::create_calendar_item(
                "CALENDAR",
                None,
                &new_event_to_calendar_item_xml(&event).unwrap(),
                false,
            )
            .replace(
                r#"<t:FolderId Id="CALENDAR"/>"#,
                r#"<t:DistinguishedFolderId Id="calendar"/>"#,
            );
            write(
                name,
                format!(
                    "{what} Subject: {}. Aperio's zone: {tzid:?} -> {zone:?}.",
                    event.title
                ),
                envelope,
            );
        };
        let update = |name: &str, what: &str, event: Event| {
            let tzid = event.recurrence.as_ref().and_then(|r| r.tzid.clone());
            let zone = crate::windows_tz::windows_zone_for(
                cal_core::written_series_zone(tzid.as_deref(), event.all_day),
                None,
            );
            let (set, del) = event_to_update_field_xml(&event).unwrap();
            let envelope =
                crate::soap::update_calendar_item("ITEM_ID", Some("CHANGEKEY"), &set, &del, false);
            write(
                name,
                format!(
                    "{what} Aperio's zone: {tzid:?} -> {zone:?}. Replace ITEM_ID and CHANGEKEY."
                ),
                envelope,
            );
        };

        create(
            "A1-create-berlin.xml",
            "Step A1: a weekly Berlin series as Aperio creates it.",
            series("Aperio zone test A1 Berlin", Some("Europe/Berlin")),
        );
        create(
            "A2-create-no-zone.xml",
            "Step A2: a series without a zone, as Aperio creates one for every UTC name.",
            series("Aperio zone test A2 no zone", None),
        );
        create(
            "A3-create-beirut.xml",
            "Step A3: Beirut, which the old table wrote as the invented id Lebanon Standard Time.",
            series("Aperio zone test A3 Beirut", Some("Asia/Beirut")),
        );
        for (name, subject, tzid) in [
            (
                "A4-create-volgograd.xml",
                "Aperio zone test A4 Volgograd",
                "Europe/Volgograd",
            ),
            (
                "A5-create-juba.xml",
                "Aperio zone test A5 Juba",
                "Africa/Juba",
            ),
            (
                "A6-create-qyzylorda.xml",
                "Aperio zone test A6 Qyzylorda",
                "Asia/Qyzylorda",
            ),
            (
                "A7-create-punta-arenas.xml",
                "Aperio zone test A7 Punta Arenas",
                "America/Punta_Arenas",
            ),
        ] {
            create(
                name,
                "A Windows id Aperio writes for the first time: does this server know it?",
                series(subject, Some(tzid)),
            );
        }
        // All-day as the app sends it: the local midnights of 19 and 20 October
        // on the machine that writes the file, whose offset the comment names.
        let offset = Local.offset_from_utc_datetime(&local_midnight(2026, 10, 19).naive_utc());
        create(
            "A8-create-allday-los-angeles.xml",
            &format!(
                "Step A8: an all-day weekly series with a zone, written on a machine at UTC{offset}. \
                 Which weekday does OWA show, and which Start values come back?"
            ),
            NewEvent {
                start: local_midnight(2026, 10, 19),
                end: local_midnight(2026, 10, 20),
                all_day: true,
                recurrence: Some(rule(Some("America/Los_Angeles"))),
                ..new_event_min("Aperio zone test A8 all-day")
            },
        );
        // Not Aperio's rule: the A1 request with an id no server knows.
        let invented = new_event_to_calendar_item_xml(&series(
            "Aperio zone test A9 invented id",
            Some("Europe/Berlin"),
        ))
        .unwrap()
        .replace(
            r#"Id="W. Europe Standard Time""#,
            r#"Id="Lebanon Standard Time""#,
        );
        assert!(invented.contains(r#"<t:StartTimeZone Id="Lebanon Standard Time"/>"#));
        write(
            "A9-create-invented-id.xml",
            "Step A9: NOT Aperio's rule. The A1 request with the invented id Lebanon Standard Time, \
             which the old table wrote for Beirut. Which ResponseCode does an unknown id get?"
                .to_string(),
            crate::soap::create_calendar_item("CALENDAR", None, &invented, false).replace(
                r#"<t:FolderId Id="CALENDAR"/>"#,
                r#"<t:DistinguishedFolderId Id="calendar"/>"#,
            ),
        );
        update(
            "B1-update-berlin-without-zone.xml",
            "Step B1: update the A1 Berlin series as Aperio does for a series that now has no zone. Is the zone kept?",
            Event {
                id: "ITEM_ID|CHANGEKEY".into(),
                title: "Aperio zone test A1 Berlin (updated without zone)".into(),
                start: "2026-10-19T08:00:00Z".parse().unwrap(),
                end: "2026-10-19T09:00:00Z".parse().unwrap(),
                recurrence: Some(rule(None)),
                ..zoned_master(None)
            },
        );
        update(
            "B2-update-no-zone-to-bangkok.xml",
            "Step B2: update the A2 series with a zone, in Aperio's field order (Start and End before the zone). Do the occurrences stay at 08:00Z?",
            Event {
                id: "ITEM_ID|CHANGEKEY".into(),
                title: "Aperio zone test A2 no zone (updated to Bangkok)".into(),
                start: "2026-10-19T08:00:00Z".parse().unwrap(),
                end: "2026-10-19T09:00:00Z".parse().unwrap(),
                recurrence: Some(rule(Some("Asia/Bangkok"))),
                ..zoned_master(None)
            },
        );

        // Second round (decision 42a): which all-day series keep their day.
        let envelope = |item_xml: &str| {
            crate::soap::create_calendar_item("CALENDAR", None, item_xml, false).replace(
                r#"<t:FolderId Id="CALENDAR"/>"#,
                r#"<t:DistinguishedFolderId Id="calendar"/>"#,
            )
        };
        let all_day = |subject: &str, tzid: Option<&str>| NewEvent {
            start: local_midnight(2026, 10, 19),
            end: local_midnight(2026, 10, 20),
            all_day: true,
            recurrence: Some(rule(tzid)),
            ..new_event_min(subject)
        };
        let zone_fields = |id: &str| {
            format!(
                "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:StartTimeZone\"/>\n              <t:CalendarItem>\n                <t:StartTimeZone Id=\"{id}\"/>\n              </t:CalendarItem>\n            </t:SetItemField>\n            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:EndTimeZone\"/>\n              <t:CalendarItem>\n                <t:EndTimeZone Id=\"{id}\"/>\n              </t:CalendarItem>\n            </t:SetItemField>\n"
            )
        };
        create(
            "A10-create-allday-no-zone.xml",
            &format!(
                "Step A10: an all-day weekly series without a zone, written on a machine at \
                 UTC{offset}. Which weekday does OWA show?"
            ),
            all_day("Aperio zone test A10 all-day no zone", None),
        );
        let utc = new_event_to_calendar_item_xml(&all_day("Aperio zone test A11 all-day UTC", None))
            .unwrap()
            .replace(
                "        </t:CalendarItem>",
                "          <t:StartTimeZone Id=\"UTC\"/>\n          <t:EndTimeZone Id=\"UTC\"/>\n        </t:CalendarItem>",
            );
        assert!(utc.contains(r#"<t:StartTimeZone Id="UTC"/>"#));
        write(
            "A11-create-allday-utc.xml",
            format!(
                "Step A11: NOT Aperio's rule yet. The A10 request with the zone UTC written \
                 explicitly, on a machine at UTC{offset}. Which weekday does OWA show?"
            ),
            envelope(&utc),
        );
        create(
            "A12-create-abidjan.xml",
            "Step A12: a series in Africa/Abidjan (Greenwich Standard Time), to see whether a real \
             Greenwich series keeps Greenwich as its EndTimeZone, unlike A2 without a zone.",
            series("Aperio zone test A12 Abidjan", Some("Africa/Abidjan")),
        );
        create(
            "A13-create-allday-los-angeles-again.xml",
            "Step A13: a second all-day series with a zone, like A8, for step B4.",
            all_day("Aperio zone test A13 all-day", Some("America/Los_Angeles")),
        );
        let repaired = |title: &str| Event {
            id: "ITEM_ID|CHANGEKEY".into(),
            title: title.into(),
            start: local_midnight(2026, 10, 19),
            end: local_midnight(2026, 10, 20),
            all_day: true,
            recurrence: Some(rule(None)),
            ..zoned_master(None)
        };
        let (set, del) =
            event_to_update_field_xml(&repaired("Aperio zone test A8 all-day (UTC after start)"))
                .unwrap();
        write(
            "B3-update-allday-utc-after-start.xml",
            "Step B3: NOT Aperio's rule yet. Update the A8 series with all-day Start and End and \
             the zone UTC set after them, in Aperio's field order. Replace ITEM_ID and CHANGEKEY. \
             Which weekday does OWA show afterwards?"
                .to_string(),
            crate::soap::update_calendar_item(
                "ITEM_ID",
                Some("CHANGEKEY"),
                &format!("{set}{}", zone_fields("UTC")),
                &del,
                false,
            ),
        );
        let (set, del) =
            event_to_update_field_xml(&repaired("Aperio zone test A13 all-day (UTC before start)"))
                .unwrap();
        write(
            "B4-update-allday-utc-before-start.xml",
            "Step B4: NOT Aperio's rule yet. Update the A13 series with the zone UTC set BEFORE \
             the all-day Start and End. Replace ITEM_ID and CHANGEKEY. Which weekday does OWA \
             show afterwards?"
                .to_string(),
            crate::soap::update_calendar_item(
                "ITEM_ID",
                Some("CHANGEKEY"),
                &format!("{}{set}", zone_fields("UTC")),
                &del,
                false,
            ),
        );
    }

    /// Writes the requests of live test round 3 (decisions 47a and 49a) as
    /// Aperio builds them, into the directory `APERIO_LIVE_TEST_DIR` names, for
    /// the owner's `R3-run.ps1`:
    ///
    /// `APERIO_LIVE_TEST_DIR=<dir> cargo test -p adapter-ews --lib live_test_requests_round_3 -- --ignored`
    ///
    /// The owner creates all-day series and singles in Outlook on a Berlin clock.
    /// The updates start from Aperio's own read of that planned stored shape (a
    /// SyncFolderItems row through `parse_sync_folder_items_response` and
    /// `to_event`), so they are what Aperio sends after reading those items as
    /// the owner is asked to create them: no reminder, location or body. The
    /// requests marked "NOT Aperio's rule" differ on purpose:
    /// - the 47a prototype puts Start and End on midnights of the stored zone
    ///   instead of the UTC midnights of the day;
    /// - the Tokyo items carry a zone on an all-day single;
    /// - R3-7b leaves out the Recurrence delete.
    #[test]
    #[ignore = "writes the live Exchange test requests of round 3; see the doc comment"]
    fn live_test_requests_round_3() {
        const BERLIN: &str = "W. Europe Standard Time";
        const TOKYO: &str = "Tokyo Standard Time";
        const WEEKLY: &str = "<t:Recurrence><t:WeeklyRecurrence><t:Interval>1</t:Interval><t:DaysOfWeek>Monday</t:DaysOfWeek><t:FirstDayOfWeek>Monday</t:FirstDayOfWeek></t:WeeklyRecurrence><t:NumberedRecurrence><t:StartDate>2026-10-19+02:00</t:StartDate><t:NumberOfOccurrences>4</t:NumberOfOccurrences></t:NumberedRecurrence></t:Recurrence>";
        const REPLACE: &str = "Replace ITEM_ID and CHANGEKEY.";

        let dir = std::path::PathBuf::from(
            std::env::var("APERIO_LIVE_TEST_DIR")
                .expect("APERIO_LIVE_TEST_DIR names the output directory"),
        );
        std::fs::create_dir_all(&dir).unwrap();
        let utc = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
        // The owner's items are stored in W. Europe, so "midnight in the stored
        // zone" is the Berlin clock these files must be written on.
        assert_eq!(
            local_midnight(2026, 10, 19),
            utc("2026-10-18T22:00:00Z"),
            "write round 3 on a Berlin clock"
        );
        assert_eq!(
            local_midnight(2026, 10, 26),
            utc("2026-10-25T23:00:00Z"),
            "write round 3 on a Berlin clock"
        );

        let write = |name: &str, comment: &str, envelope: String| {
            let prelude = "<?xml version=\"1.0\" encoding=\"utf-8\"?>";
            assert!(envelope.contains(prelude), "{name}");
            assert!(
                !comment.contains("--"),
                "an XML comment cannot hold --: {comment}"
            );
            let envelope = envelope.replacen(prelude, &format!("{prelude}\n<!-- {comment} -->"), 1);
            std::fs::write(dir.join(name), envelope).unwrap();
        };

        // Aperio's read of an item stored in the planned shape. The cache takes
        // an item's all-day flag, zones and type from its SyncFolderItems row
        // (the GetItem parser reads no IsAllDayEvent), so the item goes through
        // that parser, with the recurrence GetItem would add already in the row.
        let read = |kind: &str, start: &str, end: &str, zone: &str, recurrence: &str| {
            let xml = format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Body>
    <m:SyncFolderItemsResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:SyncState>ROUND-3</m:SyncState>
          <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
          <m:Changes>
            <t:Create>
            <t:CalendarItem>
              <t:ItemId Id="ITEM" ChangeKey="CK"/>
              <t:Subject>Outlook</t:Subject>
              <t:Start>{start}</t:Start>
              <t:End>{end}</t:End>
              <t:IsAllDayEvent>true</t:IsAllDayEvent>
              <t:CalendarItemType>{kind}</t:CalendarItemType>
              {recurrence}
              <t:StartTimeZone Id="{zone}"/>
              <t:EndTimeZone Id="{zone}"/>
            </t:CalendarItem>
            </t:Create>
          </m:Changes>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </s:Body>
</s:Envelope>"#
            );
            let mut result = parse_sync_folder_items_response(&xml).unwrap();
            assert_eq!(result.changes.len(), 1, "{kind}");
            match result.changes.remove(0) {
                SyncChange::Create(item) => to_event(item, "CALENDAR").unwrap(),
                other => panic!("expected a Create for {kind}, got {other:?}"),
            }
        };
        let master = read(
            "RecurringMaster",
            "2026-10-18T22:00:00Z",
            "2026-10-19T22:00:00Z",
            BERLIN,
            WEEKLY,
        );
        assert!(master.all_day);
        assert_eq!(
            (master.start, master.end),
            (utc("2026-10-18T22:00:00Z"), utc("2026-10-19T22:00:00Z"))
        );
        let rule = master
            .recurrence
            .clone()
            .expect("a master reads with its rule");
        assert_eq!(rule.tzid.as_deref(), Some("Europe/Berlin"));
        for part in ["FREQ=WEEKLY", "BYDAY=MO", "COUNT=4"] {
            assert!(rule.rrule.contains(part), "{}", rule.rrule);
        }
        let single = read(
            "Single",
            "2026-10-18T22:00:00Z",
            "2026-10-19T22:00:00Z",
            BERLIN,
            "",
        );
        assert!(single.all_day && single.recurrence.is_none());
        let exception = read(
            "Exception",
            "2026-10-25T23:00:00Z",
            "2026-10-26T23:00:00Z",
            BERLIN,
            "",
        );
        assert!(exception.all_day && exception.recurrence.is_none());
        assert_eq!(
            (exception.start, exception.end),
            (utc("2026-10-25T23:00:00Z"), utc("2026-10-26T23:00:00Z"))
        );
        let tokyo = read(
            "Single",
            "2026-10-18T15:00:00Z",
            "2026-10-19T15:00:00Z",
            TOKYO,
            "",
        );
        assert_eq!(
            (tokyo.start, tokyo.end),
            (utc("2026-10-18T22:00:00Z"), utc("2026-10-19T22:00:00Z")),
            "the anchor reads Tokyo's day on the Berlin clock"
        );

        // Aperio's update fields for an edited item; no all-day write names a zone (46a).
        let fields = |event: &Event| {
            let (set, del) = event_to_update_field_xml(event).unwrap();
            assert!(
                !set.contains("TimeZone") && !del.contains("TimeZone"),
                "{set}{del}"
            );
            (set, del)
        };
        // The 47a prototype: Start and End on midnights of the stored zone (for
        // these items, this Berlin clock) instead of the UTC midnights of the day.
        let on_stored_midnights = |event: &Event, mut set: String| {
            for (tag, when) in [("Start", event.start), ("End", event.end)] {
                let today = format!(
                    "<t:{tag}>{}</t:{tag}>",
                    format_ews_datetime(ews_all_day_boundary(when))
                );
                let prototype = format!("<t:{tag}>{}</t:{tag}>", format_ews_datetime(when));
                assert_eq!(set.matches(&today).count(), 1, "{tag} in {set}");
                set = set.replace(&today, &prototype);
            }
            set
        };
        let update = |set: &str, del: &str| {
            crate::soap::update_calendar_item("ITEM_ID", Some("CHANGEKEY"), set, del, false)
        };
        let edited = |event: &Event, title: &str| Event {
            title: title.into(),
            ..event.clone()
        };

        let s1 = edited(&master, "Aperio zone test S1 series (47a)");
        let (set, del) = fields(&s1);
        write(
            "R3-1-update-s1-47a.xml",
            &format!(
                "Step R3-1: NOT Aperio's rule yet (47a prototype). Aperio's update of Outlook's S1 \
                 series with a new title, Start and End moved to midnights of the stored zone. {REPLACE}"
            ),
            update(&on_stored_midnights(&s1, set), &del),
        );
        let s2 = edited(&master, "Aperio zone test S2 series (today's rule)");
        let (set, del) = fields(&s2);
        write(
            "R3-2-update-s2-today.xml",
            &format!(
                "Step R3-2: Aperio's update of Outlook's S2 series with a new title, as this build \
                 sends it (all-day series write no zone, 46a). {REPLACE}"
            ),
            update(&set, &del),
        );
        let t1 = Event {
            start: local_midnight(2026, 10, 20),
            end: local_midnight(2026, 10, 21),
            ..edited(&single, "Aperio zone test T1 single (47a, Tuesday)")
        };
        let (set, del) = fields(&t1);
        write(
            "R3-3-update-t1-47a.xml",
            &format!(
                "Step R3-3: NOT Aperio's rule yet (47a prototype). Aperio's update of Outlook's T1 \
                 single moved to Tuesday 20 October, Start and End on midnights of the stored zone. {REPLACE}"
            ),
            update(&on_stored_midnights(&t1, set), &del),
        );
        let t2 = edited(&single, "Aperio zone test T2 single (today's rule)");
        let (set, del) = fields(&t2);
        write(
            "R3-4-update-t2-today.xml",
            &format!(
                "Step R3-4: Aperio's update of Outlook's T2 single with a new title, for the planned item (no reminder, location or body) byte for byte as \
                 main sends it. {REPLACE}"
            ),
            update(&set, &del),
        );
        let tokyo_update = edited(&tokyo, "Aperio zone test R3-5b Tokyo update (today's rule)");
        let (set, del) = fields(&tokyo_update);
        assert!(
            set.contains("<t:Start>2026-10-19T00:00:00Z</t:Start>"),
            "{set}"
        );
        write(
            "R3-5b-u-update-tokyo-today.xml",
            &format!(
                "Step R3-5b-u: Aperio's update of the Tokyo single R3-5b with a new title, as it sends \
                 it today. In which zone does Exchange round its UTC midnights? {REPLACE}"
            ),
            update(&set, &del),
        );
        let moved = Event {
            start: local_midnight(2026, 10, 27),
            end: local_midnight(2026, 10, 28),
            ..edited(&exception, "Aperio zone test S3 exception (47a, Tuesday)")
        };
        let (set, del) = fields(&moved);
        assert!(del.contains(r#"FieldURI="calendar:Recurrence""#), "{del}");
        write(
            "R3-7-update-s3-exception-47a.xml",
            &format!(
                "Step R3-7: NOT Aperio's rule yet (47a prototype). Aperio's override update of the S3 \
                 exception moved to Tuesday 27 October, Start and End on midnights of the stored zone, \
                 with the Recurrence delete the override path sent until PR #77. {REPLACE}"
            ),
            update(&on_stored_midnights(&moved, set.clone()), &del),
        );
        let at = del.find(r#"FieldURI="calendar:Recurrence""#).unwrap();
        let open = del[..at].rfind("<t:DeleteItemField>").unwrap();
        let close =
            at + del[at..].find("</t:DeleteItemField>").unwrap() + "</t:DeleteItemField>".len();
        let del_without_rule = format!("{}{}", &del[..open], &del[close..]);
        assert!(
            !del_without_rule.contains("calendar:Recurrence"),
            "{del_without_rule}"
        );
        assert!(
            del_without_rule.contains("calendar:Location"),
            "{del_without_rule}"
        );
        write(
            "R3-7b-update-s3-exception-no-recurrence-delete.xml",
            &format!(
                "Step R3-7b: NOT Aperio's request. R3-7 without the Recurrence delete, as the override path writes since PR #77; sent only if \
                 Exchange refuses R3-7. {REPLACE}"
            ),
            update(&on_stored_midnights(&moved, set), &del_without_rule),
        );

        let envelope = |item_xml: &str| {
            crate::soap::create_calendar_item("CALENDAR", None, item_xml, false).replace(
                r#"<t:FolderId Id="CALENDAR"/>"#,
                r#"<t:DistinguishedFolderId Id="calendar"/>"#,
            )
        };
        let all_day_single = |subject: &str| NewEvent {
            start: local_midnight(2026, 10, 19),
            end: local_midnight(2026, 10, 20),
            all_day: true,
            ..new_event_min(subject)
        };
        let tokyo_create = |subject: &str| {
            let xml = new_event_to_calendar_item_xml(&all_day_single(subject))
                .unwrap()
                .replace(
                    "<t:Start>2026-10-19T00:00:00Z</t:Start>",
                    "<t:Start>2026-10-18T15:00:00Z</t:Start>",
                )
                .replace(
                    "<t:End>2026-10-20T00:00:00Z</t:End>",
                    "<t:End>2026-10-19T15:00:00Z</t:End>",
                )
                .replace(
                    "        </t:CalendarItem>",
                    &format!(
                        "          <t:StartTimeZone Id=\"{TOKYO}\"/>\n          <t:EndTimeZone Id=\"{TOKYO}\"/>\n        </t:CalendarItem>"
                    ),
                );
            for part in [
                "<t:Start>2026-10-18T15:00:00Z</t:Start>",
                "<t:End>2026-10-19T15:00:00Z</t:End>",
                "<t:StartTimeZone Id=\"Tokyo Standard Time\"/>",
            ] {
                assert!(xml.contains(part), "{part}: {xml}");
            }
            envelope(&xml)
        };
        write(
            "R3-5-create-tokyo-display.xml",
            "Step R3-5: NOT Aperio's rule (a zone on an all-day single). An all-day single on the \
             midnights of Monday 19 October in Tokyo. Which weekday and how many days does Outlook \
             show in Berlin?",
            tokyo_create("Aperio zone test R3-5 Tokyo display"),
        );
        write(
            "R3-5b-create-tokyo-update.xml",
            "Step R3-5b: NOT Aperio's rule. The same Tokyo single, as the item R3-5b-u updates.",
            tokyo_create("Aperio zone test R3-5b Tokyo update"),
        );
        let daily = NewEvent {
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=DAILY;COUNT=4".into(),
                exceptions: Vec::new(),
                tzid: None,
            }),
            ..all_day_single("Aperio zone test R3-6 daily")
        };
        let daily_xml = new_event_to_calendar_item_xml(&daily).unwrap();
        assert!(
            daily_xml.contains("<t:StartDate>2026-10-18</t:StartDate>"),
            "{daily_xml}"
        );
        write(
            "R3-6-create-daily.xml",
            "Step R3-6: a daily all-day series from Aperio's create builder, as main sends it: \
             StartDate 2026-10-18 for Monday 19 October. On which day does Exchange start it?",
            envelope(&daily_xml),
        );
    }

    #[test]
    fn event_to_update_field_xml_sets_start_time_zone_for_zoned_master() {
        let ev = Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: "IID|CK".into(),
            calendar_id: "FID|FK".into(),
            title: "Weekly".into(),
            description: None,
            location: None,
            start: "2026-05-20T08:00:00Z".parse().unwrap(),
            end: "2026-05-20T09:00:00Z".parse().unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY".into(),
                exceptions: Vec::new(),
                tzid: Some("Europe/Berlin".into()),
            }),
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("CK".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        };
        let (set, _del) = event_to_update_field_xml(&ev).unwrap();
        assert!(
            set.contains(r#"FieldURI="calendar:StartTimeZone""#),
            "{set}"
        );
        assert!(set.contains(r#"<t:StartTimeZone Id="W. Europe Standard Time"/>"#));
    }

    #[test]
    fn new_event_to_calendar_item_xml_emits_all_day_flag() {
        let mut ev = new_event_min("Holiday");
        ev.all_day = true;
        let xml = new_event_to_calendar_item_xml(&ev).unwrap();
        assert!(xml.contains("<t:IsAllDayEvent>true</t:IsAllDayEvent>"));
    }

    /// All-day instants the way the frontend produces them: LOCAL
    /// midnights (end exclusive), expressed in UTC. Keeps the asserted
    /// wire dates timezone-agnostic.
    fn local_midnight(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Local
            .from_local_datetime(
                &chrono::NaiveDate::from_ymd_opt(y, m, d)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }

    /// The off-by-one guard: a two-day all-day event (June 10–11, end
    /// exclusive June 12) must hit the wire pinned to UTC midnights of
    /// the LOCAL days — not the raw boundary instants, which for a UTC+2
    /// user serialise as 22:00Z of the previous day and make Exchange
    /// pin the event to the wrong day.
    #[test]
    fn all_day_write_pins_local_days_to_utc_midnight() {
        let mut ev = new_event_min("Conference");
        ev.all_day = true;
        ev.start = local_midnight(2026, 6, 10);
        ev.end = local_midnight(2026, 6, 12);
        let xml = new_event_to_calendar_item_xml(&ev).unwrap();
        assert!(
            xml.contains("<t:Start>2026-06-10T00:00:00Z</t:Start>"),
            "{xml}"
        );
        assert!(xml.contains("<t:End>2026-06-12T00:00:00Z</t:End>"), "{xml}");
    }

    /// Read side: all-day boundaries re-anchor at LOCAL midnight, so the
    /// instant renders on the same LOCAL calendar day Exchange pinned.
    #[test]
    fn all_day_read_anchors_local_midnight() {
        let item = ParsedItem {
            item_id: "IID".into(),
            subject: "Conference".into(),
            start: Some("2026-06-10T00:00:00Z".parse().unwrap()),
            end: Some("2026-06-12T00:00:00Z".parse().unwrap()),
            is_all_day: true,
            ..Default::default()
        };
        let ev = to_event(item, "FID").unwrap();
        assert!(ev.all_day);
        // Round-trip stability: writing the read event reproduces the
        // same wire days (no drift on repeated edits).
        let xml = new_event_to_calendar_item_xml(&NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: ev.title.clone(),
            description: None,
            location: None,
            start: ev.start,
            end: ev.end,
            all_day: true,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        })
        .unwrap();
        assert!(
            xml.contains("<t:Start>2026-06-10T00:00:00Z</t:Start>"),
            "{xml}"
        );
        assert!(xml.contains("<t:End>2026-06-12T00:00:00Z</t:End>"), "{xml}");
    }

    #[test]
    fn new_event_to_calendar_item_xml_escapes_subject_specials() {
        let mut ev = new_event_min("Sync & lunch");
        ev.location = Some("Room <A>".into());
        let xml = new_event_to_calendar_item_xml(&ev).unwrap();
        assert!(xml.contains("Sync &amp; lunch"));
        assert!(xml.contains("Room &lt;A&gt;"));
    }

    #[test]
    fn new_event_to_calendar_item_xml_emits_required_attendees() {
        let mut ev = new_event_min("Review");
        ev.attendees = vec![
            "Alice Smith <alice@example.com>".into(),
            "bob@example.com".into(),
        ];
        // Attendees are written whenever present, independent of send_invitations.
        let xml = new_event_to_calendar_item_xml(&ev).unwrap();
        assert!(xml.contains("<t:RequiredAttendees>"));
        assert!(xml.contains("<t:Name>Alice Smith</t:Name>"));
        assert!(xml.contains("<t:EmailAddress>alice@example.com</t:EmailAddress>"));
        assert!(xml.contains("<t:EmailAddress>bob@example.com</t:EmailAddress>"));
        // No attendees → no RequiredAttendees block at all.
        let none = new_event_to_calendar_item_xml(&new_event_min("Solo")).unwrap();
        assert!(!none.contains("<t:RequiredAttendees>"));
    }

    #[test]
    fn rrule_daily_translates_to_daily_recurrence() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=DAILY;INTERVAL=2", start).unwrap();
        assert!(xml.contains("<t:DailyRecurrence>"));
        assert!(xml.contains("<t:Interval>2</t:Interval>"));
        assert!(xml.contains("<t:NoEndRecurrence>"));
        assert!(xml.contains("<t:StartDate>2026-05-20</t:StartDate>"));
    }

    #[test]
    fn rrule_weekly_with_byday_translates_day_names() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=WEEKLY;BYDAY=MO,WE,FR", start).unwrap();
        assert!(xml.contains("<t:WeeklyRecurrence>"));
        assert!(xml.contains("<t:DaysOfWeek>Monday Wednesday Friday</t:DaysOfWeek>"));
    }

    #[test]
    fn rrule_monthly_with_bymonthday_translates_to_absolute_monthly() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=MONTHLY;BYMONTHDAY=15", start).unwrap();
        assert!(xml.contains("<t:AbsoluteMonthlyRecurrence>"));
        assert!(xml.contains("<t:DayOfMonth>15</t:DayOfMonth>"));
    }

    #[test]
    fn rrule_yearly_with_bymonth_translates_to_absolute_yearly() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=15", start).unwrap();
        assert!(xml.contains("<t:AbsoluteYearlyRecurrence>"));
        assert!(xml.contains("<t:Month>March</t:Month>"));
        assert!(xml.contains("<t:DayOfMonth>15</t:DayOfMonth>"));
    }

    #[test]
    fn rrule_count_translates_to_numbered_recurrence() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=DAILY;COUNT=5", start).unwrap();
        assert!(xml.contains("<t:NumberedRecurrence>"));
        assert!(xml.contains("<t:NumberOfOccurrences>5</t:NumberOfOccurrences>"));
    }

    #[test]
    fn rrule_until_translates_to_end_date_recurrence() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml =
            rrule_to_ews_recurrence("FREQ=WEEKLY;BYDAY=TU;UNTIL=20260901T235959Z", start).unwrap();
        assert!(xml.contains("<t:EndDateRecurrence>"));
        assert!(xml.contains("<t:EndDate>2026-09-01</t:EndDate>"));
    }

    #[test]
    fn rrule_relative_monthly_single_day_translates_to_relative_monthly() {
        // "Second Wednesday of every month" → RelativeMonthly with
        // a single DaysOfWeek + DayOfWeekIndex=Second.
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=MONTHLY;BYDAY=2WE", start).unwrap();
        assert!(xml.contains("<t:RelativeMonthlyRecurrence>"));
        assert!(xml.contains("<t:DaysOfWeek>Wednesday</t:DaysOfWeek>"));
        assert!(xml.contains("<t:DayOfWeekIndex>Second</t:DayOfWeekIndex>"));
    }

    #[test]
    fn rrule_relative_monthly_last_maps_negative_ordinal() {
        // BYDAY=-1FR ("last Friday") → DayOfWeekIndex=Last.
        let start: DateTime<Utc> = "2026-05-29T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=MONTHLY;BYDAY=-1FR", start).unwrap();
        assert!(xml.contains("<t:RelativeMonthlyRecurrence>"));
        assert!(xml.contains("<t:DaysOfWeek>Friday</t:DaysOfWeek>"));
        assert!(xml.contains("<t:DayOfWeekIndex>Last</t:DayOfWeekIndex>"));
    }

    #[test]
    fn rrule_relative_monthly_weekday_composite_via_bysetpos() {
        // "Last weekday of the month": multi-day BYDAY + BYSETPOS=-1
        // collapses back into the EWS composite token `Weekday`.
        let start: DateTime<Utc> = "2026-05-29T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1", start)
            .unwrap();
        assert!(xml.contains("<t:RelativeMonthlyRecurrence>"));
        assert!(xml.contains("<t:DaysOfWeek>Weekday</t:DaysOfWeek>"));
        assert!(xml.contains("<t:DayOfWeekIndex>Last</t:DayOfWeekIndex>"));
    }

    #[test]
    fn rrule_relative_yearly_translates_to_relative_yearly() {
        // "First Friday of March every year".
        let start: DateTime<Utc> = "2026-03-06T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=YEARLY;BYMONTH=3;BYDAY=1FR", start).unwrap();
        assert!(xml.contains("<t:RelativeYearlyRecurrence>"));
        assert!(xml.contains("<t:DaysOfWeek>Friday</t:DaysOfWeek>"));
        assert!(xml.contains("<t:DayOfWeekIndex>First</t:DayOfWeekIndex>"));
        assert!(xml.contains("<t:Month>March</t:Month>"));
    }

    #[test]
    fn rrule_relative_recurrence_round_trips_through_read_path() {
        // Writer → reader → writer must be stable. Author "third
        // Wednesday monthly", parse the emitted XML back, and the
        // re-derived RRULE must equal the input.
        let start: DateTime<Utc> = "2024-05-15T10:30:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=MONTHLY;BYDAY=3WE", start).unwrap();
        let reparsed = parse_ews_recurrence(&xml).unwrap();
        assert_rrule_equivalent(&reparsed.to_rrule(), "FREQ=MONTHLY;BYDAY=3WE");
    }

    #[test]
    fn rrule_with_unknown_freq_rejected() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let err = rrule_to_ews_recurrence("FREQ=HOURLY", start).unwrap_err();
        match err {
            EwsError::Protocol(m) => assert!(m.contains("HOURLY")),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn event_to_update_field_xml_deletes_empty_optional_fields() {
        let ev = Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: "IID|CK".into(),
            calendar_id: "FID|FK".into(),
            title: "Updated".into(),
            description: None,
            location: None,
            start: "2026-05-20T08:00:00Z".parse().unwrap(),
            end: "2026-05-20T09:00:00Z".parse().unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("CK".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        };
        let (set, del) = event_to_update_field_xml(&ev).unwrap();
        assert!(set.contains("<t:Subject>Updated</t:Subject>"));
        // No reminder → ReminderIsSet=false (NOT a delete of the
        // minutes field, which EWS rejects with
        // ErrorInvalidPropertyDelete).
        assert!(set.contains("<t:ReminderIsSet>false</t:ReminderIsSet>"));
        assert!(
            !del.contains("FieldURI=\"item:ReminderMinutesBeforeStart\""),
            "ReminderMinutesBeforeStart must not be deleted: {del}",
        );
        // Body is never deleted on EWS — it's loaded lazily and would
        // otherwise be wiped when absent from our cached read model.
        assert!(
            !del.contains("FieldURI=\"item:Body\""),
            "Body must not be deleted (lazy-loaded, would cause data loss): {del}",
        );
        // Location + Recurrence ARE genuinely deletable and still
        // become DeleteItemField blocks when cleared.
        assert!(del.contains("FieldURI=\"calendar:Location\""));
        assert!(del.contains("FieldURI=\"calendar:Recurrence\""));
    }

    /// A saved single, as an editor hands it back.
    fn saved_single(title: &str) -> Event {
        Event {
            id: "S:IID|CK".into(),
            title: title.into(),
            recurrence: None,
            ..zoned_master(None)
        }
    }

    /// A 15-minute reminder.
    fn quarter_hour() -> Vec<Reminder> {
        vec![Reminder {
            kind: ReminderKind::Relative { minutes_before: 15 },
            sound: None,
        }]
    }

    /// Every `FieldURI="…"` a built update names, in order.
    fn field_uris(xml: &str) -> Vec<String> {
        xml.split(r#"FieldURI=""#)
            .skip(1)
            .filter_map(|rest| rest.split('"').next().map(str::to_string))
            .collect()
    }

    /// **The D6/H2 proof** (decision 58a, live round 5). Moving an occurrence
    /// by half an hour writes the slot and nothing else: not the title, and
    /// not the location — which, on an occurrence that has none of its own,
    /// used to go out as a DELETE on every single save.
    #[test]
    fn an_update_that_changed_only_the_time_leaves_the_location_alone() {
        let before = Event {
            location: Some("Room 2".into()),
            ..saved_single("Standup")
        };
        let edit = Event {
            start: before.start + chrono::Duration::minutes(30),
            end: before.end + chrono::Duration::minutes(30),
            ..before.clone()
        };
        let (set, del) =
            event_to_update_field_xml_on(&edit, Some(&before), None, EventIdKind::Exception)
                .unwrap();
        assert_eq!(
            field_uris(&set),
            ["calendar:Start", "calendar:End", "calendar:IsAllDayEvent"],
            "the slot, and only the slot: {set}",
        );
        assert!(del.is_empty(), "and nothing is deleted: {del}");
    }

    /// Clearing a location still clears it: "not changed" and "emptied" are
    /// different facts.
    #[test]
    fn an_update_that_cleared_the_location_deletes_it() {
        let before = Event {
            location: Some("Room 2".into()),
            ..saved_single("Standup")
        };
        let edit = Event {
            location: None,
            ..before.clone()
        };
        let (set, del) =
            event_to_update_field_xml_on(&edit, Some(&before), None, EventIdKind::Single).unwrap();
        assert!(set.is_empty(), "nothing is set: {set}");
        assert_eq!(field_uris(&del), ["calendar:Location"], "{del}");
    }

    /// Without a copy to compare with, every field goes out exactly as it
    /// always did. This is what an unreadable `before` falls back to on a
    /// single or a series head (decision 102).
    #[test]
    fn an_update_without_a_before_writes_every_field_as_it_always_did() {
        let edit = Event {
            location: Some("Room 2".into()),
            reminders: quarter_hour(),
            ..saved_single("Standup")
        };
        let (set, del) =
            event_to_update_field_xml_on(&edit, None, None, EventIdKind::Single).unwrap();
        assert_eq!(
            field_uris(&set),
            [
                "item:Subject",
                "calendar:Location",
                "calendar:Start",
                "calendar:End",
                "calendar:IsAllDayEvent",
                "item:ReminderIsSet",
                "item:ReminderMinutesBeforeStart",
            ],
            "{set}",
        );
        assert_eq!(field_uris(&del), ["calendar:Recurrence"], "{del}");
    }

    /// Every `SetItemField` and `DeleteItemField` block a built update holds,
    /// whole: the field AND the value it writes.
    fn update_blocks(xml: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        for (open, close) in [
            ("<t:SetItemField>", "</t:SetItemField>"),
            ("<t:DeleteItemField>", "</t:DeleteItemField>"),
        ] {
            let mut rest = xml;
            while let Some(at) = rest.find(open) {
                let tail = &rest[at..];
                let end = tail.find(close).expect("a closed block") + close.len();
                blocks.push(tail[..end].to_string());
                rest = &tail[end..];
            }
        }
        blocks
    }

    /// **The subset invariant.** Whatever the pair and whatever the target,
    /// every block an update holds WITH a before — field and value — is one the
    /// same update holds WITHOUT one. The comparison can make an update
    /// smaller; it can never add a field, and never write a field with another
    /// value.
    ///
    /// It cannot see a gate that is missing or wired to the wrong field: that
    /// only writes MORE, or less, of the same blocks.
    /// `each_field_is_written_exactly_when_it_changed` watches the gates.
    #[test]
    fn no_update_writes_a_field_it_would_not_write_without_a_before() {
        let base = Event {
            location: Some("Room 2".into()),
            description: Some("Bring the notes".into()),
            attendees: vec!["alice@example.com".into()],
            reminders: quarter_hour(),
            ..saved_single("Standup")
        };
        let weekly = |tzid: Option<&str>, rrule: &str| {
            Some(EventRecurrence {
                rrule: rrule.into(),
                exceptions: Vec::new(),
                tzid: tzid.map(str::to_string),
            })
        };
        let variants: Vec<(&str, Event)> = vec![
            ("unchanged", base.clone()),
            (
                "retitled",
                Event {
                    title: "Standup, later".into(),
                    ..base.clone()
                },
            ),
            (
                "moved",
                Event {
                    start: base.start + chrono::Duration::hours(1),
                    end: base.end + chrono::Duration::hours(1),
                    ..base.clone()
                },
            ),
            (
                "a day later",
                Event {
                    start: base.start + chrono::Duration::days(1),
                    end: base.end + chrono::Duration::days(1),
                    ..base.clone()
                },
            ),
            (
                "all day",
                Event {
                    all_day: true,
                    ..base.clone()
                },
            ),
            (
                "no location",
                Event {
                    location: None,
                    ..base.clone()
                },
            ),
            (
                "no description",
                Event {
                    description: None,
                    ..base.clone()
                },
            ),
            (
                "no reminder",
                Event {
                    reminders: Vec::new(),
                    ..base.clone()
                },
            ),
            (
                "no invitee",
                Event {
                    attendees: Vec::new(),
                    clear_attendees: true,
                    ..base.clone()
                },
            ),
            (
                "invitees kept",
                Event {
                    keep_attendees: true,
                    keep_fields: Vec::new(),
                    ..base.clone()
                },
            ),
            (
                "weekly in Berlin",
                Event {
                    recurrence: weekly(Some("Europe/Berlin"), "FREQ=WEEKLY"),
                    ..base.clone()
                },
            ),
            (
                "daily in Berlin",
                Event {
                    recurrence: weekly(Some("Europe/Berlin"), "FREQ=DAILY"),
                    ..base.clone()
                },
            ),
            (
                "weekly, no zone",
                Event {
                    recurrence: weekly(None, "FREQ=WEEKLY"),
                    ..base.clone()
                },
            ),
            (
                "weekly in Berlin, a day later",
                Event {
                    start: base.start + chrono::Duration::days(1),
                    end: base.end + chrono::Duration::days(1),
                    recurrence: weekly(Some("Europe/Berlin"), "FREQ=WEEKLY"),
                    ..base.clone()
                },
            ),
        ];
        let server = ServerTimeZones::new(["w. europe standard time"]);
        // What the host may mark kept (decision 106): nothing, some, all.
        let keeps: [Vec<EventField>; 5] = [
            Vec::new(),
            vec![
                EventField::Title,
                EventField::Location,
                EventField::Reminders,
            ],
            vec![EventField::Start, EventField::End, EventField::AllDay],
            vec![EventField::Recurrence, EventField::Start],
            EventField::ALL
                .into_iter()
                .filter(|f| !matches!(f, EventField::Attendees | EventField::ColorHex))
                .collect(),
        ];
        for keep in &keeps {
            for (before_name, before) in &variants {
                for (edit_name, edit) in &variants {
                    let edit = &Event {
                        keep_fields: keep.clone(),
                        ..edit.clone()
                    };
                    for kind in [
                        EventIdKind::Single,
                        EventIdKind::Exception,
                        EventIdKind::RecurringMaster,
                    ] {
                        let blocks = |(set, del): (String, String)| -> Vec<String> {
                            update_blocks(&set)
                                .into_iter()
                                .chain(update_blocks(&del))
                                .collect()
                        };
                        let with = blocks(
                            event_to_update_field_xml_on(edit, Some(before), Some(&server), kind)
                                .unwrap(),
                        );
                        let without = blocks(
                            event_to_update_field_xml_on(edit, None, Some(&server), kind).unwrap(),
                        );
                        for block in with {
                            // The one exception, on purpose: a kept rule and
                            // its zone are the SERVER's, rebuilt on the start
                            // being written (decision 106) — so they are the
                            // server's own values, not this device's.
                            let rule_or_zone = [
                                "calendar:Recurrence",
                                "calendar:StartTimeZone",
                                "calendar:EndTimeZone",
                            ]
                            .iter()
                            .any(|f| block.contains(&format!(r#"FieldURI="{f}""#)));
                            if rule_or_zone && keep.contains(&EventField::Recurrence) {
                                let rule = before.recurrence.as_ref().map(|r| {
                                    rrule_to_ews_recurrence(&r.rrule, edit.start).unwrap()
                                });
                                let zone = series_windows_zone(
                                    edit.all_day,
                                    before.recurrence.as_ref(),
                                    Some(&server),
                                );
                                assert!(
                                    rule.is_some_and(|rule| block.contains(&rule))
                                        || zone.is_some_and(|zone| block.contains(zone)),
                                    "{kind:?}, {before_name} -> {edit_name}, kept {keep:?}: a \
                                     kept rule or zone that is not the server's:\n{block}",
                                );
                                continue;
                            }
                            assert!(
                                without.contains(&block),
                                "{kind:?}, {before_name} -> {edit_name}, kept {keep:?}: written \
                             with a before but not without one, or with another value:\n{block}",
                            );
                        }
                    }
                }
            }
        }
    }

    /// Each gated field, both ways: changed against the before, it is written;
    /// equal to it, it is not. A gate that is dropped, inverted or wired to
    /// the wrong field fails here, by the name of the case.
    #[test]
    fn each_field_is_written_exactly_when_it_changed() {
        let before = Event {
            description: Some("Bring the notes".into()),
            location: Some("Room 2".into()),
            reminders: quarter_hour(),
            ..zoned_master(Some("Europe/Berlin"))
        };
        let slot: &[&str] = &["calendar:Start", "calendar:End", "calendar:IsAllDayEvent"];
        let zone: &[&str] = &["calendar:StartTimeZone", "calendar:EndTimeZone"];
        let cases: Vec<(&str, Event, Vec<&str>)> = vec![
            ("nothing", before.clone(), vec![]),
            (
                "title",
                Event {
                    title: "Weekly sync".into(),
                    ..before.clone()
                },
                vec!["item:Subject"],
            ),
            (
                "description",
                Event {
                    description: Some("Bring the minutes".into()),
                    ..before.clone()
                },
                vec!["item:Body"],
            ),
            (
                "location",
                Event {
                    location: Some("Room 3".into()),
                    ..before.clone()
                },
                vec!["calendar:Location"],
            ),
            (
                "no location",
                Event {
                    location: None,
                    ..before.clone()
                },
                vec!["calendar:Location"],
            ),
            (
                "reminder",
                Event {
                    reminders: vec![Reminder {
                        kind: ReminderKind::Relative { minutes_before: 30 },
                        sound: None,
                    }],
                    ..before.clone()
                },
                vec!["item:ReminderIsSet", "item:ReminderMinutesBeforeStart"],
            ),
            (
                "no reminder",
                Event {
                    reminders: Vec::new(),
                    ..before.clone()
                },
                vec!["item:ReminderIsSet"],
            ),
            (
                // Same day, same weekday: the rule as built is the same, so it
                // stays; the zone rides with the slot.
                "an hour later",
                Event {
                    start: before.start + chrono::Duration::hours(1),
                    end: before.end + chrono::Duration::hours(1),
                    ..before.clone()
                },
                [slot, zone].concat(),
            ),
            (
                "daily",
                Event {
                    recurrence: Some(EventRecurrence {
                        rrule: "FREQ=DAILY".into(),
                        exceptions: Vec::new(),
                        tzid: Some("Europe/Berlin".into()),
                    }),
                    ..before.clone()
                },
                [&["calendar:Recurrence"][..], zone].concat(),
            ),
            (
                "no rule",
                Event {
                    recurrence: None,
                    ..before.clone()
                },
                vec!["calendar:Recurrence"],
            ),
        ];
        for (name, edit, expected) in cases {
            let (set, del) = event_to_update_field_xml_on(
                &edit,
                Some(&before),
                None,
                EventIdKind::RecurringMaster,
            )
            .unwrap();
            let written: Vec<String> = field_uris(&set)
                .into_iter()
                .chain(field_uris(&del))
                .collect();
            assert_eq!(written, expected, "{name}:\nset: {set}\ndel: {del}");
        }
    }

    /// A series dragged to another day keeps its rule's TEXT, but not the rule
    /// Exchange stores: the range's StartDate is the start's date, and a weekly
    /// rule without BYDAY repeats on the start's weekday. Both go out again, or
    /// the server keeps the old first day (review of #89).
    #[test]
    fn a_series_moved_to_another_day_rewrites_its_rule() {
        let weekly = zoned_master(Some("Europe/Berlin"));
        let ten_days = Event {
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=DAILY;COUNT=10".into(),
                exceptions: Vec::new(),
                tzid: Some("Europe/Berlin".into()),
            }),
            ..zoned_master(Some("Europe/Berlin"))
        };
        for (before, days, first_day, also) in [
            (
                weekly,
                1,
                "2026-05-21",
                "<t:DaysOfWeek>Thursday</t:DaysOfWeek>",
            ),
            (
                ten_days,
                7,
                "2026-05-27",
                "<t:NumberOfOccurrences>10</t:NumberOfOccurrences>",
            ),
        ] {
            let edit = Event {
                start: before.start + chrono::Duration::days(days),
                end: before.end + chrono::Duration::days(days),
                ..before.clone()
            };
            let (set, _) = event_to_update_field_xml_on(
                &edit,
                Some(&before),
                None,
                EventIdKind::RecurringMaster,
            )
            .unwrap();
            let rrule = &before.recurrence.as_ref().unwrap().rrule;
            assert!(
                field_uris(&set).contains(&"calendar:Recurrence".to_string()),
                "{rrule}: {set}"
            );
            assert!(
                set.contains(&format!("<t:StartDate>{first_day}</t:StartDate>")),
                "{rrule}: {set}"
            );
            assert!(set.contains(also), "{rrule}: {set}");
        }
    }

    /// Every field but the slot, as the host marks an edit that only moved it.
    fn all_but_the_slot() -> Vec<EventField> {
        EventField::ALL
            .into_iter()
            .filter(|f| {
                !matches!(
                    f,
                    EventField::Start
                        | EventField::End
                        | EventField::Attendees
                        | EventField::ColorHex
                )
            })
            .collect()
    }

    /// Decision 106, the round-5 remnant. A row that inherited the SERIES'
    /// content (#88's fallback), moved by the user. What the user did not
    /// touch is kept, so the occurrence's own title, place and reminder —
    /// which differ from the row's — are not overwritten with the series'.
    #[test]
    fn an_inherited_occurrence_writes_only_what_the_user_moved() {
        let opened = Event {
            etag: Some("inherited:ECK:MCK".into()),
            location: Some("Room 1".into()),
            ..saved_single("Standup")
        };
        let own = Event {
            title: "Kickoff".into(),
            location: Some("Room 7".into()),
            reminders: quarter_hour(),
            ..opened.clone()
        };
        let mut moved = Event {
            start: opened.start + chrono::Duration::hours(2),
            end: opened.end + chrono::Duration::hours(2),
            ..opened.clone()
        };
        moved.keep_fields = cal_core::event_diff::kept_fields(&moved, Some(&opened), true);

        let (set, del) =
            event_to_update_field_xml_on(&moved, Some(&own), None, EventIdKind::Exception).unwrap();
        assert_eq!(
            field_uris(&set),
            ["calendar:Start", "calendar:End", "calendar:IsAllDayEvent"],
            "{set}"
        );
        assert!(del.is_empty(), "{del}");

        // Without the host's proof, the comparison with the server alone writes
        // the row's stale content back — what #89 did, and still does then.
        let blind = Event {
            keep_fields: Vec::new(),
            ..moved.clone()
        };
        let (set, _) =
            event_to_update_field_xml_on(&blind, Some(&own), None, EventIdKind::Exception).unwrap();
        assert!(set.contains("<t:Subject>Standup</t:Subject>"), "{set}");
    }

    /// A zoned weekly series (no BYDAY), which another device has since moved
    /// from Wednesday to Thursday; this device's copy still says Wednesday.
    fn series_moved_elsewhere() -> (Event, Event) {
        let opened = zoned_master(Some("Europe/Berlin"));
        let server = Event {
            start: opened.start + chrono::Duration::days(1),
            end: opened.end + chrono::Duration::days(1),
            ..opened.clone()
        };
        (opened, server)
    }

    /// A slot that puts a start on the server that is not the server's takes
    /// the rule along, rebuilt from that start — even a kept rule. Otherwise
    /// the start says Wednesday and the stored rule's first day Thursday.
    #[test]
    fn a_slot_that_moves_the_servers_start_rebuilds_a_kept_rule() {
        let (opened, server) = series_moved_elsewhere();
        let mut longer = Event {
            end: opened.end + chrono::Duration::minutes(30),
            ..opened.clone()
        };
        longer.keep_fields = cal_core::event_diff::kept_fields(&longer, Some(&opened), true);
        assert!(longer.keep_fields.contains(&EventField::Start));
        assert!(longer.keep_fields.contains(&EventField::Recurrence));

        let (set, _) = event_to_update_field_xml_on(
            &longer,
            Some(&server),
            None,
            EventIdKind::RecurringMaster,
        )
        .unwrap();
        assert!(
            field_uris(&set).contains(&"calendar:Recurrence".to_string()),
            "{set}"
        );
        assert!(
            set.contains("<t:StartDate>2026-05-20</t:StartDate>"),
            "{set}"
        );
        assert!(
            set.contains("<t:DaysOfWeek>Wednesday</t:DaysOfWeek>"),
            "{set}"
        );
    }

    /// A kept rule under a start that stays where the server has it is the
    /// server's: another device's COUNT survives a change to the duration.
    #[test]
    fn a_kept_rule_under_a_start_that_stays_is_left_as_the_server_has_it() {
        let opened = zoned_master(Some("Europe/Berlin"));
        let server = Event {
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;COUNT=5".into(),
                exceptions: Vec::new(),
                tzid: Some("Europe/Berlin".into()),
            }),
            ..opened.clone()
        };
        let mut longer = Event {
            end: opened.end + chrono::Duration::minutes(30),
            ..opened.clone()
        };
        longer.keep_fields = cal_core::event_diff::kept_fields(&longer, Some(&opened), true);

        let (set, _) = event_to_update_field_xml_on(
            &longer,
            Some(&server),
            None,
            EventIdKind::RecurringMaster,
        )
        .unwrap();
        assert!(
            !field_uris(&set).contains(&"calendar:Recurrence".to_string()),
            "{set}"
        );
    }

    /// And the other way round: a rule the user changed, built from a kept
    /// start another device has since moved, brings its slot along.
    #[test]
    fn a_changed_rule_on_a_start_the_server_moved_brings_the_slot_along() {
        let (opened, server) = series_moved_elsewhere();
        let mut daily = Event {
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=DAILY".into(),
                exceptions: Vec::new(),
                tzid: Some("Europe/Berlin".into()),
            }),
            ..opened.clone()
        };
        daily.keep_fields = cal_core::event_diff::kept_fields(&daily, Some(&opened), true);
        assert!(daily.keep_fields.contains(&EventField::Start));

        let (set, _) =
            event_to_update_field_xml_on(&daily, Some(&server), None, EventIdKind::RecurringMaster)
                .unwrap();
        let written = field_uris(&set);
        for field in ["calendar:Start", "calendar:End", "calendar:Recurrence"] {
            assert!(written.contains(&field.to_string()), "{field}: {set}");
        }
    }

    /// Without a copy to compare with, a moved series still writes its rule —
    /// a missing copy's rule is unknown, not equal — and a moved single never
    /// deletes one.
    #[test]
    fn a_blind_write_that_moves_a_series_writes_its_rule() {
        let series = zoned_master(Some("Europe/Berlin"));
        let moved = Event {
            start: series.start + chrono::Duration::days(1),
            end: series.end + chrono::Duration::days(1),
            keep_fields: all_but_the_slot(),
            ..series.clone()
        };
        let (set, del) =
            event_to_update_field_xml_on(&moved, None, None, EventIdKind::RecurringMaster).unwrap();
        assert!(
            set.contains("<t:StartDate>2026-05-21</t:StartDate>"),
            "{set}"
        );
        assert!(del.is_empty(), "{del}");

        let single = Event {
            keep_fields: all_but_the_slot(),
            ..saved_single("Standup")
        };
        let moved = Event {
            start: single.start + chrono::Duration::days(1),
            end: single.end + chrono::Duration::days(1),
            ..single
        };
        let (set, del) =
            event_to_update_field_xml_on(&moved, None, None, EventIdKind::Single).unwrap();
        assert_eq!(
            field_uris(&set),
            ["calendar:Start", "calendar:End", "calendar:IsAllDayEvent"],
            "{set}"
        );
        assert!(del.is_empty(), "no rule is deleted: {del}");
    }

    /// A kept rule that must go along with a moved start is the SERVER's rule,
    /// rebuilt on that start: another device's end-after-five survives a drag
    /// of the series to another day.
    #[test]
    fn a_kept_rule_is_the_servers_when_the_slot_takes_it_along() {
        let opened = zoned_master(Some("Europe/Berlin"));
        let server = Event {
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;COUNT=5".into(),
                exceptions: Vec::new(),
                tzid: Some("Europe/Berlin".into()),
            }),
            ..opened.clone()
        };
        let mut dragged = Event {
            start: opened.start + chrono::Duration::days(1),
            end: opened.end + chrono::Duration::days(1),
            ..opened.clone()
        };
        dragged.keep_fields = cal_core::event_diff::kept_fields(&dragged, Some(&opened), true);
        assert!(dragged.keep_fields.contains(&EventField::Recurrence));

        let (set, del) = event_to_update_field_xml_on(
            &dragged,
            Some(&server),
            None,
            EventIdKind::RecurringMaster,
        )
        .unwrap();
        assert!(
            set.contains("<t:NumberOfOccurrences>5</t:NumberOfOccurrences>"),
            "the server's end: {set}"
        );
        assert!(
            set.contains("<t:StartDate>2026-05-21</t:StartDate>"),
            "{set}"
        );
        assert!(del.is_empty(), "{del}");
    }

    /// A series another device has since made a single stays a single when
    /// this one moves it: a kept rule is rebuilt only onto a rule that is there.
    #[test]
    fn a_moved_series_never_restores_a_rule_the_server_dropped() {
        let opened = Event {
            etag: Some("v1".into()),
            ..zoned_master(Some("Europe/Berlin"))
        };
        let server = Event {
            recurrence: None,
            ..opened.clone()
        };
        let mut moved = Event {
            start: opened.start + chrono::Duration::hours(1),
            end: opened.end + chrono::Duration::hours(1),
            ..opened.clone()
        };
        moved.keep_fields = cal_core::event_diff::kept_fields(&moved, Some(&opened), true);

        let (set, del) =
            event_to_update_field_xml_on(&moved, Some(&server), None, EventIdKind::RecurringMaster)
                .unwrap();
        assert!(set.contains(r#"FieldURI="calendar:Start""#), "{set}");
        assert!(!set.contains("calendar:Recurrence"), "{set}");
        assert!(!del.contains("calendar:Recurrence"), "{del}");
        assert!(!set.contains("TimeZone"), "no zone for a single: {set}");
    }

    /// A kept rule's zone is the server's too: another device moved the series
    /// to New York, and lengthening it here does not put it back in Berlin.
    #[test]
    fn a_kept_rules_zone_is_the_servers() {
        let opened = zoned_master(Some("Europe/Berlin"));
        let server = Event {
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY".into(),
                exceptions: Vec::new(),
                tzid: Some("America/New_York".into()),
            }),
            ..opened.clone()
        };
        let mut longer = Event {
            end: opened.end + chrono::Duration::minutes(30),
            ..opened.clone()
        };
        longer.keep_fields = cal_core::event_diff::kept_fields(&longer, Some(&opened), true);

        let (set, _) = event_to_update_field_xml_on(
            &longer,
            Some(&server),
            None,
            EventIdKind::RecurringMaster,
        )
        .unwrap();
        assert!(
            set.contains(r#"<t:StartTimeZone Id="Eastern Standard Time"/>"#),
            "{set}"
        );
        assert!(!set.contains("W. Europe"), "{set}");
    }

    /// An event the user moved but whose rule they never touched — there was
    /// none when they opened it — does not delete the rule another device has
    /// since given it.
    #[test]
    fn a_moved_event_never_deletes_a_rule_it_did_not_touch() {
        let opened = Event {
            etag: Some("v1".into()),
            ..saved_single("Standup")
        };
        let server = Event {
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY".into(),
                exceptions: Vec::new(),
                tzid: None,
            }),
            ..opened.clone()
        };
        let mut moved = Event {
            start: opened.start + chrono::Duration::days(1),
            end: opened.end + chrono::Duration::days(1),
            ..opened.clone()
        };
        moved.keep_fields = cal_core::event_diff::kept_fields(&moved, Some(&opened), true);

        let (_, del) =
            event_to_update_field_xml_on(&moved, Some(&server), None, EventIdKind::Single).unwrap();
        assert!(!del.contains("calendar:Recurrence"), "{del}");
    }

    /// A rule that did not change is neither written nor deleted — and a
    /// title-only save on a zoned series carries no zone pair either.
    #[test]
    fn an_unchanged_recurrence_is_neither_set_nor_deleted() {
        let before = zoned_master(Some("Europe/Berlin"));
        let edit = Event {
            title: "Weekly sync".into(),
            ..before.clone()
        };
        let (set, del) =
            event_to_update_field_xml_on(&edit, Some(&before), None, EventIdKind::RecurringMaster)
                .unwrap();
        assert_eq!(field_uris(&set), ["item:Subject"], "{set}");
        assert!(del.is_empty(), "{del}");
    }

    /// The zone rides with the RULE, not with the clock: weekly to daily
    /// without moving the series still carries the zone the series repeats in
    /// (decision 41a), because a rule written without one re-expands in UTC.
    #[test]
    fn a_rule_change_carries_the_zone_even_when_the_time_stands_still() {
        let before = zoned_master(Some("Europe/Berlin"));
        let edit = Event {
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=DAILY".into(),
                exceptions: Vec::new(),
                tzid: Some("Europe/Berlin".into()),
            }),
            ..before.clone()
        };
        let (set, _) =
            event_to_update_field_xml_on(&edit, Some(&before), None, EventIdKind::RecurringMaster)
                .unwrap();
        assert_eq!(
            field_uris(&set),
            [
                "calendar:Recurrence",
                "calendar:StartTimeZone",
                "calendar:EndTimeZone",
            ],
            "{set}",
        );
    }

    /// The slot is one fact: turning an event into an all-day one writes both
    /// boundaries with it, because `ews_all_day_boundary` rewrites them from
    /// the flag and Exchange validates a Start against the End it has stored.
    #[test]
    fn the_slot_is_written_as_one() {
        let before = saved_single("Trip");
        let edit = Event {
            all_day: true,
            ..before.clone()
        };
        let (set, _) =
            event_to_update_field_xml_on(&edit, Some(&before), None, EventIdKind::Single).unwrap();
        assert_eq!(
            field_uris(&set),
            ["calendar:Start", "calendar:End", "calendar:IsAllDayEvent"],
            "{set}",
        );
    }

    /// The invitee rules keep their authority: what goes out is decided by
    /// `keep_attendees` and `clear_attendees` (decisions 71a, 74a), never by
    /// the field diff. The host knows whether the edit touched the list; the
    /// diff only knows that this device's copy of it looks the same.
    #[test]
    fn attendees_are_not_suppressed_by_the_field_diff() {
        let before = Event {
            attendees: vec!["alice@example.com".into()],
            ..saved_single("Review")
        };
        let edit = Event {
            title: "Review, moved room".into(),
            ..before.clone()
        };
        let (set, _) =
            event_to_update_field_xml_on(&edit, Some(&before), None, EventIdKind::Single).unwrap();
        assert_eq!(
            field_uris(&set),
            ["item:Subject", "calendar:RequiredAttendees"],
            "{set}",
        );
    }

    /// Live test round 3: Exchange refuses `DeleteItemField calendar:Recurrence`
    /// on an exception (`ErrorInvalidPropertyDelete`) and fails the whole
    /// update. An override edit carries no rule, so the delete must stay out
    /// for an exception target, and stay in for every other kind.
    #[test]
    fn an_update_of_an_exception_never_deletes_its_rule() {
        let override_edit = Event {
            recurrence: None,
            ..zoned_master(None)
        };
        let (_, del) =
            event_to_update_field_xml_on(&override_edit, None, None, EventIdKind::Exception)
                .unwrap();
        assert!(!del.contains("calendar:Recurrence"), "{del}");
        // Without a copy to compare with, an absent location is still cleared.
        assert!(del.contains(r#"FieldURI="calendar:Location""#), "{del}");
        // With one, it is not touched at all: it never differed (58a). This
        // half is the round-5 defect; the rule half above is round 3's.
        let (set, del) = event_to_update_field_xml_on(
            &override_edit,
            Some(&override_edit),
            None,
            EventIdKind::Exception,
        )
        .unwrap();
        assert!(
            !set.contains("calendar:Location") && !del.contains("calendar:Location"),
            "set: {set}\ndel: {del}",
        );
        for kind in [EventIdKind::Single, EventIdKind::RecurringMaster] {
            let (_, del) = event_to_update_field_xml_on(&override_edit, None, None, kind).unwrap();
            assert!(
                del.contains(r#"FieldURI="calendar:Recurrence""#),
                "{kind:?}: {del}"
            );
        }
    }

    #[test]
    fn encode_then_decode_event_id_roundtrips() {
        let cases = [
            (EventIdKind::Single, "I1", Some("C1")),
            (EventIdKind::Occurrence, "I2", Some("C2")),
            (EventIdKind::Exception, "I3", Some("C3")),
            (EventIdKind::RecurringMaster, "I4", None),
        ];
        for (kind, id, ck) in cases {
            let encoded = encode_event_id(kind, id, ck);
            let decoded = decode_event_id(&encoded);
            assert_eq!(decoded.kind, kind);
            assert_eq!(decoded.item_id, id);
            assert_eq!(decoded.change_key.as_deref(), ck);
        }
    }

    #[test]
    fn decode_event_id_falls_back_to_single_for_unprefixed_legacy_ids() {
        // ids minted before 6f.1c land as "id|ck" without a prefix —
        // decoder should treat them as Single so persisted local-only
        // EXDATE rows etc keep resolving.
        let decoded = decode_event_id("RAW-ID|RAW-CK");
        assert_eq!(decoded.kind, EventIdKind::Single);
        assert_eq!(decoded.item_id, "RAW-ID");
        assert_eq!(decoded.change_key.as_deref(), Some("RAW-CK"));
    }

    #[test]
    fn decode_event_id_handles_id_without_change_key() {
        let decoded = decode_event_id("O:JUST-ID");
        assert_eq!(decoded.kind, EventIdKind::Occurrence);
        assert_eq!(decoded.item_id, "JUST-ID");
        assert!(decoded.change_key.is_none());
    }

    #[test]
    fn to_event_picks_kind_from_calendar_item_type() {
        let mk = |item_type: Option<&str>| ParsedItem {
            my_response_type: None,
            item_id: "IID".into(),
            change_key: Some("ICK".into()),
            subject: "X".into(),
            body: None,
            location: None,
            start: Some("2026-05-20T08:00:00Z".parse().unwrap()),
            end: Some("2026-05-20T09:00:00Z".parse().unwrap()),
            is_all_day: false,
            is_recurring: false,
            reminder_is_set: false,
            reminder_minutes_before_start: None,
            created: None,
            last_modified: None,
            item_type: item_type.map(String::from),
            start_time_zone: None,
            end_time_zone: None,
            original_start: None,
            recurrence: None,
            deleted_occurrence_starts: Vec::new(),
            modified_occurrences: Vec::new(),
            organizer: None,
            attendees: Vec::new(),
            detail_fetched: false,
            cancelled: false,
            appointment_state: None,
        };
        assert_eq!(to_event(mk(Some("Single")), "FID").unwrap().id, "S:IID|ICK");
        assert_eq!(
            to_event(mk(Some("Occurrence")), "FID").unwrap().id,
            "O:IID|ICK"
        );
        assert_eq!(
            to_event(mk(Some("Exception")), "FID").unwrap().id,
            "E:IID|ICK"
        );
        assert_eq!(
            to_event(mk(Some("RecurringMaster")), "FID").unwrap().id,
            "M:IID|ICK",
        );
        // Missing element → Single (defensive default).
        assert_eq!(to_event(mk(None), "FID").unwrap().id, "S:IID|ICK");
    }

    #[test]
    fn parse_first_item_id_extracts_attrs() {
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:CreateItemResponse>
      <m:ResponseMessages>
        <m:CreateItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="NEWID" ChangeKey="NEWCK"/>
            </t:CalendarItem>
          </m:Items>
        </m:CreateItemResponseMessage>
      </m:ResponseMessages>
    </m:CreateItemResponse>
  </s:Body>
</s:Envelope>"#;
        let r = parse_first_item_id(xml).unwrap();
        assert_eq!(r.id, "NEWID");
        assert_eq!(r.change_key.as_deref(), Some("NEWCK"));
    }

    // ── EWS recurrence parser ────────────────────────────────────

    /// Each test below runs the same script: hand-craft the
    /// expected RRULE → `rrule_to_ews_recurrence` to obtain the
    /// XML the server would send → `parse_ews_recurrence` →
    /// `to_rrule()`. The roundtrip must produce a semantically
    /// equivalent rule string (same key/value pairs, order may
    /// differ — we compare after normalising).
    fn normalise_rrule(s: &str) -> Vec<String> {
        let mut parts: Vec<String> = s.split(';').map(str::to_string).collect();
        parts.sort();
        parts
    }
    fn assert_rrule_equivalent(a: &str, b: &str) {
        assert_eq!(
            normalise_rrule(a),
            normalise_rrule(b),
            "RRULE not equivalent: {a} vs {b}",
        );
    }

    #[test]
    fn parse_daily_recurrence_roundtrips() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=DAILY;INTERVAL=3", start).unwrap();
        let rec = parse_ews_recurrence(&xml).unwrap();
        assert_eq!(rec.pattern, EwsRecurrencePattern::Daily { interval: 3 },);
        assert_eq!(rec.range, EwsRecurrenceRange::NoEnd);
        assert_rrule_equivalent(&rec.to_rrule(), "FREQ=DAILY;INTERVAL=3");
    }

    #[test]
    fn parse_weekly_with_byday_roundtrips() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=10", start).unwrap();
        let rec = parse_ews_recurrence(&xml).unwrap();
        assert_eq!(
            rec.pattern,
            EwsRecurrencePattern::Weekly {
                interval: 1,
                days_of_week: vec![EwsDay::Monday, EwsDay::Wednesday, EwsDay::Friday],
                first_day_of_week: EwsDay::Monday,
            },
        );
        assert_eq!(rec.range, EwsRecurrenceRange::Numbered { occurrences: 10 },);
        assert_rrule_equivalent(&rec.to_rrule(), "FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=10");
    }

    #[test]
    fn weekly_first_day_of_week_round_trips_through_wkst() {
        let start: DateTime<Utc> = "2026-07-05T09:00:00Z".parse().unwrap();

        // WRITE: a WKST=SU rule pins the wire FirstDayOfWeek to Sunday.
        let xml = rrule_to_ews_recurrence("FREQ=WEEKLY;INTERVAL=2;BYDAY=SU,MO,TU;WKST=SU", start)
            .unwrap();
        assert!(
            xml.contains("<t:FirstDayOfWeek>Sunday</t:FirstDayOfWeek>"),
            "expected Sunday first-day on the wire, got {xml}",
        );

        // READ back: the pattern carries Sunday, and to_rrule re-emits WKST=SU so
        // the frontend expands the straddling series on the right week grid.
        let rec = parse_ews_recurrence(&xml).unwrap();
        assert!(matches!(
            rec.pattern,
            EwsRecurrencePattern::Weekly {
                first_day_of_week: EwsDay::Sunday,
                ..
            }
        ));
        assert!(
            rec.to_rrule().contains("WKST=SU"),
            "expected WKST=SU, got {}",
            rec.to_rrule(),
        );

        // A Monday-week rule: FirstDayOfWeek=Monday on the wire, but NO WKST in
        // the RRULE (the RFC-5545 default — keeps the common rule byte-identical).
        let mon = rrule_to_ews_recurrence("FREQ=WEEKLY;BYDAY=TH", start).unwrap();
        assert!(mon.contains("<t:FirstDayOfWeek>Monday</t:FirstDayOfWeek>"));
        assert!(!parse_ews_recurrence(&mon)
            .unwrap()
            .to_rrule()
            .contains("WKST"));
    }

    #[test]
    fn parse_absolute_monthly_with_enddate_roundtrips() {
        let start: DateTime<Utc> = "2026-05-15T08:00:00Z".parse().unwrap();
        let xml =
            rrule_to_ews_recurrence("FREQ=MONTHLY;BYMONTHDAY=15;UNTIL=20271231T000000Z", start)
                .unwrap();
        let rec = parse_ews_recurrence(&xml).unwrap();
        assert_eq!(
            rec.pattern,
            EwsRecurrencePattern::AbsoluteMonthly {
                interval: 1,
                day_of_month: 15,
            },
        );
        assert_eq!(
            rec.range,
            EwsRecurrenceRange::EndDate {
                end: "2027-12-31".into(),
            },
        );
        // Reader re-emits UNTIL as an end-of-day UTC instant so the
        // rule validates against a UTC DTSTART (the strict `rrule`
        // crate rejects a date-only UNTIL there).
        assert_rrule_equivalent(
            &rec.to_rrule(),
            "FREQ=MONTHLY;BYMONTHDAY=15;UNTIL=20271231T235959Z",
        );
    }

    #[test]
    fn parse_absolute_yearly_roundtrips() {
        let start: DateTime<Utc> = "2026-03-21T09:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=21", start).unwrap();
        let rec = parse_ews_recurrence(&xml).unwrap();
        assert_eq!(
            rec.pattern,
            EwsRecurrencePattern::AbsoluteYearly {
                day_of_month: 21,
                month: EwsMonth::March,
            },
        );
        assert_eq!(rec.range, EwsRecurrenceRange::NoEnd);
        assert_rrule_equivalent(&rec.to_rrule(), "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=21");
    }

    #[test]
    fn parse_recurrence_relative_monthly_single_day() {
        // "Third Wednesday of every month" — single-day Relative
        // shape. The RRULE form folds the index prefix into the
        // BYDAY token (`BYDAY=3WE`) rather than emitting a
        // separate BYSETPOS; rrule.js handles both forms
        // identically but the single-token form is what
        // we ship.
        let xml = r#"<t:Recurrence>
            <t:RelativeMonthlyRecurrence>
              <t:Interval>1</t:Interval>
              <t:DaysOfWeek>Wednesday</t:DaysOfWeek>
              <t:DayOfWeekIndex>Third</t:DayOfWeekIndex>
            </t:RelativeMonthlyRecurrence>
            <t:NoEndRecurrence>
              <t:StartDate>2024-05-15</t:StartDate>
            </t:NoEndRecurrence>
          </t:Recurrence>"#;
        let rec = parse_ews_recurrence(xml).unwrap();
        assert_eq!(
            rec.pattern,
            EwsRecurrencePattern::RelativeMonthly {
                interval: 1,
                days_of_week: vec![EwsDay::Wednesday],
                day_of_week_index: EwsDayOfWeekIndex::Third,
            },
        );
        assert_rrule_equivalent(&rec.to_rrule(), "FREQ=MONTHLY;BYDAY=3WE");
    }

    #[test]
    fn parse_recurrence_relative_monthly_last_weekday_composite() {
        // "Last weekday of every other month" — composite DaysOfWeek
        // token (`Weekday`) + Last index. Expansion: Weekday → MO-FR,
        // multi-day branch → BYDAY=MO,TU,WE,TH,FR + BYSETPOS=-1.
        let xml = r#"<t:Recurrence>
            <t:RelativeMonthlyRecurrence>
              <t:Interval>2</t:Interval>
              <t:DaysOfWeek>Weekday</t:DaysOfWeek>
              <t:DayOfWeekIndex>Last</t:DayOfWeekIndex>
            </t:RelativeMonthlyRecurrence>
            <t:NoEndRecurrence>
              <t:StartDate>2026-01-30</t:StartDate>
            </t:NoEndRecurrence>
          </t:Recurrence>"#;
        let rec = parse_ews_recurrence(xml).unwrap();
        match &rec.pattern {
            EwsRecurrencePattern::RelativeMonthly {
                interval,
                days_of_week,
                day_of_week_index,
            } => {
                assert_eq!(*interval, 2);
                assert_eq!(
                    days_of_week,
                    &vec![
                        EwsDay::Monday,
                        EwsDay::Tuesday,
                        EwsDay::Wednesday,
                        EwsDay::Thursday,
                        EwsDay::Friday,
                    ],
                );
                assert_eq!(*day_of_week_index, EwsDayOfWeekIndex::Last);
            }
            other => panic!("expected RelativeMonthly, got {other:?}"),
        }
        assert_rrule_equivalent(
            &rec.to_rrule(),
            "FREQ=MONTHLY;INTERVAL=2;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
        );
    }

    #[test]
    fn parse_recurrence_relative_yearly() {
        // "First Friday of March every year".
        let xml = r#"<t:Recurrence>
            <t:RelativeYearlyRecurrence>
              <t:DaysOfWeek>Friday</t:DaysOfWeek>
              <t:DayOfWeekIndex>First</t:DayOfWeekIndex>
              <t:Month>March</t:Month>
            </t:RelativeYearlyRecurrence>
            <t:NoEndRecurrence>
              <t:StartDate>2026-03-06</t:StartDate>
            </t:NoEndRecurrence>
          </t:Recurrence>"#;
        let rec = parse_ews_recurrence(xml).unwrap();
        assert_eq!(
            rec.pattern,
            EwsRecurrencePattern::RelativeYearly {
                days_of_week: vec![EwsDay::Friday],
                day_of_week_index: EwsDayOfWeekIndex::First,
                month: EwsMonth::March,
            },
        );
        assert_rrule_equivalent(&rec.to_rrule(), "FREQ=YEARLY;BYMONTH=3;BYDAY=1FR");
    }

    #[test]
    fn parse_recurrence_relative_monthly_requires_day_of_week_index() {
        // Server returns RelativeMonthly without the required
        // DayOfWeekIndex element — surface as Protocol so the
        // caller drops just this row's recurrence (consistent with
        // the AbsoluteYearly + missing-Month case).
        let xml = r#"<t:Recurrence>
            <t:RelativeMonthlyRecurrence>
              <t:Interval>1</t:Interval>
              <t:DaysOfWeek>Monday</t:DaysOfWeek>
            </t:RelativeMonthlyRecurrence>
            <t:NoEndRecurrence>
              <t:StartDate>2026-01-01</t:StartDate>
            </t:NoEndRecurrence>
          </t:Recurrence>"#;
        let err = parse_ews_recurrence(xml).unwrap_err();
        match err {
            EwsError::Protocol(m) => assert!(m.contains("DayOfWeekIndex"), "got {m}"),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn parse_recurrence_handles_unprefixed_namespaces() {
        // Some servers (or aggressive XML stripping intermediaries)
        // drop the `t:` namespace prefix on element names. The
        // walker compares on local names already, so this should
        // still parse — guard the invariant with a test.
        let xml = r#"<Recurrence>
            <DailyRecurrence>
              <Interval>2</Interval>
            </DailyRecurrence>
            <NoEndRecurrence>
              <StartDate>2026-01-01</StartDate>
            </NoEndRecurrence>
          </Recurrence>"#;
        let rec = parse_ews_recurrence(xml).unwrap();
        assert_eq!(rec.pattern, EwsRecurrencePattern::Daily { interval: 2 });
    }

    #[test]
    fn parse_recurrence_rejects_missing_pattern() {
        let xml = r#"<t:Recurrence>
            <t:NoEndRecurrence>
              <t:StartDate>2026-01-01</t:StartDate>
            </t:NoEndRecurrence>
          </t:Recurrence>"#;
        let err = parse_ews_recurrence(xml).unwrap_err();
        assert!(matches!(err, EwsError::Protocol(_)));
    }

    #[test]
    fn parse_recurrence_rejects_missing_range() {
        let xml = r#"<t:Recurrence>
            <t:DailyRecurrence><t:Interval>1</t:Interval></t:DailyRecurrence>
          </t:Recurrence>"#;
        let err = parse_ews_recurrence(xml).unwrap_err();
        assert!(matches!(err, EwsError::Protocol(_)));
    }

    // ── SyncFolderItems response parser ──────────────────────────

    #[test]
    fn parse_sync_response_with_create_update_delete() {
        // Exercises all three change kinds in one batch + the
        // SyncState + IncludesLastItemInRange tail. Folder/IDs
        // come from a real EWS log so the shape matches what
        // Exchange Online actually emits.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:SyncFolderItemsResponse>
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:SyncState>STATE-COOKIE-VALUE</m:SyncState>
          <m:IncludesLastItemInRange>false</m:IncludesLastItemInRange>
          <m:Changes>
            <t:Create>
              <t:CalendarItem>
                <t:ItemId Id="NEW-1" ChangeKey="CK-1"/>
                <t:Subject>Brand new</t:Subject>
                <t:Start>2026-05-20T08:00:00Z</t:Start>
                <t:End>2026-05-20T09:00:00Z</t:End>
                <t:IsAllDayEvent>false</t:IsAllDayEvent>
                <t:IsRecurring>false</t:IsRecurring>
                <t:IsCancelled>false</t:IsCancelled>
                <t:AppointmentState>5</t:AppointmentState>
                <t:CalendarItemType>Single</t:CalendarItemType>
              </t:CalendarItem>
            </t:Create>
            <t:Update>
              <t:CalendarItem>
                <t:ItemId Id="UPD-1" ChangeKey="CK-2"/>
                <t:Subject>Edited</t:Subject>
                <t:Start>2026-05-21T10:00:00Z</t:Start>
                <t:End>2026-05-21T11:00:00Z</t:End>
                <t:IsAllDayEvent>false</t:IsAllDayEvent>
                <t:IsRecurring>true</t:IsRecurring>
                <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
              </t:CalendarItem>
            </t:Update>
            <t:Delete>
              <t:ItemId Id="DEL-1" ChangeKey="CK-3"/>
            </t:Delete>
          </m:Changes>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </soap:Body>
</soap:Envelope>"#;
        let r = parse_sync_folder_items_response(xml).unwrap();
        assert_eq!(r.new_sync_state, "STATE-COOKIE-VALUE");
        assert!(!r.includes_last);
        assert_eq!(r.changes.len(), 3);
        match &r.changes[0] {
            SyncChange::Create(item) => {
                assert_eq!(item.item_id, "NEW-1");
                assert_eq!(item.change_key.as_deref(), Some("CK-1"));
                assert_eq!(item.subject, "Brand new");
                assert_eq!(item.item_type.as_deref(), Some("Single"));
                // AppointmentState is parsed off the SyncFolderItems shape
                // (the asfCanceled 0x4 bit drives cancelled detection).
                assert!(!item.cancelled);
                assert_eq!(item.appointment_state, Some(5));
            }
            other => panic!("expected Create, got {other:?}"),
        }
        match &r.changes[1] {
            SyncChange::Update(item) => {
                assert_eq!(item.item_id, "UPD-1");
                assert_eq!(item.subject, "Edited");
                assert!(item.is_recurring);
                assert_eq!(item.item_type.as_deref(), Some("RecurringMaster"));
            }
            other => panic!("expected Update, got {other:?}"),
        }
        match &r.changes[2] {
            SyncChange::Delete(id) => assert_eq!(id, "DEL-1"),
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn parse_sync_response_includes_last_true_signals_end_of_pagination() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:SyncFolderItemsResponse>
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:SyncState>FINAL-COOKIE</m:SyncState>
          <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
          <m:Changes/>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </soap:Body>
</soap:Envelope>"#;
        let r = parse_sync_folder_items_response(xml).unwrap();
        assert_eq!(r.new_sync_state, "FINAL-COOKIE");
        assert!(r.includes_last);
        assert!(r.changes.is_empty());
    }

    #[test]
    fn parse_sync_response_rejects_missing_sync_state() {
        // A response without `<m:SyncState>` is malformed — the
        // caller would otherwise persist an empty cookie and
        // accidentally restart the sync from scratch.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:SyncFolderItemsResponse>
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
          <m:Changes/>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </soap:Body>
</soap:Envelope>"#;
        let err = parse_sync_folder_items_response(xml).unwrap_err();
        assert!(matches!(err, EwsError::Protocol(_)));
    }

    // ── SOAP envelope (sync_folder_items) ────────────────────────

    #[test]
    fn sync_folder_items_envelope_includes_recurrence_fields_and_sync_state() {
        let body = crate::soap::sync_folder_items(
            "FOLDER-ID",
            Some("FOLDER-CK"),
            Some("PRIOR-COOKIE"),
            512,
        );
        // SyncFolderItems wrapper + folder id + change key.
        assert!(body.contains("<m:SyncFolderItems>"));
        assert!(body.contains(r#"<t:FolderId Id="FOLDER-ID" ChangeKey="FOLDER-CK"/>"#));
        // Prior state cookie is echoed back so the server replies
        // with deltas only.
        assert!(body.contains("<m:SyncState>PRIOR-COOKIE</m:SyncState>"));
        // MaxChangesReturned matches what we asked for.
        assert!(body.contains("<m:MaxChangesReturned>512</m:MaxChangesReturned>"));
        // Recurrence + exception field URIs requested — otherwise
        // EWS would drop them from the default shape.
        assert!(body.contains(r#"FieldURI="calendar:Recurrence""#));
        assert!(body.contains(r#"FieldURI="calendar:ModifiedOccurrences""#));
        assert!(body.contains(r#"FieldURI="calendar:DeletedOccurrences""#));
    }

    #[test]
    fn parse_sync_response_captures_recurrence_and_deleted_occurrences() {
        // A master row carrying a `<t:Recurrence>` block PLUS a
        // `<t:DeletedOccurrences>` list — the two pieces of
        // metadata the read path needs to render the series
        // correctly. The walker has to handle them inline (no
        // collision with the master's own `<t:Start>`).
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:SyncFolderItemsResponse>
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:SyncState>NEW-STATE</m:SyncState>
          <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
          <m:Changes>
            <t:Create>
              <t:CalendarItem>
                <t:ItemId Id="MASTER-1" ChangeKey="CK-A"/>
                <t:Subject>Wöchentliches Standup</t:Subject>
                <t:Start>2026-05-04T09:00:00Z</t:Start>
                <t:End>2026-05-04T09:30:00Z</t:End>
                <t:IsAllDayEvent>false</t:IsAllDayEvent>
                <t:IsRecurring>true</t:IsRecurring>
                <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
                <t:Recurrence>
                  <t:WeeklyRecurrence>
                    <t:Interval>1</t:Interval>
                    <t:DaysOfWeek>Monday</t:DaysOfWeek>
                  </t:WeeklyRecurrence>
                  <t:NoEndRecurrence>
                    <t:StartDate>2026-05-04</t:StartDate>
                  </t:NoEndRecurrence>
                </t:Recurrence>
                <t:DeletedOccurrences>
                  <t:DeletedOccurrence>
                    <t:Start>2026-05-18T09:00:00Z</t:Start>
                  </t:DeletedOccurrence>
                  <t:DeletedOccurrence>
                    <t:Start>2026-06-15T09:00:00Z</t:Start>
                  </t:DeletedOccurrence>
                </t:DeletedOccurrences>
              </t:CalendarItem>
            </t:Create>
          </m:Changes>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </soap:Body>
</soap:Envelope>"#;
        let r = parse_sync_folder_items_response(xml).unwrap();
        assert_eq!(r.changes.len(), 1);
        let item = match &r.changes[0] {
            SyncChange::Create(i) => i,
            other => panic!("expected Create, got {other:?}"),
        };
        // Master fields survived the recurrence subtree walk —
        // critical guard: the inner `<t:Start>` in DeletedOccurrence
        // must NOT overwrite the master's own start.
        assert_eq!(item.item_id, "MASTER-1");
        assert_eq!(item.subject, "Wöchentliches Standup");
        assert_eq!(
            item.start.unwrap().to_rfc3339(),
            "2026-05-04T09:00:00+00:00",
        );
        // Recurrence assembled.
        let rec = item.recurrence.as_ref().expect("recurrence parsed");
        assert_eq!(
            rec.pattern,
            EwsRecurrencePattern::Weekly {
                interval: 1,
                days_of_week: vec![EwsDay::Monday],
                first_day_of_week: EwsDay::Monday,
            },
        );
        assert_eq!(rec.range, EwsRecurrenceRange::NoEnd);
        // Deleted occurrences captured as datetimes — the master's
        // own start (May 4) is NOT in this list.
        assert_eq!(item.deleted_occurrence_starts.len(), 2);
        assert_eq!(
            item.deleted_occurrence_starts[0].to_rfc3339(),
            "2026-05-18T09:00:00+00:00",
        );
        assert_eq!(
            item.deleted_occurrence_starts[1].to_rfc3339(),
            "2026-06-15T09:00:00+00:00",
        );
    }

    #[test]
    fn parse_sync_response_captures_start_time_zone() {
        // A recurring master carries its WINDOWS zone in <t:StartTimeZone>; the
        // walker must capture it (and NOT confuse it with EndTimeZone) so the
        // mapper can translate it to IANA for DST-correct expansion.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:SyncFolderItemsResponse>
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:SyncState>NEW-STATE</m:SyncState>
          <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
          <m:Changes>
            <t:Create>
              <t:CalendarItem>
                <t:ItemId Id="MASTER-TZ" ChangeKey="CK"/>
                <t:Subject>OAGDU</t:Subject>
                <t:Start>2025-12-15T00:00:00Z</t:Start>
                <t:End>2025-12-15T01:00:00Z</t:End>
                <t:IsRecurring>true</t:IsRecurring>
                <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
                <t:StartTimeZone Id="Eastern Standard Time" Name="Eastern Standard Time"/>
                <t:EndTimeZone Id="Eastern Standard Time"/>
                <t:Recurrence>
                  <t:WeeklyRecurrence>
                    <t:Interval>1</t:Interval>
                    <t:DaysOfWeek>Sunday</t:DaysOfWeek>
                  </t:WeeklyRecurrence>
                  <t:NoEndRecurrence>
                    <t:StartDate>2025-12-15</t:StartDate>
                  </t:NoEndRecurrence>
                </t:Recurrence>
              </t:CalendarItem>
            </t:Create>
          </m:Changes>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </soap:Body>
</soap:Envelope>"#;
        let r = parse_sync_folder_items_response(xml).unwrap();
        let item = match &r.changes[0] {
            SyncChange::Create(i) => i,
            other => panic!("expected Create, got {other:?}"),
        };
        // Captured from StartTimeZone (the master's own Start also survived).
        assert_eq!(
            item.start_time_zone.as_deref(),
            Some("Eastern Standard Time")
        );
        // The end zone is captured on its own: it tells a series created
        // without a zone (`tzone://Microsoft/Utc`) from a Greenwich one.
        assert_eq!(item.end_time_zone.as_deref(), Some("Eastern Standard Time"));
        assert_eq!(
            item.start.unwrap().to_rfc3339(),
            "2025-12-15T00:00:00+00:00"
        );
        // …and it reads as tzdata's zone for the frontend expander.
        assert_eq!(
            crate::windows_tz::read_windows_zone(item.start_time_zone.as_deref().unwrap()),
            crate::windows_tz::WindowsZoneRead::Zone("America/New_York")
        );
    }

    /// An exception read on its own carries the slot it fills as its own
    /// `OriginalStart`; a master's `OriginalStart`s belong to its modified
    /// occurrences and must not leak onto the master.
    /// Decision 58a: the GetItem parser knew neither the location, nor the
    /// reminder, nor the all-day flag, nor the timestamps — the shape never
    /// asked for them, so an occurrence read through it could only inherit the
    /// series'. Both are fixed; this is the parser half.
    #[test]
    fn get_item_reads_what_an_occurrence_owns() {
        let dt = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="EXC" ChangeKey="ECK"/>
        <t:Subject>Nur dieser Termin</t:Subject>
        <t:Body BodyType="Text">Agenda der Ausnahme</t:Body>
        <t:DateTimeCreated>2026-07-01T09:00:00Z</t:DateTimeCreated>
        <t:LastModifiedTime>2026-08-19T08:00:00Z</t:LastModifiedTime>
        <t:ReminderIsSet>true</t:ReminderIsSet>
        <t:ReminderMinutesBeforeStart>5</t:ReminderMinutesBeforeStart>
        <t:Location>Raum 2</t:Location>
        <t:Start>2026-08-20T15:00:00Z</t:Start>
        <t:End>2026-08-20T15:30:00Z</t:End>
        <t:IsAllDayEvent>false</t:IsAllDayEvent>
        <t:CalendarItemType>Exception</t:CalendarItemType>
        <t:OriginalStart>2026-08-20T12:00:00Z</t:OriginalStart>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let items = parse_get_calendar_items_response(xml).expect("parses");
        let exc = items.iter().find(|i| i.item_id == "EXC").expect("the item");
        assert_eq!(exc.location.as_deref(), Some("Raum 2"));
        assert!(exc.reminder_is_set);
        assert_eq!(exc.reminder_minutes_before_start, Some(5));
        assert!(!exc.is_all_day);
        assert_eq!(exc.created, Some(dt("2026-07-01T09:00:00Z")));
        assert_eq!(exc.last_modified, Some(dt("2026-08-19T08:00:00Z")));
        assert_eq!(exc.subject, "Nur dieser Termin");
        assert_eq!(exc.body.as_deref(), Some("Agenda der Ausnahme"));
    }

    /// The same names occur INSIDE `<t:ModifiedOccurrences>`, where they
    /// describe the slot rather than the master. A master's own location must
    /// not be overwritten by a nested one.
    #[test]
    fn a_masters_own_fields_survive_its_occurrence_list() {
        let dt = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="MASTER" ChangeKey="MCK"/>
        <t:Subject>Serie</t:Subject>
        <t:Location>Raum 1</t:Location>
        <t:IsAllDayEvent>false</t:IsAllDayEvent>
        <t:ReminderIsSet>true</t:ReminderIsSet>
        <t:ReminderMinutesBeforeStart>60</t:ReminderMinutesBeforeStart>
        <t:Start>2026-07-06T09:00:00Z</t:Start>
        <t:End>2026-07-06T10:00:00Z</t:End>
        <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
        <t:ModifiedOccurrences><t:Occurrence>
          <t:ItemId Id="EXC" ChangeKey="ECK"/>
          <t:Start>2026-07-23T15:00:00Z</t:Start>
          <t:End>2026-07-23T16:00:00Z</t:End>
          <t:OriginalStart>2026-07-20T09:00:00Z</t:OriginalStart>
        </t:Occurrence></t:ModifiedOccurrences>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let items = parse_get_calendar_items_response(xml).expect("parses");
        let master = items.first().expect("the master");
        assert_eq!(master.location.as_deref(), Some("Raum 1"));
        assert_eq!(master.reminder_minutes_before_start, Some(60));
        assert_eq!(master.modified_occurrences.len(), 1);
        assert_eq!(
            master.modified_occurrences[0].start,
            dt("2026-07-23T15:00:00Z")
        );
        // The list carries no item of its own until the enrichment reads one.
        assert!(master.modified_occurrences[0].own.is_none());
    }

    #[test]
    fn get_item_reads_an_exceptions_own_original_start() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="EXC" ChangeKey="ECK"/>
        <t:Start>2026-07-23T15:00:00Z</t:Start>
        <t:End>2026-07-23T16:00:00Z</t:End>
        <t:CalendarItemType>Exception</t:CalendarItemType>
        <t:OriginalStart>2026-07-20T09:00:00Z</t:OriginalStart>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="MASTER" ChangeKey="MCK"/>
        <t:Start>2026-07-06T09:00:00Z</t:Start>
        <t:End>2026-07-06T10:00:00Z</t:End>
        <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
        <t:ModifiedOccurrences><t:Occurrence>
          <t:ItemId Id="EXC" ChangeKey="ECK"/>
          <t:Start>2026-07-23T15:00:00Z</t:Start>
          <t:End>2026-07-23T16:00:00Z</t:End>
          <t:OriginalStart>2026-07-20T09:00:00Z</t:OriginalStart>
        </t:Occurrence></t:ModifiedOccurrences>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let items = parse_get_calendar_items_response(xml).unwrap();
        assert_eq!(items.len(), 2);
        let slot: DateTime<Utc> = "2026-07-20T09:00:00Z".parse().unwrap();
        assert_eq!(items[0].original_start, Some(slot));
        assert_eq!(
            items[0].start,
            Some("2026-07-23T15:00:00Z".parse().unwrap())
        );
        assert_eq!(
            items[1].original_start, None,
            "the master has no slot of its own"
        );
        assert_eq!(items[1].modified_occurrences[0].original_start, slot);
    }

    /// 43b through `to_event`: the master's zone comes from both zone fields.
    /// A series Exchange stored without a zone (Greenwich start, the
    /// `tzone://Microsoft/Utc` end) repeats in UTC; the same start with a
    /// Greenwich end is an Abidjan series.
    #[test]
    fn to_event_reads_the_series_zone_from_start_and_end_zone() {
        let master = |end: &str| ParsedItem {
            item_id: "M".into(),
            change_key: Some("CK".into()),
            subject: "Weekly".into(),
            start: Some("2026-05-20T08:00:00Z".parse().unwrap()),
            end: Some("2026-05-20T09:00:00Z".parse().unwrap()),
            is_recurring: true,
            item_type: Some("RecurringMaster".into()),
            start_time_zone: Some("Greenwich Standard Time".into()),
            end_time_zone: Some(end.into()),
            recurrence: Some(EwsRecurrence {
                pattern: EwsRecurrencePattern::Daily { interval: 7 },
                range: EwsRecurrenceRange::NoEnd,
            }),
            ..ParsedItem::default()
        };
        let tzid = |end: &str| {
            to_event(master(end), "FID|CK")
                .unwrap()
                .recurrence
                .expect("a master keeps its recurrence")
                .tzid
        };
        assert_eq!(tzid("tzone://Microsoft/Utc"), None);
        assert_eq!(
            tzid("Greenwich Standard Time").as_deref(),
            Some("Africa/Abidjan")
        );
    }

    #[test]
    fn parse_sync_response_captures_modified_occurrences() {
        // A master with one moved instance. Critical regression
        // guards:
        //   - the override's nested ItemId does NOT overwrite the
        //     master's ItemId (would silently retarget every
        //     subsequent push at the wrong row);
        //   - the override's nested Start/End/OriginalStart land
        //     in the right collection, NOT in the master's own
        //     start/end fields.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:SyncFolderItemsResponse>
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:SyncState>STATE</m:SyncState>
          <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
          <m:Changes>
            <t:Create>
              <t:CalendarItem>
                <t:ItemId Id="MASTER-X" ChangeKey="CK-MX"/>
                <t:Subject>Daily standup</t:Subject>
                <t:Start>2026-06-01T09:00:00Z</t:Start>
                <t:End>2026-06-01T09:30:00Z</t:End>
                <t:IsRecurring>true</t:IsRecurring>
                <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
                <t:Recurrence>
                  <t:DailyRecurrence><t:Interval>1</t:Interval></t:DailyRecurrence>
                  <t:NumberedRecurrence>
                    <t:StartDate>2026-06-01</t:StartDate>
                    <t:NumberOfOccurrences>10</t:NumberOfOccurrences>
                  </t:NumberedRecurrence>
                </t:Recurrence>
                <t:ModifiedOccurrences>
                  <t:Occurrence>
                    <t:ItemId Id="OCC-MOVED" ChangeKey="CK-OM"/>
                    <t:Start>2026-06-03T14:00:00Z</t:Start>
                    <t:End>2026-06-03T14:30:00Z</t:End>
                    <t:OriginalStart>2026-06-03T09:00:00Z</t:OriginalStart>
                  </t:Occurrence>
                </t:ModifiedOccurrences>
              </t:CalendarItem>
            </t:Create>
          </m:Changes>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </soap:Body>
</soap:Envelope>"#;
        let r = parse_sync_folder_items_response(xml).unwrap();
        let item = match &r.changes[0] {
            SyncChange::Create(i) => i,
            other => panic!("expected Create, got {other:?}"),
        };
        // Master fields preserved.
        assert_eq!(item.item_id, "MASTER-X");
        assert_eq!(item.change_key.as_deref(), Some("CK-MX"));
        assert_eq!(
            item.start.unwrap().to_rfc3339(),
            "2026-06-01T09:00:00+00:00",
        );
        // One override captured with correct fields.
        assert_eq!(item.modified_occurrences.len(), 1);
        let ov = &item.modified_occurrences[0];
        assert_eq!(ov.item_id, "OCC-MOVED");
        assert_eq!(ov.change_key.as_deref(), Some("CK-OM"));
        assert_eq!(ov.start.to_rfc3339(), "2026-06-03T14:00:00+00:00");
        assert_eq!(ov.original_start.to_rfc3339(), "2026-06-03T09:00:00+00:00");
    }

    #[test]
    fn to_event_folds_override_original_start_into_exdate_list() {
        // Modified occurrences displace the RRULE slot at their
        // OriginalStart — the master's EXDATE list must include
        // that slot so the frontend expander doesn't render two
        // events (the wrongly-placed master-expanded one + the
        // override). The deleted-occurrence list survives the
        // merge intact.
        let mut item = ParsedItem {
            item_id: "M".into(),
            subject: "X".into(),
            start: Some("2026-01-01T08:00:00Z".parse().unwrap()),
            end: Some("2026-01-01T08:30:00Z".parse().unwrap()),
            is_recurring: true,
            item_type: Some("RecurringMaster".into()),
            ..ParsedItem::default()
        };
        item.recurrence = Some(EwsRecurrence {
            pattern: EwsRecurrencePattern::Daily { interval: 1 },
            range: EwsRecurrenceRange::Numbered { occurrences: 30 },
        });
        item.deleted_occurrence_starts = vec!["2026-01-05T08:00:00Z".parse().unwrap()];
        item.modified_occurrences = vec![ModifiedOccurrence {
            item_id: "OCC".into(),
            change_key: None,
            start: "2026-01-10T15:00:00Z".parse().unwrap(),
            end: "2026-01-10T15:30:00Z".parse().unwrap(),
            original_start: "2026-01-10T08:00:00Z".parse().unwrap(),
            cancelled: false,
            own: None,
        }];

        let ev = to_event(item, "cal").unwrap();
        let rec = ev.recurrence.expect("master has recurrence");
        // Both the deleted slot AND the displaced slot land in
        // exceptions; the deleted-only one keeps its place.
        assert_eq!(rec.exceptions.len(), 2);
        assert_eq!(rec.exceptions[0].to_rfc3339(), "2026-01-05T08:00:00+00:00");
        assert_eq!(rec.exceptions[1].to_rfc3339(), "2026-01-10T08:00:00+00:00");
    }

    #[test]
    fn to_event_anchors_allday_exdates_like_the_master_start() {
        // For an all-day master, `start` is re-anchored to LOCAL midnight; the
        // EXDATEs (deleted + displaced slots) must be anchored the SAME way, or
        // the frontend expander — which anchors on `start` and matches EXDATEs by
        // exact instant — won't suppress the vacated slot on a non-UTC device,
        // rendering it as a duplicate. Zone-generic: asserts the exceptions equal
        // `all_day_local_anchor` of the raw instants (identity under UTC, shifted
        // under any other zone), matching whatever transform hit `start`.
        let del: DateTime<Utc> = "2026-01-05T00:00:00Z".parse().unwrap();
        let orig: DateTime<Utc> = "2026-01-10T00:00:00Z".parse().unwrap();
        let mut item = ParsedItem {
            item_id: "M".into(),
            subject: "All-day standup".into(),
            start: Some("2026-01-01T00:00:00Z".parse().unwrap()),
            end: Some("2026-01-02T00:00:00Z".parse().unwrap()),
            is_all_day: true,
            is_recurring: true,
            item_type: Some("RecurringMaster".into()),
            ..ParsedItem::default()
        };
        item.recurrence = Some(EwsRecurrence {
            pattern: EwsRecurrencePattern::Daily { interval: 1 },
            range: EwsRecurrenceRange::Numbered { occurrences: 30 },
        });
        item.deleted_occurrence_starts = vec![del];
        item.modified_occurrences = vec![ModifiedOccurrence {
            item_id: "OCC".into(),
            change_key: None,
            start: "2026-01-10T00:00:00Z".parse().unwrap(),
            end: "2026-01-11T00:00:00Z".parse().unwrap(),
            original_start: orig,
            cancelled: false,
            own: None,
        }];

        let ev = to_event(item, "cal").unwrap();
        let rec = ev.recurrence.expect("master has recurrence");
        assert_eq!(rec.exceptions.len(), 2);
        assert_eq!(rec.exceptions[0], all_day_local_anchor(del));
        assert_eq!(rec.exceptions[1], all_day_local_anchor(orig));
        // The master start got the same transform, so grid + EXDATEs line up.
        assert_eq!(
            ev.start,
            all_day_local_anchor("2026-01-01T00:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn nominal_occurrence_index_maps_dates_to_ews_instance_index() {
        let dt = |s: &str| s.parse::<DateTime<Utc>>().unwrap();

        // Weekly Monday, no end, anchored 2026-07-06 → 07-06(1), 07-13(2),
        // 07-20(3), 07-27(4).
        let weekly = EwsRecurrence {
            pattern: EwsRecurrencePattern::Weekly {
                interval: 1,
                days_of_week: vec![EwsDay::Monday],
                first_day_of_week: EwsDay::Monday,
            },
            range: EwsRecurrenceRange::NoEnd,
        };
        let start = dt("2026-07-06T09:00:00Z");
        assert_eq!(
            nominal_occurrence_index(&weekly, start, dt("2026-07-06T09:00:00Z")),
            Some(1)
        );
        assert_eq!(
            nominal_occurrence_index(&weekly, start, dt("2026-07-20T09:00:00Z")),
            Some(3)
        );

        // Toni's actual series: biweekly Thursday, anchored 2026-07-23 →
        // 07-23(1), 08-06(2), 08-20(3), 09-03(4). The InstanceIndex is the
        // NOMINAL position — even if 08-06 were already deleted, 08-20 stays 3.
        let biweekly = EwsRecurrence {
            pattern: EwsRecurrencePattern::Weekly {
                interval: 2,
                days_of_week: vec![EwsDay::Thursday],
                first_day_of_week: EwsDay::Monday,
            },
            range: EwsRecurrenceRange::NoEnd,
        };
        let ts = dt("2026-07-23T12:00:00Z");
        assert_eq!(
            nominal_occurrence_index(&biweekly, ts, dt("2026-08-06T12:00:00Z")),
            Some(2)
        );
        assert_eq!(
            nominal_occurrence_index(&biweekly, ts, dt("2026-09-03T12:00:00Z")),
            Some(4)
        );

        // Daily, interval 1, anchored 2026-07-01 → the Nth day is index N.
        let daily = EwsRecurrence {
            pattern: EwsRecurrencePattern::Daily { interval: 1 },
            range: EwsRecurrenceRange::NoEnd,
        };
        let ds = dt("2026-07-01T08:00:00Z");
        assert_eq!(
            nominal_occurrence_index(&daily, ds, dt("2026-07-10T08:00:00Z")),
            Some(10)
        );
    }

    #[test]
    fn to_event_translates_master_with_recurrence_into_rrule() {
        // End-to-end: ParsedItem with recurrence + EXDATEs
        // round-trips through `to_event` into a cal-core Event
        // whose `recurrence` field carries the RRULE the frontend
        // expander expects.
        let mut item = ParsedItem {
            item_id: "MASTER-2".into(),
            change_key: Some("CK-B".into()),
            subject: "Daily standup".into(),
            start: Some("2026-01-01T08:00:00Z".parse().unwrap()),
            end: Some("2026-01-01T08:15:00Z".parse().unwrap()),
            is_recurring: true,
            item_type: Some("RecurringMaster".into()),
            ..ParsedItem::default()
        };
        item.recurrence = Some(EwsRecurrence {
            pattern: EwsRecurrencePattern::Daily { interval: 1 },
            range: EwsRecurrenceRange::Numbered { occurrences: 20 },
        });
        item.deleted_occurrence_starts = vec!["2026-01-05T08:00:00Z".parse().unwrap()];

        let ev = to_event(item, "cal-id").unwrap();
        let rec = ev.recurrence.expect("event carries recurrence");
        assert_rrule_equivalent(&rec.rrule, "FREQ=DAILY;COUNT=20");
        assert_eq!(rec.exceptions.len(), 1);
    }

    #[test]
    fn sync_folder_items_envelope_omits_sync_state_on_initial_sync() {
        let body = crate::soap::sync_folder_items("FOLDER-ID", None, None, 100);
        // Initial sync has no prior cookie — the SyncState
        // element MUST be absent (sending an empty one makes EWS
        // think the cookie is invalid).
        assert!(!body.contains("<m:SyncState>"));
        // FolderId without ChangeKey.
        assert!(body.contains(r#"<t:FolderId Id="FOLDER-ID"/>"#));
    }

    #[test]
    fn parse_get_items_response_extracts_recurrence_and_overrides() {
        // Shape of a real `GetItemResponse` body that the recurrence
        // enrichment fan-out parses: two CalendarItem rows side by
        // side, each a RecurringMaster with its own recurrence shape
        // — one weekly with a deleted occurrence, one daily-numbered
        // with a modified occurrence. The parser must surface both
        // rows independently with their full recurrence + overrides.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:GetItemResponse>
      <m:ResponseMessages>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="MASTER-W" ChangeKey="CK-W"/>
              <t:Subject>Weekly</t:Subject>
              <t:Start>2026-05-04T09:00:00Z</t:Start>
              <t:End>2026-05-04T09:30:00Z</t:End>
              <t:IsRecurring>true</t:IsRecurring>
              <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
              <t:Recurrence>
                <t:WeeklyRecurrence>
                  <t:Interval>1</t:Interval>
                  <t:DaysOfWeek>Monday</t:DaysOfWeek>
                </t:WeeklyRecurrence>
                <t:NoEndRecurrence>
                  <t:StartDate>2026-05-04</t:StartDate>
                </t:NoEndRecurrence>
              </t:Recurrence>
              <t:DeletedOccurrences>
                <t:DeletedOccurrence>
                  <t:Start>2026-05-18T09:00:00Z</t:Start>
                </t:DeletedOccurrence>
              </t:DeletedOccurrences>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="MASTER-D" ChangeKey="CK-D"/>
              <t:Subject>Daily</t:Subject>
              <t:Start>2026-06-01T09:00:00Z</t:Start>
              <t:End>2026-06-01T09:30:00Z</t:End>
              <t:IsRecurring>true</t:IsRecurring>
              <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
              <t:Recurrence>
                <t:DailyRecurrence><t:Interval>1</t:Interval></t:DailyRecurrence>
                <t:NumberedRecurrence>
                  <t:StartDate>2026-06-01</t:StartDate>
                  <t:NumberOfOccurrences>10</t:NumberOfOccurrences>
                </t:NumberedRecurrence>
              </t:Recurrence>
              <t:ModifiedOccurrences>
                <t:Occurrence>
                  <t:ItemId Id="OCC-MOVED" ChangeKey="CK-OM"/>
                  <t:Start>2026-06-03T14:00:00Z</t:Start>
                  <t:End>2026-06-03T14:30:00Z</t:End>
                  <t:OriginalStart>2026-06-03T09:00:00Z</t:OriginalStart>
                </t:Occurrence>
              </t:ModifiedOccurrences>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
      </m:ResponseMessages>
    </m:GetItemResponse>
  </soap:Body>
</soap:Envelope>"#;
        let parsed = parse_get_calendar_items_response(xml).unwrap();
        assert_eq!(parsed.len(), 2);

        // First master: weekly + one EXDATE.
        let weekly = parsed.iter().find(|i| i.item_id == "MASTER-W").unwrap();
        assert_eq!(weekly.change_key.as_deref(), Some("CK-W"));
        let rec = weekly.recurrence.as_ref().expect("weekly recurrence");
        assert_eq!(
            rec.pattern,
            EwsRecurrencePattern::Weekly {
                interval: 1,
                days_of_week: vec![EwsDay::Monday],
                first_day_of_week: EwsDay::Monday,
            },
        );
        assert_eq!(weekly.deleted_occurrence_starts.len(), 1);
        assert_eq!(
            weekly.deleted_occurrence_starts[0].to_rfc3339(),
            "2026-05-18T09:00:00+00:00",
        );

        // Second master: daily-numbered + one moved override. The
        // override's nested ItemId must NOT have overwritten the
        // master's id (same regression guard as the SyncFolderItems
        // parser).
        let daily = parsed.iter().find(|i| i.item_id == "MASTER-D").unwrap();
        assert_eq!(daily.change_key.as_deref(), Some("CK-D"));
        assert!(matches!(
            daily.recurrence.as_ref().unwrap().range,
            EwsRecurrenceRange::Numbered { occurrences: 10 },
        ));
        assert_eq!(daily.modified_occurrences.len(), 1);
        let ov = &daily.modified_occurrences[0];
        assert_eq!(ov.item_id, "OCC-MOVED");
        assert_eq!(ov.start.to_rfc3339(), "2026-06-03T14:00:00+00:00");
        assert_eq!(ov.original_start.to_rfc3339(), "2026-06-03T09:00:00+00:00");
    }

    #[test]
    fn parse_get_items_response_reads_cancelled_exception() {
        // The occurrence-exception fan-out GetItems the exception item behind a
        // ModifiedOccurrence. When the organizer cancelled just that instance,
        // the exception carries IsCancelled=true (and/or the asfCanceled bit).
        // The GetItem parser must surface those so `resolve_cancelled` flags it.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:GetItemResponse>
      <m:ResponseMessages>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="OCC-CANCELLED" ChangeKey="CK-X"/>
              <t:Subject>Austausch Frank - Toni</t:Subject>
              <t:Start>2026-08-06T12:00:00Z</t:Start>
              <t:End>2026-08-06T12:30:00Z</t:End>
              <t:IsCancelled>true</t:IsCancelled>
              <t:AppointmentState>7</t:AppointmentState>
              <t:CalendarItemType>Exception</t:CalendarItemType>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
      </m:ResponseMessages>
    </m:GetItemResponse>
  </soap:Body>
</soap:Envelope>"#;
        let parsed = parse_get_calendar_items_response(xml).unwrap();
        assert_eq!(parsed.len(), 1);
        let exc = &parsed[0];
        assert_eq!(exc.item_id, "OCC-CANCELLED");
        assert!(exc.cancelled);
        assert_eq!(exc.appointment_state, Some(7));
        assert!(resolve_cancelled(exc));
    }

    #[test]
    fn parse_get_items_response_tolerates_per_item_error() {
        // The occurrence-exception fan-out POSTs via post_soap_raw (no fault
        // check), so a batch can come back with a per-item ResponseClass="Error"
        // (a deleted/inaccessible exception) alongside the successes. The parser
        // must return the successful row and simply omit the errored id — never
        // Err — so one bad exception can't poison cancelled-state for the rest.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:GetItemResponse>
      <m:ResponseMessages>
        <m:GetItemResponseMessage ResponseClass="Error">
          <m:MessageText>The specified object was not found in the store.</m:MessageText>
          <m:ResponseCode>ErrorItemNotFound</m:ResponseCode>
          <m:DescriptiveLinkKey>0</m:DescriptiveLinkKey>
          <m:Items/>
        </m:GetItemResponseMessage>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="OCC-OK" ChangeKey="CK-OK"/>
              <t:Subject>Austausch Frank - Toni</t:Subject>
              <t:Start>2026-08-20T12:00:00Z</t:Start>
              <t:End>2026-08-20T12:30:00Z</t:End>
              <t:IsCancelled>true</t:IsCancelled>
              <t:CalendarItemType>Exception</t:CalendarItemType>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
      </m:ResponseMessages>
    </m:GetItemResponse>
  </soap:Body>
</soap:Envelope>"#;
        let parsed = parse_get_calendar_items_response(xml).unwrap();
        assert_eq!(parsed.len(), 1, "only the successful row survives");
        assert_eq!(parsed[0].item_id, "OCC-OK");
        assert!(parsed[0].cancelled);
    }

    #[test]
    fn parse_get_items_response_reads_organizer_and_attendees() {
        let xml = r#"<?xml version="1.0"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <soap:Body>
    <m:GetItemResponse>
      <m:ResponseMessages>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="MTG-1" ChangeKey="CK"/>
              <t:Subject>Planning</t:Subject>
              <t:Organizer>
                <t:Mailbox>
                  <t:Name>The Boss</t:Name>
                  <t:EmailAddress>boss@example.com</t:EmailAddress>
                </t:Mailbox>
              </t:Organizer>
              <t:MyResponseType>Tentative</t:MyResponseType>
              <t:RequiredAttendees>
                <t:Attendee>
                  <t:Mailbox>
                    <t:Name>The Boss</t:Name>
                    <t:EmailAddress>boss@example.com</t:EmailAddress>
                  </t:Mailbox>
                  <t:ResponseType>Organizer</t:ResponseType>
                </t:Attendee>
                <t:Attendee>
                  <t:Mailbox>
                    <t:Name>Me</t:Name>
                    <t:EmailAddress>me@example.com</t:EmailAddress>
                  </t:Mailbox>
                  <t:ResponseType>Tentative</t:ResponseType>
                </t:Attendee>
              </t:RequiredAttendees>
              <t:OptionalAttendees>
                <t:Attendee>
                  <t:Mailbox>
                    <t:EmailAddress>maybe@example.com</t:EmailAddress>
                  </t:Mailbox>
                  <t:ResponseType>Decline</t:ResponseType>
                </t:Attendee>
              </t:OptionalAttendees>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
      </m:ResponseMessages>
    </m:GetItemResponse>
  </soap:Body>
</soap:Envelope>"#;
        let parsed = parse_get_calendar_items_response(xml).unwrap();
        assert_eq!(parsed.len(), 1);
        let item = &parsed[0];
        assert_eq!(item.organizer.as_deref(), Some("boss@example.com"));
        // Required + optional attendees collected in document order.
        assert_eq!(item.attendees.len(), 3);
        assert_eq!(item.attendees[0].email, "boss@example.com");
        assert_eq!(item.attendees[0].name.as_deref(), Some("The Boss"));
        assert_eq!(
            item.attendees[0].response_type.as_deref(),
            Some("Organizer")
        );
        assert_eq!(item.attendees[1].email, "me@example.com");
        assert_eq!(
            item.attendees[1].response_type.as_deref(),
            Some("Tentative")
        );
        assert_eq!(item.attendees[2].email, "maybe@example.com");
        assert_eq!(item.attendees[2].response_type.as_deref(), Some("Decline"));

        assert_eq!(item.my_response_type.as_deref(), Some("Tentative"));

        // The cal-core mapping normalises the response types and leaves the
        // organizer out of the invitees (decision 67a). The mailbox answered
        // Tentative, so someone else organizes this meeting (decision 70a).
        let mut full = item.clone();
        full.start = Some("2026-05-25T10:00:00Z".parse().unwrap());
        full.end = Some("2026-05-25T11:00:00Z".parse().unwrap());
        let ev = to_event(full, "cal-1").unwrap();
        assert_eq!(ev.organizer.as_deref(), Some("boss@example.com"));
        assert_eq!(ev.attendees, ["Me <me@example.com>", "maybe@example.com"]);
        assert_eq!(ev.attendee_responses[0].status, AttendeeStatus::Tentative);
        assert_eq!(ev.attendee_responses[1].status, AttendeeStatus::Declined);
        assert_eq!(ev.attendee_responses.len(), 2);
        assert!(ev.organized_elsewhere);
    }

    /// An appointment made in Outlook: Exchange lists its organizer, the
    /// mailbox itself, as the only attendee (live round 4, D6-D8). It has no
    /// invitees, so an update writes no attendee list and a detach creates a
    /// plain appointment, not a meeting.
    #[test]
    fn an_outlook_appointment_has_no_invitees() {
        let mut item = ParsedItem {
            subject: "Aperio R4 Exchange geaendert".into(),
            start: Some("2026-10-06T07:00:00Z".parse().unwrap()),
            end: Some("2026-10-06T07:30:00Z".parse().unwrap()),
            organizer: Some("toni@example.com".into()),
            my_response_type: Some("Organizer".into()),
            ..ParsedItem::default()
        };
        item.item_id = "APPT-1".into();
        item.attendees = vec![EwsAttendee {
            email: "toni@example.com".into(),
            name: Some("Toni".into()),
            response_type: Some("Organizer".into()),
        }];
        let ev = to_event(item, "cal-1").unwrap();
        assert!(ev.attendees.is_empty(), "{:?}", ev.attendees);
        assert!(ev.attendee_responses.is_empty());
        assert!(!ev.organized_elsewhere, "the mailbox organizes it");

        let (set, _) = event_to_update_field_xml(&ev).unwrap();
        assert!(!set.contains("calendar:RequiredAttendees"), "{set}");
        let new = NewEvent {
            title: ev.title.clone(),
            description: None,
            location: None,
            start: ev.start,
            end: ev.end,
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: ev.attendees.clone(),
            send_invitations: false,
            organizer: None,
            organized_elsewhere: false,
        };
        let xml = new_event_to_calendar_item_xml(&new).unwrap();
        assert!(!xml.contains("<t:RequiredAttendees>"), "{xml}");
    }

    /// An edit that left the invitees alone writes no attendee list, so the
    /// server keeps its own, the organizer's row included (decision 71a).
    #[test]
    fn an_unchanged_attendee_list_is_not_written() {
        let mut item = ParsedItem {
            start: Some("2026-10-06T07:00:00Z".parse().unwrap()),
            end: Some("2026-10-06T07:30:00Z".parse().unwrap()),
            ..ParsedItem::default()
        };
        item.item_id = "MTG-3".into();
        item.attendees = vec![EwsAttendee {
            email: "bob@example.com".into(),
            name: None,
            response_type: Some("Accept".into()),
        }];
        let mut ev = to_event(item, "cal-1").unwrap();
        let (set, _) = event_to_update_field_xml(&ev).unwrap();
        assert!(set.contains("calendar:RequiredAttendees"), "{set}");
        ev.keep_attendees = true;
        let (set, _) = event_to_update_field_xml(&ev).unwrap();
        assert!(!set.contains("calendar:RequiredAttendees"), "{set}");
    }

    /// Removing every invitee clears both collections the read merges, but
    /// only when the host says so (decision 74a): an empty list alone
    /// deletes nothing.
    #[test]
    fn removing_the_last_invitee_deletes_the_list() {
        let mut item = ParsedItem {
            start: Some("2026-10-06T07:00:00Z".parse().unwrap()),
            end: Some("2026-10-06T07:30:00Z".parse().unwrap()),
            ..ParsedItem::default()
        };
        item.item_id = "MTG-5".into();
        let mut ev = to_event(item, "cal-1").unwrap();
        assert!(ev.attendees.is_empty());
        let (set, del) = event_to_update_field_xml(&ev).unwrap();
        assert!(!set.contains("Attendees"), "{set}");
        assert!(!del.contains("Attendees"), "{del}");

        ev.clear_attendees = true;
        let (set, del) = event_to_update_field_xml(&ev).unwrap();
        assert!(!set.contains("Attendees"), "{set}");
        assert!(del.contains("calendar:RequiredAttendees"), "{del}");
        assert!(del.contains("calendar:OptionalAttendees"), "{del}");
    }

    /// The organizer's row is found by its flag, whatever address Exchange
    /// names the organizer by: an Exchange-internal (EX) address does not
    /// match the row's SMTP address.
    #[test]
    fn the_organizer_row_is_found_by_its_flag() {
        let mut item = ParsedItem {
            start: Some("2026-10-06T07:00:00Z".parse().unwrap()),
            end: Some("2026-10-06T07:30:00Z".parse().unwrap()),
            organizer: Some(
                "/o=ExchangeLabs/ou=Exchange Administrative Group/cn=Recipients/cn=boss".into(),
            ),
            ..ParsedItem::default()
        };
        item.item_id = "MTG-2".into();
        item.attendees = vec![
            EwsAttendee {
                email: "boss@example.com".into(),
                name: Some("The Boss".into()),
                response_type: Some("Organizer".into()),
            },
            EwsAttendee {
                email: "bob@example.com".into(),
                name: None,
                response_type: Some("Accept".into()),
            },
        ];
        let ev = to_event(item, "cal-1").unwrap();
        assert_eq!(ev.attendees, ["bob@example.com"]);
        assert!(ev.organized_elsewhere, "no MyResponseType: not confirmed");
    }

    /// MyResponseType is the mailbox's own answer and counts even when the
    /// item names no organizer address. "Unknown" answers nothing.
    #[test]
    fn my_response_type_decides_without_an_organizer_address() {
        let item_answering = |answer: &str| {
            let mut item = ParsedItem {
                start: Some("2026-10-06T07:00:00Z".parse().unwrap()),
                end: Some("2026-10-06T07:30:00Z".parse().unwrap()),
                my_response_type: Some(answer.into()),
                ..ParsedItem::default()
            };
            item.item_id = "MTG-4".into();
            item.attendees = vec![EwsAttendee {
                email: "bob@example.com".into(),
                name: None,
                response_type: Some("Accept".into()),
            }];
            to_event(item, "cal-1").unwrap()
        };
        assert!(item_answering("Accept").organized_elsewhere);
        assert!(!item_answering("Organizer").organized_elsewhere);
        assert!(
            !item_answering("Unknown").organized_elsewhere,
            "no answer and no organizer: the mailbox's own"
        );
    }

    #[test]
    fn parse_get_items_response_keeps_other_rows_when_one_master_has_malformed_recurrence() {
        // Regression for the production bug where ONE master with
        // a broken recurrence poisoned the whole GetItem fan-out
        // and caused zero events from the EWS calendar to render
        // — including singles in the same drain (which depend on
        // a successful sync state commit). The walker must
        // tolerate the bad row, leave its recurrence empty, and
        // continue parsing the rest.
        //
        // After EWS-H landed Relative*Recurrence is fully
        // supported, so the original repro (RelativeMonthly +
        // Third Wednesday) now parses fine. To keep the resilience
        // invariant under test we use a different broken shape:
        // a RelativeMonthlyRecurrence with no DayOfWeekIndex —
        // server response is incomplete and the PatternBuilder's
        // finish() surfaces a Protocol error, which the parser
        // must swallow for this single row.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:GetItemResponse>
      <m:ResponseMessages>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="MASTER-BAD" ChangeKey="CK-B"/>
              <t:Subject>Broken master</t:Subject>
              <t:Start>2024-05-15T10:30:00Z</t:Start>
              <t:End>2024-05-15T12:00:00Z</t:End>
              <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
              <t:Recurrence>
                <t:RelativeMonthlyRecurrence>
                  <t:Interval>1</t:Interval>
                  <t:DaysOfWeek>Wednesday</t:DaysOfWeek>
                </t:RelativeMonthlyRecurrence>
                <t:NoEndRecurrence>
                  <t:StartDate>2024-05-15</t:StartDate>
                </t:NoEndRecurrence>
              </t:Recurrence>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="MASTER-OK" ChangeKey="CK-OK"/>
              <t:Subject>Weekly OK</t:Subject>
              <t:Start>2026-05-04T09:00:00Z</t:Start>
              <t:End>2026-05-04T09:30:00Z</t:End>
              <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
              <t:Recurrence>
                <t:WeeklyRecurrence>
                  <t:Interval>1</t:Interval>
                  <t:DaysOfWeek>Monday</t:DaysOfWeek>
                </t:WeeklyRecurrence>
                <t:NoEndRecurrence>
                  <t:StartDate>2026-05-04</t:StartDate>
                </t:NoEndRecurrence>
              </t:Recurrence>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
      </m:ResponseMessages>
    </m:GetItemResponse>
  </soap:Body>
</soap:Envelope>"#;
        // The whole batch must parse cleanly — no propagated Err.
        let parsed = parse_get_calendar_items_response(xml).unwrap();
        assert_eq!(parsed.len(), 2, "both rows must survive");

        // Bad row is present but recurrence-less (so it'll render
        // as a single event rather than expanding wrong).
        let bad = parsed.iter().find(|i| i.item_id == "MASTER-BAD").unwrap();
        assert!(
            bad.recurrence.is_none(),
            "malformed recurrence must drop the recurrence",
        );
        assert_eq!(bad.subject, "Broken master");

        // Good row survived with its weekly RRULE intact.
        let good = parsed.iter().find(|i| i.item_id == "MASTER-OK").unwrap();
        let rec = good.recurrence.as_ref().expect("good row keeps recurrence");
        assert_eq!(
            rec.pattern,
            EwsRecurrencePattern::Weekly {
                interval: 1,
                days_of_week: vec![EwsDay::Monday],
                first_day_of_week: EwsDay::Monday,
            },
        );
    }

    #[test]
    fn update_field_xml_clears_reminder_without_deleting_minutes() {
        // Regression: editing an event (incl. a recurring master)
        // down to "no reminder" must NOT emit a DeleteItemField for
        // ReminderMinutesBeforeStart — EWS rejects that with
        // ErrorInvalidPropertyDelete ("Die Löschaktion wird für
        // diese Eigenschaft nicht unterstützt"). The reminder is
        // turned off via ReminderIsSet=false instead.
        let item = ParsedItem {
            item_id: "M".into(),
            subject: "Series".into(),
            start: Some("2026-06-05T08:00:00Z".parse().unwrap()),
            end: Some("2026-06-05T08:30:00Z".parse().unwrap()),
            item_type: Some("RecurringMaster".into()),
            ..ParsedItem::default()
        };
        let mut ev = to_event(item, "cal").unwrap();
        ev.reminders = Vec::new(); // user cleared / never had a reminder

        let (set, del) = event_to_update_field_xml(&ev).unwrap();
        // Reminder turned off by setting the flag, not by deleting
        // the minutes field.
        assert!(
            set.contains("<t:ReminderIsSet>false</t:ReminderIsSet>"),
            "expected ReminderIsSet=false in set fields: {set}",
        );
        assert!(
            !del.contains("ReminderMinutesBeforeStart"),
            "must NOT DeleteItemField ReminderMinutesBeforeStart: {del}",
        );
    }

    #[test]
    fn parse_get_items_response_captures_body() {
        // SyncFolderItems never carries <t:Body>; the detail GetItem
        // fan-out is what pulls the description. The parser must
        // capture it into ParsedItem.body so to_event maps it to
        // Event.description (and the edit path round-trips it instead
        // of wiping it).
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:GetItemResponse>
      <m:ResponseMessages>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="WITH-BODY" ChangeKey="CK"/>
              <t:Subject>Has a description</t:Subject>
              <t:Body BodyType="Text">Bring the quarterly figures.</t:Body>
              <t:Start>2026-06-05T08:00:00Z</t:Start>
              <t:End>2026-06-05T08:30:00Z</t:End>
              <t:CalendarItemType>Single</t:CalendarItemType>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
      </m:ResponseMessages>
    </m:GetItemResponse>
  </soap:Body>
</soap:Envelope>"#;
        let parsed = parse_get_calendar_items_response(xml).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].item_id, "WITH-BODY");
        assert_eq!(
            parsed[0].body.as_deref(),
            Some("Bring the quarterly figures."),
        );
    }

    #[test]
    fn get_calendar_items_envelope_lists_all_ids_and_requests_recurrence() {
        let ids = vec![
            ("ID-1".to_string(), Some("CK-1".to_string())),
            ("ID-2".to_string(), None),
        ];
        let body = crate::soap::get_calendar_items_with_recurrence(&ids);
        // Both ids should be present in the request, with ChangeKey
        // attached only for the first.
        assert!(body.contains(r#"<t:ItemId Id="ID-1" ChangeKey="CK-1"/>"#));
        assert!(body.contains(r#"<t:ItemId Id="ID-2"/>"#));
        // The whole point of this envelope is to ask for the complex
        // properties that SyncFolderItems silently drops: the
        // recurrence shape AND the plain-text body (description).
        assert!(body.contains(r#"<t:FieldURI FieldURI="calendar:Recurrence"/>"#));
        assert!(body.contains(r#"<t:FieldURI FieldURI="calendar:ModifiedOccurrences"/>"#));
        assert!(body.contains(r#"<t:FieldURI FieldURI="calendar:DeletedOccurrences"/>"#));
        assert!(body.contains(r#"<t:FieldURI FieldURI="item:Body"/>"#));
        assert!(body.contains("<t:BodyType>Text</t:BodyType>"));
    }
}
