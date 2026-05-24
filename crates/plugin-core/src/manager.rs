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
//!      called to produce the [`AperioPlugin`] descriptor.
//!   5. Plugin's optional `init` lifecycle hook is fired with the
//!      caller-supplied per-plugin config JSON (host pulls these
//!      from user_prefs by plugin id; the resolver hooks in P6
//!      do that wiring).
//!   6. The plugin lives in the manager until the process exits.
//!      [`PluginManager::drop`] fires `destroy` on each plugin
//!      then `aperio_plugin_destroy`, then drops the loaded
//!      library.
//!
//! ## Thread safety
//!
//! `Arc<PluginManager>` is the canonical sharing shape — multiple
//! Tauri command handlers hold the same Arc concurrently. Lookups
//! ([`PluginManager::get`], [`PluginManager::all`]) take an
//! `RwLock` read guard; loads + unloads take a write guard.
//! Plugin vtable invocations themselves don't need to touch the
//! manager's lock — the host snapshots the [`LoadedPlugin`] Arc
//! once and calls into the vtable directly.
//!
//! ## Static-plugins build
//!
//! The `static-plugins` feature flag (DESIGN.md §20.6, P5 of this
//! phase plan) flips the manager into a path where bundled
//! adapters are registered via a compile-time list instead of
//! `dlopen`. P1 lays the API surface for that ([`PluginManager::register_static`])
//! but the actual feature gate + the static-list ingestion fly in
//! during P5 when the mobile build comes online.

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use libloading::Library;
use tracing::{info, warn};

use crate::abi::{
    AperioPlugin, AperioPluginCreateFn, AperioPluginDestroyFn, SYMBOL_CREATE,
    SYMBOL_DESTROY,
};
use crate::error::{PluginError, PluginResult};
use crate::manifest::{PluginManifest, MANIFEST_FILENAME};
use crate::plugin_type::PluginType;
use crate::version::check_abi_version;

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

    /// The dlopen'd library. Drop order: `destroy` (fired in
    /// the manager's destructor) → `aperio_plugin_destroy` →
    /// `library.drop()` (which calls `dlclose`). Static-plugin
    /// builds (P5) set this to `None` so dropping doesn't try
    /// to unload anything.
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
    pub fn vtable_ptr(&self) -> *mut std::os::raw::c_void {
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
        // Tear-down sequence per the C header: AperioPlugin.destroy
        // (if set), then aperio_plugin_destroy. The library handle
        // is dropped last via the auto-drop of `self.library`.
        let descriptor = self.descriptor();
        if let Some(d) = descriptor.destroy {
            // SAFETY: pointer was returned by the plugin's create()
            // and we've not yet destroyed it. The plugin contract
            // says destroy() must tolerate being called even if
            // init() was never invoked.
            unsafe { d() };
        }
        // SAFETY: same — the destroy_fn was looked up at load time,
        // is part of the still-loaded library, and we're handing it
        // back its own `*mut AperioPlugin`.
        unsafe { (self.destroy_fn)(self.plugin_ptr) };
        // self.library drops here -> Library::drop() -> dlclose.
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
    /// Marked `pub` so the Phase P8 community-plugin installer
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
            // SAFETY: we own the descriptor right now and haven't
            // called destroy yet; bail out by calling it manually
            // before returning the error so the plugin can clean
            // up.
            unsafe {
                if let Some(d) = descriptor.destroy {
                    d();
                }
                let destroy_sym: libloading::Symbol<AperioPluginDestroyFn> =
                    library.get(SYMBOL_DESTROY).expect("looked up above");
                destroy_sym(plugin_ptr);
            }
            return Err(PluginError::Manifest(format!(
                "manifest id {:?} doesn't match plugin descriptor id {:?}",
                manifest.id, runtime_id
            )));
        }

        // Fire the optional init hook. We pass an empty string
        // for now; the per-plugin config-json plumbing lives
        // outside this crate (in src-tauri's user_prefs lookup
        // by plugin id) and lands as part of P6 / the registry
        // cutover.
        if let Some(init) = descriptor.init {
            let empty: &[u8] = b"\0";
            // SAFETY: init's signature accepts a `*const c_char`
            // that points at a NUL-terminated string. An empty
            // C-string "\0" is the most conservative no-config
            // call we can make.
            let rc = unsafe { init(empty.as_ptr() as *const c_char) };
            if rc != crate::abi::PLUGIN_OK {
                // Tear back down before returning.
                unsafe {
                    if let Some(d) = descriptor.destroy {
                        d();
                    }
                    let destroy_sym: libloading::Symbol<AperioPluginDestroyFn> =
                        library.get(SYMBOL_DESTROY).expect("looked up above");
                    destroy_sym(plugin_ptr);
                }
                return Err(PluginError::Manifest(format!(
                    "{} init returned status {}",
                    manifest.id, rc
                )));
            }
        }

        let loaded = LoadedPlugin {
            manifest,
            plugin_ptr,
            destroy_fn,
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
    /// API surface lands now so the static-plugins feature
    /// flag in P5 has a target to call into; the gate itself
    /// flips in that phase.
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
            library: None,
        };
        let id = loaded.manifest.id.clone();
        info!(plugin_id = %id, "static plugin registered");
        self.insert(id, Arc::new(loaded))
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

    /// All plugins of a given type. The src-tauri registry
    /// cutover (P6) calls this once per call to build its
    /// per-type collections.
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
}

/// Default subdir under the data dir / app dir where bundled
/// plugins are staged. The P5 build pipeline copies each
/// adapter's shared library here.
pub const BUNDLED_PLUGINS_DIR: &str = "plugins/bundled";

/// Default subdir for community plugins. The Phase P8 installer
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
    // Try the canonical filename. Try the id (e.g.
    // "com.aperio.cal-adapter-local") first; some build systems
    // produce that verbatim. If that doesn't exist, also try the
    // suffix after the last `.` (so "com.aperio.local" would also
    // match "local.so").
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
    // Last-ditch: any file with the right extension in the
    // directory. Sorted for deterministic behaviour across runs.
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
/// Hidden behind `#[doc(hidden)]` and `cfg(test)` so it never
/// shows up in release builds or in the rustdoc surface.
#[doc(hidden)]
#[cfg(test)]
pub mod test_support {
    use super::{LoadedPlugin, PluginManifest};
    use crate::abi::{AperioPlugin, AperioPluginDestroyFn};

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
            library: None,
        }
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
        // The error is the manifest IO failure from
        // PluginManifest::read_from — message contains "plugin.json".
        match &errors[0] {
            PluginError::Io(msg) | PluginError::Manifest(msg) => {
                // Either Io (file not found) or Manifest (json) is fine —
                // we just want to confirm it surfaced as a load error.
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
