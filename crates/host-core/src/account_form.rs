//! The connect form a plugin declares, as the frontends actually receive it.
//!
//! One builder, for the same reason [`crate::wire`] holds one container row:
//! this was written twice — once in `src-tauri/src/commands/accounts.rs` for
//! the desktop's Tauri command, once as a hand-built `serde_json::json!` in
//! `crates/cal-ffi/src/host.rs` for the mobile bridge — and the two had drifted
//! in BOTH directions.
//!
//! What that cost: the mobile builder emitted no `options`, no `default_bool`,
//! no `default_text` and no `device_local`, while
//! `mobile/src/components/AccountSchemaForm.tsx` renders a `choice` field with
//! `field.options.map(…)`. The FTP and SFTP plugins both declare choice fields,
//! so adding either account on a phone dereferenced `undefined` and took the
//! form down. TypeScript did not catch it because the mobile side parses the
//! bridge's answer with a bare `as AccountFormSpec` cast — the type said the
//! field was there and nothing checked. In the other direction the mobile
//! builder emitted an `app_redirect_uri` on the OAuth block that no frontend
//! reads and the desktop never sent.
//!
//! The types are generated to TypeScript (`ts-export`), so the cast now
//! describes something a machine derived from this file rather than something
//! a person typed twice.
//!
//! # Everything here is already in the reader's language
//!
//! Labels are resolved against the PLUGIN's own catalogue before they leave —
//! the app carries no word about somebody else's provider, and a third-party
//! adapter with no catalogue arrives as the literal its author wrote, which
//! beats a missing-key marker. That is why this returns finished labels and not
//! i18n keys: the keys are the plugin's, not Aperio's, and the app has no table
//! to look them up in (DESIGN §4.5 covers the core's own rule; this is the
//! documented exception, and it is the host speaking, not `cal-core`).

use std::sync::Arc;

use plugin_core::account_schema::{AccountFieldDefault, AccountFieldKind};
use plugin_core::manager::{LoadedPlugin, PluginManager};
use plugin_core::strings::StringCatalogue;
use plugin_core::PluginManifest;
use serde::Serialize;

use crate::account_setup::{has_builtin_client, supports_credential_test};

/// The account form for a plugin, as its manifest declares it.
///
/// A wire copy rather than the manifest type, so the frontend sees exactly what
/// it needs and nothing else — and so the built-in-credentials question is
/// answered here, where the credentials live, rather than by asking a frontend
/// to reason about a posture it cannot check.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AccountFormSpec {
    pub plugin_id: String,
    pub fields: Vec<AccountFormField>,
    /// Buttons besides "add" that this adapter offers on its form.
    #[serde(default)]
    pub actions: Vec<AccountFormAction>,
    /// Present when connecting runs an OAuth sign-in.
    pub oauth: Option<AccountFormOauth>,
    /// Whether accounts of this adapter own calendars and task lists. Derived
    /// from the plugin's declared TYPE, so a frontend can skip the catalog
    /// refresh after connecting a videoconference account — which owns neither,
    /// and whose catalog calls have a blocking cold path — without keeping its
    /// own list of which adapters those are.
    pub owns_containers: bool,
    /// Whether "test connection" can mean anything before the account exists.
    /// Answered here rather than re-derived in each frontend, so the button and
    /// the probe cannot disagree — see
    /// [`crate::account_setup::supports_credential_test`].
    pub supports_credential_test: bool,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AccountFormOption {
    pub value: String,
    /// Already in the caller's language, like every other label here.
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AccountFormField {
    pub key: String,
    /// How to present and validate the field. Carried as the manifest's own
    /// enum rather than as a string, so the union the two forms switch on is
    /// generated from one list instead of retyped beside it.
    pub kind: AccountFieldKind,
    /// Already in the caller's language. The adapter names the field and its
    /// own catalogue supplies the words; the frontend renders what it is given.
    pub label: String,
    pub hint: Option<String>,
    pub required: bool,
    pub default_bool: Option<bool>,
    pub default_text: Option<String>,
    /// The choices, for `kind == "choice"`. Empty otherwise — and EMPTY, never
    /// absent: the mobile form maps over it without a guard, and an absent key
    /// is what took that form down.
    pub options: Vec<AccountFormOption>,
    /// Whether this value belongs only to the device that entered it — a
    /// filesystem path, typically. Only the adapter can answer it: a host
    /// cannot tell a path from a URL by looking.
    pub device_local: bool,
}

/// One button the connect form should offer, everything already in the reader's
/// language.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AccountFormAction {
    pub key: String,
    pub label: String,
    pub busy_label: Option<String>,
    pub success: Option<String>,
    pub hint: Option<String>,
    pub requires: Vec<AccountFormRequirement>,
}

/// A field an action needs filled before it can run, and what to say when it is
/// not.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AccountFormRequirement {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AccountFormOauth {
    /// True when this build carries credentials for the provider, so the two
    /// client fields may be left blank and the form need not show them at all.
    pub builtin: bool,
    pub client_id_field: String,
    pub client_secret_field: Option<String>,
}

/// The form for one adapter kind, or `None` when the plugin declares none.
///
/// `lang` is the language to render the labels in — the UI's, since the person
/// reading them is the one at the keyboard. `None` means English.
pub fn account_form_spec(
    manager: &PluginManager,
    adapter_kind: &str,
    lang: Option<&str>,
) -> Option<AccountFormSpec> {
    // No "unknown kind" branch: which kinds exist is a fact about which plugins
    // are loaded, so an unrecognised one is simply a plugin that declares no
    // form — the same answer as an adapter still on the older per-kind path.
    let plugin = manager.plugin_for_adapter_kind(adapter_kind)?;
    spec_for_plugin(&plugin, lang)
}

/// The same answer for a plugin already in hand.
pub fn spec_for_plugin(plugin: &Arc<LoadedPlugin>, lang: Option<&str>) -> Option<AccountFormSpec> {
    let lang = lang.unwrap_or(plugin_core::FALLBACK_LANG);
    // Resolving the catalogue is the one step that needs a LOADED plugin: a
    // plugin may export its strings across the FFI boundary rather than declare
    // them in its manifest. Everything after it is a pure function of the
    // manifest, which is why the split exists — `spec_from_manifest` is
    // testable from a `plugin.json` alone.
    let strings = PluginManager::strings_for(plugin, lang);
    spec_from_manifest(&plugin.manifest, &strings, lang)
}

/// The form a manifest declares, given the catalogue to resolve labels against.
///
/// Pure: no plugin has to be loaded, no library opened. That matters because
/// the failure this module exists to prevent was a `choice` field arriving
/// without its `options`, and the only way to pin that is to run a real
/// manifest through the real builder.
pub fn spec_from_manifest(
    manifest: &PluginManifest,
    strings: &StringCatalogue,
    lang: &str,
) -> Option<AccountFormSpec> {
    let schema = manifest.account.clone()?;
    let label_of = |key: Option<&str>, verbatim: &str| {
        plugin_core::resolve_label(Some(strings), key, verbatim, lang).to_string()
    };
    // A label that resolves to nothing is ABSENT, not empty: the frontends
    // render a hint only when there is one, and an empty string would put a
    // blank line under the field for a screen reader to walk through.
    let optional_label = |verbatim: Option<&str>, key: Option<&str>| {
        verbatim
            .or(key.map(|_| ""))
            .map(|v| label_of(key, v))
            .filter(|s| !s.is_empty())
    };

    Some(AccountFormSpec {
        plugin_id: manifest.id.clone(),
        fields: schema
            .fields
            .iter()
            .map(|f| AccountFormField {
                key: f.key.clone(),
                kind: f.kind,
                label: label_of(f.label_key.as_deref(), &f.label),
                hint: optional_label(f.hint.as_deref(), f.hint_key.as_deref()),
                required: f.required,
                default_bool: match &f.default {
                    Some(AccountFieldDefault::Bool(b)) => Some(*b),
                    _ => None,
                },
                default_text: match &f.default {
                    Some(AccountFieldDefault::Text(t)) => Some(t.clone()),
                    _ => None,
                },
                options: f
                    .options
                    .iter()
                    .map(|o| AccountFormOption {
                        value: o.value.clone(),
                        label: label_of(o.label_key.as_deref(), &o.label),
                    })
                    .collect(),
                device_local: f.device_local,
            })
            .collect(),
        actions: schema
            .actions
            .iter()
            .map(|a| AccountFormAction {
                key: a.key.clone(),
                label: label_of(a.label_key.as_deref(), &a.label),
                busy_label: optional_label(a.busy_label.as_deref(), a.busy_label_key.as_deref()),
                success: optional_label(a.success.as_deref(), a.success_key.as_deref()),
                hint: optional_label(a.hint.as_deref(), a.hint_key.as_deref()),
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
            builtin: has_builtin_client(o),
            client_id_field: o.client_id_field.clone(),
            client_secret_field: o.client_secret_field.clone(),
        }),
        owns_containers: manifest.has_data_family(),
        supports_credential_test: supports_credential_test(&schema),
    })
}
