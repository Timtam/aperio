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
            Ok(XmlEvent::Text(t)) => {
                if text_target == Some("name") {
                    let s = t.unescape().map(|c| c.to_string()).unwrap_or_default();
                    current.display_name.push_str(&s);
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

/// One calendar item pulled from a `FindItem` response.
#[derive(Debug, Clone, Default)]
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
                                current.item_id =
                                    String::from_utf8_lossy(&a.value).into_owned();
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

/// EWS serialises timestamps as `YYYY-MM-DDTHH:MM:SSZ` (or
/// `YYYY-MM-DDTHH:MM:SS.fffZ`). Both parse cleanly through
/// `DateTime::parse_from_rfc3339`.
fn parse_ews_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// Translate a parsed item into a cal-core `Event`. The calendar id is
/// supplied by the caller (the API layer knows which folder we just
/// listed). Recurrence is left as `None` for now — EWS returns
/// `CalendarView` results already expanded, so each row is an
/// individual occurrence rather than a master + RRULE pair. When the
/// write-side lands in 6f.1b we'll wire `Recurrence`-element parsing
/// here too.
pub fn to_event(item: ParsedItem, calendar_id: &str) -> EwsResult<Event> {
    let start = item.start.ok_or_else(|| {
        EwsError::Protocol("CalendarItem missing Start".into())
    })?;
    let end = item.end.ok_or_else(|| {
        EwsError::Protocol("CalendarItem missing End".into())
    })?;

    let id = match &item.change_key {
        Some(ck) => format!("{}|{}", item.item_id, ck),
        None => item.item_id.clone(),
    };

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

    // `is_recurring=true` on a CalendarView row means "this row is one
    // expanded occurrence of a series". We don't have the master's
    // RRULE here, but we still want the frontend to render the series
    // hint (the chip in EventCard / MonthCell). Encoding it as a
    // synthetic `FREQ=DAILY;COUNT=1` would lie about the cadence — so
    // we drop the recurrence info on the read path and let the write
    // path (6f.1b) reach into Recurrence on a per-master GetItem when
    // it needs to edit a series.
    let recurrence: Option<EventRecurrence> = None;
    let _ = item.is_recurring; // suppress unused-field warning until 6f.1b

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
    if let Some(location) = event
        .location
        .as_deref()
        .filter(|s| !s.is_empty())
    {
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
    push_set_bool(&mut set, "calendar:IsAllDayEvent", "IsAllDayEvent", event.all_day);

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
pub fn rrule_to_ews_recurrence(
    rrule: &str,
    start: DateTime<Utc>,
) -> EwsResult<String> {
    let parts = parse_rrule(rrule);
    let freq = parts.get("FREQ").cloned().ok_or_else(|| {
        EwsError::Protocol(format!("RRULE missing FREQ: {rrule}"))
    })?;
    let interval = parts
        .get("INTERVAL")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);

    let pattern_xml = match freq.as_str() {
        "DAILY" => format!(
            "<t:DailyRecurrence><t:Interval>{interval}</t:Interval></t:DailyRecurrence>",
        ),
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
                EwsError::Protocol(format!(
                    "RRULE BYMONTH out of range: {month_num}"
                ))
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
        let count = count_str.parse::<u32>().map_err(|_| {
            EwsError::Protocol(format!("RRULE COUNT not numeric: {count_str}"))
        })?;
        format!(
            "<t:NumberedRecurrence><t:StartDate>{start_date}</t:StartDate><t:NumberOfOccurrences>{count}</t:NumberOfOccurrences></t:NumberedRecurrence>",
        )
    } else if let Some(until_str) = parts.get("UNTIL") {
        let end_date = parse_until_date(until_str).ok_or_else(|| {
            EwsError::Protocol(format!("RRULE UNTIL not parseable: {until_str}"))
        })?;
        format!(
            "<t:EndDateRecurrence><t:StartDate>{start_date}</t:StartDate><t:EndDate>{end_date}</t:EndDate></t:EndDateRecurrence>",
        )
    } else {
        format!(
            "<t:NoEndRecurrence><t:StartDate>{start_date}</t:StartDate></t:NoEndRecurrence>",
        )
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
        let stripped: &str = tok.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '+');
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
        assert_eq!(
            it.start.unwrap().to_rfc3339(),
            "2026-05-20T08:00:00+00:00"
        );
        assert_eq!(
            it.end.unwrap().to_rfc3339(),
            "2026-05-20T08:30:00+00:00"
        );
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
        };
        let ev = to_event(item, "FID|CK").unwrap();
        assert_eq!(ev.id, "IID|ICK");
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
        let xml = rrule_to_ews_recurrence(
            "FREQ=WEEKLY;BYDAY=MO,WE,FR",
            start,
        )
        .unwrap();
        assert!(xml.contains("<t:WeeklyRecurrence>"));
        assert!(xml.contains("<t:DaysOfWeek>Monday Wednesday Friday</t:DaysOfWeek>"));
    }

    #[test]
    fn rrule_monthly_with_bymonthday_translates_to_absolute_monthly() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence("FREQ=MONTHLY;BYMONTHDAY=15", start)
            .unwrap();
        assert!(xml.contains("<t:AbsoluteMonthlyRecurrence>"));
        assert!(xml.contains("<t:DayOfMonth>15</t:DayOfMonth>"));
    }

    #[test]
    fn rrule_yearly_with_bymonth_translates_to_absolute_yearly() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let xml = rrule_to_ews_recurrence(
            "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=15",
            start,
        )
        .unwrap();
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
        let xml = rrule_to_ews_recurrence(
            "FREQ=WEEKLY;BYDAY=TU;UNTIL=20260901T235959Z",
            start,
        )
        .unwrap();
        assert!(xml.contains("<t:EndDateRecurrence>"));
        assert!(xml.contains("<t:EndDate>2026-09-01</t:EndDate>"));
    }

    #[test]
    fn rrule_with_relative_monthly_rejected() {
        let start: DateTime<Utc> = "2026-05-20T08:00:00Z".parse().unwrap();
        let err = rrule_to_ews_recurrence(
            "FREQ=MONTHLY;BYDAY=2WE",
            start,
        )
        .unwrap_err();
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
}
