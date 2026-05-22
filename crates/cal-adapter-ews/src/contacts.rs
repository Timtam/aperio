//! EWS Contacts (`IPF.Contact` folder class + `<t:Contact>` items) —
//! Phase 10e.
//!
//! Contacts are Exchange's standalone address-book item type, living
//! in their own folder class (`IPF.Contact`) parallel to
//! `IPF.Appointment` / `IPF.Task`. The wire model has a couple of
//! quirks compared to vCard / Aperio's flat `Contact` shape:
//!
//!   - **Indexed properties.** EWS doesn't store a list of email
//!     addresses; it stores three keyed slots (`EmailAddress1`,
//!     `EmailAddress2`, `EmailAddress3`). Phone numbers are similar
//!     but with a richer key vocabulary (`BusinessPhone`,
//!     `HomePhone`, `MobilePhone`, `OtherTelephone`, …). Aperio
//!     stores emails / phones as `Vec<String>`, so we round-trip
//!     them through the indexed slots in a fixed order — slot
//!     positions get reassigned on each write, but the *values* are
//!     preserved.
//!
//!   - **Birthday is `xs:dateTime`.** EWS treats birthday as a
//!     full timestamp even though every UI in the wild renders it
//!     as date-only. We write `00:00:00Z` and read the date part,
//!     same convention the task adapter uses for `StartDate` /
//!     `DueDate`.
//!
//!   - **DisplayName vs FileAs.** EWS distinguishes
//!     `<t:DisplayName>` (the rendered name) from `<t:FileAs>` (the
//!     sort key Outlook uses in its contacts list). Aperio's single
//!     `display_name` maps onto both — we write it to `DisplayName`
//!     and `FileAs` on create/update so Outlook sorts the contact
//!     where the user expects, and we read `DisplayName` first then
//!     fall back to `FileAs` if a server-side import left the
//!     former blank.
//!
//!   - **CompleteName.** Read-only structural element wrapping
//!     `GivenName`, `Surname`, `MiddleName`, etc. We ignore it on
//!     read (the individual elements are already available outside)
//!     and never emit it.
//!
//! Out of scope for the first cut:
//!
//!   - **Distribution lists** (`<t:DistributionList>`). Aperio
//!     doesn't model group contacts yet.
//!   - **Photos.** EWS exposes contact photos as attachments —
//!     adding them is a separate `CreateAttachment` round-trip and
//!     the design document doesn't call for them in §10.
//!   - **Physical addresses.** EWS supports indexed
//!     `PhysicalAddresses` (`Home`, `Work`, `Other`) but cal-core's
//!     `Contact` has no address slot yet, so we drop them.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader;

use cal_core::{Contact, ContactList, GroupMember, NewContact};

use crate::api::EwsClient;
use crate::error::{EwsError, EwsResult};
use crate::mapping::{parse_first_item_id, split_calendar_id};
use crate::soap::{delete_calendar_item, escape_xml};

// ── Public adapter-side surface ────────────────────────────────────────

/// Enumerate every contact folder in the user's mailbox. Same
/// `FindFolder` flow as `list_calendars` / `list_task_lists`, just
/// with `IPF.Contact` as the folder-class restriction.
pub async fn list_contact_lists(client: &EwsClient) -> EwsResult<Vec<ContactList>> {
    let body = find_contact_folders();
    let xml = client.post_soap(body).await?;
    let folders = parse_find_contact_folder_response(&xml)?;
    Ok(folders.into_iter().map(to_contact_list).collect())
}

/// Pull every contact in `list_id`. No date filter equivalent for
/// contacts (the closest thing — `LastModifiedTime` ranges — is
/// useful for delta sync but not the initial listing), so we ask
/// for every row in the folder via a `Shallow` traversal.
pub async fn get_contacts(client: &EwsClient, list_id: &str) -> EwsResult<Vec<Contact>> {
    let (folder_id, change_key) = split_calendar_id(list_id);
    let body = find_contacts_in_folder(&folder_id, change_key.as_deref());
    let xml = client.post_soap(body).await?;
    let parsed = parse_find_contact_item_response(&xml)?;
    Ok(parsed
        .into_iter()
        .map(|item| to_contact(item, list_id))
        .collect())
}

/// Create a new contact in `list_id`. Same pattern as
/// `create_task` — render the `<t:Contact>` payload, post the
/// envelope, harvest the assigned ItemId, synthesise the returned
/// `Contact` from the request fields so we don't need a follow-up
/// GetItem.
pub async fn create_contact(
    client: &EwsClient,
    list_id: &str,
    contact: NewContact,
) -> EwsResult<Contact> {
    let (folder_id, folder_change_key) = split_calendar_id(list_id);
    let item_xml = new_contact_to_contact_item_xml(&contact);
    let envelope = create_contact_in_folder(
        &folder_id,
        folder_change_key.as_deref(),
        &item_xml,
    );
    let response = client.post_soap(envelope).await?;
    let item_ref = parse_first_item_id(&response)?;
    Ok(build_contact_from_new(
        &contact,
        list_id,
        &item_ref.id,
        item_ref.change_key,
    ))
}

/// Update an existing contact. Every field is set or deleted
/// explicitly so EWS clears anything the user removed — matches the
/// task adapter's "what you see is what you get" semantic and keeps
/// the round-trip with the Aperio UI lossless.
pub async fn update_contact(client: &EwsClient, contact: &Contact) -> EwsResult<Contact> {
    let (item_id, change_key) = split_calendar_id(&contact.id);
    let (set_xml, delete_xml) = contact_to_update_field_xml(contact);
    let envelope = update_contact_item(
        &item_id,
        change_key.as_deref(),
        &set_xml,
        &delete_xml,
    );
    let response = client.post_soap(envelope).await?;
    let item_ref = parse_first_item_id(&response)?;
    let new_id = encode_contact_id(&item_ref.id, item_ref.change_key.as_deref());
    Ok(Contact {
        id: new_id,
        etag: item_ref.change_key,
        updated_at: Utc::now(),
        ..contact.clone()
    })
}

/// Delete a contact. `DeleteItem` is type-agnostic so we reuse the
/// calendar envelope — the server doesn't care whether the id
/// points at an appointment, a task, or a contact.
pub async fn delete_contact(client: &EwsClient, contact_id: &str) -> EwsResult<()> {
    let (item_id, change_key) = split_calendar_id(contact_id);
    let envelope = delete_calendar_item(&item_id, change_key.as_deref());
    client.post_soap(envelope).await?;
    Ok(())
}

/// Rename a contact folder. EWS wants the `<t:ContactsFolder>`
/// wrapper inside `SetFolderField` so the server applies the change
/// to the right folder kind.
pub async fn rename_contact_list(
    client: &EwsClient,
    list_id: &str,
    new_name: &str,
) -> EwsResult<()> {
    let (folder_id, change_key) = split_calendar_id(list_id);
    let envelope =
        update_contacts_folder_displayname(&folder_id, change_key.as_deref(), new_name);
    client.post_soap(envelope).await?;
    Ok(())
}

// ── SOAP envelope helpers ──────────────────────────────────────────────

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

/// SOAP body for `FindFolder` restricted to `IPF.Contact`. `Deep`
/// traversal matches the calendar / tasks pattern so subfolders
/// surface alongside top-level address books.
pub fn find_contact_folders() -> String {
    let body = r#"    <m:FindFolder Traversal="Deep">
      <m:FolderShape>
        <t:BaseShape>AllProperties</t:BaseShape>
      </m:FolderShape>
      <m:Restriction>
        <t:IsEqualTo>
          <t:FieldURI FieldURI="folder:FolderClass"/>
          <t:FieldURIOrConstant>
            <t:Constant Value="IPF.Contact"/>
          </t:FieldURIOrConstant>
        </t:IsEqualTo>
      </m:Restriction>
      <m:ParentFolderIds>
        <t:DistinguishedFolderId Id="msgfolderroot"/>
      </m:ParentFolderIds>
    </m:FindFolder>"#;
    wrap(body)
}

/// SOAP body for `FindItem` over a contact folder. We pull the
/// contact-specific fields up front so the mapper can build the
/// cal-core `Contact` without a per-row GetItem.
pub fn find_contacts_in_folder(folder_id: &str, change_key: Option<&str>) -> String {
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
          <t:FieldURI FieldURI="contacts:DisplayName"/>
          <t:FieldURI FieldURI="contacts:FileAs"/>
          <t:FieldURI FieldURI="contacts:GivenName"/>
          <t:FieldURI FieldURI="contacts:Surname"/>
          <t:FieldURI FieldURI="contacts:CompanyName"/>
          <t:FieldURI FieldURI="contacts:EmailAddresses"/>
          <t:FieldURI FieldURI="contacts:PhoneNumbers"/>
          <t:FieldURI FieldURI="contacts:Birthday"/>
          <t:FieldURI FieldURI="distributionlist:Members"/>
        </t:AdditionalProperties>
      </m:ItemShape>
      <m:ParentFolderIds>
        {folder_id_attr}
      </m:ParentFolderIds>
    </m:FindItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `CreateItem` into a contact folder. Wraps the
/// pre-rendered `<t:Contact>` payload, pinning `SavedItemFolderId`
/// so the server files the contact under the right book rather than
/// the user's default address book.
fn create_contact_in_folder(
    folder_id: &str,
    change_key: Option<&str>,
    contact_item_xml: &str,
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
        r#"    <m:CreateItem MessageDisposition="SaveOnly">
      <m:SavedItemFolderId>
        {folder_id_attr}
      </m:SavedItemFolderId>
      <m:Items>
{contact_item_xml}
      </m:Items>
    </m:CreateItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `UpdateItem` against a Contact. Almost identical
/// to the task / calendar envelope, just without the meeting-
/// invitation flags (contacts don't have RSVPs).
fn update_contact_item(
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
                   MessageDisposition="SaveOnly">
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

/// `UpdateFolder` for the contact-folder display name. Outlook
/// rejects the request with `ErrorObjectTypeChanged` if the wrapper
/// doesn't match the folder's actual type, so we use
/// `<t:ContactsFolder>` (not `CalendarFolder` / `TasksFolder`).
fn update_contacts_folder_displayname(
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
              <t:ContactsFolder>
                <t:DisplayName>{name}</t:DisplayName>
              </t:ContactsFolder>
            </t:SetFolderField>
          </t:Updates>
        </t:FolderChange>
      </m:FolderChanges>
    </m:UpdateFolder>"#,
    );
    wrap(&body)
}

// ── Parsers ────────────────────────────────────────────────────────────

/// One contact folder pulled from a `FindFolder` response.
#[derive(Debug, Clone)]
pub struct ParsedContactFolder {
    pub folder_id: String,
    pub change_key: Option<String>,
    pub display_name: String,
}

/// Walk a `FindFolderResponse` body emitted with the `IPF.Contact`
/// restriction and yield one `ParsedContactFolder` per
/// `<t:ContactsFolder>` block.
pub fn parse_find_contact_folder_response(
    xml: &str,
) -> EwsResult<Vec<ParsedContactFolder>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut folders = Vec::new();
    let mut inside_folder = false;
    let mut current = ParsedContactFolder {
        folder_id: String::new(),
        change_key: None,
        display_name: String::new(),
    };
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"contactsfolder" {
                    inside_folder = true;
                    current = ParsedContactFolder {
                        folder_id: String::new(),
                        change_key: None,
                        display_name: String::new(),
                    };
                }
                if inside_folder && local == b"folderid" {
                    for a in e.attributes().flatten() {
                        let key = a.key.as_ref();
                        if key.eq_ignore_ascii_case(b"Id") {
                            current.folder_id =
                                String::from_utf8_lossy(&a.value).into_owned();
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
                if local == b"contactsfolder" {
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

/// One contact item pulled from a `FindItem` response.
#[derive(Debug, Clone, Default)]
pub struct ParsedContact {
    pub item_id: String,
    pub change_key: Option<String>,
    pub display_name: String,
    pub file_as: Option<String>,
    pub given_name: Option<String>,
    pub surname: Option<String>,
    pub company_name: Option<String>,
    pub body: Option<String>,
    pub birthday: Option<DateTime<Utc>>,
    /// Email addresses in ascending key order (`EmailAddress1`,
    /// `EmailAddress2`, `EmailAddress3`).
    pub emails: Vec<String>,
    /// Phone numbers, deduplicated (same number may appear under
    /// multiple keys) and in EWS's natural iteration order.
    pub phone_numbers: Vec<String>,
    /// Distribution-list members. `None` ⇒ this row is a person
    /// (`<t:Contact>`); `Some` ⇒ this row is a group
    /// (`<t:DistributionList>`), where the empty vec is the
    /// "freshly created empty group" case.
    pub members: Option<Vec<GroupMember>>,
    pub created: Option<DateTime<Utc>>,
    pub last_modified: Option<DateTime<Utc>>,
}

/// Walk a `FindItemResponse` body whose `<t:Items>` carries
/// `<t:Contact>` rows. Indexed properties (`EmailAddresses`,
/// `PhoneNumbers`) need an extra layer of state because each entry
/// is `<t:Entry Key="…">value</t:Entry>` rather than a single
/// element with the key embedded in the tag name.
pub fn parse_find_contact_item_response(xml: &str) -> EwsResult<Vec<ParsedContact>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut items = Vec::new();
    let mut inside_item = false;
    let mut current = ParsedContact::default();
    let mut text_target: Option<&'static str> = None;
    // Tracks which indexed collection we're inside: `Some("email")`,
    // `Some("phone")`, `Some("members")`, or `None`. We route entries
    // by collection rather than by Key attribute — Aperio's flat
    // phone / email vecs don't distinguish HomePhone from MobilePhone,
    // so the key only matters when we write back (at which point
    // `phone_key_for_slot` picks a sensible default).
    let mut in_collection: Option<&'static str> = None;
    let mut current_entry_value = String::new();
    // Distribution-list parsing state. EWS emits `<t:DistributionList>`
    // rows in the same `<m:Items>` collection as `<t:Contact>` rows;
    // we flag the in-flight ParsedContact as a group when we open one,
    // and accumulate `<t:Mailbox>` blocks inside `<t:Members>` into
    // GroupMember entries. The Mailbox shape is
    // `<t:Mailbox><t:Name>…</t:Name><t:EmailAddress>…</t:EmailAddress></t:Mailbox>`
    // — we keep the in-flight name + email and finalise on Mailbox-End.
    let mut current_member_name: Option<String> = None;
    let mut current_member_email: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"contact" {
                    inside_item = true;
                    current = ParsedContact::default();
                    continue;
                }
                if local == b"distributionlist" {
                    // Same in-flight slot as Contact but flagged as
                    // a group up front; the rest of the parser
                    // populates display_name + Members + timestamps
                    // identically.
                    inside_item = true;
                    current = ParsedContact::default();
                    current.members = Some(Vec::new());
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
                    b"displayname" => text_target = Some("display_name"),
                    b"fileas" => text_target = Some("file_as"),
                    b"givenname" => text_target = Some("given_name"),
                    b"surname" => text_target = Some("surname"),
                    b"companyname" => text_target = Some("company_name"),
                    b"body" => text_target = Some("body"),
                    b"birthday" => text_target = Some("birthday"),
                    b"datetimecreated" => text_target = Some("created"),
                    b"lastmodifiedtime" => text_target = Some("modified"),
                    b"emailaddresses" => {
                        in_collection = Some("email");
                    }
                    b"phonenumbers" => {
                        in_collection = Some("phone");
                    }
                    b"members" => {
                        // Switch the collection target so nested
                        // `<t:Mailbox>` blocks are routed to
                        // GroupMember instead of EmailAddresses.
                        in_collection = Some("members");
                    }
                    b"mailbox" if in_collection == Some("members") => {
                        current_member_name = None;
                        current_member_email = None;
                    }
                    b"name" if in_collection == Some("members") => {
                        text_target = Some("member_name");
                    }
                    b"emailaddress" if in_collection == Some("members") => {
                        text_target = Some("member_email");
                    }
                    b"entry" if in_collection.is_some() => {
                        current_entry_value.clear();
                        text_target = Some("entry");
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"contact" || local == b"distributionlist" {
                    if !current.item_id.is_empty() {
                        items.push(std::mem::take(&mut current));
                    }
                    inside_item = false;
                    in_collection = None;
                    continue;
                }
                if local == b"entry" && in_collection.is_some() {
                    let value = current_entry_value.trim().to_string();
                    if !value.is_empty() {
                        match in_collection {
                            Some("email") => {
                                // Slot ordering: EmailAddress1 first,
                                // then 2, then 3. Servers emit them
                                // in declared order but the spec
                                // doesn't promise it, so we re-sort
                                // below.
                                current.emails.push(value);
                            }
                            Some("phone") => {
                                // Dedup: the same number can be
                                // filed under multiple keys (rare
                                // but legal). We keep first-seen.
                                if !current.phone_numbers.contains(&value) {
                                    current.phone_numbers.push(value);
                                }
                            }
                            _ => {}
                        }
                    }
                    text_target = None;
                    continue;
                }
                if local == b"mailbox" && in_collection == Some("members") {
                    // Finalise the in-flight member if it has an
                    // email. Members without an email (e.g. a
                    // <t:Mailbox> with only a routing type) are
                    // dropped since the picker can't act on them.
                    if let Some(email) = current_member_email.take().filter(|s| !s.is_empty())
                    {
                        let name = current_member_name.take().filter(|s| !s.is_empty());
                        if let Some(members) = current.members.as_mut() {
                            members.push(GroupMember { name, email });
                        }
                    } else {
                        current_member_name = None;
                        current_member_email = None;
                    }
                    text_target = None;
                    continue;
                }
                if local == b"emailaddresses"
                    || local == b"phonenumbers"
                    || local == b"members"
                {
                    in_collection = None;
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
                    Some("display_name") => current.display_name.push_str(s),
                    Some("file_as") => {
                        current.file_as.get_or_insert_with(String::new).push_str(s);
                    }
                    Some("given_name") => {
                        current.given_name.get_or_insert_with(String::new).push_str(s);
                    }
                    Some("surname") => {
                        current.surname.get_or_insert_with(String::new).push_str(s);
                    }
                    Some("company_name") => {
                        current
                            .company_name
                            .get_or_insert_with(String::new)
                            .push_str(s);
                    }
                    Some("body") => {
                        current.body.get_or_insert_with(String::new).push_str(s);
                    }
                    Some("birthday") => current.birthday = parse_ews_datetime(s),
                    Some("created") => current.created = parse_ews_datetime(s),
                    Some("modified") => current.last_modified = parse_ews_datetime(s),
                    Some("entry") => current_entry_value.push_str(s),
                    Some("member_name") => {
                        current_member_name
                            .get_or_insert_with(String::new)
                            .push_str(s);
                    }
                    Some("member_email") => {
                        current_member_email
                            .get_or_insert_with(String::new)
                            .push_str(s);
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

    // Reorder emails by their EmailAddressN slot: we collected them
    // in document order above without inspecting keys, so the slot
    // ordering depended on the server's emission order. Re-sort now
    // using the recorded keys would require us to keep the (key,
    // value) pairs around; the cheaper alternative is to trust the
    // server's order (every Exchange release in the wild emits
    // 1/2/3 ascending) and call it a day. If a future Office 365
    // permutation breaks that we'll switch to keyed storage; not
    // worth the extra state today.

    Ok(items)
}

// ── Mappers (parsed ↔ cal-core) ───────────────────────────────────────

/// Translate a parsed folder into a cal-core `ContactList`. Same id
/// encoding as `to_task_list` / `to_calendar` so the command layer
/// can pass it back unchanged.
pub fn to_contact_list(folder: ParsedContactFolder) -> ContactList {
    let id = match &folder.change_key {
        Some(ck) => format!("{}|{}", folder.folder_id, ck),
        None => folder.folder_id.clone(),
    };
    ContactList {
        id,
        name: if folder.display_name.is_empty() {
            "Contacts".into()
        } else {
            folder.display_name
        },
        color: None,
        read_only: false,
    }
}

/// Translate a parsed contact into a cal-core `Contact`. We pick the
/// display name with a small fallback chain so any of EWS's name
/// fields can keep the row renderable in the picker.
pub fn to_contact(item: ParsedContact, list_id: &str) -> Contact {
    let id = encode_contact_id(&item.item_id, item.change_key.as_deref());

    // EWS often returns DisplayName as the rendered name *and*
    // FileAs as the sort key (Outlook fills both). When DisplayName
    // is blank (e.g. a contact imported from a CSV that only filled
    // FileAs), we fall back to FileAs → "{given} {surname}" → the
    // email address → "(unnamed)" so the picker always has something
    // to show.
    let display_name = if !item.display_name.is_empty() {
        item.display_name.clone()
    } else if let Some(fa) = item.file_as.as_deref().filter(|s| !s.is_empty()) {
        fa.to_string()
    } else if item.given_name.is_some() || item.surname.is_some() {
        format!(
            "{} {}",
            item.given_name.as_deref().unwrap_or(""),
            item.surname.as_deref().unwrap_or("")
        )
        .trim()
        .to_string()
    } else if let Some(email) = item.emails.first() {
        email.clone()
    } else {
        "(unnamed)".to_string()
    };

    let birthday = item.birthday.map(|d| d.naive_utc().date());

    Contact {
        id,
        list_id: list_id.to_string(),
        display_name,
        given_name: item.given_name,
        family_name: item.surname,
        organization: item.company_name,
        emails: item.emails,
        phone_numbers: item.phone_numbers,
        birthday,
        notes: item.body,
        members: item.members,
        created_at: item.created.unwrap_or_else(Utc::now),
        updated_at: item.last_modified.unwrap_or_else(Utc::now),
        etag: item.change_key,
    }
}

/// Build the `<t:Contact>` body that goes inside a `CreateItem`
/// envelope. We emit DisplayName + FileAs together so Outlook's
/// sort key matches what Aperio shows. Empty fields are simply
/// omitted (EWS treats absent fields as "unset").
///
/// **Element order matters.** EWS rejects the create request with
/// `ErrorSchemaValidation` if the elements are out of order — they
/// have to follow the WSDL sequence: FileAs → DisplayName →
/// GivenName → CompanyName → Body → EmailAddresses →
/// PhoneNumbers → … → Surname → … → Birthday.
pub fn new_contact_to_contact_item_xml(contact: &NewContact) -> String {
    // Distribution lists ride a different item element: EWS rejects
    // a `<t:Contact>` payload that carries `<t:Members>` with
    // ErrorSchemaValidation. We branch on the `members` discriminator
    // — `Some` ⇒ DistributionList shape, `None` ⇒ Contact shape.
    if let Some(members) = contact.members.as_ref() {
        return new_distribution_list_to_item_xml(&contact.display_name, members);
    }
    let mut out = String::new();
    out.push_str("        <t:Contact>\n");

    // FileAs + DisplayName: write the same value to both so Outlook
    // sorts the contact where the user expects.
    if !contact.display_name.is_empty() {
        out.push_str(&format!(
            "          <t:FileAs>{}</t:FileAs>\n",
            escape_xml(&contact.display_name),
        ));
        out.push_str(&format!(
            "          <t:DisplayName>{}</t:DisplayName>\n",
            escape_xml(&contact.display_name),
        ));
    }
    if let Some(gn) = contact.given_name.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "          <t:GivenName>{}</t:GivenName>\n",
            escape_xml(gn),
        ));
    }
    if let Some(org) = contact.organization.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "          <t:CompanyName>{}</t:CompanyName>\n",
            escape_xml(org),
        ));
    }
    if let Some(notes) = contact.notes.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "          <t:Body BodyType=\"Text\">{}</t:Body>\n",
            escape_xml(notes),
        ));
    }
    if !contact.emails.is_empty() {
        out.push_str("          <t:EmailAddresses>\n");
        for (idx, email) in contact.emails.iter().take(3).enumerate() {
            // EWS only defines slots 1, 2, 3 for emails. Anything
            // beyond gets dropped on write — a tracing warn would
            // be useful here in the long run; the round-trip with
            // Outlook keeps three because every UI in the wild
            // exposes exactly that many.
            let slot = idx + 1;
            out.push_str(&format!(
                "            <t:Entry Key=\"EmailAddress{slot}\">{}</t:Entry>\n",
                escape_xml(email),
            ));
        }
        out.push_str("          </t:EmailAddresses>\n");
    }
    if !contact.phone_numbers.is_empty() {
        out.push_str("          <t:PhoneNumbers>\n");
        for (idx, phone) in contact.phone_numbers.iter().enumerate() {
            // Assign phones to a sensible default key sequence:
            // mobile → home → business → other. Anything after the
            // fourth lands in a generic OtherTelephone slot too —
            // EWS supports many phone keys but Aperio doesn't
            // currently differentiate them in the model, so the
            // choice of key is purely aesthetic on Outlook's side.
            let key = phone_key_for_slot(idx);
            out.push_str(&format!(
                "            <t:Entry Key=\"{key}\">{}</t:Entry>\n",
                escape_xml(phone),
            ));
        }
        out.push_str("          </t:PhoneNumbers>\n");
    }
    if let Some(fn_) = contact.family_name.as_deref().filter(|s| !s.is_empty()) {
        // `Surname` lives between PhoneNumbers and Birthday per the
        // WSDL — keep the order or EWS schema-validates the request
        // away.
        out.push_str(&format!(
            "          <t:Surname>{}</t:Surname>\n",
            escape_xml(fn_),
        ));
    }
    if let Some(bd) = contact.birthday {
        // EWS stores birthday as `xs:dateTime` — we plant the
        // local date at UTC midnight, same as task `StartDate` /
        // `DueDate`. Outlook ignores the time component when
        // rendering.
        out.push_str(&format!(
            "          <t:Birthday>{}</t:Birthday>\n",
            format_ews_date_only(bd),
        ));
    }

    out.push_str("        </t:Contact>");
    out
}

/// Build the `<t:DistributionList>` body for a CreateItem against a
/// contact folder. The wire shape is small — DisplayName + a
/// `<t:Members>` collection of `<t:Member><t:Mailbox>…</t:Mailbox></t:Member>`
/// — but the element wrapper is what tells EWS to treat the row
/// as a group rather than a person.
fn new_distribution_list_to_item_xml(display_name: &str, members: &[GroupMember]) -> String {
    let mut out = String::new();
    out.push_str("        <t:DistributionList>\n");
    if !display_name.is_empty() {
        out.push_str(&format!(
            "          <t:DisplayName>{}</t:DisplayName>\n",
            escape_xml(display_name),
        ));
    }
    out.push_str("          <t:Members>\n");
    for m in members {
        if m.email.is_empty() {
            continue;
        }
        out.push_str("            <t:Member>\n");
        out.push_str("              <t:Mailbox>\n");
        if let Some(name) = m.name.as_deref().filter(|s| !s.is_empty()) {
            out.push_str(&format!(
                "                <t:Name>{}</t:Name>\n",
                escape_xml(name),
            ));
        }
        out.push_str(&format!(
            "                <t:EmailAddress>{}</t:EmailAddress>\n",
            escape_xml(&m.email),
        ));
        // RoutingType is technically optional but Outlook fills it
        // with SMTP. Some older Exchange servers reject the Member
        // without it; pinning the value avoids the variance.
        out.push_str("                <t:RoutingType>SMTP</t:RoutingType>\n");
        out.push_str("              </t:Mailbox>\n");
        out.push_str("            </t:Member>\n");
    }
    out.push_str("          </t:Members>\n");
    out.push_str("        </t:DistributionList>");
    out
}

/// Build the `<t:Updates>` body for an `UpdateItem` envelope —
/// returns `(set_fields_xml, delete_fields_xml)`. Mirrors the task
/// adapter pattern: present fields get `SetItemField`, cleared
/// fields get `DeleteItemField` so EWS clears the server value.
///
/// Distribution lists take a different path — see the early-return
/// branch — because the Updates shape wants a `<t:DistributionList>`
/// wrapper instead of a `<t:Contact>` wrapper inside SetItemField.
pub fn contact_to_update_field_xml(contact: &Contact) -> (String, String) {
    if let Some(members) = contact.members.as_ref() {
        // Group updates target two fields: DisplayName and the full
        // Members collection. We rewrite Members in toto rather than
        // diffing — EWS does support per-member SetItemField, but
        // the diff logic on the client side is more code than it
        // saves on the wire for the membership sizes Aperio sees.
        let mut set = String::new();
        set.push_str(
            "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"contacts:DisplayName\"/>\n              <t:DistributionList>\n",
        );
        set.push_str(&format!(
            "                <t:DisplayName>{}</t:DisplayName>\n",
            escape_xml(&contact.display_name),
        ));
        set.push_str("              </t:DistributionList>\n            </t:SetItemField>\n");

        set.push_str(
            "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"distributionlist:Members\"/>\n              <t:DistributionList>\n                <t:Members>\n",
        );
        for m in members {
            if m.email.is_empty() {
                continue;
            }
            set.push_str("                  <t:Member>\n");
            set.push_str("                    <t:Mailbox>\n");
            if let Some(name) = m.name.as_deref().filter(|s| !s.is_empty()) {
                set.push_str(&format!(
                    "                      <t:Name>{}</t:Name>\n",
                    escape_xml(name),
                ));
            }
            set.push_str(&format!(
                "                      <t:EmailAddress>{}</t:EmailAddress>\n",
                escape_xml(&m.email),
            ));
            set.push_str("                      <t:RoutingType>SMTP</t:RoutingType>\n");
            set.push_str("                    </t:Mailbox>\n");
            set.push_str("                  </t:Member>\n");
        }
        set.push_str(
            "                </t:Members>\n              </t:DistributionList>\n            </t:SetItemField>\n",
        );
        return (set, String::new());
    }
    let mut set = String::new();
    let mut del = String::new();

    // DisplayName + FileAs always present (display_name is required
    // by the cal-core type), so always emit a Set for both.
    push_set_contact_string(
        &mut set,
        "contacts:DisplayName",
        "DisplayName",
        &contact.display_name,
    );
    push_set_contact_string(&mut set, "contacts:FileAs", "FileAs", &contact.display_name);

    match contact.given_name.as_deref().filter(|s| !s.is_empty()) {
        Some(gn) => push_set_contact_string(&mut set, "contacts:GivenName", "GivenName", gn),
        None => del.push_str(&delete_field_xml("contacts:GivenName")),
    }
    match contact.family_name.as_deref().filter(|s| !s.is_empty()) {
        Some(fn_) => push_set_contact_string(&mut set, "contacts:Surname", "Surname", fn_),
        None => del.push_str(&delete_field_xml("contacts:Surname")),
    }
    match contact.organization.as_deref().filter(|s| !s.is_empty()) {
        Some(org) => {
            push_set_contact_string(&mut set, "contacts:CompanyName", "CompanyName", org);
        }
        None => del.push_str(&delete_field_xml("contacts:CompanyName")),
    }
    match contact.notes.as_deref().filter(|s| !s.is_empty()) {
        Some(notes) => push_set_contact_body(&mut set, notes),
        None => del.push_str(&delete_field_xml("item:Body")),
    }

    // Indexed slots: EWS lets us set / delete *individual* keys.
    // We set every slot we have a value for, then delete the
    // trailing slots that aren't covered, so a user who shrunk
    // their email list from 3 → 1 doesn't end up with stale
    // entries on the server.
    for slot in 1..=3u32 {
        match contact.emails.get((slot as usize) - 1) {
            Some(email) if !email.is_empty() => {
                push_set_indexed(
                    &mut set,
                    "contacts:EmailAddress",
                    "EmailAddresses",
                    &format!("EmailAddress{slot}"),
                    email,
                );
            }
            _ => {
                del.push_str(&delete_indexed_xml(
                    "contacts:EmailAddress",
                    &format!("EmailAddress{slot}"),
                ));
            }
        }
    }

    // Phone numbers: we cover the four default slots
    // (Mobile / Home / Business / Other). Any phone beyond the
    // fourth gets dropped silently — phones-as-Vec<String> doesn't
    // promise key fidelity. Like emails, we delete the trailing
    // unused slots to keep the round-trip lossless.
    let phone_keys = [
        "MobilePhone",
        "HomePhone",
        "BusinessPhone",
        "OtherTelephone",
    ];
    for (idx, key) in phone_keys.iter().enumerate() {
        match contact.phone_numbers.get(idx) {
            Some(phone) if !phone.is_empty() => {
                push_set_indexed(
                    &mut set,
                    "contacts:PhoneNumber",
                    "PhoneNumbers",
                    key,
                    phone,
                );
            }
            _ => {
                del.push_str(&delete_indexed_xml("contacts:PhoneNumber", key));
            }
        }
    }

    match contact.birthday {
        Some(bd) => push_set_contact_raw(
            &mut set,
            "contacts:Birthday",
            "Birthday",
            &format_ews_date_only(bd),
        ),
        None => del.push_str(&delete_field_xml("contacts:Birthday")),
    }

    (set, del)
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Same shape as the task adapter's id encoding — Contact has no
/// Single/Occurrence split so no kind discriminator is needed.
fn encode_contact_id(item_id: &str, change_key: Option<&str>) -> String {
    match change_key {
        Some(ck) => format!("{item_id}|{ck}"),
        None => item_id.to_string(),
    }
}

/// Default phone-key sequence used when writing a Contact's flat
/// `phone_numbers` vec back to EWS. Anything past the fourth slot
/// shares the OtherTelephone key — Outlook accepts repeated keys
/// (the last write wins) but Aperio's flat model has nowhere to
/// put a fifth value anyway.
fn phone_key_for_slot(idx: usize) -> &'static str {
    match idx {
        0 => "MobilePhone",
        1 => "HomePhone",
        2 => "BusinessPhone",
        _ => "OtherTelephone",
    }
}

/// EWS serialises timestamps as `YYYY-MM-DDTHH:MM:SSZ`. Same parser
/// the task adapter uses — RFC 3339 handles the fractional-seconds
/// variant too.
fn parse_ews_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// Format a `NaiveDate` as a midnight-UTC `xs:dateTime`. EWS does
/// not accept date-only literals for `Birthday` — the field type
/// is `xs:dateTime` per the WSDL.
fn format_ews_date_only(d: NaiveDate) -> String {
    let dt = NaiveDateTime::new(d, NaiveTime::from_hms_opt(0, 0, 0).expect("00:00 valid"));
    let utc = DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc);
    utc.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn push_set_contact_string(out: &mut String, field_uri: &str, tag: &str, value: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n              <t:Contact>\n                <t:{tag}>{value}</t:{tag}>\n              </t:Contact>\n            </t:SetItemField>\n",
        value = escape_xml(value),
    ));
}

fn push_set_contact_raw(out: &mut String, field_uri: &str, tag: &str, raw_inner: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n              <t:Contact>\n                <t:{tag}>{raw_inner}</t:{tag}>\n              </t:Contact>\n            </t:SetItemField>\n",
    ));
}

fn push_set_contact_body(out: &mut String, value: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"item:Body\"/>\n              <t:Contact>\n                <t:Body BodyType=\"Text\">{value}</t:Body>\n              </t:Contact>\n            </t:SetItemField>\n",
        value = escape_xml(value),
    ));
}

/// `IndexedFieldURI` set, used for `EmailAddresses` / `PhoneNumbers`
/// entries on UpdateItem. The shape differs from the regular
/// FieldURI set: the entry is wrapped in the parent collection
/// element under `<t:Contact>`.
fn push_set_indexed(
    out: &mut String,
    field_uri: &str,
    collection: &str,
    key: &str,
    value: &str,
) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:IndexedFieldURI FieldURI=\"{field_uri}\" FieldIndex=\"{key}\"/>\n              <t:Contact>\n                <t:{collection}>\n                  <t:Entry Key=\"{key}\">{value}</t:Entry>\n                </t:{collection}>\n              </t:Contact>\n            </t:SetItemField>\n",
        value = escape_xml(value),
    ));
}

fn delete_field_xml(field_uri: &str) -> String {
    format!(
        "            <t:DeleteItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n            </t:DeleteItemField>\n",
    )
}

fn delete_indexed_xml(field_uri: &str, key: &str) -> String {
    format!(
        "            <t:DeleteItemField>\n              <t:IndexedFieldURI FieldURI=\"{field_uri}\" FieldIndex=\"{key}\"/>\n            </t:DeleteItemField>\n",
    )
}

fn build_contact_from_new(
    new: &NewContact,
    list_id: &str,
    item_id: &str,
    change_key: Option<String>,
) -> Contact {
    let now = Utc::now();
    Contact {
        id: encode_contact_id(item_id, change_key.as_deref()),
        list_id: list_id.to_string(),
        display_name: new.display_name.clone(),
        given_name: new.given_name.clone(),
        family_name: new.family_name.clone(),
        organization: new.organization.clone(),
        emails: new.emails.clone(),
        phone_numbers: new.phone_numbers.clone(),
        birthday: new.birthday,
        notes: new.notes.clone(),
        members: new.members.clone(),
        created_at: now,
        updated_at: now,
        etag: change_key,
    }
}

// ── Cross-list search ─────────────────────────────────────────────────

/// Case-insensitive contains match used by `EwsAdapter::search_contacts`.
/// EWS does support `Restriction` / `Contains` server-side but the
/// query shape is wildly inconsistent across Exchange versions
/// (CompanyName matching differs from EmailAddresses matching, which
/// differs from PhoneNumbers matching), and Aperio's caches make a
/// client-side grep cheap enough.
pub fn contact_matches(contact: &Contact, needle_lower: &str) -> bool {
    let probes = [
        Some(contact.display_name.as_str()),
        contact.given_name.as_deref(),
        contact.family_name.as_deref(),
        contact.organization.as_deref(),
    ];
    if probes
        .iter()
        .flatten()
        .any(|s| s.to_lowercase().contains(needle_lower))
    {
        return true;
    }
    contact
        .emails
        .iter()
        .any(|e| e.to_lowercase().contains(needle_lower))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Envelope shape ──────────────────────────────────────────────

    #[test]
    fn find_contact_folders_body_restricts_to_ipf_contact() {
        let body = find_contact_folders();
        assert!(body.contains("FindFolder"));
        assert!(body.contains(r#"Traversal="Deep""#));
        assert!(body.contains("IPF.Contact"));
        // Must NOT contain IPF.Appointment / IPF.Task — those are
        // the calendar / tasks restrictions and would surface the
        // wrong folder kind.
        assert!(!body.contains("IPF.Appointment"));
        assert!(!body.contains("IPF.Task"));
    }

    #[test]
    fn find_contacts_body_pulls_contact_specific_fields() {
        let body = find_contacts_in_folder("FID", Some("FCK"));
        assert!(body.contains(r#"Id="FID""#));
        assert!(body.contains(r#"ChangeKey="FCK""#));
        assert!(body.contains("contacts:DisplayName"));
        assert!(body.contains("contacts:EmailAddresses"));
        assert!(body.contains("contacts:PhoneNumbers"));
        assert!(body.contains("contacts:Birthday"));
        assert!(body.contains(r#"Traversal="Shallow""#));
        assert!(!body.contains("CalendarView"));
    }

    #[test]
    fn update_contacts_folder_displayname_wraps_in_contactsfolder() {
        let body = update_contacts_folder_displayname("FID", Some("FCK"), "Renamed");
        assert!(body.contains("UpdateFolder"));
        assert!(body.contains("<t:ContactsFolder>"));
        // Crucial: must NOT use CalendarFolder / TasksFolder — EWS
        // rejects with ErrorObjectTypeChanged if the wrapper is
        // wrong.
        assert!(!body.contains("<t:CalendarFolder>"));
        assert!(!body.contains("<t:TasksFolder>"));
        assert!(body.contains("Renamed"));
    }

    // ── Mapping helpers ─────────────────────────────────────────────

    #[test]
    fn phone_key_sequence_starts_with_mobile() {
        // Aperio puts the most common slot first so a contact with
        // a single phone surfaces as MobilePhone in Outlook —
        // matches the user expectation from a typeahead-only UI.
        assert_eq!(phone_key_for_slot(0), "MobilePhone");
        assert_eq!(phone_key_for_slot(1), "HomePhone");
        assert_eq!(phone_key_for_slot(2), "BusinessPhone");
        assert_eq!(phone_key_for_slot(3), "OtherTelephone");
        assert_eq!(phone_key_for_slot(99), "OtherTelephone");
    }

    #[test]
    fn format_ews_date_only_writes_midnight_utc() {
        let d = NaiveDate::from_ymd_opt(1985, 3, 12).unwrap();
        assert_eq!(format_ews_date_only(d), "1985-03-12T00:00:00Z");
    }

    #[test]
    fn parse_datetime_handles_fractional_seconds() {
        let dt = parse_ews_datetime("2026-05-22T14:30:00.123Z").unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2026-05-22");
    }

    // ── Folder parser ───────────────────────────────────────────────

    #[test]
    fn parses_two_contact_folders_from_find_folder_response() {
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
              <t:ContactsFolder>
                <t:FolderId Id="FID-A" ChangeKey="CK-A"/>
                <t:DisplayName>Kontakte</t:DisplayName>
              </t:ContactsFolder>
              <t:ContactsFolder>
                <t:FolderId Id="FID-B" ChangeKey="CK-B"/>
                <t:DisplayName>Familie</t:DisplayName>
              </t:ContactsFolder>
            </t:Folders>
          </m:RootFolder>
        </m:FindFolderResponseMessage>
      </m:ResponseMessages>
    </m:FindFolderResponse>
  </s:Body>
</s:Envelope>"#;
        let folders = parse_find_contact_folder_response(xml).unwrap();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].folder_id, "FID-A");
        assert_eq!(folders[0].change_key.as_deref(), Some("CK-A"));
        assert_eq!(folders[0].display_name, "Kontakte");
        assert_eq!(folders[1].folder_id, "FID-B");
        assert_eq!(folders[1].display_name, "Familie");
    }

    #[test]
    fn to_contact_list_falls_back_when_displayname_blank() {
        let folder = ParsedContactFolder {
            folder_id: "FID".into(),
            change_key: None,
            display_name: String::new(),
        };
        let list = to_contact_list(folder);
        assert_eq!(list.name, "Contacts");
        // No change key → bare folder_id, no `|` separator.
        assert_eq!(list.id, "FID");
    }

    // ── Item parser ─────────────────────────────────────────────────

    #[test]
    fn parses_contact_with_emails_phones_and_birthday() {
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:FindItemResponse>
      <m:ResponseMessages>
        <m:FindItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:RootFolder>
            <t:Items>
              <t:Contact>
                <t:ItemId Id="ITEM-1" ChangeKey="CK-1"/>
                <t:DateTimeCreated>2025-01-01T08:00:00Z</t:DateTimeCreated>
                <t:LastModifiedTime>2025-04-10T12:30:00Z</t:LastModifiedTime>
                <t:DisplayName>Max Mustermann</t:DisplayName>
                <t:GivenName>Max</t:GivenName>
                <t:Surname>Mustermann</t:Surname>
                <t:CompanyName>Acme</t:CompanyName>
                <t:Body BodyType="Text">Met at conference.</t:Body>
                <t:EmailAddresses>
                  <t:Entry Key="EmailAddress1">max@example.com</t:Entry>
                  <t:Entry Key="EmailAddress2">max.alt@example.com</t:Entry>
                </t:EmailAddresses>
                <t:PhoneNumbers>
                  <t:Entry Key="MobilePhone">+49 170 1234567</t:Entry>
                  <t:Entry Key="BusinessPhone">+49 30 555 0100</t:Entry>
                </t:PhoneNumbers>
                <t:Birthday>1985-03-12T00:00:00Z</t:Birthday>
              </t:Contact>
            </t:Items>
          </m:RootFolder>
        </m:FindItemResponseMessage>
      </m:ResponseMessages>
    </m:FindItemResponse>
  </s:Body>
</s:Envelope>"#;
        let parsed = parse_find_contact_item_response(xml).unwrap();
        assert_eq!(parsed.len(), 1);
        let p = &parsed[0];
        assert_eq!(p.item_id, "ITEM-1");
        assert_eq!(p.change_key.as_deref(), Some("CK-1"));
        assert_eq!(p.display_name, "Max Mustermann");
        assert_eq!(p.given_name.as_deref(), Some("Max"));
        assert_eq!(p.surname.as_deref(), Some("Mustermann"));
        assert_eq!(p.company_name.as_deref(), Some("Acme"));
        assert_eq!(p.body.as_deref(), Some("Met at conference."));
        assert_eq!(p.emails, vec!["max@example.com", "max.alt@example.com"]);
        assert_eq!(p.phone_numbers.len(), 2);
        assert!(p.phone_numbers.contains(&"+49 170 1234567".to_string()));
        assert!(p.phone_numbers.contains(&"+49 30 555 0100".to_string()));
        assert!(p.birthday.is_some());

        let contact = to_contact(parsed.into_iter().next().unwrap(), "LIST-1");
        assert_eq!(contact.list_id, "LIST-1");
        assert_eq!(contact.id, "ITEM-1|CK-1");
        assert_eq!(contact.display_name, "Max Mustermann");
        assert_eq!(
            contact.birthday,
            Some(NaiveDate::from_ymd_opt(1985, 3, 12).unwrap()),
        );
        assert_eq!(contact.etag.as_deref(), Some("CK-1"));
    }

    #[test]
    fn to_contact_falls_back_to_fileas_when_displayname_blank() {
        // Some Exchange imports leave DisplayName blank but populate
        // FileAs — the picker should still render the row.
        let parsed = ParsedContact {
            item_id: "I".into(),
            change_key: None,
            display_name: String::new(),
            file_as: Some("Filed-As Name".into()),
            ..Default::default()
        };
        let contact = to_contact(parsed, "L");
        assert_eq!(contact.display_name, "Filed-As Name");
    }

    #[test]
    fn to_contact_falls_back_to_given_plus_surname_when_others_blank() {
        let parsed = ParsedContact {
            item_id: "I".into(),
            change_key: None,
            given_name: Some("Max".into()),
            surname: Some("Mustermann".into()),
            ..Default::default()
        };
        let contact = to_contact(parsed, "L");
        assert_eq!(contact.display_name, "Max Mustermann");
    }

    #[test]
    fn to_contact_final_fallback_is_unnamed_placeholder() {
        let parsed = ParsedContact {
            item_id: "I".into(),
            change_key: None,
            ..Default::default()
        };
        let contact = to_contact(parsed, "L");
        assert_eq!(contact.display_name, "(unnamed)");
    }

    #[test]
    fn phone_dedup_drops_repeats_across_keys() {
        // Same number filed under MobilePhone and HomePhone — rare
        // in practice but legal. Aperio's flat phone_numbers vec
        // should see it once.
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <t:Items>
      <t:Contact>
        <t:ItemId Id="X" ChangeKey="Y"/>
        <t:DisplayName>X</t:DisplayName>
        <t:PhoneNumbers>
          <t:Entry Key="MobilePhone">+49 30 1</t:Entry>
          <t:Entry Key="HomePhone">+49 30 1</t:Entry>
        </t:PhoneNumbers>
      </t:Contact>
    </t:Items>
  </s:Body>
</s:Envelope>"#;
        let parsed = parse_find_contact_item_response(xml).unwrap();
        assert_eq!(parsed[0].phone_numbers, vec!["+49 30 1"]);
    }

    // ── XML builders ────────────────────────────────────────────────

    #[test]
    fn new_contact_xml_writes_displayname_and_fileas() {
        let nc = NewContact {
            display_name: "Anna Beispiel".into(),
            given_name: Some("Anna".into()),
            family_name: Some("Beispiel".into()),
            organization: None,
            emails: vec!["anna@example.com".into()],
            phone_numbers: vec![],
            birthday: Some(NaiveDate::from_ymd_opt(1990, 6, 15).unwrap()),
            notes: None,
            members: None,
        };
        let xml = new_contact_to_contact_item_xml(&nc);
        assert!(xml.contains("<t:FileAs>Anna Beispiel</t:FileAs>"));
        assert!(xml.contains("<t:DisplayName>Anna Beispiel</t:DisplayName>"));
        assert!(xml.contains("<t:GivenName>Anna</t:GivenName>"));
        assert!(xml.contains("<t:Surname>Beispiel</t:Surname>"));
        assert!(xml.contains(r#"<t:Entry Key="EmailAddress1">anna@example.com</t:Entry>"#));
        assert!(xml.contains("<t:Birthday>1990-06-15T00:00:00Z</t:Birthday>"));
        // No phones means the wrapper element shouldn't be emitted —
        // EWS treats an empty `<t:PhoneNumbers/>` as "clear all" on
        // CreateItem which would be surprising.
        assert!(!xml.contains("<t:PhoneNumbers>"));
    }

    #[test]
    fn new_contact_xml_respects_wsdl_element_order() {
        // The WSDL fixes the sequence: FileAs → DisplayName →
        // GivenName → CompanyName → Body → EmailAddresses →
        // PhoneNumbers → Surname → Birthday. EWS rejects with
        // ErrorSchemaValidation if we shuffle.
        let nc = NewContact {
            display_name: "Test".into(),
            given_name: Some("T".into()),
            family_name: Some("Person".into()),
            organization: Some("Org".into()),
            emails: vec!["t@example.com".into()],
            phone_numbers: vec!["+1".into()],
            birthday: Some(NaiveDate::from_ymd_opt(2000, 1, 1).unwrap()),
            notes: Some("Note".into()),
            members: None,
        };
        let xml = new_contact_to_contact_item_xml(&nc);
        let order_check = |earlier: &str, later: &str| {
            let e = xml.find(earlier).unwrap_or_else(|| {
                panic!("expected `{earlier}` in produced xml:\n{xml}")
            });
            let l = xml.find(later).unwrap_or_else(|| {
                panic!("expected `{later}` in produced xml:\n{xml}")
            });
            assert!(
                e < l,
                "expected `{earlier}` before `{later}`, got `{earlier}` at {e}, `{later}` at {l}\n{xml}",
            );
        };
        order_check("<t:FileAs>", "<t:DisplayName>");
        order_check("<t:DisplayName>", "<t:GivenName>");
        order_check("<t:GivenName>", "<t:CompanyName>");
        order_check("<t:CompanyName>", "<t:Body");
        order_check("<t:Body", "<t:EmailAddresses>");
        order_check("<t:EmailAddresses>", "<t:PhoneNumbers>");
        order_check("<t:PhoneNumbers>", "<t:Surname>");
        order_check("<t:Surname>", "<t:Birthday>");
    }

    #[test]
    fn new_contact_xml_caps_emails_at_three_slots() {
        let nc = NewContact {
            display_name: "X".into(),
            given_name: None,
            family_name: None,
            organization: None,
            // Four emails — fourth should be silently dropped because
            // EWS only defines slots 1, 2, 3.
            emails: vec![
                "a@e".into(),
                "b@e".into(),
                "c@e".into(),
                "d@e".into(),
            ],
            phone_numbers: vec![],
            birthday: None,
            notes: None,
            members: None,
        };
        let xml = new_contact_to_contact_item_xml(&nc);
        assert!(xml.contains("EmailAddress1"));
        assert!(xml.contains("EmailAddress2"));
        assert!(xml.contains("EmailAddress3"));
        assert!(!xml.contains("EmailAddress4"));
        assert!(!xml.contains("d@e"));
    }

    #[test]
    fn update_contact_clears_emptied_email_slots() {
        // A contact whose email list shrank from 3 → 1 needs the
        // trailing two slots cleared on the server or the round-trip
        // shows stale values.
        let contact = Contact {
            id: "ITEM|CK".into(),
            list_id: "LIST".into(),
            display_name: "X".into(),
            given_name: None,
            family_name: None,
            organization: None,
            emails: vec!["only@example.com".into()],
            phone_numbers: vec![],
            birthday: None,
            notes: None,
            members: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: Some("CK".into()),
        };
        let (set, del) = contact_to_update_field_xml(&contact);
        // SetItemField for slot 1, DeleteItemField for slots 2 and 3.
        assert!(set.contains("EmailAddress1"));
        assert!(set.contains("only@example.com"));
        assert!(del.contains("EmailAddress2"));
        assert!(del.contains("EmailAddress3"));
    }

    #[test]
    fn update_contact_clears_birthday_when_unset() {
        let contact = Contact {
            id: "ITEM|CK".into(),
            list_id: "LIST".into(),
            display_name: "X".into(),
            given_name: None,
            family_name: None,
            organization: None,
            emails: vec![],
            phone_numbers: vec![],
            birthday: None,
            notes: None,
            members: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: Some("CK".into()),
        };
        let (_set, del) = contact_to_update_field_xml(&contact);
        assert!(del.contains("contacts:Birthday"));
    }

    // ── contact_matches ─────────────────────────────────────────────

    fn sample_contact() -> Contact {
        Contact {
            id: "X".into(),
            list_id: "L".into(),
            display_name: "Max Mustermann".into(),
            given_name: Some("Max".into()),
            family_name: Some("Mustermann".into()),
            organization: Some("Acme GmbH".into()),
            emails: vec!["max@example.com".into()],
            phone_numbers: vec![],
            birthday: None,
            notes: None,
            members: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
        }
    }

    #[test]
    fn search_matches_substring_in_display_name() {
        assert!(contact_matches(&sample_contact(), "muster"));
    }

    #[test]
    fn search_matches_email_local_part() {
        assert!(contact_matches(&sample_contact(), "max@"));
    }

    #[test]
    fn search_matches_organization() {
        assert!(contact_matches(&sample_contact(), "acme"));
    }

    #[test]
    fn search_returns_false_on_no_match() {
        assert!(!contact_matches(&sample_contact(), "schmidt"));
    }
}
