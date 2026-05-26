//! SOAP envelope plumbing for EWS.
//!
//! Two operations Aperio's read-only first cut needs:
//!
//!   - `FindFolder` to enumerate calendar folders
//!   - `FindItem` with `CalendarView` to pull events in a date range
//!
//! EWS namespaces (per Microsoft's WSDL):
//!
//!   soap: http://schemas.xmlsoap.org/soap/envelope/
//!   t:    http://schemas.microsoft.com/exchange/services/2006/types
//!   m:    http://schemas.microsoft.com/exchange/services/2006/messages
//!
//! The body builders below stamp the same envelope skeleton with
//! different operation payloads. Response parsing happens in
//! `mapping.rs` because the shape is operation-specific.

use chrono::{DateTime, Utc};
use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader;

use crate::error::{EwsError, EwsResult};

const ENVELOPE_PRELUDE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Header>
    <t:RequestServerVersion Version="Exchange2013_SP1"/>
  </soap:Header>
  <soap:Body>"#;

const ENVELOPE_EPILOGUE: &str = r#"  </soap:Body>
</soap:Envelope>"#;

fn wrap(body: &str) -> String {
    format!("{ENVELOPE_PRELUDE}\n{body}\n{ENVELOPE_EPILOGUE}")
}

/// SOAP body for `FindFolder`: enumerate every folder under the
/// mailbox root, restricted to ones whose `FolderClass` is
/// `IPF.Appointment` (= a calendar).
///
/// `Traversal="Deep"` walks subfolders too — important for users
/// who keep a "Work / Family / Personal" tree of calendars rather
/// than a flat list.
pub fn find_calendar_folders() -> String {
    let body = r#"    <m:FindFolder Traversal="Deep">
      <m:FolderShape>
        <t:BaseShape>AllProperties</t:BaseShape>
      </m:FolderShape>
      <m:Restriction>
        <t:IsEqualTo>
          <t:FieldURI FieldURI="folder:FolderClass"/>
          <t:FieldURIOrConstant>
            <t:Constant Value="IPF.Appointment"/>
          </t:FieldURIOrConstant>
        </t:IsEqualTo>
      </m:Restriction>
      <m:ParentFolderIds>
        <t:DistinguishedFolderId Id="msgfolderroot"/>
      </m:ParentFolderIds>
    </m:FindFolder>"#;
    wrap(body)
}

/// SOAP body for `SyncFolderItems`: Outlook-style delta sync of a
/// calendar folder. Returns one batch of Create/Update/Delete
/// notifications plus a fresh sync-state cookie. The caller stashes
/// the cookie and passes it back on the next call so we only pay
/// for the deltas.
///
/// Unlike `FindItem + CalendarView`, this returns **master items
/// with their `<t:Recurrence>` inline** plus separate notifications
/// for modified / deleted occurrences — exactly what Outlook itself
/// uses, and what the read path needs to surface recurring events
/// as series rather than N expanded single events.
///
/// `sync_state` is `None` on the initial sync; subsequent calls
/// pass the cookie the server returned last time. If the server
/// has discarded that state (cookie too old, mailbox rebuilt, …)
/// it surfaces an `ErrorInvalidSyncStateData` SOAP fault and the
/// caller resets to `None`.
///
/// `max_changes` caps the batch size; the server is free to return
/// fewer. The response's `IncludesLastItemInRange` flag tells the
/// caller whether more pages remain.
pub fn sync_folder_items(
    folder_id: &str,
    change_key: Option<&str>,
    sync_state: Option<&str>,
    max_changes: u32,
) -> String {
    let folder_id_attr = match change_key {
        Some(ck) => format!(
            r#"<t:FolderId Id="{}" ChangeKey="{}"/>"#,
            escape_xml(folder_id),
            escape_xml(ck)
        ),
        None => format!(r#"<t:FolderId Id="{}"/>"#, escape_xml(folder_id)),
    };
    let sync_state_xml = match sync_state {
        Some(s) if !s.is_empty() => {
            format!("<m:SyncState>{}</m:SyncState>", escape_xml(s))
        }
        _ => String::new(),
    };
    let body = format!(
        r#"    <m:SyncFolderItems>
      <m:ItemShape>
        <t:BaseShape>Default</t:BaseShape>
        <t:AdditionalProperties>
          <t:FieldURI FieldURI="item:Body"/>
          <t:FieldURI FieldURI="item:DateTimeCreated"/>
          <t:FieldURI FieldURI="item:LastModifiedTime"/>
          <t:FieldURI FieldURI="item:ReminderIsSet"/>
          <t:FieldURI FieldURI="item:ReminderMinutesBeforeStart"/>
          <t:FieldURI FieldURI="calendar:Location"/>
          <t:FieldURI FieldURI="calendar:Start"/>
          <t:FieldURI FieldURI="calendar:End"/>
          <t:FieldURI FieldURI="calendar:IsAllDayEvent"/>
          <t:FieldURI FieldURI="calendar:IsRecurring"/>
          <t:FieldURI FieldURI="calendar:CalendarItemType"/>
          <t:FieldURI FieldURI="calendar:Recurrence"/>
          <t:FieldURI FieldURI="calendar:ModifiedOccurrences"/>
          <t:FieldURI FieldURI="calendar:DeletedOccurrences"/>
        </t:AdditionalProperties>
      </m:ItemShape>
      <m:SyncFolderId>
        {folder_id_attr}
      </m:SyncFolderId>
      {sync_state_xml}
      <m:MaxChangesReturned>{max_changes}</m:MaxChangesReturned>
      <m:SyncScope>NormalItems</m:SyncScope>
    </m:SyncFolderItems>"#,
    );
    wrap(&body)
}

/// SOAP body for `FindItem` with a `CalendarView` window: get every
/// event (recurring instances *expanded* by the server) in
/// `[start, end)`. Pulls extra fields beyond the default shape so
/// the mapper has everything it needs without per-row GetItem
/// round-trips.
pub fn find_items_in_range(
    folder_id: &str,
    change_key: Option<&str>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> String {
    let start_iso = start.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let end_iso = end.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    // EWS folder ids carry a separate ChangeKey for optimistic
    // concurrency; we attach it when we have it, but plenty of
    // servers accept the id alone.
    let folder_id_attr = match change_key {
        Some(ck) => format!(
            r#"<t:FolderId Id="{}" ChangeKey="{}"/>"#,
            escape_xml(folder_id),
            escape_xml(ck)
        ),
        None => format!(r#"<t:FolderId Id="{}"/>"#, escape_xml(folder_id)),
    };
    let body = format!(
        r#"    <m:FindItem Traversal="Shallow">
      <m:ItemShape>
        <t:BaseShape>Default</t:BaseShape>
        <t:AdditionalProperties>
          <t:FieldURI FieldURI="item:Body"/>
          <t:FieldURI FieldURI="item:DateTimeCreated"/>
          <t:FieldURI FieldURI="item:LastModifiedTime"/>
          <t:FieldURI FieldURI="item:ReminderIsSet"/>
          <t:FieldURI FieldURI="item:ReminderMinutesBeforeStart"/>
          <t:FieldURI FieldURI="calendar:Location"/>
          <t:FieldURI FieldURI="calendar:Start"/>
          <t:FieldURI FieldURI="calendar:End"/>
          <t:FieldURI FieldURI="calendar:IsAllDayEvent"/>
          <t:FieldURI FieldURI="calendar:IsRecurring"/>
          <t:FieldURI FieldURI="calendar:CalendarItemType"/>
        </t:AdditionalProperties>
      </m:ItemShape>
      <m:CalendarView StartDate="{start_iso}" EndDate="{end_iso}"/>
      <m:ParentFolderIds>
        {folder_id_attr}
      </m:ParentFolderIds>
    </m:FindItem>"#,
    );
    wrap(&body)
}

/// Look for a SOAP fault in the response body and surface it as
/// `EwsError::Soap` with the structured EWS code. Returns Ok(()) if
/// the body is a non-fault response — the caller continues parsing
/// the operation-specific payload.
pub fn check_for_fault(body: &str) -> EwsResult<()> {
    // EWS faults come in three flavours:
    //   - `<soap:Fault>` for transport-level problems (auth failed,
    //     server error).
    //   - `<m:*Response>` with `<m:ResponseMessages>` containing one
    //     or more `<m:*ResponseMessage ResponseClass="Error">` for
    //     per-operation failures (the dominant shape: FindFolder,
    //     FindItem, GetItem, …).
    //   - `<m:*Response ResponseClass="Error">` *itself* — FindPeople
    //     is the outlier here, its response body has neither a
    //     `<m:ResponseMessages>` wrapper nor a `*ResponseMessage`
    //     element. The ResponseClass attribute lives directly on the
    //     `<FindPeopleResponse>` tag. Without catching this case the
    //     caller would see `Ok([])` for an ErrorInvalidOperation
    //     (e.g. `directory` distinguished folder is rejected on
    //     on-prem Exchange) and silently render an empty list.
    //
    // All three bubble out as `EwsError::Soap`. We check for all of
    // them.
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut inside_fault = false;
    let mut current_text_target: Option<&'static str> = None;
    let mut fault_code = String::new();
    let mut fault_string = String::new();
    let mut error_class_seen = false;
    let mut error_code = String::new();
    let mut error_text = String::new();
    let mut inside_response_message = false;
    // Top-level `<*Response ResponseClass="Error">` mode — used by
    // FindPeople and a handful of other singletons that don't wrap
    // their per-call result in `<m:ResponseMessages>`. We track the
    // depth so a child element (e.g. `<ResponseCode>` inside the
    // outer Response tag) routes its text into the right buffer
    // without colliding with the per-ResponseMessage path above.
    let mut inside_response_wrapper_error = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"fault" {
                    inside_fault = true;
                }
                if inside_fault {
                    if local == b"faultcode" {
                        current_text_target = Some("fc");
                    } else if local == b"faultstring" {
                        current_text_target = Some("fs");
                    }
                }
                // Per-operation error: any ResponseMessage element
                // (FindFolderResponseMessage, FindItemResponseMessage,
                // GetItemResponseMessage …) whose ResponseClass
                // attribute is "Error".
                if local.ends_with(b"responsemessage") {
                    inside_response_message = true;
                    for a in e.attributes().flatten() {
                        if a.key.as_ref().eq_ignore_ascii_case(b"ResponseClass")
                            && a.value.as_ref().eq_ignore_ascii_case(b"Error")
                        {
                            error_class_seen = true;
                        }
                    }
                }
                // Top-level `<*Response ResponseClass="Error">` —
                // the FindPeople shape. Match any element ending
                // in `response` (so it covers FindPeopleResponse,
                // GetAttachmentResponse, etc.) but NOT the inner
                // ResponseMessage (those are handled above and end
                // in `responsemessage`).
                if local.ends_with(b"response") && !local.ends_with(b"responsemessage") {
                    for a in e.attributes().flatten() {
                        if a.key.as_ref().eq_ignore_ascii_case(b"ResponseClass")
                            && a.value.as_ref().eq_ignore_ascii_case(b"Error")
                        {
                            inside_response_wrapper_error = true;
                            error_class_seen = true;
                        }
                    }
                }
                if (inside_response_message || inside_response_wrapper_error) && error_class_seen {
                    if local == b"messagetext" {
                        current_text_target = Some("et");
                    } else if local == b"responsecode" {
                        current_text_target = Some("ec");
                    }
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"fault" {
                    inside_fault = false;
                }
                if local.ends_with(b"responsemessage") {
                    // Bail out the moment we see the first error
                    // ResponseMessage; subsequent ones would just
                    // shadow this one with the same condition.
                    if error_class_seen && inside_response_message {
                        return Err(EwsError::Soap {
                            code: if error_code.is_empty() {
                                "Unknown".into()
                            } else {
                                error_code.clone()
                            },
                            message: if error_text.is_empty() {
                                "EWS returned an Error ResponseMessage".into()
                            } else {
                                error_text.clone()
                            },
                        });
                    }
                    inside_response_message = false;
                }
                if local.ends_with(b"response") && !local.ends_with(b"responsemessage") {
                    // Top-level wrapper closes: if it was tagged
                    // Error, bail out the same way the per-message
                    // branch does.
                    if inside_response_wrapper_error {
                        return Err(EwsError::Soap {
                            code: if error_code.is_empty() {
                                "Unknown".into()
                            } else {
                                error_code.clone()
                            },
                            message: if error_text.is_empty() {
                                "EWS returned an Error response".into()
                            } else {
                                error_text.clone()
                            },
                        });
                    }
                    inside_response_wrapper_error = false;
                }
                current_text_target = None;
            }
            Ok(XmlEvent::Text(t)) => {
                if let Some(target) = current_text_target {
                    let s = t.unescape().map(|c| c.to_string()).unwrap_or_default();
                    match target {
                        "fc" => fault_code.push_str(&s),
                        "fs" => fault_string.push_str(&s),
                        "ec" => error_code.push_str(&s),
                        "et" => error_text.push_str(&s),
                        _ => {}
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

    if !fault_code.is_empty() || !fault_string.is_empty() {
        return Err(EwsError::Soap {
            code: if fault_code.is_empty() {
                "SoapFault".into()
            } else {
                fault_code
            },
            message: if fault_string.is_empty() {
                "EWS returned a SOAP fault".into()
            } else {
                fault_string
            },
        });
    }
    Ok(())
}

/// SOAP body for `GetItem` against `<t:RecurringMasterItemId>` — the
/// special id form that says "give me the master CalendarItem of
/// this occurrence". Used by the API layer's lazy master-resolution
/// step before series-wide updates / deletes.
///
/// We ask for `IdOnly` since the master's ItemId is all we need;
/// the body, recurrence and the other properties don't matter for
/// the immediate write that follows.
pub fn get_recurring_master(occurrence_id: &str, change_key: Option<&str>) -> String {
    let id_attr = match change_key {
        Some(ck) => format!(
            r#"<t:RecurringMasterItemId OccurrenceId="{}" ChangeKey="{}"/>"#,
            escape_xml(occurrence_id),
            escape_xml(ck),
        ),
        None => format!(
            r#"<t:RecurringMasterItemId OccurrenceId="{}"/>"#,
            escape_xml(occurrence_id),
        ),
    };
    let body = format!(
        r#"    <m:GetItem>
      <m:ItemShape>
        <t:BaseShape>IdOnly</t:BaseShape>
      </m:ItemShape>
      <m:ItemIds>
        {id_attr}
      </m:ItemIds>
    </m:GetItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `GetItem` against a batch of CalendarItem ids,
/// requesting the **full** recurrence shape (the `<t:Recurrence>`
/// element plus its `<t:ModifiedOccurrences>` / `<t:DeletedOccurrences>`
/// siblings).
///
/// Why this exists: `SyncFolderItems` silently strips
/// `calendar:Recurrence`, `calendar:ModifiedOccurrences`, and
/// `calendar:DeletedOccurrences` from its response **regardless** of
/// what we list in `AdditionalProperties` — a well-known EWS quirk.
/// Outlook's own client works around it the same way: do
/// `SyncFolderItems` for change notifications + the cheap shape, then
/// fan out a `GetItem` batch for the RecurringMaster ids to pick up
/// the actual recurrence rules. Without this step every series in
/// Aperio would render as a single ghost event at the master's first
/// occurrence.
///
/// `ids` is a flat list of `(item_id, change_key)` pairs; the
/// caller groups them into batches that fit inside Exchange's
/// per-request throttling cap (we use 100 ids/batch — well under
/// the documented 1000 limit and conservative on body size).
pub fn get_calendar_items_with_recurrence(ids: &[(String, Option<String>)]) -> String {
    let mut item_ids_xml = String::new();
    for (id, ck) in ids {
        let attr = match ck {
            Some(ck) => format!(
                r#"        <t:ItemId Id="{}" ChangeKey="{}"/>"#,
                escape_xml(id),
                escape_xml(ck),
            ),
            None => format!(r#"        <t:ItemId Id="{}"/>"#, escape_xml(id)),
        };
        item_ids_xml.push_str(&attr);
        item_ids_xml.push('\n');
    }
    let body = format!(
        r#"    <m:GetItem>
      <m:ItemShape>
        <t:BaseShape>IdOnly</t:BaseShape>
        <t:AdditionalProperties>
          <t:FieldURI FieldURI="item:Subject"/>
          <t:FieldURI FieldURI="calendar:Start"/>
          <t:FieldURI FieldURI="calendar:End"/>
          <t:FieldURI FieldURI="calendar:IsRecurring"/>
          <t:FieldURI FieldURI="calendar:CalendarItemType"/>
          <t:FieldURI FieldURI="calendar:Recurrence"/>
          <t:FieldURI FieldURI="calendar:ModifiedOccurrences"/>
          <t:FieldURI FieldURI="calendar:DeletedOccurrences"/>
        </t:AdditionalProperties>
      </m:ItemShape>
      <m:ItemIds>
{item_ids_xml}      </m:ItemIds>
    </m:GetItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `CreateItem` into a calendar folder. The
/// `calendar_item_xml` slice is the pre-rendered `<t:CalendarItem>`
/// payload (built by `mapping::calendar_item_create_body`); we wrap
/// it in the appropriate envelope with `MessageDisposition="SaveOnly"`
/// and `SendMeetingInvitations="SendToNone"` so EWS just stores the
/// event without firing meeting invitations to attendees.
///
/// We pin the parent folder explicitly so Exchange knows which
/// calendar to file the event under — without `SavedItemFolderId`
/// the server defaults to the principal's primary calendar, which
/// is wrong when the user picked a different one in Aperio.
pub fn create_calendar_item(
    folder_id: &str,
    change_key: Option<&str>,
    calendar_item_xml: &str,
) -> String {
    let folder_id_attr = match change_key {
        Some(ck) => format!(
            r#"<t:FolderId Id="{}" ChangeKey="{}"/>"#,
            escape_xml(folder_id),
            escape_xml(ck),
        ),
        None => format!(r#"<t:FolderId Id="{}"/>"#, escape_xml(folder_id)),
    };
    let body = format!(
        r#"    <m:CreateItem MessageDisposition="SaveOnly" SendMeetingInvitations="SendToNone">
      <m:SavedItemFolderId>
        {folder_id_attr}
      </m:SavedItemFolderId>
      <m:Items>
{calendar_item_xml}
      </m:Items>
    </m:CreateItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `UpdateItem` against an existing calendar item.
/// `set_fields_xml` is the pre-rendered list of `<t:SetItemField>`
/// blocks the mapper builds for each field the caller wants changed.
///
/// `ConflictResolution="AlwaysOverwrite"` is the right setting for
/// Aperio's "what you see is what you get" semantics — the user's
/// edit wins even if someone else touched the event in parallel.
/// `SendMeetingInvitationsOrCancellations="SendToNone"` matches the
/// CreateItem stance: no calendar invitations leave the server,
/// since Aperio's attendee story is still a list of email addresses
/// without explicit RSVP wiring.
pub fn update_calendar_item(
    item_id: &str,
    change_key: Option<&str>,
    set_fields_xml: &str,
    delete_fields_xml: &str,
) -> String {
    let id_attr = match change_key {
        Some(ck) => format!(
            r#"<t:ItemId Id="{}" ChangeKey="{}"/>"#,
            escape_xml(item_id),
            escape_xml(ck),
        ),
        None => format!(r#"<t:ItemId Id="{}"/>"#, escape_xml(item_id)),
    };
    let body = format!(
        r#"    <m:UpdateItem ConflictResolution="AlwaysOverwrite"
                   MessageDisposition="SaveOnly"
                   SendMeetingInvitationsOrCancellations="SendToNone">
      <m:ItemChanges>
        <t:ItemChange>
          {id_attr}
          <t:Updates>
{set_fields_xml}{delete_fields_xml}
          </t:Updates>
        </t:ItemChange>
      </m:ItemChanges>
    </m:UpdateItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `DeleteItem`. `MoveToDeletedItems` is the gentle
/// default — the user can restore the event from the server's bin
/// if needed. EWS also offers `HardDelete` (skip the bin) and
/// `SoftDelete` (move to dumpster); we pick the user-friendly one.
///
/// On a recurring `<CalendarItem>` returned from `CalendarView`, the
/// ItemId points at *that occurrence*, not the master. Deleting that
/// id removes only that occurrence from the series — which is exactly
/// the EXDATE-equivalent semantics Aperio's "delete only this
/// occurrence" flow needs. Series-wide delete works against the
/// master id, which we don't read in 6f.1a/1b but can address with a
/// follow-up FindItem-without-CalendarView pass when needed.
pub fn delete_calendar_item(item_id: &str, change_key: Option<&str>) -> String {
    let id_attr = match change_key {
        Some(ck) => format!(
            r#"<t:ItemId Id="{}" ChangeKey="{}"/>"#,
            escape_xml(item_id),
            escape_xml(ck),
        ),
        None => format!(r#"<t:ItemId Id="{}"/>"#, escape_xml(item_id)),
    };
    let body = format!(
        r#"    <m:DeleteItem DeleteType="MoveToDeletedItems"
                   SendMeetingCancellations="SendToNone">
      <m:ItemIds>
        {id_attr}
      </m:ItemIds>
    </m:DeleteItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `UpdateFolder` setting `folder:DisplayName`. The
/// rename pushes the new name into Outlook profile — every Exchange
/// client (Outlook desktop, OWA, mobile) picks it up on its next
/// sync.
pub fn update_folder_displayname(
    folder_id: &str,
    change_key: Option<&str>,
    new_name: &str,
) -> String {
    let id_attr = match change_key {
        Some(ck) => format!(
            r#"<t:FolderId Id="{}" ChangeKey="{}"/>"#,
            escape_xml(folder_id),
            escape_xml(ck),
        ),
        None => format!(r#"<t:FolderId Id="{}"/>"#, escape_xml(folder_id)),
    };
    let name = escape_xml(new_name);
    let body = format!(
        r#"    <m:UpdateFolder>
      <m:FolderChanges>
        <t:FolderChange>
          {id_attr}
          <t:Updates>
            <t:SetFolderField>
              <t:FieldURI FieldURI="folder:DisplayName"/>
              <t:CalendarFolder>
                <t:DisplayName>{name}</t:DisplayName>
              </t:CalendarFolder>
            </t:SetFolderField>
          </t:Updates>
        </t:FolderChange>
      </m:FolderChanges>
    </m:UpdateFolder>"#,
    );
    wrap(&body)
}

/// Cheap-and-correct XML attribute escape — the values we substitute
/// into envelope bodies are server-supplied folder ids that already
/// contain `=` and `/` from base64 encoding, so we only need to
/// guard against the five reserved characters.
pub(crate) fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_calendar_folders_body_has_appointment_restriction() {
        let body = find_calendar_folders();
        assert!(body.contains("FindFolder"));
        assert!(body.contains(r#"Traversal="Deep""#));
        assert!(body.contains("IPF.Appointment"));
        assert!(body.contains("msgfolderroot"));
    }

    #[test]
    fn find_items_body_carries_range_and_folder_id() {
        let body = find_items_in_range(
            "AAMkAGI2THVS",
            Some("CQAAABYAAA"),
            "2026-05-01T00:00:00Z".parse().unwrap(),
            "2026-06-01T00:00:00Z".parse().unwrap(),
        );
        assert!(body.contains(r#"StartDate="2026-05-01T00:00:00Z""#));
        assert!(body.contains(r#"EndDate="2026-06-01T00:00:00Z""#));
        assert!(body.contains(r#"Id="AAMkAGI2THVS""#));
        assert!(body.contains(r#"ChangeKey="CQAAABYAAA""#));
        // Defaults beyond the BaseShape are pulled in via
        // AdditionalProperties so the mapper has body + reminders +
        // location without per-row GetItem traffic.
        assert!(body.contains("calendar:Location"));
        assert!(body.contains("calendar:IsAllDayEvent"));
        assert!(body.contains("item:ReminderMinutesBeforeStart"));
    }

    #[test]
    fn detects_soap_fault_block() {
        let body = r#"<?xml version="1.0"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <soap:Fault>
      <faultcode>soap:Client</faultcode>
      <faultstring>An internal server error occurred. The operation failed.</faultstring>
    </soap:Fault>
  </soap:Body>
</soap:Envelope>"#;
        let err = check_for_fault(body).unwrap_err();
        match err {
            EwsError::Soap { code, message } => {
                assert!(code.contains("Client"));
                assert!(message.contains("internal server error"));
            }
            other => panic!("expected Soap fault, got {other:?}"),
        }
    }

    #[test]
    fn detects_response_message_error_class() {
        let body = r#"<?xml version="1.0"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:FindItemResponse>
      <m:ResponseMessages>
        <m:FindItemResponseMessage ResponseClass="Error">
          <m:MessageText>Access is denied.</m:MessageText>
          <m:ResponseCode>ErrorAccessDenied</m:ResponseCode>
        </m:FindItemResponseMessage>
      </m:ResponseMessages>
    </m:FindItemResponse>
  </soap:Body>
</soap:Envelope>"#;
        let err = check_for_fault(body).unwrap_err();
        match err {
            EwsError::Soap { code, message } => {
                assert_eq!(code, "ErrorAccessDenied");
                assert_eq!(message, "Access is denied.");
            }
            other => panic!("expected Soap error, got {other:?}"),
        }
    }

    #[test]
    fn detects_top_level_response_error_class_findpeople_style() {
        // FindPeople doesn't wrap its response in ResponseMessages —
        // the ResponseClass attribute lives on the outer
        // `<FindPeopleResponse>` tag. Without this branch the
        // caller saw `Ok([])` for an ErrorInvalidOperation against
        // an on-prem Exchange that didn't recognise the
        // `directory` distinguished folder. Real wire body from
        // mail.hs-anhalt.de (Exchange 2019 CU14), reduced.
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Body>
    <FindPeopleResponse ResponseClass="Error"
                        xmlns="http://schemas.microsoft.com/exchange/services/2006/messages">
      <MessageText>Der Distinguished Name des Ordners wurde nicht erkannt.</MessageText>
      <ResponseCode>ErrorInvalidOperation</ResponseCode>
      <DescriptiveLinkKey>0</DescriptiveLinkKey>
      <TotalNumberOfPeopleInView>0</TotalNumberOfPeopleInView>
    </FindPeopleResponse>
  </s:Body>
</s:Envelope>"#;
        let err = check_for_fault(body).unwrap_err();
        match err {
            EwsError::Soap { code, message } => {
                assert_eq!(code, "ErrorInvalidOperation");
                assert!(message.contains("Distinguished Name"));
            }
            other => panic!("expected Soap error, got {other:?}"),
        }
    }

    #[test]
    fn get_recurring_master_uses_recurring_master_item_id_form() {
        let body = get_recurring_master("OCCURRENCE-ID", Some("OCK"));
        assert!(body.contains("GetItem"));
        assert!(body.contains("<t:BaseShape>IdOnly</t:BaseShape>"));
        assert!(body.contains(r#"OccurrenceId="OCCURRENCE-ID""#));
        assert!(body.contains(r#"ChangeKey="OCK""#));
        // Should NOT use a plain ItemId — the special form is what
        // tells EWS "give me the master, not this occurrence".
        assert!(!body.contains("<t:ItemId"));
    }

    #[test]
    fn create_calendar_item_pins_parent_folder() {
        let body = create_calendar_item(
            "FOLDER-ID",
            Some("FK"),
            "<t:CalendarItem><t:Subject>Lunch</t:Subject></t:CalendarItem>",
        );
        assert!(body.contains("CreateItem"));
        assert!(body.contains(r#"SendMeetingInvitations="SendToNone""#));
        assert!(body.contains(r#"Id="FOLDER-ID""#));
        assert!(body.contains(r#"ChangeKey="FK""#));
        assert!(body.contains("<t:Subject>Lunch</t:Subject>"));
    }

    #[test]
    fn update_calendar_item_wraps_set_and_delete_fields() {
        let body = update_calendar_item(
            "ITEM-ID",
            Some("IK"),
            "<t:SetItemField><t:FieldURI FieldURI=\"item:Subject\"/></t:SetItemField>",
            "<t:DeleteItemField><t:FieldURI FieldURI=\"item:ReminderIsSet\"/></t:DeleteItemField>",
        );
        assert!(body.contains("UpdateItem"));
        assert!(body.contains(r#"ConflictResolution="AlwaysOverwrite""#));
        assert!(body.contains(r#"Id="ITEM-ID""#));
        assert!(body.contains(r#"ChangeKey="IK""#));
        assert!(body.contains("SetItemField"));
        assert!(body.contains("DeleteItemField"));
    }

    #[test]
    fn delete_calendar_item_uses_move_to_deleted_items() {
        let body = delete_calendar_item("ITEM-ID", Some("IK"));
        assert!(body.contains("DeleteItem"));
        assert!(body.contains(r#"DeleteType="MoveToDeletedItems""#));
        assert!(body.contains(r#"Id="ITEM-ID""#));
        assert!(body.contains(r#"ChangeKey="IK""#));
    }

    #[test]
    fn update_folder_displayname_escapes_xml_in_name() {
        let body = update_folder_displayname("FID", Some("FK"), "Work & Play");
        assert!(body.contains("UpdateFolder"));
        assert!(body.contains(r#"FieldURI="folder:DisplayName""#));
        assert!(body.contains("Work &amp; Play"));
        assert!(body.contains(r#"Id="FID""#));
    }

    #[test]
    fn check_for_fault_passes_on_success_response() {
        let body = r#"<?xml version="1.0"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
               xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <soap:Body>
    <m:FindFolderResponse>
      <m:ResponseMessages>
        <m:FindFolderResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
        </m:FindFolderResponseMessage>
      </m:ResponseMessages>
    </m:FindFolderResponse>
  </soap:Body>
</soap:Envelope>"#;
        assert!(check_for_fault(body).is_ok());
    }
}
