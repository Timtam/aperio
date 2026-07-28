//! Rust mirror of `aperio_plugin.h` (DESIGN.md §20.3).
//!
//! Every type in this module is `#[repr(C)]` and binary-compatible
//! with its C-header counterpart. The host loads a plugin's shared
//! library, looks up `aperio_plugin_create`, calls it to get a
//! `*mut AperioPlugin`, and from there drives the lifecycle through
//! the function pointers in the struct + the type-specific vtable.
//!
//! ## Library vs. instance lifecycle (ABI v2)
//!
//! The descriptor returned by `aperio_plugin_create` is a process-
//! singleton: one per loaded shared library. The per-account /
//! per-server adapter instances are opened on top of that via the
//! descriptor's `open_instance` + `close_instance` hooks, which a
//! single library may invoke arbitrarily often (see DESIGN.md §6.4
//! — multiple Google accounts, multiple CalDAV servers, …).
//!
//! ABI v1 had `init` + `destroy` on the descriptor and a single
//! implicit instance per library; v2 split that into the
//! library lifecycle (`create` / `destroy`) and the instance
//! lifecycle (`open_instance` / `close_instance`).
//!
//! ## Safety
//!
//! All of these types are FFI primitives. They contain raw pointers
//! and are NOT `Send`/`Sync` from Rust's point of view; the manager
//! wraps `AperioPlugin` access in its own synchronisation. Don't
//! pass these structs directly through async tasks.

use std::os::raw::{c_char, c_int, c_void};

use crate::ffi::PluginBytes;

/// Lifecycle return codes — mirror of the `APERIO_PLUGIN_OK` etc.
/// constants in `aperio_plugin.h`. Returned by the descriptor's
/// `open_instance` hook via [`OpenInstanceResult::status`].
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

/// Return value of [`AperioPlugin::open_instance`]. Either carries a
/// non-NULL `instance` handle with `status == PLUGIN_OK`, or a
/// NULL handle with a non-OK status + an optional UTF-8 error
/// message in `error`.
///
/// The plugin owns the bytes in `error` and the host releases
/// them via [`PluginBytes::free_in_place`] after extracting the
/// message — same memory protocol as [`crate::ffi::PluginCallResult`].
#[repr(C)]
pub struct OpenInstanceResult {
    /// Opaque per-instance handle. NULL on error. The host
    /// stores it and passes it back to every vtable method as
    /// the first argument; on shutdown the host calls
    /// `close_instance(handle)` to release it.
    pub instance: *mut c_void,
    /// `PLUGIN_OK` on success, or one of the `PLUGIN_ERR_*`
    /// codes on failure.
    pub status: c_int,
    /// Optional plugin-owned UTF-8 error detail. Empty on
    /// success. Released by the host via `free_in_place` after
    /// it's been copied out.
    pub error: PluginBytes,
}

impl OpenInstanceResult {
    /// Helper for the host: synthesise the "plugin doesn't ship
    /// open_instance" outcome with a clear error message so the
    /// caller's status-to-error mapping stays uniform.
    pub fn missing_hook() -> Self {
        Self {
            instance: std::ptr::null_mut(),
            status: PLUGIN_ERR_INIT,
            error: PluginBytes::empty(),
        }
    }
}

// SAFETY: pointer fields are read-only references to plugin-owned
// memory; we do not write through them and never share an
// [`OpenInstanceResult`] across threads outside the local
// "receive → decode → free" sequence the manager wraps it in.
unsafe impl Send for OpenInstanceResult {}

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
    /// proceed when this doesn't equal [`crate::ABI_VERSION`].
    pub abi_version: u32,

    /// Reverse-DNS id. Must match the manifest's `id` field.
    pub id: *const c_char,

    /// Display name. Already localised by the plugin if it cares.
    pub name: *const c_char,

    /// SemVer string.
    pub version: *const c_char,

    /// Plugin-type tag (`"calendar-adapter"`, `"sync-adapter"`, …).
    pub plugin_type: *const c_char,

    /// Open a new instance of the adapter (per account / per
    /// server). `config_json` is a NUL-terminated UTF-8 pointer
    /// (may be NULL or empty for instance-less plugins). The
    /// host calls this once per account it wants to wire up; a
    /// single loaded library may have N live instances at the
    /// same time (DESIGN.md §6.4).
    ///
    /// MAY be NULL for plugins that don't carry per-account
    /// state — the host then dispatches vtable methods with a
    /// NULL instance handle. Notification channels and other
    /// process-global plugins are the typical case.
    pub open_instance:
        Option<unsafe extern "C" fn(config_json: *const c_char) -> OpenInstanceResult>,

    /// Release an instance previously returned by
    /// [`Self::open_instance`]. Called by the host when the
    /// owning [`crate::manager::LoadedInstance`] is dropped —
    /// typically because the user deleted the corresponding
    /// account, or the app is shutting down.
    ///
    /// MAY be NULL iff [`Self::open_instance`] is also NULL.
    pub close_instance: Option<unsafe extern "C" fn(instance: *mut c_void)>,

    /// Type-specific vtable. The host casts it to the right struct
    /// pointer based on `plugin_type`. The concrete layouts live
    /// in [`crate::vtables`].
    pub vtable: *mut c_void,
}

/// Function-pointer type matching `aperio_plugin_create` in the C
/// header. The manager uses this with `libloading::Symbol` once
/// the binary is open.
pub type AperioPluginCreateFn = unsafe extern "C" fn() -> *mut AperioPlugin;

/// Function-pointer type matching `aperio_plugin_destroy`.
pub type AperioPluginDestroyFn = unsafe extern "C" fn(plugin: *mut AperioPlugin);

/// Severity values carried across the [`AperioLogFn`] boundary,
/// matching `tracing::Level` ordering (1 = most severe). Defined
/// here so the SDK's forwarding subscriber and the host's re-emit
/// agree on the wire values without sharing a Rust enum across the
/// dylib seam.
pub const LOG_LEVEL_ERROR: u8 = 1;
pub const LOG_LEVEL_WARN: u8 = 2;
pub const LOG_LEVEL_INFO: u8 = 3;
pub const LOG_LEVEL_DEBUG: u8 = 4;
pub const LOG_LEVEL_TRACE: u8 = 5;

/// Host-supplied log sink handed to a plugin via
/// `aperio_plugin_set_log`. The plugin installs a `tracing`
/// subscriber that forwards each event by calling this with the
/// level (see the `LOG_LEVEL_*` constants), the event `target`, and
/// the rendered message — both NUL-terminated UTF-8 and valid only
/// for the duration of the call. The callback must not unwind across
/// the FFI boundary.
///
/// This bridges the dlopen logging gap: each plugin `.so` statically
/// links its own `tracing` global, so without forwarding the host
/// never sees adapter logs. On the static-link (mobile) path the
/// global is already shared and `set_log` is simply never called.
pub type AperioLogFn =
    unsafe extern "C" fn(level: u8, target: *const c_char, message: *const c_char);

/// Function-pointer type matching the optional `aperio_plugin_set_log`
/// export: the host calls it once, right after `create`, to hand the
/// plugin its [`AperioLogFn`] sink. MAY be absent (plugins built
/// before this ABI addition) — the loader treats a missing symbol as
/// "no log forwarding".
pub type AperioPluginSetLogFn = unsafe extern "C" fn(log: AperioLogFn);

/// Outcome of one [`AperioHostChannelFn`] call, mirroring the
/// `APERIO_HOST_CHANNEL_*` constants in `aperio_plugin.h`.
///
/// A plugin may act on these — retry, give up, back off — but is not
/// obliged to. `ACCEPTED` is the only one that promises anything: the
/// host has taken durable responsibility for what was reported.
pub const HOST_CHANNEL_ACCEPTED: c_int = 0;
/// The `kind` is one this host does not know. Older host, newer plugin.
pub const HOST_CHANNEL_UNKNOWN_KIND: c_int = 1;
/// The capability token names no live account. The instance was closed,
/// or the token was never one the host issued.
pub const HOST_CHANNEL_UNKNOWN_SCOPE: c_int = 2;
/// Understood and deliberately declined — a credential slot that may not
/// be written, for instance.
pub const HOST_CHANNEL_REFUSED: c_int = 3;
/// Understood and attempted, but it did not land. Worth retrying later.
pub const HOST_CHANNEL_FAILED: c_int = 4;
/// The envelope was not readable at all.
pub const HOST_CHANNEL_MALFORMED: c_int = 5;
/// Too many reports from this scope too quickly.
pub const HOST_CHANNEL_THROTTLED: c_int = 6;

/// Host-supplied sink for out-of-band reports from a plugin, handed over
/// by the optional `aperio_plugin_set_host_channel` export.
///
/// This is the ONLY channel from a plugin back to the host other than the
/// log bridge. Vtable calls run host→plugin and return data; nothing in
/// them lets a plugin say "by the way, my refresh token changed" — which
/// is exactly what an OAuth provider that rotates credentials forces an
/// adapter to say.
///
/// `json_ptr` / `json_len` describe a plugin-owned UTF-8 JSON object,
/// valid only for the duration of the call. Pointer-plus-length rather
/// than a NUL-terminated string because that is what every other
/// JSON-carrying boundary in this ABI already uses, and because it avoids
/// a `CString` round-trip on a live secret. The callback must not unwind
/// across the FFI boundary.
///
/// The envelope is `{"v":1,"scope":"…","kind":"…","payload":{…}}`. `kind`
/// is an open string rather than an enum so a later signal costs no ABI
/// change; a host that does not know one answers
/// [`HOST_CHANNEL_UNKNOWN_KIND`] and carries on.
pub type AperioHostChannelFn = unsafe extern "C" fn(json_ptr: *const u8, json_len: usize) -> c_int;

/// Function-pointer type matching the optional
/// `aperio_plugin_set_host_channel` export: the host calls it once, right
/// after `create`, to hand the plugin its [`AperioHostChannelFn`]. MAY be
/// absent — a plugin that never reports anything simply does not export
/// it, and the loader treats the missing symbol as "no channel".
pub type AperioPluginSetHostChannelFn = unsafe extern "C" fn(sink: AperioHostChannelFn);

/// Envelope schema version. A host rejects anything else as malformed
/// rather than guessing.
pub const HOST_CHANNEL_ENVELOPE_V1: u32 = 1;

/// `kind` for "the credential I hold for this account has changed".
pub const KIND_CREDENTIAL_ROTATED: &str = "credential.rotated";

/// Reserved key the host splices into the TRANSIENT merged config it
/// hands `open_instance`, carrying the capability token that lets an
/// instance name its own account when it reports back.
///
/// It is never written to the persisted account row, and therefore never
/// reaches the sync event log. The token is opaque and unguessable by
/// design: deriving it from the account id would let any plugin in the
/// process forge another account's identity by string concatenation.
pub const HOST_TOKEN_CONFIG_KEY: &str = "__aperio_host_token";

/// Hard ceiling on one envelope, enforced before any allocation.
pub const HOST_CHANNEL_MAX_PAYLOAD: usize = 64 * 1024;

/// Symbol names the host's loader looks up in every plugin's
/// shared library. Defined here so the SDK macros emit
/// `#[no_mangle] pub extern "C" fn` items with the exact same
/// spelling.
pub const SYMBOL_CREATE: &[u8] = b"aperio_plugin_create";
pub const SYMBOL_DESTROY: &[u8] = b"aperio_plugin_destroy";
/// Optional log-forwarding sink installer; see [`AperioPluginSetLogFn`].
pub const SYMBOL_SET_LOG: &[u8] = b"aperio_plugin_set_log";
/// Optional plugin→host channel installer; see [`AperioPluginSetHostChannelFn`].
pub const SYMBOL_SET_HOST_CHANNEL: &[u8] = b"aperio_plugin_set_host_channel";

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    /// Sanity check that the layout the C side sees lines up with
    /// what we generate. If a future change accidentally inserts
    /// a non-`#[repr(C)]` field this test starts catching it.
    #[test]
    fn aperio_plugin_layout_is_what_we_expect() {
        assert!(align_of::<AperioPlugin>() >= align_of::<*const c_void>());
        let pointer_size = size_of::<*const c_void>();
        // 4-byte abi_version + alignment padding + 4 ptr-strings +
        // 2 fn pointers (open/close) + 1 vtable pointer = at least
        // 7 pointer-slots after the u32.
        let minimum = 4 + 7 * pointer_size;
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
        assert_eq!(AperioPluginType::Unknown as u32, 0);
        assert_eq!(AperioPluginType::CalendarAdapter as u32, 1);
        assert_eq!(AperioPluginType::SyncAdapter as u32, 2);
        assert_eq!(AperioPluginType::VideoconferenceAdapter as u32, 3);
        assert_eq!(AperioPluginType::Notification as u32, 4);
    }

    #[test]
    fn log_levels_match_c_header_values() {
        // Mirror APERIO_PLUGIN_LOG_LEVEL_* in include/aperio_plugin.h.
        // The header sync is convention-enforced (no codegen), so this
        // guards against silent Rust-side drift.
        assert_eq!(LOG_LEVEL_ERROR, 1);
        assert_eq!(LOG_LEVEL_WARN, 2);
        assert_eq!(LOG_LEVEL_INFO, 3);
        assert_eq!(LOG_LEVEL_DEBUG, 4);
        assert_eq!(LOG_LEVEL_TRACE, 5);
    }

    #[test]
    fn open_instance_missing_hook_is_failure_with_null_handle() {
        let r = OpenInstanceResult::missing_hook();
        assert!(r.instance.is_null());
        assert_ne!(r.status, PLUGIN_OK);
        assert!(r.error.is_empty());
    }
}
