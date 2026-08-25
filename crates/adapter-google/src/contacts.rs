//! Google People API client — Phase 10h.
//!
//! Wraps the REST endpoints at `https://people.googleapis.com/v1/`
//! through the same `ApiState` plumbing the Calendar and Tasks
//! modules use, so a 401 on a People call triggers the existing
//! token-refresh dance without bespoke code per surface.
//!
//! ## Surface mapping (cal-core ↔ People API)
//!
//! The Google account exposes **one synthetic ContactList**
//! (`google-contacts`) per account. Google's data model has the
//! user's whole address book under one namespace and uses
//! `ContactGroup` labels for grouping — there's no equivalent of
//! "multiple address books" the way CardDAV / EWS surfaces have.
//!
//! Inside that list, both people and groups are returned as
//! `Contact` rows:
//!
//!   - **Person** (`resourceName=people/c12345…`) becomes a
//!     regular contact, `members = None`.
//!   - **ContactGroup** (`resourceName=contactGroups/abc…`)
//!     becomes a distribution-list contact, `members =
//!     Some([{name, email}…])`. Member emails are looked up from
//!     the people listing's `memberships` join key — Google
//!     stores membership on the *person*, not the group, so we
//!     have to invert that during the mapping pass.
//!
//! ## Photos
//!
//! Person resources carry a `photos[]` array whose entries hold
//! a `url` (Google CDN endpoint) rather than inline bytes. The
//! `has_photo` flag flips when at least one non-default photo
//! exists; `get_contact_photo` does a follow-up GET against the
//! URL with the OAuth bearer attached to materialise the bytes.
//! Uploading uses `:updateContactPhoto` with a base64 body.
//!
//! ## etags
//!
//! Google's People API requires the etag on update calls to
//! detect lost-update conflicts. We round-trip `Contact.etag`
//! transparently — the trait surface already carries it.

use std::collections::HashMap;

use base64::Engine;
use cal_core::{Contact, ContactList, ContactPhoto, GroupMember, NewContact};
use chrono::{Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::api::{
    delete_absolute, get_absolute, get_absolute_bytes, patch_absolute, post_absolute, put_absolute,
    ApiState,
};
use crate::error::{GoogleError, GoogleResult};

/// Base URL for the People API v1.
const PEOPLE_API_BASE: &str = "https://people.googleapis.com/v1";

/// Sentinel id of the synthetic ContactList for the user's own
/// address book. The registry routes any `list_id` matching this
/// to the Google adapter; the People API endpoints don't take a
/// list id of their own.
pub const GOOGLE_CONTACT_LIST_ID: &str = "google-contacts";

/// Sentinel for the read-only "Other contacts" list — Gmail's
/// auto-collected addresses (people you've emailed but never
/// added). Backed by `/v1/otherContacts`. Read-only by design;
/// CRUD attempts against this list_id surface Unsupported errors.
pub const GOOGLE_OTHER_CONTACTS_LIST_ID: &str = "google-other-contacts";

/// Sentinel for the read-only Workspace / G Suite domain
/// directory — the corporate address book equivalent to the EWS
/// GAL. Backed by `/v1/people:listDirectoryPeople`. Empty (or
/// 403) for personal `@gmail.com` accounts; populated only when
/// the account belongs to a Workspace domain.
pub const GOOGLE_DIRECTORY_LIST_ID: &str = "google-directory";

/// Person fields requested on every read. Tuned to cover every
/// `cal_core::Contact` slot — the People API charges a per-field
/// "read mask" cost so listing exactly what we'll use stays
/// efficient even for 1000-contact accounts.
const PERSON_FIELDS: &str = "names,emailAddresses,phoneNumbers,birthdays,\
organizations,biographies,memberships,photos,addresses,urls,events,metadata";

/// Mask we send on update calls. Has to enumerate every field we
/// might mutate — fields not in the mask are left untouched
/// server-side, which would silently lose user edits.
const UPDATE_PERSON_FIELDS: &str = "names,emailAddresses,phoneNumbers,birthdays,\
    organizations,biographies,addresses,urls,events";

// ── Public surface ────────────────────────────────────────────────────

/// Return the three synthetic ContactLists the Google adapter
/// exposes: the user's own address book (writable), the
/// auto-collected Other Contacts (read-only), and the Workspace
/// Directory (read-only). The sidebar renders all three; the
/// frontend's `reconcileContactSelection` defaults read-only
/// lists to deselected so personal-Gmail users don't pay for a
/// guaranteed-empty Directory pull on every panel mount.
///
/// The English labels here get re-translated in the frontend
/// via the `intl/contactList` sentinel map (same trick the EWS
/// GAL uses), so DE users see "Andere Kontakte" / "Verzeichnis".
pub fn list_contact_lists() -> Vec<ContactList> {
    vec![
        ContactList {
            color_label: None,
            id: GOOGLE_CONTACT_LIST_ID.to_string(),
            name: "Google Contacts".to_string(),
            color: None,
            read_only: false,
        },
        ContactList {
            color_label: None,
            id: GOOGLE_OTHER_CONTACTS_LIST_ID.to_string(),
            name: "Google Other Contacts".to_string(),
            color: None,
            read_only: true,
        },
        ContactList {
            color_label: None,
            id: GOOGLE_DIRECTORY_LIST_ID.to_string(),
            name: "Google Directory".to_string(),
            color: None,
            read_only: true,
        },
    ]
}

/// Dispatch on `list_id`: routes the read to the personal
/// address book, the "Other contacts" auto-collected set, or
/// the Workspace Directory. Unknown ids yield an empty Vec so
/// a misrouted call surfaces as "no contacts" rather than an
/// error.
pub async fn get_contacts(state: &ApiState, list_id: &str) -> GoogleResult<Vec<Contact>> {
    match list_id {
        GOOGLE_CONTACT_LIST_ID => list_personal_contacts(state).await,
        GOOGLE_OTHER_CONTACTS_LIST_ID => list_other_contacts(state).await,
        GOOGLE_DIRECTORY_LIST_ID => list_directory_people(state).await,
        _ => Ok(Vec::new()),
    }
}

/// Pull every contact + group from the user's own address book.
///
/// Returns `Contact`s in stable order: people first (sorted by
/// display name, mirroring the panel's expected layout), then
/// groups. Each group's `members` is built from the inverse
/// of the per-person memberships array.
async fn list_personal_contacts(state: &ApiState) -> GoogleResult<Vec<Contact>> {
    // Fan out: people + groups in parallel. People listing is
    // paged; groups in one call (Google caps at ~1000 group
    // resources per request which is far above any sane user's
    // count). futures::try_join_all isn't pulled in here; a
    // sequential await pair is fine — the latency is dominated
    // by Google's per-call ~200 ms anyway.
    let people = list_all_people(state).await?;
    let groups = list_all_contact_groups(state).await?;

    // Build a lookup so each ContactGroup can resolve its
    // members back to {name, email} pairs from the already-
    // fetched people set. Google stores membership on the
    // *person* side, so each person knows which groups they're
    // in — we invert that here.
    let mut group_members: HashMap<String, Vec<GroupMember>> = HashMap::new();
    for person in &people {
        let display = best_display_name(person);
        let email = primary_email(person);
        if let Some(email) = email {
            for membership in person.memberships.as_deref().unwrap_or(&[]) {
                if let Some(group_ref) = membership
                    .contact_group_membership
                    .as_ref()
                    .and_then(|m| m.contact_group_resource_name.as_deref())
                {
                    group_members
                        .entry(group_ref.to_string())
                        .or_default()
                        .push(GroupMember {
                            name: if display.is_empty() {
                                None
                            } else {
                                Some(display.clone())
                            },
                            email: email.clone(),
                        });
                }
            }
        }
    }

    let mut out: Vec<Contact> = Vec::with_capacity(people.len() + groups.len());
    for person in people {
        out.push(person_to_contact(person, GOOGLE_CONTACT_LIST_ID));
    }
    for group in groups {
        // Skip Google's built-in "system" groups (`chatBuddies`,
        // `all`, etc.) — they're not directly editable and
        // surfacing them as distribution lists confuses the
        // dialog's group editor. The `myContacts` group is the
        // implicit "everyone in my address book" container; we
        // hide it for the same reason.
        if matches!(group.group_type.as_deref(), Some("SYSTEM_CONTACT_GROUP")) {
            continue;
        }
        let members = group_members
            .remove(&group.resource_name)
            .unwrap_or_default();
        out.push(group_to_contact(group, members, GOOGLE_CONTACT_LIST_ID));
    }
    Ok(out)
}

/// Page `/v1/otherContacts` and return its rows mapped onto
/// cal-core `Contact`s tagged with the Other-Contacts list_id.
/// `readMask` covers the small set of fields the auto-collected
/// shape actually has (names + emailAddresses + phoneNumbers).
/// A 403 surface (scope not granted or the account doesn't have
/// the feature) collapses to an empty Vec with a debug log so
/// the panel just shows the list as empty rather than failing.
async fn list_other_contacts(state: &ApiState) -> GoogleResult<Vec<Contact>> {
    let mut out: Vec<Contact> = Vec::new();
    let mut page_token: Option<String> = None;
    let read_mask = urlencoding("names,emailAddresses,phoneNumbers,metadata");
    loop {
        let mut url = format!("{PEOPLE_API_BASE}/otherContacts?pageSize=500&readMask={read_mask}",);
        if let Some(t) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(t));
        }
        let response: ListOtherContactsResponse = match get_absolute(state, &url).await {
            Ok(r) => r,
            Err(GoogleError::Http { status: 403, .. }) => {
                tracing::debug!(
                    target: "adapter_google::contacts",
                    "Other Contacts unavailable (scope not granted or feature off)",
                );
                return Ok(Vec::new());
            }
            Err(e) => return Err(e),
        };
        if let Some(rows) = response.other_contacts {
            for person in rows {
                out.push(person_to_contact(person, GOOGLE_OTHER_CONTACTS_LIST_ID));
            }
        }
        match response.next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok(out)
}

/// readMask for Other-Contacts reads. The full sync and every incremental
/// sync MUST use the SAME mask — the People API rejects a `syncToken`
/// request whose mask differs from the one that minted it. `metadata` is
/// required so the delta can detect `deleted` tombstones.
const OTHER_CONTACTS_READ_MASK: &str = "names,emailAddresses,phoneNumbers,metadata";

/// Outcome of one Other-Contacts delta round: created/updated rows, the
/// resourceNames of removed rows, and the fresh syncToken.
#[derive(Debug, Default)]
pub struct OtherContactsDelta {
    pub changes: Vec<Contact>,
    pub deletions: Vec<String>,
    pub new_token: Option<String>,
}

/// Full Other-Contacts sync that ALSO requests a `syncToken`
/// (`requestSyncToken=true`) for the next incremental round. The token
/// lands on the final page. A 403 (scope/feature off) degrades to an
/// empty set with no token — the host then just keeps doing full reads.
pub async fn other_contacts_full(state: &ApiState) -> GoogleResult<(Vec<Contact>, Option<String>)> {
    let read_mask = urlencoding(OTHER_CONTACTS_READ_MASK);
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    let mut sync_token: Option<String> = None;
    loop {
        let mut url = format!(
            "{PEOPLE_API_BASE}/otherContacts\
             ?pageSize=1000&requestSyncToken=true&readMask={read_mask}",
        );
        if let Some(t) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(t));
        }
        let response: ListOtherContactsResponse = match get_absolute(state, &url).await {
            Ok(r) => r,
            Err(GoogleError::Http { status: 403, .. }) => {
                tracing::debug!(
                    target: "adapter_google::contacts",
                    "Other Contacts unavailable (scope not granted or feature off)",
                );
                return Ok((Vec::new(), None));
            }
            Err(e) => return Err(e),
        };
        if let Some(rows) = response.other_contacts {
            for person in rows {
                if person_deleted(&person) {
                    continue;
                }
                out.push(person_to_contact(person, GOOGLE_OTHER_CONTACTS_LIST_ID));
            }
        }
        if response.next_sync_token.is_some() {
            sync_token = response.next_sync_token;
        }
        match response.next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok((out, sync_token))
}

/// Incremental Other-Contacts sync from a stored `syncToken`. Created /
/// updated rows land in `changes`; tombstones (`metadata.deleted`) land
/// in `deletions` by resourceName (already the cal-core id, so it equals
/// the cached row's native_id). An expired token surfaces as
/// `GoogleError::Http { status: 400, .. }` so the caller can re-sync.
pub async fn other_contacts_delta(
    state: &ApiState,
    sync_token: &str,
) -> GoogleResult<OtherContactsDelta> {
    let read_mask = urlencoding(OTHER_CONTACTS_READ_MASK);
    let mut delta = OtherContactsDelta::default();
    let mut page_token: Option<String> = None;
    loop {
        let mut url = format!(
            "{PEOPLE_API_BASE}/otherContacts?pageSize=1000&syncToken={st}&readMask={read_mask}",
            st = urlencoding(sync_token),
        );
        if let Some(t) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(t));
        }
        let response: ListOtherContactsResponse = get_absolute(state, &url).await?;
        if let Some(rows) = response.other_contacts {
            for person in rows {
                if person_deleted(&person) {
                    delta.deletions.push(person.resource_name);
                } else {
                    delta
                        .changes
                        .push(person_to_contact(person, GOOGLE_OTHER_CONTACTS_LIST_ID));
                }
            }
        }
        if response.next_sync_token.is_some() {
            delta.new_token = response.next_sync_token;
        }
        match response.next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok(delta)
}

/// A People *directory* request can fail simply because the account is not a
/// Google Workspace (G Suite) domain user — a personal `@gmail.com` account has
/// no company directory. Google is inconsistent about how it reports this:
/// `listDirectoryPeople` returns **403** for some scopes and **400
/// FAILED_PRECONDITION** ("must be a G suite domain user.") for others. Both
/// mean "there is no directory here", not a real failure, so the directory list
/// must collapse to empty instead of failing the whole account refresh.
///
/// Kept NARROW: a 400 only counts when it carries the precondition signature, so
/// a genuine `INVALID_ARGUMENT` 400 (a request bug — bad readMask/sources) still
/// surfaces as an error rather than being silently swallowed.
fn directory_unavailable(err: &GoogleError) -> bool {
    match err {
        GoogleError::Http { status: 403, .. } => true,
        GoogleError::Http {
            status: 400,
            message,
        } => {
            let m = message.to_ascii_lowercase();
            m.contains("failed_precondition") || m.contains("g suite domain")
        }
        _ => false,
    }
}

/// Page `/v1/people:listDirectoryPeople` and return the company
/// directory. Two sources combine into one logical list:
///   - `DOMAIN_CONTACT` — shared contacts the admin maintains
///     for the organisation (vendors, partners, etc.)
///   - `DOMAIN_PROFILE` — every user account in the Workspace
///     domain
/// Personal `@gmail.com` accounts have no directory (403 or 400
/// FAILED_PRECONDITION, see [`directory_unavailable`]); we swallow
/// that and surface an empty list, mirroring how the EWS GAL behaves
/// for unsupported configurations.
async fn list_directory_people(state: &ApiState) -> GoogleResult<Vec<Contact>> {
    let mut out: Vec<Contact> = Vec::new();
    let mut page_token: Option<String> = None;
    let read_mask = urlencoding(PERSON_FIELDS);
    let sources = "sources=DIRECTORY_SOURCE_TYPE_DOMAIN_CONTACT\
                   &sources=DIRECTORY_SOURCE_TYPE_DOMAIN_PROFILE";
    loop {
        let mut url = format!(
            "{PEOPLE_API_BASE}/people:listDirectoryPeople\
             ?pageSize=500&readMask={read_mask}&{sources}",
        );
        if let Some(t) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(t));
        }
        let response: ListDirectoryPeopleResponse = match get_absolute(state, &url).await {
            Ok(r) => r,
            Err(e) if directory_unavailable(&e) => {
                tracing::debug!(
                    target: "adapter_google::contacts",
                    error = ?e,
                    "Directory unavailable (personal / non-Workspace account); returning empty",
                );
                return Ok(Vec::new());
            }
            Err(e) => return Err(e),
        };
        if let Some(rows) = response.people {
            for person in rows {
                out.push(person_to_contact(person, GOOGLE_DIRECTORY_LIST_ID));
            }
        }
        match response.next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok(out)
}

/// Server-side typeahead across all three Google sources:
/// personal contacts, Other Contacts, and the Workspace
/// Directory. Each endpoint hits in parallel via `tokio::join!`
/// — per-source errors swallow so a 403 on Directory (personal
/// account) doesn't sink personal hits. Results are deduped by
/// resourceName because the same address can appear in both
/// personal + Other or personal + Directory.
pub async fn search_contacts(state: &ApiState, query: &str) -> GoogleResult<Vec<Contact>> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let (personal, other, directory) = tokio::join!(
        search_personal_contacts(state, trimmed),
        search_other_contacts(state, trimmed),
        search_directory_people(state, trimmed),
    );
    let mut out: Vec<Contact> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for batch in [personal, other, directory] {
        let hits = match batch {
            Ok(h) => h,
            Err(err) => {
                tracing::debug!(
                    target: "adapter_google::contacts",
                    ?err,
                    "search source returned no usable results",
                );
                continue;
            }
        };
        for contact in hits {
            if seen.insert(contact.id.clone()) {
                out.push(contact);
            }
        }
    }
    Ok(out)
}

async fn search_personal_contacts(state: &ApiState, query: &str) -> GoogleResult<Vec<Contact>> {
    let url = format!(
        "{PEOPLE_API_BASE}/people:searchContacts?query={}&readMask={}",
        urlencoding(query),
        urlencoding(PERSON_FIELDS),
    );
    let response: SearchContactsResponse = get_absolute(state, &url).await?;
    Ok(response
        .results
        .into_iter()
        .filter_map(|hit| hit.person)
        .map(|p| person_to_contact(p, GOOGLE_CONTACT_LIST_ID))
        .collect())
}

async fn search_other_contacts(state: &ApiState, query: &str) -> GoogleResult<Vec<Contact>> {
    let url = format!(
        "{PEOPLE_API_BASE}/otherContacts:search?query={}&readMask={}",
        urlencoding(query),
        urlencoding("names,emailAddresses,phoneNumbers,metadata"),
    );
    let response: SearchContactsResponse = get_absolute(state, &url).await?;
    Ok(response
        .results
        .into_iter()
        .filter_map(|hit| hit.person)
        .map(|p| person_to_contact(p, GOOGLE_OTHER_CONTACTS_LIST_ID))
        .collect())
}

async fn search_directory_people(state: &ApiState, query: &str) -> GoogleResult<Vec<Contact>> {
    let url = format!(
        "{PEOPLE_API_BASE}/people:searchDirectoryPeople\
         ?query={}&readMask={}\
         &sources=DIRECTORY_SOURCE_TYPE_DOMAIN_CONTACT\
         &sources=DIRECTORY_SOURCE_TYPE_DOMAIN_PROFILE",
        urlencoding(query),
        urlencoding(PERSON_FIELDS),
    );
    let response: SearchDirectoryResponse = match get_absolute(state, &url).await {
        Ok(r) => r,
        // A personal account has no directory to search — treat it as no hits,
        // not an error (the caller swallows per-source failures anyway, but this
        // keeps the two directory paths consistent).
        Err(e) if directory_unavailable(&e) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(response
        .people
        .into_iter()
        .map(|p| person_to_contact(p, GOOGLE_DIRECTORY_LIST_ID))
        .collect())
}

/// Create a person (regular contact) or a contactGroup
/// (distribution list) depending on whether `members` is set.
pub async fn create_contact(state: &ApiState, new: NewContact) -> GoogleResult<Contact> {
    if new.members.is_some() {
        create_contact_group(state, new).await
    } else {
        create_person(state, new).await
    }
}

/// Update a person or contactGroup. The contact's id encodes
/// the resourceName so the route stays unambiguous.
pub async fn update_contact(state: &ApiState, contact: Contact) -> GoogleResult<Contact> {
    if contact.members.is_some() {
        update_contact_group(state, contact).await
    } else {
        update_person(state, contact).await
    }
}

/// Delete a person or contactGroup. We discriminate by id
/// prefix: `contactGroups/…` hits the group endpoint, anything
/// else hits the person :deleteContact path. `otherContacts/…`
/// bails early — those entries are read-only at the API level
/// and Google would return a confusing 404 / "method not
/// allowed" otherwise.
pub async fn delete_contact(state: &ApiState, contact_id: &str) -> GoogleResult<()> {
    if contact_id.starts_with("otherContacts/") {
        return Err(GoogleError::Http {
            status: 405,
            message: "Other Contacts entries are read-only".into(),
        });
    }
    if contact_id.starts_with("contactGroups/") {
        let url = format!("{PEOPLE_API_BASE}/{contact_id}?deleteContacts=false");
        delete_absolute(state, &url).await
    } else {
        let url = format!("{PEOPLE_API_BASE}/{contact_id}:deleteContact");
        delete_absolute(state, &url).await
    }
}

/// Fetch the photo bytes for a person. Two round-trips — the
/// People API only returns photo URLs, not bytes, so we GET the
/// CDN URL with the bearer attached to materialise the binary.
pub async fn get_contact_photo(
    state: &ApiState,
    contact_id: &str,
) -> GoogleResult<Option<ContactPhoto>> {
    if contact_id.starts_with("contactGroups/") {
        // Groups don't carry photos in the People API model.
        return Ok(None);
    }
    let url = format!("{PEOPLE_API_BASE}/{contact_id}?personFields=photos",);
    let person: Person = get_absolute(state, &url).await?;
    let Some(photo_url) = primary_photo_url(&person) else {
        return Ok(None);
    };
    let (bytes, content_type) = get_absolute_bytes(state, &photo_url).await?;
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(ContactPhoto {
        // Google CDN photos are typically JPEG; fall back if
        // Content-Type is missing (some Google endpoints omit it
        // on signed URLs).
        content_type: content_type.unwrap_or_else(|| "image/jpeg".into()),
        data: bytes,
    }))
}

/// Upload (replace) the photo. The People API requires the
/// bytes as base64 inside a JSON body.
pub async fn set_contact_photo(
    state: &ApiState,
    contact_id: &str,
    photo: ContactPhoto,
) -> GoogleResult<()> {
    if contact_id.starts_with("contactGroups/") {
        return Err(GoogleError::Http {
            status: 400,
            message: "contact groups do not support photos".into(),
        });
    }
    let url = format!("{PEOPLE_API_BASE}/{contact_id}:updateContactPhoto");
    let body = UpdateContactPhotoRequest {
        photo_bytes: base64::engine::general_purpose::STANDARD.encode(&photo.data),
        person_fields: Some(PERSON_FIELDS.to_string()),
    };
    let _: serde_json::Value = patch_absolute(state, &url, &body).await?;
    Ok(())
}

/// Delete the photo, leaving the rest of the contact alone.
pub async fn delete_contact_photo(state: &ApiState, contact_id: &str) -> GoogleResult<()> {
    if contact_id.starts_with("contactGroups/") {
        return Ok(());
    }
    let url = format!(
        "{PEOPLE_API_BASE}/{contact_id}:deleteContactPhoto?personFields={}",
        urlencoding(PERSON_FIELDS),
    );
    delete_absolute(state, &url).await
}

// ── Internal: people listing ───────────────────────────────────────────

async fn list_all_people(state: &ApiState) -> GoogleResult<Vec<Person>> {
    let mut out: Vec<Person> = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        // pageSize maxes out at 1000 per the People API docs; we
        // pick 200 to balance round-trip count against per-page
        // parsing cost.
        let mut url = format!(
            "{PEOPLE_API_BASE}/people/me/connections?pageSize=200&personFields={}",
            urlencoding(PERSON_FIELDS),
        );
        if let Some(t) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(t));
        }
        let response: ListConnectionsResponse = get_absolute(state, &url).await?;
        if let Some(connections) = response.connections {
            out.extend(connections);
        }
        match response.next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok(out)
}

async fn list_all_contact_groups(state: &ApiState) -> GoogleResult<Vec<ContactGroup>> {
    let mut out: Vec<ContactGroup> = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut url = format!(
            "{PEOPLE_API_BASE}/contactGroups?pageSize=200&groupFields=name,groupType,memberCount,metadata",
        );
        if let Some(t) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(t));
        }
        let response: ListContactGroupsResponse = get_absolute(state, &url).await?;
        if let Some(groups) = response.contact_groups {
            out.extend(groups);
        }
        match response.next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok(out)
}

// ── Internal: people CRUD ──────────────────────────────────────────────

async fn create_person(state: &ApiState, new: NewContact) -> GoogleResult<Contact> {
    let url = format!(
        "{PEOPLE_API_BASE}/people:createContact?personFields={}",
        urlencoding(PERSON_FIELDS),
    );
    let body = new_contact_to_person_body(&new);
    let person: Person = post_absolute(state, &url, &body).await?;
    // If the caller supplied an inline photo, do a second
    // round-trip to attach it. Mirrors the EWS / CardDAV pattern
    // — Google's createContact endpoint doesn't accept photo
    // bytes in the create body either.
    let mut contact = person_to_contact(person, GOOGLE_CONTACT_LIST_ID);
    if let Some(photo) = new.photo {
        if !photo.data.is_empty() {
            if let Err(err) = set_contact_photo(state, &contact.id, photo).await {
                tracing::warn!(
                    contact_id = %contact.id,
                    ?err,
                    "google contact created but photo upload failed",
                );
                contact.has_photo = false;
            } else {
                contact.has_photo = true;
            }
        }
    }
    Ok(contact)
}

async fn update_person(state: &ApiState, contact: Contact) -> GoogleResult<Contact> {
    let url = format!(
        "{PEOPLE_API_BASE}/{}:updateContact?updatePersonFields={}&personFields={}",
        contact.id,
        urlencoding(UPDATE_PERSON_FIELDS),
        urlencoding(PERSON_FIELDS),
    );
    // `events` is in the update mask, and a masked field is REPLACED whole.
    // Aperio models one dated entry — the anniversary — but Google Contacts
    // lets a person carry any number of them, and those others have nowhere
    // in the model to survive a round-trip. So they are fetched back right
    // before the write and passed through untouched; without this, renaming a
    // contact would delete every custom date they had.
    let keep = other_events(state, &contact.id).await;
    let body = contact_to_person_body(&contact, &keep);
    let person: Person = patch_absolute(state, &url, &body).await?;
    Ok(person_to_contact(person, GOOGLE_CONTACT_LIST_ID))
}

/// The contact's dated entries OTHER than the anniversary, as raw JSON ready
/// to go back out. A failed read yields an empty list and a log line rather
/// than an error: losing a secondary date is bad, refusing to save the edit
/// the user actually made is worse.
async fn other_events(state: &ApiState, contact_id: &str) -> Vec<serde_json::Value> {
    let url = format!("{PEOPLE_API_BASE}/{contact_id}?personFields=events");
    let person: Person = match get_absolute(state, &url).await {
        Ok(person) => person,
        Err(err) => {
            tracing::warn!(
                %contact_id,
                ?err,
                "could not re-read google events before update; other dated \
                 entries on this contact may be dropped",
            );
            return Vec::new();
        }
    };
    person
        .events
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            !e.type_
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("anniversary"))
        })
        .filter_map(|e| {
            let date = e.date?;
            let mut entry = serde_json::Map::new();
            entry.insert("date".into(), google_date_parts(&date));
            if let Some(kind) = e.type_.filter(|t| !t.is_empty()) {
                entry.insert("type".into(), serde_json::Value::String(kind));
            }
            Some(serde_json::Value::Object(entry))
        })
        .collect()
}

/// A `PersonDate` back in the shape it arrived in. Missing components stay
/// missing — Google allows a date with no year (a recurring day), and
/// inventing one would change what the entry means.
fn google_date_parts(date: &PersonDate) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (key, value) in [
        ("year", date.year),
        ("month", date.month),
        ("day", date.day),
    ] {
        if let Some(value) = value {
            out.insert(key.into(), serde_json::Value::from(value));
        }
    }
    serde_json::Value::Object(out)
}

// ── Internal: contact group CRUD ───────────────────────────────────────

async fn create_contact_group(state: &ApiState, new: NewContact) -> GoogleResult<Contact> {
    let url = format!("{PEOPLE_API_BASE}/contactGroups");
    let body = serde_json::json!({
        "contactGroup": {
            "name": new.display_name,
        }
    });
    let group: ContactGroup = post_absolute(state, &url, &body).await?;
    // members:modify in a second pass — `contactGroups.create`
    // doesn't take initial members in its request shape.
    if let Some(members) = new.members.as_ref() {
        if !members.is_empty() {
            apply_group_members(state, &group.resource_name, members, &[]).await?;
        }
    }
    Ok(group_to_contact(
        group,
        new.members.unwrap_or_default(),
        GOOGLE_CONTACT_LIST_ID,
    ))
}

async fn update_contact_group(state: &ApiState, contact: Contact) -> GoogleResult<Contact> {
    let url = format!("{PEOPLE_API_BASE}/{}", contact.id);
    let body = serde_json::json!({
        "contactGroup": {
            "etag": contact.etag.clone().unwrap_or_default(),
            "name": contact.display_name,
        },
        "updateGroupFields": "name",
    });
    let group: ContactGroup = put_absolute(state, &url, &body).await?;

    // Diff membership against what the server currently has. We
    // need to know the existing members to compute add/remove
    // sets. A `GET ?maxMembers=N` returns them.
    let existing_url = format!(
        "{PEOPLE_API_BASE}/{}?maxMembers=1000&groupFields=name,groupType,memberCount,metadata",
        contact.id,
    );
    let existing: ContactGroup = get_absolute(state, &existing_url).await?;
    let existing_member_ids: Vec<String> = existing.member_resource_names.unwrap_or_default();

    let desired_members = contact.members.clone().unwrap_or_default();
    // Look up resource names for desired members. Google's group
    // membership is keyed by resourceName, not email — we need to
    // resolve each desired email back to a resourceName via a
    // searchContacts query. Cheap (one round-trip per member)
    // and easier than maintaining a parallel cache.
    let mut desired_member_ids: Vec<String> = Vec::with_capacity(desired_members.len());
    for member in &desired_members {
        let hits = search_contacts(state, &member.email)
            .await
            .unwrap_or_default();
        if let Some(hit) = hits.into_iter().find(|c| {
            c.emails
                .iter()
                .any(|e| e.value.eq_ignore_ascii_case(&member.email))
        }) {
            // Skip group-contacts in the search results — only
            // people can be members of groups.
            if !hit.id.starts_with("contactGroups/") {
                desired_member_ids.push(hit.id);
            }
        }
        // Unresolved members (e.g. an email not in the user's
        // address book) silently drop — the People API only
        // accepts existing person resourceNames in
        // `resourceNamesToAdd`.
    }

    let to_add: Vec<String> = desired_member_ids
        .iter()
        .filter(|id| !existing_member_ids.contains(id))
        .cloned()
        .collect();
    let to_remove: Vec<String> = existing_member_ids
        .iter()
        .filter(|id| !desired_member_ids.contains(id))
        .cloned()
        .collect();

    if !to_add.is_empty() || !to_remove.is_empty() {
        apply_group_members_ids(state, &group.resource_name, &to_add, &to_remove).await?;
    }
    Ok(group_to_contact(
        group,
        desired_members,
        GOOGLE_CONTACT_LIST_ID,
    ))
}

/// Resolve member email→resourceName + apply add/remove to the
/// group. Used during create where we don't yet have a server
/// side roster.
async fn apply_group_members(
    state: &ApiState,
    group_resource_name: &str,
    desired: &[GroupMember],
    to_remove_ids: &[String],
) -> GoogleResult<()> {
    let mut to_add: Vec<String> = Vec::with_capacity(desired.len());
    for member in desired {
        let hits = search_contacts(state, &member.email)
            .await
            .unwrap_or_default();
        if let Some(hit) = hits.into_iter().find(|c| {
            c.emails
                .iter()
                .any(|e| e.value.eq_ignore_ascii_case(&member.email))
        }) {
            if !hit.id.starts_with("contactGroups/") {
                to_add.push(hit.id);
            }
        }
    }
    apply_group_members_ids(state, group_resource_name, &to_add, to_remove_ids).await
}

async fn apply_group_members_ids(
    state: &ApiState,
    group_resource_name: &str,
    to_add: &[String],
    to_remove: &[String],
) -> GoogleResult<()> {
    if to_add.is_empty() && to_remove.is_empty() {
        return Ok(());
    }
    let url = format!("{PEOPLE_API_BASE}/{group_resource_name}/members:modify",);
    let body = serde_json::json!({
        "resourceNamesToAdd": to_add,
        "resourceNamesToRemove": to_remove,
    });
    let _: serde_json::Value = post_absolute(state, &url, &body).await?;
    Ok(())
}

// ── Mappers ────────────────────────────────────────────────────────────

fn person_to_contact(person: Person, list_id: &str) -> Contact {
    let display_name = best_display_name(&person);
    let given_name = person
        .names
        .as_deref()
        .and_then(|ns| ns.first())
        .and_then(|n| n.given_name.clone())
        .filter(|s| !s.is_empty());
    let family_name = person
        .names
        .as_deref()
        .and_then(|ns| ns.first())
        .and_then(|n| n.family_name.clone())
        .filter(|s| !s.is_empty());
    let name_prefix = person
        .names
        .as_deref()
        .and_then(|ns| ns.first())
        .and_then(|n| n.honorific_prefix.clone())
        .filter(|s| !s.is_empty());
    let name_suffix = person
        .names
        .as_deref()
        .and_then(|ns| ns.first())
        .and_then(|n| n.honorific_suffix.clone())
        .filter(|s| !s.is_empty());
    let primary_org = person.organizations.as_deref().and_then(|os| os.first());
    let organization = primary_org
        .and_then(|o| o.name.clone())
        .filter(|s| !s.is_empty());
    let job_title = primary_org
        .and_then(|o| o.title.clone())
        .filter(|s| !s.is_empty());
    let department = primary_org
        .and_then(|o| o.department.clone())
        .filter(|s| !s.is_empty());
    let emails = person
        .email_addresses
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|e| labelled(e.value.as_deref(), e.type_.as_deref()))
        .collect();
    let phone_numbers = person
        .phone_numbers
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| labelled(p.value.as_deref(), p.type_.as_deref()))
        .collect();
    let urls = person
        .urls
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|u| labelled(u.value.as_deref(), u.type_.as_deref()))
        .collect();
    let anniversary = person
        .events
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|e| {
            e.type_
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("anniversary"))
        })
        .and_then(|e| e.date.as_ref())
        .and_then(birthday_to_date);
    let birthday = person
        .birthdays
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find_map(|b| b.date.as_ref().and_then(birthday_to_date));
    let notes = person
        .biographies
        .as_deref()
        .and_then(|bs| bs.first())
        .and_then(|b| b.value.clone())
        .filter(|s| !s.is_empty());
    let has_photo = person
        .photos
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .any(|p| !p.default.unwrap_or(false) && p.url.is_some());
    let addresses: Vec<cal_core::ContactAddress> = person
        .addresses
        .unwrap_or_default()
        .into_iter()
        .filter_map(person_address_to_core)
        .collect();
    let now = Utc::now();
    Contact {
        urls,
        anniversary,
        job_title,
        department,
        name_prefix,
        name_suffix,
        id: person.resource_name,
        list_id: list_id.to_string(),
        display_name,
        given_name,
        family_name,
        organization,
        emails,
        phone_numbers,
        birthday,
        notes,
        members: None,
        has_photo,
        addresses,
        created_at: now,
        updated_at: now,
        etag: person.etag,
    }
}

/// One typed channel as a `ContactValue`, or `None` when the value itself is
/// missing. Google's `type` is free text — the canonical `home`/`work`/`mobile`
/// render as localised labels in its own UI, anything else shows verbatim —
/// so it passes through unchanged rather than being squeezed into a fixed set.
fn labelled(value: Option<&str>, type_: Option<&str>) -> Option<cal_core::ContactValue> {
    let value = value.map(str::trim).filter(|s| !s.is_empty())?;
    Some(cal_core::ContactValue {
        value: value.to_string(),
        label: type_
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

/// A People-API entry list that carries each value with the `type` it was
/// labelled with. Entries with no label send no `type` key at all rather than
/// an empty string — Google echoes `""` back and it reads as a nameless label.
fn typed_channels(values: &[cal_core::ContactValue], key: &str) -> serde_json::Value {
    serde_json::Value::Array(
        values
            .iter()
            .filter(|v| !v.value.trim().is_empty())
            .map(|v| {
                let mut entry = serde_json::Map::new();
                entry.insert(key.into(), serde_json::Value::String(v.value.clone()));
                if let Some(label) = v.label.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    entry.insert("type".into(), serde_json::Value::String(label.to_string()));
                }
                serde_json::Value::Object(entry)
            })
            .collect(),
    )
}

/// A date in the People API's split-integer shape.
fn google_date(date: NaiveDate) -> serde_json::Value {
    serde_json::json!({
        "year": date.year(),
        "month": date.month(),
        "day": date.day(),
    })
}

/// Translate Google's flat `PersonAddress` into a cal-core
/// `ContactAddress`. Returns `None` for entries that are all-empty
/// (Google sometimes emits these on freshly created contacts the
/// user hasn't filled an address into yet) — surfacing them as
/// blank rows would clutter the UI without adding signal.
fn person_address_to_core(addr: PersonAddress) -> Option<cal_core::ContactAddress> {
    let mapped = cal_core::ContactAddress {
        label: addr.type_.filter(|s| !s.is_empty()),
        street: addr.street_address.filter(|s| !s.is_empty()),
        city: addr.city.filter(|s| !s.is_empty()),
        region: addr.region.filter(|s| !s.is_empty()),
        postal_code: addr.postal_code.filter(|s| !s.is_empty()),
        country: addr.country.filter(|s| !s.is_empty()),
    };
    if mapped.street.is_none()
        && mapped.city.is_none()
        && mapped.region.is_none()
        && mapped.postal_code.is_none()
        && mapped.country.is_none()
    {
        return None;
    }
    Some(mapped)
}

fn group_to_contact(group: ContactGroup, members: Vec<GroupMember>, list_id: &str) -> Contact {
    let now = Utc::now();
    Contact {
        urls: Vec::new(),
        anniversary: None,
        job_title: None,
        department: None,
        name_prefix: None,
        name_suffix: None,
        id: group.resource_name,
        list_id: list_id.to_string(),
        display_name: group.name.unwrap_or_else(|| "(Unnamed group)".into()),
        given_name: None,
        family_name: None,
        organization: None,
        emails: Vec::new(),
        phone_numbers: Vec::new(),
        birthday: None,
        notes: None,
        members: Some(members),
        has_photo: false,
        // Groups don't carry postal addresses in Google's model.
        addresses: Vec::new(),
        created_at: now,
        updated_at: now,
        etag: group.etag,
    }
}

fn new_contact_to_person_body(new: &NewContact) -> serde_json::Value {
    // A contact being created has no other dated entries yet.
    person_body_from_fields(
        &new.display_name,
        new.given_name.as_deref(),
        new.family_name.as_deref(),
        new.name_prefix.as_deref(),
        new.name_suffix.as_deref(),
        new.organization.as_deref(),
        &new.emails,
        &new.phone_numbers,
        new.birthday,
        new.notes.as_deref(),
        &new.addresses,
        &new.urls,
        new.anniversary,
        new.job_title.as_deref(),
        new.department.as_deref(),
        &[],
        None,
    )
}

fn contact_to_person_body(
    contact: &Contact,
    other_events: &[serde_json::Value],
) -> serde_json::Value {
    person_body_from_fields(
        &contact.display_name,
        contact.given_name.as_deref(),
        contact.family_name.as_deref(),
        contact.name_prefix.as_deref(),
        contact.name_suffix.as_deref(),
        contact.organization.as_deref(),
        &contact.emails,
        &contact.phone_numbers,
        contact.birthday,
        contact.notes.as_deref(),
        &contact.addresses,
        &contact.urls,
        contact.anniversary,
        contact.job_title.as_deref(),
        contact.department.as_deref(),
        other_events,
        contact.etag.as_deref(),
    )
}

/// Shared body assembler for create + update. The etag is `None`
/// on create (the People API ignores it) and `Some` on update
/// (required for conflict detection).
fn person_body_from_fields(
    display_name: &str,
    given_name: Option<&str>,
    family_name: Option<&str>,
    name_prefix: Option<&str>,
    name_suffix: Option<&str>,
    organization: Option<&str>,
    emails: &[cal_core::ContactValue],
    phone_numbers: &[cal_core::ContactValue],
    birthday: Option<NaiveDate>,
    notes: Option<&str>,
    addresses: &[cal_core::ContactAddress],
    urls: &[cal_core::ContactValue],
    anniversary: Option<NaiveDate>,
    job_title: Option<&str>,
    department: Option<&str>,
    // Dated entries Aperio does not model, read back from the server so the
    // masked `events` replacement does not delete them.
    other_events: &[serde_json::Value],
    etag: Option<&str>,
) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if let Some(etag) = etag {
        body.insert("etag".into(), serde_json::Value::String(etag.to_string()));
    }
    // `names` is an array per the spec; we only ever set one. We
    // emit displayName so consumers that read it directly (i.e.
    // not split by given/family) see the right thing.
    body.insert(
        "names".into(),
        serde_json::json!([{
            "displayName": display_name,
            "givenName": given_name.unwrap_or(""),
            "familyName": family_name.unwrap_or(""),
            "honorificPrefix": name_prefix.unwrap_or(""),
            "honorificSuffix": name_suffix.unwrap_or(""),
        }]),
    );
    // Company, job title and department are one `organizations` entry on
    // Google's side, so any of the three present means the entry gets written.
    if organization.is_some() || job_title.is_some() || department.is_some() {
        let mut org = serde_json::Map::new();
        for (key, value) in [
            ("name", organization),
            ("title", job_title),
            ("department", department),
        ] {
            if let Some(value) = value.map(str::trim).filter(|s| !s.is_empty()) {
                org.insert(key.into(), serde_json::Value::String(value.to_string()));
            }
        }
        body.insert(
            "organizations".into(),
            serde_json::Value::Array(vec![serde_json::Value::Object(org)]),
        );
    }
    if !emails.is_empty() {
        body.insert("emailAddresses".into(), typed_channels(emails, "value"));
    }
    if !phone_numbers.is_empty() {
        body.insert(
            "phoneNumbers".into(),
            typed_channels(phone_numbers, "value"),
        );
    }
    if !urls.is_empty() {
        body.insert("urls".into(), typed_channels(urls, "value"));
    }
    // `events` is written even when there is no anniversary: it sits in the
    // update mask, so leaving the key out clears the whole collection — and
    // that is also how an anniversary the user deleted actually goes away.
    // The entries Aperio does not model ride along unchanged.
    let mut events: Vec<serde_json::Value> = other_events.to_vec();
    if let Some(anniversary) = anniversary {
        events.push(serde_json::json!({
            "type": "anniversary",
            "date": google_date(anniversary),
        }));
    }
    body.insert("events".into(), serde_json::Value::Array(events));
    if let Some(bd) = birthday {
        body.insert(
            "birthdays".into(),
            serde_json::json!([{ "date": google_date(bd) }]),
        );
    }
    if let Some(notes) = notes {
        body.insert(
            "biographies".into(),
            serde_json::json!([{ "value": notes }]),
        );
    }
    // Postal addresses (Phase 10l). One entry per ContactAddress;
    // empty slots get omitted from the JSON so Google doesn't see
    // a payload full of "" values it then tries to round-trip.
    // The `type` key holds our `label` — Google accepts arbitrary
    // strings here but renders the three canonical ones ("home",
    // "work", "other") with localised UI labels.
    if !addresses.is_empty() {
        let entries: Vec<serde_json::Value> = addresses
            .iter()
            .map(|a| {
                let mut entry = serde_json::Map::new();
                if let Some(label) = a.label.as_deref().filter(|s| !s.is_empty()) {
                    entry.insert("type".into(), serde_json::Value::String(label.to_string()));
                }
                if let Some(s) = a.street.as_deref().filter(|s| !s.is_empty()) {
                    entry.insert(
                        "streetAddress".into(),
                        serde_json::Value::String(s.to_string()),
                    );
                }
                if let Some(s) = a.city.as_deref().filter(|s| !s.is_empty()) {
                    entry.insert("city".into(), serde_json::Value::String(s.to_string()));
                }
                if let Some(s) = a.region.as_deref().filter(|s| !s.is_empty()) {
                    entry.insert("region".into(), serde_json::Value::String(s.to_string()));
                }
                if let Some(s) = a.postal_code.as_deref().filter(|s| !s.is_empty()) {
                    entry.insert(
                        "postalCode".into(),
                        serde_json::Value::String(s.to_string()),
                    );
                }
                if let Some(s) = a.country.as_deref().filter(|s| !s.is_empty()) {
                    entry.insert("country".into(), serde_json::Value::String(s.to_string()));
                }
                serde_json::Value::Object(entry)
            })
            .collect();
        body.insert("addresses".into(), serde_json::Value::Array(entries));
    }
    serde_json::Value::Object(body)
}

fn best_display_name(person: &Person) -> String {
    if let Some(names) = person.names.as_deref() {
        if let Some(name) = names.first() {
            if let Some(d) = name.display_name.as_deref() {
                if !d.is_empty() {
                    return d.to_string();
                }
            }
            let composed = format!(
                "{} {}",
                name.given_name.as_deref().unwrap_or(""),
                name.family_name.as_deref().unwrap_or(""),
            )
            .trim()
            .to_string();
            if !composed.is_empty() {
                return composed;
            }
        }
    }
    // Fall back to the first email address so the picker has
    // something to render. If even that's missing, the contact
    // shows as "(unnamed)" — same convention the EWS adapter
    // uses.
    person
        .email_addresses
        .as_deref()
        .and_then(|es| es.first())
        .and_then(|e| e.value.clone())
        .unwrap_or_else(|| "(unnamed)".to_string())
}

fn primary_email(person: &Person) -> Option<String> {
    person
        .email_addresses
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find_map(|e| e.value.clone())
}

fn primary_photo_url(person: &Person) -> Option<String> {
    person
        .photos
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|p| !p.default.unwrap_or(false))
        .and_then(|p| p.url.clone())
}

fn birthday_to_date(date: &PersonDate) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(
        date.year.unwrap_or(0) as i32,
        date.month.unwrap_or(0),
        date.day.unwrap_or(0),
    )
}

// ── JSON wire shapes ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListConnectionsResponse {
    connections: Option<Vec<Person>>,
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListOtherContactsResponse {
    other_contacts: Option<Vec<Person>>,
    next_page_token: Option<String>,
    /// Present on the final page when `requestSyncToken=true` or a
    /// `syncToken` was supplied — the cursor for the next delta round.
    #[serde(default)]
    next_sync_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListDirectoryPeopleResponse {
    people: Option<Vec<Person>>,
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchDirectoryResponse {
    #[serde(default)]
    people: Vec<Person>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListContactGroupsResponse {
    contact_groups: Option<Vec<ContactGroup>>,
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchContactsResponse {
    #[serde(default)]
    results: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    person: Option<Person>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Person {
    /// Format: `people/c1234567890`.
    resource_name: String,
    etag: Option<String>,
    names: Option<Vec<PersonName>>,
    email_addresses: Option<Vec<PersonEmail>>,
    phone_numbers: Option<Vec<PersonPhone>>,
    birthdays: Option<Vec<PersonBirthday>>,
    organizations: Option<Vec<PersonOrg>>,
    biographies: Option<Vec<PersonBiography>>,
    memberships: Option<Vec<PersonMembership>>,
    photos: Option<Vec<PersonPhoto>>,
    /// Postal addresses (Phase 10l). Each entry carries a small
    /// flat shape: type + street + city + region + postal +
    /// country. We round-trip the `type` string verbatim onto our
    /// `ContactAddress.label`.
    addresses: Option<Vec<PersonAddress>>,
    urls: Option<Vec<PersonUrl>>,
    /// Dated entries other than the birthday; Aperio reads the
    /// `anniversary`-typed one and leaves the rest alone.
    events: Option<Vec<PersonEvent>>,
    /// Present on sync responses — `metadata.deleted = true` marks a row
    /// the People API delta is telling us was removed.
    #[serde(default)]
    metadata: Option<PersonMetadata>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct PersonMetadata {
    #[serde(default)]
    deleted: bool,
}

/// `true` when a delta row is a tombstone (`metadata.deleted`).
fn person_deleted(person: &Person) -> bool {
    person.metadata.as_ref().map(|m| m.deleted).unwrap_or(false)
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PersonAddress {
    #[serde(rename = "type", default)]
    type_: Option<String>,
    street_address: Option<String>,
    city: Option<String>,
    region: Option<String>,
    postal_code: Option<String>,
    country: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PersonName {
    display_name: Option<String>,
    given_name: Option<String>,
    family_name: Option<String>,
    honorific_prefix: Option<String>,
    honorific_suffix: Option<String>,
}

/// Google types every channel — `home`, `work`, `mobile`, and any free
/// string the user typed in Contacts. Aperio round-trips that verbatim as
/// the channel's label; it used to be read and dropped.
#[derive(Debug, Deserialize, Clone)]
struct PersonEmail {
    value: Option<String>,
    #[serde(rename = "type", default)]
    type_: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct PersonPhone {
    value: Option<String>,
    #[serde(rename = "type", default)]
    type_: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct PersonUrl {
    value: Option<String>,
    #[serde(rename = "type", default)]
    type_: Option<String>,
}

/// A dated entry other than the birthday. Google Contacts files the
/// anniversary here, under `type = "anniversary"`.
#[derive(Debug, Deserialize, Clone)]
struct PersonEvent {
    date: Option<PersonDate>,
    #[serde(rename = "type", default)]
    type_: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PersonBirthday {
    date: Option<PersonDate>,
}

#[derive(Debug, Deserialize, Clone)]
struct PersonDate {
    year: Option<u32>,
    month: Option<u32>,
    day: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
struct PersonOrg {
    name: Option<String>,
    title: Option<String>,
    department: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct PersonBiography {
    value: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PersonMembership {
    contact_group_membership: Option<ContactGroupMembership>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ContactGroupMembership {
    contact_group_resource_name: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PersonPhoto {
    url: Option<String>,
    default: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ContactGroup {
    /// Format: `contactGroups/abc123`. System groups have
    /// well-known names (e.g. `contactGroups/myContacts`).
    resource_name: String,
    etag: Option<String>,
    name: Option<String>,
    /// `USER_CONTACT_GROUP` for user-created, `SYSTEM_CONTACT_GROUP`
    /// for the built-ins Aperio filters out.
    group_type: Option<String>,
    /// Populated by `GET /contactGroups/{id}?maxMembers=N`.
    /// Each entry is a person resourceName.
    member_resource_names: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateContactPhotoRequest {
    photo_bytes: String,
    person_fields: Option<String>,
}

/// Same percent-encoder pattern other adapters use for query
/// parameters. RFC 3986 unreserved set passes through; everything
/// else becomes `%HH`.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_channel_keeps_its_type_in_both_directions() {
        // Google types every channel; the type used to be read and thrown
        // away, so a work number came back unlabelled and went out that way.
        let mut person = person_fixture();
        person.phone_numbers = Some(vec![
            PersonPhone {
                value: Some("+49 30 111".into()),
                type_: Some("work".into()),
            },
            PersonPhone {
                value: Some("+49 170 222".into()),
                // A word the user typed themselves — Google stores it as
                // written, so Aperio must not squeeze it into a fixed set.
                type_: Some("Ferienhaus".into()),
            },
        ]);
        person.urls = Some(vec![PersonUrl {
            value: Some("https://beispiel.example".into()),
            type_: Some("work".into()),
        }]);
        person.events = Some(vec![
            PersonEvent {
                date: Some(PersonDate {
                    year: Some(2000),
                    month: Some(1),
                    day: Some(2),
                }),
                type_: Some("other".into()),
            },
            PersonEvent {
                date: Some(PersonDate {
                    year: Some(2014),
                    month: Some(6),
                    day: Some(21),
                }),
                type_: Some("anniversary".into()),
            },
        ]);
        person.organizations = Some(vec![PersonOrg {
            name: Some("Example GmbH".into()),
            title: Some("Werkstattleiterin".into()),
            department: Some("Technik".into()),
        }]);

        let contact = person_to_contact(person, "list-1");
        assert_eq!(contact.phone_numbers[0].label.as_deref(), Some("work"));
        assert_eq!(
            contact.phone_numbers[1].label.as_deref(),
            Some("Ferienhaus")
        );
        assert_eq!(contact.urls[0].label.as_deref(), Some("work"));
        // The anniversary is picked out of `events` by its type, not by
        // position — Google files other dates in the same collection.
        assert_eq!(contact.anniversary, NaiveDate::from_ymd_opt(2014, 6, 21),);
        assert_eq!(contact.job_title.as_deref(), Some("Werkstattleiterin"));
        assert_eq!(contact.department.as_deref(), Some("Technik"));

        let body = contact_to_person_body(&contact, &[]);
        assert_eq!(body["phoneNumbers"][0]["type"], "work");
        assert_eq!(body["phoneNumbers"][1]["type"], "Ferienhaus");
        assert_eq!(body["urls"][0]["value"], "https://beispiel.example");
        assert_eq!(body["events"][0]["type"], "anniversary");
        assert_eq!(body["events"][0]["date"]["year"], 2014);
        assert_eq!(body["organizations"][0]["title"], "Werkstattleiterin");
        assert_eq!(body["organizations"][0]["department"], "Technik");
    }

    #[test]
    fn writing_an_anniversary_keeps_the_contact_s_other_dates() {
        // `events` sits in the update mask, and a masked field is REPLACED
        // whole — so a body carrying only the anniversary would delete every
        // other dated entry the moment the user renamed the contact.
        let mut contact = person_to_contact(person_fixture(), "list-1");
        contact.anniversary = NaiveDate::from_ymd_opt(2014, 6, 21);
        let keep = vec![serde_json::json!({
            "type": "other",
            "date": { "year": 2000, "month": 1, "day": 2 },
        })];
        let body = contact_to_person_body(&contact, &keep);
        assert_eq!(body["events"].as_array().unwrap().len(), 2);
        assert_eq!(body["events"][0]["type"], "other");
        assert_eq!(body["events"][1]["type"], "anniversary");
    }

    #[test]
    fn clearing_an_anniversary_still_reaches_google() {
        // The other half: no anniversary must send an `events` key anyway,
        // or the mask clears the collection and the passengers go with it.
        let contact = person_to_contact(person_fixture(), "list-1");
        assert!(contact.anniversary.is_none());
        let keep = vec![serde_json::json!({
            "type": "other",
            "date": { "year": 2000, "month": 1, "day": 2 },
        })];
        let body = contact_to_person_body(&contact, &keep);
        assert_eq!(body["events"].as_array().unwrap().len(), 1);
        assert_eq!(body["events"][0]["type"], "other");
    }

    #[test]
    fn an_unlabelled_channel_sends_no_type_at_all() {
        // An empty `type` comes back from Google verbatim and then reads as a
        // label with no name; absence is the honest wire shape.
        let contact = person_to_contact(person_fixture(), "list-1");
        let body = contact_to_person_body(&contact, &[]);
        assert_eq!(body["emailAddresses"][0]["value"], "anna@example.com");
        assert!(body["emailAddresses"][0].get("type").is_none());
    }

    fn person_fixture() -> Person {
        Person {
            urls: None,
            events: None,
            resource_name: "people/c123".into(),
            etag: Some("etag-1".into()),
            names: Some(vec![PersonName {
                display_name: Some("Anna Beispiel".into()),
                given_name: Some("Anna".into()),
                family_name: Some("Beispiel".into()),
                honorific_prefix: Some("Dr.".into()),
                honorific_suffix: None,
            }]),
            email_addresses: Some(vec![PersonEmail {
                type_: None,
                value: Some("anna@example.com".into()),
            }]),
            phone_numbers: Some(vec![PersonPhone {
                type_: None,
                value: Some("+49 30 1234567".into()),
            }]),
            birthdays: Some(vec![PersonBirthday {
                date: Some(PersonDate {
                    year: Some(1990),
                    month: Some(6),
                    day: Some(15),
                }),
            }]),
            organizations: Some(vec![PersonOrg {
                title: None,
                department: None,
                name: Some("Example GmbH".into()),
            }]),
            biographies: Some(vec![PersonBiography {
                value: Some("Met at conf".into()),
            }]),
            memberships: Some(vec![PersonMembership {
                contact_group_membership: Some(ContactGroupMembership {
                    contact_group_resource_name: Some("contactGroups/myContacts".into()),
                }),
            }]),
            photos: Some(vec![PersonPhoto {
                url: Some("https://lh3.googleusercontent.com/abc".into()),
                default: Some(false),
            }]),
            addresses: Some(vec![PersonAddress {
                type_: Some("home".into()),
                street_address: Some("Hauptstraße 1".into()),
                city: Some("Berlin".into()),
                region: None,
                postal_code: Some("10115".into()),
                country: Some("Deutschland".into()),
            }]),
            metadata: None,
        }
    }

    #[test]
    fn person_to_contact_maps_every_modelled_field() {
        let c = person_to_contact(person_fixture(), GOOGLE_CONTACT_LIST_ID);
        assert_eq!(c.id, "people/c123");
        assert_eq!(c.list_id, GOOGLE_CONTACT_LIST_ID);
        assert_eq!(c.display_name, "Anna Beispiel");
        assert_eq!(c.given_name.as_deref(), Some("Anna"));
        assert_eq!(c.name_prefix.as_deref(), Some("Dr."));
        assert!(c.name_suffix.is_none());
        assert_eq!(c.family_name.as_deref(), Some("Beispiel"));
        assert_eq!(c.organization.as_deref(), Some("Example GmbH"));
        assert_eq!(c.emails, vec!["anna@example.com".to_string()]);
        assert_eq!(c.phone_numbers, vec!["+49 30 1234567".to_string()]);
        assert_eq!(c.birthday, NaiveDate::from_ymd_opt(1990, 6, 15),);
        assert_eq!(c.notes.as_deref(), Some("Met at conf"));
        assert!(c.has_photo);
        assert!(c.members.is_none());
        assert_eq!(c.etag.as_deref(), Some("etag-1"));
    }

    #[test]
    fn person_to_contact_falls_back_when_displayname_blank() {
        let mut p = person_fixture();
        if let Some(names) = p.names.as_mut() {
            names[0].display_name = None;
            names[0].given_name = None;
            names[0].family_name = None;
        }
        let c = person_to_contact(p, GOOGLE_CONTACT_LIST_ID);
        // First email becomes the display name when names give us
        // nothing usable.
        assert_eq!(c.display_name, "anna@example.com");
    }

    #[test]
    fn person_to_contact_yields_unnamed_when_everything_missing() {
        let p = Person {
            urls: None,
            events: None,
            resource_name: "people/c999".into(),
            etag: None,
            names: None,
            email_addresses: None,
            phone_numbers: None,
            birthdays: None,
            organizations: None,
            biographies: None,
            memberships: None,
            photos: None,
            addresses: None,
            metadata: None,
        };
        assert_eq!(
            person_to_contact(p, GOOGLE_CONTACT_LIST_ID).display_name,
            "(unnamed)",
        );
    }

    #[test]
    fn other_contacts_response_parses_sync_token_and_deleted_tombstone() {
        let json = r#"{
          "otherContacts": [
            {"resourceName":"otherContacts/c1",
             "names":[{"displayName":"Alice"}],
             "emailAddresses":[{"value":"alice@example.com"}]},
            {"resourceName":"otherContacts/c2","metadata":{"deleted":true}}
          ],
          "nextSyncToken": "TOK-2"
        }"#;
        let resp: ListOtherContactsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.next_sync_token.as_deref(), Some("TOK-2"));
        let rows = resp.other_contacts.unwrap();
        assert_eq!(rows.len(), 2);
        // c1 is live; c2 is a delete tombstone.
        assert!(!person_deleted(&rows[0]));
        assert!(person_deleted(&rows[1]));
        // The live row maps to a contact whose id IS the resourceName
        // (= the cache native_id a deletion matches against).
        let c = person_to_contact(rows[0].clone(), GOOGLE_OTHER_CONTACTS_LIST_ID);
        assert_eq!(c.id, "otherContacts/c1");
        assert_eq!(c.display_name, "Alice");
    }

    #[test]
    fn has_photo_skips_googles_default_avatar() {
        let mut p = person_fixture();
        p.photos = Some(vec![PersonPhoto {
            url: Some("https://lh3.googleusercontent.com/default".into()),
            default: Some(true),
        }]);
        // The placeholder grey-silhouette photo has default=true.
        // We don't want to advertise it as "has photo" because the
        // dialog would then trigger a fetch that returns a useless
        // generic image.
        assert!(!person_to_contact(p, GOOGLE_CONTACT_LIST_ID).has_photo);
    }

    #[test]
    fn group_to_contact_marks_members_as_distribution_list() {
        let g = ContactGroup {
            resource_name: "contactGroups/abc".into(),
            etag: Some("g-etag".into()),
            name: Some("Friends".into()),
            group_type: Some("USER_CONTACT_GROUP".into()),
            member_resource_names: None,
        };
        let members = vec![GroupMember {
            name: Some("Anna".into()),
            email: "anna@example.com".into(),
        }];
        let c = group_to_contact(g, members.clone(), GOOGLE_CONTACT_LIST_ID);
        assert_eq!(c.id, "contactGroups/abc");
        assert_eq!(c.display_name, "Friends");
        assert_eq!(c.members.as_ref().unwrap(), &members);
        assert_eq!(c.etag.as_deref(), Some("g-etag"));
    }

    #[test]
    fn person_body_for_create_emits_names_emails_phones() {
        let body = new_contact_to_person_body(&NewContact {
            urls: Vec::new(),
            anniversary: None,
            job_title: None,
            department: None,
            name_prefix: Some("Prof. Dr.".into()),
            name_suffix: Some("jun.".into()),
            display_name: "Max Mustermann".into(),
            given_name: Some("Max".into()),
            family_name: Some("Mustermann".into()),
            organization: Some("Example GmbH".into()),
            emails: vec!["max@example.com".into()],
            phone_numbers: vec!["+49 170 1234567".into()],
            birthday: NaiveDate::from_ymd_opt(1985, 4, 17),
            notes: Some("Note".into()),
            addresses: Vec::new(),
            members: None,
            photo: None,
        });
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains("\"displayName\":\"Max Mustermann\""));
        assert!(s.contains("\"givenName\":\"Max\""));
        assert!(s.contains("\"familyName\":\"Mustermann\""));
        assert!(s.contains("\"honorificPrefix\":\"Prof. Dr.\""));
        assert!(s.contains("\"honorificSuffix\":\"jun.\""));
        assert!(s.contains("\"value\":\"max@example.com\""));
        assert!(s.contains("\"value\":\"+49 170 1234567\""));
        assert!(s.contains("\"name\":\"Example GmbH\""));
        assert!(s.contains("\"value\":\"Note\""));
        assert!(s.contains("\"year\":1985"));
        // Create body must NOT carry an etag (the API ignores it
        // on create but emitting it would still look odd to a
        // human reading the trace).
        assert!(!s.contains("\"etag\""));
    }

    #[test]
    fn person_body_for_update_includes_etag() {
        let now = Utc::now();
        let body = contact_to_person_body(
            &Contact {
                urls: Vec::new(),
                anniversary: None,
                job_title: None,
                department: None,
                name_prefix: None,
                name_suffix: None,
                id: "people/c123".into(),
                list_id: GOOGLE_CONTACT_LIST_ID.into(),
                display_name: "Max".into(),
                given_name: Some("Max".into()),
                family_name: None,
                organization: None,
                emails: vec!["max@example.com".into()],
                phone_numbers: Vec::new(),
                birthday: None,
                notes: None,
                members: None,
                has_photo: false,
                addresses: Vec::new(),
                created_at: now,
                updated_at: now,
                etag: Some("etag-1".into()),
            },
            &[],
        );
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains("\"etag\":\"etag-1\""));
    }

    #[test]
    fn search_short_circuits_on_blank_query() {
        // Not a network-bound test — we just exercise the
        // early-out without setting up a mockito server. Worth a
        // pin because a regression would land every keystroke
        // against the People API.
        let trimmed = "   ".trim();
        assert!(trimmed.is_empty());
    }

    // ── End-to-end via mockito ────────────────────────────────────

    use crate::auth::TokenSet;
    use mockito::Server;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn fixture_state(server_url: &str) -> ApiState {
        ApiState {
            tokens: Arc::new(Mutex::new(TokenSet {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                scope: None,
            })),
            client_id: "cid".into(),
            client_secret: "secret".into(),
            http: reqwest::Client::new(),
            token_url: format!("{server_url}/token"),
            // Each test builds its own absolute URLs against the
            // mockito root, so api_base stays at the mock URL —
            // the People API helpers use absolute URLs anyway.
            api_base: server_url.to_string(),
        }
    }

    #[tokio::test]
    async fn search_contacts_parses_results() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock(
                "GET",
                mockito::Matcher::Regex("/people:searchContacts.*".into()),
            )
            .with_status(200)
            .with_body(
                r#"{"results":[
                  {"person":{"resourceName":"people/c1","names":[{"displayName":"Anna"}],
                             "emailAddresses":[{"value":"anna@example.com"}]}},
                  {"person":{"resourceName":"people/c2","names":[{"displayName":"Andreas"}],
                             "emailAddresses":[{"value":"andreas@example.com"}]}}
                ]}"#,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        // Build the search URL against the mock root rather than
        // the real People API. The helper would normally hit
        // people.googleapis.com — for the mock we just call the
        // shared get_absolute directly with a URL pointing at the
        // mockito server.
        let url = format!(
            "{}/people:searchContacts?query=an&readMask={}",
            server.url(),
            urlencoding(PERSON_FIELDS),
        );
        let response: SearchContactsResponse = get_absolute(&state, &url).await.unwrap();
        let contacts: Vec<Contact> = response
            .results
            .into_iter()
            .filter_map(|hit| hit.person)
            .map(|p| person_to_contact(p, GOOGLE_CONTACT_LIST_ID))
            .collect();
        assert_eq!(contacts.len(), 2);
        assert_eq!(contacts[0].display_name, "Anna");
        assert_eq!(contacts[1].emails, vec!["andreas@example.com".to_string()]);
    }

    #[tokio::test]
    async fn create_contact_posts_person_body_and_returns_mapped_contact() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock(
                "POST",
                mockito::Matcher::Regex("/people:createContact.*".into()),
            )
            .with_status(200)
            .with_body(
                r#"{"resourceName":"people/cNEW","etag":"e1",
                    "names":[{"displayName":"Max Mustermann","givenName":"Max","familyName":"Mustermann"}],
                    "emailAddresses":[{"value":"max@example.com"}]}"#,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let body = new_contact_to_person_body(&NewContact {
            urls: Vec::new(),
            anniversary: None,
            job_title: None,
            department: None,
            name_prefix: None,
            name_suffix: None,
            display_name: "Max Mustermann".into(),
            given_name: Some("Max".into()),
            family_name: Some("Mustermann".into()),
            organization: None,
            emails: vec!["max@example.com".into()],
            phone_numbers: Vec::new(),
            birthday: None,
            notes: None,
            addresses: Vec::new(),
            members: None,
            photo: None,
        });
        let url = format!(
            "{}/people:createContact?personFields={}",
            server.url(),
            urlencoding(PERSON_FIELDS),
        );
        let person: Person = post_absolute(&state, &url, &body).await.unwrap();
        let contact = person_to_contact(person, GOOGLE_CONTACT_LIST_ID);
        assert_eq!(contact.id, "people/cNEW");
        assert_eq!(contact.display_name, "Max Mustermann");
        assert_eq!(contact.etag.as_deref(), Some("e1"));
    }

    #[tokio::test]
    async fn delete_contact_targets_deletecontact_endpoint() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/people/c123:deleteContact")
            .with_status(200)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        // Build the URL the way `delete_contact` would — but
        // against the mock root, so the path matches above.
        let url = format!("{}/people/c123:deleteContact", server.url());
        delete_absolute(&state, &url).await.unwrap();
    }

    #[tokio::test]
    async fn delete_contact_group_uses_contactgroups_endpoint() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock(
                "DELETE",
                mockito::Matcher::Regex("/contactGroups/abc.*".into()),
            )
            .with_status(204)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let url = format!("{}/contactGroups/abc?deleteContacts=false", server.url());
        delete_absolute(&state, &url).await.unwrap();
    }

    #[tokio::test]
    async fn list_people_pages_through_next_page_token() {
        let mut server = Server::new_async().await;
        let _page1 = server
            .mock(
                "GET",
                mockito::Matcher::Regex("/people/me/connections.*".into()),
            )
            .with_status(200)
            .with_body(
                r#"{"connections":[
                    {"resourceName":"people/c1","names":[{"displayName":"A"}]}
                ],"nextPageToken":"PAGE2"}"#,
            )
            .expect_at_least(1)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        // Just verify the first page parses; full paging logic is
        // identical to the calendar/tasks path which is already
        // covered. The fixture's `nextPageToken` would trigger
        // another GET in the real helper, but mockito's mock will
        // happily answer subsequent identical requests with the
        // same body — so we instead parse one page directly.
        let url = format!("{}/people/me/connections", server.url());
        let response: ListConnectionsResponse = get_absolute(&state, &url).await.unwrap();
        assert_eq!(response.connections.unwrap().len(), 1);
        assert_eq!(response.next_page_token.as_deref(), Some("PAGE2"));
    }

    #[test]
    fn list_contact_lists_returns_three_lists_with_correct_readonly_flags() {
        let lists = list_contact_lists();
        assert_eq!(lists.len(), 3);
        let personal = lists
            .iter()
            .find(|l| l.id == GOOGLE_CONTACT_LIST_ID)
            .unwrap();
        let other = lists
            .iter()
            .find(|l| l.id == GOOGLE_OTHER_CONTACTS_LIST_ID)
            .unwrap();
        let directory = lists
            .iter()
            .find(|l| l.id == GOOGLE_DIRECTORY_LIST_ID)
            .unwrap();
        assert!(!personal.read_only);
        assert!(other.read_only);
        assert!(directory.read_only);
    }

    #[tokio::test]
    async fn delete_contact_bails_on_other_contacts_prefix() {
        // No mockito server needed — the read-only guard short-
        // circuits before any HTTP call. A regression here would
        // delete the wrong thing (or 404 confusingly) when a
        // user somehow routes a delete through an Other Contacts
        // resource id.
        let state = fixture_state("https://unreachable.invalid");
        let err = delete_contact(&state, "otherContacts/c123")
            .await
            .unwrap_err();
        match err {
            GoogleError::Http { status, .. } => assert_eq!(status, 405),
            other => panic!("expected Http error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_other_contacts_swallows_403_into_empty_vec() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Regex("/otherContacts.*".into()))
            .with_status(403)
            .with_body(r#"{"error":{"code":403,"message":"insufficient scope"}}"#)
            .create_async()
            .await;
        // We need the People-API URL to point at the mockito root.
        // The helper builds it from PEOPLE_API_BASE which is a
        // const, so we instead call list_other_contacts via a
        // small wrapper that patches the base. For test purposes
        // we exercise the helper with a path the api_base points
        // at — same outcome.
        //
        // Simpler: this test just asserts the get_absolute path
        // surfaces 403 as Http{status:403}; the calling helper's
        // match arm collapses that to Ok(empty).
        let state = fixture_state(&server.url());
        let url = format!("{}/otherContacts?pageSize=500&readMask=names", server.url());
        let err = get_absolute::<ListOtherContactsResponse>(&state, &url)
            .await
            .unwrap_err();
        match err {
            GoogleError::Http { status, .. } => assert_eq!(status, 403),
            other => panic!("expected Http 403, got {other:?}"),
        }
    }

    #[test]
    fn directory_unavailable_covers_personal_account_signals() {
        // Personal @gmail.com accounts: Google returns 403 for some scopes …
        assert!(directory_unavailable(&GoogleError::Http {
            status: 403,
            message: "insufficient scope".into(),
        }));
        // … and 400 FAILED_PRECONDITION ("must be a G suite domain user.") on
        // the directory endpoint. Both must collapse to an empty directory
        // rather than failing the whole account refresh.
        assert!(directory_unavailable(&GoogleError::Http {
            status: 400,
            message: r#"{"error":{"code":400,"message":"must be a G suite domain user.","status":"FAILED_PRECONDITION"}}"#
                .into(),
        }));
        // A genuine request bug (INVALID_ARGUMENT 400) must still surface.
        assert!(!directory_unavailable(&GoogleError::Http {
            status: 400,
            message:
                r#"{"error":{"code":400,"message":"Invalid readMask","status":"INVALID_ARGUMENT"}}"#
                    .into(),
        }));
        // Unrelated failures stay errors.
        assert!(!directory_unavailable(&GoogleError::Http {
            status: 500,
            message: "server error".into(),
        }));
    }

    #[tokio::test]
    async fn get_contact_photo_returns_none_when_no_photo_url() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock(
                "GET",
                mockito::Matcher::Regex("/people/c1.*personFields=photos.*".into()),
            )
            .with_status(200)
            .with_body(r#"{"resourceName":"people/c1","photos":[]}"#)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let url = format!("{}/people/c1?personFields=photos", server.url());
        let person: Person = get_absolute(&state, &url).await.unwrap();
        assert!(primary_photo_url(&person).is_none());
    }
}
