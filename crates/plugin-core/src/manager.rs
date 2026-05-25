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

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use libloading::Library;
use tracing::{info, warn};

use crate::abi::{
    AperioPlugin, AperioPluginCreateFn, AperioPluginDestroyFn, OpenInstanceResult,
    PLUGIN_OK, SYMBOL_CREATE, SYMBOL_DESTROY,
};
use crate::error::{PluginError, PluginResult};
use crate::ffi::{PluginCallResult, PLUGIN_CALL_OK};
use crate::manifest::{PluginManifest, MANIFEST_FILENAME};
use crate::plugin_type::PluginType;
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
    /// + ABI mismatches are logged and skipped — one bad plugin
    ///   must NEVER prevent the rest from loading.
    ///
    /// Missing `dir` is not an error: typical first-launch
    /// behaviour is that `plugins/user/` doesn't exist yet.
    pub fn scan_dir(&self, dir: impl AsRef<Path>) -> Vec<PluginError> {
        let dir = dir.as_ref();
        let mut errors: Vec<PluginError> = Vec::new();

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Common case on first run. Not an error.
                return errors;
            }
            Err(err) => {
                errors.push(PluginError::Io(format!(
                    "scan_dir({}): {err}",
                    dir.display()
                )));
                return errors;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Err(err) = self.load_from_dir(&path) {
                warn!(plugin_dir = %path.display(), ?err, "plugin load failed");
                errors.push(err);
            }
        }
        errors
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
        let library = unsafe { Library::new(&lib_path) }
            .map_err(|err| PluginError::Manifest(format!(
                "dlopen({}): {err}",
                lib_path.display()
            )))?;

        let plugin_ptr = unsafe {
            let create: libloading::Symbol<AperioPluginCreateFn> = library
                .get(SYMBOL_CREATE)
                .map_err(|err| PluginError::Manifest(format!(
                    "missing `{}` symbol in {}: {err}",
                    std::str::from_utf8(SYMBOL_CREATE).unwrap_or("?"),
                    lib_path.display()
                )))?;
            create()
        };
        if plugin_ptr.is_null() {
            return Err(PluginError::Manifest(format!(
                "{} aperio_plugin_create returned NULL",
                manifest.id
            )));
        }

        // Look up the destructor up-front so the LoadedPlugin's
        // Drop impl can call it without fallible lookup logic.
        // We move the resolved fn pointer into the struct; the
        // library handle keeps the pointed-at code alive.
        let destroy_fn = unsafe {
            let sym: libloading::Symbol<AperioPluginDestroyFn> = library
                .get(SYMBOL_DESTROY)
                .map_err(|err| PluginError::Manifest(format!(
                    "missing `{}` symbol in {}: {err}",
                    std::str::from_utf8(SYMBOL_DESTROY).unwrap_or("?"),
                    lib_path.display()
                )))?;
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
    /// Static-linked plugins don't get an `interactive_auth_fn`
    /// or `discover_fn` — the named-symbol lookup mechanism the
    /// dlopen path uses has no static-link analogue. Plugins
    /// that need either entry point must therefore be dlopen-
    /// loaded (relevant for any OAuth-style or Autodiscover-
    /// style adapter).
    pub fn register_static(
        &self,
        manifest: PluginManifest,
        descriptor: *mut AperioPlugin,
        destroy_fn: AperioPluginDestroyFn,
    ) -> PluginResult<()> {
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
            interactive_auth_fn: None,
            discover_fn: None,
            library: None,
        };
        let id = loaded.manifest.id.clone();
        info!(plugin_id = %id, "static plugin registered");
        self.insert(id, Arc::new(loaded))
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
            .map_err(|e| PluginError::Manifest(format!(
                "config_json contains NUL byte: {e}"
            )))?;
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
                message: "open_instance returned NULL handle with OK status"
                    .to_string(),
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
            return Err(PluginError::Manifest(format!(
                "duplicate plugin id {id}"
            )));
        }
        inner.order.push(id.clone());
        inner.plugins.insert(id, loaded);
        Ok(())
    }

    /// Look up a plugin by manifest id. Returns `None` for any
    /// plugin that didn't get loaded — e.g. an account whose
    /// `adapter_kind` refers to a plugin id Aperio doesn't have
    /// installed (the §20.8 "Plugin fehlt" trigger).
    pub fn get(&self, id: &str) -> Option<Arc<LoadedPlugin>> {
        self.inner
            .read()
            .expect("manager poisoned")
            .plugins
            .get(id)
            .cloned()
    }

    /// All loaded plugins in load order. The Settings → Plugins
    /// panel renders this list directly.
    pub fn all(&self) -> Vec<Arc<LoadedPlugin>> {
        let inner = self.inner.read().expect("manager poisoned");
        inner
            .order
            .iter()
            .filter_map(|id| inner.plugins.get(id).cloned())
            .collect()
    }

    /// All plugins of a given type. The host registry cutover
    /// calls this once per call to build its per-type collections.
    pub fn by_type(&self, plugin_type: &PluginType) -> Vec<Arc<LoadedPlugin>> {
        self.all()
            .into_iter()
            .filter(|p| &p.manifest.plugin_type == plugin_type)
            .collect()
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
        let (status, bytes) = join.map_err(|err| {
            InteractiveAuthError::Plugin(format!("plugin task: {err}"))
        })?;
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
        let (status, bytes) = join.map_err(|err| {
            DiscoverError::Plugin(format!("plugin task: {err}"))
        })?;
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
fn locate_library(
    plugin_dir: &Path,
    manifest: &PluginManifest,
) -> PluginResult<PathBuf> {
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
        .map_err(|err| PluginError::Io(format!(
            "scan {}: {err}",
            plugin_dir.display()
        )))?
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
    hits.into_iter().next().ok_or_else(|| PluginError::Io(format!(
        "no shared library found in {} (looked for {:?})",
        plugin_dir.display(),
        exts
    )))
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
        assert!(mgr.by_type(&PluginType::CalendarAdapter).is_empty());
        assert!(mgr.by_type(&PluginType::SyncAdapter).is_empty());
    }
}
