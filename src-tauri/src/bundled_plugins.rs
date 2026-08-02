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

    /// The release workflow derives "how many plugins should be staged" by
    /// reading `build.rs`, and it has to read the same number this build
    /// stages.
    ///
    /// It is a shell one-liner over a Rust file, which is exactly as fragile as
    /// it sounds — and it broke: `build.rs` grew a helper that filters stale
    /// directories on the `com.aperio.` PREFIX, one string literal in a
    /// function that stages nothing, and the count went up by one. Every
    /// artifact build then failed against a tree that was perfectly fine, and
    /// nothing said so until a thirty-minute CI run came back red.
    ///
    /// So the derivation is pinned here, where `cargo test` finds a mismatch in
    /// seconds. The extraction below mirrors the workflow's: take the
    /// `const PLUGINS` block, count the `"com.aperio.` lines in it.
    #[test]
    fn the_workflow_derives_the_same_plugin_count_this_build_stages() {
        let build_rs = include_str!("../build.rs");
        let table = build_rs
            .split_once("\nconst PLUGINS")
            .expect("build.rs declares a PLUGINS table")
            .1
            .split_once("\n];")
            .expect("the PLUGINS table is terminated")
            .0;
        let derived = table
            .lines()
            .filter(|line| line.contains("\"com.aperio."))
            .count();

        // The entry count, measured a DIFFERENT way — off each tuple's first
        // field, the cdylib crate name. Counting the ids again would just
        // restate the line above and assert nothing.
        let entries = table
            .lines()
            .filter(|line| line.trim_end().ends_with("-cdylib\","))
            .count();

        assert_eq!(
            derived, entries,
            "the workflow's `sed '/^const PLUGINS/,/^];/p' | grep -c '\"com\\.aperio\\.'` \
             sees {derived} plugins but the table has {entries} entries — the \
             artifact build will fail against a healthy tree. Something inside the \
             table carries a `\"com.aperio.` literal that is not an entry's id.",
        );
        assert!(
            entries >= 12,
            "only {entries} plugins in the table; if an adapter was unplugged on \
             purpose, lower this floor deliberately rather than by accident",
        );
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
    /// `build.rs` stages into `target/<profile>/plugins/bundled` — one level
    /// further up.
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
            eprintln!("skipping: no staged plugins dir — run `cargo build --workspace` first");
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
    fn scan_bundled_loads_every_expected_plugin_when_staged() {
        let Some(scan_dir) = staged_plugins_dir() else {
            eprintln!("skipping: no staged plugins dir — run `cargo build --workspace` first");
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
        //
        // The count below is derived from this list rather than written out
        // again. It used to be a separate literal, which is how it came to say
        // 17 while the list said 14: unplugging an adapter edits the list, and
        // a number somewhere underneath it does not follow.
        const EXPECTED: &[&str] = &[
            "com.aperio.cal-adapter-caldav",
            "com.aperio.cal-adapter-ical",
            "com.aperio.cal-adapter-google",
            "com.aperio.cal-adapter-microsoft-graph",
            "com.aperio.cal-adapter-ews",
            "com.aperio.cal-adapter-vikunja",
            "com.aperio.cal-adapter-todoist",
            "com.aperio.sync-adapter-webdav",
            "com.aperio.sync-adapter-ftp",
            "com.aperio.sync-adapter-sftp",
            "com.aperio.sync-adapter-dropbox",
            "com.aperio.vc-adapter-webex",
        ];
        for id in EXPECTED {
            assert!(
                manager.get(id).is_some(),
                "plugin {id} not loaded — check that build.rs staged it",
            );
        }
        assert_eq!(
            manager.len(),
            EXPECTED.len(),
            "a staged plugin nobody expected, or an expected one missing",
        );
    }
}
