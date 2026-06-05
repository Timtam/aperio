//! Task list and task commands.

use cal_adapter_local::LocalAdapter;
use cal_core::{
    MemberRight, NewTask, Section, Task, TaskList, TaskListShare, TaskUser, TasksFeature,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use sync_core::{EventPayload, IdPayload, SyncEvent};
use tauri::{AppHandle, State};

use plugin_core::{PluginManager, TaskCapabilities};

use super::cache_swr;
use super::plugins::plugin_id_for_adapter_kind;
use super::{CommandError, CommandResult};
use crate::accounts::{AccountsRepo, AdapterKind};
use crate::cache::{CacheStore, RefreshCoordinator, SyncScope};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::overrides::{apply_color_to_task_lists, apply_to_task_lists, OverridesRepo};
use crate::registry::{AdapterRegistry, LOCAL_ID};
use crate::reminders::SchedulerHandle;

/// One-shot pref marker recording that we've replayed the existing
/// LOCAL task lists + tasks as `TaskListCreated`/`TaskCreated`
/// events. Needed because of the `boot_at` writer bug: local
/// lists/tasks created while it was live DID emit events, but the
/// writer's session file was deleted out from under it mid-session,
/// so those emits never reached `pending/` (and thus never the
/// remote). On the first boot after the fix we walk the local store
/// once and re-emit so they finally propagate.
///
/// `.v2`: the first fix (sharing one `boot_at`) was incomplete — the
/// session filename is second-granular while `boot_at` carried
/// sub-seconds, so the stub-cleanup still reaped the live file and ate
/// events created AFTER the v1 backfill ran. Bumping the key forces one
/// more replay now that the comparison is second-granular, recovering
/// those lost tasks. Idempotent (receivers dedupe via
/// `sync_applied_events`).
const PREF_LOCAL_TASKS_BACKFILLED: &str = "sync.localTasks.eventBackfillDone.v2";

/// Catch-up emit for local task lists + their tasks. Idempotent:
/// gated by [`PREF_LOCAL_TASKS_BACKFILLED`], and receivers dedupe
/// via the applier's `sync_applied_events` table, so a list/task
/// that already made it across is a no-op on the peer. Best-effort —
/// any read failure logs a `warn!` and leaves the pref unset so the
/// next boot retries; a backfill hiccup never blocks startup.
///
/// Emits the exact same `EventPayload` shape (`serde_json::to_value`
/// of the row) that the live `create_task_list` / `create_task`
/// command paths produce, so the applier upserts identically.
pub fn backfill_local_task_events(db: &DbHandle, event_log: &EventLogWriter) {
    let shared = db.shared();
    let prefs = crate::user_prefs::UserPrefsRepo::new(&shared);
    match prefs.get(PREF_LOCAL_TASKS_BACKFILLED) {
        Ok(Some(v)) if v == "true" => return,
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(?err, "local-task backfill: prefs read failed");
            return;
        }
    }

    // `list_task_lists` / `get_tasks` are async trait methods backed
    // by synchronous SQLite. `run()` isn't itself inside an async
    // context, so driving them with block_on on the calling thread is
    // safe (same pattern as the app-exit push hook).
    let adapter = LocalAdapter::new(db.shared());
    let collected: cal_core::Result<Vec<SyncEvent>> = tauri::async_runtime::block_on(async move {
        let lists = adapter.list_task_lists().await?;
        let mut out: Vec<SyncEvent> = Vec::new();
        for list in &lists {
            if let Ok(fields) = serde_json::to_value(list) {
                out.push(SyncEvent::TaskListCreated(EventPayload {
                    id: list.id.clone(),
                    fields,
                }));
            }
            let tasks = adapter.get_tasks(&list.id).await?;
            for task in &tasks {
                if let Ok(fields) = serde_json::to_value(task) {
                    out.push(SyncEvent::TaskCreated(EventPayload {
                        id: task.id.clone(),
                        fields,
                    }));
                }
            }
        }
        Ok(out)
    });

    let events = match collected {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(?err, "local-task backfill: enumeration failed");
            return;
        }
    };

    let lists_n = events
        .iter()
        .filter(|e| matches!(e, SyncEvent::TaskListCreated(_)))
        .count();
    let tasks_n = events.len() - lists_n;
    for ev in events {
        event_log.append(ev);
    }
    if let Err(err) = prefs.set(PREF_LOCAL_TASKS_BACKFILLED, "true") {
        tracing::warn!(?err, "local-task backfill: pref write failed");
        return;
    }
    tracing::info!(
        lists = lists_n,
        tasks = tasks_n,
        "local-task backfill: replayed existing local lists + tasks",
    );
}

/// Wire-format TaskList enriched with the owning account id. Same
/// shape + rationale as `CalendarRow` — the frontend uses it to
/// group containers by source for the account-aware sidebar.
///
/// `inner` is `serde(flatten)`ed, so `TaskList.parent_id` rides along
/// at the top level for free — the nested-project tree in the sidebar
/// reads it without a second round-trip.
#[derive(Debug, Serialize)]
pub struct TaskListRow {
    #[serde(flatten)]
    pub inner: TaskList,
    pub account_id: String,
    /// Task-organisation shapes the owning adapter supports (nested
    /// projects, sections, subtask depth, …), resolved from the
    /// account's plugin manifest. The frontend gates affordances on
    /// these — e.g. only shows "add section" where `sections` is true.
    /// Local + unknown sources report [`TaskCapabilities::default`].
    pub task_capabilities: TaskCapabilities,
}

/// The local SQLite store's task capabilities. Unlike a plugin-backed
/// account it has no manifest, so we hard-code what the store actually
/// supports: it nests projects (`task_lists.parent_id`) and groups
/// tasks into sections, on top of the cal-core-native subtasks /
/// recurrence / cross-list-move support the default already carries.
fn local_task_capabilities() -> TaskCapabilities {
    TaskCapabilities {
        nested_projects: true,
        sections: true,
        create_lists: true,
        delete_lists: true,
        ..TaskCapabilities::default()
    }
}

/// Resolve an account's task capabilities from its plugin manifest.
/// Mirrors `recurrence_caps_for_account` in `calendars.rs`: the local
/// store reports its own capabilities; accounts whose plugin we can't
/// resolve fall back to the permissive cal-core-native default.
fn task_caps_for_account(
    account_id: &str,
    account_kinds: &std::collections::HashMap<String, AdapterKind>,
    plugin_manager: &PluginManager,
) -> TaskCapabilities {
    if account_id == LOCAL_ID {
        return local_task_capabilities();
    }
    let Some(kind) = account_kinds.get(account_id) else {
        return TaskCapabilities::default();
    };
    let Some(plugin_id) = plugin_id_for_adapter_kind(*kind) else {
        // No plugin for this kind — default capabilities.
        return TaskCapabilities::default();
    };
    plugin_manager
        .get_including_disabled(plugin_id)
        .map(|p| p.manifest.tasks.clone())
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskListRequest {
    pub name: String,
    /// Which account to create the list in. `None` / `"local"` ⇒ the
    /// local SQLite store; otherwise routed to that account's adapter
    /// (gated UI-side on its `create_lists` capability).
    #[serde(default)]
    pub account_id: Option<String>,
    /// Optional parent list for nesting (Vikunja / Todoist). Ignored by
    /// flat backends and the local create path.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Local-only: embed the list in a calendar (CalDAV-VTODO style).
    pub embedded_in_calendar: Option<String>,
    /// Local-only: bind the new list's color to this color-label id.
    /// External providers don't carry a color at create; their binding
    /// is set afterwards via the color override.
    #[serde(default)]
    pub color_label: Option<String>,
}

#[tauri::command]
pub async fn list_task_lists(
    app: AppHandle,
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    coord: State<'_, Arc<RefreshCoordinator>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<TaskListRow>> {
    let registry = Arc::clone(&registry);
    let cache = Arc::clone(&cache);
    let coord = Arc::clone(&coord);
    let local = adapter.list_task_lists().await?;
    for l in &local {
        registry.note_task_list_route(&l.id, LOCAL_ID);
    }
    // External task lists: serve from the snapshot cache, refresh in the
    // background (stale-while-revalidate) so the sidebar isn't gated on
    // the slowest provider at startup.
    let mut external = external_task_lists_swr(&app, &registry, &cache, &coord).await;
    let mut out = local;
    out.append(&mut external);
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    apply_to_task_lists(&repo, &mut out);
    apply_color_to_task_lists(&repo, &mut out);
    // Snapshot account_id → adapter_kind once so the per-row caps
    // lookup is a cheap map hit. Same permissive-on-failure default
    // as the calendar path: a read failure degrades to "every account
    // looks local" → full cal-core-native capabilities.
    let account_kinds: std::collections::HashMap<String, AdapterKind> = AccountsRepo::new(&shared)
        .list()
        .map(|accounts| {
            accounts
                .into_iter()
                .map(|a| (a.id, a.adapter_kind))
                .collect()
        })
        .unwrap_or_default();
    Ok(out
        .into_iter()
        .map(|list| {
            let account_id = registry
                .account_for_task_list(&list.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            let task_capabilities =
                task_caps_for_account(&account_id, &account_kinds, &plugin_manager);
            TaskListRow {
                inner: list,
                account_id,
                task_capabilities,
            }
        })
        .collect())
}

/// Cache-first aggregation of every external account's task lists. Serves
/// the snapshot instantly (registering routes so `get_tasks` can resolve
/// them), refreshing past the freshness window. On a cold miss it fetches
/// synchronously and writes through; on a network error it falls back to
/// any stale snapshot.
async fn external_task_lists_swr(
    app: &AppHandle,
    registry: &Arc<AdapterRegistry>,
    cache: &Arc<CacheStore>,
    coord: &Arc<RefreshCoordinator>,
) -> Vec<TaskList> {
    let mut out = Vec::new();
    for (account, adapter) in registry.snapshot_task_adapters() {
        let state = cache
            .get_sync_state(&account, SyncScope::TaskLists, "")
            .ok()
            .flatten();
        // Cache-first, never blocking — see `external_calendars_swr` for
        // the rationale (a slow provider's catalog enumeration must not gate
        // `storeLoading` and therefore the whole UI at startup). Serve the
        // snapshot (empty on first run) and spawn a deduplicated background
        // refresh when missing or stale; `cache-updated` re-runs this listing
        // and re-fetches items once it lands.
        let cached = cache.read_task_lists(&account).unwrap_or_default();
        for l in &cached {
            registry.note_task_list_route(&l.id, &account);
        }
        if cache_swr::is_stale(&state, cache_swr::SWR_TTL_SECS) {
            let adapter_bg = Arc::clone(&adapter);
            let reg = Arc::clone(registry);
            let acc = account.clone();
            cache_swr::spawn_refresh(
                app.clone(),
                Arc::clone(cache),
                Arc::clone(coord),
                SyncScope::TaskLists,
                account.clone(),
                String::new(),
                move || async move { adapter_bg.list_task_lists().await },
                move |c, lists: &[TaskList]| {
                    for l in lists {
                        reg.note_task_list_route(&l.id, &acc);
                    }
                    c.replace_task_lists(&acc, lists)
                },
            );
        }
        out.extend(cached);
    }
    out
}

#[tauri::command]
pub async fn create_task_list(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateTaskListRequest,
) -> CommandResult<TaskListRow> {
    let account = request
        .account_id
        .clone()
        .unwrap_or_else(|| LOCAL_ID.to_string());

    // Local SQLite store — the typed adapter handle keeps the richer
    // create signature (embedded_in_calendar) and emits a sync event.
    if account == LOCAL_ID {
        let color_label = request.color_label.clone().map(cal_core::ColorLabelId);
        let list = adapter.create_task_list(
            &request.name,
            None,
            color_label,
            None,
            request.embedded_in_calendar,
        )?;
        if let Ok(fields) = serde_json::to_value(&list) {
            event_log.append(SyncEvent::TaskListCreated(EventPayload {
                id: list.id.clone(),
                fields,
            }));
        }
        return Ok(TaskListRow {
            inner: list,
            account_id: LOCAL_ID.to_string(),
            task_capabilities: local_task_capabilities(),
        });
    }

    // External provider — route through the registry like the read /
    // rename paths. The provider owns its own sync, so no event-log
    // entry. We stamp the new route immediately so a follow-up op
    // reaches the right adapter before the next full refresh.
    let Some(ext) = registry.task_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("account '{account}' is not routable"),
        });
    };
    let list = ext
        .create_task_list(&request.name, request.parent_id.as_deref())
        .await?;
    registry.note_task_list_route(&list.id, &account);
    // Write-through: drop the listing snapshot so the sidebar's refresh
    // re-fetches and shows the new list.
    let _ = cache.invalidate(&account, SyncScope::TaskLists, "");

    let shared = db.shared();
    let account_kinds: std::collections::HashMap<String, AdapterKind> = AccountsRepo::new(&shared)
        .list()
        .map(|accounts| {
            accounts
                .into_iter()
                .map(|a| (a.id, a.adapter_kind))
                .collect()
        })
        .unwrap_or_default();
    let task_capabilities = task_caps_for_account(&account, &account_kinds, &plugin_manager);
    Ok(TaskListRow {
        inner: list,
        account_id: account,
        task_capabilities,
    })
}

#[tauri::command]
pub async fn delete_task_list(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
) -> CommandResult<()> {
    let account = registry
        .account_for_task_list(&id)
        .unwrap_or_else(|| LOCAL_ID.to_string());

    if account == LOCAL_ID {
        adapter.delete_task_list(&id)?;
        event_log.append(SyncEvent::TaskListDeleted(IdPayload { id: id.clone() }));
        return Ok(());
    }

    let Some(ext) = registry.task_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("account '{account}' is not routable"),
        });
    };
    ext.delete_task_list(&id).await?;
    // Write-through: the list is gone — drop the snapshot so the next
    // listing re-fetches without it, plus its cached task rows.
    let _ = cache.invalidate(&account, SyncScope::TaskLists, "");
    let _ = cache.invalidate(&account, SyncScope::Tasks, &id);
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ReparentTaskListRequest {
    pub id: String,
    /// New parent list id, or `None` to promote to top level.
    pub parent_id: Option<String>,
}

/// Reparent a local task list under another (or to the top level).
/// Local-store only — external-provider projects are reparented in
/// their own UI; the frontend gates the gesture to local lists. The
/// backend independently enforces the no-self / no-cycle invariant so a
/// buggy caller can't corrupt the tree, then emits a `task_list.updated`
/// event so the move propagates cross-device.
#[tauri::command]
pub async fn reparent_task_list(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: ReparentTaskListRequest,
) -> CommandResult<TaskList> {
    if let Some(parent) = &request.parent_id {
        if parent == &request.id {
            return Err(CommandError {
                code: "invalid",
                message: "a task list cannot be its own parent".into(),
            });
        }
        // Walk up the prospective parent's ancestor chain; reaching the
        // moved list means the move would form a cycle.
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(parent.clone());
        while let Some(cur) = cursor {
            if cur == request.id {
                return Err(CommandError {
                    code: "invalid",
                    message: "reparenting would create a cycle".into(),
                });
            }
            if !seen.insert(cur.clone()) {
                break;
            }
            cursor = adapter.get_task_list_by_id(&cur)?.and_then(|l| l.parent_id);
        }
    }

    let updated = adapter.reparent_task_list(&request.id, request.parent_id.as_deref())?;
    if let Ok(fields) = serde_json::to_value(&updated) {
        event_log.append(SyncEvent::TaskListUpdated(EventPayload {
            id: updated.id.clone(),
            fields,
        }));
    }
    Ok(updated)
}

#[tauri::command]
pub async fn get_tasks(
    app: AppHandle,
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    coord: State<'_, Arc<RefreshCoordinator>>,
    list_id: String,
) -> CommandResult<Vec<Task>> {
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(adapter.get_tasks(&list_id).await?);
    }
    let Some(ext) = registry.task_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("task list '{list_id}' is not routable"),
        });
    };

    // Stale-while-revalidate: serve the snapshot instantly when we have
    // one, refreshing in the background past the freshness window.
    let state = cache
        .get_sync_state(&account, SyncScope::Tasks, &list_id)
        .ok()
        .flatten();
    if cache_swr::has_snapshot(&state) {
        let cached = cache.read_tasks(&account, &list_id).unwrap_or_default();
        if cache_swr::is_stale(&state, cache_swr::SWR_TTL_SECS) {
            let ext_bg = Arc::clone(&ext);
            let cache_bg = Arc::clone(&cache);
            let acc = account.clone();
            let list = list_id.clone();
            cache_swr::spawn_item_refresh(
                app.clone(),
                Arc::clone(&cache),
                Arc::clone(&coord),
                SyncScope::Tasks,
                account.clone(),
                list_id.clone(),
                move || async move {
                    cache_swr::refresh_tasks(&cache_bg, ext_bg.as_ref(), &acc, &list).await
                },
            );
        }
        return Ok(cached);
    }

    // Cold path: no snapshot yet. Don't block the first paint on the
    // network — serve whatever rows exist now (usually none on a genuine
    // cold start) and refresh in the background; `cache-updated` fills the
    // list in when the fetch lands.
    let snapshot = cache.read_tasks(&account, &list_id).unwrap_or_default();
    let ext_bg = Arc::clone(&ext);
    let cache_bg = Arc::clone(&cache);
    let acc = account.clone();
    let list = list_id.clone();
    cache_swr::spawn_item_refresh(
        app.clone(),
        Arc::clone(&cache),
        Arc::clone(&coord),
        SyncScope::Tasks,
        account.clone(),
        list_id.clone(),
        move || async move { cache_swr::refresh_tasks(&cache_bg, ext_bg.as_ref(), &acc, &list).await },
    );
    Ok(snapshot)
}

/// List the sections (Vikunja buckets / Todoist sections) of one
/// list. Routes by the list's owning account exactly like
/// `get_tasks`: local lists hit the SQLite store, external lists hit
/// the provider adapter. Section-less backends return an empty list
/// via the `TasksFeature::list_sections` default.
#[tauri::command]
pub async fn get_sections(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
) -> CommandResult<Vec<Section>> {
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(adapter.list_sections(&list_id).await?);
    }
    let Some(ext) = registry.task_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("task list '{list_id}' is not routable"),
        });
    };
    Ok(ext.list_sections(&list_id).await?)
}

/// The users who can be ASSIGNED a task in `list_id` — the list's
/// collaborator pool (DESIGN §9.7), feeding the assignee picker. Local
/// lists have no members (returns empty); external lists hit the
/// provider adapter (Vikunja `projectusers`, …). A non-routable or
/// member-less backend yields an empty list, so the UI just shows no
/// candidates rather than an error.
#[tauri::command]
pub async fn task_list_members(
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
) -> CommandResult<Vec<TaskUser>> {
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(Vec::new());
    }
    let Some(ext) = registry.task_adapter(&account) else {
        return Ok(Vec::new());
    };
    Ok(ext.list_task_list_members(&list_id).await?)
}

/// The connected account's own identity ("me") for the account that
/// owns `list_id`, used to mark "assigned to me" in the UI. Local lists
/// (and providers without a user concept) return `None`.
#[tauri::command]
pub async fn task_current_user(
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
) -> CommandResult<Option<TaskUser>> {
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(None);
    }
    let Some(ext) = registry.task_adapter(&account) else {
        return Ok(None);
    };
    Ok(ext.current_user().await?)
}

/// The editable membership/shares of `list_id` (DESIGN §9.7), driving
/// the members dialog. Local / non-manageable backends return empty.
#[tauri::command]
pub async fn task_list_shares(
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
) -> CommandResult<Vec<TaskListShare>> {
    // TEMP diagnostics (members dialog "nothing happens"): if this fires
    // after picking "Manage members", the dialog DID open and the issue is
    // focus, not the open path.
    eprintln!("[aperio-diag] task_list_shares invoked for list {list_id}");
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(Vec::new());
    }
    let Some(ext) = registry.task_adapter(&account) else {
        return Ok(Vec::new());
    };
    Ok(ext.list_task_list_shares(&list_id).await?)
}

/// Search the owning account's user directory for people to add to
/// `list_id` (Vikunja). Empty for backends without a directory.
#[tauri::command]
pub async fn task_search_users(
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
    query: String,
) -> CommandResult<Vec<TaskUser>> {
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(Vec::new());
    }
    let Some(ext) = registry.task_adapter(&account) else {
        return Ok(Vec::new());
    };
    Ok(ext.search_users(&query).await?)
}

fn route_task_list(
    registry: &AdapterRegistry,
    list_id: &str,
) -> CommandResult<Arc<dyn TasksFeature>> {
    let account = registry
        .account_for_task_list(list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    registry.task_adapter(&account).ok_or_else(|| CommandError {
        code: "not_found",
        message: format!("task list '{list_id}' is not routable"),
    })
}

/// Add/invite a member to `list_id` (Vikunja username; Todoist email).
#[tauri::command]
pub async fn task_add_member(
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
    member_ref: String,
    right: Option<MemberRight>,
) -> CommandResult<()> {
    let ext = route_task_list(&registry, &list_id)?;
    Ok(ext
        .add_task_list_member(&list_id, &member_ref, right)
        .await?)
}

/// Remove a member from `list_id`.
#[tauri::command]
pub async fn task_remove_member(
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
    member_ref: String,
) -> CommandResult<()> {
    let ext = route_task_list(&registry, &list_id)?;
    Ok(ext.remove_task_list_member(&list_id, &member_ref).await?)
}

/// Change a member's right (Vikunja).
#[tauri::command]
pub async fn task_set_member_right(
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
    member_ref: String,
    right: MemberRight,
) -> CommandResult<()> {
    let ext = route_task_list(&registry, &list_id)?;
    Ok(ext
        .set_task_list_member_right(&list_id, &member_ref, right)
        .await?)
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub list_id: String,
    #[serde(flatten)]
    pub task: NewTask,
}

#[tauri::command]
pub async fn get_task_by_id(
    adapter: State<'_, LocalAdapter>,
    id: String,
) -> CommandResult<Option<Task>> {
    Ok(adapter.get_task_by_id(&id)?)
}

#[tauri::command]
pub async fn create_task(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateTaskRequest,
) -> CommandResult<Task> {
    let account = registry
        .account_for_task_list(&request.list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    let is_local = account == LOCAL_ID;
    let task = if is_local {
        adapter.create_task(&request.list_id, request.task).await?
    } else {
        let Some(ext) = registry.task_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("task list '{}' is not routable", request.list_id),
            });
        };
        ext.create_task(&request.list_id, request.task).await?
    };
    if is_local {
        if let Ok(fields) = serde_json::to_value(&task) {
            event_log.append(SyncEvent::TaskCreated(EventPayload {
                id: task.id.clone(),
                fields,
            }));
        }
    } else {
        // Write-through: force the next read of this list to re-fetch so
        // the new task shows up rather than the pre-create snapshot.
        let _ = cache.invalidate(&account, SyncScope::Tasks, &request.list_id);
    }
    scheduler.invalidate();
    Ok(task)
}

#[tauri::command]
pub async fn update_task(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    task: Task,
    previous_list_id: Option<String>,
) -> CommandResult<Task> {
    let target_account = registry
        .account_for_task_list(&task.list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());

    // Cross-list move detection — same shape as `update_event`'s
    // cross-calendar move guard. The TaskDialog's list picker
    // doubles as a "move to another list" gesture; without this
    // hint, a save against a different list would PATCH the
    // wrong resource (CalDAV VTODO at the old URL → 412 with
    // If-Match; Google Tasks `tasks.patch` against the wrong
    // tasklist → 404; iCloud-shaped CalDAV → Conflict).
    let is_move = previous_list_id
        .as_deref()
        .map(|prev| prev != task.list_id)
        .unwrap_or(false);

    if is_move {
        let previous = previous_list_id.expect("checked above");
        let source_account = registry
            .account_for_task_list(&previous)
            .unwrap_or_else(|| LOCAL_ID.to_string());

        // Local ↔ Local: the LocalAdapter does the move via a
        // single SQL UPDATE on the list_id column. No
        // create+delete dance needed.
        if source_account == LOCAL_ID && target_account == LOCAL_ID {
            let updated = adapter.update_task(task).await?;
            // Local↔Local task move = single SQL UPDATE on list_id.
            // Emit one TaskUpdated with the full row.
            if let Ok(fields) = serde_json::to_value(&updated) {
                event_log.append(SyncEvent::TaskUpdated(EventPayload {
                    id: updated.id.clone(),
                    fields,
                }));
            }
            scheduler.invalidate();
            return Ok(updated);
        }

        // Cross-list move involving an external adapter — works
        // across providers too (Google Tasks → Todoist, iCloud
        // CalDAV-VTODO → Microsoft To Do, etc.). The new task
        // gets a fresh adapter-assigned id; the frontend's
        // dataVersion bump on dialog-close forces a refetch that
        // surfaces the new id naturally without any caller
        // having to translate the old id to the new one. Create
        // BEFORE delete so a half-failed move never leaves the
        // user with nothing.
        let new_payload = NewTask {
            assignees: Vec::new(),
            title: task.title.clone(),
            description: task.description.clone(),
            status: task.status,
            priority: task.priority,
            scheduled_date: task.scheduled_date,
            scheduled_time: task.scheduled_time,
            deadline_date: task.deadline_date,
            deadline_time: task.deadline_time,
            recurrence: task.recurrence.clone(),
            parent_id: task.parent_id.clone(),
            section_id: None,
            color_label: task.color_label.clone(),
            reminders: task.reminders.clone(),
            sound: task.sound.clone(),
        };

        let created = if target_account == LOCAL_ID {
            adapter.create_task(&task.list_id, new_payload).await?
        } else {
            let Some(ext) = registry.task_adapter(&target_account) else {
                return Err(CommandError {
                    code: "not_found",
                    message: format!("target task list '{}' is not routable", task.list_id,),
                });
            };
            ext.create_task(&task.list_id, new_payload).await?
        };

        // Delete from source. Warn-but-continue on failure: the
        // create at the target already succeeded, and a bubbled
        // error here would tempt the user to retry, doubling
        // the duplicate. A leftover row in the source list is
        // the lesser evil.
        let delete_result = if source_account == LOCAL_ID {
            adapter
                .delete_task(&task.id)
                .await
                .map_err(CommandError::from)
        } else if let Some(ext) = registry.task_adapter(&source_account) {
            ext.delete_task(&task.id).await.map_err(CommandError::from)
        } else {
            Ok(())
        };
        if let Err(err) = delete_result {
            tracing::warn!(
                task_id = %task.id,
                source = %previous,
                target = %task.list_id,
                code = %err.code,
                message = %err.message,
                "delete from source task list failed after move; duplicate may exist",
            );
        }

        // Sync-event emission: same shape as the event move —
        // each LOCAL side emits its own event, external sides
        // stay silent and rely on the provider's sync mesh.
        if target_account == LOCAL_ID {
            if let Ok(fields) = serde_json::to_value(&created) {
                event_log.append(SyncEvent::TaskCreated(EventPayload {
                    id: created.id.clone(),
                    fields,
                }));
            }
        }
        if source_account == LOCAL_ID {
            event_log.append(SyncEvent::TaskDeleted(IdPayload {
                id: task.id.clone(),
            }));
        }

        // Write-through: re-fetch both ends of an external move.
        if target_account != LOCAL_ID {
            let _ = cache.invalidate(&target_account, SyncScope::Tasks, &task.list_id);
        }
        if source_account != LOCAL_ID {
            let _ = cache.invalidate(&source_account, SyncScope::Tasks, &previous);
        }

        scheduler.invalidate();
        return Ok(created);
    }

    // Plain in-place update.
    let is_local = target_account == LOCAL_ID;
    let updated = if is_local {
        adapter.update_task(task).await?
    } else {
        let Some(ext) = registry.task_adapter(&target_account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("task list '{}' is not routable", task.list_id),
            });
        };
        ext.update_task(task).await?
    };
    if is_local {
        if let Ok(fields) = serde_json::to_value(&updated) {
            event_log.append(SyncEvent::TaskUpdated(EventPayload {
                id: updated.id.clone(),
                fields,
            }));
        }
    } else {
        let _ = cache.invalidate(&target_account, SyncScope::Tasks, &updated.list_id);
    }
    scheduler.invalidate();
    Ok(updated)
}

#[tauri::command]
pub async fn delete_task(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
    list_id: Option<String>,
) -> CommandResult<()> {
    let account = list_id
        .as_deref()
        .and_then(|lid| registry.account_for_task_list(lid))
        .unwrap_or_else(|| LOCAL_ID.to_string());
    let is_local = account == LOCAL_ID;
    if is_local {
        adapter.delete_task(&id).await?;
    } else {
        let Some(ext) = registry.task_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("account '{account}' is not routable"),
            });
        };
        ext.delete_task(&id).await?;
    }
    if is_local {
        event_log.append(SyncEvent::TaskDeleted(IdPayload { id: id.clone() }));
    } else if let Some(lid) = &list_id {
        let _ = cache.invalidate(&account, SyncScope::Tasks, lid);
    }
    scheduler.invalidate();
    Ok(())
}

// ── Section commands ────────────────────────────────────────────────
//
// Sections are currently a local-store concept: the user creates and
// reorders them on local lists, and they propagate cross-device via the
// `section.*` event log. External-provider sections (Vikunja buckets,
// Todoist sections) are read-only here — they surface through
// `get_sections` but are managed in the provider's own UI, so these
// mutation commands always target the local adapter.

#[derive(Debug, Deserialize)]
pub struct CreateSectionRequest {
    pub list_id: String,
    pub name: String,
    /// Display order; defaults to 0 (the frontend appends with the
    /// current section count to keep new sections at the bottom).
    #[serde(default)]
    pub position: u32,
}

#[tauri::command]
pub async fn create_section(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateSectionRequest,
) -> CommandResult<Section> {
    let section = adapter.create_section(&request.list_id, &request.name, request.position)?;
    if let Ok(fields) = serde_json::to_value(&section) {
        event_log.append(SyncEvent::SectionCreated(EventPayload {
            id: section.id.clone(),
            fields,
        }));
    }
    Ok(section)
}

#[tauri::command]
pub async fn update_section(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    section: Section,
) -> CommandResult<Section> {
    let updated = adapter.update_section(section)?;
    if let Ok(fields) = serde_json::to_value(&updated) {
        event_log.append(SyncEvent::SectionUpdated(EventPayload {
            id: updated.id.clone(),
            fields,
        }));
    }
    Ok(updated)
}

#[tauri::command]
pub async fn delete_section(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_section(&id)?;
    event_log.append(SyncEvent::SectionDeleted(IdPayload { id: id.clone() }));
    Ok(())
}
