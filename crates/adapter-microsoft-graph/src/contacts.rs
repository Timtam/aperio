//! Microsoft Graph Contacts client — Phase 10i.
//!
//! Wraps the REST endpoints under `/me/contactFolders` and `/me/people`
//! through the same `ApiState` plumbing the Calendar and Tasks modules
//! use, so a 401 on any contact call triggers the shared
//! token-refresh dance rather than carrying bespoke code per surface.
//!
//! ## Surface mapping (cal-core ↔ Graph)
//!
//! Each Outlook **contactFolder** the user owns becomes one
//! Aperio `ContactList`:
//!
//!   - The default "Contacts" folder + any user-created sub-folders
//!     enumerate from `/me/contactFolders`.
//!   - Photos live on each contact item via the `/photo/$value`
//!     navigation property.
//!   - Contact ids are mailbox-wide unique, so updates and deletes
//!     can hit `/me/contacts/{id}` directly without routing through
//!     the owning folder (same pattern Graph events use — see the
//!     `/me/events/{id}` shortcut in `api::update_event`).
//!
//! On top of the user-owned folders we surface **one synthetic
//! read-only list — "Suggested People"** — backed by `/me/people`.
//! That endpoint returns relevance-ranked contacts from Outlook
//! traffic plus Azure-AD profile suggestions, which is the closest
//! "GAL-equivalent" available without pulling in the heavy
//! `Directory.Read.All` scope (most tenants gate that behind admin
//! consent). The Suggested People list is read-only — its rows
//! aren't real Contact resources, so create / update / delete /
//! photo upload all return Unsupported against it.
//!
//! ## Distribution lists
//!
//! Outlook calls them "Contact Groups" but Microsoft never exposed
//! a clean REST shape for them through Graph (the legacy EWS
//! `<t:DistributionList>` shape is *not* round-tripped to /me/contacts).
//! We deliberately don't surface them in Phase 10i — every Graph
//! contact lands with `members = None` and the dialog renders it
//! as a regular person.

use cal_core::{Contact, ContactList, ContactPhoto, NewContact};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::api::ApiState;
use crate::error::{GraphError, GraphResult};

/// Sentinel id of the synthetic read-only "Suggested People" list
/// backed by `/me/people`. Distinct from any real contactFolder id
/// Graph hands out, so the dispatch in `get_contacts` can route on
/// `list_id` without ambiguity.
///
/// The English label here gets re-translated in the frontend via
/// the `intl/contactList` sentinel map (same trick the EWS GAL +
/// Google Other / Directory lists use), so DE users see
/// "Vorgeschlagene Personen".
pub const GRAPH_SUGGESTED_PEOPLE_LIST_ID: &str = "graph-suggested-people";

/// Field selector for `/me/contacts` reads. The default
/// `$select=*` payload includes the full mailbox-wide address book
/// + phone schema — a couple dozen fields the Aperio Contact model
/// has nowhere to put. Pruning here keeps the response small even
/// on accounts with thousands of rows.
///
/// Graph returns ONLY what is selected, so anything missing from this list is
/// permanently absent from the model — `homeAddress` and friends were mapped in
/// `map_contact` but never requested, which left every Outlook contact
/// address-less no matter what the server held.
const CONTACT_SELECT: &str = "id,displayName,givenName,surname,companyName,\
emailAddresses,businessPhones,homePhones,mobilePhone,birthday,personalNotes,\
homeAddress,businessAddress,otherAddress,parentFolderId,\
jobTitle,department,businessHomePage,\
createdDateTime,lastModifiedDateTime";

/// Field selector for `/me/people` reads. Person resources are a
/// different shape — no `givenName` / `surname` split, addresses
/// arrive as `scoredEmailAddresses`.
const PEOPLE_SELECT: &str = "id,displayName,givenName,surname,companyName,\
scoredEmailAddresses,phones,personType";

// ── Public surface ────────────────────────────────────────────────────

/// Enumerate every Aperio-visible `ContactList` for this account:
///
///   - every Outlook contactFolder under `/me/contactFolders` (the
///     user's writable address books — the default "Contacts"
///     folder plus any sub-folders they've made), and
///   - the synthetic read-only "Suggested People" list backed by
///     `/me/people`.
///
/// Listed in folder order, with Suggested People appended last so
/// the sidebar tree puts the writable books on top and the
/// suggestion stream below — matches the visual hierarchy users
/// get in Outlook on the web.
pub async fn list_contact_lists(state: &ApiState) -> GraphResult<Vec<ContactList>> {
    let mut out: Vec<ContactList> = Vec::new();
    let mut next: Option<String> =
        Some("/me/contactFolders?$select=id,displayName&$top=100".to_string());
    while let Some(path) = next {
        let response: ContactFolderListResponse = state.get_json(&path).await?;
        for folder in response.value {
            out.push(ContactList {
                color_label: None,
                id: folder.id,
                name: folder.display_name.unwrap_or_else(|| "Contacts".into()),
                color: None,
                read_only: false,
            });
        }
        next = response.next_link;
    }
    // Append the synthetic read-only Suggested People list. The
    // English label here is replaced by the frontend's `intl/
    // contactList` sentinel map, so DE users see the translated
    // name.
    out.push(ContactList {
        color_label: None,
        id: GRAPH_SUGGESTED_PEOPLE_LIST_ID.to_string(),
        name: "Suggested People".to_string(),
        color: None,
        read_only: true,
    });
    Ok(out)
}

/// Dispatch on `list_id`: route the read to the folder-scoped
/// `/me/contactFolders/{id}/contacts` for real folders, or to
/// `/me/people` for the synthetic Suggested People sentinel.
/// Unknown ids yield an empty Vec so a misrouted call surfaces as
/// "no contacts" rather than a 404.
pub async fn get_contacts(state: &ApiState, list_id: &str) -> GraphResult<Vec<Contact>> {
    if list_id == GRAPH_SUGGESTED_PEOPLE_LIST_ID {
        list_suggested_people(state).await
    } else if list_id.is_empty() {
        Ok(Vec::new())
    } else {
        list_folder_contacts(state, list_id).await
    }
}

/// Page through every contact in a given contactFolder. Graph
/// returns `@odata.nextLink` on overflow; we follow it verbatim
/// until exhausted. `$top=100` mirrors the listing default and
/// keeps each round-trip small enough that a 500-contact folder
/// still loads in a few seconds.
async fn list_folder_contacts(state: &ApiState, folder_id: &str) -> GraphResult<Vec<Contact>> {
    let folder_enc = urlencoding(folder_id);
    let select_enc = urlencoding(CONTACT_SELECT);
    let mut out: Vec<Contact> = Vec::new();
    let mut next: Option<String> = Some(format!(
        "/me/contactFolders/{folder_enc}/contacts\
         ?$select={select_enc}&$top=100"
    ));
    while let Some(path) = next {
        let response: ContactListResponse = state.get_json(&path).await?;
        for entry in response.value {
            out.push(map_contact(entry, folder_id));
        }
        next = response.next_link;
    }
    Ok(out)
}

/// Outcome of one `contacts/delta` round: created/updated contacts, the
/// ids of removed contacts, and the `@odata.deltaLink` to pass back next
/// time (stored opaquely by the host as the sync token).
#[derive(Debug, Default)]
pub struct ContactDelta {
    pub changes: Vec<Contact>,
    pub deletions: Vec<String>,
    pub new_token: Option<String>,
}

/// Bootstrap a folder delta: a full `contactFolders/{id}/contacts/delta`
/// that also yields the initial `@odata.deltaLink`. Used for the no-token
/// and expired-token (410) paths. The synthetic Suggested People list has
/// no delta endpoint — callers must route it elsewhere (full read).
pub async fn initial_contacts_delta(
    state: &ApiState,
    folder_id: &str,
) -> GraphResult<ContactDelta> {
    let folder_enc = urlencoding(folder_id);
    let select_enc = urlencoding(CONTACT_SELECT);
    let first = format!("/me/contactFolders/{folder_enc}/contacts/delta?$select={select_enc}");
    drain_contacts_delta(state, first, folder_id).await
}

/// Resume a folder delta from a stored `@odata.deltaLink` (an absolute
/// Graph URL carrying the `$deltatoken`). A `410 Gone` surfaces as
/// `GraphError::Http { status: 410, .. }`; the caller re-bootstraps.
pub async fn follow_contacts_delta(
    state: &ApiState,
    delta_link: &str,
    folder_id: &str,
) -> GraphResult<ContactDelta> {
    drain_contacts_delta(state, delta_link.to_string(), folder_id).await
}

/// Drain every page of a contacts delta from `first_url`, following
/// `@odata.nextLink` until the final page hands back `@odata.deltaLink`.
///
/// Live contacts map normally. Tombstones (`@removed`) become deletions
/// by their bare id — a Graph contact's cal-core id IS the raw resource
/// id, so it equals its `native_id` host-side.
async fn drain_contacts_delta(
    state: &ApiState,
    first_url: String,
    folder_id: &str,
) -> GraphResult<ContactDelta> {
    let mut delta = ContactDelta::default();
    let mut next: Option<String> = Some(first_url);
    while let Some(p) = next {
        let resp: ContactDeltaResponse = state.get_json(&p).await?;
        for raw in resp.value {
            if raw.get("@removed").is_some() {
                if let Some(id) = raw.get("id").and_then(|v| v.as_str()) {
                    delta.deletions.push(id.to_string());
                }
                continue;
            }
            let entry: ContactEntry = match serde_json::from_value(raw) {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(?err, "skipping undecodable delta contact row");
                    continue;
                }
            };
            delta.changes.push(map_contact(entry, folder_id));
        }
        if resp.delta_link.is_some() {
            delta.new_token = resp.delta_link;
        }
        next = resp.next_link;
    }
    Ok(delta)
}

/// Pull the relevance-ranked Suggested People stream from
/// `/me/people`. The endpoint caps the response at 1000 items and
/// doesn't paginate the way contactFolders/{id}/contacts does —
/// callers needing more than that should use search instead.
async fn list_suggested_people(state: &ApiState) -> GraphResult<Vec<Contact>> {
    let select_enc = urlencoding(PEOPLE_SELECT);
    let path = format!("/me/people?$select={select_enc}&$top=1000");
    let response: PeopleListResponse = state.get_json(&path).await?;
    Ok(response
        .value
        .into_iter()
        // Drop the awkward `unknownFutureValue` etc. entries Graph
        // sometimes seeds when there's nothing useful to return —
        // they show up with empty emailAddresses + a synthetic id
        // and would render as "(unnamed)" rows.
        .filter(|p| {
            !p.scored_email_addresses
                .as_deref()
                .unwrap_or(&[])
                .is_empty()
                || p.display_name.as_deref().map(str::is_empty) == Some(false)
        })
        .map(|p| map_person(p, GRAPH_SUGGESTED_PEOPLE_LIST_ID))
        .collect())
}

/// Server-side typeahead. Fans out across personal contacts
/// (`/me/contacts?$search=`) and Suggested People (`/me/people?
/// $search=`) in parallel; results are deduped by id and the
/// fan-out swallows per-source errors so a transient 5xx on one
/// path doesn't sink the other.
///
/// Graph requires `$search` queries to be wrapped in double
/// quotes — we add them here so callers can pass plain strings.
pub async fn search_contacts(state: &ApiState, query: &str) -> GraphResult<Vec<Contact>> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let (personal, people) = tokio::join!(
        search_personal_contacts(state, trimmed),
        search_suggested_people(state, trimmed),
    );

    let mut out: Vec<Contact> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for batch in [personal, people] {
        let hits = match batch {
            Ok(h) => h,
            Err(err) => {
                tracing::debug!(
                    target: "adapter_microsoft_graph::contacts",
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

async fn search_personal_contacts(state: &ApiState, query: &str) -> GraphResult<Vec<Contact>> {
    // `$search="…"` requires `ConsistencyLevel: eventual` on the
    // request, which `ApiState::get_json` doesn't set — we use the
    // simpler `$filter=startswith(displayName, '…')` fallback that
    // works without the extra header. Graph allows OR'd
    // startswith clauses across the searchable fields, so we walk
    // the obvious ones (display name, email, surname).
    let q = odata_escape(query);
    let filter = format!(
        "startswith(displayName,'{q}') or startswith(givenName,'{q}') or \
         startswith(surname,'{q}') or \
         emailAddresses/any(e:startswith(e/address,'{q}'))"
    );
    let select_enc = urlencoding(CONTACT_SELECT);
    let filter_enc = urlencoding(&filter);
    let path = format!("/me/contacts?$select={select_enc}&$filter={filter_enc}&$top=100");
    let response: ContactListResponse = state.get_json(&path).await?;
    Ok(response
        .value
        .into_iter()
        // Server-side filter can't tell us which folder the row
        // came from; tag with the row's `parentFolderId` if Graph
        // returned it, falling back to an empty list_id (the
        // dialog handles the empty case by hiding folder-aware
        // affordances).
        .map(|c| {
            let folder = c.parent_folder_id.clone().unwrap_or_default();
            map_contact(c, &folder)
        })
        .collect())
}

async fn search_suggested_people(state: &ApiState, query: &str) -> GraphResult<Vec<Contact>> {
    // `/me/people` supports `$search` natively without the
    // ConsistencyLevel header — wrap the query in double quotes
    // per Graph's syntax requirement.
    let escaped = query.replace('\\', "\\\\").replace('"', "\\\"");
    let select_enc = urlencoding(PEOPLE_SELECT);
    let search_enc = urlencoding(&format!("\"{escaped}\""));
    let path = format!("/me/people?$select={select_enc}&$search={search_enc}&$top=100");
    let response: PeopleListResponse = state.get_json(&path).await?;
    Ok(response
        .value
        .into_iter()
        .map(|p| map_person(p, GRAPH_SUGGESTED_PEOPLE_LIST_ID))
        .collect())
}

/// Create a contact in the target folder. Graph's create
/// endpoint sits under `/me/contactFolders/{folder}/contacts`;
/// the resulting `id` is mailbox-wide unique so subsequent
/// update / delete calls don't need to re-route via the folder.
pub async fn create_contact(
    state: &ApiState,
    folder_id: &str,
    new: NewContact,
) -> GraphResult<Contact> {
    if folder_id == GRAPH_SUGGESTED_PEOPLE_LIST_ID || folder_id.is_empty() {
        return Err(GraphError::Http {
            status: 405,
            message:
                "Suggested People is a read-only relevance stream — create a contact in a folder instead"
                    .into(),
        });
    }
    let folder_enc = urlencoding(folder_id);
    let path = format!("/me/contactFolders/{folder_enc}/contacts");
    let body = new_contact_to_body(&new);
    let entry: ContactEntry = state.post_json(&path, &body).await?;
    let mut contact = map_contact(entry, folder_id);
    // Optional inline photo upload after the row exists. Mirrors
    // the Google / EWS pattern: Graph's contact create body
    // doesn't accept photo bytes either, so we do a second
    // round-trip to PUT them. A failure here keeps the contact
    // (no point unwinding the successful create) and just leaves
    // the avatar empty.
    if let Some(photo) = new.photo {
        if !photo.data.is_empty() {
            if let Err(err) = set_contact_photo(state, &contact.id, photo).await {
                tracing::warn!(
                    contact_id = %contact.id,
                    ?err,
                    "graph contact created but photo upload failed",
                );
                contact.has_photo = false;
            } else {
                contact.has_photo = true;
            }
        }
    }
    Ok(contact)
}

/// PATCH a contact in place. Graph's id is mailbox-wide unique so
/// the `/me/contacts/{id}` endpoint accepts the update regardless
/// of which folder owns the row — same shortcut Graph events use.
pub async fn update_contact(state: &ApiState, contact: Contact) -> GraphResult<Contact> {
    let id_enc = urlencoding(&contact.id);
    let path = format!("/me/contacts/{id_enc}");
    let body = contact_to_body(&contact);
    let entry: ContactEntry = state.patch_json(&path, &body).await?;
    Ok(map_contact(entry, &contact.list_id))
}

/// Delete a contact by id. `/me/contacts/{id}` works regardless
/// of folder — Graph keeps a flat id namespace.
pub async fn delete_contact(state: &ApiState, contact_id: &str) -> GraphResult<()> {
    let id_enc = urlencoding(contact_id);
    let path = format!("/me/contacts/{id_enc}");
    state.delete_request(&path).await
}

/// Fetch the binary contact photo through `/photo/$value`. Two
/// shapes Graph can return:
///
///   - 200 OK with `image/jpeg` (or another image mime) bytes —
///     the common case.
///   - 404 — the contact exists but has no photo. We surface as
///     `Ok(None)` so callers can treat "no avatar" without a
///     conditional error check.
pub async fn get_contact_photo(
    state: &ApiState,
    contact_id: &str,
) -> GraphResult<Option<ContactPhoto>> {
    let id_enc = urlencoding(contact_id);
    let path = format!("/me/contacts/{id_enc}/photo/$value");
    match state.get_bytes(&path).await {
        Ok((bytes, content_type)) => {
            if bytes.is_empty() {
                return Ok(None);
            }
            Ok(Some(ContactPhoto {
                content_type: content_type.unwrap_or_else(|| "image/jpeg".into()),
                data: bytes,
            }))
        }
        Err(GraphError::Http { status: 404, .. }) => Ok(None),
        Err(err) => Err(err),
    }
}

/// PUT the photo bytes back to `/photo/$value`. Graph uploads
/// expect the raw image bytes as the request body with the
/// matching `Content-Type` header — no base64 wrapping here.
pub async fn set_contact_photo(
    state: &ApiState,
    contact_id: &str,
    photo: ContactPhoto,
) -> GraphResult<()> {
    let id_enc = urlencoding(contact_id);
    let path = format!("/me/contacts/{id_enc}/photo/$value");
    state
        .put_bytes(&path, &photo.content_type, photo.data)
        .await
}

/// Delete the photo without touching any other field on the
/// contact. Graph wants a DELETE against `/photo/$value` — the
/// JPEG slot, not the underlying photo navigation property.
pub async fn delete_contact_photo(state: &ApiState, contact_id: &str) -> GraphResult<()> {
    let id_enc = urlencoding(contact_id);
    let path = format!("/me/contacts/{id_enc}/photo/$value");
    state.delete_request(&path).await
}

// ── Mappers ────────────────────────────────────────────────────────────

fn map_contact(entry: ContactEntry, list_id: &str) -> Contact {
    // Graph's `emailAddress` carries a display `name`, not a kind — there is
    // no home/work distinction for addresses on this provider, so they stay
    // unlabelled rather than being given an invented one.
    let emails: Vec<cal_core::ContactValue> = entry
        .email_addresses
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| e.address)
        .filter(|s| !s.is_empty())
        .map(cal_core::ContactValue::bare)
        .collect();
    // Which collection a number arrives in is the only record Graph keeps of
    // what KIND of number it is, and it used to be flattened away. Everything
    // came back unlabelled, and the write path then re-filed by position — so
    // a business number could return home as the mobile one.
    let mut phones: Vec<cal_core::ContactValue> = Vec::new();
    for (numbers, label) in [(entry.business_phones, "work"), (entry.home_phones, "home")] {
        for number in numbers
            .unwrap_or_default()
            .into_iter()
            .filter(|s| !s.is_empty())
        {
            phones.push(cal_core::ContactValue {
                value: number,
                label: Some(label.into()),
            });
        }
    }
    if let Some(mobile) = entry.mobile_phone.filter(|s| !s.is_empty()) {
        phones.push(cal_core::ContactValue {
            value: mobile,
            label: Some("mobile".into()),
        });
    }
    let urls: Vec<cal_core::ContactValue> = entry
        .business_home_page
        .filter(|s| !s.trim().is_empty())
        .map(|url| cal_core::ContactValue {
            value: url,
            label: Some("work".into()),
        })
        .into_iter()
        .collect();
    let birthday = entry.birthday.as_deref().and_then(parse_graph_birthday);

    let display = best_contact_display_name(
        entry.display_name.as_deref(),
        entry.given_name.as_deref(),
        entry.surname.as_deref(),
        emails.first().map(|e| e.value.as_str()),
    );

    let created = entry.created_date_time.unwrap_or_else(Utc::now);
    let updated = entry.last_modified_date_time.unwrap_or(created);

    // Graph flattens postal addresses across three slots
    // (homeAddress / businessAddress / otherAddress). Collect any
    // non-empty ones into our normalised Vec<ContactAddress>.
    let mut addresses: Vec<cal_core::ContactAddress> = Vec::new();
    if let Some(addr) = graph_address_to_core(entry.home_address.as_ref(), "home") {
        addresses.push(addr);
    }
    if let Some(addr) = graph_address_to_core(entry.business_address.as_ref(), "work") {
        addresses.push(addr);
    }
    if let Some(addr) = graph_address_to_core(entry.other_address.as_ref(), "other") {
        addresses.push(addr);
    }

    Contact {
        urls,
        // Graph v1.0 has no anniversary property on `contact` — only
        // `birthday`. Nothing to read, and nothing to write either.
        anniversary: None,
        job_title: entry.job_title.filter(|s| !s.is_empty()),
        department: entry.department.filter(|s| !s.is_empty()),
        id: entry.id,
        list_id: list_id.to_string(),
        display_name: display,
        given_name: entry.given_name.filter(|s| !s.is_empty()),
        family_name: entry.surname.filter(|s| !s.is_empty()),
        organization: entry.company_name.filter(|s| !s.is_empty()),
        emails,
        phone_numbers: phones,
        birthday,
        notes: entry.personal_notes.filter(|s| !s.is_empty()),
        members: None,
        // `has_photo` is expensive to compute at listing time —
        // Graph doesn't include a "has-picture" flag on the
        // contact resource, only a navigation property under
        // `/photo`. The frontend issues a lazy
        // `get_contact_photo` on dialog open; until then we
        // assume "maybe has photo" by flipping the flag on so
        // the avatar placeholder doesn't suppress the fetch.
        // Adapters that *can* compute the flag cheaply (EWS via
        // `HasPicture`) set it precisely; for Graph the
        // optimistic-true default is the best we can do without
        // an extra round-trip per contact.
        has_photo: true,
        addresses,
        created_at: created,
        updated_at: updated,
        etag: entry.etag,
    }
}

/// Lift one of Graph's flat address slots into a cal-core
/// `ContactAddress`. `default_label` is the slot's canonical name
/// ("home" / "work" / "other") and gets baked onto the result so
/// the round-trip back to Graph drops the entry into the matching
/// slot. Returns `None` when the slot is missing or all-empty.
fn graph_address_to_core(
    raw: Option<&PhysicalAddress>,
    default_label: &str,
) -> Option<cal_core::ContactAddress> {
    let raw = raw?;
    let mapped = cal_core::ContactAddress {
        label: Some(default_label.to_string()),
        street: raw.street.clone().filter(|s| !s.is_empty()),
        city: raw.city.clone().filter(|s| !s.is_empty()),
        region: raw.state.clone().filter(|s| !s.is_empty()),
        postal_code: raw.postal_code.clone().filter(|s| !s.is_empty()),
        country: raw.country_or_region.clone().filter(|s| !s.is_empty()),
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

fn map_person(entry: PersonEntry, list_id: &str) -> Contact {
    let emails: Vec<cal_core::ContactValue> = entry
        .scored_email_addresses
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| e.address)
        .filter(|s| !s.is_empty())
        .map(cal_core::ContactValue::bare)
        .collect();
    // Person phones are typed, unlike a contact's — the enum names its own
    // kind, so pass it through instead of guessing.
    let phones: Vec<cal_core::ContactValue> = entry
        .phones
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            let number = p.number.filter(|s| !s.is_empty())?;
            Some(cal_core::ContactValue {
                value: number,
                label: person_phone_label(p.kind.as_deref()),
            })
        })
        .collect();
    let display = best_contact_display_name(
        entry.display_name.as_deref(),
        entry.given_name.as_deref(),
        entry.surname.as_deref(),
        emails.first().map(|e| e.value.as_str()),
    );
    let now = Utc::now();
    Contact {
        urls: Vec::new(),
        anniversary: None,
        job_title: None,
        department: None,
        id: entry.id,
        list_id: list_id.to_string(),
        display_name: display,
        given_name: entry.given_name.filter(|s| !s.is_empty()),
        family_name: entry.surname.filter(|s| !s.is_empty()),
        organization: entry.company_name.filter(|s| !s.is_empty()),
        emails,
        phone_numbers: phones,
        birthday: None,
        notes: None,
        members: None,
        // People surface no photo property — no avatar.
        has_photo: false,
        // The /me/people endpoint doesn't expose postal addresses
        // in its default projection; relevance-ranked hits surface
        // without them.
        addresses: Vec::new(),
        created_at: now,
        updated_at: now,
        etag: None,
    }
}

fn best_contact_display_name(
    display: Option<&str>,
    given: Option<&str>,
    surname: Option<&str>,
    email: Option<&str>,
) -> String {
    if let Some(d) = display {
        let trimmed = d.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let composed = format!("{} {}", given.unwrap_or(""), surname.unwrap_or(""))
        .trim()
        .to_string();
    if !composed.is_empty() {
        return composed;
    }
    email
        .map(str::to_string)
        .unwrap_or_else(|| "(unnamed)".to_string())
}

fn parse_graph_birthday(raw: &str) -> Option<NaiveDate> {
    // Graph emits ISO 8601 with timezone for birthdays —
    // e.g. "1990-06-15T00:00:00Z". Strip the time portion;
    // birthdays carry no useful clock component.
    if let Ok(dt) = raw.parse::<DateTime<Utc>>() {
        return Some(dt.date_naive());
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
}

// ── Body builders ──────────────────────────────────────────────────────

fn new_contact_to_body(new: &NewContact) -> serde_json::Value {
    contact_body(
        &new.display_name,
        new.given_name.as_deref(),
        new.family_name.as_deref(),
        new.organization.as_deref(),
        &new.emails,
        &new.phone_numbers,
        new.birthday,
        new.notes.as_deref(),
        &new.addresses,
        &new.urls,
        new.job_title.as_deref(),
        new.department.as_deref(),
    )
}

fn contact_to_body(contact: &Contact) -> serde_json::Value {
    contact_body(
        &contact.display_name,
        contact.given_name.as_deref(),
        contact.family_name.as_deref(),
        contact.organization.as_deref(),
        &contact.emails,
        &contact.phone_numbers,
        contact.birthday,
        contact.notes.as_deref(),
        &contact.addresses,
        &contact.urls,
        contact.job_title.as_deref(),
        contact.department.as_deref(),
    )
}

fn contact_body(
    display_name: &str,
    given_name: Option<&str>,
    family_name: Option<&str>,
    organization: Option<&str>,
    emails: &[cal_core::ContactValue],
    phones: &[cal_core::ContactValue],
    birthday: Option<NaiveDate>,
    notes: Option<&str>,
    addresses: &[cal_core::ContactAddress],
    urls: &[cal_core::ContactValue],
    job_title: Option<&str>,
    department: Option<&str>,
) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert(
        "displayName".into(),
        serde_json::Value::String(display_name.to_string()),
    );
    if let Some(g) = given_name {
        body.insert("givenName".into(), serde_json::Value::String(g.to_string()));
    }
    if let Some(s) = family_name {
        body.insert("surname".into(), serde_json::Value::String(s.to_string()));
    }
    if let Some(org) = organization {
        body.insert(
            "companyName".into(),
            serde_json::Value::String(org.to_string()),
        );
    }
    // Written even when empty. This body is also the PATCH body, and a field
    // left out of a PATCH is a field left alone — so an address the user
    // deleted has to be sent as an absence, or it survives on the server and
    // comes straight back on the next read.
    body.insert(
        "emailAddresses".into(),
        serde_json::Value::Array(
            emails
                .iter()
                .filter(|e| !e.value.is_empty())
                .map(|e| serde_json::json!({ "address": e.value }))
                .collect(),
        ),
    );
    // Outlook splits phone numbers across three slots —
    // businessPhones (array), homePhones (array), mobilePhone
    // (scalar) — and the LABEL picks which one, the same way it
    // picks the address slot further down. Before this the first
    // number became the mobile one and every other became a
    // business number whatever the user had called it, and
    // `homePhones` was never written at all: a private number
    // moved to the business list the first time Aperio saved.
    //
    // `mobilePhone` holds exactly one. A second number labelled
    // mobile joins the business list instead of evicting the
    // first — Outlook shows it either way, under a heading the
    // user did not choose, so the log says which one moved.
    let mut mobile: Option<&str> = None;
    let mut home_phones: Vec<serde_json::Value> = Vec::new();
    let mut business_phones: Vec<serde_json::Value> = Vec::new();
    for phone in phones.iter().filter(|p| !p.value.is_empty()) {
        let number = || serde_json::Value::String(phone.value.clone());
        match phone.label.as_deref().map(str::trim) {
            Some("mobile") | Some("cell") if mobile.is_none() => {
                mobile = Some(phone.value.as_str());
            }
            Some("mobile") | Some("cell") => {
                tracing::warn!(
                    "Outlook keeps one mobile number per contact; a second one was written to the business list instead"
                );
                business_phones.push(number());
            }
            Some("home") | Some("private") => home_phones.push(number()),
            _ => business_phones.push(number()),
        }
    }
    if let Some(mobile) = mobile {
        body.insert(
            "mobilePhone".into(),
            serde_json::Value::String(mobile.to_string()),
        );
    }
    // Written even when empty: a cleared collection has to reach the server,
    // or a number the user deleted lives on in Outlook forever.
    body.insert("homePhones".into(), serde_json::Value::Array(home_phones));
    body.insert(
        "businessPhones".into(),
        serde_json::Value::Array(business_phones),
    );
    if let Some(bd) = birthday {
        // Graph wants ISO-8601 with timezone — date-only strings
        // are rejected.
        body.insert(
            "birthday".into(),
            serde_json::Value::String(bd.format("%Y-%m-%dT00:00:00Z").to_string()),
        );
    }
    if let Some(notes) = notes {
        body.insert(
            "personalNotes".into(),
            serde_json::Value::String(notes.to_string()),
        );
    }
    // Postal addresses (Phase 10l). Graph's three flat slots map
    // 1:1 to our `label`:
    //   - "home"  → homeAddress
    //   - "work"  → businessAddress
    //   - "other" / unknown → otherAddress
    // The first matching entry per slot wins; subsequent entries
    // with the same label drop because Graph has nowhere to put
    // them. A user wanting "two work addresses" needs to keep them
    // separate via the otherAddress slot — same trade-off the
    // Outlook UI surfaces.
    let mut home: Option<&cal_core::ContactAddress> = None;
    let mut business: Option<&cal_core::ContactAddress> = None;
    let mut other: Option<&cal_core::ContactAddress> = None;
    for addr in addresses {
        match addr
            .label
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("home") => home.get_or_insert(addr),
            Some("work") | Some("business") => business.get_or_insert(addr),
            _ => other.get_or_insert(addr),
        };
    }
    if let Some(addr) = home {
        body.insert("homeAddress".into(), address_to_json(addr));
    }
    if let Some(addr) = business {
        body.insert("businessAddress".into(), address_to_json(addr));
    }
    if let Some(addr) = other {
        body.insert("otherAddress".into(), address_to_json(addr));
    }
    // `null` rather than omission, for the same reason the collections above
    // are written empty: on a PATCH, an omitted field keeps its old value, so
    // a title the user cleared would quietly come back.
    body.insert("jobTitle".into(), string_or_null(job_title));
    body.insert("department".into(), string_or_null(department));
    // Graph gives a contact ONE website. A work-labelled one is the natural
    // occupant of a field called `businessHomePage`; failing that the first
    // URL goes, so a contact with a single personal site still has it stored.
    let website = urls
        .iter()
        .filter(|u| !u.value.is_empty())
        .find(|u| {
            matches!(
                u.label.as_deref().map(str::trim),
                Some("work") | Some("business")
            )
        })
        .or_else(|| urls.iter().find(|u| !u.value.is_empty()));
    if urls.len() > 1 {
        tracing::warn!(
            "Outlook keeps one website per contact; {} further URL(s) were not written",
            urls.len() - 1,
        );
    }
    body.insert(
        "businessHomePage".into(),
        string_or_null(website.map(|u| u.value.as_str())),
    );
    serde_json::Value::Object(body)
}

/// A present value as a JSON string, an absent one as JSON `null` — the
/// difference between "leave this field alone" and "clear it" on a PATCH.
fn string_or_null(value: Option<&str>) -> serde_json::Value {
    match value.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => serde_json::Value::String(text.to_string()),
        None => serde_json::Value::Null,
    }
}

/// Serialise a single cal-core `ContactAddress` into Graph's
/// `PhysicalAddress` JSON shape. Empty fields drop because the
/// `PhysicalAddress` struct uses `skip_serializing_if` — keeps
/// Graph from echoing empty strings back on the next read.
fn address_to_json(addr: &cal_core::ContactAddress) -> serde_json::Value {
    serde_json::to_value(PhysicalAddress {
        street: addr.street.clone(),
        city: addr.city.clone(),
        state: addr.region.clone(),
        country_or_region: addr.country.clone(),
        postal_code: addr.postal_code.clone(),
    })
    .unwrap_or_else(|_| serde_json::Value::Object(Default::default()))
}

/// Escape a literal for use inside an OData `'…'` string. Graph
/// follows the OData spec: single quotes within a quoted string
/// are doubled (`O'Brien` → `O''Brien`).
fn odata_escape(s: &str) -> String {
    s.replace('\'', "''")
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ── JSON wire shapes ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ContactFolderListResponse {
    #[serde(default)]
    value: Vec<ContactFolderEntry>,
    #[serde(default, rename = "@odata.nextLink")]
    next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContactFolderEntry {
    id: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContactListResponse {
    #[serde(default)]
    value: Vec<ContactEntry>,
    #[serde(default, rename = "@odata.nextLink")]
    next_link: Option<String>,
}

/// One page of a `contacts/delta` response. `value` is kept as raw JSON
/// so a tombstone (`{ "id": "…", "@removed": { … } }`) can be told apart
/// from a live contact before deserialising. Intermediate pages carry
/// `@odata.nextLink`; the final page carries `@odata.deltaLink`.
#[derive(Debug, Deserialize)]
struct ContactDeltaResponse {
    #[serde(default)]
    value: Vec<serde_json::Value>,
    #[serde(default, rename = "@odata.nextLink")]
    next_link: Option<String>,
    #[serde(default, rename = "@odata.deltaLink")]
    delta_link: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ContactEntry {
    pub id: String,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, rename = "givenName")]
    pub given_name: Option<String>,
    #[serde(default)]
    pub surname: Option<String>,
    #[serde(default, rename = "companyName")]
    pub company_name: Option<String>,
    #[serde(default, rename = "emailAddresses")]
    pub email_addresses: Option<Vec<EmailAddress>>,
    #[serde(default, rename = "businessPhones")]
    pub business_phones: Option<Vec<String>>,
    #[serde(default, rename = "homePhones")]
    pub home_phones: Option<Vec<String>>,
    #[serde(default, rename = "mobilePhone")]
    pub mobile_phone: Option<String>,
    /// Graph emits this as `YYYY-MM-DDTHH:MM:SSZ`. We parse it
    /// into a NaiveDate via `parse_graph_birthday`.
    #[serde(default)]
    pub birthday: Option<String>,
    #[serde(default, rename = "personalNotes")]
    pub personal_notes: Option<String>,
    #[serde(default, rename = "jobTitle")]
    pub job_title: Option<String>,
    #[serde(default)]
    pub department: Option<String>,
    /// Outlook's single website slot. Graph has no general URL
    /// collection, so this is the one web address a contact can
    /// carry — Aperio surfaces it as a `work`-labelled URL.
    #[serde(default, rename = "businessHomePage")]
    pub business_home_page: Option<String>,
    /// Present when the row was returned by `/me/contacts` rather
    /// than a folder-scoped endpoint — lets us route the result
    /// back into the right `list_id` on the cal-core side.
    #[serde(default, rename = "parentFolderId")]
    pub parent_folder_id: Option<String>,
    /// Graph flattens addresses into three named slots rather
    /// than a single typed array. We round-trip onto our normalised
    /// `Vec<ContactAddress>` via `map_contact` / `contact_body`.
    #[serde(default, rename = "homeAddress")]
    pub home_address: Option<PhysicalAddress>,
    #[serde(default, rename = "businessAddress")]
    pub business_address: Option<PhysicalAddress>,
    #[serde(default, rename = "otherAddress")]
    pub other_address: Option<PhysicalAddress>,
    #[serde(default, rename = "createdDateTime")]
    pub created_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "lastModifiedDateTime")]
    pub last_modified_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "@odata.etag")]
    pub etag: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct PhysicalAddress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// Graph's `state` is the region/province slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(
        default,
        rename = "countryOrRegion",
        skip_serializing_if = "Option::is_none"
    )]
    pub country_or_region: Option<String>,
    #[serde(
        default,
        rename = "postalCode",
        skip_serializing_if = "Option::is_none"
    )]
    pub postal_code: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EmailAddress {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PeopleListResponse {
    #[serde(default)]
    value: Vec<PersonEntry>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PersonEntry {
    pub id: String,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, rename = "givenName")]
    pub given_name: Option<String>,
    #[serde(default)]
    pub surname: Option<String>,
    #[serde(default, rename = "companyName")]
    pub company_name: Option<String>,
    #[serde(default, rename = "scoredEmailAddresses")]
    pub scored_email_addresses: Option<Vec<ScoredEmailAddress>>,
    #[serde(default)]
    pub phones: Option<Vec<PersonPhone>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScoredEmailAddress {
    #[serde(default)]
    pub address: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PersonPhone {
    #[serde(default)]
    pub number: Option<String>,
    /// Graph's `phoneType`: `home`, `business`, `mobile`, `other`,
    /// `assistant`, `homeFax`, `businessFax`, `otherFax`, `pager`,
    /// `radio`. Named `kind` here because `type` is a keyword.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// A Graph `phoneType` as the label Aperio speaks. Unknown members of the
/// enum keep their own name rather than becoming nothing — a number filed
/// under `pager` says "pager", not silence.
fn person_phone_label(kind: Option<&str>) -> Option<String> {
    let kind = kind?.trim();
    if kind.is_empty() {
        return None;
    }
    let label = match kind {
        "business" => "work",
        "homeFax" | "businessFax" | "otherFax" => "fax",
        other => other,
    };
    Some(label.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_contact_collects_every_phone_slot() {
        let entry = ContactEntry {
            job_title: None,
            department: None,
            business_home_page: None,
            id: "AAA".into(),
            display_name: Some("Anna Beispiel".into()),
            given_name: Some("Anna".into()),
            surname: Some("Beispiel".into()),
            company_name: Some("Example AG".into()),
            email_addresses: Some(vec![EmailAddress {
                address: Some("anna@example.com".into()),
                name: Some("Anna Beispiel".into()),
            }]),
            business_phones: Some(vec!["+49 30 1111".into()]),
            home_phones: Some(vec!["+49 30 2222".into()]),
            mobile_phone: Some("+49 170 3333".into()),
            birthday: Some("1990-06-15T00:00:00Z".into()),
            personal_notes: Some("met at conf".into()),
            parent_folder_id: Some("FOLDER-1".into()),
            home_address: Some(PhysicalAddress {
                street: Some("Hauptstraße 1".into()),
                city: Some("Berlin".into()),
                state: None,
                country_or_region: Some("Deutschland".into()),
                postal_code: Some("10115".into()),
            }),
            business_address: None,
            other_address: None,
            created_date_time: None,
            last_modified_date_time: None,
            etag: Some("W/\"abc\"".into()),
        };
        let c = map_contact(entry, "FOLDER-1");
        assert_eq!(c.id, "AAA");
        assert_eq!(c.list_id, "FOLDER-1");
        assert_eq!(c.display_name, "Anna Beispiel");
        assert_eq!(c.given_name.as_deref(), Some("Anna"));
        assert_eq!(c.family_name.as_deref(), Some("Beispiel"));
        assert_eq!(c.organization.as_deref(), Some("Example AG"));
        assert_eq!(c.emails, vec!["anna@example.com".to_string()]);
        assert_eq!(
            c.phone_numbers,
            vec![
                "+49 30 1111".to_string(),
                "+49 30 2222".to_string(),
                "+49 170 3333".to_string(),
            ]
        );
        assert_eq!(c.birthday, NaiveDate::from_ymd_opt(1990, 6, 15),);
        assert_eq!(c.notes.as_deref(), Some("met at conf"));
        assert!(c.members.is_none());
        // Optimistic has_photo — see comment in map_contact.
        assert!(c.has_photo);
        assert_eq!(c.etag.as_deref(), Some("W/\"abc\""));
        // Home address lifted from the homeAddress slot with the
        // `label="home"` tag baked on by `graph_address_to_core`.
        assert_eq!(c.addresses.len(), 1);
        assert_eq!(c.addresses[0].label.as_deref(), Some("home"));
        assert_eq!(c.addresses[0].street.as_deref(), Some("Hauptstraße 1"));
        assert_eq!(c.addresses[0].city.as_deref(), Some("Berlin"));
        assert_eq!(c.addresses[0].postal_code.as_deref(), Some("10115"));
    }

    #[test]
    fn map_contact_falls_back_to_email_when_names_empty() {
        let entry = ContactEntry {
            job_title: None,
            department: None,
            business_home_page: None,
            id: "BBB".into(),
            display_name: Some("".into()),
            given_name: None,
            surname: None,
            company_name: None,
            email_addresses: Some(vec![EmailAddress {
                address: Some("nobody@example.com".into()),
                name: None,
            }]),
            business_phones: None,
            home_phones: None,
            mobile_phone: None,
            birthday: None,
            personal_notes: None,
            parent_folder_id: None,
            home_address: None,
            business_address: None,
            other_address: None,
            created_date_time: None,
            last_modified_date_time: None,
            etag: None,
        };
        let c = map_contact(entry, "FOLDER-1");
        assert_eq!(c.display_name, "nobody@example.com");
    }

    #[test]
    fn parse_graph_birthday_accepts_iso_and_date() {
        assert_eq!(
            parse_graph_birthday("1990-06-15T00:00:00Z"),
            NaiveDate::from_ymd_opt(1990, 6, 15),
        );
        assert_eq!(
            parse_graph_birthday("1990-06-15"),
            NaiveDate::from_ymd_opt(1990, 6, 15),
        );
        assert!(parse_graph_birthday("nonsense").is_none());
    }

    #[test]
    fn map_person_produces_a_contact_without_photo() {
        let entry = PersonEntry {
            id: "P-1".into(),
            display_name: Some("Bob Müller".into()),
            given_name: Some("Bob".into()),
            surname: Some("Müller".into()),
            company_name: Some("Contoso".into()),
            scored_email_addresses: Some(vec![ScoredEmailAddress {
                address: Some("bob@contoso.com".into()),
            }]),
            phones: Some(vec![PersonPhone {
                kind: None,
                number: Some("+49 30 7777".into()),
            }]),
        };
        let c = map_person(entry, GRAPH_SUGGESTED_PEOPLE_LIST_ID);
        assert_eq!(c.list_id, GRAPH_SUGGESTED_PEOPLE_LIST_ID);
        assert_eq!(c.display_name, "Bob Müller");
        assert_eq!(c.emails, vec!["bob@contoso.com".to_string()]);
        assert_eq!(c.phone_numbers, vec!["+49 30 7777".to_string()]);
        assert!(!c.has_photo);
        assert!(c.members.is_none());
    }

    #[test]
    fn the_label_picks_the_outlook_collection_not_the_position() {
        let new = NewContact {
            urls: vec![cal_core::ContactValue {
                value: "https://beispiel.example".into(),
                label: None,
            }],
            anniversary: None,
            job_title: Some("Werkstattleiterin".into()),
            department: Some("Technik".into()),
            display_name: "Anna".into(),
            given_name: Some("Anna".into()),
            family_name: Some("Beispiel".into()),
            organization: None,
            emails: vec!["anna@example.com".into()],
            phone_numbers: vec![
                cal_core::ContactValue {
                    value: "+49 30 1111".into(),
                    label: Some("work".into()),
                },
                cal_core::ContactValue {
                    value: "+49 30 2222".into(),
                    label: Some("home".into()),
                },
                cal_core::ContactValue {
                    value: "+49 170 3333".into(),
                    label: Some("mobile".into()),
                },
            ],
            birthday: NaiveDate::from_ymd_opt(1990, 6, 15),
            notes: None,
            addresses: vec![cal_core::ContactAddress {
                label: Some("home".into()),
                street: Some("Hauptstraße 1".into()),
                city: Some("Berlin".into()),
                region: None,
                postal_code: Some("10115".into()),
                country: Some("Deutschland".into()),
            }],
            members: None,
            photo: None,
        };
        let body = new_contact_to_body(&new);
        assert_eq!(body["displayName"], "Anna");
        // Each number lands in the collection its label names, wherever it sat
        // in the list. Positionally, the work number would have become the
        // mobile one and the private number a business number.
        assert_eq!(body["mobilePhone"], "+49 170 3333");
        assert_eq!(body["homePhones"][0], "+49 30 2222");
        assert_eq!(body["businessPhones"][0], "+49 30 1111");
        assert_eq!(body["emailAddresses"][0]["address"], "anna@example.com");
        assert_eq!(body["jobTitle"], "Werkstattleiterin");
        assert_eq!(body["department"], "Technik");
        assert_eq!(body["businessHomePage"], "https://beispiel.example");
        assert_eq!(body["birthday"], "1990-06-15T00:00:00Z");
        assert_eq!(body["homeAddress"]["street"], "Hauptstraße 1");
        assert_eq!(body["homeAddress"]["postalCode"], "10115");
        // No work / other addresses on this fixture; serde
        // `skip_serializing_if` keeps the keys out entirely.
        assert!(body.get("businessAddress").is_none());
        assert!(body.get("otherAddress").is_none());
    }

    #[test]
    fn odata_escape_doubles_single_quotes() {
        assert_eq!(odata_escape("O'Brien"), "O''Brien");
        assert_eq!(odata_escape("plain"), "plain");
    }
}
