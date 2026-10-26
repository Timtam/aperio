//! A meeting someone else organizes, imported from a file, kept as theirs and
//! telling nobody (DESIGN-file-import.md, I6).
//!
//! Exchange will not take an appointment with somebody else's organizer: the
//! ordinary `CreateItem` refuses an `Organizer` field (live round ICS),
//! and a meeting it makes is the mailbox's own (ICS-3). What it takes is the
//! way such a meeting arrives by mail — the organizer's request, `METHOD:
//! REQUEST` in `MimeContent` —, which becomes a meeting request MESSAGE in
//! the folder (ICS-2). A tentative reply to it, saved and not sent, makes the
//! appointment: their organizer, the invitees, the file's UID, the mailbox's
//! answer tentative, nothing sent, and the request moved to Deleted Items
//! (ICS-3, ICS-4). That request is removed from there recoverably (Toni,
//! 2026-10-26). Exchange files the appointment in the default calendar,
//! whichever folder the request went to (ICS-6), so in any other folder it is
//! moved there afterwards (Toni, 2026-10-26). Then it is read back as it now
//! stands.

use base64::Engine as _;
use cal_core::{Event, NewEvent};
use quick_xml::events::Event as XmlEvent;
use quick_xml::Reader;

use crate::api::EwsClient;
use crate::error::{EwsError, EwsResult};
use crate::mapping::{
    parse_first_item_id, parse_get_calendar_items_response, split_calendar_id, to_event, ItemRef,
};
use crate::soap::{
    create_calendar_item, delete_calendar_item, get_calendar_items_with_recurrence,
    get_item_parent_folder, move_item, respond_to_meeting, soft_delete_item,
};

/// The request `organizer` would have sent for `event`: under the file's UID,
/// or one of its own for a copy imported without it, naming
/// [`cal_core::attendee::invitation_attendees`] — the mailbox among them, as
/// the reply that makes the appointment answers for its row.
pub(crate) fn request_of(event: &NewEvent, organizer: &str, own: Option<&str>) -> String {
    let uid = event
        .uid_to_keep
        .as_deref()
        .map(str::trim)
        .filter(|uid| !uid.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}@aperio", uuid::Uuid::new_v4()));
    let attendees = cal_core::attendee::invitation_attendees(&event.attendees, organizer, own);
    ical_mapping::mapping::invitation_request(&uid, event, organizer, &attendees)
}

/// Create someone else's meeting in `calendar_id` the way Exchange takes one.
/// `organizer` is [`cal_core::attendee::invitation_organizer`]'s answer.
pub(crate) async fn create(
    client: &EwsClient,
    calendar_id: &str,
    event: &NewEvent,
    organizer: &str,
    own: Option<&str>,
) -> EwsResult<Event> {
    let (folder_id, folder_change_key) = split_calendar_id(calendar_id);
    let request = request_of(event, organizer, own);
    let mime = base64::engine::general_purpose::STANDARD.encode(request.as_bytes());
    let item = format!(
        "        <t:CalendarItem>\n          <t:MimeContent CharacterSet=\"UTF-8\">{mime}</t:MimeContent>\n        </t:CalendarItem>\n"
    );
    let made = client
        .post_soap(create_calendar_item(
            &folder_id,
            folder_change_key.as_deref(),
            &item,
            false,
        ))
        .await?;
    let request_ref = crate::mapping::parse_first_item_id(&made)?;

    let reply = client
        .post_soap(respond_to_meeting(
            &request_ref.id,
            request_ref.change_key.as_deref(),
            "TentativelyAcceptItem",
            false,
        ))
        .await?;
    let made = parse_reply_items(&reply)?;
    let appointment = made.appointment.ok_or_else(|| {
        EwsError::Protocol("the tentative reply made no appointment of the request".into())
    })?;
    if let Some(moved) = made.request {
        // Out of Deleted Items, recoverably. The appointment stands either
        // way; a request left there is clutter, not a fault.
        if let Err(err) = client.post_soap(soft_delete_item(&moved.id)).await {
            tracing::warn!(target: "adapter_ews::invitation", %err, "the imported invitation's request stayed in Deleted Items");
        }
    }
    let appointment = into_folder(client, appointment, &folder_id).await?;

    let detail = client
        .post_soap(get_calendar_items_with_recurrence(&[(
            appointment.id.clone(),
            appointment.change_key.clone(),
        )]))
        .await?;
    let item = parse_get_calendar_items_response(&detail)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            EwsError::Protocol("the imported invitation could not be read back".into())
        })?;
    to_event(item, calendar_id)
}

/// `appointment` in `folder_id`, moved there when Exchange filed it elsewhere
/// (live round ICS-6: always the default calendar). A move that fails takes
/// the appointment out again, recoverably and telling nobody (as an attendee's
/// delete never does, ICS-4), so the import lands where it was asked to or
/// not at all.
async fn into_folder(
    client: &EwsClient,
    appointment: ItemRef,
    folder_id: &str,
) -> EwsResult<ItemRef> {
    let answer = client
        .post_soap(get_item_parent_folder(&appointment.id))
        .await?;
    if parse_parent_folder_id(&answer)? == folder_id {
        return Ok(appointment);
    }
    match client
        .post_soap(move_item(&appointment.id, folder_id))
        .await
    {
        Ok(moved) => parse_first_item_id(&moved),
        Err(err) => {
            if let Err(cleanup) = client
                .post_soap(delete_calendar_item(&appointment.id, None, false))
                .await
            {
                tracing::warn!(target: "adapter_ews::invitation", %cleanup, "the imported invitation stayed in the default calendar after its move failed");
            }
            Err(err)
        }
    }
}

/// The `Id` of the first `ParentFolderId` in a `GetItem` answer.
pub(crate) fn parse_parent_folder_id(xml: &str) -> EwsResult<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                if e.local_name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"ParentFolderId")
                {
                    for a in e.attributes().flatten() {
                        if a.key.as_ref().eq_ignore_ascii_case(b"Id") {
                            return Ok(String::from_utf8_lossy(&a.value).into_owned());
                        }
                    }
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => return Err(EwsError::Protocol(format!("xml parse: {err}"))),
            _ => {}
        }
        buf.clear();
    }
    Err(EwsError::Protocol(
        "the answer named no folder the item is in".into(),
    ))
}

/// What a tentative reply made: the appointment, and the request it moved.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ReplyItems {
    pub appointment: Option<ItemRef>,
    pub request: Option<ItemRef>,
}

/// The `CreateItem` answer to a reply: a `CalendarItem` and the
/// `MeetingRequest` it was made from, each by its `ItemId`.
pub(crate) fn parse_reply_items(xml: &str) -> EwsResult<ReplyItems> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut inside: Option<&'static str> = None;
    let mut out = ReplyItems::default();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                match local.as_slice() {
                    b"calendaritem" => inside = Some("appointment"),
                    b"meetingrequest" => inside = Some("request"),
                    b"itemid" => {
                        let mut id = String::new();
                        let mut change_key = None;
                        for a in e.attributes().flatten() {
                            let key = a.key.as_ref();
                            if key.eq_ignore_ascii_case(b"Id") {
                                id = String::from_utf8_lossy(&a.value).into_owned();
                            } else if key.eq_ignore_ascii_case(b"ChangeKey") {
                                change_key = Some(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                        if !id.is_empty() {
                            let item = Some(ItemRef { id, change_key });
                            match inside {
                                Some("appointment") if out.appointment.is_none() => {
                                    out.appointment = item
                                }
                                Some("request") if out.request.is_none() => out.request = item,
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"calendaritem" || local == b"meetingrequest" {
                    inside = None;
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => return Err(EwsError::Protocol(format!("xml parse: {err}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::BasicCredentials;
    use mockito::Server;

    fn invitation(attendees: &[&str]) -> NewEvent {
        NewEvent {
            organized_elsewhere: true,
            organizer: Some("mailto:Boss@Example.net".into()),
            title: "Jour fixe".into(),
            description: None,
            location: None,
            start: "2026-11-24T09:00:00Z".parse().unwrap(),
            end: "2026-11-24T10:00:00Z".parse().unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: attendees.iter().map(|a| a.to_string()).collect(),
            send_invitations: false,
            uid_to_keep: Some("jf-1@example.net".into()),
            keeps_invitation: true,
        }
    }

    /// The request carries the file's UID, its organizer, and the invitees
    /// without the organizer, the mailbox added; a copy imported without the
    /// UID gets one of its own.
    #[test]
    fn the_request_names_the_mailbox_under_the_files_uid() {
        let event = invitation(&["boss@example.net", "Gast <gast@example.net>"]);
        let text = request_of(&event, "Boss@Example.net", Some("toni@example.org"));
        let file = ical_mapping::mapping::read_calendar_file(&text).unwrap();
        assert_eq!(file.method.as_deref(), Some("REQUEST"));
        let [read] = file.events.as_slice() else {
            panic!("{text}")
        };
        assert_eq!(read.ical_uid.as_deref(), Some("jf-1@example.net"));
        assert_eq!(read.organizer.as_deref(), Some("Boss@Example.net"));
        assert_eq!(
            read.attendees,
            ["Gast <gast@example.net>", "toni@example.org"]
        );

        let copy = NewEvent {
            uid_to_keep: None,
            ..event
        };
        let text = request_of(&copy, "Boss@Example.net", None);
        let file = ical_mapping::mapping::read_calendar_file(&text).unwrap();
        let uid = file.events[0].ical_uid.clone().unwrap();
        assert!(
            uid.ends_with("@aperio") && uid != "jf-1@example.net",
            "{uid}"
        );
    }

    /// The answer to the reply as Exchange 2019 gave it (live run 080508):
    /// the appointment, then the request it moved.
    #[test]
    fn the_reply_names_the_appointment_and_the_moved_request() {
        let xml = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:CreateItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages><m:CreateItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Items><t:CalendarItem><t:ItemId Id="APPT" ChangeKey="CK1"/></t:CalendarItem><t:MeetingRequest><t:ItemId Id="REQ-MOVED" ChangeKey="CK2"/></t:MeetingRequest></m:Items></m:CreateItemResponseMessage></m:ResponseMessages></m:CreateItemResponse></s:Body></s:Envelope>"#;
        let made = parse_reply_items(xml).unwrap();
        assert_eq!(made.appointment.map(|r| r.id).as_deref(), Some("APPT"));
        assert_eq!(made.request.map(|r| r.id).as_deref(), Some("REQ-MOVED"));
    }

    const OK_DELETE: &str = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:DeleteItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"><m:ResponseMessages><m:DeleteItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode></m:DeleteItemResponseMessage></m:ResponseMessages></m:DeleteItemResponse></s:Body></s:Envelope>"#;

    const REQUEST_MADE: &str = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:CreateItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages><m:CreateItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Items><t:MeetingRequest><t:ItemId Id="REQ" ChangeKey="CK0"/></t:MeetingRequest></m:Items></m:CreateItemResponseMessage></m:ResponseMessages></m:CreateItemResponse></s:Body></s:Envelope>"#;

    const REPLY_MADE: &str = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:CreateItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages><m:CreateItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Items><t:CalendarItem><t:ItemId Id="APPT" ChangeKey="CK1"/></t:CalendarItem><t:MeetingRequest><t:ItemId Id="REQ-MOVED" ChangeKey="CK2"/></t:MeetingRequest></m:Items></m:CreateItemResponseMessage></m:ResponseMessages></m:CreateItemResponse></s:Body></s:Envelope>"#;

    const APPOINTMENT: &str = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:GetItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages><m:GetItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Items><t:CalendarItem><t:ItemId Id="APPT" ChangeKey="CK1"/><t:Subject>Jour fixe</t:Subject><t:Start>2026-11-24T09:00:00Z</t:Start><t:End>2026-11-24T10:00:00Z</t:End><t:CalendarItemType>Single</t:CalendarItemType><t:Organizer><t:Mailbox><t:EmailAddress>Boss@Example.net</t:EmailAddress></t:Mailbox></t:Organizer><t:MyResponseType>Tentative</t:MyResponseType><t:RequiredAttendees><t:Attendee><t:Mailbox><t:EmailAddress>toni@example.org</t:EmailAddress></t:Mailbox><t:ResponseType>Tentative</t:ResponseType></t:Attendee></t:RequiredAttendees><t:UID>jf-1@example.net</t:UID></t:CalendarItem></m:Items></m:GetItemResponseMessage></m:ResponseMessages></m:GetItemResponse></s:Body></s:Envelope>"#;

    /// The `GetItem` answer naming the folder an item is in.
    fn parent_answer(folder: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:GetItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages><m:GetItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Items><t:CalendarItem><t:ItemId Id="APPT" ChangeKey="CK1"/><t:ParentFolderId Id="{folder}" ChangeKey="FCK9"/></t:CalendarItem></m:Items></m:GetItemResponseMessage></m:ResponseMessages></m:GetItemResponse></s:Body></s:Envelope>"#
        )
    }

    const MOVED: &str = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:MoveItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages><m:MoveItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Items><t:CalendarItem><t:ItemId Id="APPT-MOVED" ChangeKey="CK3"/></t:CalendarItem></m:Items></m:MoveItemResponseMessage></m:ResponseMessages></m:MoveItemResponse></s:Body></s:Envelope>"#;

    const MOVE_REFUSED: &str = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:MoveItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages><m:MoveItemResponseMessage ResponseClass="Error"><m:MessageText>The specified folder could not be found in the store.</m:MessageText><m:ResponseCode>ErrorFolderNotFound</m:ResponseCode></m:MoveItemResponseMessage></m:ResponseMessages></m:MoveItemResponse></s:Body></s:Envelope>"#;

    /// The request, the tentative reply and the soft delete of the moved
    /// request, each expected once, and the question where the appointment
    /// is (by its id alone), answered with `parent`.
    async fn way_in(server: &mut mockito::ServerGuard, parent: &str) -> Vec<mockito::Mock> {
        let request = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("<m:CreateItem".into()),
                mockito::Matcher::Regex("SendMeetingInvitations=\"SendToNone\"".into()),
                mockito::Matcher::Regex("<t:MimeContent CharacterSet=\"UTF-8\">".into()),
            ]))
            .with_status(200)
            .with_body(REQUEST_MADE)
            .expect(1)
            .create_async()
            .await;
        let reply = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("<t:TentativelyAcceptItem>".into()),
                mockito::Matcher::Regex("MessageDisposition=\"SaveOnly\"".into()),
                mockito::Matcher::Regex("<t:ReferenceItemId Id=\"REQ\" ChangeKey=\"CK0\"/>".into()),
            ]))
            .with_status(200)
            .with_body(REPLY_MADE)
            .expect(1)
            .create_async()
            .await;
        let removed = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("DeleteType=\"SoftDelete\"".into()),
                mockito::Matcher::Regex("<t:ItemId Id=\"REQ-MOVED\"/>".into()),
            ]))
            .with_status(200)
            .with_body(OK_DELETE)
            .expect(1)
            .create_async()
            .await;
        let asked = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("FieldURI=\"item:ParentFolderId\"".into()),
                mockito::Matcher::Regex("<t:ItemId Id=\"APPT\"/>".into()),
            ]))
            .with_status(200)
            .with_body(parent_answer(parent))
            .expect(1)
            .create_async()
            .await;
        vec![request, reply, removed, asked]
    }

    /// The appointment read back by its id and change key, as Exchange
    /// answers it.
    async fn read_back(
        server: &mut mockito::ServerGuard,
        item_id: &str,
        times: usize,
    ) -> mockito::Mock {
        server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("<m:GetItem>".into()),
                mockito::Matcher::Regex(item_id.into()),
            ]))
            .with_status(200)
            .with_body(APPOINTMENT)
            .expect(times)
            .create_async()
            .await
    }

    fn client_of(server: &mockito::ServerGuard) -> EwsClient {
        EwsClient::new(
            server.url(),
            BasicCredentials {
                username: "toni@example.org".into(),
                password: "pw".into(),
            },
            reqwest::Client::builder().build().unwrap(),
        )
    }

    /// The whole way in: the request created, the tentative reply saved, the
    /// moved request soft-deleted, the appointment found in the folder asked
    /// for and read back as someone else's.
    #[tokio::test]
    async fn a_kept_invitation_goes_in_as_a_request_and_a_tentative_reply() {
        let mut server = Server::new_async().await;
        let way = way_in(&mut server, "FOLDER").await;
        let moved = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("<m:MoveItem>".into()))
            .expect(0)
            .create_async()
            .await;
        let read = read_back(&mut server, "<t:ItemId Id=\"APPT\" ChangeKey=\"CK1\"/>", 1).await;

        let created =
            crate::api::create_event(&client_of(&server), "FOLDER|FCK", invitation(&[]), None)
                .await
                .unwrap();
        for mock in way {
            mock.assert_async().await;
        }
        moved.assert_async().await;
        read.assert_async().await;
        assert_eq!(created.organizer.as_deref(), Some("Boss@Example.net"));
        assert!(created.organized_elsewhere);
        assert_eq!(created.ical_uid.as_deref(), Some("jf-1@example.net"));
        assert_eq!(created.title, "Jour fixe");
    }

    /// Filed in the default calendar (ICS-6), the appointment is moved into
    /// the folder asked for and read back under its new id there.
    #[tokio::test]
    async fn an_invitation_filed_in_the_default_calendar_is_moved_to_the_folder_asked_for() {
        let mut server = Server::new_async().await;
        let way = way_in(&mut server, "DEFAULT").await;
        let moved = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("<m:MoveItem>".into()),
                mockito::Matcher::Regex("<m:ToFolderId>\\s*<t:FolderId Id=\"FOLDER\"/>".into()),
                mockito::Matcher::Regex("<t:ItemId Id=\"APPT\"/>".into()),
            ]))
            .with_status(200)
            .with_body(MOVED)
            .expect(1)
            .create_async()
            .await;
        let read = read_back(
            &mut server,
            "<t:ItemId Id=\"APPT-MOVED\" ChangeKey=\"CK3\"/>",
            1,
        )
        .await;

        let created =
            crate::api::create_event(&client_of(&server), "FOLDER|FCK", invitation(&[]), None)
                .await
                .unwrap();
        for mock in way {
            mock.assert_async().await;
        }
        moved.assert_async().await;
        read.assert_async().await;
        assert_eq!(created.calendar_id, "FOLDER|FCK");
        assert!(created.organized_elsewhere);
    }

    /// A move Exchange refuses takes the appointment out of the default
    /// calendar again, telling nobody, and the create fails: nothing stands
    /// where it was not asked for.
    #[tokio::test]
    async fn a_refused_move_takes_the_appointment_out_again() {
        let mut server = Server::new_async().await;
        let way = way_in(&mut server, "DEFAULT").await;
        let moved = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("<m:MoveItem>".into()))
            .with_status(200)
            .with_body(MOVE_REFUSED)
            .expect(1)
            .create_async()
            .await;
        let taken_out = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("DeleteType=\"MoveToDeletedItems\"".into()),
                mockito::Matcher::Regex("SendMeetingCancellations=\"SendToNone\"".into()),
                mockito::Matcher::Regex("<t:ItemId Id=\"APPT\"/>".into()),
            ]))
            .with_status(200)
            .with_body(OK_DELETE)
            .expect(1)
            .create_async()
            .await;
        let read = read_back(&mut server, "<t:ItemId Id=\"APPT\" ChangeKey=\"CK1\"/>", 0).await;

        let failed =
            crate::api::create_event(&client_of(&server), "FOLDER|FCK", invitation(&[]), None)
                .await;
        assert!(failed.is_err(), "{failed:?}");
        for mock in way {
            mock.assert_async().await;
        }
        moved.assert_async().await;
        taken_out.assert_async().await;
        read.assert_async().await;
    }

    #[test]
    fn the_folder_an_item_is_in_is_read_from_its_answer() {
        assert_eq!(parse_parent_folder_id(&parent_answer("F1")).unwrap(), "F1");
        assert!(parse_parent_folder_id(MOVED).is_err());
    }
}
