//! Writing down where this device syncs — as an account row, not as twenty
//! preferences.
//!
//! [`super::migrate`] turns a target that already exists into an account, once.
//! This is the other direction and the one that runs every day: a user fills in
//! the connect form, the host probes it, and what gets PERSISTED is a row, a
//! device-local half, keychain entries under that row's id, and a pointer
//! saying this device syncs through it. The twenty `sync.adapter.*`
//! preferences and the seven keychain pseudo-accounts are retired as the last
//! step of the same call.
//!
//! ## Why both copies may not stay alive
//!
//! They disagree. That is not a hypothetical: `disconnect` used to delete one
//! preference — the kind — and leave the host, the path and the password
//! exactly where they were, so the next launch rebuilt the adapter from what
//! was left and resumed uploading to a target the user had disconnected from.
//! A record that no writer maintains and some reader still trusts is the shape
//! that bug takes, and the only way to not have it is to not have the second
//! record.
//!
//! So: [`connect`] writes the account and then removes the legacy copy, in that
//! order and only once the caller's probe has succeeded; [`disconnect`] removes
//! every trace either reader could act on.
//!
//! ## What deliberately does not move
//!
//! The end-to-end encryption key (`SECRET_ACCOUNT_E2E`) and its flag. They
//! belong to the DATASET and to this device's posture towards it, not to the
//! place the dataset is kept — the reasoning is in [`super::from_account`] and
//! this is not the place to re-open it. It matters most on the path below:
//! disconnecting from a target is not a decision to throw away the key that
//! makes the data readable, and the ordinary "delete the account, delete its
//! secrets" sweep would have taken it.
//!
//! Host-key pins stay keyed by `host:port`. A user who already confirmed a
//! server's fingerprint must not be asked again because Aperio changed how it
//! models the target.
//!
//! ## No event is emitted
//!
//! A sync-only account does not travel — `accounts::travels_between_devices`
//! says so — but the stronger guarantee is structural, as in the migration:
//! nothing here takes an emitter, so there is nothing to call.

use std::sync::Arc;

use serde_json::{Map, Value};
use sync_core::SyncAdapter;
use sync_engine::{SecretError, SecretSlot, SecretStore};

use super::from_account::selected_account_id_result;
use super::migrate::{
    account_kind_for, credential_routes, name_from, name_source, schema_field_for, stored_kind_for,
};
use super::*;
use crate::account_setup::{choose_oauth_client, host_slot, plan_new_account};
use crate::accounts::{Account, AccountsError, AccountsRepo, AdapterKind};
use crate::registry::HostKeyPins;
use crate::user_prefs::UserPrefsRepo;

/// Why the choice could not be written down.
///
/// Strings rather than the underlying error types, as in [`super::persist`] and
/// [`super::migrate`]: both hosts turn this straight into their own wire error,
/// and nothing downstream matches on the cause.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// A kind this build does not serve. Refused rather than guessed: there is
    /// no field table to translate it with.
    #[error("unknown sync adapter kind: {0}")]
    UnknownKind(String),
    /// No loaded plugin declares the kind, so nothing can say which of the
    /// form's values are secret and which stay on this device.
    #[error("no loaded plugin serves `{0}`")]
    NoPlugin(String),
    /// The form does not describe an account this build can open.
    #[error("{0}")]
    Invalid(String),
    #[error("preferences: {0}")]
    Prefs(String),
    #[error("accounts: {0}")]
    Accounts(String),
    #[error("keychain: {0}")]
    Secret(String),
    /// The protocol pins host keys and this device has not confirmed one for
    /// this host. A step in the flow rather than a fault: the frontend answers
    /// it with the fingerprint dialog.
    #[error("the host key for {host_port} has not been confirmed on this device")]
    HostKeyNotTrusted { host_port: String },
    /// The plugin refused the config it was handed.
    #[error("{0}")]
    PluginRefused(String),
}

fn prefs_err(err: impl std::fmt::Display) -> ConnectError {
    ConnectError::Prefs(err.to_string())
}

fn accounts_err(err: impl std::fmt::Display) -> ConnectError {
    ConnectError::Accounts(err.to_string())
}

fn secret_err(err: impl std::fmt::Display) -> ConnectError {
    ConnectError::Secret(err.to_string())
}

/// One form value as the text it will be stored as, or `None` for absent and
/// blank.
///
/// Numbers arrive as JSON numbers from one host and as strings from the other —
/// a port is `2222` or `"2222"` depending on which frontend asked — and both
/// mean the same thing.
fn text_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Connect this device to a target, and make the account row the only record of
/// it.
///
/// `values` is the connect form, keyed the way both hosts' wire shapes key it
/// (`path`, `url`, `host`, `password`, …). `kind` is the stored spelling those
/// forms use — `local`, not `local_folder`.
///
/// ## The order of the writes, and why it is that order
///
/// The row, then this device's half, then the credentials, then the pointer,
/// then the legacy copy is retired, then the row this device moved off is
/// dropped. Callers run their probe and their `orchestrator.configure` BEFORE
/// this, so nothing here can be reached by a target that was rejected.
///
/// - The pointer comes after everything the row needs, because a pointer at a
///   half-written account is worse than no pointer at all: it stops the legacy
///   fallback and offers nothing in its place.
/// - Retiring the legacy copy comes after the pointer, so a failure in between
///   leaves a device that still has BOTH records and a pointer that decides
///   between them — which is the state every reader already handles.
/// - Dropping the previous account is last, because a spare row in a list is a
///   smaller thing to leave behind than a device with no target at all.
/// Open an adapter from what the user just typed, WITHOUT writing anything
/// down.
///
/// The onboarding flow needs this and [`connect`] cannot give it. Setting up a
/// target from the settings is one decision — this is the target, use it — and
/// the row can be written as soon as the probe passes. Joining a dataset is
/// two: reach the target, see whether a dataset is already there, and only
/// then choose between adopting it and starting a fresh one. The first half
/// has to happen against a live adapter that nothing has committed to.
///
/// Everything provider-specific still comes from the schema, so the wizard
/// stops needing a form per backend — which is the point. It splits the values
/// exactly as [`connect`] does, through the same `plan_new_account`, and then
/// puts the secrets back under their own field names rather than fetching them
/// from a keychain they are not in yet.
pub fn preview_adapter(
    plugins: &dyn SyncPlugins,
    pins: &dyn HostKeyPins,
    kind: &str,
    values: &Map<String, Value>,
) -> Result<Arc<dyn SyncAdapter>, ConnectError> {
    let kind = kind.trim();
    let account_kind =
        account_kind_for(kind).ok_or_else(|| ConnectError::UnknownKind(kind.to_string()))?;
    let (plugin_id, schema) = plugins
        .resolve(account_kind)
        .ok_or_else(|| ConnectError::NoPlugin(account_kind.to_string()))?;

    // The form, under the plugin's own field names.
    let mut fields = Map::new();
    for (form_key, value) in values {
        let field = schema_field_for(kind, form_key).unwrap_or(form_key.as_str());
        fields.insert(field.to_string(), value.clone());
    }

    let oauth = match schema.oauth.as_ref() {
        Some(oauth) => Some(
            choose_oauth_client(
                oauth,
                fields.get(&oauth.client_id_field).and_then(Value::as_str),
                oauth
                    .client_secret_field
                    .as_deref()
                    .and_then(|key| fields.get(key))
                    .and_then(Value::as_str),
            )
            .map_err(|err| ConnectError::Invalid(err.to_string()))?,
        ),
        None => None,
    };

    let plan = plan_new_account(&schema, &fields, oauth.as_ref())
        .map_err(|err| ConnectError::Invalid(err.to_string()))?;

    // `probe_config_json` merges this device's half back in; the secrets are
    // still only in the plan, so they go in here under the keys the schema
    // gave them. On the committed path the keychain does this, which is why
    // that path has no equivalent line.
    let mut config: Value = serde_json::from_str(&plan.probe_config_json())
        .map_err(|err| ConnectError::Invalid(err.to_string()))?;
    let obj = config
        .as_object_mut()
        .ok_or_else(|| ConnectError::Invalid("config is not a JSON object".into()))?;
    for field in &schema.fields {
        let Some(slot) = field.secret_slot.map(host_slot) else {
            continue;
        };
        if let Some((_, value)) = plan.secrets.iter().find(|(s, _)| *s == slot) {
            obj.insert(field.key.clone(), Value::String(value.clone()));
        }
    }

    let mut config_json = config.to_string();
    if let Some(pin) = schema.host_key_pin.as_ref() {
        config_json = super::from_account::merge_pin_for_preview(&config_json, pin, pins)
            .map_err(|host_port| ConnectError::HostKeyNotTrusted { host_port })?;
    }

    plugins
        .open(&plugin_id, config_json)
        .map_err(ConnectError::PluginRefused)
}

pub fn connect(
    prefs: &UserPrefsRepo<'_>,
    accounts: &AccountsRepo<'_>,
    secrets: &dyn SecretStore,
    plugins: &dyn SyncPlugins,
    kind: &str,
    values: &Map<String, Value>,
) -> Result<String, ConnectError> {
    let kind = kind.trim();
    let account_kind =
        account_kind_for(kind).ok_or_else(|| ConnectError::UnknownKind(kind.to_string()))?;
    // Everything provider-specific comes from here: which values are secret and
    // which slot each lives in, which stay on this device, which are required.
    // This function knows none of it.
    let (_, schema) = plugins
        .resolve(account_kind)
        .ok_or_else(|| ConnectError::NoPlugin(account_kind.to_string()))?;

    // The row this device syncs through today, if any. Read through the
    // fallible twin: a database that will not say whether this device has
    // chosen must not be answered for, because "nobody chose" would create a
    // second account and leave the first one unreferenced.
    let previous = match selected_account_id_result(prefs).map_err(prefs_err)? {
        Some(id) => accounts.get(&id).map_err(accounts_err)?,
        None => None,
    };
    // Reusable only when it is the same kind. A WebDAV row is not an SFTP row
    // with different fields — most obviously its keychain entries are the WebDAV
    // server's, and inheriting them below would authenticate against the new
    // host with the old host's password.
    let current = previous
        .as_ref()
        .filter(|account| account.adapter_kind == account_kind);

    // The form, under the plugin's own field names.
    let mut fields = Map::new();
    for (form_key, value) in values {
        let field = schema_field_for(kind, form_key).unwrap_or(form_key.as_str());
        fields.insert(field.to_string(), value.clone());
    }

    // Whether this is the SAME target with an edited credential, or a different
    // one that merely speaks the same protocol.
    //
    // The distinction decides whether a stored password may be inherited below,
    // and getting it wrong sends the old server's credential to the new one at
    // probe time. Same kind is not enough: two WebDAV servers are the same kind.
    //
    // The test is that no non-secret value changed. It is deliberately blunt —
    // editing a folder path on the same server also costs a retyped password —
    // because the alternative is the host deciding which fields identify a
    // server, and that is exactly the per-adapter knowledge this layer does not
    // have. Erring toward "you changed where this goes, so say the password
    // again" is the safe direction.
    //
    // Compared on the PLUGIN's keys, above, rather than the form's: a field
    // whose two names differ would otherwise read as "not supplied", count as
    // unchanged, and let the credential through — the one direction that must
    // not happen by accident.
    let same_target = current.is_some_and(|account| {
        let stored: Map<String, Value> = serde_json::from_str(&account.config_json)
            .ok()
            .and_then(|v: Value| v.as_object().cloned())
            .unwrap_or_default();
        let local = crate::account_local::load(
            prefs,
            &account.id,
            &schema
                .fields
                .iter()
                .filter(|f| f.device_local && !f.is_secret())
                .map(|f| f.key.clone())
                .collect::<Vec<_>>(),
        );
        schema
            .fields
            .iter()
            .filter(|f| !f.is_secret())
            .all(|field| {
                // A value the form left blank says nothing either way: an optional
                // field nobody filled in is not a change.
                match text_of(fields.get(&field.key)) {
                    None => true,
                    Some(entered) => {
                        let held = local.get(&field.key).or_else(|| stored.get(&field.key));
                        match held {
                            Some(Value::String(s)) => s.trim() == entered,
                            Some(Value::Number(n)) => n.to_string() == entered,
                            Some(Value::Bool(b)) => b.to_string() == entered,
                            _ => false,
                        }
                    }
                }
            })
    });

    // "Edit the host without retyping the password." A credential the form left
    // out is the one already stored, and the plan below has to SEE it —
    // otherwise it refuses a required field the user never lost, and an OAuth
    // pair with half a value refuses too.
    //
    // Seeing it is all it does. These slots are recorded so nothing writes them
    // back: the plan trims every text value it takes, and a credential is not
    // text to be tidied — some generators emit passwords with a trailing space,
    // they survive a paste, and they match only in their exact form. The value
    // is already where it belongs, or it is moved byte for byte further down.
    let mut inherited: Vec<SecretSlot> = Vec::new();
    for field in &schema.fields {
        let Some(slot) = field.secret_slot.map(host_slot) else {
            continue;
        };
        if text_of(fields.get(&field.key)).is_some() {
            continue;
        }
        if !same_target {
            continue;
        }
        if let Some(held) = held_credential(secrets, kind, current, slot)? {
            inherited.push(slot);
            fields.insert(field.key.clone(), Value::String(held));
        }
    }

    // The OAuth client pair is settled by the posture rather than by the loop
    // below, so a schema that declares one has to be asked. Skipping this does
    // not merely lose the choice — `plan_new_account` leaves BOTH fields out of
    // the row entirely when no choice is passed, and the account comes back
    // without a client id.
    let oauth = match schema.oauth.as_ref() {
        Some(oauth) => Some(
            choose_oauth_client(
                oauth,
                fields.get(&oauth.client_id_field).and_then(Value::as_str),
                oauth
                    .client_secret_field
                    .as_deref()
                    .and_then(|key| fields.get(key))
                    .and_then(Value::as_str),
            )
            .map_err(|err| ConnectError::Invalid(err.to_string()))?,
        ),
        None => None,
    };

    // The same split every other account goes through, rather than a second
    // implementation of it: a value that landed in `config_json` when the
    // schema calls it device-local would travel to every other device, and one
    // that landed in this device's store when the schema does not would be
    // missing on every other device.
    let plan = plan_new_account(&schema, &fields, oauth.as_ref())
        .map_err(|err| ConnectError::Invalid(err.to_string()))?;

    // The built-in store is one row that already exists — created at bootstrap,
    // never deleted, and the one every calendar without a provider hangs off.
    // Choosing it as the storage must not mint a second one, and must not touch
    // its name or its config: the only thing the form carries for it is the
    // folder, which is device-local and written below like any other.
    let builtin = AdapterKind::new(account_kind)
        .is_host_internal()
        .then(|| accounts.get(account_kind))
        .transpose()
        .map_err(accounts_err)?
        .flatten();
    let account = match builtin {
        Some(existing) => existing,
        None => match current {
            // An edit of the target this device already syncs through. The row
            // keeps its id — its credentials and its device-local half are keyed by
            // that.
            //
            // It keeps its NAME only while it is the same target: a name the user
            // changed must survive a password edit, but a row now pointing at a
            // different server must not go on announcing the old one. That name is
            // what the sync panel reads back, so leaving it would have shown one
            // host while uploading to another.
            Some(existing) if same_target => accounts
                .set_config(&existing.id, &plan.config_json)
                .map_err(accounts_err)?,
            Some(existing) => {
                // The row is reused — its id is what the device-local half and the
                // keychain entries are keyed by — but it now describes a DIFFERENT
                // server, so every credential on it belongs to the old one. Not
                // inheriting them into the form was only half the job: they would
                // still be sitting in the keychain under this id, and `init_config`
                // reads them from there at open time. So the new host would have
                // been handed the old host's password anyway, by a different route.
                for field in &schema.fields {
                    if let Some(slot) = field.secret_slot.map(host_slot) {
                        let _ = secrets.delete(&existing.id, slot);
                    }
                }
                let renamed = accounts
                    .set_config(&existing.id, &plan.config_json)
                    .map_err(accounts_err)?;
                accounts
                    .rename(
                        &renamed.id,
                        &name_from(
                            kind,
                            name_source(kind).and_then(|form_key| text_of(values.get(form_key))),
                        ),
                    )
                    .map_err(accounts_err)?
            }
            None => accounts
                .create(
                    AdapterKind::new(account_kind),
                    &name_from(
                        kind,
                        name_source(kind).and_then(|form_key| text_of(values.get(form_key))),
                    ),
                    &plan.config_json,
                )
                .map_err(accounts_err)?,
        },
    };

    // Every device-local field the schema declares, so one the form left out is
    // actively cleared rather than surviving from the target before it — a
    // stale SSH key path that a later switch back to key auth would resurrect
    // as if the user had re-entered it.
    let local_fields: Vec<String> = schema
        .fields
        .iter()
        .filter(|field| field.device_local && !field.is_secret())
        .map(|field| field.key.clone())
        .collect();
    crate::account_local::store(prefs, &account.id, &local_fields, &plan.device_local)
        .map_err(prefs_err)?;

    for (slot, value) in &plan.secrets {
        if inherited.contains(slot) {
            continue;
        }
        secrets
            .store(&account.id, *slot, value)
            .map_err(secret_err)?;
    }

    // What the form did not carry: a credential inherited from the legacy
    // pseudo-account, and the OAuth refresh token, which is not a schema FIELD
    // at all. The sign-in runs before this and writes it under the kind's
    // pseudo-account, because at that moment there is no row to write it to.
    // Retiring that entry below without moving it first would take the one
    // credential the target cannot be opened without.
    let written: Vec<SecretSlot> = plan
        .secrets
        .iter()
        .map(|(slot, _)| *slot)
        .filter(|slot| !inherited.contains(slot))
        .collect();
    for (legacy_account, legacy_slot, slot) in credential_routes(kind) {
        if written.contains(&slot) {
            continue;
        }
        match secrets.retrieve(legacy_account, legacy_slot) {
            // Moved BYTE FOR BYTE — a credential is not text to be tidied, and
            // a password with a trailing space matches only in its exact form.
            Ok(value) if !value.is_empty() => secrets
                .store(&account.id, slot, &value)
                .map_err(secret_err)?,
            Ok(_) => {}
            Err(SecretError::NotFound) => {}
            // A keychain that will not ANSWER is not an absent credential.
            // Carrying on would delete the legacy entry below on the strength
            // of a read that failed.
            Err(err) => return Err(secret_err(err)),
        }
    }

    select_account(prefs, Some(&account.id)).map_err(prefs_err)?;
    retire_legacy(prefs, secrets, kind)?;

    // A switch leaves ONE account, not two. The row this device moved off is a
    // sync-only account: it never travelled, nothing else references it, and
    // leaving it behind puts a target the user replaced back in their account
    // list with a live credential attached to it.
    //
    // Unless it is the built-in store, which is not that kind of row at all:
    // every calendar without a provider hangs off it, and deleting it to tidy
    // up a storage choice would take the user's local data with it. Its
    // device-local half IS cleared — the folder means something only while this
    // device mirrors into it, and a later switch back that silently reused a
    // path the user has since forgotten is worse than asking again.
    if let Some(old) = previous.filter(|account| account.adapter_kind != account_kind) {
        secrets.delete_all(&old.id).map_err(secret_err)?;
        crate::account_local::forget_all(prefs, &old.id).map_err(prefs_err)?;
        if !old.adapter_kind.is_host_internal() {
            accounts.delete(&old.id).map_err(accounts_err)?;
        }
    }

    Ok(account.id)
}

/// Stop syncing, and leave nothing any reader could act on.
///
/// ## The order of the deletes, and why a failure part-way is still safe
///
/// Two readers can start a sync: the account path, gated on the pointer, and
/// the legacy path, gated on `sync.adapter.kind`. They are mutually exclusive —
/// a pointer switches the legacy one off entirely — so the FIRST write here has
/// to be the one that stops whichever is live:
///
/// 1. the account row, which stops the pointer path immediately (a pointer at a
///    missing row refuses, it does not fall back);
/// 2. `sync.adapter.kind`, which stops the legacy path on a device that never
///    had a pointer;
///
/// after which no configuration on this device can produce an adapter. What
/// follows is hygiene, and a failure in it leaves values under an account id
/// nothing references any more — unreachable, never a working target:
///
/// 3. the rest of the legacy preferences;
/// 4. the legacy keychain pseudo-accounts, except the encryption key;
/// 5. the row's own credentials and device-local half;
/// 6. the pointer, last, because it is what names the row in step 5.
pub fn disconnect(
    prefs: &UserPrefsRepo<'_>,
    accounts: &AccountsRepo<'_>,
    secrets: &dyn SecretStore,
) -> Result<(), ConnectError> {
    let chosen = selected_account_id_result(prefs).map_err(prefs_err)?;

    if let Some(id) = chosen.as_deref() {
        match accounts.delete(id) {
            // A pointer at a row that is already gone is the state a previous,
            // interrupted disconnect leaves. Finishing the job is the point.
            Ok(()) | Err(AccountsError::NotFound(_)) => {}
            Err(err) => return Err(accounts_err(err)),
        }
    }

    prefs.delete(PREF_ADAPTER_KIND).map_err(prefs_err)?;
    // Every kind's preferences, not just the one that was selected. The kind
    // preference is gone by now, so there is nothing left to ask WHICH target
    // this was — and a device that has switched backends over the years has
    // values for several of them lying around. None may survive a disconnect.
    for (key, _) in all_pref_keys() {
        prefs.delete(key).map_err(prefs_err)?;
    }

    for (pseudo_account, slot) in SECRET_SLOTS {
        // The dataset's encryption key is not a property of the target and does
        // not go with it. Losing it means losing the ability to rejoin the
        // dataset at all — a disconnected device that reconnects would be
        // holding ciphertext it can no longer read.
        if *pseudo_account == SECRET_ACCOUNT_E2E {
            continue;
        }
        secrets.delete(pseudo_account, *slot).map_err(secret_err)?;
    }

    if let Some(id) = chosen.as_deref() {
        secrets.delete_all(id).map_err(secret_err)?;
        crate::account_local::forget_all(prefs, id).map_err(prefs_err)?;
    }

    // [`PREF_MIGRATED_ACCOUNT`] deliberately stays. It is a marker that PREVENTS
    // the migration from running again, not a record of a target, and clearing
    // it would let one preference this sweep somehow missed be turned back into
    // an account on the next launch. Keeping it can only refuse work.
    select_account(prefs, None).map_err(prefs_err)?;
    Ok(())
}

/// The credential this device already holds for `kind` in `slot`, wherever it
/// still lives.
///
/// What the hosts' builders call where they used to read a keychain
/// pseudo-account directly, so "edit the host without retyping the password"
/// survives the move to accounts.
///
/// ## Why the legacy entry is preferred, and why that is not a second truth
///
/// [`connect`] DELETES every legacy entry for the kind it writes. So an entry
/// that is there was written since the last connect — by an OAuth sign-in,
/// which has no row to write to yet, or on a device that has never connected
/// through this path at all. Preferring the row would hand a target that was
/// just re-authorised the refresh token it had only a moment ago replaced, and
/// the probe would fail with a credential the user had already fixed.
pub fn stored_secret(
    prefs: &UserPrefsRepo<'_>,
    accounts: &AccountsRepo<'_>,
    secrets: &dyn SecretStore,
    kind: &str,
    slot: SecretSlot,
) -> Option<String> {
    if let Some(value) = legacy_credential(secrets, kind, slot) {
        return Some(value);
    }
    let account_kind = account_kind_for(kind)?;
    let id = selected_account_id(prefs)?;
    let account = accounts.get(&id).ok().flatten()?;
    // A row of a different kind holds a different server's credential.
    if account.adapter_kind != account_kind {
        return None;
    }
    secrets
        .retrieve(&account.id, slot)
        .ok()
        .filter(|value| !value.is_empty())
}

/// What this device syncs through, for a settings card: the kind in the
/// spelling both frontends switch on, and one line naming the target.
///
/// `None` on a device that has not chosen an account — its caller then reads
/// the legacy preferences it has not moved off yet.
///
/// The detail is the account's DISPLAY NAME rather than a re-assembled
/// `user@host:port/path`. That name is derived from the target when the account
/// is created and the user can change it afterwards, which makes it strictly
/// better here: two SFTP targets on one server were previously one string
/// repeated twice.
pub fn summary(prefs: &UserPrefsRepo<'_>, accounts: &AccountsRepo<'_>) -> SummaryOutcome {
    let Some(id) = selected_account_id(prefs) else {
        return SummaryOutcome::NotChosen;
    };
    match accounts.get(&id).ok().flatten() {
        Some(account) => match stored_kind_for(account.adapter_kind.as_str()) {
            Some(kind) => SummaryOutcome::Chosen(kind.to_string(), account.display_name),
            None => SummaryOutcome::Missing,
        },
        None => SummaryOutcome::Missing,
    }
}

/// What the sync panel should say about the chosen target.
///
/// Three answers rather than an `Option`, because two of them used to be the
/// same one and the caller then guessed wrong. A device that has never chosen
/// falls back to the legacy preferences, which is right — that is where a
/// device that has not connected on this build still keeps its target. A device
/// whose pointer names a row that is GONE must not: the legacy preferences are
/// still complete on a migrated device by design, so the panel would announce
/// the pre-migration target while the orchestrator was somewhere else entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryOutcome {
    /// A chosen account: its stored-kind name and its display name.
    Chosen(String, String),
    /// This device has not chosen. The caller may fall back.
    NotChosen,
    /// A pointer to a row that no longer exists, or one this build cannot name.
    /// The caller must NOT fall back — say nothing rather than something wrong.
    Missing,
}

/// The legacy pseudo-account value for one of `kind`'s credentials.
fn legacy_credential(secrets: &dyn SecretStore, kind: &str, slot: SecretSlot) -> Option<String> {
    credential_routes(kind)
        .into_iter()
        .find(|(_, _, target)| *target == slot)
        .and_then(|(pseudo_account, legacy_slot, _)| {
            secrets.retrieve(pseudo_account, legacy_slot).ok()
        })
        .filter(|value| !value.is_empty())
}

/// The credential a connect form left out: the legacy entry if one is still
/// there, else what the row this device already syncs through holds.
///
/// A keychain that will not answer for the ROW is an error rather than an
/// absent value — the row is the record this call is about to rewrite, and
/// treating "cannot read" as "not set" would drop a credential the user still
/// has whenever the keystore is locked or busy.
fn held_credential(
    secrets: &dyn SecretStore,
    kind: &str,
    current: Option<&Account>,
    slot: SecretSlot,
) -> Result<Option<String>, ConnectError> {
    if let Some(value) = legacy_credential(secrets, kind, slot) {
        return Ok(Some(value));
    }
    let Some(account) = current else {
        return Ok(None);
    };
    match secrets.retrieve(&account.id, slot) {
        Ok(value) => Ok(Some(value).filter(|value| !value.is_empty())),
        Err(SecretError::NotFound) => Ok(None),
        Err(err) => Err(secret_err(err)),
    }
}

/// Remove the record the account row replaces: `kind`'s preferences, the kind
/// preference itself, and `kind`'s keychain pseudo-accounts.
///
/// Only this kind's. The pointer now decides, so another kind's leftovers can
/// no longer be read by anything — and a user switching back to a backend they
/// used last year keeps the password they never re-typed. A DISCONNECT clears
/// them all, because there is no pointer left to stop them being read.
fn retire_legacy(
    prefs: &UserPrefsRepo<'_>,
    secrets: &dyn SecretStore,
    kind: &str,
) -> Result<(), ConnectError> {
    // First, so a failure below leaves preferences no reader consults rather
    // than a kind whose fields are half gone.
    prefs.delete(PREF_ADAPTER_KIND).map_err(prefs_err)?;
    for field in fields_for(kind).unwrap_or(&[]) {
        if let Some(key) = field.pref {
            prefs.delete(key).map_err(prefs_err)?;
        }
    }
    for (pseudo_account, legacy_slot, _) in credential_routes(kind) {
        secrets
            .delete(pseudo_account, legacy_slot)
            .map_err(secret_err)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use plugin_core::account_schema::AccountSchema;
    use plugin_core::manifest::PluginManifest;
    use std::sync::Arc;
    use sync_core::SyncAdapter;

    use super::*;
    use crate::db::DbHandle;
    use crate::sync_target::persist::FakeSecrets;

    /// The manifests as they ship, so the field keys these tests assert on are
    /// the ones the plugins actually declare. A hand-written twin of a schema
    /// drifts exactly the way every other hand-written twin in this workspace
    /// has, and the whole failure mode here is a key one character off.
    fn schema_for(stored_kind: &str) -> AccountSchema {
        let bytes: &[u8] = match stored_kind {
            // Folder sync folded into the built-in store, which is
            // where the folder field is declared now.
            "local" => include_bytes!("../../../adapter-local/plugin.json"),
            "webdav" => include_bytes!("../../../adapter-webdav-plugin/plugin.json"),
            "sftp" => include_bytes!("../../../adapter-sftp-plugin/plugin.json"),
            "ftp" => include_bytes!("../../../adapter-ftp-plugin/plugin.json"),
            "dropbox" => include_bytes!("../../../adapter-dropbox-plugin/plugin.json"),
            // Adopted by the Google adapter, which is where its schema
            // lives now — see `adopts_adapter_kinds`.
            "googledrive" => include_bytes!("../../../adapter-google-plugin/plugin.json"),
            other => panic!("no shipped manifest for {other}"),
        };
        PluginManifest::from_bytes(bytes)
            .expect("the shipped manifest parses")
            .account
            .expect("the shipped manifest declares an account schema")
    }

    /// Serves whichever shipped schema the kind under test needs.
    struct Plugins(&'static str);

    impl SyncPlugins for Plugins {
        fn resolve(&self, adapter_kind: &str) -> Option<(String, AccountSchema)> {
            Some((
                format!("com.aperio.sync-adapter-{adapter_kind}"),
                schema_for(self.0),
            ))
        }
        fn open(
            &self,
            _plugin_id: &str,
            _config_json: String,
        ) -> Result<Arc<dyn SyncAdapter>, String> {
            Err("no test here opens an adapter".into())
        }
    }

    fn values(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect()
    }

    fn webdav_form() -> Map<String, Value> {
        values(&[
            ("url", "https://cloud.example.test/dav/aperio/"),
            ("user", "anna"),
            ("password", "hunter2"),
        ])
    }

    /// A target as the OLD code would have left it: the kind, the fields, the
    /// keychain pseudo-account.
    fn legacy_webdav(prefs: &UserPrefsRepo<'_>, secrets: &FakeSecrets) {
        prefs.set(PREF_ADAPTER_KIND, "webdav").unwrap();
        prefs
            .set(PREF_WEBDAV_URL, "https://old.example.test/dav/")
            .unwrap();
        prefs.set(PREF_WEBDAV_USER, "anna").unwrap();
        secrets
            .store(SECRET_ACCOUNT_WEBDAV, SecretSlot::Password, "old-password")
            .unwrap();
    }

    struct Fixture {
        db: DbHandle,
        secrets: FakeSecrets,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                db: DbHandle::open_in_memory().unwrap(),
                secrets: FakeSecrets::default(),
            }
        }
    }

    /// The whole point, in one test: what connect leaves behind is a row, its
    /// credential under the row's id, a pointer — and none of the record it
    /// replaces.
    #[test]
    fn connect_writes_a_row_a_secret_a_pointer_and_clears_the_legacy_copy() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        legacy_webdav(&prefs, &f.secrets);

        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &webdav_form(),
        )
        .expect("a complete form connects");

        let account = accounts.get(&id).unwrap().expect("the row exists");
        assert_eq!(account.adapter_kind, "webdav");
        let config: Value = serde_json::from_str(&account.config_json).unwrap();
        assert_eq!(
            config.get("url").and_then(Value::as_str),
            Some("https://cloud.example.test/dav/aperio/"),
        );
        assert!(
            !account.config_json.contains("hunter2"),
            "config_json is appended to the event log in the clear: {}",
            account.config_json,
        );
        assert_eq!(
            f.secrets.retrieve(&id, SecretSlot::Password).ok(),
            Some("hunter2".to_string()),
        );
        assert_eq!(selected_account_id(&prefs).as_deref(), Some(id.as_str()));

        // And the record it replaces is gone — every part of it.
        assert_eq!(prefs.get(PREF_ADAPTER_KIND).unwrap(), None);
        assert_eq!(prefs.get(PREF_WEBDAV_URL).unwrap(), None);
        assert_eq!(prefs.get(PREF_WEBDAV_USER).unwrap(), None);
        assert!(
            f.secrets
                .retrieve(SECRET_ACCOUNT_WEBDAV, SecretSlot::Password)
                .is_err(),
            "the legacy credential survived; two records that can disagree is the bug",
        );
    }

    /// The hosts probe before they call this, and a probe that says no must
    /// leave the working setup exactly as it was — including on the second
    /// connect, where a rejected edit must not disturb the row already there.
    #[test]
    fn a_connect_that_never_happens_leaves_the_previous_setup_untouched() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);

        let first = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &webdav_form(),
        )
        .unwrap();

        // What a rejected probe means: the host returns before it reaches this
        // module at all. Nothing may have changed.
        let before = accounts.get(&first).unwrap().unwrap();
        assert_eq!(selected_account_id(&prefs).as_deref(), Some(first.as_str()));

        // And a form this module itself refuses — an empty required URL —
        // changes nothing either.
        let err = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &values(&[("url", "  "), ("user", "someone-else")]),
        )
        .expect_err("a blank required field must refuse");
        assert!(err.to_string().contains("url"), "{err}");

        let after = accounts.get(&first).unwrap().unwrap();
        assert_eq!(after.config_json, before.config_json);
        assert_eq!(accounts.list().unwrap().len(), 2, "local plus this one");
        assert_eq!(selected_account_id(&prefs).as_deref(), Some(first.as_str()));
        assert_eq!(
            f.secrets.retrieve(&first, SecretSlot::Password).ok(),
            Some("hunter2".to_string()),
        );
    }

    /// Nothing a restore path could find: not the row, not the preferences, not
    /// the keychain.
    #[test]
    fn disconnect_leaves_nothing_that_could_bring_the_target_back() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        // A device that also still carries an older kind's leftovers, which is
        // what switching backends over the years produces.
        prefs.set(PREF_SFTP_HOST, "backup.example.test").unwrap();
        f.secrets
            .store(SECRET_ACCOUNT_SFTP, SecretSlot::Password, "sftp-password")
            .unwrap();
        legacy_webdav(&prefs, &f.secrets);
        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &webdav_form(),
        )
        .unwrap();

        disconnect(&prefs, &accounts, &f.secrets).expect("disconnects");

        assert_eq!(selected_account_id(&prefs), None);
        assert_eq!(accounts.get(&id).unwrap().map(|a| a.id), None);
        assert!(
            f.secrets.retrieve(&id, SecretSlot::Password).is_err(),
            "the row's credential outlived the row",
        );
        for (key, kind) in all_pref_keys() {
            assert_eq!(
                prefs.get(key).unwrap(),
                None,
                "{kind} left {key} behind, and the legacy reader acts on it",
            );
        }
        assert_eq!(prefs.get(PREF_ADAPTER_KIND).unwrap(), None);
        for (pseudo_account, slot) in SECRET_SLOTS {
            if *pseudo_account == SECRET_ACCOUNT_E2E {
                continue;
            }
            assert!(
                f.secrets.retrieve(pseudo_account, *slot).is_err(),
                "{pseudo_account} still holds a credential for a target the user left",
            );
        }
    }

    /// The key is not a property of the target, and the ordinary
    /// delete-the-account sweep would have taken it. Losing it means losing the
    /// ability to rejoin the dataset at all.
    #[test]
    fn disconnect_keeps_the_encryption_key_and_its_flag() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        f.secrets
            .store(
                SECRET_ACCOUNT_E2E,
                SecretSlot::SyncEncryptionKey,
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            )
            .unwrap();
        prefs
            .set(sync_engine::whitelist::PREF_E2E_ENABLED, "true")
            .unwrap();
        connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &webdav_form(),
        )
        .unwrap();

        disconnect(&prefs, &accounts, &f.secrets).unwrap();

        assert_eq!(
            f.secrets
                .retrieve(SECRET_ACCOUNT_E2E, SecretSlot::SyncEncryptionKey)
                .ok(),
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string()),
        );
        assert_eq!(
            prefs
                .get(sync_engine::whitelist::PREF_E2E_ENABLED)
                .unwrap()
                .as_deref(),
            Some("true"),
        );
    }

    /// One target per device. Switching backends replaces the row rather than
    /// collecting them, and the pointer names the one that is left.
    #[test]
    fn switching_kinds_leaves_exactly_one_account_and_one_pointer() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);

        let webdav = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &webdav_form(),
        )
        .unwrap();
        // The pin the SFTP schema demands is not this module's business — it is
        // read when the adapter is OPENED, not when the choice is written.
        let sftp = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("sftp"),
            "sftp",
            &values(&[
                ("host", "backup.example.test"),
                ("port", "22"),
                ("user", "anna"),
                ("path", "/srv/aperio"),
                ("auth_method", "key"),
                ("key_path", "/home/anna/.ssh/id_ed25519"),
                ("key_passphrase", "keypass"),
            ]),
        )
        .unwrap();

        assert_ne!(webdav, sftp);
        let sync_accounts: Vec<String> = accounts
            .list()
            .unwrap()
            .into_iter()
            .filter(|a| !a.adapter_kind.is_host_internal())
            .map(|a| a.id)
            .collect();
        assert_eq!(sync_accounts, vec![sftp.clone()]);
        assert_eq!(selected_account_id(&prefs).as_deref(), Some(sftp.as_str()));
        assert!(
            f.secrets.retrieve(&webdav, SecretSlot::Password).is_err(),
            "the target the user left kept a live credential",
        );
    }

    /// The renames the two hosts' wire shapes carry: both call it `path`, and
    /// the plugins call it `remote_root` and `base_path`. A value under the
    /// wrong key is not an error anywhere — no plugin sets
    /// `deny_unknown_fields` — it is a field that silently takes its default.
    #[test]
    fn a_form_key_reaches_the_plugins_own_field_name() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);

        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("local"),
            "local",
            &values(&[("path", "/srv/aperio")]),
        )
        .unwrap();
        // The local folder's one field is device-local, so the row that travels
        // carries nothing at all.
        assert_eq!(accounts.get(&id).unwrap().unwrap().config_json, "{}");
        assert_eq!(
            crate::account_local::load(&prefs, &id, &["remote_root".to_string()])
                .get("remote_root")
                .and_then(Value::as_str),
            Some("/srv/aperio"),
        );

        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("dropbox"),
            "dropbox",
            &values(&[
                ("client_id", "app-key"),
                ("client_secret", "app-secret"),
                ("path", "/Apps/Aperio"),
            ]),
        )
        .unwrap();
        let config: Value =
            serde_json::from_str(&accounts.get(&id).unwrap().unwrap().config_json).unwrap();
        assert_eq!(
            config.get("base_path").and_then(Value::as_str),
            Some("/Apps/Aperio"),
        );
        assert!(config.get("path").is_none(), "the plugin does not read it");
        assert_eq!(
            f.secrets
                .retrieve(&id, SecretSlot::OauthClientSecret)
                .ok()
                .as_deref(),
            Some("app-secret"),
            "the OAuth pair is settled by the posture and would be dropped otherwise",
        );
    }

    /// The refresh token the sign-in wrote before there was a row to write it
    /// to. Retiring the pseudo-account without carrying it across would take
    /// the one credential a Dropbox target cannot be opened without.
    #[test]
    fn an_oauth_refresh_token_moves_onto_the_row_it_could_not_be_written_to() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        f.secrets
            .store(
                SECRET_ACCOUNT_DROPBOX,
                SecretSlot::RefreshToken,
                "r3fresh-from-the-sign-in",
            )
            .unwrap();

        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("dropbox"),
            "dropbox",
            &values(&[("client_id", "app-key"), ("client_secret", "app-secret")]),
        )
        .unwrap();

        assert_eq!(
            f.secrets.retrieve(&id, SecretSlot::RefreshToken).ok(),
            Some("r3fresh-from-the-sign-in".to_string()),
        );
        assert!(f
            .secrets
            .retrieve(SECRET_ACCOUNT_DROPBOX, SecretSlot::RefreshToken)
            .is_err());
    }

    /// A URL-only edit keeps the password. The credential the form leaves out
    /// is the one already stored, and after the first connect that is the row's
    /// — the legacy entry is gone by then.
    #[test]
    fn an_edit_that_omits_the_password_keeps_the_stored_one() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);

        let first = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &webdav_form(),
        )
        .unwrap();
        let again = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            // The SAME target, password omitted. Changing the URL as well is
            // what this test used to do, and it is exactly the case the
            // inheritance rule now refuses — the stored credential belongs to
            // the server that was there before.
            &values(&[
                ("url", "https://cloud.example.test/dav/aperio/"),
                ("user", "anna"),
            ]),
        )
        .unwrap();

        assert_eq!(first, again, "an edit of the same target keeps the row");
        assert_eq!(
            f.secrets.retrieve(&again, SecretSlot::Password).ok(),
            Some("hunter2".to_string()),
        );
        let config: Value =
            serde_json::from_str(&accounts.get(&again).unwrap().unwrap().config_json).unwrap();
        assert_eq!(
            config.get("url").and_then(Value::as_str),
            Some("https://cloud.example.test/dav/aperio/"),
        );
    }

    /// FTP's password is `required` in the shipped schema, so the same edit has
    /// to inherit it or the plan refuses a field the user never lost.
    #[test]
    fn a_required_credential_is_inherited_rather_than_demanded_again() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        let form = values(&[
            ("host", "ftp.example.test"),
            ("port", "21"),
            ("user", "anna"),
            ("path", "/aperio"),
            ("mode", "explicit"),
            ("password", "hunter2"),
        ]);
        let id = connect(&prefs, &accounts, &f.secrets, &Plugins("ftp"), "ftp", &form).unwrap();

        let again = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("ftp"),
            "ftp",
            // Same host, same everything, password omitted — the case the
            // inheritance exists for. Pointing at `ftp2` instead would be a
            // different server, and the stored password is not its.
            &values(&[
                ("host", "ftp.example.test"),
                ("port", "21"),
                ("user", "anna"),
                ("path", "/aperio"),
                ("mode", "explicit"),
            ]),
        )
        .expect("an omitted password is the stored one, not a missing required field");
        assert_eq!(id, again);
        assert_eq!(
            f.secrets.retrieve(&again, SecretSlot::Password).ok(),
            Some("hunter2".to_string()),
        );
    }

    /// A credential the form did not carry is never rewritten, and therefore
    /// never tidied.
    ///
    /// Some generators emit passwords with a trailing space and they survive a
    /// paste. Trimming one on an edit that never touched it turns a working
    /// target into an authentication failure against what looks like the right
    /// password — and the form's own values ARE trimmed, so routing the
    /// inherited one through the same split would have done exactly that.
    #[test]
    fn an_inherited_credential_crosses_byte_for_byte() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        f.secrets
            .store(SECRET_ACCOUNT_WEBDAV, SecretSlot::Password, " hunter2 ")
            .unwrap();

        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &values(&[("url", "https://cloud.example.test/dav/"), ("user", "anna")]),
        )
        .unwrap();

        assert_eq!(
            f.secrets.retrieve(&id, SecretSlot::Password).ok(),
            Some(" hunter2 ".to_string()),
        );
    }

    /// The port is a JSON number in the row: the plugins declare `u16`, serde
    /// will not coerce `"21"`, and the whole init config fails with it.
    #[test]
    fn a_port_is_a_number_in_the_row() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("ftp"),
            "ftp",
            &values(&[
                ("host", "ftp.example.test"),
                ("port", "2121"),
                ("user", "anna"),
                ("password", "hunter2"),
            ]),
        )
        .unwrap();
        let config: Value =
            serde_json::from_str(&accounts.get(&id).unwrap().unwrap().config_json).unwrap();
        assert_eq!(config["port"], serde_json::json!(2121));
    }

    /// Where the hosts' builders look for the credential a form left out.
    #[test]
    fn the_stored_credential_is_found_before_and_after_the_move() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        legacy_webdav(&prefs, &f.secrets);

        assert_eq!(
            stored_secret(
                &prefs,
                &accounts,
                &f.secrets,
                "webdav",
                SecretSlot::Password
            )
            .as_deref(),
            Some("old-password"),
            "a device that has not connected through the account path yet",
        );

        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &webdav_form(),
        )
        .unwrap();
        assert_eq!(
            stored_secret(
                &prefs,
                &accounts,
                &f.secrets,
                "webdav",
                SecretSlot::Password
            )
            .as_deref(),
            Some("hunter2"),
        );

        // A sign-in that ran since: it has no row to write to, so it lands in
        // the pseudo-account, and it is the newer of the two.
        f.secrets
            .store(SECRET_ACCOUNT_WEBDAV, SecretSlot::Password, "re-typed")
            .unwrap();
        assert_eq!(
            stored_secret(
                &prefs,
                &accounts,
                &f.secrets,
                "webdav",
                SecretSlot::Password
            )
            .as_deref(),
            Some("re-typed"),
        );
        assert_eq!(
            summary(&prefs, &accounts),
            SummaryOutcome::Chosen("webdav".to_string(), "cloud.example.test".to_string()),
        );
        assert!(accounts.get(&id).unwrap().is_some());
    }

    /// A pointer to a row that is gone must not read like a device that never
    /// chose one: the caller falls back to the legacy preferences on the
    /// second, and on a migrated device those still name the pre-migration
    /// target, so the panel would announce a server the round is not using.
    #[test]
    fn a_pointer_to_a_missing_row_is_not_an_unchosen_device() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);

        assert_eq!(summary(&prefs, &accounts), SummaryOutcome::NotChosen);

        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &webdav_form(),
        )
        .unwrap();
        accounts.delete(&id).unwrap();

        assert_eq!(summary(&prefs, &accounts), SummaryOutcome::Missing);
    }

    /// The SFTP key passphrase sits in a pseudo-account of its own in the
    /// `Password` slot and belongs in `KeyPassphrase` on a row that holds both.
    /// Sharing one slot would make the second write win and the value come back
    /// under both names.
    #[test]
    fn an_sftp_key_passphrase_does_not_become_a_password() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        f.secrets
            .store(SECRET_ACCOUNT_SFTP, SecretSlot::Password, "pw")
            .unwrap();
        f.secrets
            .store(SECRET_ACCOUNT_SFTP_KEY, SecretSlot::Password, "keypass")
            .unwrap();

        let id = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("sftp"),
            "sftp",
            &values(&[
                ("host", "backup.example.test"),
                ("port", "22"),
                ("user", "anna"),
                ("path", "/srv/aperio"),
                ("auth_method", "key"),
                ("key_path", "/home/anna/.ssh/id_ed25519"),
            ]),
        )
        .unwrap();

        assert_eq!(
            f.secrets
                .retrieve(&id, SecretSlot::Password)
                .ok()
                .as_deref(),
            Some("pw"),
        );
        assert_eq!(
            f.secrets
                .retrieve(&id, SecretSlot::KeyPassphrase)
                .ok()
                .as_deref(),
            Some("keypass"),
        );
        // And the two device-local answers stayed off the row that travels.
        let config: Value =
            serde_json::from_str(&accounts.get(&id).unwrap().unwrap().config_json).unwrap();
        assert!(config.get("key_path").is_none(), "{config}");
        assert!(config.get("auth_method").is_none(), "{config}");
    }

    /// Every kind this build serves must be connectable, or the form for it
    /// leads nowhere.
    #[test]
    fn every_kind_this_build_serves_resolves_to_an_account_kind() {
        for (kind, _) in KINDS {
            let account_kind =
                account_kind_for(kind).unwrap_or_else(|| panic!("{kind} has no account kind"));
            assert_eq!(stored_kind_for(account_kind), Some(*kind));
            assert!(
                !schema_for(kind).fields.is_empty(),
                "{kind} declares no fields to split",
            );
        }
    }

    /// The one that would have sent a password to a server it does not belong
    /// to.
    ///
    /// Editing only the credential keeps the stored one visible to the plan, so
    /// "change the password without retyping the URL" still works. Pointing at
    /// a DIFFERENT server of the same kind must not: the row's keychain entry
    /// is the old host's, and inheriting it means the probe authenticates
    /// against the new host with the old host's password.
    #[test]
    fn a_different_server_of_the_same_kind_does_not_inherit_the_password() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);

        let first = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &values(&[
                ("url", "https://one.example.test/dav/"),
                ("user", "anna"),
                ("password", "first-secret"),
            ]),
        )
        .expect("first connect");

        // Same kind, same user, DIFFERENT host, password left blank.
        connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &values(&[("url", "https://two.example.test/dav/"), ("user", "anna")]),
        )
        .expect("WebDAV allows an anonymous target, so this connects");

        // Not inherited into the form, AND not left sitting in the keychain
        // under the reused row id — `init_config` would have read it back and
        // handed the new host the old host's password by a different route.
        assert!(
            f.secrets.retrieve(&first, SecretSlot::Password).is_err(),
            "the previous server's password survived a change of target",
        );
    }

    /// The behaviour the narrowing must not cost: same target, new password,
    /// nothing else retyped.
    #[test]
    fn the_same_target_still_takes_a_new_password_alone() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);

        let first = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &values(&[
                ("url", "https://one.example.test/dav/"),
                ("user", "anna"),
                ("password", "old"),
            ]),
        )
        .expect("first connect");

        let again = connect(
            &prefs,
            &accounts,
            &f.secrets,
            &Plugins("webdav"),
            "webdav",
            &values(&[
                ("url", "https://one.example.test/dav/"),
                ("user", "anna"),
                ("password", "new"),
            ]),
        )
        .expect("same target, new password");

        assert_eq!(first, again, "the row is reused, not replaced");
        assert_eq!(
            f.secrets.retrieve(&again, SecretSlot::Password).unwrap(),
            "new",
        );
    }
}
