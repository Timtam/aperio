//! Calendar and event commands.

use cal_adapter_local::LocalAdapter;
use cal_core::{
    AttendeeStatus, Calendar, CalendarFeature, ColorLabelId, DateRange, Event, FreeBusy, NewEvent,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use sync_core::{EventPayload, IdPayload, SyncEvent};
use tauri::{AppHandle, State};

use super::cache_swr;
use super::cache_swr::TauriCacheObserver;
use crate::cache::{CacheObserver, CacheStore, RefreshCoordinator, SyncScope};

use plugin_core::{PluginManager, RecurrenceCapabilities};

use super::birthdays::{
    is_birthday_calendar_id, list_birthday_calendars, synthesise_birthday_events,
};
use super::plugins::plugin_id_for_adapter_kind;
use super::{CommandError, CommandResult};
use crate::accounts::{AccountsRepo, AdapterKind};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::overrides::{
    apply_color_to_calendars, apply_color_to_events, apply_to_calendars, OverridesRepo,
};
use crate::registry::{AdapterRegistry, LOCAL_ID};
use crate::reminders::SchedulerHandle;

/// Wire-format Calendar enriched with the owning account id. Lets
/// the frontend group containers by source without a second
/// round-trip to fetch the registry's route map.
///
/// `serde(flatten)` writes every Calendar field at the top level so
/// the existing TypeScript Calendar type only needs one new field
/// (`account_id`) to consume this shape.
#[derive(Debug, Serialize)]
pub struct CalendarRow {
    #[serde(flatten)]
    pub inner: Calendar,
    pub account_id: String,
    /// Recurrence shapes the owning adapter can store, resolved
    /// from the account's plugin manifest. The EventDialog greys
    /// out options this source can't round-trip (e.g. EWS has no
    /// yearly interval). Local + unknown sources report full
    /// RFC-5545 support via [`RecurrenceCapabilities::default`].
    pub recurrence_capabilities: RecurrenceCapabilities,
}

/// Resolve an account's recurrence capabilities from its plugin
/// manifest. Local calendars (`account_id == LOCAL_ID`) and any
/// account whose plugin we can't resolve fall back to full
/// RFC-5545 support — the host's own SQLite store has no
/// restrictions, and a missing manifest shouldn't silently strip
/// options the source might actually support.
fn recurrence_caps_for_account(
    account_id: &str,
    account_kinds: &std::collections::HashMap<String, AdapterKind>,
    plugin_manager: &PluginManager,
) -> RecurrenceCapabilities {
    let Some(kind) = account_kinds.get(account_id) else {
        return RecurrenceCapabilities::default();
    };
    let Some(plugin_id) = plugin_id_for_adapter_kind(*kind) else {
        // Local has no plugin — full support.
        return RecurrenceCapabilities::default();
    };
    plugin_manager
        .get_including_disabled(plugin_id)
        .map(|p| p.manifest.recurrence.clone())
        .unwrap_or_default()
}

/// Frontend-supplied payload for creating a local calendar.
#[derive(Debug, Deserialize)]
pub struct CreateCalendarRequest {
    pub name: String,
    /// Bind the new calendar's color to this color-label id (or `None`
    /// for no color). The rendered hex resolves from the label live.
    #[serde(default)]
    pub color_label: Option<String>,
}

#[tauri::command]
pub async fn list_calendars(
    app: AppHandle,
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    coord: State<'_, Arc<RefreshCoordinator>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<CalendarRow>> {
    tracing::info!(
        target: "aperio::commands",
        "list_calendars command invoked",
    );
    let registry = Arc::clone(&registry);
    let cache = Arc::clone(&cache);
    let coord = Arc::clone(&coord);
    // Local first so the implicit "local" account stays at the top
    // of the user's calendar list. Each local calendar id is
    // pre-registered in the route map so the write-path commands
    // can recognise it without falling back to the legacy "assume
    // local" branch.
    let local = adapter.list_calendars().await?;
    for c in &local {
        registry.note_calendar_route(&c.id, LOCAL_ID);
    }
    // External calendars: cache-first, background-refreshed (SWR) so the
    // sidebar isn't gated on the slowest provider at startup.
    let mut external = external_calendars_swr(&app, &registry, &cache, &coord).await;
    tracing::info!(
        target: "aperio::commands",
        local_count = local.len(),
        external_count = external.len(),
        "list_calendars aggregation",
    );
    let mut out = local;
    out.append(&mut external);
    // Stamp local rename overrides on top of whatever each adapter
    // returned. The adapter never sees the override so its
    // edit-path (where it exists) keeps writing into the source
    // server with the source name — the override is purely a
    // frontend-facing read-time projection.
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    apply_to_calendars(&repo, &mut out);
    // External calendars get their user-chosen color-label binding from
    // the override layer; local calendars already carry it on the row.
    apply_color_to_calendars(&repo, &mut out);
    // Synthesised birthday calendars (DESIGN.md §10.3) — one per
    // contact list that has at least one contact with a `birthday`
    // set. Each carries `read_only = true` so the UI hides edit /
    // delete affordances; rendered events flow through `get_events`
    // with the prefix-routed shim below. We stamp the route map
    // here too so subsequent `get_events` calls reach the right
    // contacts adapter via the same registry mechanism real
    // calendars use.
    let birthday_rows = list_birthday_calendars(&*adapter, &registry, &cache).await;
    for (cal, account_id) in &birthday_rows {
        registry.note_calendar_route(&cal.id, account_id);
    }

    // Snapshot account_id → adapter_kind once so the per-row caps
    // lookup is a cheap map hit rather than a SQL round-trip each.
    // A read failure degrades to "every account looks local" → full
    // RFC-5545 support, which is the safe permissive default.
    let account_kinds: std::collections::HashMap<String, AdapterKind> = AccountsRepo::new(&shared)
        .list()
        .map(|accounts| {
            accounts
                .into_iter()
                .map(|a| (a.id, a.adapter_kind))
                .collect()
        })
        .unwrap_or_default();

    // Decorate each row with its owning account id (from the
    // registry's route map) + the source's recurrence capabilities.
    // Local rows fall back to LOCAL_ID; external rows look themselves
    // up in the routes. The frontend uses account_id for the
    // account-grouped sidebar and recurrence_capabilities to grey out
    // unsupported options in the EventDialog — both stamped here so
    // neither needs a second round-trip.
    let mut decorated: Vec<CalendarRow> = out
        .into_iter()
        .map(|cal| {
            let account_id = registry
                .account_for_calendar(&cal.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            let recurrence_capabilities =
                recurrence_caps_for_account(&account_id, &account_kinds, &plugin_manager);
            CalendarRow {
                inner: cal,
                account_id,
                recurrence_capabilities,
            }
        })
        .collect();
    for (cal, account_id) in birthday_rows {
        // Birthday calendars are read-only synthetics — recurrence
        // editing never targets them, so the default (full) is moot
        // but kept consistent.
        let recurrence_capabilities =
            recurrence_caps_for_account(&account_id, &account_kinds, &plugin_manager);
        decorated.push(CalendarRow {
            inner: cal,
            account_id,
            recurrence_capabilities,
        });
    }
    Ok(decorated)
}

/// Cache-first aggregation of every external account's calendars. Serves
/// the snapshot instantly (registering routes so `get_events` can resolve
/// them), refreshing past the freshness window. Same stale-while-
/// revalidate shape as `external_task_lists_swr`.
async fn external_calendars_swr(
    app: &AppHandle,
    registry: &Arc<AdapterRegistry>,
    cache: &Arc<CacheStore>,
    coord: &Arc<RefreshCoordinator>,
) -> Vec<Calendar> {
    let mut out = Vec::new();
    for (account, adapter) in registry.snapshot_calendar_adapters() {
        let state = cache
            .get_sync_state(&account, SyncScope::Calendars, "")
            .ok()
            .flatten();
        // Cache-first and NEVER blocking — the catalog read must not await
        // the network at startup, or a single slow provider (a CalDAV
        // calendar-home PROPFIND, an EWS folder walk) gates the whole UI:
        // `list_calendars` feeds the frontend's `storeLoading`, which in
        // turn holds `useEvents`/`useTasks` from fetching anything. We serve
        // whatever the snapshot holds (empty on a true first run) and, when
        // it's missing or stale, spawn ONE deduplicated background refresh.
        // The refresh's write closure registers the routes and emits
        // `cache-updated`, which re-runs this listing (now warm) and
        // re-fetches items — so external calendars fill in a beat later
        // instead of blocking first paint. `is_stale` is true for a cold
        // snapshot (`last_refreshed_at` is None), so the first run refreshes
        // exactly once.
        let cached = cache.read_calendars(&account).unwrap_or_default();
        for c in &cached {
            registry.note_calendar_route(&c.id, &account);
        }
        if cache_swr::is_stale(&state, cache_swr::SWR_TTL_SECS) {
            let adapter_bg = Arc::clone(&adapter);
            let reg = Arc::clone(registry);
            let acc = account.clone();
            let rt = tauri::async_runtime::handle();
            cache_swr::spawn_refresh(
                rt.inner(),
                Arc::new(TauriCacheObserver { app: app.clone() }) as Arc<dyn CacheObserver>,
                Arc::clone(cache),
                Arc::clone(coord),
                SyncScope::Calendars,
                account.clone(),
                String::new(),
                move || async move { adapter_bg.list_calendars().await },
                move |c, cals: &[Calendar]| {
                    for cal in cals {
                        reg.note_calendar_route(&cal.id, &acc);
                    }
                    c.replace_calendars(&acc, cals)
                },
            );
        }
        out.extend(cached);
    }
    out
}

#[tauri::command]
pub async fn create_calendar(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateCalendarRequest,
) -> CommandResult<CalendarRow> {
    let color_label = request.color_label.map(cal_core::ColorLabelId);
    let cal = adapter.create_calendar(&request.name, None, color_label, None)?;
    // Local creates always belong to the implicit local account.
    // Mint a CalendarCreated event so other devices in the sync
    // mesh learn about the new container.
    if let Ok(fields) = serde_json::to_value(&cal) {
        event_log.append(SyncEvent::CalendarCreated(EventPayload {
            id: cal.id.clone(),
            fields,
        }));
    }
    Ok(CalendarRow {
        inner: cal,
        account_id: LOCAL_ID.to_string(),
        // Local calendars live in the host's own SQLite store, which
        // has no recurrence restrictions — full RFC-5545.
        recurrence_capabilities: RecurrenceCapabilities::default(),
    })
}

#[tauri::command]
pub async fn delete_calendar(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_calendar(&id)?;
    event_log.append(SyncEvent::CalendarDeleted(IdPayload { id: id.clone() }));
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct EventRangeRequest {
    pub calendar_id: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Whether the external calendar `calendar_id` (owned by `account`) stores a
/// per-event color natively (RFC 7986 `COLOR`). Read from the cached calendar
/// listing — which the sidebar populates before any event edit. An unknown /
/// uncached id degrades to `false` (the safe default: the color stays a
/// host-local override, matching Stage 1).
fn calendar_supports_event_color(cache: &CacheStore, account: &str, calendar_id: &str) -> bool {
    cache
        .read_calendars(account)
        .ok()
        .into_iter()
        .flatten()
        .find(|c| c.id == calendar_id)
        .map(|c| c.supports_event_color)
        .unwrap_or(false)
}

/// Resolve an event's `color_label` to the `#rrggbb` a color-capable provider
/// stores in `COLOR`. `None` (no label, or the label was deleted) clears the
/// provider color on the next write.
fn resolve_color_hex(adapter: &LocalAdapter, label: Option<&ColorLabelId>) -> Option<String> {
    adapter.resolve_label_to_hex(label?.as_str()).ok().flatten()
}

/// Map each event's native `color_hex` (set by a color-capable adapter from
/// the provider's `COLOR`) back to a `color_label`, in place. Runs *before*
/// the host-local override stamp so a user override still wins for a
/// non-capable provider (whose events never carry `color_hex`).
fn map_color_hex_to_labels(adapter: &LocalAdapter, events: &mut [Event]) {
    for ev in events.iter_mut() {
        let Some(hex) = ev.color_hex.as_deref() else {
            continue;
        };
        if let Ok(Some(label)) = adapter.match_hex_to_label(hex) {
            ev.color_label = Some(ColorLabelId::new(label));
        }
    }
}

#[tauri::command]
pub async fn get_events(
    app: AppHandle,
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    coord: State<'_, Arc<RefreshCoordinator>>,
    db: State<'_, DbHandle>,
    request: EventRangeRequest,
) -> CommandResult<Vec<Event>> {
    let range = DateRange::new(request.start, request.end);
    // Synthesised birthday calendars short-circuit the adapter
    // routing — they aren't backed by any provider and have no
    // events of their own. The `synthesise_birthday_events`
    // helper computes them on the fly from the underlying
    // contact list. Returns `None` when the id doesn't carry the
    // birthday prefix; in that case we fall through to the
    // regular adapter path.
    if is_birthday_calendar_id(&request.calendar_id) {
        if let Some(events) =
            synthesise_birthday_events(&*adapter, &registry, &cache, &request.calendar_id, range)
                .await
        {
            return Ok(events);
        }
        // Unknown synthesised id (e.g. the underlying contact
        // list has been removed since the listing). Surface an
        // empty list rather than 404 — the sidebar still has
        // the layer ticked, the next refresh will drop it.
        return Ok(Vec::new());
    }
    let account = registry
        .account_for_calendar(&request.calendar_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    tracing::info!(
        target: "aperio::commands",
        calendar_id = %request.calendar_id,
        account_id = %account,
        is_local = (account == LOCAL_ID),
        range_start = %range.start.to_rfc3339(),
        range_end = %range.end.to_rfc3339(),
        "get_events command invoked",
    );
    if account == LOCAL_ID {
        return Ok(adapter.get_events(&request.calendar_id, range).await?);
    }
    let Some(ext) = registry.calendar_adapter(&account) else {
        tracing::warn!(
            target: "aperio::commands",
            calendar_id = %request.calendar_id,
            account_id = %account,
            "get_events: calendar mapped to account but no CalendarFeature adapter registered",
        );
        return Err(CommandError {
            code: "not_found",
            message: format!("calendar '{}' is not routable", request.calendar_id),
        });
    };

    // Stale-while-revalidate. Read the sync state once, serve whatever the
    // snapshot holds for `range` immediately (covered or only partially —
    // `read_events` filters to the range either way, so the first paint never
    // blocks on the network), and queue a background refresh when the snapshot
    // is STALE *or* doesn't COVER the requested range.
    //
    // Refreshing on a coverage miss — not just staleness — stops events "going
    // missing" when the view moves to a date the cached window never reached
    // (issue #1). The whole decision, INCLUDING the cold-cache feedback-loop
    // cooldown, lives in the shared `cache_swr::event_self_warm_needed` so the
    // desktop command and the mobile cal-ffi host self-warm identically; see its
    // doc for the loop rationale. `covers`/`stale` here only feed the diagnostic
    // log below.
    let state = cache
        .get_sync_state(&account, SyncScope::Events, &request.calendar_id)
        .ok()
        .flatten();
    let covers = matches!(
        state.as_ref().map(|s| (s.window_start, s.window_end)),
        Some((Some(ws), Some(we))) if ws <= range.start && we >= range.end
    );
    let stale = cache_swr::is_stale(&state, cache_swr::SWR_TTL_SECS);
    let mut cached = cache
        .read_events(&account, &request.calendar_id, range)
        .unwrap_or_default();
    // Native per-event colors first: a color-capable provider (RFC 7986
    // COLOR) carries `color_hex` on the event — map it back to a color label.
    map_color_hex_to_labels(&adapter, &mut cached);
    // Then stamp host-local color overrides for external events whose provider
    // can't store a per-event color (iCloud etc.). `apply_color_to_events`
    // skips any event already carrying a native `color_hex`, so a leftover
    // Stage-1 override can never shadow a provider's native color here.
    apply_color_to_events(&OverridesRepo::new(&db.shared()), &mut cached);
    tracing::info!(
        target: "aperio::cache",
        calendar_id = %request.calendar_id,
        count = cached.len(),
        covers,
        stale,
        "get_events served from cache",
    );
    if cache_swr::event_self_warm_needed(&state, range) {
        let ext_bg = Arc::clone(&ext);
        let cache_bg = Arc::clone(&cache);
        let acc = account.clone();
        let cal = request.calendar_id.clone();
        let rt = tauri::async_runtime::handle();
        cache_swr::spawn_item_refresh(
            rt.inner(),
            Arc::new(TauriCacheObserver { app: app.clone() }) as Arc<dyn CacheObserver>,
            Arc::clone(&cache),
            Arc::clone(&coord),
            SyncScope::Events,
            account.clone(),
            request.calendar_id.clone(),
            move || async move {
                cache_swr::refresh_events(&cache_bg, ext_bg.as_ref(), &acc, &cal, range).await
            },
        );
    }
    Ok(cached)
}

#[derive(Debug, Deserialize)]
pub struct CreateEventRequest {
    pub calendar_id: String,
    #[serde(flatten)]
    pub event: NewEvent,
}

#[tauri::command]
pub async fn create_event(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateEventRequest,
) -> CommandResult<Event> {
    let account = registry
        .account_for_calendar(&request.calendar_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    let is_local = account == LOCAL_ID;
    let event = if is_local {
        adapter
            .create_event(&request.calendar_id, request.event)
            .await?
    } else {
        let Some(ext) = registry.calendar_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("calendar '{}' is not routable", request.calendar_id),
            });
        };
        let mut new_event = request.event;
        // Color-capable provider: resolve the label to a hex so the adapter
        // writes a native RFC 7986 COLOR. Non-capable providers keep the
        // color as a host-local override (set separately via set_event_color).
        if calendar_supports_event_color(&cache, &account, &request.calendar_id) {
            new_event.color_hex = resolve_color_hex(&adapter, new_event.color_label.as_ref());
        }
        ext.create_event(&request.calendar_id, new_event).await?
    };
    // Only LOCAL events flow through the event log — external
    // adapters (Google, iCloud, EWS, Graph) handle their own
    // multi-device sync via the respective provider APIs, see
    // DESIGN.md §19.12. Pushing those through the event log too
    // would race against the provider's authoritative source.
    if is_local {
        if let Ok(fields) = serde_json::to_value(&event) {
            event_log.append(SyncEvent::EventCreated(EventPayload {
                id: event.id.clone(),
                fields,
            }));
        }
    } else {
        let _ = cache.invalidate(&account, SyncScope::Events, &request.calendar_id);
    }
    scheduler.invalidate();
    Ok(event)
}

#[tauri::command]
pub async fn update_event(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    db: State<'_, DbHandle>,
    event: Event,
    previous_calendar_id: Option<String>,
) -> CommandResult<Event> {
    let target_account = registry
        .account_for_calendar(&event.calendar_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());

    // Cross-calendar move detection. When the frontend captured the
    // event's *original* calendar_id on dialog open and passes it
    // through here, we can tell that the save also moves the event
    // — the EventDialog's calendar picker doubles as a move
    // gesture, in addition to the dedicated MoveCopyDialog.
    //
    // Without this detection, a save against a different calendar
    // would PUT to a resource that doesn't exist on the target,
    // carrying the old calendar's ETag in If-Match. iCloud rejects
    // that with 412 because the precondition can never be met for
    // a non-existent target resource — the user sees "Conflict"
    // and the move silently fails.
    let is_move = previous_calendar_id
        .as_deref()
        .map(|prev| prev != event.calendar_id)
        .unwrap_or(false);

    if is_move {
        let previous = previous_calendar_id.expect("checked above");
        let source_account = registry
            .account_for_calendar(&previous)
            .unwrap_or_else(|| LOCAL_ID.to_string());

        // Local ↔ Local moves go through `update_event` directly:
        // the LocalAdapter handles the calendar_id change as a
        // single SQL UPDATE without resource-URL gymnastics, so
        // there's nothing to gain from a two-call dance here.
        if source_account == LOCAL_ID && target_account == LOCAL_ID {
            let updated = adapter.update_event(event).await?;
            // Local↔Local move surfaces as a single UPDATE on the
            // calendar_id column — emit one EventUpdated event
            // carrying the full row.
            if let Ok(fields) = serde_json::to_value(&updated) {
                event_log.append(SyncEvent::EventUpdated(EventPayload {
                    id: updated.id.clone(),
                    fields,
                }));
            }
            scheduler.invalidate();
            return Ok(updated);
        }

        // Cross-calendar move involving at least one external
        // adapter (two iCloud calendars, iCloud → Google,
        // Local → CalDAV, etc.) all reduce to the same shape:
        // create on the target, then delete from the source. We
        // create FIRST so a half-failed move can never lose data
        // — if the create succeeds and the delete fails, the user
        // sees a duplicate they can resolve manually rather than
        // an empty calendar where their event used to live.
        let mut new_payload = NewEvent {
            // A cross-calendar move re-creates the event at the target; the
            // organizer-notify intent isn't carried through this path (a
            // dedicated "notify on move" decision is future work, DESIGN §7.5).
            send_invitations: false,
            title: event.title.clone(),
            description: event.description.clone(),
            location: event.location.clone(),
            start: event.start,
            end: event.end,
            all_day: event.all_day,
            recurrence: event.recurrence.clone(),
            color_label: event.color_label.clone(),
            // Carry the native color hex across the move; the target adapter
            // emits/strips it per its own capability (iCloud clears it).
            color_hex: event.color_hex.clone(),
            reminders: event.reminders.clone(),
            sound: event.sound.clone(),
            attendees: event.attendees.clone(),
        };
        // Preserve the color when moving INTO a color-capable provider:
        // resolve the label to a hex so the target stores it natively (the
        // incoming event carries `color_label`, not `color_hex`).
        if target_account != LOCAL_ID
            && calendar_supports_event_color(&cache, &target_account, &event.calendar_id)
        {
            new_payload.color_hex = resolve_color_hex(&adapter, new_payload.color_label.as_ref());
        }

        let created = if target_account == LOCAL_ID {
            adapter
                .create_event(&event.calendar_id, new_payload)
                .await?
        } else {
            let Some(ext) = registry.calendar_adapter(&target_account) else {
                return Err(CommandError {
                    code: "not_found",
                    message: format!("target calendar '{}' is not routable", event.calendar_id,),
                });
            };
            ext.create_event(&event.calendar_id, new_payload).await?
        };

        // Delete from source. We log on failure rather than
        // bubbling — the create already succeeded, the event
        // exists at the target. Bubbling here would make the
        // command return Err even though the move is partially
        // through, and the user might retry, doubling the
        // duplicate. A warning + best-effort cleanup is the
        // less-bad failure mode.
        let delete_result = if source_account == LOCAL_ID {
            adapter
                // A cross-calendar move is not a cancellation — the event
                // still exists at the target, so never email attendees here.
                .delete_event(&event.id, false)
                .await
                .map_err(CommandError::from)
        } else if let Some(ext) = registry.calendar_adapter(&source_account) {
            ext.delete_event(&event.id, false)
                .await
                .map_err(CommandError::from)
        } else {
            // Source isn't routable (account was removed between
            // the dialog opening and save). Treat as a "no
            // cleanup needed" — the create on the target stands.
            Ok(())
        };
        if let Err(err) = delete_result {
            tracing::warn!(
                event_id = %event.id,
                source = %previous,
                target = %event.calendar_id,
                code = %err.code,
                message = %err.message,
                "delete from source calendar failed after move; duplicate may exist",
            );
        }

        // Sync-event emission for cross-adapter moves. A move is
        // create-on-target + delete-from-source under the hood,
        // and the event log records the same shape: each side
        // emits its own event IF the side is local. External-
        // adapter sides stay silent (the provider's own sync
        // mesh propagates the change).
        if target_account == LOCAL_ID {
            if let Ok(fields) = serde_json::to_value(&created) {
                event_log.append(SyncEvent::EventCreated(EventPayload {
                    id: created.id.clone(),
                    fields,
                }));
            }
        }
        if source_account == LOCAL_ID {
            event_log.append(SyncEvent::EventDeleted(IdPayload {
                id: event.id.clone(),
            }));
        }

        // Write-through: re-fetch both ends of an external move.
        if target_account != LOCAL_ID {
            let _ = cache.invalidate(&target_account, SyncScope::Events, &event.calendar_id);
        }
        if source_account != LOCAL_ID {
            let _ = cache.invalidate(&source_account, SyncScope::Events, &previous);
        }

        scheduler.invalidate();
        return Ok(created);
    }

    // Plain in-place update — no calendar change, the existing
    // single-PUT/SQL-UPDATE path handles it.
    let is_local = target_account == LOCAL_ID;
    let updated = if is_local {
        adapter.update_event(event).await?
    } else {
        let Some(ext) = registry.calendar_adapter(&target_account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("calendar '{}' is not routable", event.calendar_id),
            });
        };
        let capable = calendar_supports_event_color(&cache, &target_account, &event.calendar_id);
        let mut event = event;
        if capable {
            // Color-capable provider: resolve the label to a hex so the
            // adapter writes (or clears) the native RFC 7986 COLOR.
            event.color_hex = resolve_color_hex(&adapter, event.color_label.as_ref());
        }
        let updated = ext.update_event(event).await?;
        if capable {
            // The provider's COLOR is now authoritative — drop any host-local
            // override left over from Stage 1 (when this calendar was treated
            // as non-capable) so it can't shadow the native color on read.
            let shared = db.shared();
            let repo = OverridesRepo::new(&shared);
            if let Err(err) = repo.clear_event_color_label(&updated.id) {
                tracing::warn!(?err, event_id = %updated.id, "clearing stale event color override failed; non-fatal");
            }
        }
        updated
    };
    if is_local {
        if let Ok(fields) = serde_json::to_value(&updated) {
            event_log.append(SyncEvent::EventUpdated(EventPayload {
                id: updated.id.clone(),
                fields,
            }));
        }
    } else {
        let _ = cache.invalidate(&target_account, SyncScope::Events, &updated.calendar_id);
    }
    scheduler.invalidate();
    Ok(updated)
}

#[tauri::command]
pub async fn delete_event(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
    calendar_id: Option<String>,
    send_cancellations: Option<bool>,
) -> CommandResult<()> {
    // The frontend now passes the parent calendar_id so the registry
    // can route the delete to the right adapter. Older callers
    // (pre-6b.4) that only sent `id` are still served by the local
    // adapter — its own data model can locate the row by uid alone.
    let account = calendar_id
        .as_deref()
        .and_then(|cid| registry.account_for_calendar(cid))
        .unwrap_or_else(|| LOCAL_ID.to_string());
    // Organizer-side cancellation: only meaningful for external,
    // scheduling-capable calendars (the frontend gates it on
    // `supports_scheduling`); defaults off when the caller omits it.
    let send_cancellations = send_cancellations.unwrap_or(false);
    let is_local = account == LOCAL_ID;
    if is_local {
        adapter.delete_event(&id, false).await?;
    } else {
        let Some(ext) = registry.calendar_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("account '{account}' is not routable"),
            });
        };
        ext.delete_event(&id, send_cancellations).await?;
        if let Some(cid) = &calendar_id {
            let _ = cache.invalidate(&account, SyncScope::Events, cid);
        }
    }
    if is_local {
        event_log.append(SyncEvent::EventDeleted(IdPayload { id: id.clone() }));
    }
    scheduler.invalidate();
    Ok(())
}

/// Frontend payload for a free/busy query: which calendar's account to ask
/// through, the attendee emails to look up, and the window.
#[derive(Debug, Deserialize)]
pub struct FreeBusyRequest {
    pub calendar_id: String,
    pub emails: Vec<String>,
    pub range_start: DateTime<Utc>,
    pub range_end: DateTime<Utc>,
}

/// Query attendee availability through the account that owns `calendar_id`.
/// Best-effort: local/iCal calendars and any provider error or missing
/// permission degrade to an empty result (the UI reads that as "couldn't
/// determine", never an error) so the dialog never blocks on it.
#[tauri::command]
pub async fn query_free_busy(
    registry: State<'_, Arc<AdapterRegistry>>,
    request: FreeBusyRequest,
) -> CommandResult<Vec<FreeBusy>> {
    let account = registry
        .account_for_calendar(&request.calendar_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(Vec::new());
    }
    let Some(ext) = registry.calendar_adapter(&account) else {
        return Ok(Vec::new());
    };
    let range = DateRange::new(request.range_start, request.range_end);
    let refs: Vec<&str> = request.emails.iter().map(|s| s.as_str()).collect();
    match ext.get_free_busy(&refs, range).await {
        Ok(fb) => Ok(fb),
        Err(err) => {
            tracing::warn!(
                target: "aperio::commands",
                account = %account,
                ?err,
                "free/busy query failed; returning empty",
            );
            Ok(Vec::new())
        }
    }
}

/// The connected account's email for `calendar_id`, used by the RSVP
/// affordance to decide whether the user is an *attendee* (not the
/// organizer) of a meeting. Local/iCal calendars and any provider that
/// can't report an identity return `None`, which hides the RSVP buttons.
#[tauri::command]
pub async fn calendar_current_user_email(
    registry: State<'_, Arc<AdapterRegistry>>,
    calendar_id: String,
) -> CommandResult<Option<String>> {
    let account = registry
        .account_for_calendar(&calendar_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(None);
    }
    let Some(ext) = registry.calendar_adapter(&account) else {
        return Ok(None);
    };
    Ok(ext.current_user_email().await.unwrap_or(None))
}

/// RSVP to an invitation: set the connected user's participation status
/// on `event_id`. When `send_response` is true the provider also emails
/// the reply to the organizer. Invalidates the calendar's event cache so
/// the next read reflects the new status. Local/iCal calendars and
/// unroutable accounts return a not-found error (the UI only offers RSVP
/// on scheduling-capable, non-organizer meetings).
#[tauri::command]
pub async fn respond_to_event(
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    calendar_id: String,
    event_id: String,
    status: AttendeeStatus,
    send_response: bool,
) -> CommandResult<()> {
    let account = registry
        .account_for_calendar(&calendar_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Err(CommandError {
            code: "unsupported",
            message: "RSVP is only available on external calendar accounts".into(),
        });
    }
    let Some(ext) = registry.calendar_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("account '{account}' is not routable"),
        });
    };
    ext.respond_to_event(&event_id, status, send_response)
        .await?;
    let _ = cache.invalidate(&account, SyncScope::Events, &calendar_id);
    Ok(())
}

#[tauri::command]
pub async fn get_event_by_id(
    adapter: State<'_, LocalAdapter>,
    id: String,
) -> CommandResult<Option<Event>> {
    Ok(adapter.get_event_by_id(&id)?)
}

#[tauri::command]
pub async fn add_event_exdate(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
    occurrence: DateTime<Utc>,
    calendar_id: Option<String>,
) -> CommandResult<()> {
    // `calendar_id` was added in Phase 6b.7 — older callers that
    // only pass `id` fall back to "assume local", which is still
    // right for local-only events but would have wrongly hit the
    // local adapter when the event lived on iCloud / Nextcloud.
    let account = calendar_id
        .as_deref()
        .and_then(|cid| registry.account_for_calendar(cid))
        .unwrap_or_else(|| LOCAL_ID.to_string());
    let is_local = account == LOCAL_ID;
    if is_local {
        adapter.add_event_exdate(&id, occurrence)?;
    } else {
        let Some(ext) = registry.calendar_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("account '{account}' is not routable"),
            });
        };
        ext.add_event_exdate(&id, occurrence).await?;
        if let Some(cid) = &calendar_id {
            let _ = cache.invalidate(&account, SyncScope::Events, cid);
        }
    }
    // For local events the exdate mutation rewrote the master
    // event's recurrence.exceptions list. Re-read the row so the
    // event log carries the new state. Cheap — single SQL row
    // fetch — and the alternative (id-only payload) would force
    // the applier to do the same read against its local DB.
    if is_local {
        if let Ok(Some(refreshed)) = adapter.get_event_by_id(&id) {
            if let Ok(fields) = serde_json::to_value(&refreshed) {
                event_log.append(SyncEvent::EventUpdated(EventPayload {
                    id: id.clone(),
                    fields,
                }));
            }
        }
    }
    scheduler.invalidate();
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Smoke test: round-trip a calendar + event through the command layer
// using an in-memory adapter. Mirrors what the frontend will do at startup.
// ────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandError;
    use cal_adapter_local::LocalAdapter;
    use chrono::Duration;

    fn fresh_adapter() -> LocalAdapter {
        // Use the shared test schema so it tracks later migrations (e.g.
        // the container color-label columns from 0022) without each test
        // helper having to re-list every CREATE/ALTER.
        LocalAdapter::new(cal_adapter_local::test_support::open_test_db())
    }

    #[tokio::test]
    async fn create_calendar_then_event() {
        let a = fresh_adapter();
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let now = Utc::now();
        let ev = a
            .create_event(
                &cal.id,
                NewEvent {
                    title: "Standup".into(),
                    description: None,
                    location: None,
                    start: now,
                    end: now + Duration::minutes(15),
                    all_day: false,
                    recurrence: None,
                    color_label: None,
                    color_hex: None,
                    reminders: vec![],
                    sound: None,
                    attendees: vec![],
                    send_invitations: false,
                },
            )
            .await
            .unwrap();

        // The CommandError mapping must preserve the original error code.
        let err: CommandError = cal_core::Error::NotFound("x".into()).into();
        assert_eq!(err.code, "not_found");

        let cals = a.list_calendars().await.unwrap();
        assert_eq!(cals[0].id, cal.id);
        let evs = a
            .get_events(
                &cal.id,
                DateRange::new(now - Duration::minutes(1), now + Duration::hours(1)),
            )
            .await
            .unwrap();
        assert_eq!(evs[0].id, ev.id);
    }
}
