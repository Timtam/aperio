//! `plugin-core` — Aperio's plugin ABI + runtime (DESIGN.md §20).
//!
//! Layered:
//!
//!   - **Contract** ([`abi`]): the `#[repr(C)]` types that line up
//!     byte-for-byte with the canonical C header in
//!     `include/aperio_plugin.h`. Lifecycle, type tag, symbol
//!     names.
//!   - **Manifest** ([`manifest`]): `plugin.json` →
//!     [`PluginManifest`] with serde + compatibility gates.
//!   - **FFI primitives** ([`ffi`]): shared [`PluginBytes`] /
//!     [`PluginCallResult`] / status codes that every vtable
//!     method uses for arguments + responses.
//!   - **Vtables** ([`vtables`]): one `#[repr(C)]` struct of fn
//!     pointers per plugin type — calendar, tasks, contacts,
//!     sync — mirrored against the cal-core / sync-core trait
//!     surfaces.
//!   - **Manager** ([`manager`]): runtime that walks
//!     `plugins/bundled/` + `plugins/user/`, dlopens the right
//!     library, validates ABI + min-app-version, and stores the
//!     loaded [`manager::LoadedPlugin`]s keyed by manifest id.
//!   - **Shim** ([`shim`]): Rust-side adapters that implement
//!     cal-core / sync-core traits by dispatching across the
//!     FFI boundary. P1 ships
//!     [`shim::FfiCalendarAdapter`] as the canonical pattern;
//!     the Tasks / Contacts / Sync shims arrive in P1b.

pub mod abi;
pub mod archive;
pub mod capability;
pub mod error;
pub mod ffi;
pub mod manager;
pub mod manifest;
pub mod plugin_type;
pub mod shim;
pub mod vtables;
pub mod version;

pub use abi::{
    AperioPlugin, AperioPluginCreateFn, AperioPluginDestroyFn,
    AperioPluginType, OpenInstanceResult, PLUGIN_ERR_INIT, PLUGIN_ERR_INTERNAL,
    PLUGIN_ERR_INVALID_CONFIG, PLUGIN_OK, SYMBOL_CREATE, SYMBOL_DESTROY,
};
pub use capability::Capability;
pub use error::{PluginError, PluginResult};
pub use ffi::{
    PluginBytes, PluginCallResult, PLUGIN_CALL_ERR_AUTH,
    PLUGIN_CALL_ERR_CONFLICT, PLUGIN_CALL_ERR_FORBIDDEN,
    PLUGIN_CALL_ERR_INTERNAL as PLUGIN_CALL_ERR_INTERNAL_FFI,
    PLUGIN_CALL_ERR_INVALID, PLUGIN_CALL_ERR_IO,
    PLUGIN_CALL_ERR_NETWORK, PLUGIN_CALL_ERR_NOT_FOUND,
    PLUGIN_CALL_ERR_PROTOCOL, PLUGIN_CALL_ERR_UNSUPPORTED, PLUGIN_CALL_OK,
};
pub use archive::{inspect_archive, install_archive, InstalledArchive};
pub use manager::{
    DiscoverError, DiscoverFn, InteractiveAuthError, InteractiveAuthFn,
    LoadedInstance, LoadedPlugin, PluginManager, ProbeHostKeyError,
    ProbeHostKeyFn, BUNDLED_PLUGINS_DIR, SYMBOL_DISCOVER,
    SYMBOL_INTERACTIVE_AUTH, SYMBOL_PROBE_HOST_KEY, USER_PLUGINS_DIR,
};
pub use manifest::{PluginManifest, MANIFEST_FILENAME};
pub use plugin_type::PluginType;
pub use shim::{
    FfiCalendarAdapter, FfiContactsAdapter, FfiSyncAdapter, FfiTasksAdapter,
    FfiVcAdapter,
};
pub use version::{check_abi_version, check_min_app_version, Version, ABI_VERSION};
pub use vtables::{
    CalendarAdapterVtable, CalendarVtable, ContactsVtable, SyncVtable, TasksVtable,
    VcVtable,
};
