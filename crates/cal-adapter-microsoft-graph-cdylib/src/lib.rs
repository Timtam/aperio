//! cdylib FFI shell for `cal-adapter-microsoft-graph-plugin` — emits the #[no_mangle] C-ABI plugin
//! exports the desktop dlopen loader resolves. All logic lives in the
//! -plugin rlib; this crate is the one place the duplicate-prone symbols
//! are defined, so the mobile static-link build can pull in every -plugin
//! rlib without colliding on them.

plugin_sdk::declare_cdylib_exports! {
    plugin_crate: cal_adapter_microsoft_graph_plugin,
    interactive_auth: yes,
}
