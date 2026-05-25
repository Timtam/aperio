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
//! `cargo build --workspace` builds every plugin cdylib;
//! `build.rs` stages them into
//! `target/<profile>/plugins/bundled/<id>/`. A subsequent
//! `cargo run -p aperio` scans that directory + loads each
//! plugin via `libloading`.
//!
//! ### The two-build race
//!
//! On a fresh `target/` (e.g. after `cargo clean`), a SINGLE
//! `cargo build --workspace` is not enough. The plugin
//! crates and `aperio` have NO cargo-dep edges between them
//! (by design — adding them would link 17×`#[no_mangle]
//! aperio_plugin_create` into the host binary + collide at
//! link time). Cargo therefore schedules them in parallel.
//! `aperio`'s `build.rs` runs at some non-deterministic
//! point during the workspace build and may see an empty
//! target dir if the cdylibs haven't landed yet. Its
//! `cargo:rerun-if-changed=<cdylib_src>` would re-fire the
//! staging next time, but in a single-invocation build
//! "next time" never comes.
//!
//! The fix is two cargo invocations chained:
//!
//! 1. `cargo build --workspace` — produces every cdylib.
//!    aperio's build.rs may not stage anything this round.
//! 2. `cargo build -p aperio` — cargo sees the cdylibs are
//!    now present where they were absent before;
//!    rerun-if-changed fires; build.rs stages.
//!
//! `cargo tauri dev` + `cargo tauri build` automate this
//! via `tauri.conf.json`'s `beforeDevCommand` /
//! `beforeBuildCommand` (both chain the two-build sequence
//! ahead of the frontend build). On a warm tree both builds
//! collapse to ~1 s of up-to-date checks.
//!
//! Running `cargo run -p aperio` directly (bypassing tauri)
//! ALONE leaves the bundled-plugins dir empty; aperio still
//! starts but every external calendar/sync/vc adapter
//! surfaces as "plugin missing" until the two-build chain
//! has run at least once.

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
    #[test]
    fn scan_bundled_loads_every_expected_plugin_when_staged() {
        // bundled_dir() resolves relative to current_exe(). For
        // a `cargo test -p aperio --lib` run, current_exe is
        // `target/<profile>/deps/aperio-<hash>.exe`, so the
        // bundled dir resolves to `target/<profile>/deps/
        // plugins/bundled/` — which `build.rs` doesn't
        // populate. The real workspace build stages plugins
        // under `target/<profile>/plugins/bundled/` (one level
        // up). Walk both to find whichever exists.
        let direct = bundled_dir().expect("current_exe");
        let parent_alt = direct
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("plugins").join("bundled"));
        let scan_dir = if direct.is_dir() {
            direct
        } else if let Some(p) = parent_alt.filter(|p| p.is_dir()) {
            p
        } else {
            eprintln!(
                "skipping: no staged plugins dir found — run `cargo build --workspace` first",
            );
            return;
        };

        let manager = PluginManager::new(env!("CARGO_PKG_VERSION"));
        let errors = manager.scan_dir(&scan_dir);
        assert!(
            errors.is_empty(),
            "scan_dir against staged plugins should report no errors, got {errors:?}",
        );

        // Every plugin the workspace produces should be present.
        // Mirrors the list in build.rs::PLUGINS.
        for id in [
            "com.aperio.cal-adapter-caldav",
            "com.aperio.cal-adapter-ical",
            "com.aperio.cal-adapter-google",
            "com.aperio.cal-adapter-microsoft-graph",
            "com.aperio.cal-adapter-ews",
            "com.aperio.cal-adapter-vikunja",
            "com.aperio.cal-adapter-todoist",
            "com.aperio.sync-adapter-local",
            "com.aperio.sync-adapter-webdav",
            "com.aperio.sync-adapter-ftp",
            "com.aperio.sync-adapter-sftp",
            "com.aperio.sync-adapter-dropbox",
            "com.aperio.sync-adapter-googledrive",
            "com.aperio.vc-adapter-zoom",
            "com.aperio.vc-adapter-teams",
            "com.aperio.vc-adapter-meet",
            "com.aperio.vc-adapter-webex",
        ] {
            assert!(
                manager.get(id).is_some(),
                "plugin {id} not loaded — check that build.rs staged it",
            );
        }
        assert_eq!(manager.len(), 17);
    }
}
