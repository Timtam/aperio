//! cdylib FFI shell for `vc-adapter-webex-plugin` — emits the #[no_mangle] C-ABI plugin
//! exports the desktop dlopen loader resolves. All logic lives in the
//! -plugin rlib; this crate is the one place the duplicate-prone symbols
//! are defined, so the mobile static-link build can pull in every -plugin
//! rlib without colliding on them.

plugin_sdk::declare_cdylib_exports! {
    plugin_crate: vc_adapter_webex_plugin,
    // Webex signs in interactively. The rlib declares the handler; without
    // this line the shared library never exports the symbol, and the desktop's
    // dlopen loader — which resolves it by name — reports the plugin as not
    // supporting interactive auth at all.
    interactive_auth: yes,
}
