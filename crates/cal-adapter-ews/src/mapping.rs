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

use chrono::{DateTime, Utc};
use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};

use cal_core::{Calendar, Event, EventRecurrence, Reminder, ReminderKind};

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
    let id = match &folder.change_key {
        Some(ck) => format!("{}|{}", folder.folder_id, ck),
        None => folder.folder_id.clone(),
    };
    Calendar {
        id,
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
    pub reminder_is_set: bool,
    pub reminder_minutes_before_start: Option<i64>,
    pub created: Option<DateTime<Utc>>,
    pub last_modified: Option<DateTime<Utc>>,
    /// `<t:CalendarItemType>` element value, normalised. Defaults to
    /// `Single` when EWS omits it (e.g. older servers that don't
    /// honour the property request).
    pub item_type: Option<String>,
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
    let mut inside_deleted_occurrences = false;
    let mut inside_deleted_occurrence = false;
    let mut inside_modified_occurrences = false;
    let mut inside_modified_occurrence = false;
    let mut current_override = ModifiedOccurrenceBuilder::default();

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
                    b"occurrence" if inside_modified_occurrence => {
                        inside_modified_occurrence = false;
                        if let Some(o) = std::mem::take(&mut current_override).finish() {
                            current.modified_occurrences.push(o);
                        }
                    }
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
                    Some("subject") => current.subject.push_str(s),
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
        }
    });

    Ok(Event {
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
        reminders,
        sound: None,
        attendees: Vec::new(),
        created_at: item.created.unwrap_or_else(Utc::now),
        updated_at: item.last_modified.unwrap_or_else(Utc::now),
        etag: item.change_key,
    })
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
    out.push_str(&format!(
        "          <t:Start>{}</t:Start>\n",
        format_ews_datetime(event.start)
    ));
    out.push_str(&format!(
        "          <t:End>{}</t:End>\n",
        format_ews_datetime(event.end)
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
    if let Some(rec) = &event.recurrence {
        let rec_xml = rrule_to_ews_recurrence(&rec.rrule, event.start)?;
        out.push_str("          ");
        out.push_str(&rec_xml);
        out.push('\n');
    }
    out.push_str("        </t:CalendarItem>");
    Ok(out)
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
    match event.description.as_deref().filter(|s| !s.is_empty()) {
        Some(desc) => {
            push_set_body(&mut set, desc);
        }
        None => {
            del.push_str(delete_item_field_xml("item:Body").as_str());
        }
    }
    match event.location.as_deref().filter(|s| !s.is_empty()) {
        Some(loc) => {
            push_set_string(&mut set, "calendar:Location", "Location", loc);
        }
        None => {
            del.push_str(delete_item_field_xml("calendar:Location").as_str());
        }
    }
    push_set_datetime(&mut set, "calendar:Start", "Start", event.start);
    push_set_datetime(&mut set, "calendar:End", "End", event.end);
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
    } else {
        del.push_str(delete_item_field_xml("item:ReminderMinutesBeforeStart").as_str());
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
//   - YEARLY  + BYMONTH=3 BYMONTHDAY=15 → AbsoluteYearlyRecurrence
//
// And the three ranges:
//
//   - default (no UNTIL/COUNT)    → NoEndRecurrence
//   - COUNT=N                     → NumberedRecurrence
//   - UNTIL=YYYYMMDD[THHMMSSZ]    → EndDateRecurrence
//
// Relative monthly / yearly ("last Wednesday of the month") is not
// covered yet — the EventDialog can't author them today and emitting
// a wrong rule would lose data on server-side rebuild. We bail with
// Protocol so the user sees "this rule isn't supported".

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
            // Absolute-monthly (BYMONTHDAY) is the only branch we
            // emit; if the user picked a relative shape (BYDAY=2WE
            // for "second Wednesday") we surface Protocol so the
            // dialog can flag it.
            if parts.contains_key("BYDAY") {
                return Err(EwsError::Protocol(
                    "relative monthly recurrence (BYDAY) is not supported by Aperio's EWS writer yet".into(),
                ));
            }
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
        "YEARLY" => {
            if parts.contains_key("BYDAY") {
                return Err(EwsError::Protocol(
                    "relative yearly recurrence (BYDAY) is not supported by Aperio's EWS writer yet".into(),
                ));
            }
            use chrono::Datelike;
            let day = parts
                .get("BYMONTHDAY")
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or_else(|| start.day() as u8);
            let month_num = parts
                .get("BYMONTH")
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or_else(|| start.month());
            let month_name = month_number_to_name(month_num).ok_or_else(|| {
                EwsError::Protocol(format!("RRULE BYMONTH out of range: {month_num}"))
            })?;
            format!(
                "<t:AbsoluteYearlyRecurrence><t:DayOfMonth>{day}</t:DayOfMonth><t:Month>{month_name}</t:Month></t:AbsoluteYearlyRecurrence>",
            )
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
//   - AbsoluteYearlyRecurrence   ↔  FREQ=YEARLY;BYMONTH=n;BYMONTHDAY=n
//   - NoEndRecurrence            ↔  (no UNTIL / COUNT)
//   - NumberedRecurrence         ↔  COUNT=n
//   - EndDateRecurrence          ↔  UNTIL=YYYYMMDD
//
// Relative monthly/yearly ("first Monday") still falls under "not
// yet supported by Aperio's EWS writer" — we surface those as a
// Protocol error so they don't silently turn into the wrong rule on
// the read side either.

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
        }
        match &self.range {
            EwsRecurrenceRange::NoEnd => {}
            EwsRecurrenceRange::Numbered { occurrences } => {
                parts.push(format!("COUNT={occurrences}"));
            }
            EwsRecurrenceRange::EndDate { end } => {
                // EWS sends EndDate as YYYY-MM-DD; RRULE UNTIL wants
                // YYYYMMDD (date-only form is legal per RFC 5545).
                let compact: String = end.chars().filter(|c| *c != '-').collect();
                parts.push(format!("UNTIL={compact}"));
            }
        }
        parts.join(";")
    }
}

/// Parse a `<t:Recurrence>...</t:Recurrence>` block (the full XML
/// fragment including the wrapping element) into a structured
/// [`EwsRecurrence`]. Returns `Err(Protocol)` on shapes we don't
/// support yet (`RelativeMonthlyRecurrence`, `RelativeYearlyRecurrence`,
/// missing pattern or range, …) so the caller can decide whether to
/// drop the master entirely or surface a user-visible warning.
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
    /// Set when we encounter a recurrence shape Aperio doesn't yet
    /// support (currently the two Relative* variants). The walker
    /// keeps consuming the rest of the subtree quietly and surfaces
    /// the error from [`Self::finish`] instead — so the caller can
    /// drop just this one row's recurrence rather than aborting the
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
                self.unsupported = Some("RelativeMonthlyRecurrence");
            }
            b"relativeyearlyrecurrence" => {
                self.unsupported = Some("RelativeYearlyRecurrence");
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
                    | Some(PatternBuilder::AbsoluteMonthly { interval, .. }) => {
                        *interval = v;
                    }
                    _ => {}
                }
            }
            Some("days_of_week") => {
                if let Some(PatternBuilder::Weekly { days_of_week, .. }) = self.pattern.as_mut() {
                    for tok in s.split_whitespace() {
                        if let Some(day) = EwsDay::from_wire(tok) {
                            days_of_week.push(day);
                        }
                    }
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
            Some("month") => {
                if let Some(PatternBuilder::AbsoluteYearly { month, .. }) = self.pattern.as_mut() {
                    *month = EwsMonth::from_wire(s);
                }
            }
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
    fn to_calendar_encodes_change_key_into_id() {
        let folder = ParsedFolder {
            folder_id: "FID".into(),
            change_key: Some("CK".into()),
            display_name: "Work".into(),
        };
        let cal = to_calendar(folder, false);
        assert_eq!(cal.id, "FID|CK");
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
            recurrence: None,
            deleted_occurrence_starts: Vec::new(),
            modified_occurrences: Vec::new(),
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
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
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
    fn new_event_to_calendar_item_xml_emits_all_day_flag() {
        let mut ev = new_event_min("Holiday");
        ev.all_day = true;
        let xml = new_event_to_calendar_item_xml(&ev).unwrap();
        assert!(xml.contains("<t:IsAllDayEvent>true</t:IsAllDayEvent>"));
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
    fn rrule_with_relative_monthly_rejected() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let err = rrule_to_ews_recurrence("FREQ=MONTHLY;BYDAY=2WE", start).unwrap_err();
        match err {
            EwsError::Protocol(m) => assert!(m.contains("relative monthly")),
            other => panic!("expected Protocol, got {other:?}"),
        }
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
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("CK".into()),
        };
        let (set, del) = event_to_update_field_xml(&ev).unwrap();
        assert!(set.contains("<t:Subject>Updated</t:Subject>"));
        assert!(set.contains("ReminderIsSet"));
        // Optional fields without values become DeleteItemField blocks.
        assert!(del.contains("FieldURI=\"item:Body\""));
        assert!(del.contains("FieldURI=\"calendar:Location\""));
        assert!(del.contains("FieldURI=\"calendar:Recurrence\""));
        assert!(del.contains("FieldURI=\"item:ReminderMinutesBeforeStart\""));
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
            recurrence: None,
            deleted_occurrence_starts: Vec::new(),
            modified_occurrences: Vec::new(),
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
        // Writer drops the time portion in EndDate → reader's
        // UNTIL is date-only. Both are RFC 5545 legal.
        assert_rrule_equivalent(&rec.to_rrule(), "FREQ=MONTHLY;BYMONTHDAY=15;UNTIL=20271231");
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
    fn parse_recurrence_rejects_relative_monthly() {
        // We don't author RelativeMonthlyRecurrence today, but
        // legacy series on the server might use it. Surface as
        // Protocol so the caller can drop the master (and the
        // user sees a recognisable error in logs).
        let xml = r#"<t:Recurrence>
            <t:RelativeMonthlyRecurrence>
              <t:Interval>1</t:Interval>
              <t:DaysOfWeek>Monday</t:DaysOfWeek>
              <t:DayOfWeekIndex>First</t:DayOfWeekIndex>
            </t:RelativeMonthlyRecurrence>
            <t:NoEndRecurrence>
              <t:StartDate>2026-01-01</t:StartDate>
            </t:NoEndRecurrence>
          </t:Recurrence>"#;
        let err = parse_ews_recurrence(xml).unwrap_err();
        match err {
            EwsError::Protocol(m) => {
                assert!(m.contains("RelativeMonthlyRecurrence"), "got {m}");
            }
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
    fn parse_get_items_response_keeps_other_rows_when_one_master_uses_unsupported_recurrence() {
        // Regression for the production bug where ONE master with
        // a RelativeMonthlyRecurrence ("Abgesagt: Themenoffener
        // Austausch", repeating every third Wednesday) poisoned
        // the whole GetItem fan-out and caused zero events from
        // the EWS calendar to render — including singles in the
        // same drain (which depend on a successful sync state
        // commit). The walker must tolerate the bad row, leave
        // its recurrence empty, and continue parsing the rest.
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
              <t:Subject>Third Wednesday meeting</t:Subject>
              <t:Start>2024-05-15T10:30:00Z</t:Start>
              <t:End>2024-05-15T12:00:00Z</t:End>
              <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
              <t:Recurrence>
                <t:RelativeMonthlyRecurrence>
                  <t:Interval>1</t:Interval>
                  <t:DaysOfWeek>Wednesday</t:DaysOfWeek>
                  <t:DayOfWeekIndex>Third</t:DayOfWeekIndex>
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
            "unsupported Relative* must drop the recurrence",
        );
        assert_eq!(bad.subject, "Third Wednesday meeting");

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
        // recurrence properties that SyncFolderItems silently drops.
        assert!(body.contains(r#"<t:FieldURI FieldURI="calendar:Recurrence"/>"#));
        assert!(body.contains(r#"<t:FieldURI FieldURI="calendar:ModifiedOccurrences"/>"#));
        assert!(body.contains(r#"<t:FieldURI FieldURI="calendar:DeletedOccurrences"/>"#));
    }
}
