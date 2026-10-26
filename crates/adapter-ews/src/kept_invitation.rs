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
//! 2026-10-26), and the appointment is read back as it now stands.

use base64::Engine as _;
use cal_core::attendee::{parse, same_address};
use cal_core::{Event, NewEvent};
use quick_xml::events::Event as XmlEvent;
use quick_xml::Reader;

use crate::api::EwsClient;
use crate::error::{EwsError, EwsResult};
use crate::mapping::{parse_get_calendar_items_response, split_calendar_id, to_event, ItemRef};
use crate::soap::{
    create_calendar_item, get_calendar_items_with_recurrence, respond_to_meeting, soft_delete_item,
};

/// The organizer of a create marked [`NewEvent::keeps_invitation`] when it is
/// someone else — `None` for an unmarked create, and for one the mailbox
/// (`own`) organizes itself, which is the mailbox's own meeting.
pub(crate) fn foreign_organizer(event: &NewEvent, own: Option<&str>) -> Option<String> {
    if !event.keeps_invitation {
        return None;
    }
    let organizer = event.organizer.as_deref()?.trim();
    let organizer = organizer
        .strip_prefix("mailto:")
        .or_else(|| organizer.strip_prefix("MAILTO:"))
        .unwrap_or(organizer)
        .trim();
    if organizer.is_empty() || own.is_some_and(|own| same_address(own, organizer)) {
        return None;
    }
    Some(organizer.to_string())
}

/// The invitees the request names: the file's, never the organizer, and the
/// mailbox among them where the file named it under another address or not at
/// all (Toni, 2026-10-26) — the reply that makes the appointment answers for
/// the mailbox's own row.
pub(crate) fn request_attendees(
    event: &NewEvent,
    organizer: &str,
    own: Option<&str>,
) -> Vec<String> {
    let mut attendees: Vec<String> = event
        .attendees
        .iter()
        .filter(|entry| {
            let address = parse(entry).1;
            !address.trim().is_empty() && !same_address(&address, organizer)
        })
        .cloned()
        .collect();
    if let Some(own) = own.map(str::trim).filter(|own| !own.is_empty()) {
        if !attendees
            .iter()
            .any(|entry| same_address(&parse(entry).1, own))
        {
            attendees.push(own.to_string());
        }
    }
    attendees
}

/// Create someone else's meeting in `calendar_id` the way Exchange takes one.
pub(crate) async fn create(
    client: &EwsClient,
    calendar_id: &str,
    event: &NewEvent,
    organizer: &str,
    own: Option<&str>,
) -> EwsResult<Event> {
    let (folder_id, folder_change_key) = split_calendar_id(calendar_id);
    let uid = event
        .uid_to_keep
        .as_deref()
        .map(str::trim)
        .filter(|uid| !uid.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}@aperio", uuid::Uuid::new_v4()));
    let attendees = request_attendees(event, organizer, own);
    let request = ical_mapping::mapping::invitation_request(&uid, event, organizer, &attendees);
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

    /// Only a marked create is someone else's, and only while somebody else
    /// organizes it: the mailbox's own meeting goes in as its own.
    #[test]
    fn an_invitation_is_someone_elses_only_when_marked_and_foreign() {
        let event = invitation(&[]);
        assert_eq!(
            foreign_organizer(&event, Some("toni@example.org")).as_deref(),
            Some("Boss@Example.net")
        );
        assert_eq!(
            foreign_organizer(&event, None).as_deref(),
            Some("Boss@Example.net")
        );
        assert_eq!(foreign_organizer(&event, Some("boss@example.net")), None);
        let unmarked = NewEvent {
            keeps_invitation: false,
            ..event.clone()
        };
        assert_eq!(foreign_organizer(&unmarked, None), None);
        let nobody = NewEvent {
            organizer: Some("  ".into()),
            ..event
        };
        assert_eq!(foreign_organizer(&nobody, None), None);
    }

    /// The request names the file's invitees, never the organizer, and the
    /// mailbox where the file did not.
    #[test]
    fn the_request_names_the_mailbox_and_never_the_organizer() {
        let event = invitation(&["boss@example.net", "Gast <gast@example.net>"]);
        assert_eq!(
            request_attendees(&event, "Boss@Example.net", Some("toni@example.org")),
            ["Gast <gast@example.net>", "toni@example.org"]
        );
        let named = invitation(&["Toni <TONI@example.org>"]);
        assert_eq!(
            request_attendees(&named, "boss@example.net", Some("toni@example.org")),
            ["Toni <TONI@example.org>"]
        );
        assert_eq!(
            request_attendees(&named, "boss@example.net", None),
            ["Toni <TONI@example.org>"]
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

    /// The whole way in: the request created, the tentative reply saved, the
    /// moved request soft-deleted, the appointment read back as someone
    /// else's.
    #[tokio::test]
    async fn a_kept_invitation_goes_in_as_a_request_and_a_tentative_reply() {
        let mut server = Server::new_async().await;
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
        let read = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("<m:GetItem>".into()),
                mockito::Matcher::Regex("<t:ItemId Id=\"APPT\" ChangeKey=\"CK1\"/>".into()),
            ]))
            .with_status(200)
            .with_body(APPOINTMENT)
            .expect(1)
            .create_async()
            .await;

        let client = EwsClient::new(
            server.url(),
            BasicCredentials {
                username: "toni@example.org".into(),
                password: "pw".into(),
            },
            reqwest::Client::builder().build().unwrap(),
        );
        let created = crate::api::create_event(&client, "FOLDER|FCK", invitation(&[]), None)
            .await
            .unwrap();
        request.assert_async().await;
        reply.assert_async().await;
        removed.assert_async().await;
        read.assert_async().await;
        assert_eq!(created.organizer.as_deref(), Some("Boss@Example.net"));
        assert!(created.organized_elsewhere);
        assert_eq!(created.ical_uid.as_deref(), Some("jf-1@example.net"));
        assert_eq!(created.title, "Jour fixe");
    }
}
