//! Vikunja tasks adapter packaged as a plugin (DESIGN.md §20).
//!
//! Single-capability tasks adapter — same shape as
//! adapter-todoist-plugin, but Vikunja additionally needs
//! the self-hosted instance's `server_url`.
//!
//! ## Init config
//!
//! ```json
//! {
//!   "server_url": "https://vikunja.example.com",
//!   "token": "…"
//! }
//! ```

use std::os::raw::{c_char, c_void};

use adapter_vikunja::VikunjaAdapter;
use cal_core::adapter::{Capability, Credentials as CalCredentials};
use cal_core::types::{MemberRight, NewTask};
use cal_core::TasksFeature;
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{AdapterVtable, TasksVtable};
use plugin_sdk::{decode_args, ok_response, open_instance_with, PluginInstance};
use serde::Deserialize;

plugin_sdk::cal_dispatch_helpers!(VikunjaAdapter);

#[derive(Debug, Deserialize)]
struct InitConfig {
    server_url: String,
    token: String,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.server_url.trim().is_empty() || cfg.token.trim().is_empty() {
            return Err("server_url and token must not be empty".to_string());
        }
        VikunjaAdapter::new(cfg.server_url.trim(), cfg.token)
            .map_err(|e| format!("adapter ctor failed: {e:?}"))
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<VikunjaAdapter>::drop_handle(handle);
}

// ── Adapter base ───────────────────────────────────────────

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

// ── TasksFeature ───────────────────────────────────────────

unsafe extern "C" fn ffi_list_task_lists(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_task_lists().await })
}

unsafe extern "C" fn ffi_get_tasks(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.get_tasks(&list_id).await })
}

#[derive(Debug, Deserialize)]
struct CreateTaskArgs {
    list_id: String,
    task: NewTask,
}

unsafe extern "C" fn ffi_create_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateTaskArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.create_task(&args.list_id, args.task).await
    })
}

unsafe extern "C" fn ffi_update_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let task: cal_core::Task = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.update_task(task).await })
}

unsafe extern "C" fn ffi_delete_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let task_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move { p.delete_task(&task_id).await })
}

#[derive(Debug, Deserialize)]
struct RenameTaskListArgs {
    list_id: String,
    new_name: String,
}

unsafe extern "C" fn ffi_rename_task_list(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: RenameTaskListArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.rename_task_list(&args.list_id, &args.new_name).await
    })
}

unsafe extern "C" fn ffi_list_sections(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.list_sections(&list_id).await })
}

#[derive(Debug, Deserialize)]
struct CreateTaskListArgs {
    name: String,
    parent_id: Option<String>,
}

unsafe extern "C" fn ffi_create_task_list(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: CreateTaskListArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.create_task_list(&args.name, args.parent_id.as_deref())
            .await
    })
}

unsafe extern "C" fn ffi_delete_task_list(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(
        h,
        move |p| async move { p.delete_task_list(&list_id).await },
    )
}

#[derive(Debug, Deserialize)]
struct CreateSectionArgs {
    list_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct UpdateSectionArgs {
    list_id: String,
    section_id: String,
    new_name: String,
}

#[derive(Debug, Deserialize)]
struct DeleteSectionArgs {
    list_id: String,
    section_id: String,
}

unsafe extern "C" fn ffi_create_section(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: CreateSectionArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.create_section(&args.list_id, &args.name).await
    })
}

unsafe extern "C" fn ffi_update_section(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: UpdateSectionArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.update_section(&args.list_id, &args.section_id, &args.new_name)
            .await
    })
}

unsafe extern "C" fn ffi_delete_section(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: DeleteSectionArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.delete_section(&args.list_id, &args.section_id).await
    })
}

unsafe extern "C" fn ffi_list_task_list_members(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.list_task_list_members(&list_id).await
    })
}

unsafe extern "C" fn ffi_current_user(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch(h, move |p| async move { p.current_user().await })
}

unsafe extern "C" fn ffi_list_task_list_shares(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.list_task_list_shares(&list_id).await
    })
}

unsafe extern "C" fn ffi_search_users(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let query: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.search_users(&query).await })
}

#[derive(Debug, Deserialize)]
struct AddMemberArgs {
    list_id: String,
    member_ref: String,
    #[serde(default)]
    right: Option<MemberRight>,
}

unsafe extern "C" fn ffi_add_task_list_member(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: AddMemberArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.add_task_list_member(&args.list_id, &args.member_ref, args.right)
            .await
    })
}

#[derive(Debug, Deserialize)]
struct MemberRefArgs {
    list_id: String,
    member_ref: String,
}

unsafe extern "C" fn ffi_remove_task_list_member(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: MemberRefArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.remove_task_list_member(&args.list_id, &args.member_ref)
            .await
    })
}

#[derive(Debug, Deserialize)]
struct SetRightArgs {
    list_id: String,
    member_ref: String,
    right: MemberRight,
}

unsafe extern "C" fn ffi_set_task_list_member_right(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: SetRightArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.set_task_list_member_right(&args.list_id, &args.member_ref, args.right)
            .await
    })
}

pub static TASKS_VTABLE: TasksVtable = TasksVtable {
    authenticate: Some(ffi_authenticate),
    capabilities: Some(ffi_capabilities),
    list_task_lists: Some(ffi_list_task_lists),
    get_tasks: Some(ffi_get_tasks),
    create_task: Some(ffi_create_task),
    update_task: Some(ffi_update_task),
    delete_task: Some(ffi_delete_task),
    rename_task_list: Some(ffi_rename_task_list),
    list_sections: Some(ffi_list_sections),
    create_task_list: Some(ffi_create_task_list),
    delete_task_list: Some(ffi_delete_task_list),
    list_task_list_members: Some(ffi_list_task_list_members),
    current_user: Some(ffi_current_user),
    list_task_list_shares: Some(ffi_list_task_list_shares),
    search_users: Some(ffi_search_users),
    add_task_list_member: Some(ffi_add_task_list_member),
    remove_task_list_member: Some(ffi_remove_task_list_member),
    set_task_list_member_right: Some(ffi_set_task_list_member_right),
    create_section: Some(ffi_create_section),
    update_section: Some(ffi_update_section),
    delete_section: Some(ffi_delete_section),
    ..TasksVtable::empty()
};

pub static ADAPTER_VTABLE: AdapterVtable = AdapterVtable {
    tasks: &TASKS_VTABLE,
    ..AdapterVtable::empty()
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-vikunja",
    name: "Aperio Vikunja",
    version: "0.1.0",
    plugin_type: "adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}

#[cfg(test)]
mod tests {

    /// The manifest ships beside this crate and is the ONLY thing that tells
    /// the host how to set up a Vikunja account. Parsing it here means a typo
    /// fails the build rather than the first user who tries to connect.
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
        //
        // Vikunja is where that is easiest to get wrong: the form has always
        // called the credential an "API token", but the key `InitConfig`
        // deserialises is `token`. The wording belongs in the label; the key
        // has to be what the struct reads.
        let schema = manifest()
            .account
            .expect("Vikunja declares an account schema");
        let known = ["server_url", "token"];
        for field in &schema.fields {
            assert!(
                known.contains(&field.key.as_str()),
                "schema field `{}` is not read by InitConfig",
                field.key
            );
        }
        // Both keys are required by `InitConfig` — it has no `Option` and no
        // `serde(default)`, so a missing one fails the whole parse and the
        // instance never opens.
        for key in known {
            let field = schema
                .field(key)
                .unwrap_or_else(|| panic!("`{key}` must be declared — InitConfig has no default"));
            assert!(field.required, "`{key}` is not optional to InitConfig");
        }
        // A long-lived token the user pastes in; there is no flow for the host
        // to run, so declaring one would send it looking for an
        // `aperio_plugin_interactive_auth` this crate does not export.
        assert!(schema.oauth.is_none(), "Vikunja has no OAuth flow");
    }

    #[test]
    fn the_token_is_routed_away_from_the_account_row_and_the_url_is_not() {
        use plugin_sdk::plugin_core::account_schema::{AccountFieldKind, AccountSecretSlot};
        let schema = manifest().account.unwrap();

        let token = schema.field("token").expect("the credential");
        assert_eq!(token.kind, AccountFieldKind::Secret);
        // `api_token` rather than `password`: it is what the host's own slot
        // selection has always used for Vikunja, and the keychain service
        // suffix is derived from it — the wrong slot would write the token
        // where nothing ever reads it back.
        assert_eq!(token.secret_slot, Some(AccountSecretSlot::ApiToken));

        // The server URL is NOT a secret. It has to stay in `config_json`:
        // it is what tells an account which instance it belongs to, and the
        // host shows it back to the user.
        let url = schema.field("server_url").expect("the instance");
        assert_eq!(url.kind, AccountFieldKind::Url);
        assert!(!url.is_secret());

        // Nothing else is collected, so nothing else can land in `config_json`.
        assert_eq!(schema.fields.len(), 2);
        // No rotating credential to report back, so no capability token.
        assert!(!schema.host_channel);
    }

    #[test]
    fn both_declared_languages_carry_every_key_the_schema_names() {
        let manifest = manifest();
        let schema = manifest.account.as_ref().unwrap();
        assert_eq!(manifest.strings.languages(), vec!["de", "en"]);
        for field in &schema.fields {
            for key in [field.label_key.as_deref(), field.hint_key.as_deref()]
                .into_iter()
                .flatten()
            {
                for lang in ["en", "de"] {
                    let resolved = manifest
                        .strings
                        .lookup(key, lang)
                        .unwrap_or_else(|| panic!("`{key}` is missing in `{lang}`"));
                    assert!(!resolved.trim().is_empty(), "`{key}` in `{lang}` is empty");
                }
                // A German reader must not silently get the English line back.
                assert_ne!(
                    manifest.strings.lookup(key, "de"),
                    manifest.strings.lookup(key, "en"),
                    "`{key}` falls through to English"
                );
            }
        }
    }
}
