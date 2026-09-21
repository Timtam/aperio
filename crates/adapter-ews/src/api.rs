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

use cal_core::{AttendeeStatus, Calendar, DateRange, Event, FreeBusy, NewEvent};
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderValue, CONTENT_TYPE};

use crate::auth::{basic_auth_header, BasicCredentials};
use crate::error::{EwsError, EwsResult};
use crate::mapping::{
    decode_event_id, encode_event_id, event_to_update_field_xml_on,
    new_event_to_calendar_item_xml_on, parse_find_folder_response, parse_find_item_response,
    parse_first_item_id, parse_get_user_availability, parse_server_time_zones,
    parse_sync_folder_items_counts, parse_sync_folder_items_response, split_calendar_id,
    to_calendar, to_event, DecodedEventId, EventIdKind, ParsedItem, SyncChange,
};
use crate::soap::{
    check_for_fault, create_calendar_item, delete_calendar_item, delete_occurrence_item,
    find_calendar_folders, find_items_in_range, get_occurrence_item, get_recurring_master,
    get_server_time_zones, get_user_availability, respond_to_meeting, sync_folder_items,
    sync_folder_items_idonly, update_calendar_item, update_folder_displayname,
};
use crate::windows_tz::ServerTimeZones;

/// The Windows zone ids this server knows (`GetServerTimeZones`), for writing
/// only ids it accepts (decision 41a).
pub async fn server_time_zones(client: &EwsClient) -> EwsResult<ServerTimeZones> {
    let response = client.post_soap(get_server_time_zones()).await?;
    parse_server_time_zones(&response)
}

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
    ///
    /// Debug logging: both request and response bodies go out on
    /// `tracing::debug!` under the `adapter_ews::soap` target.
    /// Enable with `RUST_LOG=adapter_ews=debug` to see exactly
    /// what Exchange is being asked and what it sends back — useful
    /// when an event shows up in Outlook but not in Aperio.
    /// Bodies are truncated to 16 KiB per direction to keep the log
    /// readable; the truncation marker stays in the line.
    ///
    /// Crate-visible so `tasks.rs` can route its own SOAP envelopes
    /// through the same client without duplicating the HTTP +
    /// fault-check plumbing.
    pub(crate) async fn post_soap(&self, body: String) -> EwsResult<String> {
        let text = self.post_soap_raw(body).await?;
        check_for_fault(&text)?;
        Ok(text)
    }

    /// Like [`post_soap`] but **without** the [`check_for_fault`] pass.
    ///
    /// Used by `GetUserAvailability`, where a per-mailbox
    /// `<m:ResponseMessage ResponseClass="Error">` (an attendee we
    /// can't resolve or aren't permitted to see) is an expected,
    /// partial outcome that must NOT abort the whole free/busy query —
    /// `check_for_fault` bails on the first such message, which is
    /// exactly the wrong behaviour here. The availability parser
    /// instead degrades that one mailbox to "no data". Genuine
    /// transport faults still surface via the HTTP status check below.
    pub(crate) async fn post_soap_raw(&self, body: String) -> EwsResult<String> {
        let auth = basic_auth_header(&self.credentials)?;
        tracing::debug!(
            target: "adapter_ews::soap",
            endpoint = %self.endpoint,
            request = %truncate_for_log(&body),
            "EWS request",
        );
        let mut req = self.http.post(&self.endpoint).body(body).header(
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
        tracing::debug!(
            target: "adapter_ews::soap",
            status = %status.as_u16(),
            response = %truncate_for_log(&text),
            "EWS response",
        );
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
///
/// **Deprecated read path.** Recurring series come back as N
/// flattened occurrences with no master/RRULE, so the frontend
/// can't render them as series (the "series chip", bulk edit and
/// EXDATE skip all silently miss). Kept for tests of the parser
/// alone; the live adapter uses [`sync_events_to_completion`] +
/// the local cache instead.
#[allow(dead_code)]
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

/// Result of one full `SyncFolderItems` drain against `folder_id`:
/// the absolute set of currently-known items + the cookie to pass
/// next time. The caller (the adapter wrapper) folds this into its
/// per-folder cache.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SyncedFolderState {
    /// All items currently believed to live in the folder, keyed
    /// by EWS ItemId. After Create/Update merges and Delete
    /// removals, this is "the truth as of `new_sync_state`".
    pub items: std::collections::HashMap<String, ParsedItem>,
    /// Server cookie to pass back on the next sync. `None` only
    /// when this state has never seen a successful round.
    pub sync_state: Option<String>,
    /// Which item parser filled `items` ([`ITEM_PARSER`]). `0` for state
    /// written before the field existed.
    #[serde(default)]
    pub parser: u32,
}

/// The item parser's version. A cached item from an older parser lacks fields
/// the read rule needs, so a folder state with a lower `parser` is dropped and
/// the folder drained from scratch once.
///
/// 1: stage 4 review — items carry `end_time_zone` (decision 43b).
/// 2: items carry `my_response_type`, whether the mailbox organizes them
///    (decision 70a).
/// 3: a changed occurrence carries its OWN item (`ModifiedOccurrence::own`,
///    decision 58a), so the row stops showing the series' subject, location,
///    body and reminder. A state written by parser 2 holds occurrences whose
///    `own` is `None` and whose cached rows carry the series' content, and
///    the enrichment alone would not mend the rows already handed out.
/// 4: a row that inherits the series' content carries a version of its own
///    (`inherited:{occurrence key}:{series key}`, decision 106), so the host
///    can never take it for the occurrence's own copy. Rows handed out by
///    parser 3 carry the occurrence's bare key for both.
pub const ITEM_PARSER: u32 = 4;

/// How many changes to ask for per `SyncFolderItems` request.
/// Exchange Online caps at 512 per call; smaller is fine but means
/// more round-trips on the initial drain. We pick the maximum to
/// minimise initial-sync latency on cold caches.
const SYNC_BATCH_SIZE: u32 = 512;

/// Run `SyncFolderItems` in a loop against `calendar_id` until the
/// server reports `IncludesLastItemInRange=true`, applying each
/// batch's changes to `state`. Returns the updated state with the
/// freshest sync-cookie.
///
/// On `ErrorInvalidSyncStateData` (the cookie has aged out or the
/// mailbox was rebuilt), the caller should drop `state` and call
/// again with `None` to do a full re-sync. We surface that error
/// verbatim so the caller can branch — handling it inline would
/// silently mask other auth/protocol failures.
pub async fn sync_events_to_completion(
    client: &EwsClient,
    calendar_id: &str,
    state: SyncedFolderState,
) -> EwsResult<SyncedFolderState> {
    // Thin wrapper over `sync_events_delta`: the full-snapshot read path
    // (`refresh_and_read_events`) only needs the merged state, not which
    // ids moved. The delta read path calls `sync_events_delta` directly.
    let (state, _changed, _deleted) = sync_events_delta(client, calendar_id, state).await?;
    Ok(state)
}

/// Like [`sync_events_to_completion`], but additionally reports which
/// item ids the drain touched: `changed` (Create/Update, still present
/// once the drain settles) and `deleted` (Delete, confirmed gone). These
/// drive the incremental cache merge in `EwsAdapter::get_events_delta` —
/// `changed` ids translate into the `ChangeSet`'s events, `deleted` ids
/// are the raw EWS ItemIds the host removes by native id.
///
/// Touched ids are reconciled across pages and against the final merged
/// state: a Create/Update cancels a prior Delete of the same id (and
/// vice-versa), so a server that recreates an id mid-drain reports it
/// once, on the correct side.
pub async fn sync_events_delta(
    client: &EwsClient,
    calendar_id: &str,
    mut state: SyncedFolderState,
) -> EwsResult<(SyncedFolderState, Vec<String>, Vec<String>)> {
    let (folder_id, change_key) = split_calendar_id(calendar_id);
    let cold_start = state.sync_state.is_none();
    let items_before = state.items.len();
    let started = std::time::Instant::now();
    tracing::info!(
        target: "adapter_ews::sync",
        calendar = %calendar_id,
        cold_start,
        items_before,
        "starting SyncFolderItems drain",
    );
    let mut page = 0usize;
    let mut totals = (0usize, 0usize, 0usize); // creates, updates, deletes
                                               // Bound the loop in case a buggy server keeps reporting
                                               // includes_last=false. 64 pages × 512 items = 32 768 items per
                                               // refresh, well past any plausible calendar size.
                                               // Ids touched this drain. A later Delete moves an id out of
                                               // `changed` and into `deleted`; a later Create/Update moves it
                                               // back. Final membership is reconciled against `state.items`.
    let mut changed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut deleted: std::collections::HashSet<String> = std::collections::HashSet::new();
    for _ in 0..64 {
        page += 1;
        let body = sync_folder_items(
            &folder_id,
            change_key.as_deref(),
            state.sync_state.as_deref(),
            SYNC_BATCH_SIZE,
        );
        let xml = client.post_soap(body).await?;
        // DEBUG-only stderr dump of the raw SOAP response. The
        // plugin's tracing dispatcher is cdylib-isolated, so
        // tracing-based debug logs disappear; eprintln! reaches
        // the process stderr unconditionally. Wrapped in an env
        // toggle so it only fires when the user explicitly opts
        // in.
        if std::env::var("APERIO_EWS_DUMP_SOAP").is_ok() {
            eprintln!(
                "[EWS] === SyncFolderItems response (page {}) ===\n{xml}\n[EWS] === end ===",
                page,
            );
        }
        let result = parse_sync_folder_items_response(&xml).inspect_err(|err| {
            eprintln!("[EWS] parse_sync_folder_items_response FAILED on page {page}: {err}",);
        })?;
        let (mut c, mut u, mut d) = (0usize, 0usize, 0usize);
        for change in result.changes {
            match change {
                SyncChange::Create(item) => {
                    eprintln!(
                        "[EWS] Create item_id={} subject={:?} item_type={:?} is_recurring={} has_recurrence={} mods={} dels={}",
                        item.item_id,
                        item.subject,
                        item.item_type,
                        item.is_recurring,
                        item.recurrence.is_some(),
                        item.modified_occurrences.len(),
                        item.deleted_occurrence_starts.len(),
                    );
                    c += 1;
                    deleted.remove(&item.item_id);
                    changed.insert(item.item_id.clone());
                    state.items.insert(item.item_id.clone(), item);
                }
                SyncChange::Update(item) => {
                    eprintln!(
                        "[EWS] Update item_id={} subject={:?} item_type={:?} is_recurring={} has_recurrence={}",
                        item.item_id,
                        item.subject,
                        item.item_type,
                        item.is_recurring,
                        item.recurrence.is_some(),
                    );
                    u += 1;
                    deleted.remove(&item.item_id);
                    changed.insert(item.item_id.clone());
                    state.items.insert(item.item_id.clone(), item);
                }
                SyncChange::Delete(id) => {
                    eprintln!("[EWS] Delete item_id={}", id);
                    d += 1;
                    changed.remove(&id);
                    deleted.insert(id.clone());
                    state.items.remove(&id);
                }
            }
        }
        totals.0 += c;
        totals.1 += u;
        totals.2 += d;
        tracing::debug!(
            target: "adapter_ews::sync",
            calendar = %calendar_id,
            page,
            creates = c,
            updates = u,
            deletes = d,
            includes_last = result.includes_last,
            "SyncFolderItems page",
        );
        state.sync_state = Some(result.new_sync_state);
        if result.includes_last {
            tracing::info!(
                target: "adapter_ews::sync",
                calendar = %calendar_id,
                cold_start,
                pages = page,
                creates = totals.0,
                updates = totals.1,
                deletes = totals.2,
                items_after = state.items.len(),
                duration_ms = started.elapsed().as_millis() as u64,
                "SyncFolderItems drain complete",
            );
            // SyncFolderItems silently strips the complex calendar
            // properties from its response — recurrence shapes AND
            // the description (`<t:Body>`) only come back via GetItem.
            // Fan out before returning so the caller sees a fully-
            // populated state (and persists it with recurrence + body
            // already filled in, sparing the next boot the round-trip).
            enrich_item_details(client, &mut state).await?;
            // Reconcile the touched-id sets against the merged truth:
            // drop any changed id the state no longer holds, and any
            // deleted id it somehow still does.
            let changed_ids: Vec<String> = changed
                .into_iter()
                .filter(|id| state.items.contains_key(id))
                .collect();
            let deleted_ids: Vec<String> = deleted
                .into_iter()
                .filter(|id| !state.items.contains_key(id))
                .collect();
            // Every cached item now comes from the current parser: the caller
            // hands in a state from it or an empty one (`ITEM_PARSER`).
            state.parser = ITEM_PARSER;
            return Ok((state, changed_ids, deleted_ids));
        }
    }
    Err(EwsError::Protocol(
        "SyncFolderItems didn't terminate after 64 pages".into(),
    ))
}

/// Outcome of an IdOnly `SyncFolderItems` probe drain: whether the folder
/// changed since the supplied cookie, and the fresh cookie to store.
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub changed: bool,
    pub sync_state: String,
}

/// Drain an IdOnly `SyncFolderItems` probe against `list_id` to
/// completion, reporting whether ANY item changed since `sync_state` plus
/// the freshest cookie. Cheap — IdOnly responses carry just ids, no item
/// bodies — so the Tasks/Contacts delta path can gate a full re-read on
/// it (the CalDAV CTag pattern, EWS-style).
///
/// `ErrorInvalidSyncStateData` (the cookie aged out) surfaces verbatim,
/// same as [`sync_events_delta`]; the caller drops the cookie and
/// re-probes from `None`.
pub async fn probe_folder_sync(
    client: &EwsClient,
    list_id: &str,
    sync_state: Option<&str>,
) -> EwsResult<ProbeOutcome> {
    let (folder_id, change_key) = split_calendar_id(list_id);
    let mut cookie = sync_state.map(|s| s.to_string());
    let mut changed = false;
    for _ in 0..64 {
        let body = sync_folder_items_idonly(
            &folder_id,
            change_key.as_deref(),
            cookie.as_deref(),
            SYNC_BATCH_SIZE,
        );
        let xml = client.post_soap(body).await?;
        let probe = parse_sync_folder_items_counts(&xml)?;
        if probe.change_count > 0 {
            changed = true;
        }
        cookie = Some(probe.new_sync_state);
        if probe.includes_last {
            return Ok(ProbeOutcome {
                changed,
                sync_state: cookie.unwrap_or_default(),
            });
        }
    }
    Err(EwsError::Protocol(
        "SyncFolderItems probe didn't terminate after 64 pages".into(),
    ))
}

/// Per-request cap for the GetItem fan-out. Microsoft documents a
/// throttling limit around 1000 ids but anything past ~200 starts
/// producing unwieldy request bodies; 100 keeps each round-trip
/// comfortably under 100 KiB and gives the server room under its
/// per-call CPU quota.
const GET_ITEM_BATCH_SIZE: usize = 100;

/// For every cached item whose detail hasn't been fetched yet,
/// batch-GetItem the missing fields (the description `<t:Body>` for
/// all items, plus the `<t:Recurrence>` shape + occurrence overrides
/// for masters) and merge them back into `state.items`. No-op if
/// nothing needs enriching (warm reads where the previous run
/// already populated everything).
///
/// We deliberately drive enrichment off the **cached state**, not
/// off the latest sync batch's deltas: that handles three flows
/// uniformly — cold start (every item is new), Update for an item
/// (the Update overwrites the cached row with `detail_fetched=false`,
/// so it re-qualifies and a changed body/recurrence is re-pulled),
/// and resumed sync after a crash before persistence (still picks
/// them up on the next run).
///
/// The `detail_fetched` flag — not "body is None" — gates the work,
/// so the (very common) bodyless item isn't re-fetched on every
/// single sync.
async fn enrich_item_details(client: &EwsClient, state: &mut SyncedFolderState) -> EwsResult<()> {
    let to_enrich: Vec<(String, Option<String>)> = state
        .items
        .values()
        .filter(|it| !it.detail_fetched)
        .map(|it| (it.item_id.clone(), it.change_key.clone()))
        .collect();

    if to_enrich.is_empty() {
        return Ok(());
    }

    tracing::info!(
        target: "adapter_ews::sync",
        items = to_enrich.len(),
        "GetItem fan-out for body + recurrence enrichment",
    );

    for batch in to_enrich.chunks(GET_ITEM_BATCH_SIZE) {
        let body = crate::soap::get_calendar_items_with_recurrence(batch);
        let xml = client.post_soap(body).await?;
        if std::env::var("APERIO_EWS_DUMP_SOAP").is_ok() {
            eprintln!("[EWS] === GetItem (recurrence) response ===\n{xml}\n[EWS] === end ===",);
        }
        let parsed = crate::mapping::parse_get_calendar_items_response(&xml)?;
        for fresh in parsed {
            let Some(cached) = state.items.get_mut(&fresh.item_id) else {
                continue;
            };
            // Merge the detail-only fields the SyncFolderItems shape
            // couldn't carry: the description (`body`) for every item
            // and the recurrence shape + occurrence overrides for
            // masters. The base fields (subject/start/end/…) keep the
            // authoritative values from the sync drain — don't clobber
            // them with the GetItem copy. `detail_fetched` flips true
            // so this row isn't re-pulled on the next sync.
            cached.body = fresh.body;
            cached.recurrence = fresh.recurrence;
            cached.modified_occurrences = fresh.modified_occurrences;
            cached.deleted_occurrence_starts = fresh.deleted_occurrence_starts;
            // Organizer + attendees (RSVP / availability) are detail-only
            // too: the SyncFolderItems shape never carries them, so they
            // arrive solely through this GetItem fan-out. Merge them here
            // or the read path renders every meeting with an empty
            // attendee list.
            cached.organizer = fresh.organizer;
            cached.my_response_type = fresh.my_response_type;
            cached.attendees = fresh.attendees;
            cached.detail_fetched = true;
            // Refresh the ChangeKey if GetItem returned a newer one
            // — detail reads don't bump it server-side typically,
            // but it costs nothing to keep in sync.
            if fresh.change_key.is_some() {
                cached.change_key = fresh.change_key;
            }
            tracing::debug!(
                target: "adapter_ews::sync",
                item_id = %cached.item_id,
                has_body = cached.body.is_some(),
                has_recurrence = cached.recurrence.is_some(),
                attendees = cached.attendees.len(),
                "EWS detail enrich",
            );
        }
    }

    enrich_occurrence_details(client, state, &to_enrich).await?;

    Ok(())
}

/// Second GetItem pass, run after the master enrichment: a recurring master's
/// `ModifiedOccurrences` each point to a separate EXCEPTION item, and when the
/// organizer cancels just one instance of a series it arrives as a *cancelled
/// exception* — but the inline `<t:ModifiedOccurrences>` shape carries no
/// cancelled flag (only ItemId / Start / End / OriginalStart). Without this the
/// adapter emits a synthetic override for that slot inheriting the master's
/// (un-cancelled) state, so a cancelled occurrence renders as a normal event.
///
/// We fetch the exception items for the masters just enriched, resolve each
/// one's cancelled state from its own `IsCancelled`/`AppointmentState`/subject,
/// and stamp `ModifiedOccurrence.cancelled`.
///
/// Two failure modes are handled deliberately, because the caller persists
/// `detail_fetched=true` for these masters and an unchanged master never
/// re-enriches — so a silently-skipped batch would leave the cancelled state
/// permanently wrong until the series next changes:
///   * A single deleted/inaccessible exception in a 100-id batch comes back as
///     a per-item `ResponseClass="Error"`. We POST via [`post_soap_raw`] (NOT
///     `post_soap`) so `check_for_fault` doesn't abort the whole batch on it —
///     the failed id simply yields no `CalendarItem` and its override stays
///     un-cancelled, while every other exception in the chunk is still stamped.
///   * A genuine transport/parse failure IS propagated (`?`): the surrounding
///     drain then fails without persisting, so the next drain re-runs from the
///     same sync cookie and retries — rather than baking in a half-filled state.
async fn enrich_occurrence_details(
    client: &EwsClient,
    state: &mut SyncedFolderState,
    to_enrich: &[(String, Option<String>)],
) -> EwsResult<()> {
    let enriched: std::collections::HashSet<&str> =
        to_enrich.iter().map(|(id, _)| id.as_str()).collect();

    // WHICH occurrences to ask about:
    //   - those of the masters just enriched, as always;
    //   - every occurrence still without its own item, whatever master it
    //     belongs to. A master is enriched once (`detail_fetched`), so an
    //     occurrence whose GetItem failed that one time would otherwise keep
    //     the series' subject for ever;
    //   - those whose ChangeKey moved since we read them: the master's inline
    //     list was just re-read, and a bumped key is the server saying this
    //     occurrence changed.
    // De-duplicated by item id, so a series with many occurrences asks once.
    let mut seen = std::collections::HashSet::new();
    let exception_refs: Vec<(String, Option<String>)> = state
        .items
        .values()
        .flat_map(|it| {
            let fresh = enriched.contains(it.item_id.as_str());
            it.modified_occurrences.iter().map(move |ov| (fresh, ov))
        })
        .filter(|(fresh, ov)| {
            let stale_key = ov
                .own
                .as_deref()
                .is_some_and(|own| own.change_key != ov.change_key);
            *fresh || ov.own.is_none() || stale_key
        })
        .filter(|(_, ov)| seen.insert(ov.item_id.clone()))
        .map(|(_, ov)| (ov.item_id.clone(), ov.change_key.clone()))
        .collect();

    if exception_refs.is_empty() {
        return Ok(());
    }

    tracing::info!(
        target: "adapter_ews::sync",
        exceptions = exception_refs.len(),
        "GetItem fan-out for the occurrences' own items",
    );

    let mut own_by_id: std::collections::HashMap<String, crate::mapping::ParsedItem> =
        std::collections::HashMap::new();
    for batch in exception_refs.chunks(GET_ITEM_BATCH_SIZE) {
        let body = crate::soap::get_exception_items(batch);
        // `post_soap_raw` skips the fault check so a per-item Error (a deleted or
        // inaccessible occurrence) doesn't poison the whole batch; the successful
        // rows still parse. Transport failures still surface as `Err` here.
        let xml = client.post_soap_raw(body).await?;
        // A fault for the WHOLE batch parses as "no items", which used to read
        // as "nothing changed" and left every occurrence with the series'
        // content, silently. Name it — the code and the ids it hit — and go on
        // to the next batch; the per-item tolerance above is untouched.
        if let Err(err) = crate::soap::check_for_fault(&xml) {
            tracing::warn!(
                target: "adapter_ews::sync",
                error = %err,
                ids = ?batch.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
                "the server refused a whole batch of occurrence items; \
                 those occurrences keep the series' content for now",
            );
            continue;
        }
        let parsed = crate::mapping::parse_get_calendar_items_response(&xml)?;
        for exc in parsed {
            own_by_id.insert(exc.item_id.clone(), exc);
        }
    }

    if own_by_id.is_empty() {
        return Ok(());
    }
    // Stamped across EVERY cached master, not only the ones just enriched:
    // the request list above may hold occurrences of a master enriched long
    // ago, and dropping their answers on the floor would mean asking for them
    // again on every drain, for ever.
    for it in state.items.values_mut() {
        for ov in it.modified_occurrences.iter_mut() {
            let Some(own) = own_by_id.get(&ov.item_id) else {
                continue;
            };
            ov.cancelled = crate::mapping::resolve_cancelled(own);
            // The item must still fill the slot the master named. A server
            // that answers for another slot has moved the occurrence under
            // us, and inheriting the series' content is the safe reading.
            match own.original_start {
                Some(slot) if slot != ov.original_start => {
                    tracing::warn!(
                        target: "adapter_ews::sync",
                        item = %ov.item_id,
                        expected = %ov.original_start,
                        answered = %slot,
                        "an occurrence item answered for another slot; keeping the series' content",
                    );
                    ov.own = None;
                }
                _ => ov.own = Some(Box::new(own.clone())),
            }
        }
    }
    Ok(())
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
    server_zones: Option<&ServerTimeZones>,
) -> EwsResult<Event> {
    let (folder_id, folder_change_key) = split_calendar_id(calendar_id);
    let item_xml = new_event_to_calendar_item_xml_on(&event, server_zones)?;
    // Only ask Exchange to send when the user opted in AND there are
    // attendees to notify — `SendToAllAndSaveCopy` on an attendee-less item
    // would still drop a stray copy into Sent Items for nothing.
    let notify = event.send_invitations && !event.attendees.is_empty();
    let envelope =
        create_calendar_item(&folder_id, folder_change_key.as_deref(), &item_xml, notify);
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
/// Fields with a value are set; some emptied ones become DeleteItemField
/// blocks so EWS clears them server-side (see
/// [`crate::mapping::event_to_update_field_xml_on`]).
///
/// A plain occurrence id resolves to its series master (`GetItem` with
/// `RecurringMasterItemId`), and the edit applies to the whole series. An
/// override id (`{master}::rid::{original_start}`) resolves to the exception's
/// own item through `resolve_override_target`; that update never deletes
/// `calendar:Recurrence`, which Exchange refuses on an exception
/// (`ErrorInvalidPropertyDelete`), and the event comes back with its override
/// id unchanged and the exception's new ChangeKey as `etag`. Skipping an
/// occurrence goes through `add_event_exdate`.
///
/// Exchange will not move an exception onto or past a neighbouring occurrence
/// of its series (`ErrorOccurrenceCrossingBoundary`; Outlook has the same rule).
/// Such an exception is detached instead, see [`detach_exception`], the way
/// Aperio moves every unchanged occurrence on its own.
pub async fn update_event(
    client: &EwsClient,
    event: &Event,
    server_zones: Option<&ServerTimeZones>,
) -> EwsResult<Event> {
    let decoded = decode_event_id(&event.id);
    let target = resolve_write_target(client, &decoded).await?;
    // The provider's own copy, so the update writes what the edit CHANGED
    // rather than every field the row happens to carry (decision 58a). Read
    // here, after the target is resolved: an exception's own ItemId is only
    // known once `resolve_write_target` has found it.
    //
    // An exception is never written without its own copy: every field written
    // blind there is exactly the round-5 defect. What the user is told depends
    // on what went wrong (decision 92: say it). A failed request keeps its own
    // error — a dropped connection, an expired sign-in, a busy or vanished
    // item are not facts about the occurrence, and a retry or a new sign-in
    // may well work. Only an answer that arrives WITHOUT the occurrence's copy
    // is the refusal.
    let before = match read_before(client, &target, &event.calendar_id).await {
        Ok(None) if target.kind == EventIdKind::Exception => {
            tracing::warn!(
                target: "adapter_ews::write",
                event_id = %event.id,
                "the server answered without the occurrence's own copy; refusing to write",
            );
            return Err(EwsError::Protocol(
                cal_core::WriteRefusal::OccurrenceNotWritable.message("own-copy-unreadable"),
            ));
        }
        Ok(found) => found,
        Err(err) if target.kind == EventIdKind::Exception => {
            tracing::warn!(
                target: "adapter_ews::write",
                ?err,
                event_id = %event.id,
                "the occurrence's own copy could not be read; nothing is written",
            );
            return Err(err);
        }
        Err(err) => {
            // A single or a series head: the row is the user's own, so it is
            // written without a comparison — every field the host did not mark
            // as left alone (decision 106).
            tracing::warn!(
                target: "adapter_ews::write",
                ?err,
                event_id = %event.id,
                "the current copy could not be read; writing every field not kept, and a kept rule with a moved slot",
            );
            None
        }
    };
    let (set_xml, delete_xml) =
        event_to_update_field_xml_on(event, before.as_ref(), server_zones, target.kind)?;
    // Nothing to write. Asked of the BUILT XML, not of the diff: the diff can
    // report attendees changed while `keep_attendees` suppresses that block,
    // and an `UpdateItem` with an empty `<t:Updates>` is a fault. This sits
    // AFTER `resolve_write_target`, so an occurrence that is no longer an
    // exception in its series is still refused rather than reported saved.
    if set_xml.is_empty() && delete_xml.is_empty() {
        tracing::info!(
            target: "adapter_ews::write",
            event_id = %event.id,
            "nothing changed; sending no update",
        );
        return Ok(Event {
            etag: before
                .as_ref()
                .and_then(|b| b.etag.clone())
                .or_else(|| event.etag.clone()),
            updated_at: Utc::now(),
            ..event.clone()
        });
    }
    // Removed invitees count too: with every one removed (`clear_attendees`)
    // they may still get a cancellation (decision 74a).
    let notify = event.send_invitations && (!event.attendees.is_empty() || event.clear_attendees);
    // The version the update is applied to is the one the read answered with,
    // not the one the id was minted with (decision 58a). The set is a delta
    // against THAT copy, so that copy is what it belongs on, and a server that
    // checks the ChangeKey (this envelope asks for `AlwaysOverwrite`, so ours
    // does not) is given the version the delta was computed from rather than
    // whatever the host cached last. It is not a lock, and it does not keep
    // another device's change: see `event_to_update_field_xml_on`.
    let change_key = before
        .as_ref()
        .and_then(|b| b.etag.clone())
        .or_else(|| target.change_key.clone());
    let envelope = update_calendar_item(
        &target.item_id,
        change_key.as_deref(),
        &set_xml,
        &delete_xml,
        notify,
    );
    let response = match client.post_soap(envelope).await {
        Err(EwsError::Soap { code, .. })
            if target.kind == EventIdKind::Exception
                && code == "ErrorOccurrenceCrossingBoundary" =>
        {
            // The detached single is CREATED, and a create writes everything
            // it has. What the user did not touch is the occurrence's own, not
            // whatever this device's row said (decision 106): an exception is
            // never written without its own copy, so `before` is here.
            let detached = match before.as_ref() {
                Some(own) => cal_core::event_diff::take_kept_content(event, own),
                None => event.clone(),
            };
            return detach_exception(client, &detached, &target, server_zones).await;
        }
        other => other?,
    };
    let item_ref = parse_first_item_id(&response)?;
    // A single stays a single; a series-wide write (a master, or a master
    // resolved from an occurrence) ends on the master. An exception keeps the
    // override id it was written through: that id names the occurrence, and its
    // own ItemId is looked up live on every write (`resolve_override_target`).
    // Minting a new id from the exception's ItemId would turn it into a series
    // head for the next edit.
    let new_id = match target.kind {
        EventIdKind::Exception => event.id.clone(),
        EventIdKind::Single => encode_event_id(
            EventIdKind::Single,
            &item_ref.id,
            item_ref.change_key.as_deref(),
        ),
        _ => encode_event_id(
            EventIdKind::RecurringMaster,
            &item_ref.id,
            item_ref.change_key.as_deref(),
        ),
    };
    Ok(Event {
        id: new_id,
        etag: item_ref.change_key,
        updated_at: Utc::now(),
        ..event.clone()
    })
}

/// Move an exception Exchange will not move: create `event` as a single
/// appointment of its own, then delete the exception from its series.
///
/// This is how Aperio moves any unchanged occurrence "only this one": out of
/// its series and on its own. It is reached only after Exchange refused the
/// in-place move with `ErrorOccurrenceCrossingBoundary`, and that refusal
/// wrote nothing, so the exception is still as it was.
///
/// Created first, so a failure halfway leaves a duplicate rather than a gap.
/// The delete sends no cancellation: the appointment still exists, it only
/// moved. A delete that fails is logged and the move stands, as for a move to
/// another calendar; the event that comes back is the new single.
async fn detach_exception(
    client: &EwsClient,
    event: &Event,
    exception: &WriteTarget,
    server_zones: Option<&ServerTimeZones>,
) -> EwsResult<Event> {
    let mut single = NewEvent {
        // The exception's own organizer and who organizes it, so the rule
        // below can keep the organizer out of the invitees (decision 72a).
        organized_elsewhere: event.organized_elsewhere,
        organizer: event.organizer.clone(),
        title: event.title.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        start: event.start,
        end: event.end,
        all_day: event.all_day,
        recurrence: None,
        color_label: event.color_label.clone(),
        color_hex: event.color_hex.clone(),
        reminders: event.reminders.clone(),
        sound: event.sound.clone(),
        attendees: event.attendees.clone(),
        send_invitations: event.send_invitations,
    };
    cal_core::attendee::guard_create(&mut single);
    let created = create_event(client, &event.calendar_id, single, server_zones).await?;
    let envelope = delete_calendar_item(&exception.item_id, exception.change_key.as_deref(), false);
    if let Err(err) = client.post_soap(envelope).await {
        tracing::warn!(
            occurrence = %event.id,
            single = %created.id,
            %err,
            "the exception was detached as a single but stays in its series; a duplicate may exist",
        );
    }
    Ok(created)
}

/// Delete a calendar item. For non-recurring events this drops the
/// single row. For occurrences / exceptions of a recurring series,
/// resolves the master and deletes the whole series — matching the
/// "delete recurring event = delete series" UX.
///
/// Per-occurrence skip (the EXDATE-equivalent) goes through
/// `add_event_exdate` instead, which always targets the raw
/// occurrence id.
pub async fn delete_event(
    client: &EwsClient,
    event_id: &str,
    send_cancellations: bool,
) -> EwsResult<()> {
    let decoded = decode_event_id(event_id);
    let target = resolve_write_target(client, &decoded).await?;
    let envelope = delete_calendar_item(
        &target.item_id,
        target.change_key.as_deref(),
        send_cancellations,
    );
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
pub async fn add_event_exdate(
    client: &EwsClient,
    event_id: &str,
    send_cancellations: bool,
) -> EwsResult<()> {
    let decoded = decode_event_id(event_id);
    // `DeleteItem` on the occurrence id removes just this date from the series.
    // With `send_cancellations` the organizer notifies attendees that this one
    // occurrence was cancelled (`SendToAllAndSaveCopy`); without it the drop is
    // silent (`SendToNone`), the plain "delete only this occurrence" behaviour.
    let envelope = delete_calendar_item(
        &decoded.item_id,
        decoded.change_key.as_deref(),
        send_cancellations,
    );
    client.post_soap(envelope).await?;
    Ok(())
}

/// The slot occurrence `index` (1-based) of a recurring master fills: its
/// `OriginalStart` for an exception, which stays put when the exception is
/// moved, else its `Start`. `Ok(None)` when the index is past the end of the
/// series (EWS returns a per-item `ResponseClass="Error"`, which parses to no
/// CalendarItem). Uses `post_soap_raw` so that out-of-range Error doesn't abort
/// via `check_for_fault`.
async fn occurrence_start(
    client: &EwsClient,
    master_id: &str,
    change_key: Option<&str>,
    index: u32,
) -> EwsResult<Option<DateTime<Utc>>> {
    let xml = client
        .post_soap_raw(get_occurrence_item(master_id, change_key, index))
        .await?;
    let items = crate::mapping::parse_get_calendar_items_response(&xml)?;
    // Without the OriginalStart, an exception moved more than the tolerance
    // away from its slot never matched: the delete aborted, and a move that had
    // already created the new event left the exception behind as a duplicate.
    Ok(items
        .into_iter()
        .find_map(|it| it.original_start.or(it.start)))
}

/// The candidate InstanceIndexes to probe: the computed ordinal plus its
/// neighbours, clamped to `>= 1`. The ±1 covers a UTC-vs-local off-by-one at a
/// day boundary (see `nominal_occurrence_index`), which the server verify then
/// resolves.
fn candidate_indices(candidate: u32) -> Vec<u32> {
    let mut v = vec![candidate];
    if candidate > 1 {
        v.push(candidate - 1);
    }
    v.push(candidate + 1);
    v
}

/// Delete / cancel ONE occurrence of a recurring series addressed by its MASTER
/// id (`M:…`) + the occurrence's UTC instant `target`.
///
/// The read path only ever surfaces the master (the frontend expands the series
/// client-side), so "delete/cancel only this occurrence" hands us the master id.
/// `DeleteItem` on a master deletes the WHOLE series — so instead we address the
/// single occurrence by EWS `OccurrenceItemId(master, InstanceIndex)`.
///
/// InstanceIndex is the occurrence's position in the ORIGINAL recurrence pattern
/// (deletions leave index holes and do NOT renumber), so we compute it by
/// expanding the master's own recurrence rule (`nominal_occurrence_index`); a
/// date-based search over live occurrences is defeated by those holes. We then
/// GetItem-verify the candidate index — and its ±1 neighbours, to recover a
/// UTC-vs-local off-by-one — against the SERVER: only if a probed occurrence's
/// slot (an exception's `OriginalStart`, else its real Start) lands within a few
/// hours of `target` do we delete it, otherwise we ABORT rather than risk
/// removing the wrong date. With `send_cancellations` the
/// deleted occurrence emails a per-occurrence CANCEL to attendees.
pub async fn delete_series_occurrence(
    client: &EwsClient,
    master_id: &str,
    change_key: Option<&str>,
    target: DateTime<Utc>,
    send_cancellations: bool,
) -> EwsResult<()> {
    // Frontend and server expand the same rule, so the occurrence's Start lands
    // on `target`; allow a few hours for DST / zone skew while staying well under
    // any real inter-occurrence gap (daily = 24h).
    const TOLERANCE_SECS: i64 = 6 * 3600;

    // 1. Fetch the master's recurrence rule + anchor start.
    let master_xml = client
        .post_soap(crate::soap::get_calendar_items_with_recurrence(&[(
            master_id.to_string(),
            change_key.map(str::to_string),
        )]))
        .await?;
    let master = crate::mapping::parse_get_calendar_items_response(&master_xml)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            EwsError::Protocol("recurring master not found for occurrence delete".into())
        })?;
    let (Some(rec), Some(start)) = (master.recurrence.as_ref(), master.start) else {
        return Err(EwsError::Protocol(
            "recurring master carries no recurrence rule; cannot target an occurrence".into(),
        ));
    };

    // 2. The occurrence's nominal 1-based InstanceIndex from the pattern.
    let candidate =
        crate::mapping::nominal_occurrence_index(rec, start, target).ok_or_else(|| {
            EwsError::Protocol(format!(
                "could not compute the InstanceIndex for the occurrence at {target}"
            ))
        })?;

    // 3. Verify the candidate (and ±1) against the server; delete the occurrence
    //    whose real Start matches `target`, else abort.
    let mut best: Option<(u32, i64)> = None;
    for index in candidate_indices(candidate) {
        if let Some(s) = occurrence_start(client, master_id, change_key, index).await? {
            let delta = (s - target).num_seconds().abs();
            if best.map(|(_, bd)| delta < bd).unwrap_or(true) {
                best = Some((index, delta));
            }
        }
    }
    match best {
        Some((index, delta)) if delta <= TOLERANCE_SECS => {
            client
                .post_soap(delete_occurrence_item(
                    master_id,
                    change_key,
                    index,
                    send_cancellations,
                ))
                .await?;
            Ok(())
        }
        _ => Err(EwsError::Protocol(format!(
            "could not confirm the occurrence at {target} on the server \
             (nearest computed index {candidate}); not deleting to avoid removing the wrong date",
        ))),
    }
}

/// Resolve the (id, change_key) pair to use when writing against a
/// decoded Aperio event id. For Single / RecurringMaster the decoded
/// id is the target directly. An override id (`recurrence_id` set)
/// resolves to the exception's own pair with kind `Exception` through
/// [`resolve_override_target`], and an occurrence it cannot find is an
/// error, never widened to the series. Any other Occurrence / Exception
/// id asks the server for its master via a `GetItem` with the special
/// `RecurringMasterItemId` form and returns the master's pair with kind
/// `RecurringMaster`.
async fn resolve_write_target(
    client: &EwsClient,
    decoded: &DecodedEventId,
) -> EwsResult<WriteTarget> {
    // An override id — `{master}::rid::{original_start}`. Its `item_id` is the
    // SERIES head, so writing against it as-is edits or deletes every
    // occurrence. Resolve the exception's own item instead.
    //
    // This branch has to come first, and it has to be an error rather than a
    // fallback when the occurrence cannot be found. The id says the user is
    // acting on one instance; silently widening that to the series is the one
    // outcome nobody would ask for, and it is unrecoverable — Exchange does not
    // keep the pre-edit occurrences.
    if let Some(original_start) = decoded.recurrence_id {
        return resolve_override_target(client, decoded, original_start).await;
    }
    if !decoded.kind.is_occurrence_like() {
        return Ok(WriteTarget {
            kind: decoded.kind,
            item_id: decoded.item_id.clone(),
            change_key: decoded.change_key.clone(),
        });
    }
    let envelope = get_recurring_master(&decoded.item_id, decoded.change_key.as_deref());
    let response = client.post_soap(envelope).await?;
    let master = parse_first_item_id(&response)?;
    Ok(WriteTarget {
        kind: EventIdKind::RecurringMaster,
        item_id: master.id,
        change_key: master.change_key,
    })
}

/// The exception item behind a `…::rid::…` override id.
///
/// One `GetItem` against the series head, whose `ModifiedOccurrences` carry
/// each exception's own ItemId and ChangeKey — already parsed, see
/// [`crate::mapping::ModifiedOccurrence`]. Matched on `original_start`, the
/// RRULE slot the override displaces, which is the one value that stays put
/// when somebody moves the occurrence to another day.
///
/// Read live rather than baked into the id on purpose: a ChangeKey is an
/// optimistic-concurrency token that goes stale the moment anything else
/// touches the item, and an id that carried one would start failing writes
/// after any unrelated edit from Outlook.
async fn resolve_override_target(
    client: &EwsClient,
    decoded: &DecodedEventId,
    original_start: chrono::DateTime<chrono::Utc>,
) -> EwsResult<WriteTarget> {
    // The series head's own ChangeKey rides along: GetItem accepts it and a
    // stale one is answered with the current item rather than a failure, so it
    // costs nothing and saves a round trip when it is fresh.
    let envelope = crate::soap::get_calendar_items_with_recurrence(&[(
        decoded.item_id.clone(),
        decoded.change_key.clone(),
    )]);
    let response = client.post_soap(envelope).await?;
    let items = crate::mapping::parse_get_calendar_items_response(&response)?;
    let occurrence = items
        .iter()
        .flat_map(|item| item.modified_occurrences.iter())
        .find(|ov| ov.original_start == original_start);
    match occurrence {
        Some(ov) => Ok(WriteTarget {
            kind: EventIdKind::Exception,
            item_id: ov.item_id.clone(),
            change_key: ov.change_key.clone(),
        }),
        // Refused, not widened. The occurrence is gone from the series — someone
        // deleted it, or the series was rewritten — and the only other thing
        // this function could return is the series head, which would turn "edit
        // this one" into "edit all of them".
        None => Err(EwsError::Protocol(format!(
            "the occurrence of {} at {} is no longer an exception in the series; \
             refusing to write to the whole series instead",
            decoded.item_id,
            original_start.to_rfc3339(),
        ))),
    }
}

/// The provider's current copy of the item this update will write.
///
/// Read with the shape that carries every field `changed_fields` compares —
/// the widened detail shape for a single or a series head, the occurrence
/// shape for an exception — so "the location is gone" and "the last reminder
/// is gone" are facts about the item rather than about what the shape forgot
/// to ask for.
///
/// Asked by ITEM ID ALONE, never by the version the caller happens to hold:
/// the point is the copy the server has NOW. A ChangeKey here would make a
/// stale one (the id's, minted whenever the host last read the row) refuse the
/// read, and with it the write it is there to make safe.
///
/// `Ok(None)` when the server answered without the item; a transport or parse
/// failure is `Err`, and the caller decides what that means for the kind of
/// item it is about to write.
async fn read_before(
    client: &EwsClient,
    target: &WriteTarget,
    calendar_id: &str,
) -> EwsResult<Option<Event>> {
    let ids = vec![(target.item_id.clone(), None)];
    let body = match target.kind {
        EventIdKind::Exception => crate::soap::get_exception_items(&ids),
        _ => crate::soap::get_calendar_items_with_recurrence(&ids),
    };
    let xml = client.post_soap(body).await?;
    let items = crate::mapping::parse_get_calendar_items_response(&xml)?;
    let Some(item) = items.into_iter().find(|it| it.item_id == target.item_id) else {
        return Ok(None);
    };
    Ok(Some(crate::mapping::to_event(item, calendar_id)?))
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
    let (folder_id, _) = split_calendar_id(calendar_id);
    // The stable calendar id no longer carries a ChangeKey, but
    // UpdateFolder requires one for optimistic concurrency. Harvest a
    // fresh one via FindFolder right before the write.
    let change_key = folder_change_key(client, &folder_id).await?;
    let envelope = update_folder_displayname(&folder_id, change_key.as_deref(), new_name);
    client.post_soap(envelope).await?;
    Ok(())
}

/// RSVP to a meeting via an `AcceptItem` / `DeclineItem` /
/// `TentativelyAcceptItem` response object. The event id decodes to the
/// meeting's `ItemId` (+ ChangeKey) that the response references.
/// `NeedsAction` isn't respondable (it's the absence of a reply).
pub async fn respond_to_event(
    client: &EwsClient,
    event_id: &str,
    status: AttendeeStatus,
    send_response: bool,
) -> EwsResult<()> {
    let element = match status {
        AttendeeStatus::Accepted => "AcceptItem",
        AttendeeStatus::Declined => "DeclineItem",
        AttendeeStatus::Tentative => "TentativelyAcceptItem",
        AttendeeStatus::NeedsAction => {
            return Err(EwsError::Protocol(
                "cannot RSVP with status needs-action".into(),
            ));
        }
    };
    let decoded = decode_event_id(event_id);
    let envelope = respond_to_meeting(
        &decoded.item_id,
        decoded.change_key.as_deref(),
        element,
        send_response,
    );
    client.post_soap(envelope).await?;
    Ok(())
}

/// Query the free/busy schedule of `emails` over `range` via
/// `GetUserAvailability`.
///
/// Returns one [`FreeBusy`] per requested address, in request order;
/// a mailbox we can't resolve or aren't permitted to see degrades to
/// an empty slot list rather than failing the whole call (so adding
/// one external attendee never blanks the availability of the rest).
/// We send the body through [`EwsClient::post_soap_raw`] — bypassing
/// the per-message fault check — precisely so those partial errors
/// survive into the tolerant parser.
pub async fn query_free_busy(
    client: &EwsClient,
    emails: &[&str],
    range: DateRange,
) -> EwsResult<Vec<FreeBusy>> {
    if emails.is_empty() {
        return Ok(Vec::new());
    }
    let envelope = get_user_availability(emails, range.start, range.end);
    let xml = client.post_soap_raw(envelope).await?;
    parse_get_user_availability(&xml, emails)
}

/// Resolve the current ChangeKey for `folder_id` via FindFolder. The
/// stable calendar id carries only the folder EntryID (the ChangeKey
/// is volatile — see `mapping::to_calendar`), so writes that need a
/// ChangeKey harvest a fresh one here rather than trusting a stale one
/// baked into the id.
async fn folder_change_key(client: &EwsClient, folder_id: &str) -> EwsResult<Option<String>> {
    let xml = client.post_soap(find_calendar_folders()).await?;
    let folders = parse_find_folder_response(&xml)?;
    Ok(folders
        .into_iter()
        .find(|f| f.folder_id == folder_id)
        .and_then(|f| f.change_key))
}

/// Glue: combine a request-side `NewEvent` with the server-assigned
/// ids into a cal-core `Event`. Used by `create_event` to avoid a
/// follow-up GetItem; the field set is whatever the user typed,
/// timestamps default to "now" so the row sorts sensibly in the
/// frontend cache.
/// Trim a SOAP body for safe logging. EWS responses for big calendars
/// can run into the megabytes; dumping all of that would drown
/// `RUST_LOG=debug` output and slow the app. 16 KiB is plenty to
/// see the FindItem envelope, the response messages, and the first
/// ~30 calendar items; anything beyond that is appended with a
/// truncation marker so the reader knows there's more.
fn truncate_for_log(body: &str) -> String {
    const MAX: usize = 16 * 1024;
    if body.len() <= MAX {
        return body.to_string();
    }
    let mut out = String::with_capacity(MAX + 32);
    // Slicing on a byte boundary risks splitting a UTF-8 codepoint —
    // `char_indices` gives us the last valid prefix boundary.
    let cut = body
        .char_indices()
        .take_while(|(i, _)| *i < MAX)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    out.push_str(&body[..cut]);
    out.push_str("\n… [truncated]");
    out
}

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
        keep_attendees: false,
        keep_fields: Vec::new(),
        clear_attendees: false,
        organized_elsewhere: false,
        send_invitations: false,
        truncate_tail_overrides: false,
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
        // Transport-only; EWS has no native COLOR. Carried through (`None`).
        color_hex: new.color_hex.clone(),
        reminders: new.reminders.clone(),
        sound: new.sound.clone(),
        attendees: new.attendees.clone(),
        created_at: now,
        updated_at: now,
        etag: change_key,
        // Write path: organizer/RSVP metadata are read-only fields.
        organizer: None,
        attendee_responses: Vec::new(),
        // Freshly created/updated by us — never a cancellation.
        cancelled: false,
        scheduling_silenced: false,
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
        // Stable id = folder EntryID only; the folder's ChangeKey (K1)
        // is deliberately NOT baked into the id (it's volatile).
        assert_eq!(cals[0].id, "FA");
        assert_eq!(cals[0].name, "Calendar");
        assert!(!cals[0].read_only); // 6f.1b made EWS calendars writable
        assert_eq!(cals[1].id, "FB");
    }

    #[tokio::test]
    async fn sync_events_drains_two_pages_and_returns_final_cookie() {
        // Two-page drain: first response has includes_last=false +
        // one Create; second has includes_last=true + one more
        // Create with a different id. The mock matches FIFO, so
        // the first POST gets page 1, the second gets page 2.
        let mut server = Server::new_async().await;
        let page1 = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:SyncFolderItemsResponse>
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:SyncState>COOKIE-1</m:SyncState>
          <m:IncludesLastItemInRange>false</m:IncludesLastItemInRange>
          <m:Changes>
            <t:Create>
              <t:CalendarItem>
                <t:ItemId Id="A" ChangeKey="CKA"/>
                <t:Subject>First</t:Subject>
                <t:Start>2026-05-01T08:00:00Z</t:Start>
                <t:End>2026-05-01T09:00:00Z</t:End>
                <t:CalendarItemType>Single</t:CalendarItemType>
              </t:CalendarItem>
            </t:Create>
          </m:Changes>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </s:Body>
</s:Envelope>"#;
        let page2 = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:SyncFolderItemsResponse>
      <m:ResponseMessages>
        <m:SyncFolderItemsResponseMessage ResponseClass="Success">
          <m:ResponseCode>NoError</m:ResponseCode>
          <m:SyncState>COOKIE-2</m:SyncState>
          <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
          <m:Changes>
            <t:Create>
              <t:CalendarItem>
                <t:ItemId Id="B" ChangeKey="CKB"/>
                <t:Subject>Second</t:Subject>
                <t:Start>2026-05-02T08:00:00Z</t:Start>
                <t:End>2026-05-02T09:00:00Z</t:End>
                <t:CalendarItemType>Single</t:CalendarItemType>
              </t:CalendarItem>
            </t:Create>
          </m:Changes>
        </m:SyncFolderItemsResponseMessage>
      </m:ResponseMessages>
    </m:SyncFolderItemsResponse>
  </s:Body>
</s:Envelope>"#;
        let _m1 = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "text/xml; charset=utf-8")
            .with_body(page1)
            .expect(1)
            .create_async()
            .await;
        let _m2 = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "text/xml; charset=utf-8")
            .with_body(page2)
            .expect(1)
            .create_async()
            .await;
        let client = client_for(&server);
        let updated = sync_events_to_completion(&client, "FA|FCK", SyncedFolderState::default())
            .await
            .unwrap();
        // Both items present in the merged cache.
        assert_eq!(updated.items.len(), 2);
        assert!(updated.items.contains_key("A"));
        assert!(updated.items.contains_key("B"));
        // Cookie advanced to the last page's cookie.
        assert_eq!(updated.sync_state.as_deref(), Some("COOKIE-2"));
    }

    #[tokio::test]
    async fn probe_folder_sync_flags_changes_and_advances_cookie() {
        // A Task Create on an IdOnly probe → changed=true, fresh cookie.
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:SyncFolderItemsResponse><m:ResponseMessages>
    <m:SyncFolderItemsResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:SyncState>PC1</m:SyncState>
      <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
      <m:Changes>
        <t:Create><t:Task><t:ItemId Id="T1" ChangeKey="K1"/></t:Task></t:Create>
      </m:Changes>
    </m:SyncFolderItemsResponseMessage>
  </m:ResponseMessages></m:SyncFolderItemsResponse></s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let probe = probe_folder_sync(&client_for(&server), "TF|TCK", None)
            .await
            .unwrap();
        assert!(probe.changed);
        assert_eq!(probe.sync_state, "PC1");
    }

    #[tokio::test]
    async fn probe_folder_sync_reports_no_change_on_empty_page() {
        // Empty Changes on the probe → changed=false (host no-ops).
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:SyncFolderItemsResponse><m:ResponseMessages>
    <m:SyncFolderItemsResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:SyncState>PC2</m:SyncState>
      <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
      <m:Changes/>
    </m:SyncFolderItemsResponseMessage>
  </m:ResponseMessages></m:SyncFolderItemsResponse></s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let probe = probe_folder_sync(&client_for(&server), "TF|TCK", Some("PC1"))
            .await
            .unwrap();
        assert!(!probe.changed);
        assert_eq!(probe.sync_state, "PC2");
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
            organized_elsewhere: false,
            organizer: None,
            title: title.into(),
            description: Some("notes".into()),
            location: Some("Online".into()),
            start: "2026-05-20T08:00:00Z".parse().unwrap(),
            end: "2026-05-20T09:00:00Z".parse().unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
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
        let event = create_event(&client, "FOLDER-ID|FCK", new_event("Lunch"), None)
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
        let err = create_event(&client_for(&server), "FOLDER-ID|FCK", new_event("X"), None)
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
        // A single without a rule still clears one: only an exception target
        // leaves the Recurrence delete out. Without it this mock does not
        // match, and the update fails.
        let _m = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(
                r#"<t:DeleteItemField>\s*<t:FieldURI FieldURI="calendar:Recurrence"/>"#.into(),
            ))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let starting = Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
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
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("CK-V1".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        };
        let updated = update_event(&client_for(&server), &starting, None)
            .await
            .unwrap();
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
            .match_body(mockito::Matcher::Regex(r#"Id="ITEM-ID""#.to_string()))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        delete_event(&client_for(&server), "ITEM-ID|CK", false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn enrich_merges_organizer_and_attendees_into_cached_item() {
        // Regression: the GetItem detail fan-out parsed Organizer +
        // RequiredAttendees but the merge dropped them, so every EWS
        // meeting rendered with an empty attendee list. Drive a cold
        // sync (SyncFolderItems Create → GetItem enrich) and assert the
        // cached item carries the attendees.
        let mut server = Server::new_async().await;
        let sync_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:SyncFolderItemsResponse><m:ResponseMessages>
    <m:SyncFolderItemsResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:SyncState>COOKIE-1</m:SyncState>
      <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
      <m:Changes>
        <t:Create>
          <t:CalendarItem>
            <t:ItemId Id="MTG" ChangeKey="CK"/>
            <t:Subject>Planning</t:Subject>
            <t:Start>2026-05-22T08:00:00Z</t:Start>
            <t:End>2026-05-22T09:00:00Z</t:End>
            <t:CalendarItemType>Single</t:CalendarItemType>
          </t:CalendarItem>
        </t:Create>
      </m:Changes>
    </m:SyncFolderItemsResponseMessage>
  </m:ResponseMessages></m:SyncFolderItemsResponse></s:Body>
</s:Envelope>"#;
        let get_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:Items>
        <t:CalendarItem>
          <t:ItemId Id="MTG" ChangeKey="CK"/>
          <t:Subject>Planning</t:Subject>
          <t:Organizer><t:Mailbox>
            <t:Name>The Boss</t:Name><t:EmailAddress>boss@example.com</t:EmailAddress>
          </t:Mailbox></t:Organizer>
          <t:RequiredAttendees>
            <t:Attendee>
              <t:Mailbox><t:Name>Me</t:Name><t:EmailAddress>me@example.com</t:EmailAddress></t:Mailbox>
              <t:ResponseType>Accept</t:ResponseType>
            </t:Attendee>
          </t:RequiredAttendees>
        </t:CalendarItem>
      </m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let _sync = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("SyncFolderItems".into()))
            .with_status(200)
            .with_body(sync_body)
            .create_async()
            .await;
        let _get = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("GetItem".into()))
            .with_status(200)
            .with_body(get_body)
            .create_async()
            .await;

        let state = sync_events_to_completion(&client_for(&server), "FA|FCK", Default::default())
            .await
            .unwrap();
        let item = state.items.get("MTG").expect("item cached");
        assert_eq!(item.organizer.as_deref(), Some("boss@example.com"));
        assert_eq!(item.attendees.len(), 1);
        assert_eq!(item.attendees[0].email, "me@example.com");
        assert_eq!(item.attendees[0].response_type.as_deref(), Some("Accept"));
    }

    /// Decision 58a, end to end: a cold drain reads a series with one changed
    /// occurrence, and the occurrence's own item survives into the cached
    /// state instead of one boolean of it.
    #[tokio::test]
    async fn the_occurrences_own_item_is_kept_not_dropped() {
        let mut server = Server::new_async().await;
        let sync_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:SyncFolderItemsResponse><m:ResponseMessages>
    <m:SyncFolderItemsResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:SyncState>COOKIE-1</m:SyncState>
      <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
      <m:Changes>
        <t:Create>
          <t:CalendarItem>
            <t:ItemId Id="MASTER" ChangeKey="MCK"/>
            <t:Subject>Austausch</t:Subject>
            <t:Location>Raum 1</t:Location>
            <t:Start>2026-07-23T12:00:00Z</t:Start>
            <t:End>2026-07-23T12:30:00Z</t:End>
            <t:IsRecurring>true</t:IsRecurring>
            <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
          </t:CalendarItem>
        </t:Create>
      </m:Changes>
    </m:SyncFolderItemsResponseMessage>
  </m:ResponseMessages></m:SyncFolderItemsResponse></s:Body>
</s:Envelope>"#;
        // The master's own detail GET carries its occurrence list; the second
        // GET (the fan-out this test is about) answers for the occurrence.
        let master_detail = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:Items><t:CalendarItem>
        <t:ItemId Id="MASTER" ChangeKey="MCK"/>
        <t:Subject>Austausch</t:Subject>
        <t:Location>Raum 1</t:Location>
        <t:Start>2026-07-23T12:00:00Z</t:Start>
        <t:End>2026-07-23T12:30:00Z</t:End>
        <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
        <t:Recurrence>
          <t:DailyRecurrence><t:Interval>14</t:Interval></t:DailyRecurrence>
          <t:NoEndRecurrence><t:StartDate>2026-07-23Z</t:StartDate></t:NoEndRecurrence>
        </t:Recurrence>
        <t:ModifiedOccurrences><t:Occurrence>
          <t:ItemId Id="OCC" ChangeKey="OCK"/>
          <t:Start>2026-08-20T15:00:00Z</t:Start>
          <t:End>2026-08-20T15:30:00Z</t:End>
          <t:OriginalStart>2026-08-20T12:00:00Z</t:OriginalStart>
        </t:Occurrence></t:ModifiedOccurrences>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let occurrence_detail = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:Items><t:CalendarItem>
        <t:ItemId Id="OCC" ChangeKey="OCK"/>
        <t:Subject>Nur dieser Termin</t:Subject>
        <t:Location>Raum 2</t:Location>
        <t:Start>2026-08-20T15:00:00Z</t:Start>
        <t:End>2026-08-20T15:30:00Z</t:End>
        <t:OriginalStart>2026-08-20T12:00:00Z</t:OriginalStart>
        <t:CalendarItemType>Exception</t:CalendarItemType>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let _sync = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("SyncFolderItems".into()))
            .with_status(200)
            .with_body(sync_body)
            .create_async()
            .await;
        // The fan-out for the occurrence asks with the exception shape, which
        // never mentions ModifiedOccurrences — that is how the two GETs are
        // told apart here, and it is also the shape's own promise.
        let _occurrence = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(
                r#"(?s)GetItem.*calendar:OriginalStart"#.into(),
            ))
            .with_status(200)
            .with_body(occurrence_detail)
            .create_async()
            .await;
        let _master = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(
                r#"(?s)GetItem.*calendar:ModifiedOccurrences"#.into(),
            ))
            .with_status(200)
            .with_body(master_detail)
            .create_async()
            .await;

        let state = sync_events_to_completion(&client_for(&server), "FA|FCK", Default::default())
            .await
            .unwrap();
        let master = state.items.get("MASTER").expect("the master is cached");
        let occurrence = master
            .modified_occurrences
            .first()
            .expect("the occurrence is cached");
        let own = occurrence
            .own
            .as_deref()
            .expect("the occurrence's own item is kept");
        assert_eq!(own.subject, "Nur dieser Termin");
        assert_eq!(own.location.as_deref(), Some("Raum 2"));
        // And the master keeps its own, which is what it used to lend out.
        assert_eq!(master.subject, "Austausch");
        assert_eq!(master.location.as_deref(), Some("Raum 1"));
    }

    /// An item that answers for ANOTHER slot has been moved under us. Taking
    /// it would put one occurrence's content on another's day, so the row
    /// keeps the series' content and the mismatch is said out loud.
    #[tokio::test]
    async fn an_occurrence_item_for_another_slot_is_refused() {
        let mut server = Server::new_async().await;
        let sync_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:SyncFolderItemsResponse><m:ResponseMessages>
    <m:SyncFolderItemsResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:SyncState>COOKIE-1</m:SyncState>
      <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>
      <m:Changes><t:Create><t:CalendarItem>
        <t:ItemId Id="MASTER" ChangeKey="MCK"/>
        <t:Subject>Austausch</t:Subject>
        <t:Start>2026-07-23T12:00:00Z</t:Start>
        <t:End>2026-07-23T12:30:00Z</t:End>
        <t:IsRecurring>true</t:IsRecurring>
        <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
      </t:CalendarItem></t:Create></m:Changes>
    </m:SyncFolderItemsResponseMessage>
  </m:ResponseMessages></m:SyncFolderItemsResponse></s:Body>
</s:Envelope>"#;
        let master_detail = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:Items><t:CalendarItem>
        <t:ItemId Id="MASTER" ChangeKey="MCK"/>
        <t:Subject>Austausch</t:Subject>
        <t:Start>2026-07-23T12:00:00Z</t:Start>
        <t:End>2026-07-23T12:30:00Z</t:End>
        <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
        <t:Recurrence>
          <t:DailyRecurrence><t:Interval>14</t:Interval></t:DailyRecurrence>
          <t:NoEndRecurrence><t:StartDate>2026-07-23Z</t:StartDate></t:NoEndRecurrence>
        </t:Recurrence>
        <t:ModifiedOccurrences><t:Occurrence>
          <t:ItemId Id="OCC" ChangeKey="OCK"/>
          <t:Start>2026-08-20T15:00:00Z</t:Start>
          <t:End>2026-08-20T15:30:00Z</t:End>
          <t:OriginalStart>2026-08-20T12:00:00Z</t:OriginalStart>
        </t:Occurrence></t:ModifiedOccurrences>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        // Same item id, but it says it fills the slot a fortnight earlier.
        let wrong_slot = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:Items><t:CalendarItem>
        <t:ItemId Id="OCC" ChangeKey="OCK"/>
        <t:Subject>Ein anderer Termin</t:Subject>
        <t:Start>2026-08-06T15:00:00Z</t:Start>
        <t:End>2026-08-06T15:30:00Z</t:End>
        <t:OriginalStart>2026-08-06T12:00:00Z</t:OriginalStart>
        <t:CalendarItemType>Exception</t:CalendarItemType>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let _sync = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("SyncFolderItems".into()))
            .with_status(200)
            .with_body(sync_body)
            .create_async()
            .await;
        let _occurrence = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(
                r#"(?s)GetItem.*calendar:OriginalStart"#.into(),
            ))
            .with_status(200)
            .with_body(wrong_slot)
            .create_async()
            .await;
        let _master = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(
                r#"(?s)GetItem.*calendar:ModifiedOccurrences"#.into(),
            ))
            .with_status(200)
            .with_body(master_detail)
            .create_async()
            .await;

        let state = sync_events_to_completion(&client_for(&server), "FA|FCK", Default::default())
            .await
            .unwrap();
        let occurrence = state.items["MASTER"]
            .modified_occurrences
            .first()
            .expect("the occurrence is cached");
        assert!(
            occurrence.own.is_none(),
            "an item for another slot must not be taken",
        );
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
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
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
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("OCK".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        };
        let updated = update_event(&client_for(&server), &starting, None)
            .await
            .unwrap();
        // After series-wide write the kind flips to RecurringMaster
        // (`M:`), and the id holds the freshly rotated ChangeKey.
        assert_eq!(updated.id, "M:MASTER-ID|MCK-V2");
        assert_eq!(updated.etag.as_deref(), Some("MCK-V2"));
    }

    /// Editing one changed occurrence (an override id): the exception is
    /// resolved from the series head, the update goes to its own item without
    /// deleting a rule Exchange refuses to delete there (live test round 3), and
    /// the returned event keeps the override id.
    #[tokio::test]
    async fn update_event_on_an_override_writes_the_exception_and_keeps_its_id() {
        use std::sync::{Arc, Mutex};
        let mut server = Server::new_async().await;
        let series = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="MASTER-ID" ChangeKey="MCK-V1"/>
        <t:ModifiedOccurrences><t:Occurrence>
          <t:ItemId Id="EXC-ID" ChangeKey="ECK-V1"/>
          <t:Start>2026-10-25T23:00:00Z</t:Start>
          <t:End>2026-10-26T23:00:00Z</t:End>
          <t:OriginalStart>2026-10-25T23:00:00Z</t:OriginalStart>
        </t:Occurrence></t:ModifiedOccurrences>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let updated_body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:UpdateItemResponse><m:ResponseMessages>
    <m:UpdateItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem><t:ItemId Id="EXC-ID" ChangeKey="ECK-V2"/></t:CalendarItem></m:Items>
    </m:UpdateItemResponseMessage>
  </m:ResponseMessages></m:UpdateItemResponse></s:Body>
</s:Envelope>"#;
        // The occurrence's own copy, as `read_before` asks for it: the same
        // title and no location of its own, one day earlier than the edit.
        let occurrence = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="EXC-ID" ChangeKey="ECK-V1"/>
        <t:Subject>Moved to Tuesday</t:Subject>
        <t:Start>2026-10-25T23:00:00Z</t:Start>
        <t:End>2026-10-26T23:00:00Z</t:End>
        <t:IsAllDayEvent>true</t:IsAllDayEvent>
        <t:OriginalStart>2026-10-25T23:00:00Z</t:OriginalStart>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&requests);
        let _any = server
            .mock("POST", "/")
            .with_status(200)
            .with_body_from_request(move |request| {
                let body = request.utf8_lossy_body().unwrap().into_owned();
                let answer = if body.contains("UpdateItem") {
                    updated_body
                } else if body.contains(r#"Id="EXC-ID""#) {
                    occurrence
                } else {
                    series
                };
                seen.lock().unwrap().push(body);
                answer.as_bytes().to_vec()
            })
            .create_async()
            .await;

        let original_start: chrono::DateTime<chrono::Utc> = "2026-10-25T23:00:00Z".parse().unwrap();
        let override_id =
            crate::mapping::encode_override_event_id("M:MASTER-ID|MCK-V1", original_start);
        let edit = Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: override_id.clone(),
            calendar_id: "FOLDER-ID|FCK".into(),
            title: "Moved to Tuesday".into(),
            description: None,
            location: None,
            start: "2026-10-26T23:00:00Z".parse().unwrap(),
            end: "2026-10-27T23:00:00Z".parse().unwrap(),
            all_day: true,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: "2026-09-18T00:00:00Z".parse().unwrap(),
            updated_at: "2026-09-18T00:00:00Z".parse().unwrap(),
            etag: Some("MCK-V1".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        };
        let updated = update_event(&client_for(&server), &edit, None)
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            3,
            "the series, the occurrence's own copy, the UpdateItem",
        );
        let read = &requests[1];
        assert!(
            read.contains(r#"<t:ItemId Id="EXC-ID"/>"#),
            "the second read asks for the occurrence by id alone, not for the series: {read}",
        );
        assert!(
            read.contains(r#"FieldURI="calendar:OriginalStart""#),
            "and asks in the exception's own shape: {read}",
        );
        let update = &requests[2];
        assert!(update.contains("UpdateItem"), "{update}");
        assert!(
            update.contains(r#"<t:ItemId Id="EXC-ID" ChangeKey="ECK-V1"/>"#),
            "{update}"
        );
        assert!(!update.contains("calendar:Recurrence"), "{update}");
        // Decision 58a, changed on purpose: this used to assert that the
        // location was WRITTEN on every save — here, a DeleteItemField, on an
        // occurrence that never had a location of its own. Only the slot moved,
        // so only the slot is written.
        assert!(
            !update.contains(r#"FieldURI="calendar:Location""#),
            "an unchanged location is not written: {update}"
        );
        assert!(
            !update.contains("DeleteItemField"),
            "and nothing is deleted either: {update}"
        );
        assert!(
            !update.contains("item:ReminderIsSet"),
            "an unchanged reminder is not written: {update}"
        );
        assert!(!update.contains("item:Subject"), "{update}");
        assert_eq!(
            update.matches("<t:SetItemField>").count(),
            3,
            "Start, End and IsAllDayEvent, and nothing else: {update}"
        );
        assert_eq!(updated.id, override_id, "the override keeps its id");
        assert_eq!(updated.etag.as_deref(), Some("ECK-V2"));
    }

    /// Answers an exception update with `update_code` and records every
    /// request: the series GetItem, the GetItem for the occurrence's own copy
    /// (decision 58a), the UpdateItem, and whatever follows.
    async fn refuse_exception_update(
        server: &mut Server,
        update_code: &'static str,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
        use std::sync::{Arc, Mutex};
        let envelope = |inner: String| {
            format!(
                r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>{inner}</s:Body>
</s:Envelope>"#
            )
        };
        let series = envelope(
            r#"<m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="MASTER-ID" ChangeKey="MCK-V1"/>
        <t:ModifiedOccurrences><t:Occurrence>
          <t:ItemId Id="EXC-ID" ChangeKey="ECK-V1"/>
          <t:Start>2026-10-20T09:00:00Z</t:Start>
          <t:End>2026-10-20T10:00:00Z</t:End>
          <t:OriginalStart>2026-10-20T08:00:00Z</t:OriginalStart>
        </t:Occurrence></t:ModifiedOccurrences>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse>"#
                .to_string(),
        );
        let refused = envelope(format!(
            r#"<m:UpdateItemResponse><m:ResponseMessages>
    <m:UpdateItemResponseMessage ResponseClass="Error">
      <m:MessageText>refused</m:MessageText>
      <m:ResponseCode>{update_code}</m:ResponseCode>
    </m:UpdateItemResponseMessage>
  </m:ResponseMessages></m:UpdateItemResponse>"#
        ));
        let created = envelope(
            r#"<m:CreateItemResponse><m:ResponseMessages>
    <m:CreateItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem><t:ItemId Id="NEW-ID" ChangeKey="NCK"/></t:CalendarItem></m:Items>
    </m:CreateItemResponseMessage>
  </m:ResponseMessages></m:CreateItemResponse>"#
                .to_string(),
        );
        let deleted = envelope(
            r#"<m:DeleteItemResponse><m:ResponseMessages>
    <m:DeleteItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
    </m:DeleteItemResponseMessage>
  </m:ResponseMessages></m:DeleteItemResponse>"#
                .to_string(),
        );
        // What the exception owns, for `read_before`: its own subject at its
        // own time, so the refused update carries the fields the edit changed.
        let occurrence = envelope(
            r#"<m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="EXC-ID" ChangeKey="ECK-V1"/>
        <t:Subject>Original</t:Subject>
        <t:Start>2026-10-20T09:00:00Z</t:Start>
        <t:End>2026-10-20T10:00:00Z</t:End>
        <t:OriginalStart>2026-10-20T08:00:00Z</t:OriginalStart>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse>"#
                .to_string(),
        );
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&requests);
        server
            .mock("POST", "/")
            .with_status(200)
            .with_body_from_request(move |request| {
                let body = request.utf8_lossy_body().unwrap().into_owned();
                let answer = if body.contains("UpdateItem") {
                    &refused
                } else if body.contains("CreateItem") {
                    &created
                } else if body.contains("DeleteItem") {
                    &deleted
                } else if body.contains(r#"Id="EXC-ID""#) {
                    &occurrence
                } else {
                    &series
                };
                seen.lock().unwrap().push(body);
                answer.as_bytes().to_vec()
            })
            .create_async()
            .await;
        requests
    }

    /// The exception of the Tuesday 08:00Z slot, dragged to Thursday.
    fn exception_moved_to_thursday() -> Event {
        let original_start: chrono::DateTime<chrono::Utc> = "2026-10-20T08:00:00Z".parse().unwrap();
        Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: crate::mapping::encode_override_event_id("M:MASTER-ID|MCK-V1", original_start),
            calendar_id: "FOLDER-ID|FCK".into(),
            title: "Retitled".into(),
            description: None,
            location: None,
            start: "2026-10-22T09:00:00Z".parse().unwrap(),
            end: "2026-10-22T10:00:00Z".parse().unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: "2026-09-18T00:00:00Z".parse().unwrap(),
            updated_at: "2026-09-18T00:00:00Z".parse().unwrap(),
            etag: Some("MCK-V1".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        }
    }

    /// Exchange will not move an exception past a neighbouring occurrence
    /// (decision 64a). The exception is detached instead, as an unchanged
    /// occurrence always is: a single at the new time, the exception gone
    /// from its series without a cancellation.
    #[tokio::test]
    async fn an_exception_exchange_will_not_move_is_detached() {
        let mut server = Server::new_async().await;
        let requests =
            refuse_exception_update(&mut server, "ErrorOccurrenceCrossingBoundary").await;

        let updated = update_event(&client_for(&server), &exception_moved_to_thursday(), None)
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            5,
            "the series, the occurrence's own copy, UpdateItem, CreateItem, DeleteItem",
        );
        let update = &requests[2];
        assert!(
            update.contains("Retitled") && update.contains("2026-10-22T09:00:00Z"),
            "the refused update carried the title and the time the edit changed: {update}",
        );
        let create = &requests[3];
        assert!(create.contains("CreateItem"), "{create}");
        assert!(create.contains("Retitled"), "{create}");
        assert!(create.contains("2026-10-22T09:00:00Z"), "{create}");
        assert!(!create.contains("<t:Recurrence>"), "{create}");
        // The detached single is CREATED, not updated: it is written in full,
        // with the location the edit carries and none it does not.
        let delete = &requests[4];
        assert!(delete.contains("DeleteItem"), "{delete}");
        assert!(
            delete.contains(r#"<t:ItemId Id="EXC-ID" ChangeKey="ECK-V1"/>"#),
            "the exception's own item, never the series: {delete}"
        );
        assert!(delete.contains("SendToNone"), "{delete}");
        assert_eq!(
            updated.id,
            encode_event_id(EventIdKind::Single, "NEW-ID", Some("NCK")),
            "the event that comes back is the new single"
        );
    }

    /// Any other refusal stays a refusal: nothing is created or deleted.
    #[tokio::test]
    async fn an_exception_update_refused_otherwise_is_an_error() {
        let mut server = Server::new_async().await;
        let requests = refuse_exception_update(&mut server, "ErrorIrresolvableConflict").await;

        let err = update_event(&client_for(&server), &exception_moved_to_thursday(), None)
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("ErrorIrresolvableConflict"),
            "{err}"
        );
        assert_eq!(
            requests.lock().unwrap().len(),
            3,
            "the series, the occurrence's own copy, and the UpdateItem — nothing else",
        );
    }

    /// The server's copy of `ITEM-ID` at version `change_key`: "Standup" in
    /// Room 2, 08:00Z to 09:00Z, plus whatever `extra` the test needs.
    fn server_copy(change_key: &str, extra: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="ITEM-ID" ChangeKey="{change_key}"/>
        <t:Subject>Standup</t:Subject>
        <t:Location>Room 2</t:Location>
        <t:Start>2026-05-20T08:00:00Z</t:Start>
        <t:End>2026-05-20T09:00:00Z</t:End>
{extra}
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#
        )
    }

    /// A `GetItem` that fails with `code`.
    fn read_fault(code: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Error">
      <m:MessageText>no</m:MessageText>
      <m:ResponseCode>{code}</m:ResponseCode>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#
        )
    }

    /// A successful `UpdateItem` answer leaving `ITEM-ID` at `change_key`.
    fn update_answer(change_key: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:UpdateItemResponse><m:ResponseMessages>
    <m:UpdateItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem><t:ItemId Id="ITEM-ID" ChangeKey="{change_key}"/></t:CalendarItem></m:Items>
    </m:UpdateItemResponseMessage>
  </m:ResponseMessages></m:UpdateItemResponse></s:Body>
</s:Envelope>"#
        )
    }

    /// Answers every `GetItem` with `read` and everything else with `write`,
    /// recording each request body in order.
    async fn read_then_write(
        server: &mut Server,
        read: String,
        write: String,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
        use std::sync::{Arc, Mutex};
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&requests);
        server
            .mock("POST", "/")
            .with_status(200)
            .with_body_from_request(move |request| {
                let body = request.utf8_lossy_body().unwrap().into_owned();
                let answer = if body.contains("<m:GetItem>") {
                    read.clone()
                } else {
                    write.clone()
                };
                seen.lock().unwrap().push(body);
                answer.into_bytes()
            })
            .create_async()
            .await;
        requests
    }

    /// The edit the host hands over for `ITEM-ID`: field for field what
    /// [`server_copy`] holds.
    fn saved_single() -> Event {
        Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: "S:ITEM-ID|CK-V1".into(),
            calendar_id: "FOLDER-ID|FCK".into(),
            title: "Standup".into(),
            description: None,
            location: Some("Room 2".into()),
            start: "2026-05-20T08:00:00Z".parse().unwrap(),
            end: "2026-05-20T09:00:00Z".parse().unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            updated_at: "2026-05-19T00:00:00Z".parse().unwrap(),
            etag: Some("CK-V1".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        }
    }

    /// A save that changes nothing the server stores sends no update at all:
    /// an `UpdateItem` with an empty `<t:Updates>` is a fault, and on a
    /// meeting every write can mail the guests (decision 76a). What comes
    /// back is the version the server has, so the next save compares against
    /// the right one.
    #[tokio::test]
    async fn a_save_that_builds_no_field_sends_no_update() {
        let mut server = Server::new_async().await;
        let requests = read_then_write(
            &mut server,
            server_copy("CK-V9", ""),
            update_answer("CK-V2"),
        )
        .await;

        let saved = update_event(&client_for(&server), &saved_single(), None)
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            1,
            "the read, and nothing after it: {requests:?}"
        );
        assert_eq!(
            saved.etag.as_deref(),
            Some("CK-V9"),
            "the version the server has, not the one the edit carried",
        );
    }

    /// The same when the diff DOES report a change the write then suppresses:
    /// the server's guest list carries a third person this device never read,
    /// and the host says the edit left the invitees alone (decision 71a). The
    /// gate therefore asks the BUILT XML, not the diff.
    #[tokio::test]
    async fn a_save_the_attendee_rules_suppress_sends_no_update() {
        let mut server = Server::new_async().await;
        let requests = read_then_write(
            &mut server,
            server_copy(
                "CK-V9",
                "        <t:RequiredAttendees><t:Attendee><t:Mailbox>\
<t:EmailAddress>carol@example.com</t:EmailAddress></t:Mailbox></t:Attendee>\
</t:RequiredAttendees>",
            ),
            update_answer("CK-V2"),
        )
        .await;

        let edit = Event {
            keep_attendees: true,
            keep_fields: Vec::new(),
            ..saved_single()
        };
        update_event(&client_for(&server), &edit, None)
            .await
            .unwrap();

        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "the read, and nothing after it",
        );
    }

    /// The update is applied to the version the read answered with, not to
    /// the one baked into the id (which the host may have cached long ago).
    #[tokio::test]
    async fn the_update_carries_the_change_key_the_before_read_returned() {
        let mut server = Server::new_async().await;
        let requests = read_then_write(
            &mut server,
            server_copy("CK-V9", ""),
            update_answer("CK-V10"),
        )
        .await;

        let edit = Event {
            title: "Standup, 15 minutes".into(),
            ..saved_single()
        };
        let saved = update_event(&client_for(&server), &edit, None)
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "the read, then the update");
        assert!(
            requests[0].contains(r#"<t:ItemId Id="ITEM-ID"/>"#),
            "the read asks by id alone: {}",
            requests[0],
        );
        let update = &requests[1];
        assert!(
            update.contains(r#"<t:ItemId Id="ITEM-ID" ChangeKey="CK-V9"/>"#),
            "the version the read answered with, not the id's CK-V1: {update}",
        );
        assert!(
            !update.contains(r#"FieldURI="calendar:Location""#),
            "and only the title is written: {update}",
        );
        assert_eq!(saved.id, "S:ITEM-ID|CK-V10");
    }

    /// A read that fails on a single or a series head is not fatal: the row is
    /// the user's own, the save writes every field as it always did, and the
    /// log says why (decision 102).
    #[tokio::test]
    async fn a_failed_before_read_on_a_single_still_saves_the_whole_edit() {
        let mut server = Server::new_async().await;
        let requests = read_then_write(
            &mut server,
            read_fault("ErrorItemNotFound"),
            update_answer("CK-V2"),
        )
        .await;

        update_event(&client_for(&server), &saved_single(), None)
            .await
            .expect("the save still goes through");

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "the failed read, then the update");
        let update = &requests[1];
        for field in ["item:Subject", "calendar:Location", "calendar:Start"] {
            assert!(
                update.contains(field),
                "{field} is written without a before: {update}",
            );
        }
        assert!(
            update.contains(r#"<t:ItemId Id="ITEM-ID" ChangeKey="CK-V1"/>"#),
            "with the only version there is, the id's: {update}",
        );
    }

    /// Serves the series that names `EXC-ID` as the exception of the Tuesday
    /// 08:00Z slot, answers the read of `EXC-ID` itself with `own_copy`, and
    /// records every request.
    async fn read_exception_with(
        server: &mut Server,
        own_copy: String,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
        use std::sync::{Arc, Mutex};
        let series = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="MASTER-ID" ChangeKey="MCK-V1"/>
        <t:ModifiedOccurrences><t:Occurrence>
          <t:ItemId Id="EXC-ID" ChangeKey="ECK-V1"/>
          <t:Start>2026-10-20T09:00:00Z</t:Start>
          <t:End>2026-10-20T10:00:00Z</t:End>
          <t:OriginalStart>2026-10-20T08:00:00Z</t:OriginalStart>
        </t:Occurrence></t:ModifiedOccurrences>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#
            .to_string();
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&requests);
        server
            .mock("POST", "/")
            .with_status(200)
            .with_body_from_request(move |request| {
                let body = request.utf8_lossy_body().unwrap().into_owned();
                let answer = if body.contains(r#"Id="EXC-ID""#) {
                    own_copy.clone()
                } else {
                    series.clone()
                };
                seen.lock().unwrap().push(body);
                answer.into_bytes()
            })
            .create_async()
            .await;
        requests
    }

    /// An exception is the one kind where a full write is the measured defect
    /// (live round 5): without its own copy, every field written is the
    /// SERIES'. When the server answers without that copy, the write is
    /// refused, and the surfaces say so in their own sentence.
    #[tokio::test]
    async fn an_exception_whose_own_copy_cannot_be_read_is_refused() {
        let mut server = Server::new_async().await;
        let answered_without_it = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items/>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#;
        let requests = read_exception_with(&mut server, answered_without_it.to_string()).await;

        let err = update_event(&client_for(&server), &exception_moved_to_thursday(), None)
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("occurrence-not-writable"),
            "the surfaces read the refusal from the start of the message: {err}",
        );
        assert!(err.to_string().contains("own-copy-unreadable"), "{err}");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "the series, the copy that was not there");
        assert!(
            !requests.iter().any(|body| body.contains("UpdateItem")),
            "and nothing was written",
        );
    }

    /// A request that FAILS is not a fact about the occurrence. Nothing is
    /// written, but the error keeps its own kind — here a busy server, which a
    /// second attempt may well get past — instead of telling the user this
    /// occurrence cannot be changed on its own.
    #[tokio::test]
    async fn a_failed_read_of_an_exception_keeps_its_own_error() {
        let mut server = Server::new_async().await;
        let requests = read_exception_with(&mut server, read_fault("ErrorServerBusy")).await;

        let err = update_event(&client_for(&server), &exception_moved_to_thursday(), None)
            .await
            .unwrap_err();

        match &err {
            EwsError::Soap { code, .. } => assert_eq!(code, "ErrorServerBusy"),
            other => panic!("expected the server's own fault, got {other:?}"),
        }
        assert!(
            !err.to_string().contains("occurrence-not-writable"),
            "{err}"
        );
        let requests = requests.lock().unwrap();
        assert!(
            !requests.iter().any(|body| body.contains("UpdateItem")),
            "nothing was written",
        );
    }

    /// The fields a time-only edit leaves alone, as the host marks them.
    fn kept_but_the_slot() -> Vec<cal_core::event_diff::EventField> {
        use cal_core::event_diff::EventField;
        EventField::ALL
            .into_iter()
            .filter(|f| {
                !matches!(
                    f,
                    EventField::Start
                        | EventField::End
                        | EventField::Attendees
                        | EventField::ColorHex
                )
            })
            .collect()
    }

    /// Decision 106: another device renamed the event after this one read it.
    /// A time-only save leaves the title as the server has it, although this
    /// device's copy — and so the edit — still carries the old one.
    #[tokio::test]
    async fn another_devices_title_survives_a_time_only_save() {
        let mut server = Server::new_async().await;
        let requests = read_then_write(
            &mut server,
            server_copy("CK-V9", ""),
            update_answer("CK-V10"),
        )
        .await;

        let edit = Event {
            title: "Standup, before the rename".into(),
            start: "2026-05-20T09:00:00Z".parse().unwrap(),
            end: "2026-05-20T10:00:00Z".parse().unwrap(),
            keep_fields: kept_but_the_slot(),
            ..saved_single()
        };
        update_event(&client_for(&server), &edit, None)
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        let update = &requests[1];
        assert!(!update.contains("item:Subject"), "{update}");
        assert!(update.contains(r#"FieldURI="calendar:Start""#), "{update}");
    }

    /// A read that fails on a single still writes in full what the user did
    /// not leave alone — and nothing they did.
    #[tokio::test]
    async fn a_failed_read_still_leaves_the_kept_fields_alone() {
        use cal_core::event_diff::EventField;
        let mut server = Server::new_async().await;
        let requests = read_then_write(
            &mut server,
            read_fault("ErrorServerBusy"),
            update_answer("CK-V2"),
        )
        .await;

        let edit = Event {
            title: "Standup, renamed".into(),
            keep_fields: EventField::ALL
                .into_iter()
                .filter(|f| {
                    !matches!(
                        f,
                        EventField::Title | EventField::Attendees | EventField::ColorHex
                    )
                })
                .collect(),
            ..saved_single()
        };
        update_event(&client_for(&server), &edit, None)
            .await
            .expect("the save goes through");

        let requests = requests.lock().unwrap();
        let update = &requests[1];
        assert!(update.contains("item:Subject"), "{update}");
        for field in ["calendar:Location", "calendar:Start", "item:ReminderIsSet"] {
            assert!(!update.contains(field), "{field} was left alone: {update}");
        }
    }

    /// An exception Exchange will not move is created again as a single. What
    /// the user did not touch comes from the occurrence's own copy, not from a
    /// row that inherited the series' (decision 106).
    #[tokio::test]
    async fn a_detached_occurrence_takes_what_the_user_did_not_touch_from_its_own_copy() {
        use cal_core::event_diff::EventField;
        let mut server = Server::new_async().await;
        let requests =
            refuse_exception_update(&mut server, "ErrorOccurrenceCrossingBoundary").await;

        let edit = Event {
            title: "The series' title".into(),
            keep_fields: vec![
                EventField::Title,
                EventField::Description,
                EventField::Location,
                EventField::Reminders,
            ],
            ..exception_moved_to_thursday()
        };
        update_event(&client_for(&server), &edit, None)
            .await
            .expect("detached");

        let requests = requests.lock().unwrap();
        let update = &requests[2];
        assert!(!update.contains("item:Subject"), "{update}");
        let create = &requests[3];
        assert!(create.contains("CreateItem"), "{create}");
        assert!(
            create.contains("<t:Subject>Original</t:Subject>"),
            "{create}"
        );
        assert!(!create.contains("The series' title"), "{create}");
        assert!(
            create.contains("2026-10-22T09:00:00Z"),
            "the slot is the edit's: {create}"
        );
    }

    /// The diff belongs to the update path alone. An exception Exchange will
    /// not move is CREATED elsewhere, and a create writes everything it has —
    /// there is nothing on the server yet to compare with.
    #[tokio::test]
    async fn a_detached_exception_still_writes_every_field() {
        let mut server = Server::new_async().await;
        let requests =
            refuse_exception_update(&mut server, "ErrorOccurrenceCrossingBoundary").await;

        let edit = Event {
            location: Some("Room 5".into()),
            reminders: vec![cal_core::Reminder {
                kind: cal_core::ReminderKind::Relative { minutes_before: 15 },
                sound: None,
            }],
            ..exception_moved_to_thursday()
        };
        update_event(&client_for(&server), &edit, None)
            .await
            .expect("detached");

        let requests = requests.lock().unwrap();
        let create = &requests[3];
        assert!(create.contains("CreateItem"), "{create}");
        assert!(
            create.contains("<t:Location>Room 5</t:Location>"),
            "{create}"
        );
        assert!(
            create.contains("<t:ReminderIsSet>true</t:ReminderIsSet>"),
            "{create}",
        );
        assert!(
            create.contains("<t:ReminderMinutesBeforeStart>15</t:ReminderMinutesBeforeStart>"),
            "{create}",
        );
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
            // mockito would return the default 501. Silent skip →
            // SendToNone (no attendee cancellation).
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("DeleteType".into()),
                mockito::Matcher::Regex("SendToNone".into()),
            ]))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        add_event_exdate(&client_for(&server), "O:OCC-ID|OCK", false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn add_event_exdate_with_cancellations_notifies_attendees() {
        // Organizer cancelling just this occurrence → DeleteItem the occurrence
        // id with SendToAllAndSaveCopy so attendees get a per-occurrence CANCEL.
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
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("DeleteType".into()),
                mockito::Matcher::Regex("SendToAllAndSaveCopy".into()),
            ]))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        add_event_exdate(&client_for(&server), "O:OCC-ID|OCK", true)
            .await
            .unwrap();
    }

    /// A `GetItemResponse` for one occurrence with the given `Start`.
    fn occurrence_get_response(start_iso: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="OCC" ChangeKey="OCK"/>
        <t:Start>{start_iso}</t:Start>
        <t:End>{start_iso}</t:End>
        <t:CalendarItemType>Occurrence</t:CalendarItemType>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#
        )
    }

    /// A `GetItemResponse` for the recurring MASTER "MASTER|CK": a weekly-Monday
    /// series (no end) anchored 2026-07-06 → nominal occurrences 07-06 (idx 1),
    /// 07-13 (2), 07-20 (3), 07-27 (4), …
    fn master_weekly_response() -> String {
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="MASTER" ChangeKey="CK"/>
        <t:Subject>Weekly</t:Subject>
        <t:Start>2026-07-06T09:00:00Z</t:Start>
        <t:End>2026-07-06T09:30:00Z</t:End>
        <t:IsRecurring>true</t:IsRecurring>
        <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
        <t:Recurrence>
          <t:WeeklyRecurrence>
            <t:Interval>1</t:Interval>
            <t:DaysOfWeek>Monday</t:DaysOfWeek>
          </t:WeeklyRecurrence>
          <t:NoEndRecurrence><t:StartDate>2026-07-06</t:StartDate></t:NoEndRecurrence>
        </t:Recurrence>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#
            .to_string()
    }

    /// Register the master-recurrence GetItem mock (matches the plain
    /// `ItemId Id="MASTER"` request, distinct from the `OccurrenceItemId` probes).
    async fn mock_master(server: &mut Server) -> mockito::Mock {
        server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("m:GetItem".into()),
                mockito::Matcher::Regex(r#"ItemId Id="MASTER""#.into()),
            ]))
            .with_status(200)
            .with_body(master_weekly_response())
            .create_async()
            .await
    }

    #[tokio::test]
    async fn delete_series_occurrence_targets_the_matching_index() {
        // Weekly Monday series; target the 3rd occurrence (2026-07-20). The
        // InstanceIndex is computed from the master's rule (nominal position 3),
        // then verified against the server, and the DeleteItem must hit
        // OccurrenceItemId InstanceIndex=3 — NOT the master, NOT a neighbour.
        let mut server = Server::new_async().await;
        let _master = mock_master(&mut server).await;
        let dates = [
            (2, "2026-07-13T09:00:00Z"),
            (3, "2026-07-20T09:00:00Z"),
            (4, "2026-07-27T09:00:00Z"),
        ];
        let mut get_mocks = Vec::new();
        for (idx, iso) in dates {
            get_mocks.push(
                server
                    .mock("POST", "/")
                    .match_body(mockito::Matcher::AllOf(vec![
                        mockito::Matcher::Regex("m:GetItem".into()),
                        mockito::Matcher::Regex(format!(r#"InstanceIndex="{idx}""#)),
                    ]))
                    .with_status(200)
                    .with_body(occurrence_get_response(iso))
                    .create_async()
                    .await,
            );
        }
        // The delete must hit index 3 with SendToNone (silent skip).
        let del = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("DeleteType".into()),
                mockito::Matcher::Regex(r#"InstanceIndex="3""#.into()),
                mockito::Matcher::Regex("SendToNone".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <s:Body><m:DeleteItemResponse><m:ResponseMessages>
    <m:DeleteItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
    </m:DeleteItemResponseMessage>
  </m:ResponseMessages></m:DeleteItemResponse></s:Body>
</s:Envelope>"#,
            )
            .expect(1)
            .create_async()
            .await;

        let target = "2026-07-20T09:00:00Z".parse().unwrap();
        delete_series_occurrence(&client_for(&server), "MASTER", Some("CK"), target, false)
            .await
            .unwrap();
        del.assert_async().await;
    }

    #[tokio::test]
    async fn delete_series_occurrence_aborts_when_no_occurrence_lines_up() {
        // Target 2026-07-21 — a date with NO occurrence (the series is Mondays).
        // The nearest occurrence (07-20) is >6h away, so we must ERROR rather than
        // delete it. No DeleteItem mock: a stray delete would 501 and fail.
        let mut server = Server::new_async().await;
        let _master = mock_master(&mut server).await;
        for (idx, iso) in [(2, "2026-07-13T09:00:00Z"), (4, "2026-07-27T09:00:00Z")] {
            let _m = server
                .mock("POST", "/")
                .match_body(mockito::Matcher::AllOf(vec![
                    mockito::Matcher::Regex("m:GetItem".into()),
                    mockito::Matcher::Regex(format!(r#"InstanceIndex="{idx}""#)),
                ]))
                .with_status(200)
                .with_body(occurrence_get_response(iso))
                .create_async()
                .await;
        }
        // index 3 lookups return the 07-20 occurrence, but the target is 07-21
        // (no occurrence there) — nearest (07-20) is >6h away → abort.
        let _m3 = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("m:GetItem".into()),
                mockito::Matcher::Regex(r#"InstanceIndex="3""#.into()),
            ]))
            .with_status(200)
            .with_body(occurrence_get_response("2026-07-20T09:00:00Z"))
            .create_async()
            .await;

        let target = "2026-07-21T09:00:00Z".parse().unwrap();
        let err =
            delete_series_occurrence(&client_for(&server), "MASTER", Some("CK"), target, false)
                .await
                .unwrap_err();
        assert!(matches!(err, EwsError::Protocol(_)));
    }

    /// An exception moved three days away from its slot is still the
    /// occurrence of that slot: it is matched by its `OriginalStart`, not by
    /// where it now starts. Matched by its Start it was never found, the delete
    /// aborted, and a move that had already created the new event left the
    /// exception behind as a duplicate.
    #[tokio::test]
    async fn delete_series_occurrence_finds_an_exception_moved_far_by_its_slot() {
        let mut server = Server::new_async().await;
        let _master = mock_master(&mut server).await;
        for (idx, iso) in [(2, "2026-07-13T09:00:00Z"), (4, "2026-07-27T09:00:00Z")] {
            let _m = server
                .mock("POST", "/")
                .match_body(mockito::Matcher::AllOf(vec![
                    mockito::Matcher::Regex("m:GetItem".into()),
                    mockito::Matcher::Regex(format!(r#"InstanceIndex="{idx}""#)),
                ]))
                .with_status(200)
                .with_body(occurrence_get_response(iso))
                .create_async()
                .await;
        }
        // Index 3 is the exception: its slot is 07-20 09:00, but it now starts
        // on 07-23 at 15:00. The probe only gets its OriginalStart when it asks
        // for it, so this mock answers only such a probe.
        let _m3 = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("m:GetItem".into()),
                mockito::Matcher::Regex(r#"InstanceIndex="3""#.into()),
                mockito::Matcher::Regex(r#"FieldURI="calendar:OriginalStart""#.into()),
            ]))
            .with_status(200)
            .with_body(
                r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body><m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
      <m:Items><t:CalendarItem>
        <t:ItemId Id="EXC" ChangeKey="ECK"/>
        <t:Start>2026-07-23T15:00:00Z</t:Start>
        <t:End>2026-07-23T16:00:00Z</t:End>
        <t:CalendarItemType>Exception</t:CalendarItemType>
        <t:OriginalStart>2026-07-20T09:00:00Z</t:OriginalStart>
      </t:CalendarItem></m:Items>
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse></s:Body>
</s:Envelope>"#,
            )
            .create_async()
            .await;
        let del = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("DeleteType".into()),
                mockito::Matcher::Regex(r#"InstanceIndex="3""#.into()),
            ]))
            .with_status(200)
            .with_body(
                r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages">
  <s:Body><m:DeleteItemResponse><m:ResponseMessages>
    <m:DeleteItemResponseMessage ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>
    </m:DeleteItemResponseMessage>
  </m:ResponseMessages></m:DeleteItemResponse></s:Body>
</s:Envelope>"#,
            )
            .expect(1)
            .create_async()
            .await;

        let slot = "2026-07-20T09:00:00Z".parse().unwrap();
        delete_series_occurrence(&client_for(&server), "MASTER", Some("CK"), slot, false)
            .await
            .unwrap();
        del.assert_async().await;
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
        delete_event(&client_for(&server), "O:OCC-ID|OCK", false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rename_calendar_round_trips_against_server() {
        let mut server = Server::new_async().await;
        // The stable calendar id carries no ChangeKey, so rename first
        // does a FindFolder to harvest a fresh one, then UpdateFolder.
        let find_body = r#"<?xml version="1.0"?>
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
                <t:FolderId Id="FOLDER-ID" ChangeKey="FCK-V1"/>
                <t:DisplayName>Calendar</t:DisplayName>
              </t:CalendarFolder>
            </t:Folders>
          </m:RootFolder>
        </m:FindFolderResponseMessage>
      </m:ResponseMessages>
    </m:FindFolderResponse>
  </s:Body>
</s:Envelope>"#;
        let _find = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("FindFolder".to_string()))
            .with_status(200)
            .with_body(find_body)
            .create_async()
            .await;
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
            .match_body(mockito::Matcher::Regex("DisplayName".to_string()))
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        // Pass the new stable id form (folder EntryID only).
        rename_calendar(&client_for(&server), "FOLDER-ID", "Renamed")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn query_free_busy_sends_mailboxes_and_parses_busy_blocks() {
        let mut server = Server::new_async().await;
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>
    <m:GetUserAvailabilityResponse>
      <m:FreeBusyResponseArray>
        <m:FreeBusyResponse>
          <m:ResponseMessage ResponseClass="Success">
            <m:ResponseCode>NoError</m:ResponseCode>
          </m:ResponseMessage>
          <m:FreeBusyView>
            <t:FreeBusyViewType>Detailed</t:FreeBusyViewType>
            <t:CalendarEventArray>
              <t:CalendarEvent>
                <t:StartTime>2026-06-01T09:00:00</t:StartTime>
                <t:EndTime>2026-06-01T10:00:00</t:EndTime>
                <t:BusyType>Busy</t:BusyType>
              </t:CalendarEvent>
            </t:CalendarEventArray>
          </m:FreeBusyView>
        </m:FreeBusyResponse>
      </m:FreeBusyResponseArray>
    </m:GetUserAvailabilityResponse>
  </s:Body>
</s:Envelope>"#;
        let _m = server
            .mock("POST", "/")
            // Assert the request shape: a GetUserAvailability envelope
            // carrying the requested mailbox address.
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("GetUserAvailabilityRequest".to_string()),
                mockito::Matcher::Regex("bob@example.com".to_string()),
            ]))
            .with_status(200)
            .with_header("content-type", "text/xml; charset=utf-8")
            .with_body(body)
            .create_async()
            .await;
        let range = DateRange::new(
            "2026-06-01T00:00:00Z".parse().unwrap(),
            "2026-06-02T00:00:00Z".parse().unwrap(),
        );
        let fb = query_free_busy(&client_for(&server), &["bob@example.com"], range)
            .await
            .unwrap();
        assert_eq!(fb.len(), 1);
        assert_eq!(fb[0].email, "bob@example.com");
        assert_eq!(fb[0].slots.len(), 1);
        assert_eq!(
            fb[0].slots[0].start,
            "2026-06-01T09:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[tokio::test]
    async fn query_free_busy_empty_emails_short_circuits() {
        // No mailboxes → no round-trip, empty result. The mock with
        // expect(0) asserts we never hit the wire.
        let mut server = Server::new_async().await;
        let m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body("unused")
            .expect(0)
            .create_async()
            .await;
        let range = DateRange::new(
            "2026-06-01T00:00:00Z".parse().unwrap(),
            "2026-06-02T00:00:00Z".parse().unwrap(),
        );
        let fb = query_free_busy(&client_for(&server), &[], range)
            .await
            .unwrap();
        assert!(fb.is_empty());
        m.assert_async().await;
    }
}
