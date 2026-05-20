//! EWS SOAP client.
//!
//! Compared to the Graph / Google adapters this layer is markedly
//! simpler:
//!
//!   - No token refresh dance — Basic auth headers don't expire.
//!   - No JSON serialisation — we build SOAP bodies by hand in
//!     `soap.rs` and parse responses in `mapping.rs`.
//!   - No pagination plumbing — Aperio's read flow asks for a single
//!     date-bounded window with `CalendarView`, which EWS returns
//!     unpaged unless the result exceeds the server's MaxEntries cap.
//!     If we ever bump into that we'll add `IndexedPageItemView`
//!     paging here; for the first cut a bounded window is plenty.
//!
//! All HTTP calls go to the same `endpoint` URL. EWS rejects requests
//! whose `Content-Type` isn't `text/xml; charset=utf-8`, so we set it
//! explicitly even though reqwest would default to no header.

use chrono::{DateTime, Utc};
use cal_core::{Calendar, Event, NewEvent};
use reqwest::header::{HeaderValue, CONTENT_TYPE};

use crate::auth::{basic_auth_header, BasicCredentials};
use crate::error::{EwsError, EwsResult};
use crate::mapping::{
    decode_event_id, encode_event_id, event_to_update_field_xml, new_event_to_calendar_item_xml,
    parse_find_folder_response, parse_find_item_response, parse_first_item_id,
    split_calendar_id, to_calendar, to_event, DecodedEventId, EventIdKind,
};
use crate::soap::{
    check_for_fault, create_calendar_item, delete_calendar_item, find_calendar_folders,
    find_items_in_range, get_recurring_master, update_calendar_item,
    update_folder_displayname,
};

/// State carried by the adapter — endpoint + credentials + reqwest
/// client. `Clone` because the trait impls hand it off to async tasks.
#[derive(Debug, Clone)]
pub struct EwsClient {
    pub endpoint: String,
    pub credentials: BasicCredentials,
    pub http: reqwest::Client,
}

impl EwsClient {
    pub fn new(endpoint: String, credentials: BasicCredentials, http: reqwest::Client) -> Self {
        Self {
            endpoint,
            credentials,
            http,
        }
    }

    /// POST a SOAP body to the EWS endpoint and return the response
    /// body as a string. The caller hands the result to
    /// `check_for_fault` first, then to an operation-specific parser.
    async fn post_soap(&self, body: String) -> EwsResult<String> {
        let auth = basic_auth_header(&self.credentials)?;
        let mut req = self
            .http
            .post(&self.endpoint)
            .body(body)
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("text/xml; charset=utf-8"),
            );
        // Microsoft documents `SOAPAction: ""` (empty) for EWS; some
        // older servers reject a missing header and accept the empty
        // string only.
        req = req.header("SOAPAction", HeaderValue::from_static("\"\""));
        for (k, v) in auth.iter() {
            req = req.header(k, v.clone());
        }

        let res = req.send().await?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(EwsError::Http {
                status: status.as_u16(),
                message: if text.is_empty() {
                    status.canonical_reason().unwrap_or("").to_string()
                } else {
                    text
                },
            });
        }
        check_for_fault(&text)?;
        Ok(text)
    }
}

/// Enumerate every calendar folder reachable from the user's
/// mailbox root. With write paths landed in 6f.1b every folder the
/// server hands us is editable — Aperio's "read-only" flag is meant
/// for "the adapter can't write back to this source at all", which
/// no longer applies once CreateItem/UpdateItem/DeleteItem are
/// wired up.
pub async fn list_calendars(client: &EwsClient) -> EwsResult<Vec<Calendar>> {
    let body = find_calendar_folders();
    let xml = client.post_soap(body).await?;
    let parsed = parse_find_folder_response(&xml)?;
    Ok(parsed.into_iter().map(|f| to_calendar(f, false)).collect())
}

/// Pull every event in `[start, end)` from `calendar_id`. EWS
/// expands recurring series server-side via `CalendarView`, so the
/// result is a flat list of occurrences — no client-side expansion
/// needed.
pub async fn get_events(
    client: &EwsClient,
    calendar_id: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> EwsResult<Vec<Event>> {
    let (folder_id, change_key) = split_calendar_id(calendar_id);
    let body = find_items_in_range(&folder_id, change_key.as_deref(), start, end);
    let xml = client.post_soap(body).await?;
    let parsed = parse_find_item_response(&xml)?;
    parsed
        .into_iter()
        .map(|item| to_event(item, calendar_id))
        .collect()
}

/// Create a new calendar item in `calendar_id`. Returns the
/// freshly-saved event with the server-assigned ItemId in
/// `Event.id`, prefixed with the CalendarItemType so the next
/// edit/delete cycle routes correctly.
///
/// Implementation note: we don't issue a follow-up `GetItem` to
/// reconstruct the full event from the server. The fields we have on
/// the `NewEvent` are already canonical (the user just typed them
/// in), so we mint the resulting `Event` locally from the request
/// payload and only pull the freshly assigned ItemId from the
/// response. That's one round-trip instead of two.
pub async fn create_event(
    client: &EwsClient,
    calendar_id: &str,
    event: NewEvent,
) -> EwsResult<Event> {
    let (folder_id, folder_change_key) = split_calendar_id(calendar_id);
    let item_xml = new_event_to_calendar_item_xml(&event)?;
    let envelope = create_calendar_item(
        &folder_id,
        folder_change_key.as_deref(),
        &item_xml,
    );
    let response = client.post_soap(envelope).await?;
    let item_ref = parse_first_item_id(&response)?;
    // A freshly created item is a RecurringMaster when it has a
    // recurrence rule (CreateItem on a recurring CalendarItem produces
    // the series template), otherwise a Single.
    let kind = if event.recurrence.is_some() {
        EventIdKind::RecurringMaster
    } else {
        EventIdKind::Single
    };
    Ok(build_event_from_new(
        &event,
        calendar_id,
        kind,
        &item_ref.id,
        item_ref.change_key,
    ))
}

/// Update an existing calendar item with the supplied event payload.
/// All fields are set; absent fields become DeleteItemField blocks
/// so EWS clears them server-side.
///
/// For occurrences of a recurring series, we resolve the master id
/// first (via `GetItem` with `RecurringMasterItemId`) and run the
/// UpdateItem against that — matching Aperio's "edit recurring event
/// = edit the whole series" semantics. Per-occurrence overrides go
/// through a separate flow (`add_event_exdate` for skips, or a
/// future exception-override-create API).
pub async fn update_event(client: &EwsClient, event: &Event) -> EwsResult<Event> {
    let decoded = decode_event_id(&event.id);
    let target = resolve_write_target(client, &decoded).await?;
    let (set_xml, delete_xml) = event_to_update_field_xml(event)?;
    let envelope = update_calendar_item(
        &target.item_id,
        target.change_key.as_deref(),
        &set_xml,
        &delete_xml,
    );
    let response = client.post_soap(envelope).await?;
    let item_ref = parse_first_item_id(&response)?;
    // The kind we wrote against was `Single` (no recurrence) or
    // `RecurringMaster` (resolved from an occurrence). On a series-
    // wide update the row we ended up with is the master; otherwise
    // it's the single event itself.
    let returned_kind = match target.kind {
        EventIdKind::Single => EventIdKind::Single,
        _ => EventIdKind::RecurringMaster,
    };
    let new_id = encode_event_id(returned_kind, &item_ref.id, item_ref.change_key.as_deref());
    Ok(Event {
        id: new_id,
        etag: item_ref.change_key,
        updated_at: Utc::now(),
        ..event.clone()
    })
}

/// Delete a calendar item. For non-recurring events this drops the
/// single row. For occurrences / exceptions of a recurring series,
/// resolves the master and deletes the whole series — matching the
/// "delete recurring event = delete series" UX.
///
/// Per-occurrence skip (the EXDATE-equivalent) goes through
/// `add_event_exdate` instead, which always targets the raw
/// occurrence id.
pub async fn delete_event(client: &EwsClient, event_id: &str) -> EwsResult<()> {
    let decoded = decode_event_id(event_id);
    let target = resolve_write_target(client, &decoded).await?;
    let envelope =
        delete_calendar_item(&target.item_id, target.change_key.as_deref());
    client.post_soap(envelope).await?;
    Ok(())
}

/// Skip a single occurrence of a recurring series. EWS doesn't model
/// EXDATE as an editable property on the master — the equivalent is
/// to `DeleteItem` the specific occurrence id, which removes that
/// date from future expansions without touching the rest of the
/// series. We deliberately *do not* resolve to the master here.
///
/// Single (non-recurring) events shouldn't normally hit this path —
/// the frontend only offers "delete only this occurrence" on series
/// events — but if they do, we delete the row regardless, which is
/// the same result the user would get by clicking the regular
/// delete button.
pub async fn add_event_exdate(client: &EwsClient, event_id: &str) -> EwsResult<()> {
    let decoded = decode_event_id(event_id);
    let envelope =
        delete_calendar_item(&decoded.item_id, decoded.change_key.as_deref());
    client.post_soap(envelope).await?;
    Ok(())
}

/// Resolve the (id, change_key) pair to use when writing against a
/// decoded Aperio event id. For Single / RecurringMaster the decoded
/// id is the target directly; for Occurrence / Exception we ask the
/// server "what's the master of this occurrence?" via a `GetItem`
/// with the special `RecurringMasterItemId` form, then return the
/// master's id pair.
async fn resolve_write_target(
    client: &EwsClient,
    decoded: &DecodedEventId,
) -> EwsResult<WriteTarget> {
    if !decoded.kind.is_occurrence_like() {
        return Ok(WriteTarget {
            kind: decoded.kind,
            item_id: decoded.item_id.clone(),
            change_key: decoded.change_key.clone(),
        });
    }
    let envelope =
        get_recurring_master(&decoded.item_id, decoded.change_key.as_deref());
    let response = client.post_soap(envelope).await?;
    let master = parse_first_item_id(&response)?;
    Ok(WriteTarget {
        kind: EventIdKind::RecurringMaster,
        item_id: master.id,
        change_key: master.change_key,
    })
}

/// Helper: the resolved (id, change_key) pair plus the kind we
/// believe we're writing against.
#[derive(Debug, Clone)]
struct WriteTarget {
    kind: EventIdKind,
    item_id: String,
    change_key: Option<String>,
}

/// Rename a calendar folder via `UpdateFolder` + `folder:DisplayName`.
/// Mirrors the CalDAV adapter's PROPPATCH-displayname flow — the new
/// name lands in Outlook profile next sync.
pub async fn rename_calendar(
    client: &EwsClient,
    calendar_id: &str,
    new_name: &str,
) -> EwsResult<()> {
    let (folder_id, change_key) = split_calendar_id(calendar_id);
    let envelope =
        update_folder_displayname(&folder_id, change_key.as_deref(), new_name);
    client.post_soap(envelope).await?;
    Ok(())
}

/// Glue: combine a request-side `NewEvent` with the server-assigned
/// ids into a cal-core `Event`. Used by `create_event` to avoid a
/// follow-up GetItem; the field set is whatever the user typed,
/// timestamps default to "now" so the row sorts sensibly in the
/// frontend cache.
fn build_event_from_new(
    new: &NewEvent,
    calendar_id: &str,
    kind: EventIdKind,
    item_id: &str,
    change_key: Option<String>,
) -> Event {
    let now = Utc::now();
    let aperio_id = encode_event_id(kind, item_id, change_key.as_deref());
    Event {
        id: aperio_id,
        calendar_id: calendar_id.to_string(),
        title: new.title.clone(),
        description: new.description.clone(),
        location: new.location.clone(),
        start: new.start,
        end: new.end,
        all_day: new.all_day,
        recurrence: new.recurrence.clone(),
        color_label: new.color_label.clone(),
        reminders: new.reminders.clone(),
        sound: new.sound.clone(),
        attendees: new.attendees.clone(),
        created_at: now,
        updated_at: now,
        etag: change_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    fn creds() -> BasicCredentials {
        BasicCredentials {
            username: "alice".into(),
            password: "pw".into(),
        }
    }

    fn client_for(server: &Server) -> EwsClient {
        EwsClient::new(
            server.url(),
            creds(),
            reqwest::Client::builder().build().unwrap(),
        )
    }

    #[tokio::test]
    async fn list_calendars_parses_two_folders() {
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
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
                <t:FolderId Id="FA" ChangeKey="K1"/>
                <t:DisplayName>Calendar</t:DisplayName>
              </t:CalendarFolder>
              <t:CalendarFolder>
                <t:FolderId Id="FB"/>
                <t:DisplayName>Work</t:DisplayName>
              </t:CalendarFolder>
            </t:Folders>
          </m:RootFolder>
        </m:FindFolderResponseMessage>
      </m:ResponseMessages>
    </m:FindFolderResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "text/xml; charset=utf-8")
            .with_body(body)
            .create_async()
            .await;
        let client = client_for(&server);
        let cals = list_calendars(&client).await.unwrap();
        assert_eq!(cals.len(), 2);
        assert_eq!(cals[0].id, "FA|K1");
        assert_eq!(cals[0].name, "Calendar");
        assert!(!cals[0].read_only); // 6f.1b made EWS calendars writable
        assert_eq!(cals[1].id, "FB");
    }

    #[tokio::test]
    async fn list_calendars_surfaces_soap_fault() {
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Body>
    <s:Fault>
      <faultcode>s:Client</faultcode>
      <faultstring>The user is not authorized.</faultstring>
    </s:Fault>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let err = list_calendars(&client_for(&server)).await.unwrap_err();
        match err {
            EwsError::Soap { code, message } => {
                assert!(code.contains("Client"));
                assert!(message.contains("not authorized"));
            }
            other => panic!("expected Soap, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_calendars_surfaces_http_error() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(401)
            .with_body("denied")
            .create_async()
            .await;
        let err = list_calendars(&client_for(&server)).await.unwrap_err();
        match err {
            EwsError::Http { status, .. } => assert_eq!(status, 401),
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_events_parses_two_items() {
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:FindItemResponse>
      <m:ResponseMessages>
        <m:FindItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:RootFolder TotalItemsInView="2">
            <t:Items>
              <t:CalendarItem>
                <t:ItemId Id="I1" ChangeKey="K1"/>
                <t:Subject>Standup</t:Subject>
                <t:Start>2026-05-20T08:00:00Z</t:Start>
                <t:End>2026-05-20T08:30:00Z</t:End>
                <t:IsAllDayEvent>false</t:IsAllDayEvent>
                <t:ReminderIsSet>false</t:ReminderIsSet>
              </t:CalendarItem>
              <t:CalendarItem>
                <t:ItemId Id="I2"/>
                <t:Subject>All-hands</t:Subject>
                <t:Start>2026-05-20T15:00:00Z</t:Start>
                <t:End>2026-05-20T16:00:00Z</t:End>
                <t:IsAllDayEvent>false</t:IsAllDayEvent>
                <t:ReminderIsSet>true</t:ReminderIsSet>
                <t:ReminderMinutesBeforeStart>5</t:ReminderMinutesBeforeStart>
              </t:CalendarItem>
            </t:Items>
          </m:RootFolder>
        </m:FindItemResponseMessage>
      </m:ResponseMessages>
    </m:FindItemResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let client = client_for(&server);
        let events = get_events(
            &client,
            "FA|K1",
            "2026-05-20T00:00:00Z".parse().unwrap(),
            "2026-05-21T00:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Standup");
        assert!(events[0].reminders.is_empty());
        assert_eq!(events[1].reminders.len(), 1);
    }

    #[tokio::test]
    async fn post_soap_sends_basic_auth_header() {
        let mut server = Server::new_async().await;
        // mockito's match_header verifies the request carried the
        // Basic-auth value we computed for "alice:pw".
        let _m = server
            .mock("POST", "/")
            .match_header(
                "Authorization",
                "Basic YWxpY2U6cHc=",
            )
            .match_header("Content-Type", "text/xml; charset=utf-8")
            .with_status(200)
            .with_body(
                r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <s:Body><m:FindFolderResponse><m:ResponseMessages>
    <m:FindFolderResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:RootFolder><t:Folders xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"/></m:RootFolder>
    </m:FindFolderResponseMessage>
  </m:ResponseMessages></m:FindFolderResponse></s:Body>
</s:Envelope>"#,
            )
            .create_async()
            .await;
        let cals = list_calendars(&client_for(&server)).await.unwrap();
        assert!(cals.is_empty());
    }

    fn create_item_success_body() -> &'static str {
        r#"<?xml version="1.0"?>
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
              <t:ItemId Id="NEW-ITEM-ID" ChangeKey="NEW-CK"/>
            </t:CalendarItem>
          </m:Items>
        </m:CreateItemResponseMessage>
      </m:ResponseMessages>
    </m:CreateItemResponse>
  </s:Body>
</s:Envelope>"#
    }

    fn new_event(title: &str) -> NewEvent {
        NewEvent {
            title: title.into(),
            description: Some("notes".into()),
            location: Some("Online".into()),
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

    #[tokio::test]
    async fn create_event_returns_event_with_server_assigned_id() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(create_item_success_body())
            .create_async()
            .await;
        let client = client_for(&server);
        let event = create_event(&client, "FOLDER-ID|FCK", new_event("Lunch"))
            .await
            .unwrap();
        // Non-recurring create → Single, so the id carries the
        // `S:` prefix.
        assert_eq!(event.id, "S:NEW-ITEM-ID|NEW-CK");
        assert_eq!(event.title, "Lunch");
        assert_eq!(event.calendar_id, "FOLDER-ID|FCK");
        assert_eq!(event.etag.as_deref(), Some("NEW-CK"));
    }

    #[tokio::test]
    async fn create_event_surfaces_soap_fault_unchanged() {
        let mut server = Server::new_async().await;
        let fault_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <s:Body>
    <m:CreateItemResponse>
      <m:ResponseMessages>
        <m:CreateItemResponseMessage ResponseClass="Error">
          <m:MessageText>Access denied.</m:MessageText>
          <m:ResponseCode>ErrorAccessDenied</m:ResponseCode>
        </m:CreateItemResponseMessage>
      </m:ResponseMessages>
    </m:CreateItemResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(fault_body)
            .create_async()
            .await;
        let err =
            create_event(&client_for(&server), "FOLDER-ID|FCK", new_event("X"))
                .await
                .unwrap_err();
        match err {
            EwsError::Soap { code, .. } => assert_eq!(code, "ErrorAccessDenied"),
            other => panic!("expected Soap, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn update_event_replaces_change_key_in_id() {
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:UpdateItemResponse>
      <m:ResponseMessages>
        <m:UpdateItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="ITEM-ID" ChangeKey="CK-V2"/>
            </t:CalendarItem>
          </m:Items>
        </m:UpdateItemResponseMessage>
      </m:ResponseMessages>
    </m:UpdateItemResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let starting = Event {
            id: "S:ITEM-ID|CK-V1".into(),
            calendar_id: "FOLDER-ID|FCK".into(),
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
            etag: Some("CK-V1".into()),
        };
        let updated = update_event(&client_for(&server), &starting).await.unwrap();
        // ChangeKey advances on every successful UpdateItem; the
        // kind stays Single (no master-lookup happens for `S:` ids).
        assert_eq!(updated.id, "S:ITEM-ID|CK-V2");
        assert_eq!(updated.etag.as_deref(), Some("CK-V2"));
    }

    #[tokio::test]
    async fn delete_event_round_trips_against_server() {
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <s:Body>
    <m:DeleteItemResponse>
      <m:ResponseMessages>
        <m:DeleteItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
        </m:DeleteItemResponseMessage>
      </m:ResponseMessages>
    </m:DeleteItemResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(
                r#"Id="ITEM-ID""#.to_string(),
            ))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        delete_event(&client_for(&server), "ITEM-ID|CK")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn update_event_on_occurrence_resolves_master_first() {
        // Two POSTs in sequence to the same endpoint:
        //   1. GetItem with RecurringMasterItemId → returns the
        //      master's ItemId (MASTER-ID / MCK-V1).
        //   2. UpdateItem against MASTER-ID → returns the new
        //      ChangeKey (MCK-V2).
        // We use mockito's body-regex matching to bind each mock to
        // the right SOAP body shape, so the order doesn't depend on
        // when the framework happens to evaluate them.
        let mut server = Server::new_async().await;
        let master_lookup_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:GetItemResponse>
      <m:ResponseMessages>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="MASTER-ID" ChangeKey="MCK-V1"/>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
      </m:ResponseMessages>
    </m:GetItemResponse>
  </s:Body>
</s:Envelope>"#;
        let update_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:UpdateItemResponse>
      <m:ResponseMessages>
        <m:UpdateItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="MASTER-ID" ChangeKey="MCK-V2"/>
            </t:CalendarItem>
          </m:Items>
        </m:UpdateItemResponseMessage>
      </m:ResponseMessages>
    </m:UpdateItemResponse>
  </s:Body>
</s:Envelope>"#;
        let _m1 = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("RecurringMasterItemId".into()))
            .with_status(200)
            .with_body(master_lookup_body)
            .create_async()
            .await;
        let _m2 = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("UpdateItem".into()))
            .with_status(200)
            .with_body(update_body)
            .create_async()
            .await;

        let starting = Event {
            // Occurrence-prefixed id — update_event should resolve
            // master via GetItem before issuing the UpdateItem.
            id: "O:OCC-ID|OCK".into(),
            calendar_id: "FOLDER-ID|FCK".into(),
            title: "Renamed series".into(),
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
            etag: Some("OCK".into()),
        };
        let updated = update_event(&client_for(&server), &starting).await.unwrap();
        // After series-wide write the kind flips to RecurringMaster
        // (`M:`), and the id holds the freshly rotated ChangeKey.
        assert_eq!(updated.id, "M:MASTER-ID|MCK-V2");
        assert_eq!(updated.etag.as_deref(), Some("MCK-V2"));
    }

    #[tokio::test]
    async fn add_event_exdate_targets_occurrence_directly_no_master_lookup() {
        // A single mock matching the delete envelope is enough — if
        // add_event_exdate accidentally resolved master, mockito would
        // raise "no matching mock for the GetItem POST" and fail the
        // test.
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <s:Body>
    <m:DeleteItemResponse>
      <m:ResponseMessages>
        <m:DeleteItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
        </m:DeleteItemResponseMessage>
      </m:ResponseMessages>
    </m:DeleteItemResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            // `DeleteType` only appears in DeleteItem envelopes —
            // if add_event_exdate accidentally resolved master and
            // sent a GetItem first, the mock wouldn't match and
            // mockito would return the default 501.
            .match_body(mockito::Matcher::Regex("DeleteType".into()))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        add_event_exdate(&client_for(&server), "O:OCC-ID|OCK")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_event_on_occurrence_resolves_master_then_deletes_series() {
        let mut server = Server::new_async().await;
        let master_lookup_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:GetItemResponse>
      <m:ResponseMessages>
        <m:GetItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:CalendarItem>
              <t:ItemId Id="MASTER-ID" ChangeKey="MCK"/>
            </t:CalendarItem>
          </m:Items>
        </m:GetItemResponseMessage>
      </m:ResponseMessages>
    </m:GetItemResponse>
  </s:Body>
</s:Envelope>"#;
        let delete_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <s:Body>
    <m:DeleteItemResponse>
      <m:ResponseMessages>
        <m:DeleteItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
        </m:DeleteItemResponseMessage>
      </m:ResponseMessages>
    </m:DeleteItemResponse>
  </s:Body>
</s:Envelope>"#;
        let _m1 = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("RecurringMasterItemId".into()))
            .with_status(200)
            .with_body(master_lookup_body)
            .create_async()
            .await;
        let _m2 = server
            .mock("POST", "/")
            // Body must mention MASTER-ID — proves we routed to the
            // master id returned by m1, not back to the occurrence.
            // MASTER-ID never appears in the GetItem request itself
            // (only the occurrence id does), so the discriminator is
            // unambiguous.
            .match_body(mockito::Matcher::Regex("MASTER-ID".into()))
            .with_status(200)
            .with_body(delete_body)
            .create_async()
            .await;
        delete_event(&client_for(&server), "O:OCC-ID|OCK")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rename_calendar_round_trips_against_server() {
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:UpdateFolderResponse>
      <m:ResponseMessages>
        <m:UpdateFolderResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Folders>
            <t:CalendarFolder>
              <t:FolderId Id="FOLDER-ID" ChangeKey="FCK-V2"/>
            </t:CalendarFolder>
          </m:Folders>
        </m:UpdateFolderResponseMessage>
      </m:ResponseMessages>
    </m:UpdateFolderResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(
                "DisplayName".to_string(),
            ))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        rename_calendar(&client_for(&server), "FOLDER-ID|FCK-V1", "Renamed")
            .await
            .unwrap();
    }
}
