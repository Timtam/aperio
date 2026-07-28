//! What a plugin needs in order to have an account — declared by the plugin,
//! executed by the host.
//!
//! The host used to know, in its own source, that CalDAV wants a server URL and
//! a password while Google wants a client id and runs an OAuth dance. Every new
//! adapter meant another branch in the connect command, another branch in the
//! registry, and another block in two frontends. An adapter that Aperio's
//! authors had never seen could not be connected at all.
//!
//! An [`AccountSchema`] inverts that. The plugin declares its fields, says
//! which of them are secrets and which keychain slot each belongs in, and — if
//! it needs one — describes its OAuth flow. The host renders the form, collects
//! the values, runs the flow, splits secrets from non-secrets, and merges
//! everything back into the plugin's init config at open time, without knowing
//! what any of it means.
//!
//! It lives in `plugin.json` rather than behind a new ABI export, alongside the
//! `recurrence` and `tasks` capability blocks that work the same way. It is
//! static data; it needs no code to produce; it can be read before the library
//! is ever loaded; and a third-party author writes JSON rather than Rust.
//!
//! ## The one invariant worth stating twice
//!
//! A field is a secret **only** together with the slot it goes to
//! ([`AccountField::secret_slot`]), and a secret NEVER reaches `config_json`.
//! That column is documented as non-secret and the sync engine appends it to
//! the event log unencrypted whenever end-to-end encryption is off, so a secret
//! there would travel to the user's own sync target in the clear. The two are
//! validated to imply each other at parse time, so a manifest cannot express
//! "secret, stored in the config" even by accident.

use serde::{Deserialize, Serialize};

use crate::error::{PluginError, PluginResult};

/// A keychain slot an account schema is allowed to name.
///
/// A deliberately smaller set than the host's own `SecretSlot`. The
/// cross-device end-to-end encryption key is a slot, but it is **not a variant
/// here** — so a manifest cannot ask for it, cannot be rejected for asking for
/// it, and cannot be one parser bug away from getting it. The host maps this
/// enum onto its own; that mapping is total, which is the point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountSecretSlot {
    /// Short-lived OAuth access token.
    AccessToken,
    /// Long-lived OAuth refresh token.
    RefreshToken,
    /// Basic-auth password.
    Password,
    /// API token (Vikunja, Todoist, …).
    ApiToken,
    /// An OAuth *client* secret, as opposed to a user credential.
    OauthClientSecret,
}

impl AccountSecretSlot {
    /// The host's wire name for this slot — the same string the keychain
    /// service suffix and the `credential.set` events use.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::AccessToken => "access_token",
            Self::RefreshToken => "refresh_token",
            Self::Password => "password",
            Self::ApiToken => "api_token",
            Self::OauthClientSecret => "oauth_client_secret",
        }
    }
}

/// How a field is presented and validated. Drives the input type on both
/// frontends — including the on-screen keyboard on mobile, which is why `Url`
/// is worth distinguishing from `Text`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountFieldKind {
    #[default]
    Text,
    Url,
    /// Masked input. Implies — and requires — a [`AccountField::secret_slot`].
    Secret,
    /// A checkbox. Its value is a JSON bool, and it is never a secret.
    Bool,
}

/// A field's starting value.
///
/// Deliberately not arbitrary JSON: a default is a checkbox state or a piece of
/// prefilled text, and nothing else. Narrowing it here means a manifest cannot
/// smuggle a structure into a place the frontends would have to guess how to
/// render — and it keeps the whole manifest comparable by value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AccountFieldDefault {
    Bool(bool),
    Text(String),
}

/// One thing the user is asked for, or one setting they can turn on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountField {
    /// Identifier, and the key this value appears under in the plugin's init
    /// config. A non-secret field is persisted in `config_json` under the same
    /// key, so the round trip needs no mapping table.
    pub key: String,

    #[serde(default)]
    pub kind: AccountFieldKind,

    /// Human-readable label. Used verbatim when the app has no translation —
    /// which is the normal case for a third-party plugin.
    pub label: String,

    /// Translation key the app resolves in the user's language, taking
    /// precedence over `label`. Bundled adapters set this; the strings then
    /// live in the app's locale files, where translations belong, while the
    /// structure stays here, where the adapter's knowledge belongs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_key: Option<String>,

    /// Optional explanatory line under the field. Same two-way arrangement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint_key: Option<String>,

    /// Whether the connect form refuses to submit without it. An OAuth client
    /// id and secret are the exception the host applies on top: see
    /// [`AccountOauth::builtin_provider`].
    #[serde(default)]
    pub required: bool,

    /// Starting value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<AccountFieldDefault>,

    /// Present exactly when this field is a secret. `None` means the value is
    /// non-secret and belongs in `config_json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_slot: Option<AccountSecretSlot>,
}

impl AccountField {
    /// Whether this value goes to the keychain rather than to `config_json`.
    pub fn is_secret(&self) -> bool {
        self.secret_slot.is_some()
    }
}

/// An OAuth sign-in the host runs on the plugin's behalf, via the plugin's own
/// `aperio_plugin_interactive_auth` export.
///
/// The host drives the *flow* — browser or native auth session, the two mobile
/// phases, storing what comes back — and knows nothing about the provider. The
/// plugin owns the endpoints, the scopes and the exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountOauth {
    /// Name of the credential set this build may carry for the provider, as
    /// `builtin-oauth` knows it (`"webex"`, `"google"`, …).
    ///
    /// When present AND this build carries that credential, the client id and
    /// secret fields become optional: leaving both blank connects with Aperio's
    /// own registration, and the account records which one it was linked to
    /// rather than the credential itself. Absent, or a build that carries
    /// nothing, means the user supplies their own — a first-class mode, not a
    /// degraded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin_provider: Option<String>,

    /// Which declared field holds the OAuth client id.
    pub client_id_field: String,

    /// Which declared field holds the client secret, for providers that require
    /// one. Absent for a public PKCE client, which has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_field: Option<String>,

    /// Init-config key the refresh token is merged under at open time. Absent
    /// means the plugin does not want one kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_field: Option<String>,

    /// Init-config key the access token is merged under. Usually absent: an
    /// adapter that re-mints on first use would rather not be handed a token
    /// the host has to keep fresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token_field: Option<String>,

    /// Redirect URI the host hands the plugin where it cannot run a loopback
    /// listener — that is, on mobile, where the app-scheme callback is the only
    /// thing a native auth session can return to. On the desktop the plugin
    /// binds its own listener and this is unused.
    #[serde(default = "default_app_redirect_uri")]
    pub app_redirect_uri: String,
}

fn default_app_redirect_uri() -> String {
    "aperio://oauth-callback".to_string()
}

/// Everything a plugin needs said about its accounts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSchema {
    /// In the order the connect form presents them.
    #[serde(default)]
    pub fields: Vec<AccountField>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<AccountOauth>,

    /// Whether instances of this plugin get a host-channel capability token, so
    /// they can report a rotated credential back and have the host persist it.
    /// Off unless asked for: the token is authority, and authority nobody
    /// requested is authority nobody audited.
    #[serde(default)]
    pub host_channel: bool,
}

impl AccountSchema {
    /// Find a declared field by key.
    pub fn field(&self, key: &str) -> Option<&AccountField> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// Check everything the host will later rely on, at parse time, so a
    /// malformed manifest fails while loading a plugin rather than halfway
    /// through creating an account.
    pub fn validate(&self) -> PluginResult<()> {
        for (index, field) in self.fields.iter().enumerate() {
            if field.key.trim().is_empty() {
                return Err(PluginError::Manifest(format!(
                    "account.fields[{index}] has an empty key"
                )));
            }
            if field.label.trim().is_empty() {
                return Err(PluginError::Manifest(format!(
                    "account field `{}` has an empty label",
                    field.key
                )));
            }
            if self.fields.iter().filter(|f| f.key == field.key).count() > 1 {
                return Err(PluginError::Manifest(format!(
                    "account field `{}` is declared twice",
                    field.key
                )));
            }
            // The invariant from the module docs, enforced in both directions.
            // A `secret` without a slot would fall through to `config_json`,
            // which is the one place a secret may never go; a slot on a
            // non-secret field would send a value the user typed in the clear
            // to the keychain and leave the form rendering it unmasked.
            match (field.kind, field.secret_slot) {
                (AccountFieldKind::Secret, None) => {
                    return Err(PluginError::Manifest(format!(
                        "account field `{}` is a secret but names no secret_slot — a secret \
                         without a slot would be persisted in config_json, which syncs in the \
                         clear",
                        field.key
                    )))
                }
                (kind, Some(_)) if kind != AccountFieldKind::Secret => {
                    return Err(PluginError::Manifest(format!(
                        "account field `{}` names a secret_slot but is not of kind `secret`",
                        field.key
                    )))
                }
                _ => {}
            }
            match (field.kind, &field.default) {
                (AccountFieldKind::Bool, Some(AccountFieldDefault::Text(_))) => {
                    return Err(PluginError::Manifest(format!(
                        "account field `{}` is a checkbox with a text default",
                        field.key
                    )))
                }
                (kind, Some(AccountFieldDefault::Bool(_))) if kind != AccountFieldKind::Bool => {
                    return Err(PluginError::Manifest(format!(
                        "account field `{}` is not a checkbox but has a boolean default",
                        field.key
                    )))
                }
                _ => {}
            }
        }

        if let Some(oauth) = &self.oauth {
            let id = self.field(&oauth.client_id_field).ok_or_else(|| {
                PluginError::Manifest(format!(
                    "account.oauth.client_id_field names `{}`, which is not a declared field",
                    oauth.client_id_field
                ))
            })?;
            if id.is_secret() {
                return Err(PluginError::Manifest(format!(
                    "account.oauth.client_id_field `{}` is marked secret — a client id is not \
                     one, and hiding it in the keychain would stop the host from recording which \
                     registration an account belongs to",
                    oauth.client_id_field
                )));
            }
            if let Some(key) = &oauth.client_secret_field {
                let secret = self.field(key).ok_or_else(|| {
                    PluginError::Manifest(format!(
                        "account.oauth.client_secret_field names `{key}`, which is not a declared \
                         field"
                    ))
                })?;
                if secret.secret_slot != Some(AccountSecretSlot::OauthClientSecret) {
                    return Err(PluginError::Manifest(format!(
                        "account.oauth.client_secret_field `{key}` must be a secret in the \
                         `oauth_client_secret` slot"
                    )));
                }
            }
            if oauth
                .builtin_provider
                .as_ref()
                .is_some_and(|p| p.trim().is_empty())
            {
                return Err(PluginError::Manifest(
                    "account.oauth.builtin_provider must not be empty when present".into(),
                ));
            }
            if oauth.app_redirect_uri.trim().is_empty() {
                return Err(PluginError::Manifest(
                    "account.oauth.app_redirect_uri must not be empty".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn a_secret_field_without_a_slot_is_refused() {
        let schema = AccountSchema {
            fields: vec![field("password", AccountFieldKind::Secret, None)],
            ..Default::default()
        };
        let err = schema.validate().expect_err("must not validate");
        assert!(
            err.to_string().contains("config_json"),
            "the message has to say WHY: {err}"
        );
    }

    #[test]
    fn a_non_secret_field_with_a_slot_is_refused() {
        let schema = AccountSchema {
            fields: vec![field(
                "server_url",
                AccountFieldKind::Text,
                Some(AccountSecretSlot::Password),
            )],
            ..Default::default()
        };
        assert!(schema.validate().is_err());
    }

    #[test]
    fn the_e2e_key_slot_cannot_be_named_at_all() {
        // Not "is rejected" — unnameable. The enum has no variant for it, so
        // this is a parse failure, and no host code has to remember to check.
        let json = r#"{"key":"k","label":"K","kind":"secret","secret_slot":"sync_encryption_key"}"#;
        assert!(serde_json::from_str::<AccountField>(json).is_err());
    }

    #[test]
    fn duplicate_keys_are_refused() {
        let schema = AccountSchema {
            fields: vec![
                field("token", AccountFieldKind::Text, None),
                field("token", AccountFieldKind::Text, None),
            ],
            ..Default::default()
        };
        assert!(schema.validate().is_err());
    }

    #[test]
    fn oauth_must_point_at_fields_that_exist_and_have_the_right_shape() {
        let mut schema = AccountSchema {
            fields: vec![
                field("client_id", AccountFieldKind::Text, None),
                field(
                    "client_secret",
                    AccountFieldKind::Secret,
                    Some(AccountSecretSlot::OauthClientSecret),
                ),
            ],
            oauth: Some(AccountOauth {
                builtin_provider: Some("webex".into()),
                client_id_field: "client_id".into(),
                client_secret_field: Some("client_secret".into()),
                refresh_token_field: Some("refresh_token".into()),
                access_token_field: None,
                app_redirect_uri: default_app_redirect_uri(),
            }),
            host_channel: true,
        };
        schema.validate().expect("a well-formed schema");

        // A client id kept in the keychain would hide which registration an
        // account belongs to, which is the one thing the fingerprint exists for.
        schema.fields[0].kind = AccountFieldKind::Secret;
        schema.fields[0].secret_slot = Some(AccountSecretSlot::OauthClientSecret);
        assert!(schema.validate().is_err());
        schema.fields[0].kind = AccountFieldKind::Text;
        schema.fields[0].secret_slot = None;

        // A client secret in the wrong slot would be picked up by nothing.
        schema.fields[1].secret_slot = Some(AccountSecretSlot::Password);
        assert!(schema.validate().is_err());
        schema.fields[1].secret_slot = Some(AccountSecretSlot::OauthClientSecret);

        schema.oauth.as_mut().unwrap().client_id_field = "nope".into();
        assert!(schema.validate().is_err());
    }

    #[test]
    fn a_schema_round_trips_through_json() {
        let json = r#"{
            "fields": [
                {"key":"client_id","kind":"text","label":"Client ID","required":true},
                {"key":"client_secret","kind":"secret","label":"Secret",
                 "secret_slot":"oauth_client_secret","required":true},
                {"key":"use_personal_room","kind":"bool","label":"Personal room","default":false}
            ],
            "oauth": {
                "builtin_provider": "webex",
                "client_id_field": "client_id",
                "client_secret_field": "client_secret",
                "refresh_token_field": "refresh_token"
            },
            "host_channel": true
        }"#;
        let schema: AccountSchema = serde_json::from_str(json).expect("parses");
        schema.validate().expect("validates");
        assert_eq!(schema.fields.len(), 3);
        assert!(schema.field("client_secret").unwrap().is_secret());
        assert!(!schema.field("use_personal_room").unwrap().is_secret());
        let oauth = schema.oauth.as_ref().unwrap();
        assert_eq!(oauth.app_redirect_uri, "aperio://oauth-callback");
        assert_eq!(oauth.access_token_field, None);

        let again: AccountSchema =
            serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
        assert_eq!(again.fields.len(), schema.fields.len());
    }

    #[test]
    fn an_absent_block_is_a_plugin_with_no_accounts_rather_than_an_error() {
        let schema = AccountSchema::default();
        schema.validate().expect("nothing declared is valid");
        assert!(schema.fields.is_empty());
        assert!(schema.oauth.is_none());
        assert!(!schema.host_channel);
    }
}
