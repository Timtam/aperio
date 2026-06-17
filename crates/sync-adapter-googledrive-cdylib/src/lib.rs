//! cdylib FFI shell for `sync-adapter-googledrive-plugin` — emits the #[no_mangle] C-ABI plugin
//! exports the desktop dlopen loader resolves. All logic lives in the
//! -plugin rlib; this crate is the one place the duplicate-prone symbols
//! are defined, so the mobile static-link build can pull in every -plugin
//! rlib without colliding on them.

plugin_sdk::declare_cdylib_exports! {
    plugin_crate: sync_adapter_googledrive_plugin,
    interactive_auth: yes,
}
