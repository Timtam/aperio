//! Birthday calendars (DESIGN.md §10.3) — the desktop orchestration.
//!
//! One synthesised, read-only calendar per contact list, populated from the
//! contacts' `birthday` field, surfaced through the same `list_calendars` /
//! `get_events` paths the real calendars use, so every view picks them up.
//!
//! The PURE synthesis (id helpers, `synthesise_calendar`, `events_for_contacts`,
//! the age description, the tests) lives in the shared, Tauri-free
//! [`host_core::birthdays`] so the mobile cal-ffi Host produces identical
//! calendars + events. What stays HERE is the orchestration that needs the host
//! snapshot cache + the adapter registry:
//!
//!   - **No persistence.** Events are computed on each `get_events` from the
//!     live contacts — editing a birthday is reflected on the next render.
//!   - **External contacts come from the snapshot CACHE, never the adapter** —
//!     a birthday layer must never trigger a network fetch from inside
//!     `list_calendars` (the EWS GAL alone would block for tens of seconds).
//!   - Contact lists with zero birthdays produce no calendar at all.

use cal_adapter_local::LocalAdapter;
use cal_core::{Calendar, ContactsFeature, DateRange, Event};
use std::sync::Arc;

use crate::cache::CacheStore;
use crate::registry::{AdapterRegistry, LOCAL_ID};

// Re-export the prefix check so the existing `super::birthdays::*` references
// (calendars.rs routes birthday ids on it) keep resolving after the extraction.
pub use host_core::birthdays::is_birthday_calendar_id;
use host_core::birthdays::{events_for_contacts, synthesise_calendar, underlying_contact_list_id};

/// Walk every contact list (local + registered external adapters) and emit a
/// synthetic birthday calendar for each one that has at least one contact with
/// a birthday set. Returns `(calendar, account_id)` pairs so the caller can
/// stamp the registry's account-routing alongside the listing.
///
/// External contacts are read from the host SNAPSHOT CACHE, never the adapter
/// (see the module docs) — a birthday layer reflects whatever is cached and
/// updates on the next listing once a contacts background refresh repopulates.
pub async fn list_birthday_calendars(
    local: &LocalAdapter,
    registry: &Arc<AdapterRegistry>,
    cache: &Arc<CacheStore>,
) -> Vec<(Calendar, String)> {
    let mut out = Vec::new();

    // Local adapter — in-process SQLite, cheap to read directly.
    match local.list_contact_lists().await {
        Ok(lists) => {
            for list in lists {
                if list_has_birthdays(local, &list.id).await {
                    out.push((
                        synthesise_calendar(&list.id, &list.name),
                        LOCAL_ID.to_string(),
                    ));
                }
            }
        }
        Err(err) => {
            tracing::warn!(?err, "birthday: local list_contact_lists failed");
        }
    }

    // External contact accounts — read the cached lists + contacts only.
    for (account_id, _adapter) in registry.snapshot_contact_adapters() {
        let lists = cache.read_contact_lists(&account_id).unwrap_or_default();
        for list in lists {
            let contacts = cache
                .read_contacts(&account_id, &list.id)
                .unwrap_or_default();
            if contacts.iter().any(|c| c.birthday.is_some()) {
                out.push((
                    synthesise_calendar(&list.id, &list.name),
                    account_id.clone(),
                ));
            }
        }
    }
    out
}

/// Compute the birthday events that fall within `range` for the underlying
/// contact list of `synthesised_calendar_id`. Routing happens via the prefix —
/// anything that isn't a birthday calendar id returns `None` (the caller falls
/// back to the regular adapter path).
pub async fn synthesise_birthday_events(
    local: &LocalAdapter,
    registry: &Arc<AdapterRegistry>,
    cache: &Arc<CacheStore>,
    synthesised_calendar_id: &str,
    range: DateRange,
) -> Option<Vec<Event>> {
    let list_id = underlying_contact_list_id(synthesised_calendar_id)?;
    // The list id alone tells us where to look (ids are unique across local +
    // external adapters). Try the local adapter first (in-process), then the
    // host snapshot CACHE for external books — never the adapter, so rendering
    // a birthday layer can't block on a network contact fetch.
    if let Ok(contacts) = local.get_contacts(list_id).await {
        if !contacts.is_empty() {
            return Some(events_for_contacts(
                contacts,
                synthesised_calendar_id,
                range,
            ));
        }
    }
    // Resolve the owning account via the route map; fall back to scanning every
    // contact account's cache if the route isn't registered yet.
    let accounts: Vec<String> = registry
        .account_for_contact_list(list_id)
        .map(|a| vec![a])
        .unwrap_or_else(|| {
            registry
                .snapshot_contact_adapters()
                .into_iter()
                .map(|(account_id, _adapter)| account_id)
                .collect()
        });
    for account_id in accounts {
        let contacts = cache
            .read_contacts(&account_id, list_id)
            .unwrap_or_default();
        if !contacts.is_empty() {
            return Some(events_for_contacts(
                contacts,
                synthesised_calendar_id,
                range,
            ));
        }
    }
    // List exists but has no cached contacts. Empty Vec rather than None so the
    // caller treats this as "successful read with zero results".
    Some(Vec::new())
}

async fn list_has_birthdays(adapter: &LocalAdapter, list_id: &str) -> bool {
    match adapter.get_contacts(list_id).await {
        Ok(contacts) => contacts.iter().any(|c| c.birthday.is_some()),
        Err(_) => false,
    }
}
