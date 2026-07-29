//! Account management commands (DESIGN.md §6.2 + §6.4).

use cal_core::{CalendarFeature, TasksFeature};
use plugin_core::shim::{FfiCalendarAdapter, FfiTasksAdapter};
use plugin_core::PluginManager;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::State;

use super::{run_plugin_auth, run_plugin_discover, CommandError, CommandResult};
use crate::accounts::{Account, AccountsError, AccountsRepo, AdapterKind};
use crate::cache::CacheRefresher;
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::registry::AdapterRegistry;
use crate::secrets::{self, SecretSlot};
use sync_core::{AccountPayload, IdPayload, SyncEvent};

/// Build an `AccountPayload` from a freshly-created or updated
/// `Account` row. Centralised so create / Google connect /
/// Microsoft connect emit the same shape — the applier on
/// receiving devices then upserts a row that's byte-identical
/// to what the snapshot path would produce.
fn account_payload(acc: &Account) -> AccountPayload {
    AccountPayload {
        id: acc.id.clone(),
        adapter_kind: acc.adapter_kind.as_str().to_string(),
        display_name: acc.display_name.clone(),
        config_json: acc.config_json.clone(),
        created_at: acc.created_at.clone(),
        updated_at: acc.updated_at.clone(),
    }
}

/// One-shot pref marker recording that we've already replayed
/// the existing local accounts as `AccountCreated` events. The
/// Account.* event variants were added in a later iteration —
/// users who created accounts before that ship would otherwise
/// have rows in SQLite that never produce sync events. On the
/// first boot after the upgrade we walk those rows once and
/// emit catch-up events so the next sync round actually carries
/// them to the remote.
const PREF_ACCOUNTS_BACKFILLED: &str = "sync.accounts.eventBackfillDone";

/// Catch-up emit: idempotent. Walks every non-local account in
/// the local accounts table and pushes an `AccountCreated`
/// event through the writer for it, then sets the pref so the
/// next boot is a no-op. Safe to call before the Tauri runtime
/// is fully wired — only depends on the writer + the shared db.
///
/// Receivers deduplicate via `sync_applied_events` (the
/// applier's idempotency table), so even if a peer somehow
/// gets the same payload twice, the second one is a no-op.
///
/// Errors are best-effort: a failed db read or pref write
/// surfaces as a `warn!` log and leaves the backfill
/// un-flagged, so the next boot retries. We never want a
/// backfill problem to block app startup.
pub fn backfill_account_events(db: &crate::db::DbHandle, event_log: &EventLogWriter) {
    let shared = db.shared();
    let prefs = crate::user_prefs::UserPrefsRepo::new(&shared);
    match prefs.get(PREF_ACCOUNTS_BACKFILLED) {
        Ok(Some(v)) if v == "true" => return,
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(?err, "account backfill: prefs read failed");
            return;
        }
    }
    let repo = AccountsRepo::new(&shared);
    let accounts = match repo.list() {
        Ok(a) => a,
        Err(err) => {
            tracing::warn!(?err, "account backfill: list failed");
            return;
        }
    };
    let mut emitted = 0usize;
    for acc in accounts {
        if acc.id == "local" || acc.adapter_kind == "local" {
            continue;
        }
        event_log.append(SyncEvent::AccountCreated(account_payload(&acc)));
        emitted += 1;
    }
    if let Err(err) = prefs.set(PREF_ACCOUNTS_BACKFILLED, "true") {
        tracing::warn!(?err, "account backfill: pref write failed");
        return;
    }
    tracing::info!(emitted, "account backfill: replayed existing accounts");
}

// ── Plugin-id constants ──────────────────────────────────────
//
// Centralised so onboarding's smoke-test fns route to the same
// plugin ids the registry uses at bootstrap time.
const PLUGIN_ID_CALDAV: &str = "com.aperio.cal-adapter-caldav";
const PLUGIN_ID_ICAL: &str = "com.aperio.cal-adapter-ical";
const PLUGIN_ID_EWS: &str = "com.aperio.cal-adapter-ews";
const PLUGIN_ID_VIKUNJA: &str = "com.aperio.cal-adapter-vikunja";
const PLUGIN_ID_TODOIST: &str = "com.aperio.cal-adapter-todoist";
const PLUGIN_ID_GOOGLE: &str = "com.aperio.cal-adapter-google";
const PLUGIN_ID_GRAPH: &str = "com.aperio.cal-adapter-microsoft-graph";

/// Wire-shape entry returned by [`list_accounts`]. Wraps the
/// persisted [`Account`] with derived per-row status the
/// AccountsPanel renders without a second round-trip:
///
///   - `plugin_loaded` — `true` when the plugin id this
///     account's adapter_kind maps to is currently loaded in
///     the host's [`PluginManager`] (and not disabled).
///     `false` triggers the §20.8 "Plugin fehlt" indicator
///     pointing the user at Settings → Plugins. Local
///     accounts always return `true` — they're host-internal.
///
/// The wrapper keeps the persisted [`Account`] struct free of
/// runtime-derived noise + lets future iterations add more
/// status fields (e.g. "credentials present") without
/// changing the storage layer.
#[derive(Debug, Serialize)]
pub struct AccountListEntry {
    #[serde(flatten)]
    pub account: Account,
    pub plugin_loaded: bool,
    /// Whether this account can mint meetings — i.e. its plugin declares itself
    /// a `videoconference-adapter`. Read from the manifest rather than from a
    /// list of provider names, so an adapter added later is offered without a
    /// change here or in the UI.
    pub is_videoconference: bool,
}

#[tauri::command]
pub async fn list_accounts(
    db: State<'_, DbHandle>,
    plugin_manager: State<'_, Arc<PluginManager>>,
) -> CommandResult<Vec<AccountListEntry>> {
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let accounts = repo.list()?;
    let out = accounts
        .into_iter()
        .map(|account| {
            // A host-internal kind has no plugin to look up and is always
            // available; anything else is loaded iff a plugin declares it.
            let plugin_loaded = account.adapter_kind.is_host_internal()
                || plugin_manager
                    .plugin_for_adapter_kind(account.adapter_kind.as_str())
                    .is_some();
            let is_videoconference = plugin_manager
                .plugin_for_adapter_kind(account.adapter_kind.as_str())
                .is_some_and(|p| {
                    p.manifest
                        .has_capability(&plugin_core::Capability::Videoconference)
                });
            AccountListEntry {
                account,
                plugin_loaded,
                is_videoconference,
            }
        })
        .collect();
    Ok(out)
}

/// §19.11 step 8 — list accounts whose keychain credentials are
/// absent on this device. After `accept_remote_dataset` on a
/// fresh device, the snapshot has populated the `accounts` table
/// (config + adapter kind) but the OS keychain is empty for
/// every entry — credentials never travel through the sync
/// store. The onboarding wizard reads this list to render the
/// "Konten verbinden" UI.
///
/// The `local` account is always skipped: it has no credentials
/// to begin with.
///
/// The credentials check is per-`adapter_kind`:
///
/// - `caldav`, `ews`: needs a `Password` slot.
/// - `vikunja`, `todoist`: needs an `ApiToken` slot.
/// - `google`, `microsoft_graph`: needs a `RefreshToken` slot
///   (the access token is short-lived and the registry can
///   re-mint it from the refresh token; an account with only
///   an access token would shortly need re-auth anyway).
/// - `ical`, `local`: no secret required — iCal feeds are
///   typically public HTTP(S) URLs, and the rare Basic-auth
///   private feed surfaces a clear 401 on the first fetch
///   instead of nagging the user up front.
///
/// A `NotFound` from the keychain → missing credential; any
/// other error → log + treat as missing (better to prompt the
/// user than to silently leave them disconnected).
#[tauri::command]
pub async fn list_accounts_missing_credentials(
    db: State<'_, DbHandle>,
    plugin_manager: State<'_, Arc<PluginManager>>,
) -> CommandResult<Vec<Account>> {
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let all = repo.list()?;
    let mut out = Vec::new();
    for acc in all {
        if acc.id == "local" || acc.adapter_kind == "local" {
            continue;
        }
        // What "connected" means is the ADAPTER's statement when it makes one:
        // the required secret fields of its schema. The per-kind table below is
        // the fallback for the adapters that have not declared one yet.
        let slots: Vec<SecretSlot> = plugin_manager
            .plugin_for_adapter_kind(acc.adapter_kind.as_str())
            .and_then(|p| p.manifest.account.clone())
            .map(|schema| host_core::account_setup::required_slots(&schema))
            .unwrap_or_else(|| {
                required_secret_slot(&acc.adapter_kind)
                    .into_iter()
                    .collect()
            });
        if slots.iter().any(|slot| !secret_present(&acc.id, *slot)) {
            out.push(acc);
        }
    }
    Ok(out)
}

/// Which keychain slot a fully-configured account of this kind must have
/// populated — the FALLBACK for adapters that declare no account schema.
///
/// An adapter that declares one answers this question itself, in its
/// `plugin.json`, and never reaches this table; see
/// `host_core::account_setup::required_slots`. This stays for the seven
/// adapters still on the older per-kind path.
///
/// `None` means the adapter has no required secret — iCal feeds are typically
/// public URLs, and an optional Basic-auth password fails open and surfaces as
/// a 401 on the first fetch.
fn required_secret_slot(kind: &AdapterKind) -> Option<SecretSlot> {
    // Exhaustive on purpose. The catch-all this replaces answered `Password`
    // for every kind it had not heard of, so a new OAuth kind was silently
    // probed for a password it never has — and every working account of that
    // kind was then reported as needing to be reconnected. A missing arm should
    // fail the build, not the user.
    match kind.as_str() {
        // No stored credential: iCal feeds are public, Local is host-internal,
        // and the mobile-only device-calendar account authenticates via the OS
        // permission grant (never reaches desktop, but the kind is shared).
        "ical" | "local" | "device_calendar" => None,
        "vikunja" | "todoist" => Some(SecretSlot::ApiToken),
        "google" | "microsoft_graph" => Some(SecretSlot::RefreshToken),
        "caldav" | "ews" => Some(SecretSlot::Password),
        // Video conferencing is OAuth throughout. Teams and Meet ride the token
        // of their calendar sibling, but each still has its own account row and
        // its own refresh token in its own slot.
        "zoom" | "teams" | "meet" | "webex" => Some(SecretSlot::RefreshToken),
        // A kind this build has no entry for. `None` rather than a guess: the
        // catch-all this replaces answered `Password`, so a new OAuth kind was
        // probed for a password it never has and every working account of that
        // kind was reported as needing to be reconnected. Saying nothing is
        // required is the harmless direction — the adapter surfaces a real auth
        // error on first use if it does need one. An adapter that declares an
        // account schema never reaches here at all.
        _ => None,
    }
}

/// Best-effort check for a keychain entry's presence. Treats any
/// error other than `NotFound` as a soft "missing" so the
/// wizard always errs on the side of letting the user
/// re-authenticate.
fn secret_present(account_id: &str, slot: SecretSlot) -> bool {
    secrets::retrieve(account_id, slot).is_ok()
}

/// Request payload for creating an account. `config_json` is the
/// adapter-specific non-secret configuration; the shape is owned by
/// each adapter and validated at adapter construction time.
#[derive(Debug, serde::Deserialize)]
pub struct CreateAccountRequest {
    pub adapter_kind: AdapterKind,
    pub display_name: String,
    #[serde(default = "default_config_json")]
    pub config_json: String,
    /// The secret half of the credentials (CalDAV password,
    /// OAuth refresh token, …). Optional because the local
    /// adapter doesn't need any. Stored only in the platform
    /// keychain, never in the SQLite store.
    #[serde(default)]
    pub secret: Option<String>,
}

fn default_config_json() -> String {
    "{}".into()
}

#[tauri::command]
pub async fn create_account(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    refresher: State<'_, Arc<CacheRefresher>>,
    request: CreateAccountRequest,
) -> CommandResult<Account> {
    // Reject adapter kinds we have no construction path for yet.
    // Local, CalDAV, iCal and EWS go through the `create_account`
    // dispatch (each with a password-style credential); Google and
    // Microsoft Graph have their own OAuth-shaped entry points. The
    // remaining kinds surface as an actionable "coming soon" envelope
    // rather than a half-broken row in the database.
    if !matches!(
        request.adapter_kind.as_str(),
        "local" | "caldav" | "ical" | "ews" | "vikunja" | "todoist"
    ) {
        return Err(CommandError {
            code: "unsupported",
            message: format!(
                "Adapter '{}' will be supported in a later phase.",
                request.adapter_kind.as_str()
            ),
        });
    }

    // Smoke-test credentials BEFORE writing anything so the user
    // sees auth / network errors instantly instead of "saved, but
    // doesn't work". Each smoke runs against an ephemeral plugin
    // instance built from the request payload + closes it
    // immediately; the persisted account gets a fresh instance
    // opened by the registry on the way in.
    //
    // The request's `config_json` is the same shape the registry
    // persists — pulling fields out of it via `Value::get` keeps
    // the host adapter-crate-agnostic. Any required field that's
    // missing surfaces as "invalid_input" from the plugin's own
    // InitConfig deserialiser, so we don't pre-validate here.
    let plugin_manager_ref: &PluginManager = plugin_manager.inner();
    let request_config: Value =
        serde_json::from_str(&request.config_json).map_err(|e| CommandError {
            code: "invalid_input",
            message: format!("invalid config JSON: {e}"),
        })?;
    let str_field = |key: &str| -> &str {
        request_config
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
    };

    if request.adapter_kind == "caldav" {
        let Some(secret) = request.secret.as_deref() else {
            return Err(CommandError {
                code: "invalid_input",
                message: "CalDAV needs a password to authenticate.".into(),
            });
        };
        // Persisted CaldavAccountConfig serialises `auth_kind`
        // as `"basic"` / `"bearer"`; the plugin's InitConfig
        // expects the same snake-case wire form. Default to
        // basic when the field is missing (older accounts
        // pre-AuthKind).
        let auth_kind = request_config
            .get("auth_kind")
            .and_then(Value::as_str)
            .unwrap_or("basic");
        smoke_test_caldav(
            plugin_manager_ref,
            str_field("server_url"),
            str_field("username"),
            auth_kind,
            secret,
        )
        .await?;
    }

    if request.adapter_kind == "ical" {
        smoke_test_ical(
            plugin_manager_ref,
            str_field("feed_url"),
            request_config.get("username").and_then(Value::as_str),
            request.secret.as_deref(),
        )
        .await?;
    }

    if request.adapter_kind == "ews" {
        let Some(secret) = request.secret.as_deref() else {
            return Err(CommandError {
                code: "invalid_input",
                message: "EWS needs a password to authenticate.".into(),
            });
        };
        smoke_test_ews(
            plugin_manager_ref,
            str_field("endpoint"),
            str_field("username"),
            secret,
        )
        .await?;
    }

    if request.adapter_kind == "vikunja" {
        let Some(secret) = request.secret.as_deref() else {
            return Err(CommandError {
                code: "invalid_input",
                message: "Vikunja needs an API token to authenticate.".into(),
            });
        };
        smoke_test_vikunja(plugin_manager_ref, str_field("server_url"), secret).await?;
    }

    if request.adapter_kind == "todoist" {
        let Some(secret) = request.secret.as_deref() else {
            return Err(CommandError {
                code: "invalid_input",
                message: "Todoist needs an API token to authenticate.".into(),
            });
        };
        smoke_test_todoist(plugin_manager_ref, secret).await?;
    }

    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let created = repo.create(
        request.adapter_kind.clone(),
        request.display_name.trim(),
        &request.config_json,
    )?;

    // Persist the secret right after the account row so the keychain
    // and the DB stay aligned. A keychain write that fails is fatal
    // — we tear the row down again so the user doesn't end up with
    // an external account that can never authenticate.
    //
    // The slot depends on the adapter kind: Vikunja (and any future
    // adapter that authenticates with a long-lived API token) lives
    // in `SecretSlot::ApiToken`, everyone else's Basic-auth-style
    // credential lives in `SecretSlot::Password`. The slot name is
    // what the registry's `register_*` paths look for, so the two
    // sides have to stay in step.
    if let Some(secret) = request.secret {
        let slot = match request.adapter_kind.as_str() {
            "vikunja" | "todoist" => SecretSlot::ApiToken,
            _ => SecretSlot::Password,
        };
        if let Err(err) = secrets::store(&created.id, slot, &secret) {
            let _ = repo.delete(&created.id);
            return Err(CommandError {
                code: "internal",
                message: format!("failed to store credential: {err}"),
            });
        }
        // E2E only: also push the secret to the user's other devices via
        // the encrypted log so the account works there without re-entry.
        // A no-op when E2E is off (credentials then stay device-local).
        crate::credential_sync::emit_credential_set(
            &event_log,
            &shared,
            &created.id,
            slot,
            &secret,
        );
    }

    // Register the freshly created external adapter so subsequent
    // reads/writes route through it. We already smoke-tested for
    // CalDAV; treating a registration failure here as fatal keeps
    // the keychain + DB + registry strictly in sync.
    if request.adapter_kind != "local" {
        if let Err(err) = registry.register(&created) {
            let _ = secrets::delete_all(&created.id);
            let _ = repo.delete(&created.id);
            return Err(CommandError {
                code: "internal",
                message: format!("adapter registration failed: {err}"),
            });
        }
    }
    // Sync the new account row to other devices. Secrets stay in
    // this device's keychain — `AccountPayload` carries only the
    // non-secret metadata, and the receiver surfaces the
    // "credentials missing" wizard for the device-local secret.
    event_log.append(SyncEvent::AccountCreated(account_payload(&created)));
    // The boot warm pass may already have run; kick a warm so the new account's
    // calendars/lists load now instead of only after the next pass / restart.
    refresher.trigger();
    Ok(created)
}

/// Discover + list calendars against the supplied CalDAV
/// credentials. The result is discarded; this command exists
/// purely to surface a clear "credentials work?" answer ahead of
/// persisting anything.
async fn smoke_test_caldav(
    plugin_manager: &PluginManager,
    server_url: &str,
    username: &str,
    auth_kind: &str,
    secret: &str,
) -> Result<(), CommandError> {
    let config = json!({
        "server_url": server_url,
        "username": username,
        "auth_kind": auth_kind,
        "secret": secret,
    });
    smoke_via_calendar_plugin(plugin_manager, PLUGIN_ID_CALDAV, config).await
}

/// One-shot fetch of the iCal feed. Confirms the URL resolves, the
/// server answers, and (if credentials are provided) Basic auth is
/// accepted. The ephemeral plugin instance is dropped after the
/// call — the real one gets opened again from the persisted
/// config so the request and storage stay in sync.
async fn smoke_test_ical(
    plugin_manager: &PluginManager,
    feed_url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<(), CommandError> {
    let config = json!({
        "feed_url": feed_url,
        "username": username,
        "password": password.filter(|s| !s.is_empty()),
    });
    smoke_via_calendar_plugin(plugin_manager, PLUGIN_ID_ICAL, config).await
}

/// EWS smoke-test: open a plugin instance against the supplied
/// endpoint + Basic-auth credentials, run `list_calendars`,
/// drop. Same pattern as `smoke_test_caldav` — surfaces wrong
/// URL, wrong password, firewall problems ahead of persisting
/// anything.
async fn smoke_test_ews(
    plugin_manager: &PluginManager,
    endpoint: &str,
    username: &str,
    password: &str,
) -> Result<(), CommandError> {
    let config = json!({
        "endpoint": endpoint,
        "username": username,
        "password": password,
    });
    smoke_via_calendar_plugin(plugin_manager, PLUGIN_ID_EWS, config).await
}

/// Vikunja round-trip: open a plugin instance against the
/// supplied server + token and hit `list_task_lists`. Surfaces
/// wrong URL / wrong token / firewall problems before we
/// persist anything.
async fn smoke_test_vikunja(
    plugin_manager: &PluginManager,
    server_url: &str,
    secret: &str,
) -> Result<(), CommandError> {
    let config = json!({
        "server_url": server_url,
        "token": secret,
    });
    smoke_via_tasks_plugin(plugin_manager, PLUGIN_ID_VIKUNJA, config).await
}

/// Todoist round-trip: open a plugin instance with the supplied
/// token and hit `list_task_lists`. Surfaces revoked tokens /
/// network problems before persistence.
async fn smoke_test_todoist(
    plugin_manager: &PluginManager,
    secret: &str,
) -> Result<(), CommandError> {
    let config = json!({ "token": secret });
    smoke_via_tasks_plugin(plugin_manager, PLUGIN_ID_TODOIST, config).await
}

/// Shared smoke-test for plugins that expose `CalendarFeature`:
/// opens an ephemeral instance, runs `list_calendars`, then
/// drops the instance. The trait method exercises the full
/// auth + protocol round-trip, which is exactly what the
/// "credentials work?" question needs answered.
async fn smoke_via_calendar_plugin(
    plugin_manager: &PluginManager,
    plugin_id: &str,
    config: Value,
) -> Result<(), CommandError> {
    let instance = open_smoke_instance(plugin_manager, plugin_id, config)?;
    let adapter = FfiCalendarAdapter::new(instance).ok_or(CommandError {
        code: "internal",
        message: format!("plugin {plugin_id} doesn't expose CalendarFeature",),
    })?;
    adapter
        .list_calendars()
        .await
        .map(|_| ())
        .map_err(plugin_cal_error_to_command)
}

/// Tasks-side counterpart to [`smoke_via_calendar_plugin`].
async fn smoke_via_tasks_plugin(
    plugin_manager: &PluginManager,
    plugin_id: &str,
    config: Value,
) -> Result<(), CommandError> {
    let instance = open_smoke_instance(plugin_manager, plugin_id, config)?;
    let adapter = FfiTasksAdapter::new(instance).ok_or(CommandError {
        code: "internal",
        message: format!("plugin {plugin_id} doesn't expose TasksFeature",),
    })?;
    adapter
        .list_task_lists()
        .await
        .map(|_| ())
        .map_err(plugin_cal_error_to_command)
}

/// Open an ephemeral plugin instance for a smoke test. The
/// returned Arc is dropped by the caller's scope, which fires
/// the plugin's `close_instance` hook + releases the runtime.
fn open_smoke_instance(
    plugin_manager: &PluginManager,
    plugin_id: &str,
    config: Value,
) -> Result<Arc<plugin_core::LoadedInstance>, CommandError> {
    let plugin = plugin_manager.get(plugin_id).ok_or(CommandError {
        code: "plugin_missing",
        message: format!("plugin {plugin_id} is not loaded"),
    })?;
    plugin_manager
        .open_instance(plugin, &config.to_string())
        .map_err(|err| match err {
            plugin_core::error::PluginError::InstanceOpen { message, .. } => CommandError {
                code: "invalid_input",
                message,
            },
            other => CommandError {
                code: "internal",
                message: other.to_string(),
            },
        })
}

/// Map `cal_core::Error` (the error type the FfiCalendarAdapter
/// / FfiTasksAdapter shims surface from the plugin) into the
/// uniform `CommandError` shape the frontend understands.
fn plugin_cal_error_to_command(err: cal_core::Error) -> CommandError {
    use cal_core::Error::*;
    let (code, message) = match err {
        Authentication(m) => ("auth", m),
        Forbidden(m) => ("forbidden", m),
        NotFound(m) => ("not_found", m),
        Conflict(m) => ("conflict", m),
        Network(m) => ("network", m),
        Protocol(m) => ("protocol", m),
        InvalidInput(m) => ("invalid_input", m),
        Unsupported(m) => ("unsupported", m),
        Internal(m) => ("internal", m),
    };
    CommandError { code, message }
}

/// Round-trip a CalDAV credential check without persisting anything.
/// Used by the AccountsDialog's optional "Test connection" button.
#[tauri::command]
pub async fn test_caldav_connection(
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: TestCaldavRequest,
) -> CommandResult<()> {
    smoke_test_caldav(
        plugin_manager.inner(),
        &request.server_url,
        &request.username,
        // The "Test connection" button is wired against the
        // Basic-auth half of the CalDAV form today; Bearer
        // configurations go through the create flow instead.
        "basic",
        &request.password,
    )
    .await
}

#[derive(Debug, serde::Deserialize)]
pub struct TestCaldavRequest {
    pub server_url: String,
    pub username: String,
    pub password: String,
}

/// Round-trip a single fetch of the supplied iCal feed without
/// persisting anything. Same pattern as [`test_caldav_connection`].
#[tauri::command]
pub async fn test_ical_feed(
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: TestIcalRequest,
) -> CommandResult<()> {
    smoke_test_ical(
        plugin_manager.inner(),
        &request.feed_url,
        request.username.as_deref(),
        request.password.as_deref(),
    )
    .await
}

#[derive(Debug, serde::Deserialize)]
pub struct TestIcalRequest {
    pub feed_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Round-trip an EWS credential check without persisting anything.
/// Used by AccountsDialog's "Test connection" button on the EWS form.
#[tauri::command]
pub async fn test_ews_connection(
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: TestEwsRequest,
) -> CommandResult<()> {
    smoke_test_ews(
        plugin_manager.inner(),
        &request.endpoint,
        &request.username,
        &request.password,
    )
    .await
}

#[derive(Debug, serde::Deserialize)]
pub struct TestEwsRequest {
    pub endpoint: String,
    pub username: String,
    pub password: String,
}

/// Round-trip a Vikunja credential check without persisting anything.
/// Used by AccountsDialog's "Test connection" button on the Vikunja
/// form. Mirrors the EWS / CalDAV pattern — same `(server URL +
/// secret) → smoke-test` shape with `secret` being the API token.
#[tauri::command]
pub async fn test_vikunja_connection(
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: TestVikunjaRequest,
) -> CommandResult<()> {
    smoke_test_vikunja(
        plugin_manager.inner(),
        &request.server_url,
        &request.api_token,
    )
    .await
}

#[derive(Debug, serde::Deserialize)]
pub struct TestVikunjaRequest {
    pub server_url: String,
    pub api_token: String,
}

/// Round-trip a Todoist API-token check without persisting anything.
/// Used by AccountsDialog's "Test connection" button on the Todoist
/// form. Same shape as Vikunja minus the server URL — Todoist is
/// hosted, the base URL is hard-coded in the adapter.
#[tauri::command]
pub async fn test_todoist_connection(
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: TestTodoistRequest,
) -> CommandResult<()> {
    smoke_test_todoist(plugin_manager.inner(), &request.api_token).await
}

#[derive(Debug, serde::Deserialize)]
pub struct TestTodoistRequest {
    pub api_token: String,
}

/// Probe the user's domain for an EWS endpoint via POX
/// Autodiscover. The frontend's "Discover" button calls this
/// with the e-mail address + password (those are the only
/// inputs Microsoft's autodiscover surface needs). On success
/// the resolved EWS URL is echoed back so the dialog can pre-
/// fill the endpoint field.
///
/// The cascade runs inside the EWS plugin via
/// `aperio_plugin_discover`; the host stays adapter-crate-
/// agnostic by declaring the response shape locally
/// ([`DiscoveredEndpoints`]) and letting `run_plugin_discover`
/// deserialise the plugin's JSON into it. The Tauri command's
/// JSON envelope is unchanged so the frontend keeps working.
///
/// Trade-off vs the pre-plugin path: the typed `EwsError`
/// variants no longer cross into the host — all
/// `Autodiscover HTTP {status}` / SOAP fault messages flow back
/// under `code: "not_found"` with the plugin's message text
/// verbatim. The frontend already renders the message field, so
/// the user-visible UX is unchanged; a future plugin-ABI
/// extension could thread a typed status enum across the FFI
/// boundary if we want per-category styling back.
#[tauri::command]
pub async fn discover_ews_endpoint(
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: DiscoverEwsRequest,
) -> CommandResult<DiscoveredEndpoints> {
    let email = request.email.trim();
    if email.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "Email must not be empty.".into(),
        });
    }
    let password = &request.password;
    if password.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "Password must not be empty.".into(),
        });
    }
    run_plugin_discover(
        plugin_manager.inner(),
        PLUGIN_ID_EWS,
        json!({ "email": email, "password": password }),
    )
    .await
}

#[derive(Debug, Deserialize)]
pub struct DiscoverEwsRequest {
    pub email: String,
    pub password: String,
}

/// Host-side mirror of `cal_adapter_ews::DiscoveredEndpoints`.
/// Field names + types match the plugin's serialised shape so
/// `serde_json` round-trips cleanly and the frontend's existing
/// `{ ews_url, account_email }` payload stays byte-identical.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredEndpoints {
    pub ews_url: String,
    pub account_email: String,
}

#[tauri::command]
pub async fn delete_account(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    repo.delete(&id)?;
    registry.unregister(&id);
    // Best-effort credential cleanup — leaves no Aperio entry behind in
    // the keychain for that account id.
    if let Err(err) = secrets::delete_all(&id) {
        tracing::warn!(?err, account_id = %id, "secrets cleanup failed");
    }
    event_log.append(SyncEvent::AccountDeleted(IdPayload { id }));
    Ok(())
}

/// Rename an account — change its user-visible `display_name`. Writes
/// the new name to the local row and emits `AccountUpdated` so other
/// synced devices pick it up (the applier upserts the row, leaving the
/// account's calendars/lists/secrets untouched). The local account can
/// be renamed too; only deletion is forbidden for it. Returns the
/// updated row so the caller can refresh without a round-trip.
#[tauri::command]
pub async fn rename_account(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
    new_name: String,
) -> CommandResult<Account> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "Account name must not be empty.".into(),
        });
    }
    let shared = db.shared();
    let account = AccountsRepo::new(&shared).rename(&id, trimmed)?;
    event_log.append(SyncEvent::AccountUpdated(account_payload(&account)));
    Ok(account)
}

impl From<AccountsError> for CommandError {
    fn from(err: AccountsError) -> Self {
        match err {
            AccountsError::NotFound(msg) => CommandError {
                code: "not_found",
                message: msg,
            },
            AccountsError::DeleteLocalForbidden => CommandError {
                code: "forbidden",
                message: "The local account cannot be deleted.".into(),
            },
            AccountsError::Sqlite(err) => CommandError {
                code: "internal",
                message: err.to_string(),
            },
        }
    }
}

/// Interactive Google sign-in. Runs the OAuth 2.0 PKCE dance via
/// the system browser, persists access + refresh tokens to the
/// platform keychain, creates the account row and registers the
/// adapter. The frontend's "Connect Google" button awaits this
/// command; the user sees a "Connecting…" state while the consent
/// screen is open.
///
/// The command is best-effort transactional: if any post-OAuth step
/// fails (DB insert, keychain write, registry registration) we tear
/// down everything we touched so the next attempt starts clean.
#[tauri::command]
pub async fn connect_google_account(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    refresher: State<'_, Arc<CacheRefresher>>,
    request: ConnectGoogleRequest,
) -> CommandResult<Account> {
    let name = request.display_name.trim();
    if name.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "display_name must not be empty".into(),
        });
    }
    let client_id = request.client_id.trim();
    if client_id.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "client_id must not be empty".into(),
        });
    }
    let client_secret = request.client_secret.trim();
    if client_secret.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "client_secret must not be empty".into(),
        });
    }

    // 1) Run the OAuth dance via the plugin. This opens the
    //    system browser and blocks the command until the user
    //    completes consent (or times out at 5 min, or the user
    //    denies). Errors here are surfaced verbatim — we
    //    haven't touched anything persistent yet.
    let tokens = run_plugin_auth(
        plugin_manager.inner(),
        PLUGIN_ID_GOOGLE,
        json!({
            "client_id": client_id,
            "client_secret": client_secret,
        }),
    )
    .await?;

    // 2) Create the account row. Config_json carries
    //    `client_id` + `client_secret` (the latter is what
    //    Google's docs themselves say "is not treated as a
    //    secret" for installed apps); the plugin's InitConfig
    //    deserialiser reads them back at open_instance time.
    let config_json = json!({
        "client_id": client_id,
        "client_secret": client_secret,
    })
    .to_string();
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let created = repo.create(AdapterKind::new("google"), name, &config_json)?;

    // 3) Persist tokens to the keychain. If either write fails
    //    we delete the row and surface an error so the user can
    //    retry with a clean slate.
    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or(CommandError {
            code: "protocol",
            message: "Google plugin returned no access_token".into(),
        })?;
    if let Err(err) = secrets::store(&created.id, SecretSlot::AccessToken, access) {
        let _ = repo.delete(&created.id);
        return Err(CommandError {
            code: "internal",
            message: format!("failed to store access token: {err}"),
        });
    }
    if let Some(refresh) = tokens.get("refresh_token").and_then(Value::as_str) {
        if let Err(err) = secrets::store(&created.id, SecretSlot::RefreshToken, refresh) {
            let _ = secrets::delete_all(&created.id);
            let _ = repo.delete(&created.id);
            return Err(CommandError {
                code: "internal",
                message: format!("failed to store refresh token: {err}"),
            });
        }
        // E2E only: sync the durable refresh token to the user's other
        // devices (the short-lived access token is re-derived per device).
        crate::credential_sync::emit_credential_set(
            &event_log,
            &shared,
            &created.id,
            SecretSlot::RefreshToken,
            refresh,
        );
    }

    // 4) Register the adapter so subsequent reads/writes route
    //    through it. The registry reads tokens from keychain
    //    again — round-trip is intentional so the read-path
    //    stays identical to what happens at app boot.
    if let Err(err) = registry.register(&created) {
        let _ = secrets::delete_all(&created.id);
        let _ = repo.delete(&created.id);
        return Err(CommandError {
            code: "internal",
            message: format!("adapter registration failed: {err}"),
        });
    }
    event_log.append(SyncEvent::AccountCreated(account_payload(&created)));
    // The boot warm pass may already have run; kick a warm so the new account's
    // calendars/lists load now instead of only after the next pass / restart.
    refresher.trigger();
    Ok(created)
}

#[derive(Debug, serde::Deserialize)]
pub struct ConnectGoogleRequest {
    pub client_id: String,
    pub client_secret: String,
    pub display_name: String,
}

/// Interactive Microsoft sign-in. Same shape as the Google
/// equivalent: opens the system browser, runs the OAuth dance,
/// persists tokens, registers the adapter. PKCE-only — no
/// `client_secret` for Microsoft public clients.
#[tauri::command]
pub async fn connect_microsoft_account(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    refresher: State<'_, Arc<CacheRefresher>>,
    request: ConnectMicrosoftRequest,
) -> CommandResult<Account> {
    let name = request.display_name.trim();
    if name.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "display_name must not be empty".into(),
        });
    }
    let client_id = request.client_id.trim();
    if client_id.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "client_id must not be empty".into(),
        });
    }
    let authority = request
        .authority
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("common");

    let tokens = run_plugin_auth(
        plugin_manager.inner(),
        PLUGIN_ID_GRAPH,
        json!({
            "client_id": client_id,
            "authority": authority,
        }),
    )
    .await?;

    let config_json = json!({
        "client_id": client_id,
        "authority": authority,
    })
    .to_string();
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let created = repo.create(AdapterKind::new("microsoft_graph"), name, &config_json)?;

    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or(CommandError {
            code: "protocol",
            message: "Microsoft Graph plugin returned no access_token".into(),
        })?;
    if let Err(err) = secrets::store(&created.id, SecretSlot::AccessToken, access) {
        let _ = repo.delete(&created.id);
        return Err(CommandError {
            code: "internal",
            message: format!("failed to store access token: {err}"),
        });
    }
    if let Some(refresh) = tokens.get("refresh_token").and_then(Value::as_str) {
        if let Err(err) = secrets::store(&created.id, SecretSlot::RefreshToken, refresh) {
            let _ = secrets::delete_all(&created.id);
            let _ = repo.delete(&created.id);
            return Err(CommandError {
                code: "internal",
                message: format!("failed to store refresh token: {err}"),
            });
        }
        // E2E only: sync the durable refresh token to the user's other
        // devices (the short-lived access token is re-derived per device).
        crate::credential_sync::emit_credential_set(
            &event_log,
            &shared,
            &created.id,
            SecretSlot::RefreshToken,
            refresh,
        );
    }

    if let Err(err) = registry.register(&created) {
        let _ = secrets::delete_all(&created.id);
        let _ = repo.delete(&created.id);
        return Err(CommandError {
            code: "internal",
            message: format!("adapter registration failed: {err}"),
        });
    }
    event_log.append(SyncEvent::AccountCreated(account_payload(&created)));
    // The boot warm pass may already have run; kick a warm so the new account's
    // calendars/lists load now instead of only after the next pass / restart.
    refresher.trigger();
    Ok(created)
}

#[derive(Debug, serde::Deserialize)]
pub struct ConnectMicrosoftRequest {
    pub client_id: String,
    #[serde(default)]
    pub authority: Option<String>,
    pub display_name: String,
}

/// The account form for a plugin, as its manifest declares it.
///
/// A wire copy rather than the manifest type so the frontend sees exactly what
/// it needs and nothing else — and so the built-in-credentials question is
/// answered here, where the credentials live, rather than by asking the
/// frontend to reason about a posture it cannot check.
#[derive(Debug, Serialize)]
pub struct AccountFormSpec {
    pub plugin_id: String,
    pub fields: Vec<AccountFormField>,
    /// Buttons besides "add" that this adapter offers on its form.
    #[serde(default)]
    pub actions: Vec<AccountFormAction>,
    /// Present when connecting runs an OAuth sign-in.
    pub oauth: Option<AccountFormOauth>,
    /// Whether accounts of this adapter own calendars and task lists. Derived
    /// from the plugin's declared TYPE, so the frontend can skip the catalog
    /// refresh after connecting a videoconference account — which owns neither,
    /// and whose catalog calls have a blocking cold path — without keeping its
    /// own list of which adapters those are.
    pub owns_containers: bool,
}

#[derive(Debug, Serialize)]
pub struct AccountFormField {
    pub key: String,
    /// `text` | `url` | `secret` | `bool`.
    pub kind: String,
    /// Already in the caller's language. The adapter names the field and its
    /// own catalogue supplies the words; the frontend renders what it is given
    /// and never looks a plugin's key up in the app's translations, because
    /// the app has no business carrying a word about somebody else's provider.
    pub label: String,
    pub hint: Option<String>,
    pub required: bool,
    pub default_bool: Option<bool>,
    pub default_text: Option<String>,
}

/// One button the connect form should offer, everything already in the
/// reader's language.
#[derive(Debug, Serialize)]
pub struct AccountFormAction {
    pub key: String,
    pub label: String,
    pub busy_label: Option<String>,
    pub success: Option<String>,
    pub hint: Option<String>,
    pub requires: Vec<AccountFormRequirement>,
}

#[derive(Debug, Serialize)]
pub struct AccountFormRequirement {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct AccountFormOauth {
    /// True when this build carries credentials for the provider, so the two
    /// client fields may be left blank and the form need not show them at all.
    pub builtin: bool,
    pub client_id_field: String,
    pub client_secret_field: Option<String>,
}

/// The account form a plugin declares, or `None` when it declares none.
///
/// The frontend renders whatever comes back. It holds no per-adapter knowledge,
/// and gains none when an adapter is added.
#[tauri::command]
pub fn account_form_spec(
    plugin_manager: State<'_, Arc<PluginManager>>,
    adapter_kind: AdapterKind,
    // The language to render the form's labels in — the UI's, since the person
    // reading them is the one at the keyboard. Absent means English.
    lang: Option<String>,
) -> CommandResult<Option<AccountFormSpec>> {
    use plugin_core::account_schema::{AccountFieldDefault, AccountFieldKind};
    let Some(plugin) = plugin_manager.plugin_for_adapter_kind(adapter_kind.as_str()) else {
        return Ok(None);
    };
    let Some(schema) = plugin.manifest.account.clone() else {
        return Ok(None);
    };
    let plugin_id = plugin.manifest.id.clone();
    let lang = lang.as_deref().unwrap_or(plugin_core::FALLBACK_LANG);
    let strings = PluginManager::strings_for(&plugin, lang);
    let label_of = |key: Option<&str>, verbatim: &str| {
        plugin_core::resolve_label(Some(&strings), key, verbatim, lang).to_string()
    };
    Ok(Some(AccountFormSpec {
        plugin_id,
        fields: schema
            .fields
            .iter()
            .map(|f| AccountFormField {
                key: f.key.clone(),
                kind: match f.kind {
                    AccountFieldKind::Text => "text",
                    AccountFieldKind::Url => "url",
                    AccountFieldKind::Secret => "secret",
                    AccountFieldKind::Bool => "bool",
                }
                .to_string(),
                label: label_of(f.label_key.as_deref(), &f.label),
                hint: f
                    .hint
                    .as_deref()
                    .or(f.hint_key.as_deref().map(|_| ""))
                    .map(|verbatim| label_of(f.hint_key.as_deref(), verbatim))
                    .filter(|hint| !hint.is_empty()),
                required: f.required,
                default_bool: match &f.default {
                    Some(AccountFieldDefault::Bool(b)) => Some(*b),
                    _ => None,
                },
                default_text: match &f.default {
                    Some(AccountFieldDefault::Text(t)) => Some(t.clone()),
                    _ => None,
                },
            })
            .collect(),
        actions: schema
            .actions
            .iter()
            .map(|a| AccountFormAction {
                key: a.key.clone(),
                label: label_of(a.label_key.as_deref(), &a.label),
                busy_label: a
                    .busy_label
                    .as_deref()
                    .or(a.busy_label_key.as_deref().map(|_| ""))
                    .map(|verbatim| label_of(a.busy_label_key.as_deref(), verbatim))
                    .filter(|s| !s.is_empty()),
                success: a
                    .success
                    .as_deref()
                    .or(a.success_key.as_deref().map(|_| ""))
                    .map(|verbatim| label_of(a.success_key.as_deref(), verbatim))
                    .filter(|s| !s.is_empty()),
                hint: a
                    .hint
                    .as_deref()
                    .or(a.hint_key.as_deref().map(|_| ""))
                    .map(|verbatim| label_of(a.hint_key.as_deref(), verbatim))
                    .filter(|s| !s.is_empty()),
                requires: a
                    .requires
                    .iter()
                    .map(|r| AccountFormRequirement {
                        field: r.field.clone(),
                        message: label_of(r.message_key.as_deref(), &r.message),
                    })
                    .collect(),
            })
            .collect(),
        oauth: schema.oauth.as_ref().map(|o| AccountFormOauth {
            builtin: host_core::account_setup::has_builtin_client(o),
            client_id_field: o.client_id_field.clone(),
            client_secret_field: o.client_secret_field.clone(),
        }),
        owns_containers: plugin.manifest.has_data_family(),
    }))
}

/// Every adapter this build can connect an account for.
///
/// Assembled from the loaded manifests, not from a list in the UI: which
/// adapters exist is decided by which plugins are installed, and the connect
/// picker has no business knowing that in advance. Enabling or disabling a
/// plugin changes the answer on the next call.
///
/// The host-internal kinds are NOT in here — the local store is implicit and
/// the device calendar is offered through its own OS permission flow, so each
/// frontend adds its own entry for those where it makes sense.
#[tauri::command]
pub fn list_adapter_kinds(
    plugin_manager: State<'_, Arc<PluginManager>>,
) -> CommandResult<Vec<plugin_core::AdapterKindInfo>> {
    Ok(plugin_manager.adapter_kinds())
}

#[derive(Debug, Deserialize)]
pub struct ConnectAccountRequest {
    pub adapter_kind: AdapterKind,
    pub display_name: String,
    /// The form's values, keyed by the schema's field keys. Text fields arrive
    /// as strings and checkboxes as booleans; anything else is rejected rather
    /// than coerced.
    #[serde(default)]
    pub values: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct RunAccountActionRequest {
    pub adapter_kind: AdapterKind,
    /// Which declared action. Not a verb the host knows — the adapter named it.
    pub action_key: String,
    /// The form's values so far, keyed by the schema's field keys.
    #[serde(default)]
    pub values: serde_json::Map<String, Value>,
}

/// Run one action a plugin declared on its connect form, and hand back what the
/// form should now contain.
///
/// The last place the host named an adapter was a `kind == "ews"` branch
/// rendering an Autodiscover button, with its labels in the app's own
/// translations. This is that button, generalised: the manifest says which entry
/// point to drive, which fields must be filled first, which form values become
/// which arguments, and which result keys land back in which fields. Nothing
/// here knows what Autodiscover is.
///
/// The returned map is keyed by FIELD key, ready to merge into the form. A
/// result the action did not produce is simply absent rather than blanking a
/// field the user had already typed into.
#[tauri::command]
pub async fn run_account_action(
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: RunAccountActionRequest,
) -> CommandResult<serde_json::Map<String, Value>> {
    let plugin = plugin_manager
        .plugin_for_adapter_kind(request.adapter_kind.as_str())
        .ok_or(CommandError {
            code: "unsupported",
            message: "no plugin serves this adapter kind".into(),
        })?;
    let schema = plugin.manifest.account.clone().ok_or(CommandError {
        code: "unsupported",
        message: "this adapter declares no account schema".into(),
    })?;
    let action = schema
        .action(&request.action_key)
        .cloned()
        .ok_or(CommandError {
            code: "not_found",
            message: "this adapter declares no such action".into(),
        })?;

    // The requirements are checked HERE as well as in the frontend, because the
    // frontend's copy is a courtesy that keeps the user from a pointless round
    // trip and this one is the actual gate.
    for requirement in &action.requires {
        let filled = request
            .values
            .get(&requirement.field)
            .and_then(Value::as_str)
            .is_some_and(|v| !v.trim().is_empty());
        if !filled {
            return Err(CommandError {
                code: "invalid_input",
                message: requirement.message.clone(),
            });
        }
    }

    let mut args = serde_json::Map::new();
    for (arg, field) in &action.inputs {
        if let Some(value) = request.values.get(field) {
            args.insert(arg.clone(), value.clone());
        }
    }

    let payload = match action.entry {
        plugin_core::account_schema::AccountActionEntry::Discover => {
            run_plugin_discover::<serde_json::Map<String, Value>>(
                plugin_manager.inner(),
                &plugin.manifest.id,
                Value::Object(args),
            )
            .await?
        }
    };

    // Back into the form, under the FIELD keys the manifest paired them with.
    let mut filled = serde_json::Map::new();
    for (field, result_key) in &action.fills {
        if let Some(value) = payload.get(result_key) {
            filled.insert(field.clone(), value.clone());
        }
    }
    Ok(filled)
}

#[derive(Debug, Deserialize)]
pub struct TestAccountRequest {
    pub adapter_kind: AdapterKind,
    /// The form's values, keyed by the schema's field keys — the same shape
    /// [`ConnectAccountRequest`] carries, so what is tested is what would be
    /// connected.
    #[serde(default)]
    pub values: serde_json::Map<String, Value>,
}

/// Round-trip the entered credentials without persisting anything.
///
/// The one test path that does not grow a branch per adapter, and the desktop
/// twin of the mobile `test_account_json`. It splits the form's values exactly
/// as `connect_account` would — same schema, same code — and hands the result to
/// the registry's probe. Testing and connecting can therefore not disagree about
/// what a field means, which five separate per-kind commands could and did.
///
/// Nothing here names a provider. An adapter Aperio has never seen is testable
/// the moment it declares a schema.
#[tauri::command]
pub async fn test_account(
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: TestAccountRequest,
) -> CommandResult<()> {
    let plugin = plugin_manager
        .plugin_for_adapter_kind(request.adapter_kind.as_str())
        .ok_or(CommandError {
            code: "unsupported",
            message: "no plugin serves this adapter kind".into(),
        })?;
    let schema = plugin.manifest.account.clone().ok_or(CommandError {
        code: "unsupported",
        message: "this adapter declares no account schema".into(),
    })?;
    // No OAuth client choice: a probe never signs in. An adapter that can only
    // be reached with a token the sign-in produces has nothing to test before
    // the account exists, and says so by failing the probe rather than by a
    // special case here.
    let plan = host_core::account_setup::plan_new_account(&schema, &request.values, None).map_err(
        |err| CommandError {
            code: "invalid_input",
            message: err.to_string(),
        },
    )?;
    // At most one credential reaches a probe. A schema with several would need
    // the registry to take them all, which no adapter has asked for yet.
    let secret = plan.secrets.first().map(|(_, value)| value.as_str());
    registry
        .probe_account(&request.adapter_kind, &plan.config_json, secret)
        .await
        .map_err(|err| CommandError {
            code: "probe_failed",
            message: err.to_string(),
        })
}

/// Create an account for any plugin that declares an account schema.
///
/// The one connect path that does not grow a branch per adapter. It reads the
/// schema, runs the OAuth sign-in if there is one, splits the collected values
/// into the non-secret row and the keychain writes the schema asked for, and
/// registers the adapter. Nothing in this function names a provider.
///
/// Best-effort transactional in the same way the older per-adapter commands
/// are: any failure after the row exists tears down everything written so far,
/// so a retry starts clean rather than from a half-connected account that fails
/// later for an unrelated-looking reason.
#[tauri::command]
pub async fn connect_account(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    refresher: State<'_, Arc<CacheRefresher>>,
    request: ConnectAccountRequest,
) -> CommandResult<Account> {
    let name = request.display_name.trim();
    if name.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "display_name must not be empty".into(),
        });
    }
    let plugin = plugin_manager
        .plugin_for_adapter_kind(request.adapter_kind.as_str())
        .ok_or(CommandError {
            code: "invalid_input",
            message: "no plugin serves this adapter kind".into(),
        })?;
    let plugin_id = plugin.manifest.id.clone();
    let schema = plugin.manifest.account.clone().ok_or(CommandError {
        code: "unsupported",
        message: "this adapter declares no account schema".into(),
    })?;

    // 1) The OAuth sign-in, if the schema has one — before anything persistent
    //    is touched, so a denied or abandoned consent leaves nothing behind.
    let mut oauth_choice = None;
    let mut tokens: Option<Value> = None;
    if let Some(oauth) = &schema.oauth {
        let supplied = |key: &str| -> Option<String> {
            request
                .values
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let supplied_id = supplied(&oauth.client_id_field);
        let supplied_secret = oauth.client_secret_field.as_deref().and_then(supplied);
        let choice = host_core::account_setup::choose_oauth_client(
            oauth,
            supplied_id.as_deref(),
            supplied_secret.as_deref(),
        )
        .map_err(account_setup_error)?;
        let mut args = serde_json::Map::new();
        args.insert("client_id".into(), Value::String(choice.client.id.clone()));
        if let Some(secret) = &choice.client.secret {
            args.insert("client_secret".into(), Value::String(secret.clone()));
        }
        tokens =
            Some(run_plugin_auth(plugin_manager.inner(), &plugin_id, Value::Object(args)).await?);
        oauth_choice = Some(choice);
    }

    // 2) Split the form into the row and the keychain writes.
    let mut plan =
        host_core::account_setup::plan_new_account(&schema, &request.values, oauth_choice.as_ref())
            .map_err(account_setup_error)?;

    // 3) Tokens the sign-in produced join the keychain writes. The refresh
    //    token is the durable credential; an access token is kept only when the
    //    plugin asked to be handed one.
    let mut refresh_for_sync = None;
    if let (Some(oauth), Some(tokens)) = (&schema.oauth, &tokens) {
        if oauth.refresh_token_field.is_some() {
            let refresh = tokens
                .get("refresh_token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or(CommandError {
                    code: "protocol",
                    message: "the provider returned no refresh token — the account could not be \
                              kept signed in"
                        .into(),
                })?
                .to_string();
            plan.secrets
                .push((SecretSlot::RefreshToken, refresh.clone()));
            refresh_for_sync = Some(refresh);
        }
        if oauth.access_token_field.is_some() {
            if let Some(access) = tokens.get("access_token").and_then(Value::as_str) {
                plan.secrets
                    .push((SecretSlot::AccessToken, access.to_string()));
            }
        }
    }

    // 4) Persist: row, then secrets, then registration — unwinding all of it on
    //    any failure.
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let created = repo.create(request.adapter_kind.clone(), name, &plan.config_json)?;
    for (slot, value) in &plan.secrets {
        if let Err(err) = secrets::store(&created.id, *slot, value) {
            let _ = secrets::delete_all(&created.id);
            let _ = repo.delete(&created.id);
            return Err(CommandError {
                code: "internal",
                message: format!("failed to store {}: {err}", slot.wire_name()),
            });
        }
    }
    // E2E only: carry the durable refresh token to the user's other devices.
    // Nothing else is synced — an OAuth client secret belongs to the build or to
    // the user's own registration, and every device resolves its own.
    if let Some(refresh) = refresh_for_sync {
        crate::credential_sync::emit_credential_set(
            &event_log,
            &shared,
            &created.id,
            SecretSlot::RefreshToken,
            &refresh,
        );
    }
    if let Err(err) = registry.register(&created) {
        let _ = secrets::delete_all(&created.id);
        let _ = repo.delete(&created.id);
        return Err(CommandError {
            code: "internal",
            message: format!("adapter registration failed: {err}"),
        });
    }
    event_log.append(SyncEvent::AccountCreated(account_payload(&created)));

    // A warm pass only has something to fetch when the account owns containers.
    // A videoconference account owns none, and the catalog calls have a
    // blocking cold path.
    let owns_containers = plugin.manifest.has_data_family();
    if owns_containers {
        refresher.trigger();
    }
    Ok(created)
}

/// Map a setup failure onto the command envelope, keeping the three meanings
/// apart: something the user can fix, a row that no longer makes sense, and a
/// credential store that would not answer.
fn account_setup_error(err: host_core::account_setup::AccountSetupError) -> CommandError {
    use host_core::account_setup::AccountSetupError as E;
    match err {
        E::InvalidInput(message) | E::Config(message) => CommandError {
            code: "invalid_input",
            message,
        },
        E::Secret(message) => CommandError {
            code: "internal",
            message,
        },
    }
}

// google_error_to_command + graph_error_to_command moved into
// the plugins themselves — the OAuth dance now runs plugin-side,
// the typed adapter error enums never cross into the host. The
// plugin's `Err(String)` from interactive_auth flows through
// `interactive_auth_error_to_command` above as `code: "auth"`
// with the message preserved verbatim. The localised German
// "Verbindung abgelehnt" / "Anmeldung hat zu lange gedauert"
// text is gone for now; a follow-up could either re-thread the
// typed enum across the FFI boundary (extending the
// InteractiveAuthError variants) or move the i18n layer onto
// the frontend.

// ---------------------------------------------------------------------------
// §19.11.8 reconnect commands — re-attach credentials to existing
// account rows pulled in via the snapshot's `accounts` section. The
// onboarding wizard ("Konten verbinden") drives these.
// ---------------------------------------------------------------------------

/// Re-attach a password / API token to an existing account row.
/// Used by the onboarding wizard for password-based backends
/// (CalDAV, iCal-with-auth, EWS, Vikunja, Todoist). The
/// `account_id` MUST already exist in the `accounts` table —
/// snapshots populate it; this command only fills in the
/// missing keychain slot.
///
/// Slot selection mirrors `create_account` + `required_secret_slot`:
/// Vikunja / Todoist get `ApiToken`, everyone else `Password`.
///
/// After the secret lands we register the adapter so subsequent
/// reads / writes route through it without an app restart.
#[tauri::command]
pub async fn set_account_secret(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    account_id: String,
    secret: String,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let account = repo.get(&account_id)?.ok_or(CommandError {
        code: "not_found",
        message: format!("account {account_id} not found"),
    })?;
    if account.adapter_kind == "local" {
        return Err(CommandError {
            code: "invalid_input",
            message: "the local account has no credential slot".into(),
        });
    }
    if matches!(account.adapter_kind.as_str(), "google" | "microsoft_graph") {
        return Err(CommandError {
            code: "invalid_input",
            message: format!(
                "OAuth accounts (kind={}) must use the dedicated reconnect command",
                account.adapter_kind.as_str(),
            ),
        });
    }
    let slot = match account.adapter_kind.as_str() {
        "vikunja" | "todoist" => SecretSlot::ApiToken,
        _ => SecretSlot::Password,
    };
    secrets::store(&account_id, slot, &secret).map_err(|err| CommandError {
        code: "internal",
        message: format!("failed to store credential: {err}"),
    })?;
    // E2E only: propagate the (re-)entered secret to other devices.
    crate::credential_sync::emit_credential_set(&event_log, &shared, &account_id, slot, &secret);
    // Register so the adapter is live for the rest of this
    // session. A registration failure leaves the secret in place
    // — the user can retry without re-typing the password.
    if let Err(err) = registry.register(&account) {
        return Err(CommandError {
            code: "internal",
            message: format!("adapter registration failed: {err}"),
        });
    }
    Ok(())
}

/// Re-run the Google OAuth flow against an existing account row.
/// Reads the persisted `client_id` / `client_secret` from
/// `config_json`, opens the system browser, and writes the fresh
/// tokens under the EXISTING account id — preserving the
/// downstream calendar / task list / event rows that reference
/// it. Subsequent reads / writes route through the
/// freshly-registered adapter without an app restart.
#[tauri::command]
pub async fn reconnect_google_account(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    account_id: String,
) -> CommandResult<()> {
    reconnect_oauth_account(
        db.inner(),
        registry.inner(),
        event_log.inner(),
        plugin_manager.inner(),
        &account_id,
        AdapterKind::new("google"),
        PLUGIN_ID_GOOGLE,
        "Google",
        |config| {
            json!({
                "client_id": config.get("client_id").cloned().unwrap_or(Value::Null),
                "client_secret": config
                    .get("client_secret")
                    .cloned()
                    .unwrap_or(Value::Null),
            })
        },
    )
    .await
}

/// Microsoft equivalent of [`reconnect_google_account`].
/// Re-runs the PKCE-only public-client OAuth flow with the
/// persisted `client_id` / `authority` and writes fresh tokens
/// against the existing account row.
#[tauri::command]
pub async fn reconnect_microsoft_account(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    account_id: String,
) -> CommandResult<()> {
    reconnect_oauth_account(
        db.inner(),
        registry.inner(),
        event_log.inner(),
        plugin_manager.inner(),
        &account_id,
        AdapterKind::new("microsoft_graph"),
        PLUGIN_ID_GRAPH,
        "Microsoft Graph",
        |config| {
            json!({
                "client_id": config.get("client_id").cloned().unwrap_or(Value::Null),
                "authority": config
                    .get("authority")
                    .cloned()
                    .unwrap_or(Value::String("common".into())),
            })
        },
    )
    .await
}

/// Shared re-OAuth flow for Google + Microsoft Graph accounts.
/// Pulls the persisted config off the row, hands the plugin the
/// just-the-OAuth-inputs subset (via `build_args`), persists
/// the fresh tokens under the existing account id, and
/// re-registers the adapter so subsequent reads route through
/// the new credentials without an app restart.
async fn reconnect_oauth_account<F>(
    db: &DbHandle,
    registry: &AdapterRegistry,
    event_log: &EventLogWriter,
    plugin_manager: &PluginManager,
    account_id: &str,
    expected_kind: AdapterKind,
    plugin_id: &str,
    plugin_label: &str,
    build_args: F,
) -> CommandResult<()>
where
    F: FnOnce(&Value) -> Value,
{
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let account = repo.get(account_id)?.ok_or(CommandError {
        code: "not_found",
        message: format!("account {account_id} not found"),
    })?;
    if account.adapter_kind != expected_kind {
        return Err(CommandError {
            code: "invalid_input",
            message: format!("account is not a {plugin_label} account",),
        });
    }
    let config: Value = serde_json::from_str(&account.config_json).map_err(|err| CommandError {
        code: "internal",
        message: format!("parse {plugin_label} config: {err}"),
    })?;
    let args = build_args(&config);
    let tokens = run_plugin_auth(plugin_manager, plugin_id, args).await?;

    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or(CommandError {
            code: "protocol",
            message: format!("{plugin_label} plugin returned no access_token"),
        })?;
    secrets::store(&account.id, SecretSlot::AccessToken, access).map_err(|err| CommandError {
        code: "internal",
        message: format!("failed to store access token: {err}"),
    })?;
    if let Some(refresh) = tokens.get("refresh_token").and_then(Value::as_str) {
        secrets::store(&account.id, SecretSlot::RefreshToken, refresh).map_err(|err| {
            CommandError {
                code: "internal",
                message: format!("failed to store refresh token: {err}"),
            }
        })?;
        // E2E only: propagate the refreshed durable token to other devices.
        crate::credential_sync::emit_credential_set(
            event_log,
            &shared,
            &account.id,
            SecretSlot::RefreshToken,
            refresh,
        );
    }
    if let Err(err) = registry.register(&account) {
        return Err(CommandError {
            code: "internal",
            message: format!("adapter registration failed: {err}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §19.11.8 — the missing-credentials check picks the right
    /// keychain slot per `AdapterKind`. Drives the dialog's
    /// password vs API-token vs OAuth branching. iCal + Local
    /// return `None` because they have no required secret —
    /// iCal feeds are typically public URLs.
    #[test]
    fn required_secret_slot_maps_each_kind() {
        // Password-based providers.
        assert!(matches!(
            required_secret_slot(&AdapterKind::new("caldav")),
            Some(SecretSlot::Password)
        ));
        assert!(matches!(
            required_secret_slot(&AdapterKind::new("ews")),
            Some(SecretSlot::Password)
        ));
        // No-secret providers — iCal feeds are public; Local
        // is host-internal; the device-calendar account uses the OS
        // permission grant, not a stored secret (so it must never show
        // the "credentials missing" repair banner).
        assert!(required_secret_slot(&AdapterKind::new("ical")).is_none());
        assert!(required_secret_slot(&AdapterKind::new("local")).is_none());
        assert!(required_secret_slot(&AdapterKind::new("device_calendar")).is_none());
        // API-token providers — surfaced as "API token" in the UI.
        assert!(matches!(
            required_secret_slot(&AdapterKind::new("vikunja")),
            Some(SecretSlot::ApiToken)
        ));
        assert!(matches!(
            required_secret_slot(&AdapterKind::new("todoist")),
            Some(SecretSlot::ApiToken)
        ));
        // OAuth providers — slot we probe for "is the user
        // signed in" is the refresh token, since the access
        // token rotates on its own.
        assert!(matches!(
            required_secret_slot(&AdapterKind::new("google")),
            Some(SecretSlot::RefreshToken)
        ));
        assert!(matches!(
            required_secret_slot(&AdapterKind::new("microsoft_graph")),
            Some(SecretSlot::RefreshToken)
        ));
    }
}
