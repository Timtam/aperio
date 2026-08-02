//! Todoist tasks adapter packaged as a plugin (DESIGN.md §20).
//!
//! Single-capability tasks adapter — fills only the
//! `tasks` slot in [`AdapterVtable`].
//!
//! ## Init config
//!
//! ```json
//! { "token": "…" }
//! ```
//!
//! Todoist auth is a long-lived API token; the OAuth dance
//! (when used) stays host-side + the host threads the token in
//! via `config_json` per ABI v2's `open_instance` hook.

use std::os::raw::{c_char, c_void};

use adapter_todoist::TodoistAdapter;
use cal_core::adapter::{Capability, Credentials as CalCredentials};
use cal_core::types::{MemberRight, NewTask};
use cal_core::TasksFeature;
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{AdapterVtable, TasksVtable};
use plugin_sdk::{decode_args, ok_response, open_instance_with, PluginInstance};
use serde::Deserialize;

plugin_sdk::cal_dispatch_helpers!(TodoistAdapter);

#[derive(Debug, Deserialize)]
struct InitConfig {
    token: String,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.token.trim().is_empty() {
            return Err("token must not be empty".to_string());
        }
        Ok(TodoistAdapter::new(cfg.token))
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<TodoistAdapter>::drop_handle(handle);
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

// ── Collaboration (DESIGN §9.7) ────────────────────────────
//
// Todoist's REST v2 exposes only the read side: the project's
// collaborators (the assignee pool). `current_user` + membership
// add/remove need the Sync API and stay null in this vtable.

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
    list_task_list_shares: Some(ffi_list_task_list_shares),
    add_task_list_member: Some(ffi_add_task_list_member),
    remove_task_list_member: Some(ffi_remove_task_list_member),
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
    id: "com.aperio.cal-adapter-todoist",
    name: "Aperio Todoist",
    version: "0.1.0",
    plugin_type: "adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}

#[cfg(test)]
mod tests {

    /// The manifest ships beside this crate and is the ONLY thing that tells
    /// the host how to set up a Todoist account. Parsing it here means a typo
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
        let schema = manifest()
            .account
            .expect("Todoist declares an account schema");
        let known = ["token"];
        for field in &schema.fields {
            assert!(
                known.contains(&field.key.as_str()),
                "schema field `{}` is not read by InitConfig",
                field.key
            );
        }
        // Todoist authenticates with a long-lived API token the user pastes in;
        // there is no flow for the host to run, so declaring one would send it
        // looking for an `aperio_plugin_interactive_auth` this crate does not
        // export.
        assert!(schema.oauth.is_none(), "Todoist has no OAuth flow");
    }

    #[test]
    fn the_token_is_routed_away_from_the_account_row() {
        use plugin_sdk::plugin_core::account_schema::{AccountFieldKind, AccountSecretSlot};
        let schema = manifest().account.unwrap();
        let token = schema.field("token").expect("the one declared field");
        assert_eq!(token.kind, AccountFieldKind::Secret);
        // `api_token` rather than `password`: it is what the host's own slot
        // selection has always used for Todoist, and the keychain service
        // suffix is derived from it — the wrong slot would write the token
        // where nothing ever reads it back.
        assert_eq!(token.secret_slot, Some(AccountSecretSlot::ApiToken));
        assert!(token.required);
        // Nothing else is collected, so nothing else can land in `config_json`.
        assert_eq!(schema.fields.len(), 1);
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
