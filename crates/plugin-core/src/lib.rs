//! `plugin-core` — Aperio's plugin ABI contract (DESIGN.md §20).
//!
//! This crate is the lowest layer of the plugin stack:
//!
//!   - The canonical C header `include/aperio_plugin.h` —
//!     authoritative ABI for non-Rust plugin authors.
//!   - The Rust mirror in [`abi`] — `#[repr(C)]` types that line
//!     up byte-for-byte with the header. Used by both the host
//!     (for `libloading`-based dispatch) and the SDK macros (for
//!     emitting the exported entry points).
//!   - Manifest parsing in [`manifest`] — `plugin.json` →
//!     [`PluginManifest`] with serde + compatibility gates.
//!   - Plugin-type + capability tags in [`plugin_type`] and
//!     [`capability`], with forward-compat `Unknown` variants so
//!     a plugin built for a future Aperio is at worst inert, never
//!     crashes the host.
//!   - Version helpers in [`version`] — the [`ABI_VERSION`]
//!     constant, a tiny home-rolled semver type, and the gates
//!     used at load time.
//!
//! ## What's NOT here yet
//!
//! - The per-feature vtable structs (CalendarVtable, TasksVtable,
//!   ContactsVtable, SyncVtable). These land in the next phase
//!   alongside the [`PluginManager`] runtime. P0's job is to lock
//!   down the lifecycle surface + manifest contract first.
//! - The actual loader. P0 only types. `libloading` shows up in
//!   plugin-core's deps in the P1 commit, not this one.

pub mod abi;
pub mod capability;
pub mod error;
pub mod manifest;
pub mod plugin_type;
pub mod version;

pub use abi::{
    AperioPlugin, AperioPluginCreateFn, AperioPluginDestroyFn,
    AperioPluginType, PLUGIN_ERR_INIT, PLUGIN_ERR_INTERNAL,
    PLUGIN_ERR_INVALID_CONFIG, PLUGIN_OK, SYMBOL_CREATE, SYMBOL_DESTROY,
};
pub use capability::Capability;
pub use error::{PluginError, PluginResult};
pub use manifest::{PluginManifest, MANIFEST_FILENAME};
pub use plugin_type::PluginType;
pub use version::{check_abi_version, check_min_app_version, Version, ABI_VERSION};
