//! Account management commands (DESIGN.md §6.2 + §6.4).

use cal_adapter_caldav::{
    config::{AuthKind, CaldavAccountConfig, Credentials as CaldavCredentials},
    CaldavAdapter,
};
use cal_adapter_google::{GoogleAccountConfig, GoogleAdapter};
use cal_adapter_ical::{
    Credentials as IcalCredentials, IcalAccountConfig, IcalAdapter,
};
use cal_core::CalendarFeature;
use tauri::State;

use super::{CommandError, CommandResult};
use crate::accounts::{Account, AccountsError, AccountsRepo, AdapterKind};
use crate::db::DbHandle;
use crate::registry::AdapterRegistry;
use crate::secrets::{self, SecretSlot};

#[tauri::command]
pub async fn list_accounts(
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<Account>> {
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    Ok(repo.list()?)
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
    registry: State<'_, AdapterRegistry>,
    request: CreateAccountRequest,
) -> CommandResult<Account> {
    // Reject adapter kinds we have no construction path for yet.
    // Local, CalDAV and iCal are the supported kinds — the others
    // surface as an actionable "coming soon" envelope rather than
    // a half-broken row in the database.
    if !matches!(
        request.adapter_kind,
        AdapterKind::Local | AdapterKind::Caldav | AdapterKind::Ical
    ) {
        return Err(CommandError {
            code: "unsupported",
            message: format!(
                "Adapter '{}' will be supported in a later phase.",
                request.adapter_kind.as_str()
            ),
        });
    }

    // For CalDAV we smoke-test the credentials *before* writing
    // anything so the user sees auth / network errors instantly
    // instead of "saved, but doesn't work". The test runs against
    // an ephemeral adapter built from the request payload; the
    // real adapter is constructed again later from the persisted
    // config so the request and the stored shape stay in sync.
    if request.adapter_kind == AdapterKind::Caldav {
        let Some(secret) = request.secret.as_deref() else {
            return Err(CommandError {
                code: "invalid_input",
                message: "CalDAV needs a password to authenticate.".into(),
            });
        };
        let config: CaldavAccountConfig = serde_json::from_str(&request.config_json)
            .map_err(|e| CommandError {
                code: "invalid_input",
                message: format!("invalid CalDAV config: {e}"),
            })?;
        smoke_test_caldav(&config, secret).await?;
    }

    // Same idea for iCal: a one-shot HEAD/GET against the feed URL
    // confirms the URL is reachable and (if Basic auth is provided)
    // the credentials are accepted. Public feeds run anonymously.
    if request.adapter_kind == AdapterKind::Ical {
        let config: IcalAccountConfig = serde_json::from_str(&request.config_json)
            .map_err(|e| CommandError {
                code: "invalid_input",
                message: format!("invalid iCal config: {e}"),
            })?;
        smoke_test_ical(&config, request.secret.as_deref()).await?;
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
    if let Some(secret) = request.secret {
        if let Err(err) = secrets::store(&created.id, SecretSlot::Password, &secret)
        {
            let _ = repo.delete(&created.id);
            return Err(CommandError {
                code: "internal",
                message: format!("failed to store credential: {err}"),
            });
        }
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
    Ok(created)
}

/// Discover + list calendars against the supplied CalDAV
/// credentials. The result is discarded; this command exists
/// purely to surface a clear "credentials work?" answer ahead of
/// persisting anything.
async fn smoke_test_caldav(
    config: &CaldavAccountConfig,
    secret: &str,
) -> Result<(), CommandError> {
    let credentials = CaldavCredentials::new(
        CaldavAccountConfig {
            server_url: config.server_url.clone(),
            username: config.username.clone(),
            auth_kind: config.auth_kind,
        },
        secret.to_string(),
    );
    let adapter = CaldavAdapter::new(credentials, None).map_err(|err| CommandError {
        code: "internal",
        message: err.to_string(),
    })?;
    // A successful list_calendars implies discovery + auth + at
    // least one PROPFIND round-trip worked.
    adapter
        .list_calendars()
        .await
        .map_err(|err| caldav_core_error_to_command(err))?;
    Ok(())
}

/// One-shot fetch of the iCal feed. Confirms the URL resolves, the
/// server answers, and (if credentials are provided) Basic auth is
/// accepted. The ephemeral adapter is dropped after the call — the
/// real one gets constructed again from the persisted config so the
/// request and storage stay in sync.
async fn smoke_test_ical(
    config: &IcalAccountConfig,
    password: Option<&str>,
) -> Result<(), CommandError> {
    let credentials = IcalCredentials::new(
        IcalAccountConfig {
            feed_url: config.feed_url.clone(),
            username: config.username.clone(),
        },
        password.filter(|s| !s.is_empty()).map(|s| s.to_string()),
    );
    let adapter = IcalAdapter::new(credentials).map_err(|err| CommandError {
        code: "invalid_input",
        message: err.to_string(),
    })?;
    adapter
        .smoke_test()
        .await
        .map_err(|err| ical_error_to_command(err))?;
    Ok(())
}

fn ical_error_to_command(err: cal_adapter_ical::IcalError) -> CommandError {
    use cal_adapter_ical::IcalError::*;
    let (code, message) = match err {
        Auth(_) => ("auth", err.to_string()),
        Url(_) | Config(_) => ("invalid_input", err.to_string()),
        Parse(_) => ("protocol", err.to_string()),
        Server(_) | Network(_) => ("network", err.to_string()),
    };
    CommandError { code, message }
}

fn caldav_core_error_to_command(err: cal_core::Error) -> CommandError {
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
    request: TestCaldavRequest,
) -> CommandResult<()> {
    let config = CaldavAccountConfig {
        server_url: request.server_url,
        username: request.username,
        auth_kind: AuthKind::Basic,
    };
    smoke_test_caldav(&config, &request.password).await?;
    Ok(())
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
    request: TestIcalRequest,
) -> CommandResult<()> {
    let config = IcalAccountConfig {
        feed_url: request.feed_url,
        username: request.username,
    };
    smoke_test_ical(&config, request.password.as_deref()).await?;
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
pub struct TestIcalRequest {
    pub feed_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[tauri::command]
pub async fn delete_account(
    db: State<'_, DbHandle>,
    registry: State<'_, AdapterRegistry>,
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
    Ok(())
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
    registry: State<'_, AdapterRegistry>,
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

    // 1) Run the OAuth dance. This opens the system browser and
    //    blocks the command until the user completes consent (or
    //    times out at 5 min, or the user denies). Errors here are
    //    surfaced verbatim — we haven't touched anything persistent
    //    yet.
    let tokens =
        GoogleAdapter::authenticate_interactive(client_id, client_secret)
            .await
            .map_err(google_error_to_command)?;

    // 2) Create the account row. Config carries client_id +
    //    client_secret (the latter is what Google's docs themselves
    //    say "is not treated as a secret" for installed apps — see
    //    the GoogleAccountConfig type docs).
    let config = GoogleAccountConfig {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
        account_label: None,
    };
    let config_json = serde_json::to_string(&config).map_err(|e| CommandError {
        code: "internal",
        message: format!("serialise config: {e}"),
    })?;
    let shared = db.shared();
    let repo = AccountsRepo::new(&shared);
    let created = repo.create(AdapterKind::Google, name, &config_json)?;

    // 3) Persist tokens to the keychain. If either write fails we
    //    delete the row and surface an error so the user can retry
    //    with a clean slate.
    if let Err(err) =
        secrets::store(&created.id, SecretSlot::AccessToken, &tokens.access_token)
    {
        let _ = repo.delete(&created.id);
        return Err(CommandError {
            code: "internal",
            message: format!("failed to store access token: {err}"),
        });
    }
    if let Some(refresh) = tokens.refresh_token.as_deref() {
        if let Err(err) = secrets::store(&created.id, SecretSlot::RefreshToken, refresh)
        {
            let _ = secrets::delete_all(&created.id);
            let _ = repo.delete(&created.id);
            return Err(CommandError {
                code: "internal",
                message: format!("failed to store refresh token: {err}"),
            });
        }
    }

    // 4) Register the adapter so subsequent reads/writes route
    //    through it. The registry reads tokens from keychain again
    //    — round-trip is intentional so the read-path stays
    //    identical to what happens at app boot.
    if let Err(err) = registry.register(&created) {
        let _ = secrets::delete_all(&created.id);
        let _ = repo.delete(&created.id);
        return Err(CommandError {
            code: "internal",
            message: format!("adapter registration failed: {err}"),
        });
    }
    Ok(created)
}

#[derive(Debug, serde::Deserialize)]
pub struct ConnectGoogleRequest {
    pub client_id: String,
    pub client_secret: String,
    pub display_name: String,
}

fn google_error_to_command(err: cal_adapter_google::GoogleError) -> CommandError {
    use cal_adapter_google::GoogleError::*;
    let (code, message) = match err {
        AuthDenied(m) => ("auth", format!("Verbindung abgelehnt: {m}")),
        AuthTimeout => (
            "auth",
            "Anmeldung hat zu lange gedauert (5 min). Bitte erneut versuchen.".into(),
        ),
        Http { status: 401, message } | Http { status: 403, message } => {
            ("auth", message)
        }
        Http { status, message } => (
            "protocol",
            format!("Google HTTP {status}: {message}"),
        ),
        Network(m) => ("network", m),
        Protocol(m) => ("protocol", m),
        Csrf => ("protocol", "CSRF-Mismatch bei OAuth-Redirect".into()),
        Io(m) => ("internal", m),
        Config(m) => ("invalid_input", m),
    };
    CommandError { code, message }
}
