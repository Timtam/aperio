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

    /// One of a fixed set, chosen from [`AccountField::options`].
    ///
    /// Declared rather than free text because the adapter knows the set and the
    /// host does not: an FTPS transport mode or an SSH authentication method is
    /// a closed list, and a typo in a text box reaches the adapter as a value
    /// it may or may not reject. Several do not — they fall back to a default
    /// and connect differently than the user asked, silently.
    Choice,

    /// A directory on this machine.
    ///
    /// Rendered with the platform's folder picker where there is one, and as a
    /// plain path field where there is not. Marked `device_local` by every
    /// adapter that uses it, for the reason on that flag.
    Directory,

    /// A file on this machine — an SSH private key, a certificate.
    ///
    /// Same rendering rule and the same reason for being device-local as
    /// [`Self::Directory`].
    File,
}

/// One entry of a [`AccountFieldKind::Choice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountFieldOption {
    /// The value stored and handed to the adapter. Never shown.
    pub value: String,

    /// What the user reads. Used verbatim when the app has no translation,
    /// which is the normal case for a third-party plugin.
    pub label: String,

    /// Translation key, taking precedence over `label` — the same arrangement
    /// the field labels use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_key: Option<String>,
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
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    /// The choices, for [`AccountFieldKind::Choice`]. Empty for every other
    /// kind, and rejected on one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AccountFieldOption>,

    /// Whether this value is meaningful only on the machine that entered it.
    ///
    /// An account row travels between a user's devices. Most of what it carries
    /// travels well — a server address, a user name, a client id, the name of a
    /// folder in someone's Drive. Some values do not: the path to an SSH
    /// private key is `/home/anna/.ssh/id_ed25519` on one machine and
    /// `C:\Users\Anna\.ssh\id_ed25519` on another, and a folder on a local
    /// disk means nothing anywhere else.
    ///
    /// Marked fields are kept out of the synced part of the account and stored
    /// per device instead. Everything else about the account still travels, so
    /// adding a calendar capability to a plugin that started as a sync backend
    /// does not change where anything lives — which is exactly what a rule
    /// based on what the account CAN do would have done.
    ///
    /// Only the adapter can answer this. The host cannot tell a filesystem path
    /// from a URL by looking, and guessing wrong in either direction is bad:
    /// too eager and a user retypes settings on every device, too shy and one
    /// machine's paths overwrite another's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub device_local: bool,
}

impl AccountField {
    /// Whether this value goes to the keychain rather than to `config_json`.
    pub fn is_secret(&self) -> bool {
        self.secret_slot.is_some()
    }

    /// Whether this value stays on the device that entered it.
    ///
    /// Secrets are already device-local by a different route — they live in the
    /// platform keychain, not in the account row — so they answer `true` here
    /// too, and a caller splitting an account into "travels" and "stays" gets
    /// the right answer for both without a second rule.
    pub fn stays_on_this_device(&self) -> bool {
        self.device_local || self.is_secret()
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

    /// Buttons the connect form offers besides "add" — a lookup the adapter can
    /// do for the user, so they do not have to find a server URL by hand.
    ///
    /// Declared rather than built in, because the alternative is what the host
    /// did before: a `kind == "ews"` branch rendering one adapter's button, its
    /// five labels living in the app's own translations, and the next adapter
    /// with a discovery protocol needing an edit to the core to get one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<AccountAction>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<AccountOauth>,

    /// Init-config key a per-account, host-owned, writable directory is handed
    /// over under — for an adapter that persists something between runs, like a
    /// sync cookie or an item cache.
    ///
    /// Declared rather than assumed, because the host has no business knowing
    /// that EWS in particular keeps a sync cookie. An adapter that persists
    /// nothing says nothing and is handed nothing; one that wants a directory
    /// names the key it reads it under, and the host creates it.
    ///
    /// Absent when directory creation fails or the host has no data dir (the
    /// test path), so the field must be optional on the plugin's side too — an
    /// adapter that cannot persist falls back to in-memory state rather than
    /// refusing to open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_dir_field: Option<String>,

    /// Whether instances of this plugin get a host-channel capability token, so
    /// they can report a rotated credential back and have the host persist it.
    /// Off unless asked for: the token is authority, and authority nobody
    /// requested is authority nobody audited.
    #[serde(default)]
    pub host_channel: bool,
}

/// Which plugin entry point an action drives.
///
/// An enum rather than a free string: the host looks the symbol up at load time
/// and there is no way to dispatch to one it does not know, so a manifest naming
/// something else is a mistake worth catching at parse time rather than a button
/// that does nothing when pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountActionEntry {
    /// `aperio_plugin_discover` — the adapter works out its own endpoint from
    /// what the user has typed so far.
    Discover,
}

/// One button the connect form offers, and everything the host needs to render
/// it, decide when it is usable, run it, and put the answer back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountAction {
    /// Stable id, for the frontend's key and for logs.
    pub key: String,

    pub entry: AccountActionEntry,

    /// Verbatim label, used when the catalogue cannot answer.
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_key: Option<String>,

    /// Shown while it runs. A button that changes its own name is how a screen
    /// reader learns the thing is working without a separate live region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_label_key: Option<String>,

    /// Announced when it succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_key: Option<String>,

    /// A description the button points at with `aria-describedby`, for saying
    /// what will happen before somebody presses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint_key: Option<String>,

    /// Fields that must be filled first, each with what to say when it is not.
    ///
    /// The message is per FIELD rather than one for the action, because "enter
    /// your address first" and "enter your password first" are different
    /// instructions and a single "fill in the form" helps nobody.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<AccountActionRequirement>,

    /// Argument name → field key. What the host sends the plugin, built from
    /// the form. The adapter names its own arguments; the host only carries
    /// values between two things that both named them.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub inputs: std::collections::BTreeMap<String, String>,

    /// Field key → result key. What the host writes back into the form from
    /// what the plugin answered.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub fills: std::collections::BTreeMap<String, String>,
}

/// A field an action cannot run without.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountActionRequirement {
    pub field: String,
    /// What to say when it is empty. Verbatim, with the usual key beside it.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_key: Option<String>,
}

impl AccountSchema {
    /// Find a declared field by key.
    pub fn field(&self, key: &str) -> Option<&AccountField> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// Find a declared action by key.
    pub fn action(&self, key: &str) -> Option<&AccountAction> {
        self.actions.iter().find(|a| a.key == key)
    }

    /// Every field an action names must be a field the schema declares.
    ///
    /// Checked at parse time because the failure otherwise arrives as a button
    /// that quietly reads nothing and writes nowhere: a typo in `inputs` sends
    /// the plugin an empty argument, and a typo in `fills` drops its answer on
    /// the floor. Neither raises anything a user could act on.
    fn validate_actions(&self) -> PluginResult<()> {
        for action in &self.actions {
            if action.key.trim().is_empty() {
                return Err(PluginError::Manifest(
                    "an account action has an empty key".into(),
                ));
            }
            if self.actions.iter().filter(|a| a.key == action.key).count() > 1 {
                return Err(PluginError::Manifest(format!(
                    "account action `{}` is declared twice",
                    action.key
                )));
            }
            if action.label.trim().is_empty() {
                return Err(PluginError::Manifest(format!(
                    "account action `{}` has an empty label",
                    action.key
                )));
            }
            let mut named: Vec<&str> = action.requires.iter().map(|r| r.field.as_str()).collect();
            named.extend(action.inputs.values().map(String::as_str));
            named.extend(action.fills.keys().map(String::as_str));
            for field in named {
                if self.field(field).is_none() {
                    return Err(PluginError::Manifest(format!(
                        "account action `{}` names field `{field}`, which the schema \
                         does not declare",
                        action.key
                    )));
                }
            }
        }
        Ok(())
    }

    /// Check everything the host will later rely on, at parse time, so a
    /// malformed manifest fails while loading a plugin rather than halfway
    /// through creating an account.
    pub fn validate(&self) -> PluginResult<()> {
        self.validate_actions()?;
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
            // A choice must offer something to choose, and the options must be
            // distinguishable. An empty list renders as a control with nothing
            // in it — the user cannot proceed and nothing says why.
            if field.kind == AccountFieldKind::Choice {
                if field.options.is_empty() {
                    return Err(PluginError::Manifest(format!(
                        "account field `{}` is a choice with no options",
                        field.key
                    )));
                }
                for option in &field.options {
                    if option.value.trim().is_empty() {
                        return Err(PluginError::Manifest(format!(
                            "account field `{}` has an option with an empty value",
                            field.key
                        )));
                    }
                    if option.label.trim().is_empty() && option.label_key.is_none() {
                        return Err(PluginError::Manifest(format!(
                            "account field `{}`: option `{}` has nothing to display",
                            field.key, option.value
                        )));
                    }
                    if field
                        .options
                        .iter()
                        .filter(|o| o.value == option.value)
                        .count()
                        > 1
                    {
                        return Err(PluginError::Manifest(format!(
                            "account field `{}` offers `{}` twice",
                            field.key, option.value
                        )));
                    }
                }
                // A default outside the list would leave the control showing
                // nothing selected while the value is non-empty.
                if let Some(AccountFieldDefault::Text(value)) = &field.default {
                    if !field.options.iter().any(|o| &o.value == value) {
                        return Err(PluginError::Manifest(format!(
                            "account field `{}` defaults to `{value}`, which it does                              not offer",
                            field.key
                        )));
                    }
                }
            } else if !field.options.is_empty() {
                // Options on anything else are a mistake that would be ignored
                // rather than reported — the renderers only read them for a
                // choice.
                return Err(PluginError::Manifest(format!(
                    "account field `{}` declares options but is not a choice",
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
    fn choice(key: &str, options: Vec<AccountFieldOption>) -> AccountField {
        AccountField {
            key: key.to_string(),
            kind: AccountFieldKind::Choice,
            label: key.to_string(),
            options,
            ..Default::default()
        }
    }

    fn option(value: &str) -> AccountFieldOption {
        AccountFieldOption {
            value: value.to_string(),
            label: value.to_string(),
            label_key: None,
        }
    }

    fn schema_of(fields: Vec<AccountField>) -> AccountSchema {
        AccountSchema {
            fields,
            ..Default::default()
        }
    }

    #[test]
    fn a_choice_must_offer_something() {
        let err = schema_of(vec![choice("mode", vec![])])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("no options"), "{err}");
    }

    #[test]
    fn a_choice_cannot_offer_the_same_value_twice() {
        let err = schema_of(vec![choice(
            "mode",
            vec![option("explicit"), option("explicit")],
        )])
        .validate()
        .unwrap_err();
        assert!(err.to_string().contains("twice"), "{err}");
    }

    /// A default outside the list leaves the control showing nothing selected
    /// while the stored value is non-empty — the user sees no answer and the
    /// adapter gets one.
    #[test]
    fn a_choice_cannot_default_to_something_it_does_not_offer() {
        let mut field = choice("mode", vec![option("explicit"), option("implicit")]);
        field.default = Some(AccountFieldDefault::Text("plain".to_string()));
        let err = schema_of(vec![field]).validate().unwrap_err();
        assert!(err.to_string().contains("which it does"), "{err}");
    }

    #[test]
    fn a_valid_choice_passes() {
        let mut field = choice("mode", vec![option("explicit"), option("implicit")]);
        field.default = Some(AccountFieldDefault::Text("explicit".to_string()));
        schema_of(vec![field]).validate().unwrap();
    }

    /// Options on a non-choice would simply never be rendered. Reported rather
    /// than ignored, because the author clearly meant something by them.
    #[test]
    fn options_on_anything_but_a_choice_are_refused() {
        let mut field = choice("host", vec![option("a")]);
        field.kind = AccountFieldKind::Text;
        let err = schema_of(vec![field]).validate().unwrap_err();
        assert!(err.to_string().contains("not a choice"), "{err}");
    }

    /// Secrets never reach the account row at all, so a caller splitting an
    /// account into "travels" and "stays" must see them on the staying side
    /// without needing a second rule for them.
    #[test]
    fn a_secret_stays_on_the_device_without_being_marked() {
        let field = AccountField {
            key: "password".into(),
            kind: AccountFieldKind::Secret,
            label: "Password".into(),
            secret_slot: Some(AccountSecretSlot::Password),
            ..Default::default()
        };
        assert!(!field.device_local);
        assert!(field.stays_on_this_device());
    }

    #[test]
    fn a_marked_path_stays_and_an_unmarked_url_travels() {
        let key_path = AccountField {
            key: "key_path".into(),
            kind: AccountFieldKind::File,
            label: "Key file".into(),
            device_local: true,
            ..Default::default()
        };
        let url = AccountField {
            key: "url".into(),
            kind: AccountFieldKind::Url,
            label: "URL".into(),
            ..Default::default()
        };
        assert!(key_path.stays_on_this_device());
        assert!(!url.stays_on_this_device());
    }

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
            options: Vec::new(),
            device_local: false,
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
    fn an_action_may_only_name_fields_the_schema_declares() {
        // The failure this catches never announces itself: a typo in `inputs`
        // hands the plugin an empty argument, a typo in `fills` drops its answer
        // on the floor, and the button looks like it worked.
        let base = |action: AccountAction| AccountSchema {
            fields: vec![
                field("endpoint", AccountFieldKind::Url, None),
                field("username", AccountFieldKind::Text, None),
            ],
            actions: vec![action],
            ..Default::default()
        };
        let sound = AccountAction {
            key: "discover".into(),
            entry: AccountActionEntry::Discover,
            label: "Discover URL".into(),
            label_key: None,
            busy_label: None,
            busy_label_key: None,
            success: None,
            success_key: None,
            hint: None,
            hint_key: None,
            requires: vec![AccountActionRequirement {
                field: "username".into(),
                message: "Enter your address first.".into(),
                message_key: None,
            }],
            inputs: [("email".to_string(), "username".to_string())]
                .into_iter()
                .collect(),
            fills: [("endpoint".to_string(), "ews_url".to_string())]
                .into_iter()
                .collect(),
        };
        base(sound.clone())
            .validate()
            .expect("a well-formed action");

        // A requirement naming a field nobody declared.
        let mut bad = sound.clone();
        bad.requires[0].field = "nope".into();
        assert!(base(bad).validate().is_err());

        // An input reading a field nobody declared.
        let mut bad = sound.clone();
        bad.inputs.insert("email".into(), "nope".into());
        assert!(base(bad).validate().is_err());

        // A fill writing into a field nobody declared. Note the direction: the
        // KEY is ours, the value is the plugin's own result name and is not
        // ours to check.
        let mut bad = sound.clone();
        bad.fills.clear();
        bad.fills.insert("nope".into(), "ews_url".into());
        assert!(base(bad).validate().is_err());

        // Two actions under one key would make "run the one called discover"
        // ambiguous.
        let mut twice = base(sound.clone());
        twice.actions.push(sound);
        assert!(twice.validate().is_err());
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
            ..Default::default()
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
