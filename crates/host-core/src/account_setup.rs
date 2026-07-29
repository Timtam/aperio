//! Executing an adapter's [`AccountSchema`] — creating an account from it, and
//! opening one against it.
//!
//! Nothing here names an adapter. Every provider-specific fact — which fields
//! exist, which are secrets, which keychain slot each belongs in, whether there
//! is an OAuth flow and what its client is called — arrives in the schema the
//! plugin published in its `plugin.json`. The host's part is the half that is
//! the same for everyone: collecting values, keeping secrets out of the column
//! that syncs in the clear, resolving which OAuth client to sign in as, and
//! handing the plugin back exactly the init config it asked for.
//!
//! Three entry points, in the order they run:
//!
//!  1. [`choose_oauth_client`] — at connect time, decide whether this is the
//!     build's own registration or the user's.
//!  2. [`plan_new_account`] — split what the user entered into the non-secret
//!     `config_json` and the list of keychain writes.
//!  3. [`init_config`] — at open time, merge the keychain values and the OAuth
//!     client back in, under the keys the plugin named.

use plugin_core::account_schema::{
    AccountFieldDefault, AccountFieldKind, AccountOauth, AccountSchema, AccountSecretSlot,
};
use serde_json::{Map, Value};
use sync_engine::{SecretError, SecretSlot};

/// Key in `config_json` recording that an account signs in with the build's own
/// OAuth registration rather than one the user supplied. Host bookkeeping — it
/// is stripped before the plugin ever sees the config.
pub const CLIENT_SOURCE_KEY: &str = "client_source";
/// The only value [`CLIENT_SOURCE_KEY`] takes.
pub const CLIENT_SOURCE_BUILTIN: &str = "builtin";
/// Digest naming *which* built-in registration an account was linked to.
pub const CLIENT_FINGERPRINT_KEY: &str = "client_fingerprint";

/// Keys the host writes into `config_json` for itself. Stripped from the init
/// config so a plugin is never handed bookkeeping it did not declare.
const HOST_KEYS: [&str; 2] = [CLIENT_SOURCE_KEY, CLIENT_FINGERPRINT_KEY];

/// Why an account could not be set up or opened.
///
/// Three variants because they mean three different things to whoever is
/// looking: something the user can fix in the form they are looking at, a
/// persisted row that no longer makes sense, and a credential store that would
/// not answer.
#[derive(Debug)]
pub enum AccountSetupError {
    /// Bad or missing input, at the moment the user is being asked for it.
    InvalidInput(String),
    /// The stored row does not describe an account this build can open.
    Config(String),
    /// A credential exists in principle but could not be read or written.
    Secret(String),
}

impl std::fmt::Display for AccountSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(m) | Self::Config(m) | Self::Secret(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for AccountSetupError {}

type Result<T> = std::result::Result<T, AccountSetupError>;

/// Map a schema's slot onto the host's. Total by construction — the schema
/// enum has no variant for the end-to-end encryption key, so there is no case
/// here to get wrong.
pub fn host_slot(slot: AccountSecretSlot) -> SecretSlot {
    match slot {
        AccountSecretSlot::AccessToken => SecretSlot::AccessToken,
        AccountSecretSlot::RefreshToken => SecretSlot::RefreshToken,
        AccountSecretSlot::Password => SecretSlot::Password,
        AccountSecretSlot::ApiToken => SecretSlot::ApiToken,
        AccountSecretSlot::OauthClientSecret => SecretSlot::OauthClientSecret,
    }
}

// ── 1. Which OAuth client ────────────────────────────────────────────────────

/// The client id and secret to sign in with, once settled.
///
/// No `Debug`: it holds a secret, and a type that can format itself is one that
/// eventually appears in a log line.
pub struct OauthClient {
    pub id: String,
    /// `None` for a public PKCE client, whose provider issues tokens without
    /// one.
    pub secret: Option<String>,
}

/// What [`choose_oauth_client`] settled on, for a caller about to create a row.
pub struct OauthClientChoice {
    pub client: OauthClient,
    /// Keys to merge into the new row's `config_json`. A built-in account gets
    /// the source marker and a fingerprint and NO client id; a bring-your-own
    /// account gets the id under the field name the schema declared.
    pub config: Vec<(String, Value)>,
    /// Whether the secret is the user's and therefore has to be kept. False for
    /// the built-in posture, which persists no credential at all.
    pub persist_secret: bool,
}

/// Decide which OAuth client a NEW account signs in as.
///
/// Both credential fields left blank means "use whatever this build carries".
/// Both filled means "use mine". Half a pair is an error rather than a silent
/// fallback: someone who typed a client id and left the secret blank meant to
/// use their own registration, and quietly signing them in as Aperio would link
/// the account to a credential they did not choose and cannot rotate, with
/// nothing visible to say so.
///
/// A provider that needs no secret (a public PKCE client, so the schema
/// declares no `client_secret_field`) still takes this path — it simply has one
/// half to consider instead of two.
pub fn choose_oauth_client(
    oauth: &AccountOauth,
    supplied_id: Option<&str>,
    supplied_secret: Option<&str>,
) -> Result<OauthClientChoice> {
    let id = supplied_id.unwrap_or_default().trim();
    let secret = supplied_secret.unwrap_or_default().trim();
    let wants_secret = oauth.client_secret_field.is_some();

    if !id.is_empty() {
        if wants_secret && secret.is_empty() {
            return Err(AccountSetupError::InvalidInput(format!(
                "`{}` is required alongside the client ID — this provider issues no token \
                 without it",
                oauth
                    .client_secret_field
                    .as_deref()
                    .unwrap_or("client_secret")
            )));
        }
        return Ok(OauthClientChoice {
            client: OauthClient {
                id: id.to_string(),
                secret: wants_secret.then(|| secret.to_string()),
            },
            config: vec![(oauth.client_id_field.clone(), Value::String(id.to_string()))],
            persist_secret: wants_secret,
        });
    }
    if !secret.is_empty() {
        return Err(AccountSetupError::InvalidInput(
            "A client ID is required alongside the secret.".into(),
        ));
    }

    // Neither half supplied — the built-in posture, if this build has one.
    let provider = oauth
        .builtin_provider
        .as_deref()
        .and_then(builtin_oauth::Provider::parse)
        .ok_or_else(missing_builtin)?;
    let client = builtin_oauth::builtin_client(provider).ok_or_else(missing_builtin)?;
    let fingerprint = client.fingerprint().as_str().to_string();
    Ok(OauthClientChoice {
        client: OauthClient {
            id: client.client_id.to_string(),
            secret: client.client_secret.map(str::to_string),
        },
        // No client id in the row: the built-in one is a property of the BUILD,
        // not of the account, and persisting it would pin the account to
        // whatever it happened to be on the day it was created.
        config: vec![
            (
                CLIENT_SOURCE_KEY.to_string(),
                Value::String(CLIENT_SOURCE_BUILTIN.to_string()),
            ),
            (
                CLIENT_FINGERPRINT_KEY.to_string(),
                Value::String(fingerprint),
            ),
        ],
        persist_secret: false,
    })
}

fn missing_builtin() -> AccountSetupError {
    AccountSetupError::InvalidInput(
        "This build carries no credentials for this provider. Register your own integration \
         with it and enter the client ID and secret."
            .into(),
    )
}

/// Whether this build carries credentials for the schema's provider — what a
/// connect form asks before deciding whether to show the credential fields at
/// all.
pub fn has_builtin_client(oauth: &AccountOauth) -> bool {
    oauth
        .builtin_provider
        .as_deref()
        .and_then(builtin_oauth::Provider::parse)
        .is_some_and(builtin_oauth::has_builtin_client)
}

/// The keychain slots an account of this schema must have populated to count as
/// connected.
///
/// What "connected" means is the adapter's own statement: a required secret
/// field is a credential the account cannot work without, and an OAuth block
/// that keeps a refresh token cannot work without that either. The alternative
/// — a table in the host mapping each adapter kind to a slot — has already gone
/// wrong once here, probing a kind for a password it never had and reporting
/// every working account of that kind as needing to be reconnected.
///
/// The OAuth CLIENT secret is deliberately absent: for the built-in posture
/// there is nothing in the keychain to find, and an account is not
/// disconnected for that.
pub fn required_slots(schema: &AccountSchema) -> Vec<SecretSlot> {
    let client_secret_field = schema
        .oauth
        .as_ref()
        .and_then(|o| o.client_secret_field.as_deref());
    let mut slots: Vec<SecretSlot> = schema
        .fields
        .iter()
        .filter(|f| f.required && Some(f.key.as_str()) != client_secret_field)
        .filter_map(|f| f.secret_slot.map(host_slot))
        .collect();
    if schema
        .oauth
        .as_ref()
        .is_some_and(|o| o.refresh_token_field.is_some())
    {
        slots.push(SecretSlot::RefreshToken);
    }
    slots.dedup();
    slots
}

// ── 2. Creating an account ───────────────────────────────────────────────────

/// What to write when creating an account.
///
/// No `Debug`: `secrets` holds cleartext credentials, and a type that can
/// format itself is one that eventually appears in a log line. Tests match on
/// its fields instead.
pub struct NewAccountPlan {
    /// The non-secret half, for the `accounts.config_json` column.
    pub config_json: String,
    /// Keychain writes, in the order they should happen. No `Debug` reaches
    /// these values; the slot names alone are loggable.
    pub secrets: Vec<(SecretSlot, String)>,
}

/// Split what the user entered into the row and the keychain.
///
/// `values` is keyed by the schema's field keys. `oauth` is the outcome of
/// [`choose_oauth_client`] when the schema has an OAuth block — its config keys
/// win over anything `values` carries for the same names, because the posture
/// decides the client, not the form.
///
/// The one rule this function exists to keep: **a secret never reaches
/// `config_json`.** Every field is routed by its declared `secret_slot`, so the
/// only way a secret could land in the column is a manifest that claimed it was
/// not one — which the schema validator rejects at load time.
pub fn plan_new_account(
    schema: &AccountSchema,
    values: &Map<String, Value>,
    oauth: Option<&OauthClientChoice>,
) -> Result<NewAccountPlan> {
    let mut config = Map::new();
    let mut secrets = Vec::new();
    // The OAuth client pair is settled by the posture, not by the form, so the
    // generic loop must not also route those two fields.
    let (id_field, secret_field) = match schema.oauth.as_ref() {
        Some(o) => (
            Some(o.client_id_field.as_str()),
            o.client_secret_field.as_deref(),
        ),
        None => (None, None),
    };

    for field in &schema.fields {
        if Some(field.key.as_str()) == id_field || Some(field.key.as_str()) == secret_field {
            continue;
        }
        let supplied = values.get(&field.key);
        match field.kind {
            AccountFieldKind::Bool => {
                let value = match supplied {
                    Some(Value::Bool(b)) => *b,
                    None | Some(Value::Null) => {
                        matches!(field.default, Some(AccountFieldDefault::Bool(true)))
                    }
                    Some(_) => {
                        return Err(AccountSetupError::InvalidInput(format!(
                            "`{}` must be true or false",
                            field.key
                        )))
                    }
                };
                config.insert(field.key.clone(), Value::Bool(value));
            }
            _ => {
                let text = match supplied {
                    Some(Value::String(s)) => s.trim().to_string(),
                    None | Some(Value::Null) => match &field.default {
                        Some(AccountFieldDefault::Text(t)) => t.clone(),
                        _ => String::new(),
                    },
                    Some(_) => {
                        return Err(AccountSetupError::InvalidInput(format!(
                            "`{}` must be text",
                            field.key
                        )))
                    }
                };
                if text.is_empty() {
                    if field.required {
                        return Err(AccountSetupError::InvalidInput(format!(
                            "`{}` is required",
                            field.key
                        )));
                    }
                    // An absent optional value is absent, not empty: writing ""
                    // would turn "the user did not say" into "the user said
                    // nothing", and adapters read those differently.
                    continue;
                }
                match field.secret_slot {
                    Some(slot) => secrets.push((host_slot(slot), text)),
                    None => {
                        config.insert(field.key.clone(), Value::String(text));
                    }
                }
            }
        }
    }

    if let Some(choice) = oauth {
        for (key, value) in &choice.config {
            config.insert(key.clone(), value.clone());
        }
        if choice.persist_secret {
            if let Some(secret) = &choice.client.secret {
                secrets.push((SecretSlot::OauthClientSecret, secret.clone()));
            }
        }
    }

    Ok(NewAccountPlan {
        config_json: Value::Object(config).to_string(),
        secrets,
    })
}

// ── 3. Opening an account ────────────────────────────────────────────────────

/// Build the init config a plugin instance is opened with.
///
/// Starts from the persisted non-secret row, strips the host's own bookkeeping,
/// then merges in — under the keys the *schema* named — every secret field's
/// keychain value plus, when there is an OAuth block, the resolved client and
/// the tokens the plugin asked to be handed.
///
/// `read_secret` is a closure rather than a map so no slot is read that this
/// account does not use: on a locked keychain even a read can raise a system
/// prompt, and the built-in OAuth posture in particular has nothing to look up.
pub fn init_config(
    schema: &AccountSchema,
    config_json: &str,
    mut read_secret: impl FnMut(SecretSlot) -> std::result::Result<String, SecretError>,
) -> Result<String> {
    let mut cfg: Value = serde_json::from_str(config_json)
        .map_err(|e| AccountSetupError::Config(format!("malformed account config: {e}")))?;
    let obj = cfg.as_object_mut().ok_or_else(|| {
        AccountSetupError::Config("account config_json must be a JSON object".into())
    })?;
    for key in HOST_KEYS {
        obj.remove(key);
    }

    let (id_field, secret_field) = match schema.oauth.as_ref() {
        Some(o) => (
            Some(o.client_id_field.as_str()),
            o.client_secret_field.as_deref(),
        ),
        None => (None, None),
    };

    for field in &schema.fields {
        let Some(slot) = field.secret_slot else {
            continue;
        };
        if Some(field.key.as_str()) == secret_field || Some(field.key.as_str()) == id_field {
            continue; // settled by the OAuth block below.
        }
        match read_secret(host_slot(slot)) {
            Ok(value) => {
                obj.insert(field.key.clone(), Value::String(value));
            }
            Err(SecretError::NotFound) if !field.required => {}
            Err(err) => {
                return Err(AccountSetupError::Secret(format!(
                    "missing `{}`: {err}",
                    field.key
                )))
            }
        }
    }

    if let Some(oauth) = &schema.oauth {
        let client = resolve_oauth_client(oauth, config_json, &mut read_secret)?;
        obj.insert(oauth.client_id_field.clone(), Value::String(client.id));
        if let (Some(key), Some(secret)) = (&oauth.client_secret_field, client.secret) {
            obj.insert(key.clone(), Value::String(secret));
        }
        if let Some(key) = &oauth.refresh_token_field {
            let refresh = read_secret(SecretSlot::RefreshToken)
                .map_err(|e| AccountSetupError::Secret(format!("missing refresh token: {e}")))?;
            obj.insert(key.clone(), Value::String(refresh));
        }
        if let Some(key) = &oauth.access_token_field {
            if let Ok(access) = read_secret(SecretSlot::AccessToken) {
                obj.insert(key.clone(), Value::String(access));
            }
        }
    }

    Ok(cfg.to_string())
}

/// Resolve the OAuth client for an EXISTING account, from its persisted row.
///
/// The built-in posture reads the credential from the build and checks the
/// fingerprint the row recorded, so a build carrying a *different* registration
/// says so plainly instead of decaying into an unexplained `invalid_grant`
/// weeks later. The bring-your-own posture reads the id from the row and the
/// secret from the keychain.
fn resolve_oauth_client(
    oauth: &AccountOauth,
    config_json: &str,
    read_secret: &mut impl FnMut(SecretSlot) -> std::result::Result<String, SecretError>,
) -> Result<OauthClient> {
    let cfg: Value = serde_json::from_str(config_json)
        .map_err(|e| AccountSetupError::Config(format!("malformed account config: {e}")))?;

    if cfg.get(CLIENT_SOURCE_KEY).and_then(Value::as_str) == Some(CLIENT_SOURCE_BUILTIN) {
        let client = oauth
            .builtin_provider
            .as_deref()
            .and_then(builtin_oauth::Provider::parse)
            .and_then(builtin_oauth::builtin_client)
            .ok_or_else(|| {
                AccountSetupError::Secret(
                    "this account uses Aperio's own credentials for its provider, and this build \
                     carries none — sign in again with your own integration"
                        .into(),
                )
            })?;
        if let Some(expected) = cfg.get(CLIENT_FINGERPRINT_KEY).and_then(Value::as_str) {
            let actual =
                builtin_oauth::ClientFingerprint::of(client.client_id, client.client_secret);
            if actual.as_str() != expected {
                return Err(AccountSetupError::Secret(format!(
                    "this account was linked to OAuth client {expected}, but this build carries \
                     {actual} — sign in again"
                )));
            }
        }
        return Ok(OauthClient {
            id: client.client_id.to_string(),
            secret: client.client_secret.map(str::to_string),
        });
    }

    let id = cfg
        .get(&oauth.client_id_field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AccountSetupError::Config(format!("this account has no `{}`", oauth.client_id_field))
        })?
        .to_string();
    let secret =
        match &oauth.client_secret_field {
            Some(_) => Some(read_secret(SecretSlot::OauthClientSecret).map_err(|e| {
                AccountSetupError::Secret(format!("missing OAuth client secret: {e}"))
            })?),
            None => None,
        };
    Ok(OauthClient { id, secret })
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_core::account_schema::AccountField;
    use std::cell::RefCell;

    fn field(
        key: &str,
        kind: AccountFieldKind,
        slot: Option<AccountSecretSlot>,
        required: bool,
    ) -> AccountField {
        AccountField {
            key: key.to_string(),
            kind,
            label: key.to_string(),
            label_key: None,
            hint: None,
            hint_key: None,
            required,
            default: None,
            secret_slot: slot,
        }
    }

    /// A schema in the shape a videoconference adapter with its own OAuth uses.
    fn oauth_schema() -> AccountSchema {
        AccountSchema {
            fields: vec![
                field("client_id", AccountFieldKind::Text, None, false),
                field(
                    "client_secret",
                    AccountFieldKind::Secret,
                    Some(AccountSecretSlot::OauthClientSecret),
                    false,
                ),
                AccountField {
                    default: Some(AccountFieldDefault::Bool(false)),
                    ..field("use_personal_room", AccountFieldKind::Bool, None, false)
                },
            ],
            oauth: Some(AccountOauth {
                builtin_provider: Some("webex".into()),
                client_id_field: "client_id".into(),
                client_secret_field: Some("client_secret".into()),
                refresh_token_field: Some("refresh_token".into()),
                access_token_field: None,
                app_redirect_uri: "aperio://oauth-callback".into(),
            }),
            host_channel: true,
            ..Default::default()
        }
    }

    /// A schema in the shape a password-based adapter uses — no OAuth at all.
    fn basic_schema() -> AccountSchema {
        AccountSchema {
            fields: vec![
                field("server_url", AccountFieldKind::Url, None, true),
                field("username", AccountFieldKind::Text, None, true),
                field(
                    "password",
                    AccountFieldKind::Secret,
                    Some(AccountSecretSlot::Password),
                    true,
                ),
            ],
            oauth: None,
            host_channel: false,
            ..Default::default()
        }
    }

    fn values(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    /// Records which slots were read, so a test can assert that a path did NOT
    /// touch the keychain.
    struct Keychain {
        stored: Vec<(SecretSlot, String)>,
        read: RefCell<Vec<SecretSlot>>,
    }

    impl Keychain {
        fn new(stored: &[(SecretSlot, &str)]) -> Self {
            Self {
                stored: stored.iter().map(|(s, v)| (*s, (*v).to_string())).collect(),
                read: RefCell::new(Vec::new()),
            }
        }
        fn reader(
            &self,
        ) -> impl FnMut(SecretSlot) -> std::result::Result<String, SecretError> + '_ {
            move |slot| {
                self.read.borrow_mut().push(slot);
                self.stored
                    .iter()
                    .find(|(s, _)| *s == slot)
                    .map(|(_, v)| v.clone())
                    .ok_or(SecretError::NotFound)
            }
        }
    }

    // ── the split ───────────────────────────────────────────────────────────

    #[test]
    fn a_password_goes_to_the_keychain_and_never_into_the_row() {
        let schema = basic_schema();
        let plan = plan_new_account(
            &schema,
            &values(&[
                ("server_url", Value::String(" https://dav.test/ ".into())),
                ("username", Value::String("toni".into())),
                ("password", Value::String("hunter2".into())),
            ]),
            None,
        )
        .unwrap_or_else(|e| panic!("a complete form: {e}"));
        assert!(
            !plan.config_json.contains("hunter2"),
            "config_json syncs in the clear: {}",
            plan.config_json
        );
        assert!(
            plan.config_json.contains("https://dav.test/"),
            "trimmed URL kept"
        );
        assert_eq!(plan.secrets, vec![(SecretSlot::Password, "hunter2".into())]);
    }

    #[test]
    fn a_missing_required_field_names_itself() {
        let schema = basic_schema();
        let err = plan_new_account(
            &schema,
            &values(&[("server_url", Value::String("https://dav.test/".into()))]),
            None,
        );
        let err = match err {
            Ok(_) => panic!("username is required"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("username"), "{err}");
    }

    #[test]
    fn an_omitted_optional_field_stays_absent_rather_than_empty() {
        // "the user did not say" and "the user said nothing" are different
        // answers, and adapters read them differently.
        let mut schema = basic_schema();
        schema.fields[1].required = false;
        let plan = plan_new_account(
            &schema,
            &values(&[
                ("server_url", Value::String("https://dav.test/".into())),
                ("password", Value::String("pw".into())),
            ]),
            None,
        )
        .unwrap_or_else(|e| panic!("username is optional here: {e}"));
        assert!(
            !plan.config_json.contains("username"),
            "{}",
            plan.config_json
        );
    }

    #[test]
    fn a_checkbox_falls_back_to_its_declared_default() {
        let schema = oauth_schema();
        let plan = plan_new_account(&schema, &Map::new(), None)
            .unwrap_or_else(|e| panic!("all optional: {e}"));
        assert!(plan.config_json.contains("\"use_personal_room\":false"));
    }

    // ── the OAuth posture ───────────────────────────────────────────────────

    #[test]
    fn a_users_own_pair_is_persisted_as_an_id_in_the_row_and_a_secret_in_the_keychain() {
        let schema = oauth_schema();
        let choice = choose_oauth_client(
            schema.oauth.as_ref().unwrap(),
            Some(" C-mine "),
            Some(" s3cr3t "),
        )
        .expect("a full pair");
        assert_eq!(choice.client.id, "C-mine", "surrounding space is trimmed");
        assert!(choice.persist_secret);

        let plan = plan_new_account(&schema, &Map::new(), Some(&choice)).unwrap();
        assert!(plan.config_json.contains("C-mine"));
        assert!(
            !plan.config_json.contains("s3cr3t"),
            "the secret must not reach the row: {}",
            plan.config_json
        );
        assert!(plan
            .secrets
            .contains(&(SecretSlot::OauthClientSecret, "s3cr3t".into())));
    }

    #[test]
    fn half_a_pair_is_refused_rather_than_quietly_completed() {
        let schema = oauth_schema();
        let oauth = schema.oauth.as_ref().unwrap();
        for (id, secret) in [
            (Some("C-mine"), Some("")),
            (Some("C-mine"), Some("  ")),
            (Some("C-mine"), None),
            (Some(""), Some("s3cr3t")),
            (None, Some("s3cr3t")),
        ] {
            match choose_oauth_client(oauth, id, secret) {
                Ok(_) => panic!("({id:?}, {secret:?}) must not resolve"),
                Err(err) => assert!(matches!(err, AccountSetupError::InvalidInput(_))),
            }
        }
    }

    #[test]
    fn a_public_client_needs_only_an_id() {
        // A provider registered as a PKCE public client declares no secret
        // field, and must not be asked for one.
        let mut schema = oauth_schema();
        schema.oauth.as_mut().unwrap().client_secret_field = None;
        let choice =
            choose_oauth_client(schema.oauth.as_ref().unwrap(), Some("C-public"), None).unwrap();
        assert_eq!(choice.client.secret, None);
        assert!(!choice.persist_secret);
    }

    #[test]
    fn an_empty_pair_follows_this_builds_posture() {
        // Green in BOTH postures: the suite must not depend on whether the
        // machine running it has a credentials file.
        let schema = oauth_schema();
        let oauth = schema.oauth.as_ref().unwrap();
        match choose_oauth_client(oauth, None, None) {
            Ok(choice) => {
                assert!(has_builtin_client(oauth));
                assert!(
                    !choice.persist_secret,
                    "the build's own secret does not belong in the user's keychain"
                );
                let keys: Vec<&str> = choice.config.iter().map(|(k, _)| k.as_str()).collect();
                assert_eq!(keys, vec![CLIENT_SOURCE_KEY, CLIENT_FINGERPRINT_KEY]);
                assert!(
                    !keys.contains(&"client_id"),
                    "pinning the id would freeze the account to today's registration"
                );
            }
            Err(err) => {
                assert!(!has_builtin_client(oauth));
                assert!(err.to_string().contains("client ID"), "{err}");
            }
        }
    }

    #[test]
    fn an_unknown_builtin_provider_reads_as_no_credentials_rather_than_a_panic() {
        // A third-party manifest can name anything at all here.
        let mut schema = oauth_schema();
        schema.oauth.as_mut().unwrap().builtin_provider = Some("not-a-provider".into());
        let oauth = schema.oauth.as_ref().unwrap();
        assert!(!has_builtin_client(oauth));
        assert!(matches!(
            choose_oauth_client(oauth, None, None),
            Err(AccountSetupError::InvalidInput(_))
        ));
    }

    // ── opening ─────────────────────────────────────────────────────────────

    #[test]
    fn opening_merges_the_keychain_values_under_the_keys_the_schema_named() {
        let schema = basic_schema();
        let kc = Keychain::new(&[(SecretSlot::Password, "hunter2")]);
        let merged = init_config(
            &schema,
            r#"{"server_url":"https://dav.test/","username":"toni"}"#,
            kc.reader(),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed["password"], "hunter2");
        assert_eq!(parsed["username"], "toni");
    }

    #[test]
    fn opening_strips_the_hosts_own_bookkeeping() {
        let schema = oauth_schema();
        let kc = Keychain::new(&[(SecretSlot::RefreshToken, "r3fresh")]);
        let config = format!(
            r#"{{"{CLIENT_SOURCE_KEY}":"nonsense","{CLIENT_FINGERPRINT_KEY}":"x","client_id":"C-mine","use_personal_room":true}}"#
        );
        // `client_source` is deliberately NOT "builtin" here, so this takes the
        // bring-your-own path and the row's own id is used.
        let kc2 = Keychain::new(&[
            (SecretSlot::RefreshToken, "r3fresh"),
            (SecretSlot::OauthClientSecret, "s3cr3t"),
        ]);
        drop(kc);
        let merged = init_config(&schema, &config, kc2.reader()).unwrap();
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert!(parsed.get(CLIENT_SOURCE_KEY).is_none());
        assert!(parsed.get(CLIENT_FINGERPRINT_KEY).is_none());
        assert_eq!(parsed["client_id"], "C-mine");
        assert_eq!(parsed["client_secret"], "s3cr3t");
        assert_eq!(parsed["refresh_token"], "r3fresh");
        assert_eq!(parsed["use_personal_room"], true);
    }

    #[test]
    fn a_plugin_that_wants_no_access_token_is_not_handed_one() {
        let schema = oauth_schema();
        assert!(schema.oauth.as_ref().unwrap().access_token_field.is_none());
        let kc = Keychain::new(&[
            (SecretSlot::RefreshToken, "r3fresh"),
            (SecretSlot::OauthClientSecret, "s3cr3t"),
            (SecretSlot::AccessToken, "should-not-appear"),
        ]);
        let merged = init_config(&schema, r#"{"client_id":"C-mine"}"#, kc.reader()).unwrap();
        assert!(!merged.contains("should-not-appear"), "{merged}");
    }

    #[test]
    fn the_builtin_path_never_reads_the_client_secret_slot() {
        // On a locked keychain even a read can raise a prompt, and there is
        // nothing there for a built-in account to find.
        let schema = oauth_schema();
        let kc = Keychain::new(&[(SecretSlot::RefreshToken, "r3fresh")]);
        let config = format!(r#"{{"{CLIENT_SOURCE_KEY}":"{CLIENT_SOURCE_BUILTIN}"}}"#);
        let _ = init_config(&schema, &config, kc.reader());
        assert!(
            !kc.read.borrow().contains(&SecretSlot::OauthClientSecret),
            "read {:?}",
            kc.read.borrow()
        );
    }

    #[test]
    fn a_builtin_account_from_a_different_registration_is_refused() {
        let schema = oauth_schema();
        let kc = Keychain::new(&[(SecretSlot::RefreshToken, "r3fresh")]);
        let config = format!(
            r#"{{"{CLIENT_SOURCE_KEY}":"{CLIENT_SOURCE_BUILTIN}","{CLIENT_FINGERPRINT_KEY}":"000000000000"}}"#
        );
        let err = init_config(&schema, &config, kc.reader()).expect_err("cannot match");
        // Either this build carries nothing, or it carries a real registration
        // whose digest is not twelve zeroes. Both are the same refusal from the
        // user's side, and both have to name the fix.
        assert!(err.to_string().contains("sign in"), "{err}");
    }

    #[test]
    fn what_planning_writes_is_what_opening_reads_back() {
        let schema = oauth_schema();
        let choice = choose_oauth_client(
            schema.oauth.as_ref().unwrap(),
            Some("C-mine"),
            Some("s3cr3t"),
        )
        .unwrap();
        let plan = plan_new_account(
            &schema,
            &values(&[("use_personal_room", Value::Bool(true))]),
            Some(&choice),
        )
        .unwrap();
        let mut stored = plan.secrets.clone();
        stored.push((SecretSlot::RefreshToken, "r3fresh".into()));
        let stored_refs: Vec<(SecretSlot, &str)> =
            stored.iter().map(|(s, v)| (*s, v.as_str())).collect();
        let kc = Keychain::new(&stored_refs);

        let merged = init_config(&schema, &plan.config_json, kc.reader()).unwrap();
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed["client_id"], "C-mine");
        assert_eq!(parsed["client_secret"], "s3cr3t");
        assert_eq!(parsed["refresh_token"], "r3fresh");
        assert_eq!(parsed["use_personal_room"], true);
    }

    #[test]
    fn a_schema_with_no_oauth_reads_no_oauth_slots() {
        let schema = basic_schema();
        let kc = Keychain::new(&[(SecretSlot::Password, "pw")]);
        let _ = init_config(&schema, r#"{"server_url":"u","username":"n"}"#, kc.reader());
        let read = kc.read.borrow();
        assert!(!read.contains(&SecretSlot::RefreshToken));
        assert!(!read.contains(&SecretSlot::OauthClientSecret));
    }
}
