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
        AccountSecretSlot::KeyPassphrase => SecretSlot::KeyPassphrase,
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

/// The account schema of the adapter that owns this kind, when a loaded plugin
/// declares one.
///
/// `None` covers two cases that both mean "no adapter holds a credential for
/// this": a host-internal kind (the local store, the device calendar, whose auth
/// is an OS permission grant), and a kind whose plugin is absent or disabled.
pub fn schema_for_kind(manager: &plugin_core::PluginManager, kind: &str) -> Option<AccountSchema> {
    manager
        .plugin_for_adapter_kind(kind)
        .and_then(|p| p.manifest.account.clone())
}

/// The keychain slots an account of this kind must have populated to count as
/// connected — [`required_slots`], resolved through the plugin that owns the
/// kind.
///
/// Empty for a kind no loaded plugin claims, which is the honest answer: a
/// credential-repair banner cannot ask the user to fix a credential that no
/// adapter wants.
///
/// Both hosts call this. They used to keep a `match` on kind names each, which
/// drifted exactly as one would expect — the desktop's table listed the four
/// videoconference kinds and the mobile one did not, so a Webex account with no
/// refresh token was flagged for repair on one platform and silently ignored on
/// the other. Those tables defaulted to `Password`, so an unrecognised OAuth
/// kind was probed for a password it never had and every working account of
/// that kind was reported as needing to be reconnected. Answering nothing is
/// the honest answer and no longer a guess.
pub fn required_slots_for_kind(
    manager: &plugin_core::PluginManager,
    kind: &str,
) -> Vec<SecretSlot> {
    schema_for_kind(manager, kind)
        .map(|schema| required_slots(&schema))
        .unwrap_or_default()
}

/// Whether connecting this adapter means a provider sign-in rather than a
/// credential the user can type.
///
/// The question every repair affordance has to answer first, and the one the
/// host used to answer with `matches!(kind, "google" | "microsoft_graph")` — in
/// five places, in two binaries, which is four opportunities to disagree and one
/// to forget the next provider. A schema that declares an `oauth` block is
/// saying exactly this about itself.
pub fn signs_in_with_oauth(schema: &AccountSchema) -> bool {
    schema.oauth.is_some()
}

/// The keychain slot a pasted credential goes into when repairing an account of
/// this schema, or `None` when there is no such thing.
///
/// `None` has two quite different causes and the caller should say which:
///
/// - The adapter signs in via OAuth. There is no credential to paste; the fix is
///   to re-run the sign-in. Check [`signs_in_with_oauth`] first if the message
///   matters, which it does — "this account cannot be repaired that way" is
///   useless next to "sign in again".
/// - The adapter declares no secret field at all, or declares more than one. One
///   secret field is what a single paste can serve; two would need the user to
///   be asked which, and no bundled adapter has two, so this refuses rather than
///   guessing and writing a password into a token slot.
///
/// The OAuth client secret never counts. It is part of the client registration
/// the user configured, not a credential that expires and needs re-entering.
pub fn repair_slot(schema: &AccountSchema) -> Option<SecretSlot> {
    if signs_in_with_oauth(schema) {
        return None;
    }
    let mut secrets = schema
        .fields
        .iter()
        .filter_map(|f| f.secret_slot)
        .filter(|s| *s != AccountSecretSlot::OauthClientSecret);
    let first = secrets.next()?;
    if secrets.next().is_some() {
        return None;
    }
    Some(host_slot(first))
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
    /// The values the adapter marked `device_local` — meaningful only on the
    /// machine that entered them, so they are kept out of `config_json`, which
    /// travels between a user's devices.
    ///
    /// A filesystem path is the clear case: an SSH key at
    /// `/home/anna/.ssh/id_ed25519` on one machine is at
    /// `C:\\Users\\Anna\\.ssh\\id_ed25519` on another. Sharing one row means
    /// whichever device wrote last decides, and the other then authenticates
    /// with a path that does not exist there — a failure with no obvious cause,
    /// on a machine nobody touched.
    ///
    /// Empty for every adapter that marks nothing, which is most of them.
    pub device_local: Map<String, Value>,
}

impl NewAccountPlan {
    /// The config to hand a probe: both halves, in one object.
    ///
    /// A probe tests what the user just typed, and it persists nothing — so the
    /// split that keeps device-local values off the synced row has no work to do
    /// here, and doing it anyway is actively wrong. There is no account id yet,
    /// so there is nowhere to read the local half back from; the values would
    /// simply be missing. An adapter whose only field is device-local then gets
    /// an empty config and fails to deserialise, and one that merely prefers a
    /// key file falls back to password auth and reports a blank password —
    /// telling the user their credentials are wrong when the form was fine.
    pub fn probe_config_json(&self) -> String {
        if self.device_local.is_empty() {
            return self.config_json.clone();
        }
        let Ok(Value::Object(mut obj)) = serde_json::from_str::<Value>(&self.config_json) else {
            return self.config_json.clone();
        };
        for (key, value) in &self.device_local {
            obj.insert(key.clone(), value.clone());
        }
        Value::Object(obj).to_string()
    }
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
    let mut device_local = Map::new();
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
                if field.device_local {
                    device_local.insert(field.key.clone(), Value::Bool(value));
                } else {
                    config.insert(field.key.clone(), Value::Bool(value));
                }
            }
            AccountFieldKind::Number => {
                // Both forms hand every value over as a string, so the text
                // path is the normal one; a JSON number is accepted too, for a
                // caller that already has one.
                let text = match supplied {
                    Some(Value::String(s)) => s.trim().to_string(),
                    Some(Value::Number(n)) => n.to_string(),
                    None | Some(Value::Null) => match &field.default {
                        Some(AccountFieldDefault::Text(t)) => t.trim().to_string(),
                        _ => String::new(),
                    },
                    Some(_) => {
                        return Err(AccountSetupError::InvalidInput(format!(
                            "`{}` must be a number",
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
                    continue;
                }
                // Rejected here rather than at the adapter, where the failure
                // would name a deserialisation error instead of the field the
                // user typed in.
                let number: i64 = text.parse().map_err(|_| {
                    AccountSetupError::InvalidInput(format!(
                        "`{}` must be a whole number, not `{text}`",
                        field.key
                    ))
                })?;
                // The declared bound, checked here for the same reason the kind
                // exists: at the adapter this is a serde error naming a Rust
                // type, and the whole struct fails with it.
                if field.min.is_some_and(|min| number < min)
                    || field.max.is_some_and(|max| number > max)
                {
                    return Err(AccountSetupError::InvalidInput(
                        match (field.min, field.max) {
                            (Some(min), Some(max)) => {
                                format!("`{}` must be between {min} and {max}", field.key)
                            }
                            (Some(min), None) => format!("`{}` must be at least {min}", field.key),
                            (None, Some(max)) => format!("`{}` must be at most {max}", field.key),
                            (None, None) => unreachable!("neither bound set"),
                        },
                    ));
                }
                let number = Value::Number(number.into());
                if field.device_local {
                    device_local.insert(field.key.clone(), number);
                } else {
                    config.insert(field.key.clone(), number);
                }
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
                    // A secret is already device-local by a different route —
                    // the keychain — so the flag adds nothing here.
                    Some(slot) => secrets.push((host_slot(slot), text)),
                    None if field.device_local => {
                        device_local.insert(field.key.clone(), Value::String(text));
                    }
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
        device_local,
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
    read_secret: impl FnMut(SecretSlot) -> std::result::Result<String, SecretError>,
) -> Result<String> {
    init_config_with_local(schema, config_json, &Map::new(), read_secret)
}

/// [`init_config`], plus this device's half of the account.
///
/// An adapter sees ONE configuration. It never learns that some of its fields
/// travelled with the account row and some were read out of this machine's own
/// store — which is the point: `device_local` is a statement about where a value
/// is kept, not about what the adapter does with it.
///
/// The local half wins where both carry a key. That only happens after a
/// migration, when a value that used to travel has been marked device-local and
/// the old copy is still sitting in `config_json`; the local one is this
/// machine's answer and the row's is some other machine's.
pub fn init_config_with_local(
    schema: &AccountSchema,
    config_json: &str,
    device_local: &Map<String, Value>,
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
    // This device's half, merged in before anything reads the map, so the rest
    // of this function cannot tell the two apart.
    for (key, value) in device_local {
        obj.insert(key.clone(), value.clone());
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
        let client = resolve_oauth_client(schema, oauth, config_json, &mut read_secret)?;
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
///
/// The whole schema is passed, not just its `oauth` block, because whether the
/// client secret is mandatory is a property of the FIELD — see the read below.
fn resolve_oauth_client(
    schema: &AccountSchema,
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
    // Declaring a client-secret field is not the same as demanding one, and the
    // schema already says which it is. A provider whose client is public —
    // PKCE, no secret issued at all — may still declare the field so that a
    // user with a confidential registration can supply one, and mark it
    // optional; an account of that shape with nothing in the slot is complete,
    // not broken. Reading the slot unconditionally and turning `NotFound` into
    // an error made such an account permanently unopenable, with nothing the
    // user could type to repair it.
    //
    // `required` keeps failing loudly, and must. Google's installed-app flow
    // presents the secret at the token endpoint, so an account that lost it
    // fails at the provider instead — later, as an `invalid_client` a long way
    // from its cause.
    //
    // Only `NotFound` is tolerated. A keychain that will not ANSWER — locked,
    // busy, broken — is an error whatever the field says, exactly as in the
    // per-field loop above: absent and unreadable are different states and
    // collapsing them reports a credential the user never lost as gone.
    let secret = match &oauth.client_secret_field {
        Some(key) => {
            // An undeclared field cannot happen — `AccountSchema::validate`
            // rejects a `client_secret_field` naming one — and if it somehow
            // did, the stricter of the two answers is the safe one.
            let required = schema.field(key).is_none_or(|field| field.required);
            match read_secret(SecretSlot::OauthClientSecret) {
                Ok(value) => Some(value),
                Err(SecretError::NotFound) if !required => None,
                Err(err) => {
                    return Err(AccountSetupError::Secret(format!(
                        "missing OAuth client secret: {err}"
                    )))
                }
            }
        }
        None => None,
    };
    Ok(OauthClient { id, secret })
}

#[cfg(test)]
mod tests {
    /// The adapter must see one configuration, not two halves.
    #[test]
    fn the_local_half_arrives_alongside_the_travelling_one() {
        let mut key_path = field("key_path", AccountFieldKind::File, None, false);
        key_path.device_local = true;
        let schema = AccountSchema {
            fields: vec![field("host", AccountFieldKind::Text, None, false), key_path],
            ..Default::default()
        };
        let mut local = Map::new();
        local.insert(
            "key_path".into(),
            Value::String("/home/anna/.ssh/id_ed25519".into()),
        );

        let json =
            init_config_with_local(&schema, r#"{"host":"backup.example.test"}"#, &local, |_| {
                Err(SecretError::NotFound)
            })
            .unwrap();
        let cfg: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(
            cfg.get("host").and_then(|v| v.as_str()),
            Some("backup.example.test"),
        );
        assert_eq!(
            cfg.get("key_path").and_then(|v| v.as_str()),
            Some("/home/anna/.ssh/id_ed25519"),
        );
    }

    /// After a field is newly marked device-local, the row may still carry the
    /// old shared value. This machine's answer is the right one — the row's is
    /// whichever device wrote last.
    #[test]
    fn the_local_half_wins_over_a_leftover_in_the_row() {
        let mut key_path = field("key_path", AccountFieldKind::File, None, false);
        key_path.device_local = true;
        let schema = AccountSchema {
            fields: vec![key_path],
            ..Default::default()
        };
        let mut local = Map::new();
        local.insert("key_path".into(), Value::String("/home/anna/mine".into()));

        let json = init_config_with_local(
            &schema,
            r#"{"key_path":"/home/someone-else/theirs"}"#,
            &local,
            |_| Err(SecretError::NotFound),
        )
        .unwrap();
        let cfg: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            cfg.get("key_path").and_then(|v| v.as_str()),
            Some("/home/anna/mine"),
        );
    }

    /// The overwhelming majority of accounts have no local half at all.
    #[test]
    fn no_local_half_changes_nothing() {
        let schema = AccountSchema {
            fields: vec![field("url", AccountFieldKind::Url, None, false)],
            ..Default::default()
        };
        let with = init_config_with_local(
            &schema,
            r#"{"url":"https://example.test/"}"#,
            &Map::new(),
            |_| Err(SecretError::NotFound),
        )
        .unwrap();
        let without = init_config(&schema, r#"{"url":"https://example.test/"}"#, |_| {
            Err(SecretError::NotFound)
        })
        .unwrap();
        assert_eq!(with, without);
    }

    /// The rule from the schema, exercised where it takes effect.
    ///
    /// A marked field must not reach `config_json`, because that column is what
    /// travels between devices; an unmarked one must, because otherwise the
    /// user retypes their server address on every phone they own.
    #[test]
    fn a_marked_field_leaves_the_travelling_half() {
        let mut key_path = field("key_path", AccountFieldKind::File, None, false);
        key_path.device_local = true;
        let schema = AccountSchema {
            fields: vec![field("host", AccountFieldKind::Text, None, false), key_path],
            ..Default::default()
        };
        let mut values = Map::new();
        values.insert("host".into(), Value::String("backup.example.test".into()));
        values.insert(
            "key_path".into(),
            Value::String("/home/anna/.ssh/id_ed25519".into()),
        );

        let plan = plan_new_account(&schema, &values, None).unwrap();
        let config: Value = serde_json::from_str(&plan.config_json).unwrap();

        assert_eq!(
            config.get("host").and_then(|v| v.as_str()),
            Some("backup.example.test"),
            "an unmarked value must travel, or every device retypes it",
        );
        assert!(
            config.get("key_path").is_none(),
            "a marked value reached config_json, which syncs",
        );
        assert_eq!(
            plan.device_local.get("key_path").and_then(|v| v.as_str()),
            Some("/home/anna/.ssh/id_ed25519"),
        );
    }

    /// Secrets never went into the row anyway. The flag must not divert them
    /// into a third place where nothing would look for them.
    #[test]
    fn a_secret_still_goes_to_the_keychain_when_also_marked() {
        let mut password = field(
            "password",
            AccountFieldKind::Secret,
            Some(AccountSecretSlot::Password),
            false,
        );
        password.device_local = true;
        let schema = AccountSchema {
            fields: vec![password],
            ..Default::default()
        };
        let mut values = Map::new();
        values.insert("password".into(), Value::String("hunter2".into()));

        let plan = plan_new_account(&schema, &values, None).unwrap();
        assert_eq!(plan.secrets.len(), 1);
        assert!(plan.device_local.is_empty());
    }

    /// Checkboxes take the same route as text, which is easy to forget because
    /// they are handled in a separate branch.
    #[test]
    fn a_marked_checkbox_is_device_local_too() {
        let mut flag = field("use_local_cache", AccountFieldKind::Bool, None, false);
        flag.device_local = true;
        let schema = AccountSchema {
            fields: vec![flag],
            ..Default::default()
        };
        let mut values = Map::new();
        values.insert("use_local_cache".into(), Value::Bool(true));

        let plan = plan_new_account(&schema, &values, None).unwrap();
        let config: Value = serde_json::from_str(&plan.config_json).unwrap();
        assert!(config.get("use_local_cache").is_none());
        assert_eq!(
            plan.device_local.get("use_local_cache"),
            Some(&Value::Bool(true))
        );
    }

    /// Most adapters mark nothing, and must be unaffected.
    #[test]
    fn an_adapter_that_marks_nothing_is_unchanged() {
        let schema = AccountSchema {
            fields: vec![field("url", AccountFieldKind::Url, None, false)],
            ..Default::default()
        };
        let mut values = Map::new();
        values.insert("url".into(), Value::String("https://example.test/".into()));
        let plan = plan_new_account(&schema, &values, None).unwrap();
        assert!(plan.device_local.is_empty());
    }

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
            options: Vec::new(),
            min: None,
            max: None,
            device_local: false,
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

    /// The trap that made this kind necessary: a plugin declaring `port: u16`
    /// rejects `"22"` outright, and the failure is the whole struct.
    #[test]
    fn a_number_reaches_the_adapter_as_a_number() {
        let schema = AccountSchema {
            fields: vec![
                field("host", AccountFieldKind::Text, None, true),
                field("port", AccountFieldKind::Number, None, false),
            ],
            ..Default::default()
        };
        let plan = plan_new_account(
            &schema,
            &values(&[
                ("host", Value::String("files.example.com".into())),
                // As it leaves both forms: text.
                ("port", Value::String("2222".into())),
            ]),
            None,
        )
        .expect("plan");
        let cfg: Value = serde_json::from_str(&plan.config_json).unwrap();
        assert_eq!(cfg["port"], serde_json::json!(2222));
        assert!(
            !cfg["port"].is_string(),
            "a string here fails the adapter's own deserialisation: {}",
            plan.config_json
        );
    }

    #[test]
    fn a_number_falls_back_to_its_declared_default() {
        let mut port = field("port", AccountFieldKind::Number, None, false);
        port.default = Some(AccountFieldDefault::Text("22".into()));
        let schema = AccountSchema {
            fields: vec![port],
            ..Default::default()
        };
        let plan = plan_new_account(&schema, &Map::new(), None).expect("plan");
        let cfg: Value = serde_json::from_str(&plan.config_json).unwrap();
        assert_eq!(cfg["port"], serde_json::json!(22));
    }

    /// Rejected here, where the message can name the field the user typed in.
    /// At the adapter it would arrive as a deserialisation error about a struct.
    #[test]
    fn a_number_that_is_not_one_is_refused_by_name() {
        let schema = AccountSchema {
            fields: vec![field("port", AccountFieldKind::Number, None, false)],
            ..Default::default()
        };
        let err = plan_new_account(
            &schema,
            &values(&[("port", Value::String("twenty-two".into()))]),
            None,
        )
        // `.err()` rather than `expect_err`: the plan carries secrets and
        // deliberately has no `Debug`.
        .err()
        .expect("must not plan");
        let text = err.to_string();
        assert!(text.contains("port"), "must name the field: {text}");
        assert!(text.contains("twenty-two"), "must quote the value: {text}");
    }

    /// A port outside `u16` would reach the adapter as a serde error naming a
    /// Rust type, and take the whole init config down with it.
    #[test]
    fn a_number_outside_its_declared_range_is_refused_by_name() {
        let mut port = field("port", AccountFieldKind::Number, None, false);
        port.min = Some(1);
        port.max = Some(65535);
        let schema = AccountSchema {
            fields: vec![port],
            ..Default::default()
        };
        let err = plan_new_account(
            &schema,
            &values(&[("port", Value::String("70000".into()))]),
            None,
        )
        .err()
        .expect("must not plan");
        let text = err.to_string();
        assert!(text.contains("port"), "must name the field: {text}");
        assert!(text.contains("65535"), "must state the bound: {text}");
    }

    /// A probe persists nothing and has no account id, so the split that keeps
    /// device-local values off the synced row has nowhere to read them back
    /// from. Both halves have to travel together, or an adapter whose only
    /// field is device-local is untestable and one that prefers a key file
    /// silently falls back to password auth.
    #[test]
    fn a_probe_sees_both_halves_of_the_form() {
        let mut folder = field("remote_root", AccountFieldKind::Directory, None, true);
        folder.device_local = true;
        let schema = AccountSchema {
            fields: vec![folder],
            ..Default::default()
        };
        let plan = plan_new_account(
            &schema,
            &values(&[("remote_root", Value::String("/srv/aperio".into()))]),
            None,
        )
        .expect("plan");

        let persisted: Value = serde_json::from_str(&plan.config_json).unwrap();
        assert!(
            persisted.get("remote_root").is_none(),
            "a device-local value must not reach the synced row: {}",
            plan.config_json
        );

        let probed: Value = serde_json::from_str(&plan.probe_config_json()).unwrap();
        assert_eq!(
            probed.get("remote_root").and_then(|v| v.as_str()),
            Some("/srv/aperio"),
            "the probe would have received an empty config",
        );
    }

    /// A blank optional number stays absent rather than becoming 0 — the
    /// adapter's own `#[serde(default)]` is the right answer, and a zero port
    /// is not.
    #[test]
    fn a_blank_optional_number_is_absent() {
        let schema = AccountSchema {
            fields: vec![field("port", AccountFieldKind::Number, None, false)],
            ..Default::default()
        };
        let plan = plan_new_account(
            &schema,
            &values(&[("port", Value::String("  ".into()))]),
            None,
        )
        .expect("plan");
        let cfg: Value = serde_json::from_str(&plan.config_json).unwrap();
        assert!(cfg.get("port").is_none(), "{}", plan.config_json);
    }

    /// The SFTP shape: a password and a key passphrase, held at once. Sharing
    /// one slot would make the second write overwrite the first, and switching
    /// auth method back would silently reuse the wrong credential.
    #[test]
    fn two_credentials_go_to_two_slots() {
        let schema = AccountSchema {
            fields: vec![
                field(
                    "password",
                    AccountFieldKind::Secret,
                    Some(AccountSecretSlot::Password),
                    false,
                ),
                field(
                    "key_passphrase",
                    AccountFieldKind::Secret,
                    Some(AccountSecretSlot::KeyPassphrase),
                    false,
                ),
            ],
            ..Default::default()
        };
        let plan = plan_new_account(
            &schema,
            &values(&[
                ("password", Value::String("pw".into())),
                ("key_passphrase", Value::String("kp".into())),
            ]),
            None,
        )
        .expect("plan");
        assert_eq!(plan.secrets.len(), 2);
        assert_eq!(
            plan.secrets
                .iter()
                .find(|(s, _)| *s == SecretSlot::Password)
                .map(|(_, v)| v.as_str()),
            Some("pw"),
        );
        assert_eq!(
            plan.secrets
                .iter()
                .find(|(s, _)| *s == SecretSlot::KeyPassphrase)
                .map(|(_, v)| v.as_str()),
            Some("kp"),
        );
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

    /// A client secret the schema does not demand may be absent, and the
    /// account still opens.
    ///
    /// The failure this prevents is permanent and silent: a provider whose
    /// client needs no secret (PKCE) leaves the slot empty, and refusing to
    /// open on `NotFound` leaves an account nothing can repair — there is no
    /// value to type, because there was never meant to be one.
    #[test]
    fn an_optional_client_secret_may_be_missing() {
        let schema = oauth_schema();
        assert!(
            !schema.field("client_secret").unwrap().required,
            "this schema is the optional case; the test below is the other one",
        );
        let kc = Keychain::new(&[(SecretSlot::RefreshToken, "r3fresh")]);
        let merged = init_config(&schema, r#"{"client_id":"C-mine"}"#, kc.reader())
            .expect("an optional secret that is absent is not a failure");
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed["client_id"], "C-mine");
        assert_eq!(parsed["refresh_token"], "r3fresh");
        assert!(
            parsed.get("client_secret").is_none(),
            "an absent secret must stay absent rather than becoming \"\": {merged}",
        );
    }

    /// Tolerating an ABSENT secret must not tolerate an UNREADABLE keychain.
    /// A locked or busy store answering `Backend` is not a provider that issues
    /// no secret, and opening without one would authenticate as a client the
    /// user did not configure.
    #[test]
    fn an_optional_client_secret_still_refuses_a_keychain_that_will_not_answer() {
        let schema = oauth_schema();
        let err = init_config(&schema, r#"{"client_id":"C-mine"}"#, |slot| {
            if slot == SecretSlot::OauthClientSecret {
                Err(SecretError::Backend("locked".into()))
            } else {
                Ok("r3fresh".to_string())
            }
        })
        .expect_err("an unreadable keychain is not an absent value");
        assert!(err.to_string().contains("client secret"), "{err}");
    }

    /// The other direction, against the manifest that ships rather than a
    /// hand-written twin of it: Google's installed-app flow presents the client
    /// secret at the token endpoint, the schema says `required`, and an account
    /// that lost it has to fail here — where the message names the credential —
    /// instead of at the provider as an `invalid_client` weeks later.
    #[test]
    fn a_required_client_secret_still_fails_loudly() {
        let manifest = plugin_core::manifest::PluginManifest::from_bytes(include_bytes!(
            "../../cal-adapter-google-plugin/plugin.json"
        ))
        .expect("the shipped manifest parses");
        let schema = manifest
            .account
            .expect("the shipped manifest declares an account schema");
        assert!(
            schema.field("client_secret").unwrap().required,
            "Google Drive's client secret is mandatory; a schema saying otherwise \
             would make the tolerance above apply to it",
        );

        let kc = Keychain::new(&[(SecretSlot::RefreshToken, "r3fresh")]);
        let err = init_config(&schema, r#"{"client_id":"C-mine"}"#, kc.reader())
            .expect_err("a required client secret that is gone must refuse");
        assert!(err.to_string().contains("client secret"), "{err}");
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
    fn a_repair_writes_where_the_schema_says_and_nowhere_else() {
        // The password adapter: one secret field, and its declared slot is the
        // answer. This used to be `matches!(kind, "vikunja" | "todoist")` and
        // everything else fell through to Password — which is right until an
        // adapter arrives whose credential is neither.
        assert_eq!(repair_slot(&basic_schema()), Some(SecretSlot::Password));
        assert!(!signs_in_with_oauth(&basic_schema()));

        // A token adapter says so itself.
        let mut token = basic_schema();
        token.fields = vec![
            field("server_url", AccountFieldKind::Url, None, true),
            field(
                "api_token",
                AccountFieldKind::Secret,
                Some(AccountSecretSlot::ApiToken),
                true,
            ),
        ];
        assert_eq!(repair_slot(&token), Some(SecretSlot::ApiToken));
    }

    #[test]
    fn an_oauth_account_is_never_offered_a_paste() {
        let schema = oauth_schema();
        assert!(signs_in_with_oauth(&schema));
        // None even though the schema HAS a secret field — the client secret is
        // part of the registration the user configured, not a credential that
        // expires. Offering to paste it as a repair would write the wrong thing
        // into the wrong slot and leave the expired grant untouched.
        assert_eq!(repair_slot(&schema), None);
    }

    #[test]
    fn two_secret_fields_refuse_rather_than_guess() {
        let mut two = basic_schema();
        two.fields.push(field(
            "second_password",
            AccountFieldKind::Secret,
            Some(AccountSecretSlot::ApiToken),
            true,
        ));
        // No bundled adapter is shaped this way. If one ever is, the user has to
        // be asked which credential they are replacing; silently taking the
        // first would write a password into a token slot.
        assert_eq!(repair_slot(&two), None);
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
