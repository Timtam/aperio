//! `ContactsFeature` implementation against the local SQLite store.
//!
//! Mirrors the shape of `tasks.rs` and `calendars.rs`:
//!
//!   - The SQLite tables (`contact_lists`, `contacts`) come from
//!     migration 0007. The seed row `local-default-contacts` is
//!     created there; we don't try to recreate it lazily.
//!   - Multi-valued fields (`emails`, `phone_numbers`) live in the
//!     DB as JSON-encoded arrays — same trade-off the tasks
//!     `reminders` column makes. The encode/decode helpers from
//!     `mapping.rs` (`encode_json` / `decode_json`) carry the
//!     conversion both ways.
//!   - Cross-list `search_contacts` does a case-insensitive `LIKE
//!     '%query%'` over `display_name`, `given_name`, `family_name`,
//!     `organization`, and `emails`. FTS5 is reserved for migration
//!     0008 when the attendees picker (§10.4) needs it; the LIKE
//!     query is fine for the current single-source workload.

use async_trait::async_trait;
use cal_core::{
    Contact, ContactList, ContactPhoto, ContactsFeature, ContainerColor, Error as CoreError,
    NewContact, Result as CoreResult,
};
use chrono::Utc;
use rusqlite::{params, Row};
use uuid::Uuid;

use crate::mapping::{
    decode_json, encode_json, fmt_date, fmt_utc, opt_text, parse_date, parse_utc,
    read_bool, read_container_color, req_text, write_container_color,
};
use crate::{map_sql_err, LocalAdapter, SOURCE_ID};

/// Hard-coded id of the seed local contact list (see migration
/// 0007). Exposed so the eventual command layer can land
/// newly-created local contacts in it without a name lookup.
pub const LOCAL_DEFAULT_CONTACT_LIST_ID: &str = "local-default-contacts";

impl LocalAdapter {
    /// Insert a new contact list under the local account. Mirrors
    /// `create_task_list`; useful for tests and for a future
    /// "add address book" UI surface.
    pub fn create_contact_list(
        &self,
        name: &str,
        color: Option<ContainerColor>,
    ) -> CoreResult<ContactList> {
        let id = Uuid::new_v4().to_string();
        let now_s = fmt_utc(&Utc::now());
        let (color_hex, color_source) = write_container_color(&color);
        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO contact_lists (
                    id, account_id, source, name, color_hex, color_source,
                    read_only, etag, created_at, updated_at
                 ) VALUES (?, 'local', ?, ?, ?, ?, 0, NULL, ?, ?)",
                params![
                    id,
                    SOURCE_ID,
                    name,
                    color_hex,
                    color_source,
                    now_s,
                    now_s,
                ],
            )
            .map_err(map_sql_err)?;
        Ok(ContactList {
            id,
            name: name.to_string(),
            color,
            read_only: false,
        })
    }

    /// Drop a contact list and every contact under it (the FK uses
    /// ON DELETE CASCADE). The local-default-contacts seed list
    /// can't be removed this way — same protection the local
    /// account itself gets.
    pub fn delete_contact_list(&self, id: &str) -> CoreResult<()> {
        if id == LOCAL_DEFAULT_CONTACT_LIST_ID {
            return Err(CoreError::Forbidden(
                "the default local contact list cannot be deleted".into(),
            ));
        }
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "DELETE FROM contact_lists WHERE id = ? AND source = 'local'",
                params![id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "contact list '{id}' not found"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl ContactsFeature for LocalAdapter {
    async fn list_contact_lists(&self) -> CoreResult<Vec<ContactList>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color_hex, color_source, read_only
                 FROM contact_lists
                 WHERE source = 'local'
                 ORDER BY name COLLATE NOCASE",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], row_to_contact_list)
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }

    async fn get_contacts(&self, list_id: &str) -> CoreResult<Vec<Contact>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        // `photo_data IS NOT NULL` projects the has_photo flag
        // without materialising the BLOB — SQLite skips reading
        // the bytes off disk when we don't reference the column
        // directly, so the listing query stays cheap even for
        // contacts with megabyte-sized avatars.
        let mut stmt = conn
            .prepare(
                "SELECT id, list_id, display_name, given_name, family_name,
                        organization, emails, phone_numbers, birthday, notes,
                        etag, created_at, updated_at, members,
                        (photo_data IS NOT NULL) AS has_photo
                 FROM contacts
                 WHERE list_id = ?
                 ORDER BY display_name COLLATE NOCASE",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map(params![list_id], row_to_contact)
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }

    async fn search_contacts(&self, query: &str) -> CoreResult<Vec<Contact>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        // Migration 0008 fills the `contacts_fts` mirror; the
        // tokeniser is `unicode61 remove_diacritics 2` (same as
        // events_fts / tasks_fts) so "Müller" matches "muller"
        // without the picker UI having to think about it. The
        // prepared query is a space-separated list of
        // `prefix*`-style tokens — short typeahead strings ("ma"
        // → "ma*") match "Max" before the user finishes typing.
        let prepared = prepare_fts_query(trimmed);
        if prepared.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT c.id, c.list_id, c.display_name, c.given_name, c.family_name,
                        c.organization, c.emails, c.phone_numbers, c.birthday, c.notes,
                        c.etag, c.created_at, c.updated_at, c.members,
                        (c.photo_data IS NOT NULL) AS has_photo
                 FROM contacts_fts f
                 JOIN contacts c ON c.id = f.id
                 WHERE contacts_fts MATCH ?
                 ORDER BY rank
                 LIMIT 50",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map(params![prepared], row_to_contact)
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }

    async fn create_contact(
        &self,
        list_id: &str,
        contact: NewContact,
    ) -> CoreResult<Contact> {
        if contact.display_name.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "display_name must not be empty".into(),
            ));
        }
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_s = fmt_utc(&now);
        let emails_json = encode_json(&contact.emails)?;
        let phones_json = encode_json(&contact.phone_numbers)?;
        let birthday_s = contact.birthday.as_ref().map(fmt_date);
        // Members column is NULL for regular contacts and a JSON
        // array (possibly empty) for distribution lists. Migration
        // 0009 added the column; old rows stay NULL on upgrade.
        let members_json = match contact.members.as_ref() {
            Some(m) => Some(encode_json(m)?),
            None => None,
        };
        // Photo travels inline on create — Some ⇒ both columns
        // get populated, None ⇒ both stay NULL. Splitting it into
        // a follow-up `set_contact_photo` after a create would
        // double the round-trip count and require the caller to
        // worry about the in-between state.
        let (photo_data, photo_content_type) = match contact.photo.as_ref() {
            Some(p) => (Some(p.data.clone()), Some(p.content_type.clone())),
            None => (None, None),
        };
        let has_photo = photo_data.is_some();

        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO contacts (
                    id, list_id, display_name, given_name, family_name,
                    organization, emails, phone_numbers, birthday, notes,
                    etag, created_at, updated_at, members,
                    photo_data, photo_content_type
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?)",
                params![
                    id,
                    list_id,
                    contact.display_name.trim(),
                    contact.given_name.as_deref(),
                    contact.family_name.as_deref(),
                    contact.organization.as_deref(),
                    emails_json,
                    phones_json,
                    birthday_s,
                    contact.notes.as_deref(),
                    now_s,
                    now_s,
                    members_json,
                    photo_data,
                    photo_content_type,
                ],
            )
            .map_err(map_sql_err)?;

        Ok(Contact {
            id,
            list_id: list_id.to_string(),
            display_name: contact.display_name.trim().to_string(),
            given_name: contact.given_name,
            family_name: contact.family_name,
            organization: contact.organization,
            emails: contact.emails,
            phone_numbers: contact.phone_numbers,
            birthday: contact.birthday,
            notes: contact.notes,
            members: contact.members,
            has_photo,
            created_at: now,
            updated_at: now,
            etag: None,
        })
    }

    async fn update_contact(&self, contact: Contact) -> CoreResult<Contact> {
        if contact.display_name.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "display_name must not be empty".into(),
            ));
        }
        let now = Utc::now();
        let now_s = fmt_utc(&now);
        let emails_json = encode_json(&contact.emails)?;
        let phones_json = encode_json(&contact.phone_numbers)?;
        let birthday_s = contact.birthday.as_ref().map(fmt_date);
        let members_json = match contact.members.as_ref() {
            Some(m) => Some(encode_json(m)?),
            None => None,
        };

        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE contacts
                    SET list_id = ?, display_name = ?, given_name = ?,
                        family_name = ?, organization = ?, emails = ?,
                        phone_numbers = ?, birthday = ?, notes = ?,
                        members = ?, updated_at = ?
                  WHERE id = ?",
                params![
                    contact.list_id,
                    contact.display_name.trim(),
                    contact.given_name.as_deref(),
                    contact.family_name.as_deref(),
                    contact.organization.as_deref(),
                    emails_json,
                    phones_json,
                    birthday_s,
                    contact.notes.as_deref(),
                    members_json,
                    now_s,
                    contact.id,
                ],
            )
            .map_err(map_sql_err)?;

        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "contact '{}' not found",
                contact.id
            )));
        }
        Ok(Contact {
            updated_at: now,
            ..contact
        })
    }

    async fn delete_contact(&self, contact_id: &str) -> CoreResult<()> {
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM contacts WHERE id = ?", params![contact_id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "contact '{contact_id}' not found"
            )));
        }
        Ok(())
    }

    async fn get_contact_photo(
        &self,
        contact_id: &str,
    ) -> CoreResult<Option<ContactPhoto>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        // First pass: confirm the contact row exists at all so we
        // can distinguish "no photo on a real contact" from "id
        // typo / stale frontend reference". Cheap — primary-key
        // lookup, no BLOB I/O.
        let exists: i64 = conn
            .query_row(
                "SELECT 1 FROM contacts WHERE id = ?",
                params![contact_id],
                |row| row.get(0),
            )
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(0),
                other => Err(other),
            })
            .map_err(map_sql_err)?;
        if exists == 0 {
            return Err(CoreError::NotFound(format!(
                "contact '{contact_id}' not found"
            )));
        }
        // Second pass: pull the BLOB and the content type. We do
        // them in two statements so the existence check can stay
        // cheap — selecting the BLOB up front would force SQLite
        // to materialise the bytes on every "is there a photo?"
        // probe.
        let row: (Option<Vec<u8>>, Option<String>) = conn
            .query_row(
                "SELECT photo_data, photo_content_type
                 FROM contacts WHERE id = ?",
                params![contact_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(map_sql_err)?;
        match row {
            (Some(data), Some(content_type)) => Ok(Some(ContactPhoto {
                content_type,
                data,
            })),
            // No photo set, or content type missing (shouldn't
            // happen — the columns are written as a pair — but
            // we tolerate it). Treat as "no photo".
            _ => Ok(None),
        }
    }

    async fn set_contact_photo(
        &self,
        contact_id: &str,
        photo: ContactPhoto,
    ) -> CoreResult<()> {
        if photo.content_type.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "photo content_type must not be empty".into(),
            ));
        }
        if photo.data.is_empty() {
            return Err(CoreError::InvalidInput(
                "photo data must not be empty".into(),
            ));
        }
        let now_s = fmt_utc(&Utc::now());
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE contacts
                    SET photo_data = ?, photo_content_type = ?,
                        updated_at = ?
                  WHERE id = ?",
                params![photo.data, photo.content_type, now_s, contact_id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "contact '{contact_id}' not found"
            )));
        }
        Ok(())
    }

    async fn delete_contact_photo(&self, contact_id: &str) -> CoreResult<()> {
        let now_s = fmt_utc(&Utc::now());
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE contacts
                    SET photo_data = NULL, photo_content_type = NULL,
                        updated_at = ?
                  WHERE id = ?",
                params![now_s, contact_id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "contact '{contact_id}' not found"
            )));
        }
        Ok(())
    }

    async fn rename_contact_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        if new_name.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "contact list name must not be empty".into(),
            ));
        }
        let now_s = fmt_utc(&Utc::now());
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE contact_lists
                    SET name = ?, updated_at = ?
                  WHERE id = ? AND source = 'local'",
                params![new_name.trim(), now_s, list_id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "contact list '{list_id}' not found"
            )));
        }
        Ok(())
    }
}

// ── FTS5 query sanitiser ───────────────────────────────────────────────

/// Turn a free-form picker input into an FTS5 MATCH expression.
///
/// More aggressive sanitiser than `search.rs::prepare_query`
/// because the attendees picker is a real-world email-paste
/// surface — strings like `"max@example.com"` are typed
/// regularly, and FTS5 treats `@` / `.` as syntax markers
/// (`column@row`, …) that produce a "syntax error" on MATCH and
/// kill the typeahead. To dodge that:
///
///   - Walk every character. Letters, digits, dash and underscore
///     pass through; everything else (incl. `@`, `.`, `,`, `:`,
///     `*`, `"`, `(`, `)`, `^`, etc.) becomes whitespace. That
///     mirrors how FTS5's `unicode61` tokeniser splits on the
///     index side — querying for the same atoms it indexed.
///   - Re-split on whitespace and lowercase each token. FTS5's
///     query parser still treats uppercase AND / OR / NOT / NEAR
///     as operators with a `*` suffix; lowercasing dodges that.
///   - Append `*` to every token so a half-typed prefix lands
///     hits as soon as a useful prefix is in.
///   - Drop empty tokens so trailing punctuation doesn't fall
///     through.
///
/// Result example: `"jane@example, dr."` →
/// `jane* example* dr*`.
///
/// Returns the empty string when every token was filtered out —
/// the caller treats that as "no query, no results".
fn prepare_fts_query(input: &str) -> String {
    let scrubbed: String = input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect();
    scrubbed
        .split_whitespace()
        .map(|tok| tok.to_lowercase())
        .filter(|tok| !tok.is_empty())
        .map(|tok| format!("{tok}*"))
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Row decoders ───────────────────────────────────────────────────────

fn row_to_contact_list(row: &Row<'_>) -> rusqlite::Result<CoreResult<ContactList>> {
    let row_result: CoreResult<ContactList> = (|| {
        let id = req_text(row, 0)?;
        let name = req_text(row, 1)?;
        let color = read_container_color(row, 2, 3)?;
        let read_only = read_bool(row, 4)?;
        Ok(ContactList {
            id,
            name,
            color,
            read_only,
        })
    })();
    Ok(row_result)
}

fn row_to_contact(row: &Row<'_>) -> rusqlite::Result<CoreResult<Contact>> {
    let row_result: CoreResult<Contact> = (|| {
        let id = req_text(row, 0)?;
        let list_id = req_text(row, 1)?;
        let display_name = req_text(row, 2)?;
        let given_name = opt_text(row, 3)?;
        let family_name = opt_text(row, 4)?;
        let organization = opt_text(row, 5)?;
        let emails_json = req_text(row, 6)?;
        let phones_json = req_text(row, 7)?;
        let birthday = opt_text(row, 8)?
            .map(|s| parse_date(&s))
            .transpose()?;
        let notes = opt_text(row, 9)?;
        let etag = opt_text(row, 10)?;
        let created_at = parse_utc(&req_text(row, 11)?)?;
        let updated_at = parse_utc(&req_text(row, 12)?)?;
        let emails: Vec<String> = decode_json(&emails_json)?;
        let phone_numbers: Vec<String> = decode_json(&phones_json)?;
        // Members column (added in migration 0009) carries the
        // distribution-list payload. NULL ⇒ regular contact;
        // JSON array (possibly empty) ⇒ this is a group.
        let members_json: Option<String> = opt_text(row, 13)?;
        let members = match members_json {
            Some(s) => Some(decode_json::<Vec<cal_core::GroupMember>>(&s)?),
            None => None,
        };
        // `has_photo` is the projected `photo_data IS NOT NULL`
        // boolean column (added in migration 0010). SQLite emits
        // it as an integer 0 / 1; `read_bool` lets us decode it
        // the same way `read_only` is decoded on contact lists.
        let has_photo = read_bool(row, 14)?;
        Ok(Contact {
            id,
            list_id,
            display_name,
            given_name,
            family_name,
            organization,
            emails,
            phone_numbers,
            birthday,
            notes,
            members,
            has_photo,
            created_at,
            updated_at,
            etag,
        })
    })();
    Ok(row_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_test_db;
    use chrono::NaiveDate;

    fn fixture_adapter() -> LocalAdapter {
        LocalAdapter::new(open_test_db())
    }

    fn sample_new_contact() -> NewContact {
        NewContact {
            display_name: "Max Mustermann".into(),
            given_name: Some("Max".into()),
            family_name: Some("Mustermann".into()),
            organization: Some("Example GmbH".into()),
            emails: vec!["max@example.com".into(), "m.muster@example.org".into()],
            phone_numbers: vec!["+49 30 1234567".into()],
            birthday: Some(NaiveDate::from_ymd_opt(1985, 4, 17).unwrap()),
            notes: Some("Met at conf 2024".into()),
            members: None,
            photo: None,
        }
    }

    /// 1x1 PNG, baked here so the photo round-trip tests don't
    /// need any external fixtures. The bytes are a real PNG
    /// (signature + IHDR + IDAT + IEND) — small enough to embed
    /// inline, large enough to verify BLOB storage actually keeps
    /// the original bits.
    fn sample_photo() -> ContactPhoto {
        const PNG_1X1: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xfa, 0xcf, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xe5, 0x27, 0xde, 0xfc,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        ContactPhoto {
            content_type: "image/png".into(),
            data: PNG_1X1.to_vec(),
        }
    }

    #[tokio::test]
    async fn seed_list_is_present_at_startup() {
        let adapter = fixture_adapter();
        let lists = adapter.list_contact_lists().await.unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].id, LOCAL_DEFAULT_CONTACT_LIST_ID);
        assert_eq!(lists[0].name, "Contacts");
        assert!(!lists[0].read_only);
    }

    #[tokio::test]
    async fn create_contact_persists_all_fields() {
        let adapter = fixture_adapter();
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        assert_eq!(created.display_name, "Max Mustermann");
        assert_eq!(created.emails.len(), 2);
        assert_eq!(created.organization.as_deref(), Some("Example GmbH"));

        // Round-trip — listing the contacts should give us the same
        // values back, including the JSON-encoded email array.
        let all = adapter
            .get_contacts(LOCAL_DEFAULT_CONTACT_LIST_ID)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, created.id);
        assert_eq!(all[0].emails, created.emails);
        assert_eq!(all[0].birthday, created.birthday);
    }

    #[tokio::test]
    async fn create_rejects_blank_display_name() {
        let adapter = fixture_adapter();
        let mut bad = sample_new_contact();
        bad.display_name = "   ".into();
        let err = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, bad)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn update_changes_fields() {
        let adapter = fixture_adapter();
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        let mut edited = created.clone();
        edited.display_name = "Maximilian Muster".into();
        edited.emails = vec!["new@example.com".into()];
        edited.notes = None;
        let updated = adapter.update_contact(edited).await.unwrap();
        assert_eq!(updated.display_name, "Maximilian Muster");
        assert_eq!(updated.emails, vec!["new@example.com".to_string()]);
        assert!(updated.notes.is_none());
        // updated_at should have advanced; we don't assert on the
        // exact value (it's `Utc::now()`) but it must not equal
        // `created_at`.
        assert!(updated.updated_at >= updated.created_at);
    }

    #[tokio::test]
    async fn update_missing_id_yields_not_found() {
        let adapter = fixture_adapter();
        let ghost = Contact {
            id: "does-not-exist".into(),
            list_id: LOCAL_DEFAULT_CONTACT_LIST_ID.into(),
            display_name: "Ghost".into(),
            given_name: None,
            family_name: None,
            organization: None,
            emails: Vec::new(),
            phone_numbers: Vec::new(),
            birthday: None,
            notes: None,
            members: None,
            has_photo: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
        };
        let err = adapter.update_contact(ghost).await.unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_contact_removes_row() {
        let adapter = fixture_adapter();
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        adapter.delete_contact(&created.id).await.unwrap();
        let all = adapter
            .get_contacts(LOCAL_DEFAULT_CONTACT_LIST_ID)
            .await
            .unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn delete_missing_yields_not_found() {
        let adapter = fixture_adapter();
        let err = adapter
            .delete_contact("not-real")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn search_finds_partial_name_match() {
        let adapter = fixture_adapter();
        let _ = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        // Different name + email AND different family/organisation,
        // so "muster" matches the first row only and "jane@" matches
        // the second row only. Re-using `sample_new_contact()` and
        // only patching display_name + emails would leave
        // `family_name = Mustermann` on the second row too, which
        // would split the "muster" search across both contacts.
        let second = NewContact {
            display_name: "Jane Doe".into(),
            given_name: Some("Jane".into()),
            family_name: Some("Doe".into()),
            organization: Some("Beispiel AG".into()),
            emails: vec!["jane@example.com".into()],
            phone_numbers: Vec::new(),
            birthday: None,
            notes: None,
            members: None,
            photo: None,
        };
        let _ = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, second)
            .await
            .unwrap();

        // Case-insensitive prefix match on family_name.
        let hits = adapter.search_contacts("muster").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].display_name, "Max Mustermann");

        // Email substring hits too.
        let mail_hits = adapter.search_contacts("jane@").await.unwrap();
        assert_eq!(mail_hits.len(), 1);
        assert_eq!(mail_hits[0].display_name, "Jane Doe");

        // Empty / whitespace query → no fan-out.
        assert!(adapter.search_contacts("   ").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_contact_list_round_trips() {
        let adapter = fixture_adapter();
        let list = adapter
            .create_contact_list(
                "Friends",
                Some(ContainerColor::custom("#ff00ff")),
            )
            .unwrap();
        let lists = adapter.list_contact_lists().await.unwrap();
        // Seed + new one = 2; ordered case-insensitively by name.
        assert_eq!(lists.len(), 2);
        assert!(lists.iter().any(|l| l.id == list.id));
        let friends = lists.iter().find(|l| l.id == list.id).unwrap();
        assert_eq!(friends.color.as_ref().unwrap().hex, "#ff00ff");
    }

    #[tokio::test]
    async fn cannot_delete_default_contact_list() {
        let adapter = fixture_adapter();
        let err = adapter
            .delete_contact_list(LOCAL_DEFAULT_CONTACT_LIST_ID)
            .unwrap_err();
        assert!(matches!(err, CoreError::Forbidden(_)));
    }

    #[tokio::test]
    async fn rename_contact_list_updates_name() {
        let adapter = fixture_adapter();
        adapter
            .rename_contact_list(LOCAL_DEFAULT_CONTACT_LIST_ID, "Kontakte")
            .await
            .unwrap();
        let lists = adapter.list_contact_lists().await.unwrap();
        assert_eq!(lists[0].name, "Kontakte");
    }

    #[tokio::test]
    async fn rename_rejects_blank_name() {
        let adapter = fixture_adapter();
        let err = adapter
            .rename_contact_list(LOCAL_DEFAULT_CONTACT_LIST_ID, "  ")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn search_folds_diacritics() {
        let adapter = fixture_adapter();
        let mut müller = sample_new_contact();
        müller.display_name = "Müller".into();
        müller.family_name = Some("Müller".into());
        müller.given_name = Some("Anna".into());
        müller.emails = vec!["anna@example.com".into()];
        adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, müller)
            .await
            .unwrap();
        // `unicode61 remove_diacritics 2` folds umlauts, so the
        // ASCII spelling lands on the umlaut row.
        let hits = adapter.search_contacts("mull").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].display_name, "Müller");
    }

    #[tokio::test]
    async fn search_prefix_matches_short_typeahead() {
        let adapter = fixture_adapter();
        adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        // "ma" → "ma*" matches "Max Mustermann" through the
        // prepare_fts_query suffix.
        let hits = adapter.search_contacts("ma").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].display_name, "Max Mustermann");
    }

    #[tokio::test]
    async fn search_handles_fts_special_chars() {
        let adapter = fixture_adapter();
        adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        // A query containing FTS5 syntax (`:`, `(`, `*`) used to
        // either crash or match nothing — the sanitiser strips
        // those so the underlying token still hits.
        let hits = adapter.search_contacts("max:").await.unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn search_picks_up_email_substring() {
        let adapter = fixture_adapter();
        adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        // The emails column is indexed via the FTS mirror, so a
        // domain-shaped query lands on the row even though no
        // name fragment matches. Picker UX: typing "@example"
        // (after stripping the special chars to "example") should
        // surface every contact at that domain.
        let hits = adapter.search_contacts("example").await.unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn search_reflects_list_rename() {
        let adapter = fixture_adapter();
        adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        // Hit before rename via the list_name column.
        assert_eq!(
            adapter.search_contacts("Contacts").await.unwrap().len(),
            1,
        );
        adapter
            .rename_contact_list(LOCAL_DEFAULT_CONTACT_LIST_ID, "Friends")
            .await
            .unwrap();
        // Old name no longer matches; new name now does. Proves
        // the `contact_lists_fts_rename` trigger rewrites the
        // denormalised column.
        assert_eq!(
            adapter.search_contacts("Contacts").await.unwrap().len(),
            0,
        );
        assert_eq!(
            adapter.search_contacts("Friends").await.unwrap().len(),
            1,
        );
    }

    #[tokio::test]
    async fn delete_list_cascades_to_contacts() {
        let adapter = fixture_adapter();
        let list = adapter
            .create_contact_list("Burner list", None)
            .unwrap();
        let _ = adapter
            .create_contact(&list.id, sample_new_contact())
            .await
            .unwrap();
        adapter.delete_contact_list(&list.id).unwrap();
        // The seed list's contacts (none) shouldn't have been touched.
        let seed = adapter
            .get_contacts(LOCAL_DEFAULT_CONTACT_LIST_ID)
            .await
            .unwrap();
        assert!(seed.is_empty());
        // The deleted list is gone.
        let lists = adapter.list_contact_lists().await.unwrap();
        assert_eq!(lists.len(), 1);
    }

    #[tokio::test]
    async fn create_carries_inline_photo_through_to_listing() {
        let adapter = fixture_adapter();
        let mut payload = sample_new_contact();
        payload.photo = Some(sample_photo());
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, payload)
            .await
            .unwrap();
        // Returned struct reflects the photo presence immediately.
        assert!(created.has_photo);

        // And the listing path projects the same flag without
        // having to hit the BLOB column.
        let listed = adapter
            .get_contacts(LOCAL_DEFAULT_CONTACT_LIST_ID)
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].has_photo);
    }

    #[tokio::test]
    async fn get_contact_photo_round_trips() {
        let adapter = fixture_adapter();
        let mut payload = sample_new_contact();
        payload.photo = Some(sample_photo());
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, payload)
            .await
            .unwrap();
        let fetched = adapter
            .get_contact_photo(&created.id)
            .await
            .unwrap()
            .expect("photo present");
        assert_eq!(fetched.content_type, "image/png");
        assert_eq!(fetched.data, sample_photo().data);
    }

    #[tokio::test]
    async fn get_photo_returns_none_when_no_photo_set() {
        let adapter = fixture_adapter();
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        // Contact exists, but the column pair is NULL ⇒ Ok(None).
        let result = adapter.get_contact_photo(&created.id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_photo_returns_not_found_for_unknown_id() {
        let adapter = fixture_adapter();
        let err = adapter
            .get_contact_photo("does-not-exist")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn set_photo_replaces_existing_bytes() {
        let adapter = fixture_adapter();
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        // Set #1.
        adapter
            .set_contact_photo(&created.id, sample_photo())
            .await
            .unwrap();
        // Set #2 — different content type + bytes — must overwrite.
        let other = ContactPhoto {
            content_type: "image/jpeg".into(),
            // Two-byte stub stands in for a different image; the
            // assertion below confirms the new bytes won.
            data: vec![0xff, 0xd8],
        };
        adapter
            .set_contact_photo(&created.id, other.clone())
            .await
            .unwrap();
        let fetched = adapter
            .get_contact_photo(&created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.content_type, other.content_type);
        assert_eq!(fetched.data, other.data);
    }

    #[tokio::test]
    async fn set_photo_rejects_empty_payload() {
        let adapter = fixture_adapter();
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, sample_new_contact())
            .await
            .unwrap();
        let empty = ContactPhoto {
            content_type: "image/png".into(),
            data: Vec::new(),
        };
        let err = adapter
            .set_contact_photo(&created.id, empty)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn delete_photo_clears_the_flag() {
        let adapter = fixture_adapter();
        let mut payload = sample_new_contact();
        payload.photo = Some(sample_photo());
        let created = adapter
            .create_contact(LOCAL_DEFAULT_CONTACT_LIST_ID, payload)
            .await
            .unwrap();
        adapter
            .delete_contact_photo(&created.id)
            .await
            .unwrap();
        // get_contact_photo returns None and the listing's flag
        // flips back to false.
        assert!(adapter
            .get_contact_photo(&created.id)
            .await
            .unwrap()
            .is_none());
        let listed = adapter
            .get_contacts(LOCAL_DEFAULT_CONTACT_LIST_ID)
            .await
            .unwrap();
        assert!(!listed[0].has_photo);
    }

    #[tokio::test]
    async fn delete_photo_on_unknown_id_yields_not_found() {
        let adapter = fixture_adapter();
        let err = adapter
            .delete_contact_photo("nope")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }
}
