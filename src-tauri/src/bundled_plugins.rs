//! Load the bundled cal-adapter and sync-adapter plugins at
//! host startup (DESIGN.md §20.5 + §22.2).
//!
//! ## Path resolution
//!
//! The release zip lays plugins out next to the binary:
//!
//! ```text
//! Aperio-VERSION-PLATFORM/
//! ├── Aperio.exe
//! └── plugins/
//!     └── bundled/
//!         └── com.aperio.cal-adapter-caldav/
//!             ├── plugin.json
//!             └── com.aperio.cal-adapter-caldav.dll
//! ```
//!
//! At runtime [`build_manager`] looks `plugins/bundled/` up
//! relative to [`std::env::current_exe`], which gives the same
//! layout for the in-tree dev build (`target/<profile>/`) and
//! the release artifact.
//!
//! ## Dev workflow
//!
//! Two commands, in this order:
//!
//! 1. `cargo build --workspace` — produces every plugin cdylib.
//! 2. `cargo xtask stage-plugins` — copies each one, with its
//!    `plugin.json`, into `target/<profile>/plugins/bundled/<id>/`.
//!
//! A subsequent `cargo run -p aperio` scans that directory and loads each
//! plugin via `libloading`. `cargo tauri dev` and `cargo tauri build` run both
//! steps themselves, through `tauri.conf.json`'s `beforeDevCommand` /
//! `beforeBuildCommand`.
//!
//! Running `cargo run -p aperio` without step 2 leaves the bundled-plugins dir
//! empty: aperio starts, and every external calendar/sync/vc adapter surfaces
//! as "plugin missing".
//!
//! ### Why it takes a second command
//!
//! The plugin crates and `aperio` have NO cargo-dep edges between them, by
//! design — adding them would link twelve copies of `#[no_mangle]
//! aperio_plugin_create` into the host binary and collide. So cargo schedules
//! them in parallel, and nothing during the build can be sure the cdylibs
//! exist yet.
//!
//! Staging used to live in `aperio`'s own `build.rs`, which meant it ran at
//! some non-deterministic point in the middle of that and frequently found
//! nothing. The workaround was to build the app TWICE — the second pass
//! existing only so `cargo:rerun-if-changed` would fire once the libraries had
//! landed — and a cdylib still missing was a `cargo:warning` the build
//! ignored. Copying after the build removes the race instead of racing it, and
//! a missing cdylib is now an error. See `xtask/src/main.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use plugin_core::{PluginManager, BUNDLED_PLUGINS_DIR, USER_PLUGINS_DIR};
use tracing::{info, warn};

/// Build a [`PluginManager`] populated by `dlopen`ing every
/// shared library found under `<binary-dir>/plugins/bundled/`
/// (read-only, ships with the app) AND
/// `<data_dir>/plugins/user/` (user-writable, populated by the
/// §20.7 `.aperio` installer).
///
/// Per-plugin load errors are logged but never fail the startup
/// — a broken plugin must NEVER keep the rest of the app from
/// coming up.
pub fn build_manager(app_version: &str, data_dir: &Path) -> Arc<PluginManager> {
    let manager = PluginManager::new(app_version);

    // Bundled scan first — these are guaranteed to be present
    // on every install and shouldn't be overridden by a
    // community plugin with the same id (the duplicate-id
    // check in `PluginManager::insert` ensures the user-side
    // load fails, leaving the bundled copy active).
    match bundled_dir() {
        Some(bundled) => {
            info!(
                path = %bundled.display(),
                "scanning bundled plugins directory",
            );
            let errors = manager.scan_dir(&bundled);
            for err in errors {
                warn!(?err, "bundled plugin failed to load");
            }
        }
        None => {
            warn!(
                "couldn't resolve `plugins/bundled/` relative to current_exe(); \
                 no bundled plugins will load",
            );
        }
    }

    // User scan: `<data_dir>/plugins/user/`. Missing dir is
    // first-run-normal and PluginManager::scan_dir handles
    // that silently.
    let user_dir = user_plugins_dir(data_dir);
    info!(
        path = %user_dir.display(),
        "scanning user plugins directory",
    );
    let errors = manager.scan_dir(&user_dir);
    for err in errors {
        warn!(?err, "user plugin failed to load");
    }

    info!(plugin_count = manager.len(), "all plugins loaded");
    Arc::new(manager)
}

/// `<data_dir>/plugins/user/` — where the §20.7 installer
/// extracts `.aperio` archives.
pub fn user_plugins_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(USER_PLUGINS_DIR)
}

/// Compute the bundled-plugins directory path:
/// `<dir-of-current-exe>/plugins/bundled/`. Returns `None` only
/// when [`std::env::current_exe`] fails (very rare — would mean
/// the OS lost track of the process binary).
fn bundled_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    Some(exe_dir.join(BUNDLED_PLUGINS_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// scan_dir against an empty directory returns 0 plugins +
    /// no errors. Mirrors the "first launch before any plugin
    /// has been staged" state.
    #[test]
    fn scan_empty_dir_loads_zero_plugins() {
        let tmp = TempDir::new().expect("tempdir");
        let bundled = tmp.path().join("plugins").join("bundled");
        fs::create_dir_all(&bundled).expect("mkdir");
        let manager = PluginManager::new("0.1.0");
        let errors = manager.scan_dir(&bundled);
        assert!(errors.is_empty(), "empty dir scan should report no errors");
        assert_eq!(manager.len(), 0);
    }

    /// `bundled_dir()` returns a path under the dir of the
    /// currently-running test binary. The path may or may not
    /// exist depending on whether `cargo build --workspace`
    /// has populated it — but the resolution itself shouldn't
    /// fail.
    #[test]
    fn bundled_dir_resolves_under_current_exe() {
        let dir = bundled_dir().expect("current_exe should resolve");
        assert!(dir.ends_with("plugins/bundled") || dir.ends_with("plugins\\bundled"));
    }

    /// End-to-end dlopen smoke: scan the staged
    /// `target/<profile>/plugins/bundled/` (populated by
    /// `cargo build --workspace` via `build.rs`) and verify the
    /// manager picks up every expected plugin id. Skipped when
    /// the dir is empty so a fresh checkout that only ran
    /// `cargo test -p aperio` still passes — the workspace
    /// build is what populates the dir, and CI scripts run it
    /// before the test step.
    /// Where the workspace build actually staged the plugins, or `None` when it
    /// has not been built yet.
    ///
    /// `bundled_dir()` resolves beside `current_exe`, which in a shipped app is
    /// right. Under `cargo test` the exe is `target/<profile>/deps/…`, while
    /// `cargo xtask stage-plugins` writes to `target/<profile>/plugins/bundled`
    /// — one level further up.
    ///
    /// Getting that arithmetic wrong is not a harmless test-only slip: it makes
    /// every test built on this silently SKIP, which reads as green. The
    /// previous version went up two levels from the bundled dir and landed back
    /// on the path it started from, so it always skipped — including the check
    /// that all 17 plugins load.
    fn staged_plugins_dir() -> Option<std::path::PathBuf> {
        let direct = bundled_dir().expect("current_exe");
        if direct.is_dir() {
            return Some(direct);
        }
        // …/deps/plugins/bundled → …/deps/plugins → …/deps → …/<profile>
        direct
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|profile| profile.join("plugins").join("bundled"))
            .filter(|p| p.is_dir())
    }

    /// A plugin whose manifest says it signs in interactively MUST export the
    /// symbol that does it.
    ///
    /// This is a two-file invariant with nothing holding the halves together:
    /// the handler is declared in the `-plugin` rlib, and the `#[no_mangle]`
    /// export is emitted by the `-cdylib` shell, which has to opt in with
    /// `interactive_auth: yes`. Forget that one line and everything still
    /// compiles, every test passes, the plugin loads — and the first person to
    /// press "Add account" gets "doesn't support interactive auth". That is
    /// exactly how it shipped for Webex.
    ///
    /// The manifest is the right thing to check against, because it is where
    /// the plugin already promises an OAuth flow: an `account.oauth` block IS
    /// the claim that connecting runs an interactive sign-in.
    #[test]
    fn a_plugin_that_declares_oauth_actually_exports_its_auth_entry_point() {
        let Some(scan_dir) = staged_plugins_dir() else {
            eprintln!(
                "skipping: no staged plugins dir — run `cargo build --workspace` and \
                 then `cargo xtask stage-plugins`",
            );
            return;
        };
        let manager = PluginManager::new(env!("CARGO_PKG_VERSION"));
        let errors = manager.scan_dir(&scan_dir);
        assert!(errors.is_empty(), "scan_dir reported {errors:?}");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let mut checked = 0usize;
        for plugin in manager.all() {
            let declares_oauth = plugin
                .manifest
                .account
                .as_ref()
                .is_some_and(|account| account.oauth.is_some());
            if !declares_oauth {
                continue;
            }
            checked += 1;
            let id = plugin.manifest.id.clone();
            // Deliberately malformed arguments: reaching the handler at all is
            // the whole question. A plugin that IS wired rejects them with its
            // own parse error; one that is not answers `Unsupported` before
            // any argument is looked at.
            let outcome = runtime.block_on(manager.interactive_auth(&id, "{}"));
            assert!(
                !matches!(
                    outcome,
                    Err(plugin_core::InteractiveAuthError::Unsupported(_))
                ),
                "{id} declares account.oauth but its cdylib does not export \
                 aperio_plugin_interactive_auth — add `interactive_auth: yes` to its \
                 declare_cdylib_exports!",
            );
        }
        assert!(
            checked > 0,
            "no plugin declared account.oauth, so this guard checked nothing — \
             it has stopped testing what it was written for",
        );
    }

    #[test]
    fn every_staged_plugin_loads() {
        let Some(scan_dir) = staged_plugins_dir() else {
            eprintln!(
                "skipping: no staged plugins dir — run `cargo build --workspace` and \
                 then `cargo xtask stage-plugins`",
            );
            return;
        };

        let manager = PluginManager::new(env!("CARGO_PKG_VERSION"));
        let errors = manager.scan_dir(&scan_dir);
        assert!(
            errors.is_empty(),
            "scan_dir against staged plugins should report no errors, got {errors:?}",
        );

        // What is on disk is what `cargo xtask stage-plugins` put there, and it
        // verifies its own work — that every plugin the workspace declares
        // arrived with both halves, and that nothing of ours is there that the
        // workspace does not declare. So the question left for this test is the
        // one only the host can answer: can it actually LOAD them.
        //
        // Deliberately no list of ids. There used to be one here and another in
        // `build.rs`, and a third derivation in the release workflow; keeping
        // three copies in step is what put a `17` under a list of fourteen.
        let staged = fs::read_dir(&scan_dir)
            .expect("the staged dir was found above")
            .flatten()
            .filter(|e| e.path().is_dir())
            .count();
        assert_eq!(
            manager.len(),
            staged,
            "every staged plugin should load: {} of {staged} did",
            manager.len(),
        );

        // Named rather than counted, because the number is the thing that
        // changes when an adapter is retired on purpose. The built-in store is
        // not among them — it is linked, not loaded — so the anchor is the
        // adapter every desktop install has a use for.
        assert!(
            manager.get("com.aperio.cal-adapter-caldav").is_some(),
            "CalDAV did not load from {}; staged: {:?}",
            scan_dir.display(),
            manager
                .all()
                .iter()
                .map(|p| p.manifest.id.clone())
                .collect::<Vec<_>>(),
        );
    }
    /// Every real adapter survives being packed into a `.aperio` archive and
    /// installed back out of it.
    ///
    /// The archive format had a complete reader — inspect, install, the
    /// path-traversal guard, the confirmation dialog — and, until the packer
    /// was written, nothing anywhere that produced one. The only `.aperio` file
    /// that had ever existed was a unit-test fixture holding a dummy library.
    ///
    /// So this is the format meeting real plugins for the first time: twelve
    /// adapters, each with a ten-megabyte cdylib and its own manifest, packed
    /// and installed and then LOADED — because an archive that unpacks into a
    /// directory the host cannot dlopen has proved nothing. The first
    /// out-of-tree adapter author should not be the one to find out.
    #[test]
    fn every_staged_plugin_survives_being_packed_and_installed() {
        let Some(scan_dir) = staged_plugins_dir() else {
            eprintln!(
                "skipping: no staged plugins dir — run `cargo build --workspace` and \
                 then `cargo xtask stage-plugins`",
            );
            return;
        };

        let work = TempDir::new().expect("tempdir");
        let archives = work.path().join("archives");
        let installed_root = work.path().join("installed");
        std::fs::create_dir_all(&archives).expect("mkdir");

        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&scan_dir)
            .expect("read staged dir")
            .flatten()
        {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let archive = archives.join(format!("{name}.aperio"));

            let packed = plugin_core::pack_archive(&dir, &archive)
                .unwrap_or_else(|e| panic!("packing {name}: {e}"));
            assert_eq!(packed.id, name, "the staged dir is named for the plugin id");

            // The dialog's preview path, before anything is written.
            let inspected = plugin_core::inspect_archive(&archive)
                .unwrap_or_else(|e| panic!("inspecting {name}: {e}"));
            assert_eq!(inspected.id, packed.id);

            let installed = plugin_core::install_archive(&archive, &installed_root)
                .unwrap_or_else(|e| panic!("installing {name}: {e}"));
            assert_eq!(installed.plugin_dir, installed_root.join(&name));
            ids.push(name);
        }

        // The whole point: what came out of the archives is loadable, by the
        // same manager and the same code path a user's install would take.
        let manager = PluginManager::new(env!("CARGO_PKG_VERSION"));
        for id in &ids {
            manager
                .load_from_dir(installed_root.join(id))
                .unwrap_or_else(|e| {
                    panic!("{id} installed from its archive but will not load: {e}")
                });
        }
        assert_eq!(
            manager.len(),
            ids.len(),
            "every installed plugin should be loaded: {} of {}",
            manager.len(),
            ids.len(),
        );

        // Named, not counted — the number is what changes when an adapter is
        // retired on purpose.
        assert!(
            ids.iter().any(|id| id == "com.aperio.cal-adapter-caldav"),
            "CalDAV was not among the plugins packed: {ids:?}",
        );
    }
}
