//! Rust mirror of `aperio_plugin.h` (DESIGN.md §20.3).
//!
//! Every type in this module is `#[repr(C)]` and binary-compatible
//! with its C-header counterpart. The host loads a plugin's shared
//! library, looks up `aperio_plugin_create`, calls it to get a
//! `*mut AperioPlugin`, and from there drives the lifecycle through
//! the function pointers in the struct + the type-specific vtable.
//!
//! ## Scope of this module in P0
//!
//! Only the lifecycle + metadata surface lands here. The per-feature
//! vtable structs (`CalendarVtable`, `SyncVtable`, …) come in P1
//! together with the manager — designing them correctly requires
//! deciding how to bridge `async fn` across the FFI boundary, which
//! is a piece of design in its own right. P0's contribution is the
//! shape of [`AperioPlugin`] itself, the lifecycle return-code
//! constants, and the [`AperioPluginType`] tag — which is enough to
//! let plugin authors start writing manifests + lifecycle stubs and
//! enough for the host to refuse mismatched ABI versions before
//! ever touching the binary.
//!
//! ## Safety
//!
//! All of these types are FFI primitives. They contain raw pointers
//! and are NOT `Send`/`Sync` from Rust's point of view; the manager
//! wraps `AperioPlugin` access in its own synchronisation. Don't
//! pass these structs directly through async tasks.

use std::os::raw::{c_char, c_int, c_void};

/// Lifecycle return codes — mirror of the `APERIO_PLUGIN_OK` etc.
/// constants in `aperio_plugin.h`. Returned by [`AperioPlugin::init`].
///
/// Repeated here as Rust constants so host code reads cleanly
/// without having to cast `c_int` literals everywhere. Adding a
/// new code requires a coordinated update to the C header.
pub const PLUGIN_OK: c_int = 0;
pub const PLUGIN_ERR_INIT: c_int = 1;
pub const PLUGIN_ERR_INVALID_CONFIG: c_int = 2;
pub const PLUGIN_ERR_INTERNAL: c_int = 3;

/// Plugin-type tag — mirror of the `AperioPluginType` enum in
/// `aperio_plugin.h`. The wire string carried in
/// [`AperioPlugin::plugin_type`] is the canonical form; this enum
/// is a convenience for C consumers doing strcmp-free dispatch.
///
/// Rust hosts go through [`crate::PluginType`] instead — that one
/// has serde + a forward-compat `Unknown` variant that's friendlier
/// to read manifests against.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AperioPluginType {
    Unknown = 0,
    CalendarAdapter = 1,
    SyncAdapter = 2,
    VideoconferenceAdapter = 3,
    Notification = 4,
}

/// Plugin descriptor — return value of `aperio_plugin_create`.
///
/// Memory ownership: every pointer field is owned by the plugin
/// and remains valid until `aperio_plugin_destroy` returns. The
/// host MUST NOT free any of them. Strings are NUL-terminated
/// UTF-8 (cf. `aperio_plugin.h`).
///
/// Layout MUST stay binary-compatible across plugin-core 0.x patch
/// versions. Adding fields requires bumping [`crate::ABI_VERSION`].
#[repr(C)]
pub struct AperioPlugin {
    /// ABI version emitted by the plugin. The manager refuses to
    /// proceed when this doesn't equal [`ABI_VERSION`].
    pub abi_version: u32,

    /// Reverse-DNS id. Must match the manifest's `id` field.
    pub id: *const c_char,

    /// Display name. Already localised by the plugin if it cares.
    pub name: *const c_char,

    /// SemVer string.
    pub version: *const c_char,

    /// Plugin-type tag (`"calendar-adapter"`, `"sync-adapter"`, …).
    pub plugin_type: *const c_char,

    /// Optional lifecycle hook. Called once before any feature-
    /// vtable methods. `config_json` is a NUL-terminated UTF-8
    /// pointer (may be NULL or empty). Returns one of the
    /// `PLUGIN_OK` / `PLUGIN_ERR_*` constants.
    ///
    /// MAY be NULL — feature-vtable-only plugins skip it.
    pub init:
        Option<unsafe extern "C" fn(config_json: *const c_char) -> c_int>,

    /// Optional teardown hook. Called once after the last
    /// feature-vtable call. MUST release whatever `init` acquired.
    ///
    /// MAY be NULL.
    pub destroy: Option<unsafe extern "C" fn()>,

    /// Type-specific vtable. The host casts it to the right struct
    /// pointer based on `plugin_type`. The concrete layouts land
    /// in P1.
    pub vtable: *mut c_void,
}

/// Function-pointer type matching `aperio_plugin_create` in the C
/// header. The manager uses this with `libloading::Symbol` once
/// the binary is open.
pub type AperioPluginCreateFn = unsafe extern "C" fn() -> *mut AperioPlugin;

/// Function-pointer type matching `aperio_plugin_destroy`.
pub type AperioPluginDestroyFn = unsafe extern "C" fn(plugin: *mut AperioPlugin);

/// Symbol names the host's loader (P1) looks up in every plugin's
/// shared library. Defined here so the SDK macros (P2) can emit
/// `#[no_mangle] pub extern "C" fn` items with the exact same
/// spelling.
pub const SYMBOL_CREATE: &[u8] = b"aperio_plugin_create";
pub const SYMBOL_DESTROY: &[u8] = b"aperio_plugin_destroy";

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    /// Sanity check that the layout the C side sees lines up with
    /// what we generate. If a future change accidentally inserts
    /// a non-`#[repr(C)]` field this test starts catching it.
    #[test]
    fn aperio_plugin_layout_is_what_we_expect() {
        // 4-byte abi_version + alignment padding + 4 pointers +
        // 2 function pointers + 1 vtable pointer.
        // We don't assert exact sizes (they're target-dependent),
        // but the struct must at least be pointer-aligned.
        assert!(align_of::<AperioPlugin>() >= align_of::<*const c_void>());
        // And big enough to hold every field.
        let pointer_size = size_of::<*const c_void>();
        let minimum = 4 + 7 * pointer_size; // 4 ptr-strings + 2 fn + 1 void
        assert!(size_of::<AperioPlugin>() >= minimum);
    }

    #[test]
    fn return_code_constants_are_distinct() {
        let codes = [
            PLUGIN_OK,
            PLUGIN_ERR_INIT,
            PLUGIN_ERR_INVALID_CONFIG,
            PLUGIN_ERR_INTERNAL,
        ];
        for (i, a) in codes.iter().enumerate() {
            for b in &codes[i + 1..] {
                assert_ne!(a, b, "duplicate return codes");
            }
        }
    }

    #[test]
    fn plugin_type_enum_matches_c_header_values() {
        // The values here must stay aligned with the C header's
        // `AperioPluginType` so a plugin author who hard-codes the
        // integer in C still gets the right dispatch.
        assert_eq!(AperioPluginType::Unknown as u32, 0);
        assert_eq!(AperioPluginType::CalendarAdapter as u32, 1);
        assert_eq!(AperioPluginType::SyncAdapter as u32, 2);
        assert_eq!(AperioPluginType::VideoconferenceAdapter as u32, 3);
        assert_eq!(AperioPluginType::Notification as u32, 4);
    }
}
