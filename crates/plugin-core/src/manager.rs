//! Plugin manager runtime (DESIGN.md §20.5).
//!
//! Owns every plugin shared library Aperio has loaded into the
//! process. Lifecycle:
//!
//!   1. Host calls [`PluginManager::new`] at app start.
//!   2. Host calls [`PluginManager::scan_dir`] for each plugin
//!      root (`plugins/bundled/`, `plugins/user/`).
//!   3. For every subdirectory the manager finds, it reads
//!      `plugin.json`, validates ABI + min-app-version against the
//!      running build, and (when both gates pass) `dlopen`s the
//!      platform-appropriate shared library next to the manifest.
//!   4. The library's `aperio_plugin_create` entry point is
//!      called to produce the [`AperioPlugin`] descriptor. The
//!      descriptor lives until the [`LoadedPlugin`] is dropped.
//!   5. Per-account work happens via [`PluginManager::open_instance`]:
//!      the host hands the descriptor a JSON config and gets back
//!      a [`LoadedInstance`]. A single loaded library can back N
//!      independent instances (DESIGN.md §6.4).
//!   6. When an account is removed (or the app shuts down) the
//!      [`LoadedInstance`] is dropped — its `Drop` calls the
//!      descriptor's `close_instance` hook. When the last
//!      reference to a [`LoadedPlugin`] goes away the manager
//!      runs `aperio_plugin_destroy` + `dlclose`.
//!
//! ## Thread safety
//!
//! `Arc<PluginManager>` is the canonical sharing shape — multiple
//! Tauri command handlers hold the same Arc concurrently. Lookups
//! ([`PluginManager::get`], [`PluginManager::all`]) take an
//! `RwLock` read guard; loads + unloads take a write guard.
//! Plugin vtable invocations themselves don't need to touch the
//! manager's lock — the host snapshots the [`LoadedInstance`] Arc
//! once and calls into the vtable directly.
//!
//! ## Static-plugins build
//!
//! The `static-plugins` feature flag (DESIGN.md §20.6) flips the
//! manager into a path where bundled adapters are registered via
//! a compile-time list instead of `dlopen`. The
//! [`PluginManager::register_static`] entry point is what that
//! flag eventually calls.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use libloading::Library;
use tracing::{debug, error, info, trace, warn};

use crate::abi::{
    AperioPlugin, AperioPluginCreateFn, AperioPluginDestroyFn, AperioPluginSetHostChannelFn,
    AperioPluginSetLogFn, OpenInstanceResult, LOG_LEVEL_DEBUG, LOG_LEVEL_ERROR, LOG_LEVEL_INFO,
    LOG_LEVEL_WARN, PLUGIN_OK, SYMBOL_CREATE, SYMBOL_DESTROY, SYMBOL_SET_HOST_CHANNEL,
    SYMBOL_SET_LOG,
};
use crate::error::{PluginError, PluginResult};
use crate::ffi::{PluginCallResult, PLUGIN_CALL_OK};
use crate::manifest::{AdapterKindInfo, PluginManifest, MANIFEST_FILENAME};
use crate::plugin_type::PluginType;
use crate::strings::StringCatalogue;
use crate::version::check_abi_version;

/// Function-pointer type for the optional
/// `aperio_plugin_interactive_auth` symbol. Plugins that need an
/// OAuth dance (or any other interactive setup step) export this
/// alongside the lifecycle exports; the host looks it up by name
/// via `libloading` at plugin-load time and caches the result.
///
/// `args_ptr` / `args_len` carry a JSON document with whatever
/// setup data the host has (e.g. `{"client_id": "..."}`); the
/// returned [`PluginCallResult`]'s payload is the credential
/// blob the host should store opaquely + thread back into
/// `open_instance` later.
pub type InteractiveAuthFn =
    unsafe extern "C" fn(args_ptr: *const u8, args_len: usize) -> PluginCallResult;

/// Canonical symbol name for the interactive-auth entry point.
pub const SYMBOL_INTERACTIVE_AUTH: &[u8] = b"aperio_plugin_interactive_auth";

/// Function-pointer type for the optional
/// `aperio_plugin_discover` symbol. Plugins that own a service-
/// discovery surface (EWS Autodiscover, CalDAV well-known URIs,
/// …) export this alongside the lifecycle exports.
///
/// `args_ptr` / `args_len` carry a JSON document with the inputs
/// discovery needs (e.g. `{"email": "...", "password": "..."}`);
/// the returned [`PluginCallResult`]'s payload is a JSON document
/// the host parses into its UI-facing shape. The host stays
/// adapter-crate-agnostic — only the plugin knows the protocol.
pub type DiscoverFn =
    unsafe extern "C" fn(args_ptr: *const u8, args_len: usize) -> PluginCallResult;

/// Canonical symbol name for the discover entry point.
pub const SYMBOL_DISCOVER: &[u8] = b"aperio_plugin_discover";

/// Function-pointer type for the optional `aperio_plugin_strings`
/// symbol. A plugin whose translations do not fit a JSON block in
/// its manifest — Fluent, gettext, plural rules, a catalogue it
/// fetches — exports this and answers per language.
///
/// `args_ptr` / `args_len` carry `{"lang": "de"}`; the payload is a
/// JSON object of key → string for that one language. The host
/// calls it ONCE per language and caches the answer, so a label
/// never costs an FFI call on a repaint, and merges it OVER the
/// manifest catalogue.
///
/// Optional by design. A plugin that ships its strings in the
/// manifest — the ordinary case, and the only one a translator can
/// send a pull request against — exports nothing.
pub type StringsFn = unsafe extern "C" fn(args_ptr: *const u8, args_len: usize) -> PluginCallResult;

/// Canonical symbol name for the strings entry point.
pub const SYMBOL_STRINGS: &[u8] = b"aperio_plugin_strings";

/// The optional named exports a STATICALLY linked plugin hands over.
///
/// The dlopen path finds these by symbol name in the loaded library. A static
/// consumer — the mobile build, where every adapter is compiled in — has no
/// library to search, so it passes the crate-mangled typed twins
/// (`<plugin>::__aperio_*_impl`) directly.
///
/// A struct rather than a parameter list because the list only ever grows, and
/// a call site reading `None, None, Some(x), None` says nothing about which
/// hook is which. Every field defaults to absent, so a caller names only what
/// it has:
///
/// ```ignore
/// StaticHooks { discover_fn: Some(my_plugin::__aperio_discover_impl), ..Default::default() }
/// ```
#[derive(Default)]
pub struct StaticHooks {
    pub interactive_auth_fn: Option<InteractiveAuthFn>,
    pub discover_fn: Option<DiscoverFn>,
    pub probe_host_key_fn: Option<ProbeHostKeyFn>,
    pub strings_fn: Option<StringsFn>,
}

/// Function-pointer type for the optional
/// `aperio_plugin_probe_host_key` symbol. Plugins that wrap a
/// TOFU-style transport (SFTP today; could extend to MQTT-over-
/// TLS or similar later) expose this so the host's trust dialog
/// can read the server's presented host-key fingerprint without
/// committing a pin or even authenticating.
///
/// `args_ptr` / `args_len` carry the connection inputs as JSON
/// (e.g. `{"host": "...", "port": 22}`); the returned
/// [`PluginCallResult`]'s payload is a JSON document carrying
/// the observed fingerprint. The host compares the fingerprint
/// against its own pinned-key store (kept device-local in
/// user_prefs) and renders the "first use" / "key changed" /
/// "unchanged" trust dialog accordingly.
pub type ProbeHostKeyFn =
    unsafe extern "C" fn(args_ptr: *const u8, args_len: usize) -> PluginCallResult;

/// Canonical symbol name for the probe-host-key entry point.
pub const SYMBOL_PROBE_HOST_KEY: &[u8] = b"aperio_plugin_probe_host_key";

/// Host log sink handed to every dlopen'd plugin via
/// `aperio_plugin_set_log`. A plugin's forwarding subscriber calls
/// this for each `tracing` event; we re-emit it into the host's own
/// subscriber (the log file) under the `aperio::plugin` target, with
/// the plugin's original event target preserved in the `source`
/// field. The host's level filter then decides what is written.
///
/// The level is matched, not threaded, because `tracing`'s macros
/// take a const level + const target; we keep the plugin's target as
/// a value field instead.
///
/// # Safety
///
/// FFI callback. `target` / `message` must be valid NUL-terminated
/// UTF-8 for the duration of the call (the plugin's forwarding
/// subscriber guarantees this); either may be NULL.
unsafe extern "C" fn forward_plugin_log(level: u8, target: *const c_char, message: *const c_char) {
    // SAFETY: caller contract — valid NUL-terminated UTF-8 or NULL.
    let read = |p: *const c_char| -> String {
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    let source = read(target);
    let message = read(message);
    match level {
        LOG_LEVEL_ERROR => error!(target: "aperio::plugin", source = %source, "{message}"),
        LOG_LEVEL_WARN => warn!(target: "aperio::plugin", source = %source, "{message}"),
        LOG_LEVEL_INFO => info!(target: "aperio::plugin", source = %source, "{message}"),
        LOG_LEVEL_DEBUG => debug!(target: "aperio::plugin", source = %source, "{message}"),
        // LOG_LEVEL_TRACE and any unknown future byte.
        _ => trace!(target: "aperio::plugin", source = %source, "{message}"),
    }
}

/// One loaded plugin — manifest + library handle + descriptor
/// pointer + the `destroy` symbol we need to call before unload.
///
/// Held inside `Arc` so the host can keep cheap references to
/// individual plugins beyond the manager's RwLock guard.
pub struct LoadedPlugin {
    /// Parsed `plugin.json`. Cloneable for the Settings UI to
    /// render without re-reading disk.
    pub manifest: PluginManifest,

    /// Pointer the plugin's `aperio_plugin_create` returned.
    /// Borrowed across the lifetime of [`Self::library`] — the
    /// vtable + every string field lives inside the library's
    /// static data so it stays valid until [`Self::library`] is
    /// dropped.
    plugin_ptr: *mut AperioPlugin,

    /// Cached `aperio_plugin_destroy` symbol. Looked up once at
    /// load time so the destructor path doesn't have to fail.
    destroy_fn: AperioPluginDestroyFn,

    /// Cached `aperio_plugin_interactive_auth` fn-pointer. `None`
    /// when the plugin doesn't export the symbol — most plugins
    /// don't need an OAuth dance + leave it unexported.
    interactive_auth_fn: Option<InteractiveAuthFn>,

    /// Cached `aperio_plugin_discover` fn-pointer. `None` when
    /// the plugin doesn't expose a service-discovery surface —
    /// most don't (only EWS Autodiscover today; CalDAV well-
    /// known URIs and Microsoft Graph endpoint probing are
    /// candidates for later).
    discover_fn: Option<DiscoverFn>,

    /// Cached `aperio_plugin_probe_host_key` fn-pointer. `None`
    /// when the plugin doesn't wrap a TOFU transport — most
    /// don't (only SFTP today).
    probe_host_key_fn: Option<ProbeHostKeyFn>,

    /// Cached `aperio_plugin_strings` fn-pointer. `None` for every
    /// plugin whose strings live in its manifest, which is the
    /// ordinary case.
    strings_fn: Option<StringsFn>,

    /// Per-language catalogues already resolved for this plugin.
    ///
    /// The manifest's block merged with whatever `strings_fn`
    /// answered, computed once per language. This is what keeps the
    /// escape hatch from putting a foreign code path on a repaint:
    /// a label lookup is a map read, and the FFI call happened the
    /// first time that language was asked for.
    strings_cache: RwLock<HashMap<String, Arc<StringCatalogue>>>,

    /// Number of FFI calls currently in flight against this
    /// plugin. The shim wrappers' trait methods bracket each
    /// dispatch with an [`InFlightGuard`] so the counter
    /// tracks active calls deterministically — strong_count
    /// on the LoadedPlugin Arc gets bumped by every shim
    /// clone too, so it can't be used as a "safe to unload"
    /// gate on its own. The unload path waits for this to
    /// reach 0 after the registry has dropped its shim Arcs.
    in_flight: Arc<AtomicUsize>,

    /// The dlopen'd library. Drop order:
    /// `aperio_plugin_destroy` → `library.drop()` (which calls
    /// `dlclose`). Static-plugin builds set this to `None` so
    /// dropping doesn't try to unload anything.
    #[allow(dead_code)] // kept alive purely so dlclose runs at drop time
    library: Option<Library>,
}

// SAFETY: the plugin's API contract requires the vtable methods
// to be thread-safe (see DESIGN.md §20.5 + ffi.rs module docs).
// The `plugin_ptr` itself is just a `*mut` that we read from
// concurrently; we never write through it after init. The library
// handle is `Send + Sync` by virtue of libloading's design.
unsafe impl Send for LoadedPlugin {}
unsafe impl Sync for LoadedPlugin {}

impl LoadedPlugin {
    /// Borrow the C-ABI descriptor. Used by shim wrappers to
    /// reach into the vtable pointer.
    ///
    /// # Safety
    ///
    /// The returned reference is valid as long as `self` is alive.
    /// Callers must not retain it past a `Drop` of the underlying
    /// [`PluginManager`].
    pub fn descriptor(&self) -> &AperioPlugin {
        // SAFETY: pointer was returned by aperio_plugin_create + the
        // plugin contract requires it to stay valid until destroy
        // runs. We never write through it after load.
        unsafe { &*self.plugin_ptr }
    }

    /// Read the descriptor's `vtable` pointer for downstream
    /// casting in the shim wrappers (e.g.
    /// `as *const CalendarVtable`).
    pub fn vtable_ptr(&self) -> *mut c_void {
        self.descriptor().vtable
    }

    /// Convenience accessor — same string the manifest carries
    /// in `id`. Returned from the C-ABI descriptor so a buggy
    /// plugin that mismatches its manifest id and runtime id
    /// can still be diagnosed.
    pub fn runtime_id(&self) -> &str {
        let d = self.descriptor();
        // SAFETY: d.id MUST be a NUL-terminated UTF-8 string per the
        // ABI contract. Malformed plugins are caught at load time.
        unsafe { CStr::from_ptr(d.id) }
            .to_str()
            .unwrap_or("<non-utf8 id>")
    }

    /// `true` iff the plugin exported an
    /// `aperio_plugin_interactive_auth` symbol that we cached at
    /// load time. Drives the Settings → Plugins panel's
    /// capability badges.
    pub fn has_interactive_auth(&self) -> bool {
        self.interactive_auth_fn.is_some()
    }

    /// `true` iff the plugin exported an
    /// `aperio_plugin_discover` symbol.
    pub fn has_discover(&self) -> bool {
        self.discover_fn.is_some()
    }

    /// `true` iff the plugin exported an
    /// `aperio_plugin_probe_host_key` symbol.
    pub fn has_probe_host_key(&self) -> bool {
        self.probe_host_key_fn.is_some()
    }

    /// Borrow the in-flight counter. Shim wrappers clone this
    /// at construction time + use it to build an
    /// [`InFlightGuard`] around every FFI dispatch so the
    /// host's unload path can wait for active calls to
    /// drain.
    pub fn in_flight_handle(&self) -> &Arc<AtomicUsize> {
        &self.in_flight
    }
}

/// RAII guard that increments a plugin's in-flight counter on
/// construction and decrements it on Drop. Held across FFI
/// dispatches in the shim wrappers' trait-method bodies so the
/// counter reflects every active call — even ones suspended at
/// an `.await` point waiting on `tokio::task::spawn_blocking`.
///
/// The counter uses `Ordering::SeqCst` for both directions:
/// the unload path needs a deterministic synchronisation
/// edge between "registry dropped its Arcs" and "in_flight ==
/// 0", and SeqCst is the cheapest correct choice on every
/// platform Aperio targets.
pub struct InFlightGuard {
    counter: Arc<AtomicUsize>,
}

impl InFlightGuard {
    /// Bump the counter + take ownership of the handle. The
    /// returned guard's Drop decrements on its way out.
    pub fn enter(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // SAFETY: the destroy_fn was looked up at load time, is
        // part of the still-loaded library, and we're handing it
        // back its own `*mut AperioPlugin`. ABI v2 no longer has
        // a descriptor-level destroy hook to call first — every
        // instance's close_instance already ran when its
        // [`LoadedInstance`] was dropped, and the descriptor
        // itself is teardown-only.
        unsafe { (self.destroy_fn)(self.plugin_ptr) };
        // self.library drops here -> Library::drop() -> dlclose.
    }
}

/// A live instance of a loaded plugin (DESIGN.md §6.4).
///
/// One [`LoadedPlugin`] can back N instances at the same time —
/// e.g. three CalDAV accounts share the same `cal-adapter-caldav`
/// library but each gets its own [`LoadedInstance`] with its own
/// handle. Vtable methods take the handle as their first
/// argument so the plugin can route work to the right per-
/// account state.
///
/// Always wrapped in `Arc` so the shim adapters
/// ([`crate::shim::FfiCalendarAdapter`] etc.) can keep a cheap
/// reference. When the last `Arc` is dropped, the descriptor's
/// `close_instance` hook fires (when present) and the plugin
/// releases its per-account state.
pub struct LoadedInstance {
    /// Keeps the library + descriptor alive while the instance
    /// is open. Drop order: [`Self`] drops first, fires
    /// `close_instance`, then the plugin Arc may go away.
    plugin: Arc<LoadedPlugin>,
    /// Opaque per-account handle from
    /// [`AperioPlugin::open_instance`]. Passed as the first
    /// argument to every vtable method on this instance. May be
    /// NULL for instance-less plugins (open_instance was None).
    handle: *mut c_void,
    /// Cached close hook. None when the descriptor didn't
    /// provide one — in that case [`Drop`] is a no-op.
    close_fn: Option<unsafe extern "C" fn(*mut c_void)>,
}

impl std::fmt::Debug for LoadedInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedInstance")
            .field("plugin_id", &self.plugin.manifest.id)
            .field("handle_addr", &(self.handle as usize))
            .field("has_close_fn", &self.close_fn.is_some())
            .finish()
    }
}

// SAFETY: same story as LoadedPlugin — handle is an opaque
// pointer the plugin promises is thread-safe to use across
// concurrent vtable calls. We never write through it.
unsafe impl Send for LoadedInstance {}
unsafe impl Sync for LoadedInstance {}

impl LoadedInstance {
    /// Opaque handle the plugin's vtable methods expect as their
    /// first argument. The shim wrappers cache this once at
    /// construction time and pass it through every FFI call.
    pub fn handle(&self) -> *mut c_void {
        self.handle
    }

    /// The plugin this instance was opened against. Shim
    /// wrappers go through this to reach the vtable.
    pub fn plugin(&self) -> &Arc<LoadedPlugin> {
        &self.plugin
    }
}

impl Drop for LoadedInstance {
    fn drop(&mut self) {
        if let Some(close) = self.close_fn {
            if !self.handle.is_null() {
                // SAFETY: handle is exactly what open_instance
                // returned to us, the corresponding library is
                // still loaded (we hold an Arc<LoadedPlugin>),
                // and we only close it once per Drop.
                unsafe { close(self.handle) };
            }
        }
    }
}

/// Process-wide registry of loaded plugins. Created once at app
/// start and stored in `Arc<PluginManager>` for sharing across
/// command handlers + background tasks.
pub struct PluginManager {
    inner: RwLock<Inner>,
    /// The running Aperio's version, used for the per-plugin
    /// `min_app_version` gate. Snapshotted at construction time
    /// so every load uses the same string + the manager's tests
    /// can pretend to be on a specific Aperio build.
    app_version: String,
}

#[derive(Default)]
struct Inner {
    /// Loaded plugins keyed by manifest id. Insertion-order
    /// preserved by a `Vec<String>` companion so the Settings
    /// list always shows the bundled plugins first (they're
    /// loaded by the manager before user/, per the order of
    /// `scan_dir` calls).
    plugins: HashMap<String, Arc<LoadedPlugin>>,
    /// Stable insertion order for [`PluginManager::all`].
    order: Vec<String>,
    /// Plugins the user has temporarily disabled via the
    /// Settings → Plugins panel (DESIGN.md §20.10). The cdylib
    /// stays loaded — the runtime gate just hides them from
    /// [`PluginManager::get`] so subsequent
    /// [`AdapterRegistry`]-side lookups behave as if the plugin
    /// id weren't installed at all. Re-enabling lifts the gate
    /// without re-running dlopen.
    disabled: HashSet<String>,
    /// Plugin directories `scan_dir` tried to load but
    /// couldn't (ABI mismatch, app-too-old, malformed
    /// manifest, dlopen failure …). The Settings panel
    /// reads this so the user sees WHY a community plugin
    /// they installed isn't appearing in the loaded list —
    /// otherwise a stale plugin after an Aperio update just
    /// silently disappears. Cleared by [`Self::clear_failed_loads`]
    /// when a directory has been re-installed via the §20.7
    /// installer.
    failed_loads: Vec<FailedPluginLoad>,
}

/// Metadata for a plugin directory the manager refused to
/// load. Returned from [`PluginManager::failed_loads`] so the
/// Settings panel can render a clear "this plugin couldn't be
/// loaded because …" row.
#[derive(Debug, Clone)]
pub struct FailedPluginLoad {
    /// The directory we tried to load from. Useful as a UI
    /// anchor + for the uninstall command path (community
    /// plugins live under `<data_dir>/plugins/user/<id>/`).
    pub plugin_dir: PathBuf,
    /// Parsed manifest if it got that far. `None` when the
    /// failure was at read / parse time (missing plugin.json,
    /// malformed JSON, missing required field).
    pub manifest: Option<PluginManifest>,
    /// User-facing categorisation. Drives the per-row hint
    /// the UI renders (ABI mismatch → "outdated plugin";
    /// AppTooOld → "update Aperio"; etc.).
    pub reason: FailedLoadReason,
    /// Underlying error message — surfaced to the user as
    /// the detail line so a plugin author / advanced user
    /// can diagnose. Always populated.
    pub error_message: String,
}

/// Coarse-grained reason a plugin failed to load. The
/// manager translates `PluginError` variants into one of
/// these so the UI can branch on intent rather than parsing
/// error strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailedLoadReason {
    /// The manifest's `abi_version` doesn't match the host's
    /// [`crate::ABI_VERSION`]. The plugin needs to be
    /// rebuilt against the running Aperio's ABI, or the
    /// user needs to upgrade/downgrade Aperio.
    AbiMismatch { host: u32, plugin: u32 },
    /// `min_app_version` is newer than the running Aperio.
    /// User should update Aperio.
    AppTooOld { required: String, running: String },
    /// plugin.json is missing, malformed, or has an unknown
    /// `plugin_type` tag (forward-compat). The UI surfaces
    /// this as "this plugin's manifest is invalid".
    ManifestInvalid,
    /// dlopen / LoadLibrary failed at runtime — typically a
    /// corrupt cdylib, wrong architecture, or a missing
    /// system dependency.
    LibraryLoad,
    /// Anything the categoriser couldn't bucket cleanly.
    /// UI falls back to showing the bare error message.
    Other,
}

impl FailedLoadReason {
    /// Bucket a [`PluginError`] into a UI-friendly reason.
    pub fn from_error(err: &PluginError) -> Self {
        match err {
            PluginError::AbiMismatch { host, plugin } => Self::AbiMismatch {
                host: *host,
                plugin: *plugin,
            },
            PluginError::AppTooOld { required, running } => Self::AppTooOld {
                required: required.clone(),
                running: running.clone(),
            },
            PluginError::Io(_) | PluginError::Semver { .. } => Self::ManifestInvalid,
            PluginError::Manifest(msg) => {
                // PluginError::Manifest is overloaded: it
                // carries both "your JSON is broken" and the
                // post-parse "dlopen(...) failed" messages
                // (the manager's load_from_dir wraps libloading
                // errors as Manifest). The prefix lets us
                // disambiguate without a wider error-enum
                // refactor — if the message names a dlopen
                // call site we bucket it as LibraryLoad,
                // otherwise as ManifestInvalid.
                if msg.starts_with("dlopen(") || msg.starts_with("LoadLibrary") {
                    Self::LibraryLoad
                } else {
                    Self::ManifestInvalid
                }
            }
            PluginError::InstanceOpen { .. } => Self::Other,
        }
    }
}

impl PluginManager {
    /// Create an empty manager. The host calls
    /// [`Self::scan_dir`] one or more times to populate it.
    /// `app_version` is the running build's version
    /// (typically `env!("CARGO_PKG_VERSION")`) — used to check
    /// each manifest's `min_app_version` at load time.
    pub fn new(app_version: impl Into<String>) -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
            app_version: app_version.into(),
        }
    }

    /// Walk `dir` looking for plugin subdirectories. Every
    /// immediate child directory that contains a `plugin.json`
    /// is treated as a plugin; missing manifests + parse errors
    /// + ABI mismatches are logged AND recorded into the
    /// manager's failed-loads list so the Settings panel can
    /// surface them — one bad plugin must NEVER prevent the
    /// rest from loading, but it must ALSO not disappear
    /// silently from the user's view.
    ///
    /// Missing `dir` is not an error: typical first-launch
    /// behaviour is that `plugins/user/` doesn't exist yet.
    pub fn scan_dir(&self, dir: impl AsRef<Path>) -> Vec<PluginError> {
        let dir = dir.as_ref();

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Common case on first run. Not an error.
                return Vec::new();
            }
            Err(err) => {
                return vec![PluginError::Io(format!(
                    "scan_dir({}): {err}",
                    dir.display()
                ))];
            }
        };

        // Every immediate child directory is a candidate plugin.
        let plugin_dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        if plugin_dirs.is_empty() {
            return Vec::new();
        }

        // Load in parallel. Each plugin's manifest parse + `dlopen` +
        // `create()` + symbol resolution is independent work; only the
        // final register (`insert`) and the failed-loads push take the
        // manager's `RwLock`, which serialises them. On a cold launch
        // this overlaps the per-library page-in instead of paying it for
        // all ~14 bundled plugins strictly back-to-back. A bounded pool
        // of scoped threads (≤ CPU count) pulls dirs off a shared index,
        // so we don't oversubscribe the disk with one thread per plugin.
        let next = AtomicUsize::new(0);
        let workers = std::cmp::min(
            plugin_dirs.len(),
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
        );
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let mut errs: Vec<PluginError> = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(path) = plugin_dirs.get(i) else {
                                break;
                            };
                            // Parse the manifest separately so the failure
                            // record carries the name + version even when a
                            // later step (dlopen, descriptor checks) failed.
                            let manifest_path = path.join(MANIFEST_FILENAME);
                            let parsed = PluginManifest::read_from(&manifest_path).ok();
                            if let Err(err) = self.load_from_dir(path) {
                                warn!(plugin_dir = %path.display(), ?err, "plugin load failed");
                                let reason = FailedLoadReason::from_error(&err);
                                let error_message = err.to_string();
                                self.inner
                                    .write()
                                    .expect("manager poisoned")
                                    .failed_loads
                                    .push(FailedPluginLoad {
                                        plugin_dir: path.clone(),
                                        manifest: parsed,
                                        reason,
                                        error_message,
                                    });
                                errs.push(err);
                            }
                        }
                        errs
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    }

    /// Snapshot of every plugin directory the manager
    /// refused to load since startup. The Settings → Plugins
    /// panel reads this to render the "Konnten nicht
    /// geladen werden"-section so the user knows WHY a
    /// previously-working community plugin isn't appearing
    /// after an Aperio update (most commonly: ABI mismatch
    /// because the host bumped ABI_VERSION and the plugin
    /// needs to be rebuilt).
    pub fn failed_loads(&self) -> Vec<FailedPluginLoad> {
        self.inner
            .read()
            .expect("manager poisoned")
            .failed_loads
            .clone()
    }

    /// Drop the failure record for `plugin_dir` from the
    /// manager's list. Called by `install_plugin_archive`
    /// after a successful install — once we have the new
    /// version on disk, the old failure is no longer
    /// actionable.
    pub fn clear_failed_load(&self, plugin_dir: &Path) {
        let mut inner = self.inner.write().expect("manager poisoned");
        inner.failed_loads.retain(|f| f.plugin_dir != plugin_dir);
    }

    /// Load one plugin from `plugin_dir`. The directory MUST
    /// contain a `plugin.json` and a platform-appropriate shared
    /// library at the canonical filename
    /// (`<id-or-name>.{dll,dylib,so}`).
    ///
    /// Marked `pub` so the §20.7 community-plugin installer
    /// (drag-and-drop) can drive a single-plugin load directly,
    /// without going through a full `scan_dir` of the user
    /// directory.
    pub fn load_from_dir(&self, plugin_dir: impl AsRef<Path>) -> PluginResult<()> {
        let plugin_dir = plugin_dir.as_ref();
        let manifest_path = plugin_dir.join(MANIFEST_FILENAME);
        let manifest = PluginManifest::read_from(&manifest_path)?;
        manifest.compatible_with(&self.app_version)?;
        // Unknown plugin types parse fine (forward-compat) but we
        // can't actually dispatch to them — skip the load + leave
        // a note so the Settings UI can show "installed, but this
        // Aperio doesn't know how to use it".
        if !manifest.plugin_type.is_known() {
            return Err(PluginError::Manifest(format!(
                "plugin {} declares unknown type {:?}; ignored on this build",
                manifest.id, manifest.plugin_type
            )));
        }

        let lib_path = locate_library(plugin_dir, &manifest)?;
        // SAFETY: libloading::Library::new is unsafe because the
        // library's constructor (DllMain on Windows, _init on
        // unixes) runs arbitrary code. We accept that — the
        // user has agreed via the §20.7 install dialog that they
        // trust the plugin author.
        let library = unsafe { Library::new(&lib_path) }.map_err(|err| {
            PluginError::Manifest(format!("dlopen({}): {err}", lib_path.display()))
        })?;

        let plugin_ptr = unsafe {
            let create: libloading::Symbol<AperioPluginCreateFn> =
                library.get(SYMBOL_CREATE).map_err(|err| {
                    PluginError::Manifest(format!(
                        "missing `{}` symbol in {}: {err}",
                        std::str::from_utf8(SYMBOL_CREATE).unwrap_or("?"),
                        lib_path.display()
                    ))
                })?;
            create()
        };
        if plugin_ptr.is_null() {
            return Err(PluginError::Manifest(format!(
                "{} aperio_plugin_create returned NULL",
                manifest.id
            )));
        }

        // Wire the plugin's isolated `tracing` global into the host
        // log. Each dlopen'd `.so` links its own `tracing` dispatcher,
        // so without this its adapter logs vanish. Optional symbol:
        // plugins built before this ABI addition simply don't forward,
        // and on the static-link (mobile) path the cdylib shell isn't
        // loaded at all. Best-effort — a missing symbol must not block
        // loading an otherwise healthy plugin.
        unsafe {
            if let Ok(set_log) = library.get::<AperioPluginSetLogFn>(SYMBOL_SET_LOG) {
                set_log(forward_plugin_log);
            }
        }

        // Hand over the plugin→host channel the same way. Optional for the
        // same reasons: a plugin that never reports anything does not export
        // it, and on the static-link path the cdylib shell is not loaded at
        // all. See `crate::host_channel` for what it carries and why the
        // account is named by an opaque token rather than by the plugin.
        unsafe {
            if let Ok(set_channel) =
                library.get::<AperioPluginSetHostChannelFn>(SYMBOL_SET_HOST_CHANNEL)
            {
                set_channel(crate::host_channel::forward_host_event);
            }
        }

        // Look up the destructor up-front so the LoadedPlugin's
        // Drop impl can call it without fallible lookup logic.
        // We move the resolved fn pointer into the struct; the
        // library handle keeps the pointed-at code alive.
        let destroy_fn = unsafe {
            let sym: libloading::Symbol<AperioPluginDestroyFn> =
                library.get(SYMBOL_DESTROY).map_err(|err| {
                    PluginError::Manifest(format!(
                        "missing `{}` symbol in {}: {err}",
                        std::str::from_utf8(SYMBOL_DESTROY).unwrap_or("?"),
                        lib_path.display()
                    ))
                })?;
            *sym
        };

        // `aperio_plugin_interactive_auth` is optional — most
        // plugins don't need an OAuth dance and leave the symbol
        // unexported. Cache the resolved fn-pointer at load time
        // so the per-call path doesn't have to re-walk the
        // library's symbol table; a libloading::Error here just
        // means the plugin doesn't expose the capability.
        let interactive_auth_fn: Option<InteractiveAuthFn> = unsafe {
            library
                .get::<InteractiveAuthFn>(SYMBOL_INTERACTIVE_AUTH)
                .ok()
                .map(|sym| *sym)
        };

        // `aperio_plugin_discover` is optional too — only adapters
        // that own a discovery protocol (EWS Autodiscover today)
        // export it. Same caching shape as interactive_auth so the
        // per-call path stays a function-pointer dispatch.
        let discover_fn: Option<DiscoverFn> = unsafe {
            library
                .get::<DiscoverFn>(SYMBOL_DISCOVER)
                .ok()
                .map(|sym| *sym)
        };

        // `aperio_plugin_strings` is optional too — only plugins whose
        // translations do not fit a JSON block in the manifest export
        // it. Same caching shape as the other named-symbol hooks.
        let strings_fn: Option<StringsFn> = unsafe {
            library
                .get::<StringsFn>(SYMBOL_STRINGS)
                .ok()
                .map(|sym| *sym)
        };

        // `aperio_plugin_probe_host_key` is optional too — only
        // adapters wrapping a TOFU transport (SFTP today) export
        // it. Same caching shape as the other named-symbol hooks.
        let probe_host_key_fn: Option<ProbeHostKeyFn> = unsafe {
            library
                .get::<ProbeHostKeyFn>(SYMBOL_PROBE_HOST_KEY)
                .ok()
                .map(|sym| *sym)
        };

        // ABI cross-check between the manifest's claim + the
        // descriptor's claim. They MUST match — a divergence
        // would mean either the plugin's build hooked up the
        // wrong header version, or someone hand-edited
        // plugin.json. Either way, refuse.
        let descriptor = unsafe { &*plugin_ptr };
        check_abi_version(descriptor.abi_version)?;
        if descriptor.abi_version != manifest.abi_version {
            return Err(PluginError::Manifest(format!(
                "{} manifest abi_version={} but descriptor abi_version={}",
                manifest.id, manifest.abi_version, descriptor.abi_version
            )));
        }
        // Compare descriptor id ↔ manifest id. Buggy plugins
        // that mismatch them get caught here rather than at the
        // first call.
        let runtime_id = unsafe { cstr_to_string(descriptor.id) };
        if runtime_id != manifest.id {
            // SAFETY: we own the descriptor right now; the
            // descriptor has no per-process state to release
            // (open/close are per-instance in v2). We just call
            // aperio_plugin_destroy to release the descriptor
            // itself before propagating the load failure.
            unsafe {
                let destroy_sym: libloading::Symbol<AperioPluginDestroyFn> =
                    library.get(SYMBOL_DESTROY).expect("looked up above");
                destroy_sym(plugin_ptr);
            }
            return Err(PluginError::Manifest(format!(
                "manifest id {:?} doesn't match plugin descriptor id {:?}",
                manifest.id, runtime_id
            )));
        }

        // What the manifest promises must be in the vtable the library
        // actually shipped. Catching it here names the plugin; catching it
        // later names nothing, because the surface is simply absent.
        crate::vtables::check_declared_surfaces(&manifest, descriptor.vtable)?;

        // Note: ABI v2 no longer fires a per-load `init` here.
        // Per-account state belongs in `open_instance`, which
        // the host calls once per registered account from the
        // registry layer.

        let loaded = LoadedPlugin {
            manifest,
            plugin_ptr,
            destroy_fn,
            interactive_auth_fn,
            discover_fn,
            probe_host_key_fn,
            strings_fn,
            strings_cache: RwLock::new(HashMap::new()),
            in_flight: Arc::new(AtomicUsize::new(0)),
            library: Some(library),
        };
        let id = loaded.manifest.id.clone();
        info!(plugin_id = %id, "plugin loaded");
        self.insert(id, Arc::new(loaded))
    }

    /// Register a statically-linked plugin (DESIGN.md §20.6 /
    /// the mobile build path). The descriptor + destroy fn
    /// come straight from the plugin crate that's part of the
    /// host binary — no `dlopen`, no [`Library`] to retain.
    ///
    /// Register a statically-linked plugin with NO optional hooks — the common
    /// case (most adapters need only lifecycle + `open_instance`).
    /// Thin wrapper over [`PluginManager::register_static_with_hooks`].
    pub fn register_static(
        &self,
        manifest: PluginManifest,
        descriptor: *mut AperioPlugin,
        destroy_fn: AperioPluginDestroyFn,
    ) -> PluginResult<()> {
        self.register_static_with_hooks(manifest, descriptor, destroy_fn, StaticHooks::default())
    }

    /// Register a statically-linked plugin, carrying its optional auth hooks
    /// (`interactive_auth_fn` / `discover_fn` / `probe_host_key_fn`). Unlike the
    /// dlopen path (which resolves these by symbol name from the loaded
    /// `Library`), a static consumer hands the crate-mangled typed twin
    /// (`<plugin>::__aperio_*_impl`) straight through; `None` for hooks the
    /// adapter doesn't expose. This is what lets OAuth (`interactive_auth`),
    /// Autodiscover (`discover`) and TOFU (`probe_host_key`) adapters work when
    /// statically embedded (e.g. on mobile), with no behaviour change for the
    /// many adapters that pass `None`.
    pub fn register_static_with_hooks(
        &self,
        manifest: PluginManifest,
        descriptor: *mut AperioPlugin,
        destroy_fn: AperioPluginDestroyFn,
        hooks: StaticHooks,
    ) -> PluginResult<()> {
        let StaticHooks {
            interactive_auth_fn,
            discover_fn,
            probe_host_key_fn,
            strings_fn,
        } = hooks;
        manifest.compatible_with(&self.app_version)?;
        if descriptor.is_null() {
            return Err(PluginError::Manifest(format!(
                "static plugin {} descriptor was NULL",
                manifest.id
            )));
        }
        let loaded = LoadedPlugin {
            manifest,
            plugin_ptr: descriptor,
            destroy_fn,
            interactive_auth_fn,
            discover_fn,
            probe_host_key_fn,
            strings_fn,
            strings_cache: RwLock::new(HashMap::new()),
            in_flight: Arc::new(AtomicUsize::new(0)),
            library: None,
        };
        // Same gate the dlopen path runs: what the manifest promises has to be
        // in the vtable. A statically embedded plugin is built from the same
        // source as its manifest, so this fires on a genuine authoring slip —
        // which is exactly when it is worth hearing about. Asked of the built
        // `LoadedPlugin` rather than the bare pointer, so the descriptor read
        // goes through the type that owns the invariant.
        crate::vtables::check_declared_surfaces(&loaded.manifest, loaded.vtable_ptr())?;
        let id = loaded.manifest.id.clone();
        info!(plugin_id = %id, "static plugin registered");
        self.insert(id, Arc::new(loaded))
    }

    /// This plugin's strings for `lang`, resolved once and cached.
    ///
    /// The manifest's catalogue is the base; anything an
    /// `aperio_plugin_strings` export answers is merged OVER it, so a plugin
    /// holding its translations in some other format can override key by key
    /// without restating the ones it is happy with.
    ///
    /// Cached per language, which is what keeps the escape hatch honest: the
    /// FFI call happens the first time a language is asked for, and every label
    /// after that is a map read. A plugin that exports nothing — the ordinary
    /// case — never calls across the boundary at all.
    pub fn strings_for(plugin: &LoadedPlugin, lang: &str) -> Arc<StringCatalogue> {
        let key = lang.to_ascii_lowercase();
        if let Some(hit) = plugin
            .strings_cache
            .read()
            .expect("strings cache poisoned")
            .get(&key)
        {
            return Arc::clone(hit);
        }
        let mut catalogue = plugin.manifest.strings.clone();
        if let Some(strings_fn) = plugin.strings_fn {
            match Self::ask_plugin_for_strings(strings_fn, &key) {
                Ok(extra) => {
                    let slot = catalogue.0.entry(key.clone()).or_default();
                    for (k, v) in extra {
                        slot.insert(k, v);
                    }
                }
                Err(err) => {
                    // A plugin that cannot answer is not a plugin that cannot
                    // work: its manifest strings, and failing those the verbatim
                    // labels, still render. Worth a line, not an interruption.
                    warn!(
                        plugin_id = %plugin.manifest.id,
                        lang = %key,
                        %err,
                        "plugin's strings export failed; falling back to its manifest",
                    );
                }
            }
        }
        let resolved = Arc::new(catalogue);
        plugin
            .strings_cache
            .write()
            .expect("strings cache poisoned")
            .insert(key, Arc::clone(&resolved));
        resolved
    }

    /// One synchronous call across the boundary for one language.
    fn ask_plugin_for_strings(
        strings_fn: StringsFn,
        lang: &str,
    ) -> Result<std::collections::BTreeMap<String, String>, String> {
        let args = format!(r#"{{"lang":"{lang}"}}"#).into_bytes();
        // SAFETY: `args` is a Vec<u8> we own for the duration of the
        // synchronous call, and `strings_fn` came out of the same library the
        // LoadedPlugin holds open.
        let result = unsafe { strings_fn(args.as_ptr(), args.len()) };
        let bytes = unsafe { result.payload.as_slice().to_vec() };
        let status = result.status;
        let mut payload = result.payload;
        unsafe { payload.free_in_place() };
        if status != PLUGIN_OK {
            return Err(format!("status {status}"));
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    /// Open a per-account instance of `plugin` with the supplied
    /// JSON config. Calls the descriptor's `open_instance` hook
    /// and wraps the returned handle in a [`LoadedInstance`].
    ///
    /// When the descriptor doesn't expose `open_instance` (rare —
    /// process-global plugins like notification channels), a
    /// NULL-handle instance is returned so the host can still
    /// call vtable methods uniformly.
    pub fn open_instance(
        &self,
        plugin: Arc<LoadedPlugin>,
        config_json: &str,
    ) -> PluginResult<Arc<LoadedInstance>> {
        // Snapshot the hooks up-front so we can release the
        // borrow on `plugin` before the eventual `move` into
        // LoadedInstance below.
        let (open_hook, close_fn) = {
            let descriptor = plugin.descriptor();
            (descriptor.open_instance, descriptor.close_instance)
        };
        let Some(open) = open_hook else {
            // Process-global plugin — no per-instance handle.
            return Ok(Arc::new(LoadedInstance {
                plugin,
                handle: std::ptr::null_mut(),
                close_fn: None,
            }));
        };
        let c_config = CString::new(config_json)
            .map_err(|e| PluginError::Manifest(format!("config_json contains NUL byte: {e}")))?;
        // SAFETY: the plugin contract says open_instance accepts a
        // NUL-terminated UTF-8 pointer. We own the CString for the
        // duration of the call.
        let mut result: OpenInstanceResult = unsafe { open(c_config.as_ptr()) };
        // Always drain the error buffer (even on success — a
        // misbehaving plugin might write to it) so we don't leak.
        let error_message = if !result.error.is_empty() {
            // SAFETY: error.data + error.len describe a plugin-owned
            // byte buffer valid until free_in_place runs.
            let bytes = unsafe { result.error.as_slice() };
            String::from_utf8_lossy(bytes).into_owned()
        } else {
            String::new()
        };
        unsafe { result.error.free_in_place() };
        if result.status != PLUGIN_OK {
            return Err(PluginError::InstanceOpen {
                status: result.status,
                message: if error_message.is_empty() {
                    format!("plugin returned status {}", result.status)
                } else {
                    error_message
                },
            });
        }
        if result.instance.is_null() {
            return Err(PluginError::InstanceOpen {
                status: result.status,
                message: "open_instance returned NULL handle with OK status".to_string(),
            });
        }
        Ok(Arc::new(LoadedInstance {
            plugin,
            handle: result.instance,
            close_fn,
        }))
    }

    fn insert(&self, id: String, loaded: Arc<LoadedPlugin>) -> PluginResult<()> {
        let mut inner = self.inner.write().expect("manager poisoned");
        if inner.plugins.contains_key(&id) {
            return Err(PluginError::Manifest(format!("duplicate plugin id {id}")));
        }
        inner.order.push(id.clone());
        inner.plugins.insert(id, loaded);
        Ok(())
    }

    /// Look up an enabled plugin by manifest id. Returns `None`
    /// for plugins that didn't get loaded (the §20.8 "Plugin
    /// fehlt" trigger) AND for loaded-but-disabled plugins
    /// (DESIGN.md §20.10 "Deaktivieren"). The two cases share
    /// the same call site by design — once a plugin is gated
    /// off the rest of the host treats it the same as if the
    /// id were never installed.
    ///
    /// Use [`Self::get_including_disabled`] when the caller
    /// genuinely needs to see disabled plugins too (the
    /// Settings panel's enable/disable toggle is the obvious
    /// case).
    pub fn get(&self, id: &str) -> Option<Arc<LoadedPlugin>> {
        let inner = self.inner.read().expect("manager poisoned");
        if inner.disabled.contains(id) {
            return None;
        }
        inner.plugins.get(id).cloned()
    }

    /// The loaded plugin serving `adapter_kind`, if one does.
    ///
    /// This replaces the kind→plugin table the host used to carry. The mapping
    /// is now a *fact about which plugins are loaded* rather than a constant in
    /// the core: each plugin declares the kind it serves in its manifest, and
    /// an adapter Aperio has never seen resolves exactly like one it ships.
    ///
    /// Disabled plugins are excluded, matching [`Self::get`] — an account whose
    /// plugin the user switched off must read as "plugin missing" rather than
    /// silently keep working.
    ///
    /// Two plugins claiming the same kind would be an install-time conflict;
    /// the first match wins and the situation is left for the plugin panel to
    /// surface, since refusing to resolve either would take the user's working
    /// accounts down over someone else's packaging mistake.
    pub fn plugin_for_adapter_kind(&self, adapter_kind: &str) -> Option<Arc<LoadedPlugin>> {
        let inner = self.inner.read().expect("manager poisoned");
        inner
            .plugins
            .values()
            .find(|p| {
                p.manifest.adapter_kind.as_deref() == Some(adapter_kind)
                    && !inner.disabled.contains(&p.manifest.id)
            })
            .cloned()
    }

    /// Every account-bearing adapter a currently loaded plugin serves, so a
    /// frontend can offer exactly the adapters this build actually has —
    /// rather than a list written into the UI, which is what it used to be.
    ///
    /// The plugin's own `name` rides along as the fallback label: the app has
    /// translations for the adapters it ships, and a third-party plugin's own
    /// name beats a missing-key marker.
    pub fn adapter_kinds(&self) -> Vec<AdapterKindInfo> {
        let inner = self.inner.read().expect("manager poisoned");
        let mut kinds: Vec<AdapterKindInfo> = inner
            .plugins
            .values()
            .filter(|p| !inner.disabled.contains(&p.manifest.id))
            .filter_map(|p| {
                p.manifest.adapter_kind.clone().map(|kind| AdapterKindInfo {
                    kind,
                    name: p.manifest.name.clone(),
                    plugin_id: p.manifest.id.clone(),
                    owns_containers: p.manifest.has_data_family(),
                    declares_account_schema: p.manifest.account.is_some(),
                    declares_oauth: p
                        .manifest
                        .account
                        .as_ref()
                        .is_some_and(|a| a.oauth.is_some()),
                    // Decided here rather than in two frontends, because it is
                    // read off the capability list and the frontends must not
                    // grow one. `holds_data` is deliberately "anything but
                    // sync" rather than `has_data_family()`: a meeting provider
                    // has no calendars and still needs an account.
                    holds_data: p
                        .manifest
                        .capabilities
                        .iter()
                        .any(|c| *c != crate::capability::Capability::Sync),
                    can_sync: p
                        .manifest
                        .capabilities
                        .contains(&crate::capability::Capability::Sync),
                })
            })
            .collect();
        kinds.sort_by(|a, b| a.kind.cmp(&b.kind));
        kinds.dedup_by(|a, b| a.kind == b.kind);
        kinds
    }

    /// Same as [`Self::get`] but ignores the disabled-flag
    /// gate. The Settings → Plugins panel uses this so the
    /// toggle that re-enables a plugin can read it back. The
    /// host registry uses it from the re-enable path to find
    /// the descriptor of a plugin it's about to start serving
    /// again.
    pub fn get_including_disabled(&self, id: &str) -> Option<Arc<LoadedPlugin>> {
        self.inner
            .read()
            .expect("manager poisoned")
            .plugins
            .get(id)
            .cloned()
    }

    /// All loaded plugins in load order — including disabled
    /// ones. The Settings → Plugins panel renders this list
    /// directly, paired with [`Self::is_enabled`] per row to
    /// render the toggle state.
    pub fn all(&self) -> Vec<Arc<LoadedPlugin>> {
        let inner = self.inner.read().expect("manager poisoned");
        inner
            .order
            .iter()
            .filter_map(|id| inner.plugins.get(id).cloned())
            .collect()
    }

    /// `true` iff the plugin id is loaded AND not disabled.
    /// `false` for both "not installed" and "installed but
    /// disabled" — same semantics as [`Self::get`].
    pub fn is_enabled(&self, id: &str) -> bool {
        let inner = self.inner.read().expect("manager poisoned");
        inner.plugins.contains_key(id) && !inner.disabled.contains(id)
    }

    /// Flip the disabled flag for `id`. Returns `true` iff the
    /// state changed (the caller can use this to decide
    /// whether to re-register affected accounts). A no-op
    /// against an unknown plugin id silently does nothing —
    /// the persistence layer (user_prefs) may carry a flag
    /// for a plugin the user uninstalled, and we shouldn't
    /// trip on that.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> bool {
        let mut inner = self.inner.write().expect("manager poisoned");
        if !inner.plugins.contains_key(id) {
            return false;
        }
        if enabled {
            inner.disabled.remove(id)
        } else {
            inner.disabled.insert(id.to_string())
        }
    }

    /// All plugins of a given type. The host registry cutover
    /// calls this once per call to build its per-type collections.
    pub fn by_type(&self, plugin_type: &PluginType) -> Vec<Arc<LoadedPlugin>> {
        self.all()
            .into_iter()
            .filter(|p| &p.manifest.plugin_type == plugin_type)
            .collect()
    }

    /// Tear a loaded plugin out of the manager so its
    /// underlying [`libloading::Library`] can `dlclose`. The
    /// caller MUST have dropped every shim's
    /// `Arc<LoadedInstance>` referencing this id before
    /// calling — the manager checks the plugin's
    /// [`LoadedPlugin::in_flight_handle`] counter and refuses
    /// with [`UnloadError::StillReferenced`] if any FFI call
    /// is currently in flight.
    ///
    /// Why `in_flight` rather than `Arc::strong_count`: the
    /// shim wrappers clone the LoadedPlugin Arc as a
    /// side-effect of construction + every per-call clone,
    /// so the strong count is noisy. The dedicated counter
    /// bumps only inside the shim's trait-method scope (via
    /// [`InFlightGuard::enter`]) — it goes to 0 exactly when
    /// no FFI dispatch is active, regardless of how many
    /// idle shim Arcs the caller forgot to drop.
    ///
    /// Typical sequence the host follows for an in-place
    /// upgrade:
    ///   1. `set_enabled(id, false)` — block new
    ///      `get(id)` lookups so no fresh registrations or
    ///      vtable dispatches start.
    ///   2. Walk the registry, unregister every account using
    ///      this plugin. This drops the registry's shim Arcs;
    ///      the FfiCalendarAdapter / FfiSyncAdapter / … each
    ///      hold an `Arc<LoadedInstance>` that prevented the
    ///      LoadedPlugin Arc from dropping. After
    ///      unregistration, only in-flight calls (which take
    ///      their own short-lived clone) keep references
    ///      alive.
    ///   3. Wait for the in-flight counter to drain to 0
    ///      (the host's async retry loop polls
    ///      [`Self::unload_plugin`] with a short sleep between
    ///      attempts).
    ///   4. Once unload_plugin returns Ok: the plugin Arcs
    ///      drop, the Library handle drops, `dlclose` runs.
    ///
    /// The `disabled` flag is cleared by this call — once the
    /// plugin is gone, a subsequent reload via
    /// `load_from_dir` starts with a fresh enabled state.
    pub fn unload_plugin(&self, id: &str) -> Result<(), UnloadError> {
        let mut inner = self.inner.write().expect("manager poisoned");
        let Some(plugin) = inner.plugins.get(id).cloned() else {
            return Err(UnloadError::NotLoaded(id.to_string()));
        };
        let in_flight = plugin.in_flight.load(Ordering::SeqCst);
        if in_flight > 0 {
            // Keep the plugin in the map; the caller polls +
            // retries.
            return Err(UnloadError::StillReferenced {
                id: id.to_string(),
                in_flight,
            });
        }
        // No active calls — safe to drop. Yank from the
        // order vec + the disabled set; remove the plugin
        // entry. The cloned `plugin` Arc above is the last
        // reference once `inner.plugins.remove(id)` returns
        // (the registry has unregistered, no in-flight
        // calls), so dropping it triggers Library::drop ->
        // dlclose.
        inner.order.retain(|other| other != id);
        inner.disabled.remove(id);
        inner.plugins.remove(id);
        drop(inner);
        drop(plugin);
        Ok(())
    }

    /// Test-only: inject a synthetic LoadedPlugin under the
    /// given id. The plugin is a no-op stub (no library, no
    /// instance hooks) — enough to exercise the disabled-flag
    /// gate paths in `get` / `is_enabled` / `set_enabled`
    /// without standing up the full FFI machinery the shim
    /// tests use.
    #[cfg(test)]
    pub(crate) fn insert_stub_for_tests(&self, id: &str, manifest: PluginManifest) {
        unsafe extern "C" fn noop_destroy(_: *mut crate::abi::AperioPlugin) {}
        let id_cstr = std::ffi::CString::new(id).unwrap();
        let name_cstr = std::ffi::CString::new(manifest.name.as_str()).unwrap();
        let version_cstr = std::ffi::CString::new(manifest.version.as_str()).unwrap();
        let type_cstr = std::ffi::CString::new(manifest.plugin_type.as_str()).unwrap();
        let descriptor = Box::new(crate::abi::AperioPlugin {
            abi_version: crate::ABI_VERSION,
            id: id_cstr.into_raw(),
            name: name_cstr.into_raw(),
            version: version_cstr.into_raw(),
            plugin_type: type_cstr.into_raw(),
            open_instance: None,
            close_instance: None,
            vtable: std::ptr::null_mut(),
        });
        let descriptor_ptr = Box::into_raw(descriptor);
        let loaded = LoadedPlugin {
            manifest,
            plugin_ptr: descriptor_ptr,
            destroy_fn: noop_destroy,
            interactive_auth_fn: None,
            discover_fn: None,
            probe_host_key_fn: None,
            strings_fn: None,
            strings_cache: RwLock::new(HashMap::new()),
            in_flight: Arc::new(AtomicUsize::new(0)),
            library: None,
        };
        // Bypass the duplicate-id check the public `insert`
        // does — tests construct each manager fresh, so this
        // is safe + saves the .unwrap() noise.
        let mut inner = self.inner.write().expect("manager poisoned");
        inner.order.push(id.to_string());
        inner.plugins.insert(id.to_string(), Arc::new(loaded));
    }

    /// Number of currently-loaded plugins. Cheap counter used
    /// by the boot-time log line.
    pub fn len(&self) -> usize {
        self.inner.read().expect("manager poisoned").plugins.len()
    }

    /// Convenience predicate.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Run the given plugin's `interactive_auth` hook with the
    /// supplied JSON args + return the resulting credential
    /// blob (typically a TokenSet for OAuth flows). Async
    /// because the OAuth dance can block on the user for up to
    /// 5 minutes — the call runs on the host's blocking pool
    /// via `tokio::task::spawn_blocking` so the reactor stays
    /// free.
    ///
    /// Errors:
    ///   - [`InteractiveAuthError::PluginMissing`] — no plugin
    ///     loaded under the given id.
    ///   - [`InteractiveAuthError::Unsupported`] — plugin
    ///     exists but doesn't export the
    ///     `aperio_plugin_interactive_auth` symbol (Basic-auth /
    ///     token-only adapters that don't need an interactive
    ///     setup step).
    ///   - [`InteractiveAuthError::Plugin`] — the plugin
    ///     returned a non-OK status; the message comes through
    ///     verbatim so the user sees the OAuth error text
    ///     (revoked grant, network problem, browser-closed
    ///     timeout, …).
    pub async fn interactive_auth(
        &self,
        plugin_id: &str,
        args_json: &str,
    ) -> Result<Vec<u8>, InteractiveAuthError> {
        let plugin = self
            .get(plugin_id)
            .ok_or_else(|| InteractiveAuthError::PluginMissing(plugin_id.to_string()))?;
        let interactive_fn = plugin
            .interactive_auth_fn
            .ok_or_else(|| InteractiveAuthError::Unsupported(plugin_id.to_string()))?;
        // Keep the LoadedPlugin Arc alive across the
        // spawn_blocking — `interactive_fn` is a function
        // pointer into the plugin's still-loaded library.
        let plugin_for_drop = plugin.clone();
        let args = args_json.as_bytes().to_vec();
        let join = tokio::task::spawn_blocking(move || {
            // SAFETY: interactive_fn was looked up out of the
            // same library that plugin_for_drop holds open;
            // args is a Vec<u8> we own for the duration of the
            // synchronous call.
            let result = unsafe { interactive_fn(args.as_ptr(), args.len()) };
            // SAFETY: result.payload is owned by the plugin; we
            // copy bytes out + free in place before the buffer
            // goes out of scope on the plugin's side.
            let bytes = unsafe { result.payload.as_slice().to_vec() };
            let status = result.status;
            let mut payload = result.payload;
            unsafe { payload.free_in_place() };
            (status, bytes)
        })
        .await;
        // Hold the plugin Arc until after spawn_blocking returns.
        drop(plugin_for_drop);
        let (status, bytes) =
            join.map_err(|err| InteractiveAuthError::Plugin(format!("plugin task: {err}")))?;
        if status == PLUGIN_CALL_OK {
            Ok(bytes)
        } else {
            let msg = String::from_utf8_lossy(&bytes).into_owned();
            Err(InteractiveAuthError::Plugin(format!(
                "plugin status {status}: {msg}",
            )))
        }
    }
}

/// Reasons [`PluginManager::unload_plugin`] can fail.
#[derive(Debug, thiserror::Error)]
pub enum UnloadError {
    /// No plugin loaded under this id. The caller's
    /// already-loaded check + unload should normally be
    /// guarded against this, but the manager surfaces it
    /// rather than silently no-op'ing to make a logic bug in
    /// the host obvious.
    #[error("plugin not loaded: {0}")]
    NotLoaded(String),

    /// At least one FFI call into this plugin is currently
    /// in flight. The shim wrappers track this via
    /// [`InFlightGuard`]; the unload path polls + retries
    /// until the counter drains.
    #[error("plugin {id} has {in_flight} active call(s); retry after they finish")]
    StillReferenced { id: String, in_flight: usize },
}

/// Reasons [`PluginManager::interactive_auth`] can fail.
#[derive(Debug, thiserror::Error)]
pub enum InteractiveAuthError {
    /// No plugin loaded under this id. Surfaces the same UX as
    /// the §20.8 "Plugin fehlt" affordance.
    #[error("plugin not installed: {0}")]
    PluginMissing(String),

    /// Plugin is loaded but doesn't expose an
    /// `aperio_plugin_interactive_auth` entry point — it's not
    /// an OAuth-style adapter.
    #[error("plugin {0} doesn't support interactive_auth")]
    Unsupported(String),

    /// Plugin returned a non-OK status. Carries the plugin's
    /// own error message so the user sees actionable text.
    #[error("{0}")]
    Plugin(String),
}

impl PluginManager {
    /// Run the given plugin's `discover` hook with the supplied
    /// JSON args + return the resulting endpoints blob (typically
    /// a JSON document the host parses into its UI shape). Async
    /// because service-discovery cascades can take a few seconds
    /// per probe and shouldn't block the reactor — the call runs
    /// on the host's blocking pool via
    /// `tokio::task::spawn_blocking`.
    ///
    /// Errors mirror [`Self::interactive_auth`]:
    ///   - [`DiscoverError::PluginMissing`] — no plugin loaded
    ///     under the given id.
    ///   - [`DiscoverError::Unsupported`] — plugin exists but
    ///     doesn't export the `aperio_plugin_discover` symbol
    ///     (adapters whose endpoints are hard-coded or supplied
    ///     by the user directly).
    ///   - [`DiscoverError::Plugin`] — the plugin returned a
    ///     non-OK status; the message comes through verbatim
    ///     so the user sees actionable text ("no EWS endpoint
    ///     found for hs-anhalt.de", "autodiscover HTTP 401",
    ///     …).
    pub async fn discover(
        &self,
        plugin_id: &str,
        args_json: &str,
    ) -> Result<Vec<u8>, DiscoverError> {
        let plugin = self
            .get(plugin_id)
            .ok_or_else(|| DiscoverError::PluginMissing(plugin_id.to_string()))?;
        let discover_fn = plugin
            .discover_fn
            .ok_or_else(|| DiscoverError::Unsupported(plugin_id.to_string()))?;
        let plugin_for_drop = plugin.clone();
        let args = args_json.as_bytes().to_vec();
        let join = tokio::task::spawn_blocking(move || {
            // SAFETY: discover_fn was looked up out of the same
            // library that plugin_for_drop holds open; args is a
            // Vec<u8> we own for the duration of the synchronous
            // call.
            let result = unsafe { discover_fn(args.as_ptr(), args.len()) };
            // SAFETY: result.payload is owned by the plugin; we
            // copy bytes out + free in place before the buffer
            // goes out of scope on the plugin's side.
            let bytes = unsafe { result.payload.as_slice().to_vec() };
            let status = result.status;
            let mut payload = result.payload;
            unsafe { payload.free_in_place() };
            (status, bytes)
        })
        .await;
        drop(plugin_for_drop);
        let (status, bytes) =
            join.map_err(|err| DiscoverError::Plugin(format!("plugin task: {err}")))?;
        if status == PLUGIN_CALL_OK {
            Ok(bytes)
        } else {
            let msg = String::from_utf8_lossy(&bytes).into_owned();
            Err(DiscoverError::Plugin(format!(
                "plugin status {status}: {msg}",
            )))
        }
    }
}

/// Reasons [`PluginManager::discover`] can fail.
#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    /// No plugin loaded under this id.
    #[error("plugin not installed: {0}")]
    PluginMissing(String),

    /// Plugin is loaded but doesn't expose an
    /// `aperio_plugin_discover` entry point — the host should
    /// fall back to user-supplied endpoint input.
    #[error("plugin {0} doesn't support discover")]
    Unsupported(String),

    /// Plugin returned a non-OK status. Carries the plugin's
    /// own error message so the user sees actionable text
    /// ("no endpoint found for hs-anhalt.de", "HTTP 401", …).
    #[error("{0}")]
    Plugin(String),
}

impl PluginManager {
    /// Run the given plugin's `probe_host_key` hook with the
    /// supplied JSON args + return the resulting fingerprint
    /// blob (typically `{"fingerprint": "SHA256:..."}`). Async
    /// because the probe opens a TCP+TLS/SSH connection and
    /// reads the server's identity — a few hundred ms in the
    /// happy path, several seconds for a dead host.
    ///
    /// Errors mirror [`Self::interactive_auth`] +
    /// [`Self::discover`]:
    ///   - [`ProbeHostKeyError::PluginMissing`] — no plugin
    ///     loaded under the given id.
    ///   - [`ProbeHostKeyError::Unsupported`] — plugin exists
    ///     but doesn't export the
    ///     `aperio_plugin_probe_host_key` symbol (adapters that
    ///     don't wrap a TOFU transport).
    ///   - [`ProbeHostKeyError::Plugin`] — the plugin returned
    ///     a non-OK status; the message comes through verbatim
    ///     so the user sees actionable text ("connection
    ///     refused", "TLS handshake failed", …).
    pub async fn probe_host_key(
        &self,
        plugin_id: &str,
        args_json: &str,
    ) -> Result<Vec<u8>, ProbeHostKeyError> {
        let plugin = self
            .get(plugin_id)
            .ok_or_else(|| ProbeHostKeyError::PluginMissing(plugin_id.to_string()))?;
        let probe_fn = plugin
            .probe_host_key_fn
            .ok_or_else(|| ProbeHostKeyError::Unsupported(plugin_id.to_string()))?;
        let plugin_for_drop = plugin.clone();
        let args = args_json.as_bytes().to_vec();
        let join = tokio::task::spawn_blocking(move || {
            // SAFETY: probe_fn was looked up out of the same
            // library that plugin_for_drop holds open; args is a
            // Vec<u8> we own for the duration of the synchronous
            // call.
            let result = unsafe { probe_fn(args.as_ptr(), args.len()) };
            // SAFETY: result.payload is owned by the plugin; we
            // copy bytes out + free in place before the buffer
            // goes out of scope on the plugin's side.
            let bytes = unsafe { result.payload.as_slice().to_vec() };
            let status = result.status;
            let mut payload = result.payload;
            unsafe { payload.free_in_place() };
            (status, bytes)
        })
        .await;
        drop(plugin_for_drop);
        let (status, bytes) =
            join.map_err(|err| ProbeHostKeyError::Plugin(format!("plugin task: {err}")))?;
        if status == PLUGIN_CALL_OK {
            Ok(bytes)
        } else {
            let msg = String::from_utf8_lossy(&bytes).into_owned();
            Err(ProbeHostKeyError::Plugin(format!(
                "plugin status {status}: {msg}",
            )))
        }
    }
}

/// Reasons [`PluginManager::probe_host_key`] can fail.
#[derive(Debug, thiserror::Error)]
pub enum ProbeHostKeyError {
    /// No plugin loaded under this id.
    #[error("plugin not installed: {0}")]
    PluginMissing(String),

    /// Plugin is loaded but doesn't expose an
    /// `aperio_plugin_probe_host_key` entry point — the host
    /// should fall back to skipping the trust dialog (or
    /// fail-closed, depending on the workflow).
    #[error("plugin {0} doesn't support probe_host_key")]
    Unsupported(String),

    /// Plugin returned a non-OK status. Carries the plugin's
    /// own error message so the user sees actionable text
    /// ("connection refused", "TLS handshake failed", …).
    #[error("{0}")]
    Plugin(String),
}

/// Default subdir under the data dir / app dir where bundled
/// plugins are staged. The release build pipeline copies each
/// adapter's shared library here.
pub const BUNDLED_PLUGINS_DIR: &str = "plugins/bundled";

/// Default subdir for community plugins. The §20.7 installer
/// drops `.aperio` archive contents here.
pub const USER_PLUGINS_DIR: &str = "plugins/user";

/// Resolve the platform-appropriate shared-library path that
/// sits next to `plugin.json` in `plugin_dir`. The lookup is:
///
///   1. Prefer `<dir>/<id>.{dll,dylib,so}` — the canonical name.
///   2. Fall back to the first matching extension in the dir if
///      no exact-id match is present (so plugin authors can pick
///      whatever filename their build system produced).
///
/// Returns `PluginError::Io` if nothing usable is found.
fn locate_library(plugin_dir: &Path, manifest: &PluginManifest) -> PluginResult<PathBuf> {
    let exts: &[&str] = if cfg!(target_os = "windows") {
        &["dll"]
    } else if cfg!(target_os = "macos") {
        &["dylib", "so"]
    } else {
        &["so"]
    };
    let id = manifest.id.as_str();
    let last_segment = id.rsplit('.').next().unwrap_or(id);
    for candidate in [id, last_segment] {
        for ext in exts {
            let p = plugin_dir.join(format!("{candidate}.{ext}"));
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    let mut hits: Vec<PathBuf> = std::fs::read_dir(plugin_dir)
        .map_err(|err| PluginError::Io(format!("scan {}: {err}", plugin_dir.display())))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
                .unwrap_or(false)
        })
        .collect();
    hits.sort();
    hits.into_iter().next().ok_or_else(|| {
        PluginError::Io(format!(
            "no shared library found in {} (looked for {:?})",
            plugin_dir.display(),
            exts
        ))
    })
}

/// Read a C string from a raw pointer into an owned String,
/// substituting "<non-utf8>" on bad bytes. The plugin contract
/// requires UTF-8 + NUL termination; the fallback exists so a
/// buggy plugin can still be diagnosed instead of panicking.
///
/// # Safety
///
/// `ptr` must be a valid NUL-terminated C string (which is what
/// the plugin ABI promises for every `*const c_char` field).
unsafe fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::from("<null>");
    }
    CStr::from_ptr(ptr)
        .to_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|_| String::from("<non-utf8>"))
}

/// Test-only constructors used by the shim crates' unit tests.
#[doc(hidden)]
#[cfg(test)]
pub mod test_support {
    use super::{LoadedInstance, LoadedPlugin, PluginManifest};
    use crate::abi::{AperioPlugin, AperioPluginDestroyFn};
    use std::os::raw::c_void;
    use std::sync::Arc;

    /// Build a LoadedPlugin from a manually-constructed
    /// descriptor + destructor pair. Used by the shim tests to
    /// stand up a fake plugin without dlopen-ing anything; the
    /// caller is responsible for any leak / cleanup since the
    /// destructor is a no-op in the typical test case.
    pub fn loaded_plugin_for_tests(
        manifest: PluginManifest,
        descriptor: *mut AperioPlugin,
        destroy_fn: AperioPluginDestroyFn,
    ) -> LoadedPlugin {
        LoadedPlugin {
            manifest,
            plugin_ptr: descriptor,
            destroy_fn,
            interactive_auth_fn: None,
            discover_fn: None,
            probe_host_key_fn: None,
            strings_fn: None,
            strings_cache: std::sync::RwLock::new(std::collections::HashMap::new()),
            in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            library: None,
        }
    }

    /// Stand up a LoadedInstance with the given handle (often
    /// NULL for tests that don't exercise per-instance state).
    /// No close_fn is wired so Drop is a no-op.
    pub fn loaded_instance_for_tests(
        plugin: Arc<LoadedPlugin>,
        handle: *mut c_void,
    ) -> Arc<LoadedInstance> {
        Arc::new(LoadedInstance {
            plugin,
            handle,
            close_fn: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// scan_dir on a non-existent path is silent — first-run
    /// behaviour where `plugins/user/` hasn't been created yet.
    #[test]
    fn scan_dir_missing_path_returns_no_errors() {
        let mgr = PluginManager::new("0.1.0");
        let errors = mgr.scan_dir("/this/path/does/not/exist");
        assert!(errors.is_empty(), "missing dir is not an error");
        assert!(mgr.is_empty());
    }

    /// scan_dir on an empty directory loads nothing and reports
    /// no errors.
    #[test]
    fn scan_dir_empty_dir_loads_nothing() {
        let mgr = PluginManager::new("0.1.0");
        let tmp = tempdir().expect("tempdir");
        let errors = mgr.scan_dir(tmp.path());
        assert!(errors.is_empty());
        assert!(mgr.is_empty());
    }

    /// A directory without a plugin.json is treated as a load
    /// failure for that subdirectory but doesn't tank the
    /// scan — the returned error vec just collects them.
    #[test]
    fn scan_dir_missing_manifest_yields_per_dir_error() {
        let mgr = PluginManager::new("0.1.0");
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("bogus")).expect("mkdir");
        let errors = mgr.scan_dir(tmp.path());
        assert_eq!(errors.len(), 1, "one bad subdir, one error");
        match &errors[0] {
            PluginError::Io(msg) | PluginError::Manifest(msg) => {
                assert!(!msg.is_empty());
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    /// get() on an unknown id returns None.
    #[test]
    fn get_unknown_id_returns_none() {
        let mgr = PluginManager::new("0.1.0");
        assert!(mgr.get("nope").is_none());
    }

    /// by_type() on an empty manager returns an empty Vec.
    #[test]
    fn by_type_empty_when_no_plugins() {
        let mgr = PluginManager::new("0.1.0");
        assert!(mgr.by_type(&PluginType::Adapter).is_empty());
        assert!(mgr.by_type(&PluginType::Notification).is_empty());
    }

    /// A `strings` export that answers German only, and overrides one key the
    /// manifest also has.
    unsafe extern "C" fn stub_strings(
        args_ptr: *const u8,
        args_len: usize,
    ) -> crate::ffi::PluginCallResult {
        let bytes = unsafe { std::slice::from_raw_parts(args_ptr, args_len) };
        let lang: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        let payload: Vec<u8> = if lang["lang"] == "de" {
            br#"{"join":"AUS DEM EXPORT","only_export":"nur hier"}"#.to_vec()
        } else {
            b"{}".to_vec()
        };
        // Leak it: the host copies the bytes out and then calls `free`, which
        // for a test buffer is a no-op — the same contract a real plugin
        // upholds with its own allocator.
        let boxed = payload.into_boxed_slice();
        let len = boxed.len();
        let data = Box::into_raw(boxed) as *mut u8;
        unsafe extern "C" fn noop_free(_: *mut u8, _: usize) {}
        crate::ffi::PluginCallResult {
            status: PLUGIN_OK,
            payload: crate::ffi::PluginBytes {
                data,
                len,
                free: Some(noop_free),
            },
        }
    }

    #[test]
    fn the_export_wins_over_the_manifest_key_by_key_and_is_asked_once() {
        // The escape hatch's whole contract: a plugin holding its translations
        // elsewhere overrides what it wants and inherits the rest, and the
        // crossing happens once per language so a label never costs an FFI call
        // on a repaint.
        let mut manifest = stub_manifest("com.example.strings");
        manifest.strings = serde_json::from_str(
            r#"{"de": {"join": "aus dem Manifest", "only_manifest": "bleibt"},
                "en": {"join": "from the manifest"}}"#,
        )
        .unwrap();
        unsafe extern "C" fn noop_destroy(_: *mut AperioPlugin) {}
        let descriptor = Box::into_raw(Box::new(crate::abi::AperioPlugin {
            abi_version: crate::ABI_VERSION,
            id: CString::new("com.example.strings").unwrap().into_raw(),
            name: CString::new("Strings").unwrap().into_raw(),
            version: CString::new("0.1.0").unwrap().into_raw(),
            plugin_type: CString::new("adapter").unwrap().into_raw(),
            open_instance: None,
            close_instance: None,
            vtable: std::ptr::null_mut(),
        }));
        let mut plugin = test_support::loaded_plugin_for_tests(manifest, descriptor, noop_destroy);
        plugin.strings_fn = Some(stub_strings);

        let de = PluginManager::strings_for(&plugin, "de");
        assert_eq!(de.lookup("join", "de"), Some("AUS DEM EXPORT"));
        assert_eq!(de.lookup("only_export", "de"), Some("nur hier"));
        // Not restated by the export, so the manifest still answers.
        assert_eq!(de.lookup("only_manifest", "de"), Some("bleibt"));

        // Cached: the same Arc comes back, so nothing crossed the boundary the
        // second time.
        let again = PluginManager::strings_for(&plugin, "de");
        assert!(Arc::ptr_eq(&de, &again));

        // A language the export has nothing for degrades to the manifest.
        let en = PluginManager::strings_for(&plugin, "en");
        assert_eq!(en.lookup("join", "en"), Some("from the manifest"));
    }

    fn stub_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: id.to_string(),
            name: "Stub".to_string(),
            version: "0.1.0".to_string(),
            plugin_type: PluginType::Adapter,
            capabilities: vec![crate::Capability::Calendar],
            abi_version: crate::ABI_VERSION,
            min_app_version: "0.1.0".to_string(),
            author: None,
            description: None,
            signed: false,
            recurrence: Default::default(),
            tasks: Default::default(),
            account: None,
            adapter_kind: None,
            strings: Default::default(),
        }
    }

    #[test]
    fn newly_loaded_plugin_is_enabled() {
        let mgr = PluginManager::new("0.1.0");
        mgr.insert_stub_for_tests("test.cal", stub_manifest("test.cal"));
        assert!(mgr.is_enabled("test.cal"));
        assert!(mgr.get("test.cal").is_some());
    }

    #[test]
    fn disabled_plugin_is_hidden_from_get_and_is_enabled() {
        let mgr = PluginManager::new("0.1.0");
        mgr.insert_stub_for_tests("test.cal", stub_manifest("test.cal"));
        let changed = mgr.set_enabled("test.cal", false);
        assert!(changed, "first disable should flip the state");
        assert!(!mgr.is_enabled("test.cal"));
        assert!(mgr.get("test.cal").is_none());
        // get_including_disabled still surfaces the LoadedPlugin
        // so the Settings panel can render the toggle.
        assert!(mgr.get_including_disabled("test.cal").is_some());
    }

    #[test]
    fn re_enabling_a_disabled_plugin_restores_get() {
        let mgr = PluginManager::new("0.1.0");
        mgr.insert_stub_for_tests("test.cal", stub_manifest("test.cal"));
        mgr.set_enabled("test.cal", false);
        let changed = mgr.set_enabled("test.cal", true);
        assert!(changed);
        assert!(mgr.is_enabled("test.cal"));
        assert!(mgr.get("test.cal").is_some());
    }

    #[test]
    fn set_enabled_is_idempotent() {
        let mgr = PluginManager::new("0.1.0");
        mgr.insert_stub_for_tests("test.cal", stub_manifest("test.cal"));
        // Re-enabling an already-enabled plugin reports no
        // state change so callers don't trigger spurious
        // re-registrations.
        assert!(!mgr.set_enabled("test.cal", true));
        mgr.set_enabled("test.cal", false);
        assert!(!mgr.set_enabled("test.cal", false));
    }

    #[test]
    fn set_enabled_on_unknown_plugin_is_a_noop() {
        let mgr = PluginManager::new("0.1.0");
        // The persistence layer (user_prefs) might carry a flag
        // for a plugin the user uninstalled. The gate must
        // silently ignore the call rather than tracking
        // disabled state for ghost ids.
        assert!(!mgr.set_enabled("ghost", false));
        assert!(!mgr.is_enabled("ghost"));
    }

    #[test]
    fn all_includes_disabled_plugins_in_load_order() {
        let mgr = PluginManager::new("0.1.0");
        mgr.insert_stub_for_tests("test.a", stub_manifest("test.a"));
        mgr.insert_stub_for_tests("test.b", stub_manifest("test.b"));
        mgr.set_enabled("test.a", false);
        let ids: Vec<_> = mgr.all().iter().map(|p| p.manifest.id.clone()).collect();
        assert_eq!(ids, vec!["test.a", "test.b"]);
    }

    #[test]
    fn unload_plugin_drops_unique_arc_and_clears_state() {
        let mgr = PluginManager::new("0.1.0");
        mgr.insert_stub_for_tests("test.cal", stub_manifest("test.cal"));
        mgr.set_enabled("test.cal", false);
        // No outside Arc clones live in this test, so the
        // strong count is just the inner map's.
        mgr.unload_plugin("test.cal")
            .expect("unload should succeed");
        assert!(mgr.get_including_disabled("test.cal").is_none());
        assert!(!mgr.is_enabled("test.cal"));
        assert!(mgr.all().is_empty());
        // Re-disabling should now report no-op because the
        // plugin id is gone — same semantics as set_enabled
        // on an unknown plugin.
        assert!(!mgr.set_enabled("test.cal", false));
    }

    #[test]
    fn unload_plugin_refuses_when_in_flight_call_active() {
        let mgr = PluginManager::new("0.1.0");
        mgr.insert_stub_for_tests("test.cal", stub_manifest("test.cal"));
        // Simulate an in-flight FFI call by entering the
        // guard manually. Holding the guard keeps the
        // counter at 1; dropping it should let unload
        // succeed.
        let plugin = mgr
            .get_including_disabled("test.cal")
            .expect("just inserted");
        let guard = InFlightGuard::enter(Arc::clone(plugin.in_flight_handle()));
        let err = mgr.unload_plugin("test.cal").expect_err("should refuse");
        match err {
            UnloadError::StillReferenced { id, in_flight } => {
                assert_eq!(id, "test.cal");
                assert_eq!(in_flight, 1);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        // Plugin must still be in the manager so the host can
        // recover (poll + retry once the in-flight call
        // returns).
        assert!(mgr.get_including_disabled("test.cal").is_some());
        drop(guard);
        drop(plugin);
        mgr.unload_plugin("test.cal").expect("unload after retry");
    }

    #[test]
    fn in_flight_guard_decrements_on_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let _g1 = InFlightGuard::enter(Arc::clone(&counter));
            assert_eq!(counter.load(Ordering::SeqCst), 1);
            {
                let _g2 = InFlightGuard::enter(Arc::clone(&counter));
                assert_eq!(counter.load(Ordering::SeqCst), 2);
            }
            assert_eq!(counter.load(Ordering::SeqCst), 1);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn scan_dir_records_failed_loads_for_broken_plugin() {
        let mgr = PluginManager::new("0.1.0");
        let tmp = tempdir().expect("tempdir");
        // Two subdirs: one with a malformed plugin.json
        // (parse failure), one with a wrong-ABI manifest.
        // Both should land in failed_loads with the right
        // reason.
        let broken_json = tmp.path().join("com.example.broken");
        std::fs::create_dir(&broken_json).unwrap();
        std::fs::write(broken_json.join("plugin.json"), b"{ not json").unwrap();

        let wrong_abi = tmp.path().join("com.example.wrong-abi");
        std::fs::create_dir(&wrong_abi).unwrap();
        std::fs::write(
            wrong_abi.join("plugin.json"),
            br#"{
                "id": "com.example.wrong-abi",
                "name": "Wrong ABI",
                "version": "1.0.0",
                "plugin_type": "adapter",
                "capabilities": ["calendar"],
                "abi_version": 999,
                "min_app_version": "0.1.0"
            }"#,
        )
        .unwrap();

        let errors = mgr.scan_dir(tmp.path());
        assert_eq!(errors.len(), 2, "both bad subdirs should fail");

        let failed = mgr.failed_loads();
        assert_eq!(failed.len(), 2);

        let by_dir: HashMap<_, _> = failed.iter().map(|f| (f.plugin_dir.clone(), f)).collect();

        let broken = by_dir.get(&broken_json).expect("broken_json recorded");
        assert!(broken.manifest.is_none(), "manifest didn't parse");
        assert_eq!(broken.reason, FailedLoadReason::ManifestInvalid);

        let abi = by_dir.get(&wrong_abi).expect("wrong_abi recorded");
        assert!(
            abi.manifest.is_some(),
            "manifest parsed even though ABI mismatched"
        );
        assert_eq!(
            abi.reason,
            FailedLoadReason::AbiMismatch {
                host: crate::ABI_VERSION,
                plugin: 999,
            },
        );
    }

    #[test]
    fn clear_failed_load_drops_only_the_matching_entry() {
        let mgr = PluginManager::new("0.1.0");
        let tmp = tempdir().expect("tempdir");
        for name in ["com.example.a", "com.example.b"] {
            let dir = tmp.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("plugin.json"), b"{ bad").unwrap();
        }
        mgr.scan_dir(tmp.path());
        assert_eq!(mgr.failed_loads().len(), 2);
        mgr.clear_failed_load(&tmp.path().join("com.example.a"));
        let remaining = mgr.failed_loads();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].plugin_dir.ends_with("com.example.b"));
    }

    #[test]
    fn unload_plugin_on_unknown_id_yields_not_loaded() {
        let mgr = PluginManager::new("0.1.0");
        let err = mgr.unload_plugin("ghost").expect_err("ghost id");
        assert!(matches!(err, UnloadError::NotLoaded(id) if id == "ghost"));
    }
}
