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

use cal_core::{
    AttendeeResponse, AttendeeStatus, Calendar, Event, EventRecurrence, FreeBusy, FreeBusySlot,
    Reminder, ReminderKind,
};

use crate::error::{EwsError, EwsResult};

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

/// Decoded Aperio event id: split apart into a CalendarItem kind,
/// the raw ItemId, and the optional ChangeKey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedEventId {
    pub kind: EventIdKind,
    pub item_id: String,
    pub change_key: Option<String>,
}

/// Decode an Aperio event id. Falls back to `Single` if the string
/// has no prefix (compat path for ids minted before 6f.1c).
pub fn decode_event_id(s: &str) -> DecodedEventId {
    // The prefix is one character followed by `:`. Anything else is
    // treated as an un-prefixed legacy id.
    let mut chars = s.chars();
    let first = chars.next();
    let second = chars.next();
    if let (Some(p), Some(':')) = (first, second) {
        if let Some(kind) = EventIdKind::from_prefix(p) {
            let rest = &s[2..];
            let (item_id, change_key) = match rest.split_once('|') {
                Some((id, ck)) => (id.to_string(), Some(ck.to_string())),
                None => (rest.to_string(), None),
            };
            return DecodedEventId {
                kind,
                item_id,
                change_key,
            };
        }
    }
    // Legacy / unprefixed path.
    let (item_id, change_key) = match s.split_once('|') {
        Some((id, ck)) => (id.to_string(), Some(ck.to_string())),
        None => (s.to_string(), None),
    };
    DecodedEventId {
        kind: EventIdKind::Single,
        item_id,
        change_key,
    }
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
                    b"start" if inside_item => text_target = Some("start"),
                    b"end" if inside_item => text_target = Some("end"),
                    b"isrecurring" if inside_item => text_target = Some("recurring"),
                    b"calendaritemtype" if inside_item => text_target = Some("item_type"),
                    // Organizer + attendee subtree. The `Mailbox`
                    // (EmailAddress/Name) is shared by both, so the
                    // text targets are scoped by the enclosing flag.
                    b"organizer" if inside_item => inside_organizer = true,
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
                    Some("subject") => current.subject.push_str(s),
                    Some("body") => {
                        let acc = current.body.get_or_insert_with(String::new);
                        acc.push_str(s);
                    }
                    Some("start") => current.start = parse_ews_datetime(s),
                    Some("end") => current.end = parse_ews_datetime(s),
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
        exceptions.extend_from_slice(&item.deleted_occurrence_starts);
        for o in &item.modified_occurrences {
            exceptions.push(o.original_start);
        }
        EventRecurrence {
            rrule: r.to_rrule(),
            exceptions,
            // EWS reports the master's zone as a WINDOWS name; translate it to
            // IANA so the frontend expands the series DST-correctly. Unmapped /
            // absent → None (UTC expansion, as before).
            tzid: item
                .start_time_zone
                .as_deref()
                .and_then(crate::windows_tz::windows_to_iana)
                .map(str::to_string),
        }
    });

    // Attendees → editable flat list ("Name <email>" / bare) + RSVP state.
    let mut attendees = Vec::new();
    let mut attendee_responses = Vec::new();
    for a in item.attendees {
        if a.email.trim().is_empty() {
            continue;
        }
        let name = a.name.filter(|n| !n.trim().is_empty());
        attendees.push(match &name {
            Some(n) if n != &a.email => format!("{n} <{}>", a.email),
            _ => a.email.clone(),
        });
        attendee_responses.push(AttendeeResponse {
            status: a
                .response_type
                .as_deref()
                .map(ews_response_type)
                .unwrap_or_default(),
            name,
            email: a.email,
        });
    }
    let organizer = item.organizer.filter(|s| !s.trim().is_empty());

    // Cancelled-state resolution. Exchange normally flips `IsCancelled=true` on
    // a cancelled meeting, but some configs (notably an attendee whose mailbox
    // hasn't auto-processed the cancellation) leave that `false` and instead
    // (a) flip the `asfCanceled` (0x4) bit in `AppointmentState`, and/or
    // (b) prefix the subject with a localized "Canceled: " / "Abgesagt: ".
    // We treat any of the three as authoritative so a withdrawn meeting is
    // dimmed + announced regardless of which signal the server actually sends.
    let state_cancelled = item
        .appointment_state
        .map(|s| s & 0x4 != 0)
        .unwrap_or(false);
    let subject_cancelled = subject_marks_cancelled(&item.subject);
    let cancelled = item.cancelled || state_cancelled || subject_cancelled;

    Ok(Event {
        send_invitations: false,
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
        attendees,
        created_at: item.created.unwrap_or_else(Utc::now),
        updated_at: item.last_modified.unwrap_or_else(Utc::now),
        etag: item.change_key,
        organizer,
        attendee_responses,
        cancelled,
    })
}

/// Exchange's auto-processing prefixes a cancelled meeting's subject with a
/// localized "Canceled: " / "Abgesagt: " (leaving the body intact) rather than
/// deleting the item. Some server configs apply that prefix WITHOUT flipping
/// `IsCancelled`/`AppointmentState` on the attendee's copy, so the prefix is a
/// real secondary signal that the meeting was withdrawn. Matched case-insensitively
/// against the confirmed English + German forms; the trailing colon keeps a
/// user-authored title like "Abgesagt wegen Krankheit" from tripping it.
fn subject_marks_cancelled(subject: &str) -> bool {
    let s = subject.trim_start().to_ascii_lowercase();
    ["canceled:", "cancelled:", "abgesagt:", "storniert:"]
        .iter()
        .any(|p| s.starts_with(p))
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
    // A zoned recurring master: tell Exchange the series' zone (as a Windows id)
    // so it expands DST-correctly server-side. Per the EWS CalendarItemType
    // element order, StartTimeZone/EndTimeZone follow <t:Recurrence>.
    if let Some(windows) = event
        .recurrence
        .as_ref()
        .and_then(|r| r.tzid.as_deref())
        .and_then(crate::windows_tz::iana_to_windows)
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
pub fn event_to_update_field_xml(event: &Event) -> EwsResult<(String, String)> {
    let mut set = String::new();
    let mut del = String::new();

    push_set_string(&mut set, "item:Subject", "Subject", &event.title);
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
    if let Some(desc) = event.description.as_deref().filter(|s| !s.is_empty()) {
        push_set_body(&mut set, desc);
    }
    match event.location.as_deref().filter(|s| !s.is_empty()) {
        Some(loc) => {
            push_set_string(&mut set, "calendar:Location", "Location", loc);
        }
        None => {
            del.push_str(delete_item_field_xml("calendar:Location").as_str());
        }
    }
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
    // NB: when there's no reminder we DON'T `DeleteItemField`
    // ReminderMinutesBeforeStart. EWS refuses that delete with
    // `ErrorInvalidPropertyDelete` — the property always carries a
    // value server-side (a default), so it isn't deletable. Setting
    // `ReminderIsSet=false` above is the canonical way to turn a
    // reminder off; the stale minutes value is then ignored by the
    // server and by Outlook. (Deleting it was the cause of the
    // "Die Löschaktion wird für diese Eigenschaft nicht unterstützt"
    // failure when editing a recurring series.)

    // Attendees: SET when present (stored as a meeting). We do NOT emit a
    // DeleteItemField for an empty list — clearing all attendees doesn't
    // propagate (acceptable, and avoids an accidental mass-uninvite on edits
    // that never touched the attendee list). Whether attendees are EMAILED is
    // governed by the envelope's SendMeetingInvitationsOrCancellations.
    let attendees_xml = required_attendees_xml(&event.attendees);
    if !attendees_xml.is_empty() {
        set.push_str(&format!(
            "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:RequiredAttendees\"/>\n              <t:CalendarItem>\n{attendees_xml}              </t:CalendarItem>\n            </t:SetItemField>\n"
        ));
    }
    if let Some(rec) = &event.recurrence {
        let rec_xml = rrule_to_ews_recurrence(&rec.rrule, event.start)?;
        // Wrap the recurrence element in a SetItemField against
        // calendar:Recurrence. EWS expects the body's inner shape to
        // start with `<t:CalendarItem>` containing the recurrence.
        set.push_str(&format!(
            "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:Recurrence\"/>\n              <t:CalendarItem>\n                {rec_xml}\n              </t:CalendarItem>\n            </t:SetItemField>\n",
        ));
    } else {
        del.push_str(delete_item_field_xml("calendar:Recurrence").as_str());
    }
    // Keep the zone on a zoned recurring master so a server-side edit doesn't
    // drop it and re-expand the series in UTC. Field-by-field SetItemField, so
    // ordering is irrelevant; only when the IANA zone maps to a Windows id.
    if let Some(windows) = event
        .recurrence
        .as_ref()
        .and_then(|r| r.tzid.as_deref())
        .and_then(crate::windows_tz::iana_to_windows)
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
/// EWS hands back a plain instant that is midnight of the intended day
/// in SOME zone (the mailbox timezone, or UTC for boundaries we wrote
/// ourselves) without saying which. Sampling 12 hours INTO the day lands
/// inside the intended day in UTC for any zone offset in (−12h, +12h],
/// so the sample's UTC date recovers the day without guessing the zone.
/// DST edge: fall forward when the local zone skips midnight.
fn all_day_local_anchor(when: DateTime<Utc>) -> DateTime<Utc> {
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
            format!(
                "<t:WeeklyRecurrence><t:Interval>{interval}</t:Interval><t:DaysOfWeek>{days}</t:DaysOfWeek></t:WeeklyRecurrence>",
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
            } => Ok(EwsRecurrencePattern::Weekly {
                interval,
                days_of_week,
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
    fn to_event_maps_cancelled_via_subject_prefix() {
        // Fallback: neither flag set, but the subject carries the localized
        // "Abgesagt: " prefix Exchange's auto-processing prepends.
        let item = ParsedItem {
            item_id: "IID".into(),
            subject: "Abgesagt: Standup".into(),
            start: Some("2026-05-20T12:00:00Z".parse().unwrap()),
            end: Some("2026-05-20T12:30:00Z".parse().unwrap()),
            cancelled: false,
            appointment_state: Some(3), // asfMeeting|asfReceived, no cancel bit
            ..Default::default()
        };
        let ev = to_event(item, "FID|CK").unwrap();
        assert!(ev.cancelled);
    }

    #[test]
    fn to_event_not_cancelled_for_ordinary_meeting() {
        // A normal received meeting: no cancel bit, no cancel prefix. A user
        // title that merely contains "abgesagt" without the colon prefix must
        // NOT be treated as cancelled.
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

    #[test]
    fn event_to_update_field_xml_sets_start_time_zone_for_zoned_master() {
        let ev = Event {
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
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("CK".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
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
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("CK".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
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
            },
        );
        assert_eq!(rec.range, EwsRecurrenceRange::Numbered { occurrences: 10 },);
        assert_rrule_equivalent(&rec.to_rrule(), "FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=10");
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
        assert_eq!(
            item.start.unwrap().to_rfc3339(),
            "2025-12-15T00:00:00+00:00"
        );
        // …and it translates to IANA for the frontend expander.
        assert_eq!(
            crate::windows_tz::windows_to_iana(item.start_time_zone.as_deref().unwrap()),
            Some("America/New_York")
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

        // And the cal-core mapping normalises the response types.
        let mut full = item.clone();
        full.start = Some("2026-05-25T10:00:00Z".parse().unwrap());
        full.end = Some("2026-05-25T11:00:00Z".parse().unwrap());
        let ev = to_event(full, "cal-1").unwrap();
        assert_eq!(ev.organizer.as_deref(), Some("boss@example.com"));
        assert_eq!(ev.attendees[0], "The Boss <boss@example.com>");
        assert_eq!(ev.attendees[2], "maybe@example.com");
        assert_eq!(ev.attendee_responses[0].status, AttendeeStatus::Accepted);
        assert_eq!(ev.attendee_responses[1].status, AttendeeStatus::Tentative);
        assert_eq!(ev.attendee_responses[2].status, AttendeeStatus::Declined);
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
