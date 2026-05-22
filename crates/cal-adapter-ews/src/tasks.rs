//! EWS Tasks (`IPF.Task` folder class + `<t:Task>` items) — Phase
//! 6f.2.
//!
//! Tasks are Exchange's standalone "to-do" item type, living in their
//! own folder class (`IPF.Task`) separate from `IPF.Appointment`
//! calendars. Shape compared to CalendarItem:
//!
//!   - Single status enum (`NotStarted`, `InProgress`, `Completed`,
//!     `WaitingOnOthers`, `Deferred`) instead of CalendarItem's
//!     status-via-meeting-response.
//!   - Two independent date slots: `StartDate` (when work begins) and
//!     `DueDate` (when work must be done). Aperio's
//!     `scheduled_date` ↔ `StartDate`, `deadline_date` ↔ `DueDate`
//!     per DESIGN.md §9.7.
//!   - `Importance` (Low / Normal / High) instead of priority on
//!     CalendarItem.
//!   - Single reminder via `ReminderIsSet` + `ReminderDueBy`.
//!   - No occurrence/exception split; recurring tasks are a master
//!     row that EWS auto-rolls forward on completion. Aperio sees
//!     each row as a distinct task — there's no occurrence id we'd
//!     need to round-trip.
//!
//! Out of scope for the first cut:
//!
//!   - Subtasks. EWS tasks don't model hierarchy.
//!   - Recurrence. EWS supports `<t:TaskRecurrence>` but it's a
//!     separate XML shape from CalendarItem's, and recurring tasks
//!     are rarer than recurring events. Round-trips lose the
//!     recurrence on read (master is parsed as a one-shot) and drop
//!     it on write (with a `tracing::warn` so we know).
//!
//! Date semantics: EWS `StartDate` / `DueDate` are `xs:dateTime`
//! even though task UIs treat them as dates. We write Aperio's
//! local `NaiveDate` + optional `NaiveTime` as UTC-tagged datetimes
//! (the local components literally placed into the UTC slot), which
//! is what Outlook itself does on its task side — sharing a task
//! across timezones DOES lose the original local interpretation,
//! but the round-trip with the SAME user stays stable.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader;

use cal_core::{NewTask, Task, TaskList, TaskPriority, TaskStatus};

use crate::api::EwsClient;
use crate::error::{EwsError, EwsResult};
use crate::mapping::{parse_first_item_id, split_calendar_id};
use crate::soap::{delete_calendar_item, escape_xml};

// ── Public adapter-side surface ────────────────────────────────────────

/// Enumerate every task folder in the user's mailbox. Mirrors the
/// CalendarFeature `list_calendars` flow, just with `IPF.Task` as
/// the folder-class restriction.
pub async fn list_task_lists(client: &EwsClient) -> EwsResult<Vec<TaskList>> {
    let body = find_task_folders();
    let xml = client.post_soap(body).await?;
    let folders = parse_find_task_folder_response(&xml)?;
    Ok(folders.into_iter().map(to_task_list).collect())
}

/// Pull every task in `list_id`. EWS doesn't have a CalendarView
/// equivalent for tasks — the date window concept doesn't map onto
/// "to-do without a fixed start" — so we ask for every row in the
/// folder. Large task folders aren't common; if we ever need
/// pagination we'll add `IndexedPageItemView` here.
pub async fn get_tasks(client: &EwsClient, list_id: &str) -> EwsResult<Vec<Task>> {
    let (folder_id, change_key) = split_calendar_id(list_id);
    let body = find_tasks_in_folder(&folder_id, change_key.as_deref());
    let xml = client.post_soap(body).await?;
    let parsed = parse_find_task_item_response(&xml)?;
    parsed
        .into_iter()
        .map(|item| to_task(item, list_id))
        .collect()
}

/// Create a new task in `list_id`. Mirrors `create_event` —
/// build the request payload, post the envelope, harvest the
/// server-assigned ItemId from the response, and synthesize the
/// returned `Task` from the request fields. Saves a follow-up
/// GetItem round-trip.
pub async fn create_task(
    client: &EwsClient,
    list_id: &str,
    task: NewTask,
) -> EwsResult<Task> {
    let (folder_id, folder_change_key) = split_calendar_id(list_id);
    let item_xml = new_task_to_task_item_xml(&task);
    let envelope = create_task_in_folder(
        &folder_id,
        folder_change_key.as_deref(),
        &item_xml,
    );
    let response = client.post_soap(envelope).await?;
    let item_ref = parse_first_item_id(&response)?;
    Ok(build_task_from_new(&task, list_id, &item_ref.id, item_ref.change_key))
}

/// Update an existing task. Every field is set or deleted explicitly
/// so EWS clears anything the user removed — matches Aperio's "what
/// you see is what you get" semantic.
pub async fn update_task(client: &EwsClient, task: &Task) -> EwsResult<Task> {
    let (item_id, change_key) = split_calendar_id(&task.id);
    let (set_xml, delete_xml) = task_to_update_field_xml(task);
    let envelope = update_task_item(
        &item_id,
        change_key.as_deref(),
        &set_xml,
        &delete_xml,
    );
    let response = client.post_soap(envelope).await?;
    let item_ref = parse_first_item_id(&response)?;
    let new_id = encode_task_id(&item_ref.id, item_ref.change_key.as_deref());
    Ok(Task {
        id: new_id,
        etag: item_ref.change_key,
        updated_at: Utc::now(),
        ..task.clone()
    })
}

/// Delete a task. `DeleteItem` works on any item type so we reuse
/// the CalendarItem envelope — the server doesn't care whether the
/// id points at an appointment or a task.
pub async fn delete_task(client: &EwsClient, task_id: &str) -> EwsResult<()> {
    let (item_id, change_key) = split_calendar_id(task_id);
    let envelope = delete_calendar_item(&item_id, change_key.as_deref());
    client.post_soap(envelope).await?;
    Ok(())
}

/// Rename a task folder. EWS uses the same `UpdateFolder` operation
/// for any folder type; we just have to send the `<t:TasksFolder>`
/// wrapper inside `SetFolderField` so the server applies the change
/// to the right folder kind.
pub async fn rename_task_list(
    client: &EwsClient,
    list_id: &str,
    new_name: &str,
) -> EwsResult<()> {
    let (folder_id, change_key) = split_calendar_id(list_id);
    let envelope =
        update_tasks_folder_displayname(&folder_id, change_key.as_deref(), new_name);
    client.post_soap(envelope).await?;
    Ok(())
}

// ── SOAP envelope helpers (task-specific) ──────────────────────────────

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

/// SOAP body for `FindFolder`: walk every folder under the user's
/// mailbox root, restricted to `IPF.Task`. `Deep` traversal pulls in
/// subfolders too — users sometimes keep a "Project/Subproject" tree
/// rather than a flat list.
pub fn find_task_folders() -> String {
    let body = r#"    <m:FindFolder Traversal="Deep">
      <m:FolderShape>
        <t:BaseShape>AllProperties</t:BaseShape>
      </m:FolderShape>
      <m:Restriction>
        <t:IsEqualTo>
          <t:FieldURI FieldURI="folder:FolderClass"/>
          <t:FieldURIOrConstant>
            <t:Constant Value="IPF.Task"/>
          </t:FieldURIOrConstant>
        </t:IsEqualTo>
      </m:Restriction>
      <m:ParentFolderIds>
        <t:DistinguishedFolderId Id="msgfolderroot"/>
      </m:ParentFolderIds>
    </m:FindFolder>"#;
    wrap(body)
}

/// SOAP body for `FindItem` with a `Shallow` traversal over a task
/// folder. Default shape plus the task-specific fields we need to
/// reconstruct a cal-core `Task` without per-row GetItem traffic.
pub fn find_tasks_in_folder(folder_id: &str, change_key: Option<&str>) -> String {
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
          <t:FieldURI FieldURI="item:Importance"/>
          <t:FieldURI FieldURI="item:ReminderIsSet"/>
          <t:FieldURI FieldURI="item:ReminderDueBy"/>
          <t:FieldURI FieldURI="task:StartDate"/>
          <t:FieldURI FieldURI="task:DueDate"/>
          <t:FieldURI FieldURI="task:CompleteDate"/>
          <t:FieldURI FieldURI="task:Status"/>
        </t:AdditionalProperties>
      </m:ItemShape>
      <m:ParentFolderIds>
        {folder_id_attr}
      </m:ParentFolderIds>
    </m:FindItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `CreateItem` into a task folder. Wraps the
/// pre-rendered `<t:Task>` payload in the appropriate envelope,
/// pinning `SavedItemFolderId` so the server files the task under
/// the right folder rather than the default one.
fn create_task_in_folder(
    folder_id: &str,
    change_key: Option<&str>,
    task_item_xml: &str,
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
{task_item_xml}
      </m:Items>
    </m:CreateItem>"#,
    );
    wrap(&body)
}

/// SOAP body for `UpdateItem` against a Task. The envelope is
/// almost identical to `update_calendar_item` — different defaults
/// for the meeting-invitation flags (tasks don't have them) and
/// no need to send meeting cancellations.
fn update_task_item(
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

/// `UpdateFolder` for the task-folder display name. EWS wants the
/// `<t:TasksFolder>` wrapper (not `<t:CalendarFolder>`) so the
/// server knows which folder type it's mutating.
fn update_tasks_folder_displayname(
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
              <t:TasksFolder>
                <t:DisplayName>{name}</t:DisplayName>
              </t:TasksFolder>
            </t:SetFolderField>
          </t:Updates>
        </t:FolderChange>
      </m:FolderChanges>
    </m:UpdateFolder>"#,
    );
    wrap(&body)
}

// ── Parsers ────────────────────────────────────────────────────────────

/// One task folder pulled from a `FindFolder` response.
#[derive(Debug, Clone)]
pub struct ParsedTaskFolder {
    pub folder_id: String,
    pub change_key: Option<String>,
    pub display_name: String,
}

/// Walk a `FindFolderResponse` body emitted with the IPF.Task
/// restriction and yield one `ParsedTaskFolder` per
/// `<t:TasksFolder>` block.
pub fn parse_find_task_folder_response(
    xml: &str,
) -> EwsResult<Vec<ParsedTaskFolder>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut folders = Vec::new();
    let mut inside_folder = false;
    let mut current = ParsedTaskFolder {
        folder_id: String::new(),
        change_key: None,
        display_name: String::new(),
    };
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"tasksfolder" {
                    inside_folder = true;
                    current = ParsedTaskFolder {
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
                if local == b"tasksfolder" {
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

/// One task item pulled from a `FindItem` response.
#[derive(Debug, Clone, Default)]
pub struct ParsedTask {
    pub item_id: String,
    pub change_key: Option<String>,
    pub subject: String,
    pub body: Option<String>,
    pub status: Option<String>,
    pub importance: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub due_date: Option<DateTime<Utc>>,
    pub complete_date: Option<DateTime<Utc>>,
    pub reminder_is_set: bool,
    pub reminder_due_by: Option<DateTime<Utc>>,
    pub created: Option<DateTime<Utc>>,
    pub last_modified: Option<DateTime<Utc>>,
}

/// Walk a `FindItemResponse` body whose `<t:Items>` carries
/// `<t:Task>` rows. Shape mirrors the calendar `parse_find_item_response`
/// but tracks the task-specific fields.
pub fn parse_find_task_item_response(xml: &str) -> EwsResult<Vec<ParsedTask>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut items = Vec::new();
    let mut inside_item = false;
    let mut current = ParsedTask::default();
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"task" {
                    inside_item = true;
                    current = ParsedTask::default();
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
                    b"status" => text_target = Some("status"),
                    b"importance" => text_target = Some("importance"),
                    b"startdate" => text_target = Some("start_date"),
                    b"duedate" => text_target = Some("due_date"),
                    b"completedate" => text_target = Some("complete_date"),
                    b"reminderisset" => text_target = Some("reminder_on"),
                    b"reminderdueby" => text_target = Some("reminder_due_by"),
                    b"datetimecreated" => text_target = Some("created"),
                    b"lastmodifiedtime" => text_target = Some("modified"),
                    _ => {}
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local == b"task" {
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
                        current.body.get_or_insert_with(String::new).push_str(s);
                    }
                    Some("status") => {
                        current.status.get_or_insert_with(String::new).push_str(s);
                    }
                    Some("importance") => {
                        current.importance.get_or_insert_with(String::new).push_str(s);
                    }
                    Some("start_date") => current.start_date = parse_ews_datetime(s),
                    Some("due_date") => current.due_date = parse_ews_datetime(s),
                    Some("complete_date") => {
                        current.complete_date = parse_ews_datetime(s);
                    }
                    Some("reminder_on") => {
                        current.reminder_is_set = s.eq_ignore_ascii_case("true");
                    }
                    Some("reminder_due_by") => {
                        current.reminder_due_by = parse_ews_datetime(s);
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

// ── Mappers (parsed ↔ cal-core) ───────────────────────────────────────

/// Translate a parsed folder into a cal-core `TaskList`. The folder
/// id + change key get packed into a single string (separator `|`,
/// same format CalDAV / calendar folders use) so the command layer
/// can pass it back to `get_tasks` unchanged.
pub fn to_task_list(folder: ParsedTaskFolder) -> TaskList {
    let id = match &folder.change_key {
        Some(ck) => format!("{}|{}", folder.folder_id, ck),
        None => folder.folder_id.clone(),
    };
    TaskList {
        id,
        name: if folder.display_name.is_empty() {
            "Tasks".into()
        } else {
            folder.display_name
        },
        color: None,
        default_sound: None,
        embedded_in_calendar: None,
        read_only: false,
    }
}

/// Translate a parsed task into a cal-core `Task`. EWS UTC datetimes
/// are split into date + optional time per the convention documented
/// at the top of this module: a `00:00:00 UTC` time-of-day round-trips
/// as "no time set", anything else surfaces as the explicit time.
pub fn to_task(item: ParsedTask, list_id: &str) -> EwsResult<Task> {
    let id = encode_task_id(&item.item_id, item.change_key.as_deref());

    let status = item
        .status
        .as_deref()
        .map(ews_status_to_task_status)
        .unwrap_or(TaskStatus::Open);
    let priority = item
        .importance
        .as_deref()
        .map(ews_importance_to_priority)
        .unwrap_or(TaskPriority::Medium);

    let (scheduled_date, scheduled_time) = split_ews_date(item.start_date);
    let (deadline_date, deadline_time) = split_ews_date(item.due_date);

    let reminders = if item.reminder_is_set {
        // EWS surfaces a single reminder via `ReminderDueBy` (the
        // absolute time the reminder fires) — model it as
        // `Reminder::Absolute` so reading + writing don't have to
        // guess at the offset from the task's due date.
        if let Some(at) = item.reminder_due_by {
            vec![cal_core::Reminder {
                kind: cal_core::ReminderKind::Absolute { at },
                sound: None,
            }]
        } else {
            // `ReminderIsSet=true` without a `ReminderDueBy` happens
            // when the server-side reminder fires "at the due date"
            // without a separate timestamp. Synthesise a
            // 0-minute-before relative reminder so the row still
            // shows a reminder in the UI rather than silently
            // dropping it.
            vec![cal_core::Reminder {
                kind: cal_core::ReminderKind::Relative { minutes_before: 0 },
                sound: None,
            }]
        }
    } else {
        Vec::new()
    };

    Ok(Task {
        id,
        list_id: list_id.to_string(),
        title: item.subject,
        description: item.body,
        status,
        priority,
        scheduled_date,
        scheduled_time,
        deadline_date,
        deadline_time,
        recurrence: None,
        parent_id: None,
        color_label: None,
        reminders,
        sound: None,
        created_at: item.created.unwrap_or_else(Utc::now),
        updated_at: item.last_modified.unwrap_or_else(Utc::now),
        completed_at: item.complete_date,
        etag: item.change_key,
    })
}

/// Build the `<t:Task>` body that goes inside a `CreateItem`
/// envelope. Only the fields Aperio actually models get written;
/// anything missing is omitted (EWS treats absent fields as
/// "unset"). The `Recurrence` element is intentionally NOT emitted
/// in this first cut — recurring tasks would need their own
/// `<t:TaskRecurrence>` shape; a `tracing::warn` flags the drop.
pub fn new_task_to_task_item_xml(task: &NewTask) -> String {
    let mut out = String::new();
    out.push_str("        <t:Task>\n");
    out.push_str(&format!(
        "          <t:Subject>{}</t:Subject>\n",
        escape_xml(&task.title)
    ));
    if let Some(desc) = task.description.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "          <t:Body BodyType=\"Text\">{}</t:Body>\n",
            escape_xml(desc)
        ));
    }
    out.push_str(&format!(
        "          <t:Importance>{}</t:Importance>\n",
        priority_to_ews_importance(task.priority),
    ));

    // Reminders: EWS supports a single reminder via `ReminderIsSet`
    // + `ReminderDueBy`. We pull the first Absolute reminder (the
    // shape `to_task` emits on read so the round-trip stays
    // lossless); a Relative reminder is left for the update path to
    // resolve against the task's due date.
    if let Some(at) = first_absolute_reminder(&task.reminders) {
        out.push_str("          <t:ReminderIsSet>true</t:ReminderIsSet>\n");
        out.push_str(&format!(
            "          <t:ReminderDueBy>{}</t:ReminderDueBy>\n",
            format_ews_datetime(at),
        ));
    } else {
        out.push_str("          <t:ReminderIsSet>false</t:ReminderIsSet>\n");
    }

    if let Some(start) = combine_date_time(task.scheduled_date, task.scheduled_time) {
        out.push_str(&format!(
            "          <t:StartDate>{}</t:StartDate>\n",
            format_ews_datetime(start),
        ));
    }
    if let Some(due) = combine_date_time(task.deadline_date, task.deadline_time) {
        out.push_str(&format!(
            "          <t:DueDate>{}</t:DueDate>\n",
            format_ews_datetime(due),
        ));
    }
    out.push_str(&format!(
        "          <t:Status>{}</t:Status>\n",
        task_status_to_ews(task.status),
    ));

    if task.recurrence.is_some() {
        tracing::warn!(
            "EWS task adapter dropping recurrence on write — task recurrence not implemented yet (Phase 6f.2 follow-up)",
        );
    }

    out.push_str("        </t:Task>");
    out
}

/// Build the `<t:Updates>` body for an `UpdateItem` envelope —
/// returns `(set_fields_xml, delete_fields_xml)`. Every field a
/// task has either gets a `SetItemField` (when the value is
/// present) or a `DeleteItemField` (when the user cleared it).
pub fn task_to_update_field_xml(task: &Task) -> (String, String) {
    let mut set = String::new();
    let mut del = String::new();

    push_set_task_string(&mut set, "item:Subject", "Subject", &task.title);
    match task.description.as_deref().filter(|s| !s.is_empty()) {
        Some(desc) => push_set_task_body(&mut set, desc),
        None => del.push_str(&delete_field_xml("item:Body")),
    }
    push_set_task_raw(
        &mut set,
        "item:Importance",
        "Importance",
        priority_to_ews_importance(task.priority),
    );
    push_set_task_raw(
        &mut set,
        "task:Status",
        "Status",
        task_status_to_ews(task.status),
    );

    match combine_date_time(task.scheduled_date, task.scheduled_time) {
        Some(start) => {
            push_set_task_datetime(&mut set, "task:StartDate", "StartDate", start);
        }
        None => del.push_str(&delete_field_xml("task:StartDate")),
    }
    match combine_date_time(task.deadline_date, task.deadline_time) {
        Some(due) => {
            push_set_task_datetime(&mut set, "task:DueDate", "DueDate", due);
        }
        None => del.push_str(&delete_field_xml("task:DueDate")),
    }

    match first_absolute_reminder(&task.reminders) {
        Some(at) => {
            push_set_task_raw(
                &mut set,
                "item:ReminderIsSet",
                "ReminderIsSet",
                "true",
            );
            push_set_task_datetime(
                &mut set,
                "item:ReminderDueBy",
                "ReminderDueBy",
                at,
            );
        }
        None => {
            push_set_task_raw(
                &mut set,
                "item:ReminderIsSet",
                "ReminderIsSet",
                "false",
            );
            del.push_str(&delete_field_xml("item:ReminderDueBy"));
        }
    }

    if task.recurrence.is_some() {
        tracing::warn!(
            "EWS task adapter dropping recurrence on update — task recurrence not implemented yet (Phase 6f.2 follow-up)",
        );
    }

    (set, del)
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Same shape as the calendar adapter's id encoding minus the
/// EventIdKind prefix — tasks don't have a Single/Occurrence split,
/// so the kind discriminator isn't needed.
fn encode_task_id(item_id: &str, change_key: Option<&str>) -> String {
    match change_key {
        Some(ck) => format!("{item_id}|{ck}"),
        None => item_id.to_string(),
    }
}

/// Map EWS `<t:Status>` enum values onto Aperio's `TaskStatus`. The
/// EWS `WaitingOnOthers` and `Deferred` states have no direct Aperio
/// equivalent — both map to `open` so the task still surfaces in
/// the day-start review. Round-trip from Aperio: `cancelled` writes
/// as `Deferred` (closest semantic match — "set aside, not actively
/// worked on").
fn ews_status_to_task_status(s: &str) -> TaskStatus {
    match s {
        "NotStarted" => TaskStatus::Open,
        "InProgress" => TaskStatus::InProgress,
        "Completed" => TaskStatus::Completed,
        "Deferred" => TaskStatus::Cancelled,
        "WaitingOnOthers" => TaskStatus::Open,
        _ => TaskStatus::Open,
    }
}

fn task_status_to_ews(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Open => "NotStarted",
        TaskStatus::InProgress => "InProgress",
        TaskStatus::Completed => "Completed",
        TaskStatus::Cancelled => "Deferred",
    }
}

fn ews_importance_to_priority(s: &str) -> TaskPriority {
    match s {
        "Low" => TaskPriority::Low,
        "High" => TaskPriority::High,
        // EWS surfaces "Normal" for the default and the enum has no
        // other defined values — anything unknown falls back to
        // medium for safety.
        _ => TaskPriority::Medium,
    }
}

fn priority_to_ews_importance(p: TaskPriority) -> &'static str {
    match p {
        TaskPriority::Low => "Low",
        TaskPriority::Medium => "Normal",
        TaskPriority::High => "High",
    }
}

/// EWS task dates come back as UTC `xs:dateTime` even when the user
/// only chose a date. Convention (documented at the top of this
/// module): the UTC date is what Aperio shows as the local date, and
/// a UTC time of exactly `00:00:00` means "no time set". Anything
/// non-midnight surfaces as the explicit time-of-day.
fn split_ews_date(dt: Option<DateTime<Utc>>) -> (Option<NaiveDate>, Option<NaiveTime>) {
    match dt {
        Some(d) => {
            let naive = d.naive_utc();
            let date = naive.date();
            let time = naive.time();
            let time_opt = if time == NaiveTime::from_hms_opt(0, 0, 0).unwrap() {
                None
            } else {
                Some(time)
            };
            (Some(date), time_opt)
        }
        None => (None, None),
    }
}

/// Inverse of `split_ews_date`: place the local date + optional time
/// into the UTC slot the EWS field expects. No timezone math —
/// matches Outlook's task semantics where dates are wall-clock
/// without a zone.
fn combine_date_time(
    date: Option<NaiveDate>,
    time: Option<NaiveTime>,
) -> Option<DateTime<Utc>> {
    let d = date?;
    let t = time.unwrap_or_else(|| NaiveTime::from_hms_opt(0, 0, 0).expect("00:00 valid"));
    Some(DateTime::<Utc>::from_naive_utc_and_offset(
        NaiveDateTime::new(d, t),
        Utc,
    ))
}

/// EWS serialises timestamps as `YYYY-MM-DDTHH:MM:SSZ` (with optional
/// fractional seconds). RFC 3339 parsing handles both shapes.
fn parse_ews_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

fn format_ews_datetime(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn first_absolute_reminder(reminders: &[cal_core::Reminder]) -> Option<DateTime<Utc>> {
    reminders.iter().find_map(|r| match &r.kind {
        cal_core::ReminderKind::Absolute { at } => Some(*at),
        _ => None,
    })
}

fn push_set_task_string(out: &mut String, field_uri: &str, tag: &str, value: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n              <t:Task>\n                <t:{tag}>{value}</t:{tag}>\n              </t:Task>\n            </t:SetItemField>\n",
        value = escape_xml(value),
    ));
}

fn push_set_task_raw(out: &mut String, field_uri: &str, tag: &str, raw_inner: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n              <t:Task>\n                <t:{tag}>{raw_inner}</t:{tag}>\n              </t:Task>\n            </t:SetItemField>\n",
    ));
}

fn push_set_task_datetime(
    out: &mut String,
    field_uri: &str,
    tag: &str,
    value: DateTime<Utc>,
) {
    push_set_task_raw(out, field_uri, tag, &format_ews_datetime(value));
}

fn push_set_task_body(out: &mut String, value: &str) {
    out.push_str(&format!(
        "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"item:Body\"/>\n              <t:Task>\n                <t:Body BodyType=\"Text\">{value}</t:Body>\n              </t:Task>\n            </t:SetItemField>\n",
        value = escape_xml(value),
    ));
}

fn delete_field_xml(field_uri: &str) -> String {
    format!(
        "            <t:DeleteItemField>\n              <t:FieldURI FieldURI=\"{field_uri}\"/>\n            </t:DeleteItemField>\n",
    )
}

fn build_task_from_new(
    new: &NewTask,
    list_id: &str,
    item_id: &str,
    change_key: Option<String>,
) -> Task {
    let now = Utc::now();
    Task {
        id: encode_task_id(item_id, change_key.as_deref()),
        list_id: list_id.to_string(),
        title: new.title.clone(),
        description: new.description.clone(),
        status: new.status,
        priority: new.priority,
        scheduled_date: new.scheduled_date,
        scheduled_time: new.scheduled_time,
        deadline_date: new.deadline_date,
        deadline_time: new.deadline_time,
        recurrence: new.recurrence.clone(),
        parent_id: new.parent_id.clone(),
        color_label: new.color_label.clone(),
        reminders: new.reminders.clone(),
        sound: new.sound.clone(),
        created_at: now,
        updated_at: now,
        completed_at: None,
        etag: change_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::{Reminder, ReminderKind};
    use mockito::Server;

    fn creds() -> crate::auth::BasicCredentials {
        crate::auth::BasicCredentials {
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

    // ── Envelope shape ──────────────────────────────────────────────

    #[test]
    fn find_task_folders_body_restricts_to_ipf_task() {
        let body = find_task_folders();
        assert!(body.contains("FindFolder"));
        assert!(body.contains(r#"Traversal="Deep""#));
        assert!(body.contains("IPF.Task"));
        // Must NOT contain IPF.Appointment — that's the calendar
        // restriction and would surface calendars in the task list.
        assert!(!body.contains("IPF.Appointment"));
    }

    #[test]
    fn find_tasks_body_pulls_task_specific_fields() {
        let body = find_tasks_in_folder("FID", Some("FCK"));
        assert!(body.contains(r#"Id="FID""#));
        assert!(body.contains(r#"ChangeKey="FCK""#));
        assert!(body.contains("task:Status"));
        assert!(body.contains("task:StartDate"));
        assert!(body.contains("task:DueDate"));
        assert!(body.contains("item:Importance"));
        assert!(body.contains("item:ReminderDueBy"));
        // Tasks are listed shallow — CalendarView would be wrong here.
        assert!(body.contains(r#"Traversal="Shallow""#));
        assert!(!body.contains("CalendarView"));
    }

    #[test]
    fn update_tasks_folder_displayname_wraps_in_tasksfolder_not_calendar() {
        let body = update_tasks_folder_displayname("FID", Some("FCK"), "Renamed");
        assert!(body.contains("UpdateFolder"));
        assert!(body.contains("<t:TasksFolder>"));
        // Crucial: must NOT use CalendarFolder — EWS rejects the
        // request with ErrorObjectTypeChanged if the wrapper doesn't
        // match the folder's actual type.
        assert!(!body.contains("<t:CalendarFolder>"));
        assert!(body.contains("Renamed"));
    }

    // ── Status / priority mapping ───────────────────────────────────

    #[test]
    fn status_mapping_round_trips_the_four_primary_states() {
        assert_eq!(ews_status_to_task_status("NotStarted"), TaskStatus::Open);
        assert_eq!(ews_status_to_task_status("InProgress"), TaskStatus::InProgress);
        assert_eq!(ews_status_to_task_status("Completed"), TaskStatus::Completed);
        assert_eq!(ews_status_to_task_status("Deferred"), TaskStatus::Cancelled);

        assert_eq!(task_status_to_ews(TaskStatus::Open), "NotStarted");
        assert_eq!(task_status_to_ews(TaskStatus::InProgress), "InProgress");
        assert_eq!(task_status_to_ews(TaskStatus::Completed), "Completed");
        assert_eq!(task_status_to_ews(TaskStatus::Cancelled), "Deferred");
    }

    #[test]
    fn status_mapping_treats_waiting_on_others_as_open() {
        // "WaitingOnOthers" has no Aperio equivalent — surfacing as
        // open keeps the task visible in the day-start review.
        assert_eq!(
            ews_status_to_task_status("WaitingOnOthers"),
            TaskStatus::Open,
        );
        // Unknown / future EWS status values also default to open
        // rather than silently dropping the row.
        assert_eq!(
            ews_status_to_task_status("SomeNewExchangeStatus"),
            TaskStatus::Open,
        );
    }

    #[test]
    fn importance_mapping_round_trips() {
        assert_eq!(ews_importance_to_priority("Low"), TaskPriority::Low);
        assert_eq!(ews_importance_to_priority("Normal"), TaskPriority::Medium);
        assert_eq!(ews_importance_to_priority("High"), TaskPriority::High);
        assert_eq!(priority_to_ews_importance(TaskPriority::Low), "Low");
        assert_eq!(priority_to_ews_importance(TaskPriority::Medium), "Normal");
        assert_eq!(priority_to_ews_importance(TaskPriority::High), "High");
    }

    // ── Date split / combine ────────────────────────────────────────

    #[test]
    fn date_split_treats_midnight_as_no_time() {
        // EWS round-tripped date-only tasks surface as midnight UTC
        // on the wire — we drop the time so the UI shows "date only".
        let dt = Some(
            DateTime::parse_from_rfc3339("2026-05-20T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let (d, t) = split_ews_date(dt);
        assert_eq!(d, Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()));
        assert_eq!(t, None);
    }

    #[test]
    fn date_split_keeps_explicit_time_of_day() {
        let dt = Some(
            DateTime::parse_from_rfc3339("2026-05-20T14:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let (d, t) = split_ews_date(dt);
        assert_eq!(d, Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()));
        assert_eq!(t, Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap()));
    }

    #[test]
    fn date_combine_writes_midnight_for_date_only() {
        let combined = combine_date_time(
            Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            None,
        )
        .unwrap();
        assert_eq!(
            combined.to_rfc3339(),
            "2026-05-20T00:00:00+00:00",
        );
    }

    #[test]
    fn date_combine_writes_explicit_time_when_set() {
        let combined = combine_date_time(
            Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap()),
        )
        .unwrap();
        assert_eq!(
            combined.to_rfc3339(),
            "2026-05-20T14:30:00+00:00",
        );
    }

    // ── Parsers ─────────────────────────────────────────────────────

    #[test]
    fn parses_two_task_folders_from_find_folder_response() {
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
              <t:TasksFolder>
                <t:FolderId Id="TFA" ChangeKey="TCK1"/>
                <t:DisplayName>Tasks</t:DisplayName>
              </t:TasksFolder>
              <t:TasksFolder>
                <t:FolderId Id="TFB"/>
                <t:DisplayName>Work tasks</t:DisplayName>
              </t:TasksFolder>
            </t:Folders>
          </m:RootFolder>
        </m:FindFolderResponseMessage>
      </m:ResponseMessages>
    </m:FindFolderResponse>
  </s:Body>
</s:Envelope>"#;
        let parsed = parse_find_task_folder_response(xml).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].folder_id, "TFA");
        assert_eq!(parsed[0].change_key.as_deref(), Some("TCK1"));
        assert_eq!(parsed[0].display_name, "Tasks");
        assert_eq!(parsed[1].folder_id, "TFB");
        assert!(parsed[1].change_key.is_none());
    }

    #[test]
    fn parses_task_item_with_full_payload() {
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
              <t:Task>
                <t:ItemId Id="TID-1" ChangeKey="TCK-1"/>
                <t:Subject>Submit report</t:Subject>
                <t:Body BodyType="Text">Draft + review</t:Body>
                <t:DateTimeCreated>2026-05-19T08:00:00Z</t:DateTimeCreated>
                <t:LastModifiedTime>2026-05-19T09:30:00Z</t:LastModifiedTime>
                <t:Importance>High</t:Importance>
                <t:ReminderIsSet>true</t:ReminderIsSet>
                <t:ReminderDueBy>2026-05-20T08:00:00Z</t:ReminderDueBy>
                <t:StartDate>2026-05-20T00:00:00Z</t:StartDate>
                <t:DueDate>2026-05-22T17:00:00Z</t:DueDate>
                <t:Status>InProgress</t:Status>
              </t:Task>
            </t:Items>
          </m:RootFolder>
        </m:FindItemResponseMessage>
      </m:ResponseMessages>
    </m:FindItemResponse>
  </s:Body>
</s:Envelope>"#;
        let items = parse_find_task_item_response(xml).unwrap();
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.item_id, "TID-1");
        assert_eq!(it.subject, "Submit report");
        assert_eq!(it.status.as_deref(), Some("InProgress"));
        assert_eq!(it.importance.as_deref(), Some("High"));
        assert!(it.reminder_is_set);
        assert!(it.start_date.is_some());
        assert!(it.due_date.is_some());
    }

    // ── to_task end-to-end (parsed → cal-core) ──────────────────────

    fn parsed_task_with_reminder() -> ParsedTask {
        ParsedTask {
            item_id: "TID".into(),
            change_key: Some("TCK".into()),
            subject: "Buy bread".into(),
            body: None,
            status: Some("NotStarted".into()),
            importance: Some("Normal".into()),
            start_date: None,
            due_date: Some(
                DateTime::parse_from_rfc3339("2026-05-22T17:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            complete_date: None,
            reminder_is_set: true,
            reminder_due_by: Some(
                DateTime::parse_from_rfc3339("2026-05-22T16:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            created: None,
            last_modified: None,
        }
    }

    #[test]
    fn to_task_maps_reminder_due_by_to_absolute_reminder() {
        let task = to_task(parsed_task_with_reminder(), "LIST|LCK").unwrap();
        assert_eq!(task.id, "TID|TCK");
        assert_eq!(task.list_id, "LIST|LCK");
        assert_eq!(task.reminders.len(), 1);
        match &task.reminders[0].kind {
            ReminderKind::Absolute { at } => {
                assert_eq!(at.to_rfc3339(), "2026-05-22T16:00:00+00:00");
            }
            other => panic!("expected Absolute reminder, got {other:?}"),
        }
    }

    #[test]
    fn to_task_drops_time_when_due_date_is_midnight_utc() {
        let mut parsed = parsed_task_with_reminder();
        parsed.due_date = Some(
            DateTime::parse_from_rfc3339("2026-05-22T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let task = to_task(parsed, "LIST|LCK").unwrap();
        assert_eq!(
            task.deadline_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
        );
        assert_eq!(task.deadline_time, None);
    }

    // ── new_task_to_task_item_xml + task_to_update_field_xml ────────

    fn sample_new_task() -> NewTask {
        NewTask {
            title: "Submit invoice".into(),
            description: Some("Q2 client work".into()),
            status: TaskStatus::Open,
            priority: TaskPriority::High,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            scheduled_time: None,
            deadline_date: Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
            deadline_time: Some(NaiveTime::from_hms_opt(17, 0, 0).unwrap()),
            recurrence: None,
            parent_id: None,
            color_label: None,
            reminders: vec![Reminder {
                kind: ReminderKind::Absolute {
                    at: DateTime::parse_from_rfc3339("2026-05-22T16:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                },
                sound: None,
            }],
            sound: None,
        }
    }

    #[test]
    fn create_task_xml_carries_every_field_we_support() {
        let xml = new_task_to_task_item_xml(&sample_new_task());
        // Wrapper element + each field
        assert!(xml.contains("<t:Task>"));
        assert!(xml.contains("<t:Subject>Submit invoice</t:Subject>"));
        assert!(xml.contains("Q2 client work"));
        assert!(xml.contains("<t:Importance>High</t:Importance>"));
        assert!(xml.contains("<t:Status>NotStarted</t:Status>"));
        assert!(xml.contains("<t:StartDate>2026-05-20T00:00:00Z</t:StartDate>"));
        assert!(xml.contains("<t:DueDate>2026-05-22T17:00:00Z</t:DueDate>"));
        assert!(xml.contains("<t:ReminderIsSet>true</t:ReminderIsSet>"));
        assert!(xml.contains("<t:ReminderDueBy>2026-05-22T16:00:00Z</t:ReminderDueBy>"));
    }

    #[test]
    fn update_task_xml_deletes_cleared_fields() {
        let task = Task {
            id: "TID|TCK".into(),
            list_id: "LIST|LCK".into(),
            title: "Title only".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            etag: Some("TCK".into()),
        };
        let (set, del) = task_to_update_field_xml(&task);
        // Subject + Importance + Status are always set
        assert!(set.contains("item:Subject"));
        assert!(set.contains("task:Status"));
        // Body, dates, ReminderDueBy → DeleteItemField since they're
        // cleared
        assert!(del.contains("item:Body"));
        assert!(del.contains("task:StartDate"));
        assert!(del.contains("task:DueDate"));
        assert!(del.contains("item:ReminderDueBy"));
        // ReminderIsSet is set to false (not deleted, because it's a
        // mandatory bool field)
        assert!(set.contains("<t:ReminderIsSet>false</t:ReminderIsSet>"));
    }

    // ── End-to-end api flows via mockito ────────────────────────────

    #[tokio::test]
    async fn list_task_lists_round_trips() {
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
              <t:TasksFolder>
                <t:FolderId Id="TFA" ChangeKey="K1"/>
                <t:DisplayName>Tasks</t:DisplayName>
              </t:TasksFolder>
            </t:Folders>
          </m:RootFolder>
        </m:FindFolderResponseMessage>
      </m:ResponseMessages>
    </m:FindFolderResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("IPF.Task".into()))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let lists = list_task_lists(&client_for(&server)).await.unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].id, "TFA|K1");
        assert_eq!(lists[0].name, "Tasks");
    }

    #[tokio::test]
    async fn create_task_returns_server_assigned_id() {
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:CreateItemResponse>
      <m:ResponseMessages>
        <m:CreateItemResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:Items>
            <t:Task>
              <t:ItemId Id="NEW-TID" ChangeKey="NEW-TCK"/>
            </t:Task>
          </m:Items>
        </m:CreateItemResponseMessage>
      </m:ResponseMessages>
    </m:CreateItemResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let task = create_task(&client_for(&server), "LIST|LCK", sample_new_task())
            .await
            .unwrap();
        assert_eq!(task.id, "NEW-TID|NEW-TCK");
        assert_eq!(task.list_id, "LIST|LCK");
        assert_eq!(task.etag.as_deref(), Some("NEW-TCK"));
    }

    #[tokio::test]
    async fn update_task_advances_change_key_in_id() {
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
            <t:Task>
              <t:ItemId Id="TID" ChangeKey="TCK-V2"/>
            </t:Task>
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
        let starting = Task {
            id: "TID|TCK-V1".into(),
            list_id: "LIST|LCK".into(),
            title: "Updated".into(),
            description: None,
            status: TaskStatus::Completed,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            etag: Some("TCK-V1".into()),
        };
        let updated = update_task(&client_for(&server), &starting).await.unwrap();
        assert_eq!(updated.id, "TID|TCK-V2");
        assert_eq!(updated.etag.as_deref(), Some("TCK-V2"));
    }

    #[tokio::test]
    async fn delete_task_round_trips() {
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
            .match_body(mockito::Matcher::Regex(r#"Id="TID""#.into()))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        delete_task(&client_for(&server), "TID|TCK").await.unwrap();
    }
}


