//! Account management commands (DESIGN.md §6.2 + §6.4).

use cal_core::{CalendarFeature, TasksFeature};
use plugin_core::shim::{FfiCalendarAdapter, FfiTasksAdapter};
use plugin_core::PluginManager;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::State;

use super::{
    plugin_id_for_adapter_kind, run_plugin_auth, run_plugin_discover, CommandError, CommandResult,
};
use crate::accounts::{Account, AccountsError, AccountsRepo, AdapterKind};
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
        if acc.id == "local" || acc.adapter_kind == AdapterKind::Local {
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
            let plugin_loaded = match plugin_id_for_adapter_kind(account.adapter_kind) {
                // Local accounts have no plugin to look up —
                // they're host-internal and always available.
                None => true,
                Some(plugin_id) => plugin_manager.is_enabled(plugin_id),
            };
            AccountListEntry {
                account,
                plugin_loaded,
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
) -> CommandResult<Vec<Account>> {
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let all = repo.list()?;
    let mut out = Vec::new();
    for acc in all {
        if acc.id == "local" || acc.adapter_kind == AdapterKind::Local {
            continue;
        }
        let Some(slot) = required_secret_slot(acc.adapter_kind) else {
            continue;
        };
        if !secret_present(&acc.id, slot) {
            out.push(acc);
        }
    }
    Ok(out)
}

/// Which keychain slot a fully-configured account of this kind
/// must have populated. `None` means the adapter has no required
/// secret (iCal feeds are typically public URLs; an optional
/// Basic-auth password fails open and surfaces as a 401 on the
/// first fetch). Kept separate from the connect-side logic so
/// the two stay consistent: any future adapter that needs a
/// different secret shape adds itself here too.
fn required_secret_slot(kind: AdapterKind) -> Option<SecretSlot> {
    match kind {
        // No stored credential: iCal feeds are public, Local is host-internal,
        // and the mobile-only device-calendar account authenticates via the OS
        // permission grant (never reaches desktop, but the kind is shared).
        AdapterKind::Ical | AdapterKind::Local | AdapterKind::DeviceCalendar => None,
        AdapterKind::Vikunja | AdapterKind::Todoist => Some(SecretSlot::ApiToken),
        AdapterKind::Google | AdapterKind::MicrosoftGraph => Some(SecretSlot::RefreshToken),
        _ => Some(SecretSlot::Password),
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
    request: CreateAccountRequest,
) -> CommandResult<Account> {
    // Reject adapter kinds we have no construction path for yet.
    // Local, CalDAV, iCal and EWS go through the `create_account`
    // dispatch (each with a password-style credential); Google and
    // Microsoft Graph have their own OAuth-shaped entry points. The
    // remaining kinds surface as an actionable "coming soon" envelope
    // rather than a half-broken row in the database.
    if !matches!(
        request.adapter_kind,
        AdapterKind::Local
            | AdapterKind::Caldav
            | AdapterKind::Ical
            | AdapterKind::Ews
            | AdapterKind::Vikunja
            | AdapterKind::Todoist
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

    if request.adapter_kind == AdapterKind::Caldav {
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

    if request.adapter_kind == AdapterKind::Ical {
        smoke_test_ical(
            plugin_manager_ref,
            str_field("feed_url"),
            request_config.get("username").and_then(Value::as_str),
            request.secret.as_deref(),
        )
        .await?;
    }

    if request.adapter_kind == AdapterKind::Ews {
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

    if request.adapter_kind == AdapterKind::Vikunja {
        let Some(secret) = request.secret.as_deref() else {
            return Err(CommandError {
                code: "invalid_input",
                message: "Vikunja needs an API token to authenticate.".into(),
            });
        };
        smoke_test_vikunja(plugin_manager_ref, str_field("server_url"), secret).await?;
    }

    if request.adapter_kind == AdapterKind::Todoist {
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
        request.adapter_kind,
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
        let slot = match request.adapter_kind {
            AdapterKind::Vikunja | AdapterKind::Todoist => SecretSlot::ApiToken,
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
    if request.adapter_kind != AdapterKind::Local {
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
            AccountsError::UnknownKind(msg) => CommandError {
                code: "invalid_input",
                message: format!("unknown adapter kind: {msg}"),
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
    let created = repo.create(AdapterKind::Google, name, &config_json)?;

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
    let created = repo.create(AdapterKind::MicrosoftGraph, name, &config_json)?;

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
    Ok(created)
}

#[derive(Debug, serde::Deserialize)]
pub struct ConnectMicrosoftRequest {
    pub client_id: String,
    #[serde(default)]
    pub authority: Option<String>,
    pub display_name: String,
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
    if account.adapter_kind == AdapterKind::Local {
        return Err(CommandError {
            code: "invalid_input",
            message: "the local account has no credential slot".into(),
        });
    }
    if matches!(
        account.adapter_kind,
        AdapterKind::Google | AdapterKind::MicrosoftGraph,
    ) {
        return Err(CommandError {
            code: "invalid_input",
            message: format!(
                "OAuth accounts (kind={}) must use the dedicated reconnect command",
                account.adapter_kind.as_str(),
            ),
        });
    }
    let slot = match account.adapter_kind {
        AdapterKind::Vikunja | AdapterKind::Todoist => SecretSlot::ApiToken,
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
        AdapterKind::Google,
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
        AdapterKind::MicrosoftGraph,
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
            required_secret_slot(AdapterKind::Caldav),
            Some(SecretSlot::Password)
        ));
        assert!(matches!(
            required_secret_slot(AdapterKind::Ews),
            Some(SecretSlot::Password)
        ));
        // No-secret providers — iCal feeds are public; Local
        // is host-internal; the device-calendar account uses the OS
        // permission grant, not a stored secret (so it must never show
        // the "credentials missing" repair banner).
        assert!(required_secret_slot(AdapterKind::Ical).is_none());
        assert!(required_secret_slot(AdapterKind::Local).is_none());
        assert!(required_secret_slot(AdapterKind::DeviceCalendar).is_none());
        // API-token providers — surfaced as "API token" in the UI.
        assert!(matches!(
            required_secret_slot(AdapterKind::Vikunja),
            Some(SecretSlot::ApiToken)
        ));
        assert!(matches!(
            required_secret_slot(AdapterKind::Todoist),
            Some(SecretSlot::ApiToken)
        ));
        // OAuth providers — slot we probe for "is the user
        // signed in" is the refresh token, since the access
        // token rotates on its own.
        assert!(matches!(
            required_secret_slot(AdapterKind::Google),
            Some(SecretSlot::RefreshToken)
        ));
        assert!(matches!(
            required_secret_slot(AdapterKind::MicrosoftGraph),
            Some(SecretSlot::RefreshToken)
        ));
    }
}
