//! Editing an existing DATA account in place — server URLs, endpoints,
//! usernames, credentials — with both halves synchronized: the non-secret
//! config travels as `account.updated`, changed secrets as `credential.set`
//! (behind the E2E gate in [`crate::credential_sync`]).
//!
//! The rules are the ones `sync_target::connect` established for the one
//! account kind that could already be edited:
//!
//!   - **A secret field the form left BLANK means "keep what is stored".**
//!     The stored value is inherited so the plan's required-field validation
//!     doesn't refuse a credential the user never lost — and the inherited
//!     slots are recorded so nothing writes them back (the plan trims text;
//!     a credential must survive byte for byte).
//!   - **The form owns exactly the schema's declared field keys.** Everything
//!     else in the stored config — the host's `client_source` /
//!     `client_fingerprint` bookkeeping, an OAuth posture's client id, a
//!     legacy leftover — carries over untouched. An optional declared field
//!     the user cleared comes through as absent and is therefore dropped:
//!     clearing works, and nothing the form never showed can be lost.
//!   - **Device-local fields are rewritten wholesale** (an absent one is
//!     cleared), staying off the synced row as always.
//!   - **The adapter is re-registered** so the new config is live at once —
//!     re-registering an id overwrites the prior entry, same as reconnect.
//!
//! OAuth client identity is deliberately out of scope: the client id/secret
//! pair is settled by the sign-in posture and swapping it invalidates the
//! stored tokens, so that remains the reconnect flow's job. `plan_new_account`
//! with no OAuth choice skips those two fields by construction.
//!
//! Known limitation, deliberate for now: an OPTIONAL stored secret cannot be
//! CLEARED through an edit — blank always means "keep". The two meanings of
//! an empty secret field are indistinguishable without a dedicated clear
//! affordance in both editors; until one exists, shedding a stored optional
//! credential means deleting and re-adding the account (documented in the
//! tutorial).

use serde_json::{Map, Value};
use sync_core::{AccountPayload, SyncEvent};
use sync_engine::{EventLogWriter, SecretError, SecretSlot, SecretStore};

use plugin_core::PluginManager;

use crate::account_local;
use crate::account_setup::{host_slot, plan_new_account, AccountSetupError};
use crate::accounts::{Account, AccountsRepo};
use crate::credential_sync;
use crate::db::SharedConn;
use crate::registry::AdapterRegistry;
use crate::user_prefs::UserPrefsRepo;

/// Apply an edit to `account_id` from schema-keyed `values` (the same shape
/// the connect form collects). Returns the updated row; the caller kicks its
/// platform's refresher afterwards.
#[allow(clippy::too_many_arguments)]
pub fn update_account_values(
    conn: &SharedConn,
    registry: &AdapterRegistry,
    plugin_manager: &PluginManager,
    secrets: &dyn SecretStore,
    event_log: &EventLogWriter,
    account_id: &str,
    display_name: Option<&str>,
    values: &Map<String, Value>,
) -> Result<Account, AccountSetupError> {
    let repo = AccountsRepo::new(conn);
    let account = repo
        .get(account_id)
        .map_err(|e| AccountSetupError::Config(e.to_string()))?
        .ok_or_else(|| {
            AccountSetupError::InvalidInput(format!("account '{account_id}' not found"))
        })?;
    if account.adapter_kind.is_host_internal() {
        return Err(AccountSetupError::InvalidInput(
            "this account is built in and has nothing to edit".into(),
        ));
    }
    let plugin = plugin_manager
        .plugin_for_adapter_kind(account.adapter_kind.as_str())
        .ok_or_else(|| AccountSetupError::Config("no plugin serves this adapter kind".into()))?;
    let schema = plugin.manifest.account.clone().ok_or_else(|| {
        AccountSetupError::Config("this adapter declares no account schema".into())
    })?;

    // Inherit every stored credential whose field the form left blank, and
    // remember the slot so it is not written back below.
    let (merged, inherited) = inherit_stored_secrets(secrets, &account.id, &schema, values)?;

    // No OAuth choice: the posture keys carry over from the stored config
    // below, and the client pair is not this flow's to change.
    let plan = plan_new_account(&schema, &merged, None)?;

    let config_json = merged_config(&schema, &plan.config_json, &account.config_json);

    // Persist: name first (cheap, reversible), then config, then the local
    // half, then the keychain — the same order connect uses.
    if let Some(name) = display_name {
        let name = name.trim();
        if !name.is_empty() && name != account.display_name {
            repo.rename(&account.id, name)
                .map_err(|e| AccountSetupError::Config(e.to_string()))?;
        }
    }
    let updated = repo
        .set_config(&account.id, &config_json)
        .map_err(|e| AccountSetupError::Config(e.to_string()))?;

    let prefs = UserPrefsRepo::new(conn);
    let device_local_fields: Vec<String> = schema
        .fields
        .iter()
        .filter(|f| f.device_local && !f.is_secret())
        .map(|f| f.key.clone())
        .collect();
    account_local::store(
        &prefs,
        &account.id,
        &device_local_fields,
        &plan.device_local,
    )
    .map_err(|e| AccountSetupError::Config(e.to_string()))?;

    for (slot, value) in &plan.secrets {
        if inherited.contains(slot) {
            continue;
        }
        secrets
            .store(&account.id, *slot, value)
            .map_err(|e| AccountSetupError::Secret(e.to_string()))?;
        credential_sync::emit_credential_set(
            event_log,
            conn,
            plugin_manager,
            &account.id,
            *slot,
            value,
        );
    }

    // The config change travels as a full-row update — the applier upserts all
    // columns, so the payload carries the complete row. Gated on the kind
    // actually travelling, like every account.* emit.
    //
    // Emitted BEFORE the registration attempt, not after: the row is already
    // committed either way, and a registration hiccup (a locked keychain, a
    // plugin refusing to open) used to leave the other devices holding the
    // OLD config while this one held the new — with the changed credential
    // already on the wire above, silently divergent forever. Registration
    // itself is retried by the fingerprint sweep every round.
    if crate::accounts::travels_between_devices(plugin_manager, updated.adapter_kind.as_str()) {
        event_log.append(SyncEvent::AccountUpdated(AccountPayload {
            id: updated.id.clone(),
            adapter_kind: updated.adapter_kind.as_str().to_string(),
            display_name: updated.display_name.clone(),
            config_json: updated.config_json.clone(),
            created_at: updated.created_at.clone(),
            updated_at: updated.updated_at.clone(),
        }));
    }

    // Live immediately: re-registering the id replaces the running adapter
    // (reconnect's precedent — no unregister, the maps overwrite). A failure
    // here still surfaces to the caller, but everything persisted above
    // stands — the sweep retries the registration.
    registry
        .register(&updated)
        .map_err(|e| AccountSetupError::Config(e.to_string()))?;

    Ok(updated)
}

/// The edited config: the plan's config (what the form said) plus every
/// stored key the form does NOT own. The form owns its declared non-secret
/// field keys — a declared key absent from the plan was cleared by the user
/// and stays dropped — while everything else (the host's `client_source` /
/// `client_fingerprint` bookkeeping, legacy leftovers) carries over.
///
/// The OAuth client pair is NOT owned by this form even though the schema
/// declares it: `plan_new_account` skips those two fields by construction and
/// both edit UIs drop them, so treating them as "owned" would DELETE the
/// stored client id on every edit — bricking a bring-your-own OAuth account
/// (registration then fails with "no client_id") over an edit that never
/// touched it. They carry over like every other non-form key.
fn merged_config(
    schema: &plugin_core::account_schema::AccountSchema,
    plan_config_json: &str,
    stored_config_json: &str,
) -> String {
    let oauth_pair: [Option<&str>; 2] = match schema.oauth.as_ref() {
        Some(o) => [
            Some(o.client_id_field.as_str()),
            o.client_secret_field.as_deref(),
        ],
        None => [None, None],
    };
    let declared: std::collections::HashSet<&str> = schema
        .fields
        .iter()
        .filter(|f| !f.is_secret())
        .map(|f| f.key.as_str())
        .filter(|key| !oauth_pair.iter().any(|p| p == &Some(*key)))
        .collect();
    let mut config: Map<String, Value> = serde_json::from_str::<Value>(plan_config_json)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    if let Ok(Value::Object(stored)) = serde_json::from_str::<Value>(stored_config_json) {
        for (key, value) in stored {
            if !declared.contains(key.as_str()) {
                config.entry(key).or_insert(value);
            }
        }
    }
    Value::Object(config).to_string()
}

/// Fill every secret field the form left blank with the credential stored for
/// `account_id`, returning the merged values plus the slots that were
/// inherited (so a caller that persists knows not to write them back — the
/// plan trims text, and a credential must survive byte for byte). Also the
/// piece the connection TEST borrows, so "edit the URL, leave the password
/// blank, press Test" probes with the stored password.
pub fn inherit_stored_secrets(
    secrets: &dyn SecretStore,
    account_id: &str,
    schema: &plugin_core::account_schema::AccountSchema,
    values: &Map<String, Value>,
) -> Result<(Map<String, Value>, Vec<SecretSlot>), AccountSetupError> {
    let mut merged = values.clone();
    let mut inherited: Vec<SecretSlot> = Vec::new();
    for field in &schema.fields {
        let Some(slot) = field.secret_slot.map(host_slot) else {
            continue;
        };
        let supplied = merged
            .get(&field.key)
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|s| !s.is_empty());
        if supplied {
            continue;
        }
        match secrets.retrieve(account_id, slot) {
            Ok(held) if !held.is_empty() => {
                inherited.push(slot);
                merged.insert(field.key.clone(), Value::String(held));
            }
            Ok(_) | Err(SecretError::NotFound) => {}
            Err(err) => {
                // "Cannot read" must not become "not set": the keystore being
                // locked would otherwise fail validation (or drop a secret)
                // for a credential the user still has.
                return Err(AccountSetupError::Secret(err.to_string()));
            }
        }
    }
    Ok((merged, inherited))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_core::account_schema::{
        AccountField, AccountFieldKind, AccountOauth, AccountSchema, AccountSecretSlot,
    };

    fn field(key: &str, kind: AccountFieldKind, slot: Option<AccountSecretSlot>) -> AccountField {
        AccountField {
            key: key.to_string(),
            kind,
            label: key.to_string(),
            label_key: None,
            hint: None,
            hint_key: None,
            required: false,
            default: None,
            secret_slot: slot,
            options: Vec::new(),
            min: None,
            max: None,
            device_local: false,
        }
    }

    fn oauth_schema() -> AccountSchema {
        AccountSchema {
            fields: vec![
                field("client_id", AccountFieldKind::Text, None),
                field(
                    "client_secret",
                    AccountFieldKind::Secret,
                    Some(AccountSecretSlot::OauthClientSecret),
                ),
                field("folder_name", AccountFieldKind::Text, None),
            ],
            oauth: Some(AccountOauth {
                builtin_provider: None,
                client_id_field: "client_id".into(),
                client_secret_field: Some("client_secret".into()),
                refresh_token_field: Some("refresh_token".into()),
                access_token_field: None,
                app_redirect_uri: "aperio://oauth-callback".into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn merged_config_keeps_the_stored_oauth_client_id() {
        // The bricking regression: `client_id` is a declared non-secret field,
        // but the plan skips the OAuth pair and both edit UIs drop it — so it
        // must carry over from the stored config, or every edit of a
        // bring-your-own OAuth account deletes it and registration fails.
        let schema = oauth_schema();
        let plan = r#"{"folder_name":"Work"}"#;
        let stored = r#"{"client_id":"my-client","folder_name":"Old","client_fingerprint":"abc"}"#;
        let merged: serde_json::Value =
            serde_json::from_str(&merged_config(&schema, plan, stored)).unwrap();
        assert_eq!(merged["client_id"], "my-client");
        assert_eq!(merged["client_fingerprint"], "abc");
        // The form OWNS folder_name — the plan's value wins.
        assert_eq!(merged["folder_name"], "Work");
    }

    #[test]
    fn merged_config_drops_a_cleared_declared_field_but_keeps_the_rest() {
        let schema = oauth_schema();
        // folder_name cleared by the user → absent from the plan → dropped.
        let plan = r#"{}"#;
        let stored = r#"{"client_id":"my-client","folder_name":"Old"}"#;
        let merged: serde_json::Value =
            serde_json::from_str(&merged_config(&schema, plan, stored)).unwrap();
        assert_eq!(merged["client_id"], "my-client");
        assert!(merged.get("folder_name").is_none());
    }
}
