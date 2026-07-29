//! iCal-feed calendar adapter packaged as a plugin
//! (DESIGN.md §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "feed_url": "https://example.com/calendar.ics",
//!   "username": null,
//!   "password": null
//! }
//! ```
//!
//! `username` + `password` are both optional — most public
//! feeds don't need them. When present, the adapter sends
//! HTTP Basic Auth.
//!
//! ## Vtable shape
//!
//! Single-capability calendar adapter — fills only the
//! `calendar` slot of [`AdapterVtable`]; `tasks` +
//! `contacts` stay null. iCal feeds are read-only at the
//! protocol level, so the write-side methods (`create_event`,
//! `update_event`, `delete_event`, `add_event_exdate`,
//! `rename_calendar`) are left at `None` and the host's shim
//! surfaces them as `cal_core::Error::Unsupported`.

use std::os::raw::{c_char, c_void};

use cal_adapter_ical::{Credentials as IcalCredentials, IcalAccountConfig, IcalAdapter};
use cal_core::adapter::{Capability, Credentials as CalCredentials};
use cal_core::types::DateRange;
use cal_core::CalendarFeature;
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{AdapterVtable, CalendarVtable};
use plugin_sdk::{decode_args, ok_response, open_instance_with, PluginInstance};
use serde::Deserialize;

plugin_sdk::cal_dispatch_helpers!(IcalAdapter);

#[derive(Debug, Deserialize)]
struct InitConfig {
    feed_url: String,
    #[serde(default)]
    username: Option<String>,
    /// Host pre-extracts the password from keychain + threads
    /// it in via `config_json`.
    #[serde(default)]
    password: Option<String>,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.feed_url.trim().is_empty() {
            return Err("feed_url must not be empty".to_string());
        }
        let credentials = IcalCredentials::new(
            IcalAccountConfig {
                feed_url: cfg.feed_url,
                username: cfg.username,
            },
            cfg.password,
        );
        IcalAdapter::new(credentials).map_err(|e| format!("adapter ctor failed: {e:?}"))
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<IcalAdapter>::drop_handle(handle);
}

// ─────────────────────────────────────────────────────────────
// Adapter base trait
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_authenticate(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let creds: CalCredentials = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        cal_core::Adapter::authenticate(p, creds).await
    })
}

unsafe extern "C" fn ffi_capabilities(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    let inst = match instance(h) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let caps: Vec<Capability> = cal_core::Adapter::capabilities(inst.plugin()).to_vec();
    ok_response(&caps)
}

// ─────────────────────────────────────────────────────────────
// CalendarFeature trait
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_list_calendars(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_calendars().await })
}

#[derive(Debug, Deserialize)]
struct GetEventsArgs {
    calendar_id: String,
    range: DateRange,
}

unsafe extern "C" fn ffi_get_events(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: GetEventsArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.get_events(&args.calendar_id, args.range).await
    })
}

#[derive(Debug, Deserialize)]
struct GetFreeBusyArgs {
    emails: Vec<String>,
    range: DateRange,
}

unsafe extern "C" fn ffi_get_free_busy(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: GetFreeBusyArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        let refs: Vec<&str> = args.emails.iter().map(|s| s.as_str()).collect();
        p.get_free_busy(&refs, args.range).await
    })
}

unsafe extern "C" fn ffi_calendar_color(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let calendar_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let inst = match instance(h) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let color = inst.plugin().calendar_color(&calendar_id);
    ok_response(&color)
}

// ─────────────────────────────────────────────────────────────
// Vtables
// ─────────────────────────────────────────────────────────────

pub static CALENDAR_VTABLE: CalendarVtable = CalendarVtable {
    authenticate: Some(ffi_authenticate),
    capabilities: Some(ffi_capabilities),
    list_calendars: Some(ffi_list_calendars),
    get_events: Some(ffi_get_events),
    create_event: None,
    update_event: None,
    delete_event: None,
    get_free_busy: Some(ffi_get_free_busy),
    calendar_color: Some(ffi_calendar_color),
    add_event_exdate: None,
    rename_calendar: None,
    ..CalendarVtable::empty()
};

pub static ADAPTER_VTABLE: AdapterVtable = AdapterVtable {
    calendar: &CALENDAR_VTABLE,
    ..AdapterVtable::empty()
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-ical",
    name: "Aperio iCal Feed",
    version: "0.1.0",
    plugin_type: "adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}

#[cfg(test)]
mod tests {
    use plugin_sdk::plugin_core::account_schema::{AccountFieldKind, AccountSecretSlot};

    /// The manifest ships beside this crate and is the ONLY thing that tells
    /// the host how to set up an iCal-feed account. Parsing it here means a
    /// typo fails the build rather than the first user who tries to connect.
    fn manifest() -> plugin_sdk::plugin_core::manifest::PluginManifest {
        plugin_sdk::plugin_core::manifest::PluginManifest::from_bytes(include_bytes!(
            "../plugin.json"
        ))
        .expect("plugin.json parses and its account schema validates")
    }

    #[test]
    fn every_schema_field_is_a_key_the_init_config_actually_reads() {
        // The schema and `InitConfig` are two descriptions of the same thing,
        // in two languages, and nothing but this test connects them. A field
        // the host faithfully collects and merges under a name the plugin does
        // not deserialise is silently dropped — the account connects, and then
        // behaves as though the setting were never set.
        let schema = manifest()
            .account
            .expect("the iCal feed adapter declares an account schema");
        let known = ["feed_url", "username", "password"];
        for field in &schema.fields {
            assert!(
                known.contains(&field.key.as_str()),
                "schema field `{}` is not read by InitConfig",
                field.key
            );
        }
        // No OAuth: a feed URL is fetched with Basic auth or with nothing.
        assert!(schema.oauth.is_none());
    }

    #[test]
    fn only_the_password_leaves_the_account_row() {
        let schema = manifest().account.unwrap();
        assert_eq!(
            schema.field("password").unwrap().secret_slot,
            Some(AccountSecretSlot::Password)
        );
        // The feed URL and the user name are what the account row IS — the
        // host shows the URL back to the user, and both are handed straight to
        // `InitConfig` from `config_json`.
        assert!(!schema.field("feed_url").unwrap().is_secret());
        assert!(!schema.field("username").unwrap().is_secret());
    }

    #[test]
    fn both_credentials_are_optional_because_most_feeds_are_public() {
        // The one thing this adapter's form must get right: a public .ics URL
        // needs no credentials at all, so a form that refused to submit without
        // them would lock out the common case.
        let schema = manifest().account.unwrap();
        assert!(schema.field("feed_url").unwrap().required);
        assert!(!schema.field("username").unwrap().required);
        assert!(!schema.field("password").unwrap().required);
        // `url` rather than `text` so mobile offers the URL keyboard.
        assert_eq!(
            schema.field("feed_url").unwrap().kind,
            AccountFieldKind::Url
        );
    }

    #[test]
    fn every_declared_label_and_hint_resolves_in_both_languages() {
        // A `label_key` that no catalogue answers degrades to the verbatim
        // English label — silently, and only for the reader whose language is
        // missing. Checking both declared languages here is what keeps that
        // from shipping.
        let manifest = manifest();
        let schema = manifest.account.as_ref().unwrap();
        assert_eq!(
            manifest.strings.languages(),
            vec!["de".to_string(), "en".to_string()]
        );
        for field in &schema.fields {
            for lang in ["en", "de"] {
                // The map directly, NOT `lookup` — that one falls back to
                // English, so it would answer for a German string that isn't
                // there and this test would pass on a half-translated form.
                let catalogue = manifest.strings.0.get(lang).expect("a declared language");
                for key in [field.label_key.as_deref(), field.hint_key.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    assert!(
                        catalogue.contains_key(key),
                        "`{key}` has no {lang} translation"
                    );
                }
            }
        }
    }
}
