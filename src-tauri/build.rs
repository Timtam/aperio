//! Aperio's build script.
//!
//! Two responsibilities:
//!
//!   1. The usual [`tauri_build::build`] hook that wires up the
//!      Tauri command surface, icon resources, etc.
//!   2. Plugin staging (DESIGN.md §20.6 + §22.2). After `cargo
//!      build` produces the 14 bundled plugins' cdylibs in
//!      `target/<profile>/`, this script copies each one + its
//!      sibling `plugin.json` into
//!      `target/<profile>/plugins/bundled/<plugin-id>/` so the
//!      running aperio binary can `dlopen` them via
//!      [`plugin_core::PluginManager::scan_dir`].
//!
//! ## Workflow
//!
//! Each plugin ships as two crates: an rlib `*-plugin` crate (the
//! adapter + crate-mangled descriptor twins, statically linkable
//! on mobile) and a thin `*-cdylib` shell that emits the
//! `#[no_mangle] aperio_plugin_*` C-ABI exports the desktop
//! dlopen loader resolves. Only the cdylib shells produce the
//! loadable libraries staged here. Neither is a cargo dep of
//! `aperio` — depending on the cdylibs would force the host to
//! link 14 copies of `aperio_plugin_create` and collide at link
//! time. Cargo therefore only builds the cdylibs when something
//! else (a `--workspace` build) triggers it.
//!
//! Use `cargo build` (or `cargo build --workspace`) from the
//! workspace root to build everything in one go. Running
//! `cargo build -p aperio` alone produces the host binary but
//! leaves the bundled-plugins dir empty; aperio still starts but
//! no external calendar/sync adapters are available until
//! `cargo build --workspace` populates the cdylibs and this
//! build script reruns to stage them.
//!
//! Missing cdylibs are logged via `cargo:warning` rather than
//! failing the build, so a partial-tree workflow doesn't trap
//! the developer.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// `(cdylib-crate, plugin-crate, plugin-id)` for every bundled
/// plugin. The cdylib crate emits the loadable library — its
/// name, with `-` replaced by `_`, is the cdylib filename stem.
/// The plugin crate owns the `plugin.json` manifest + the source
/// the staging rerun-triggers watch. The plugin id matches the
/// value the plugin's `declare_lifecycle!` invocation emits + the
/// value in the plugin's `plugin.json` manifest.
const PLUGINS: &[(&str, &str, &str)] = &[
    (
        "adapter-caldav-cdylib",
        "adapter-caldav-plugin",
        "com.aperio.cal-adapter-caldav",
    ),
    (
        "adapter-ical-cdylib",
        "adapter-ical-plugin",
        "com.aperio.cal-adapter-ical",
    ),
    (
        "adapter-google-cdylib",
        "adapter-google-plugin",
        "com.aperio.cal-adapter-google",
    ),
    (
        "adapter-microsoft-graph-cdylib",
        "adapter-microsoft-graph-plugin",
        "com.aperio.cal-adapter-microsoft-graph",
    ),
    (
        "adapter-ews-cdylib",
        "adapter-ews-plugin",
        "com.aperio.cal-adapter-ews",
    ),
    (
        "adapter-vikunja-cdylib",
        "adapter-vikunja-plugin",
        "com.aperio.cal-adapter-vikunja",
    ),
    (
        "adapter-todoist-cdylib",
        "adapter-todoist-plugin",
        "com.aperio.cal-adapter-todoist",
    ),
    (
        "adapter-webdav-cdylib",
        "adapter-webdav-plugin",
        "com.aperio.sync-adapter-webdav",
    ),
    (
        "adapter-ftp-cdylib",
        "adapter-ftp-plugin",
        "com.aperio.sync-adapter-ftp",
    ),
    (
        "adapter-sftp-cdylib",
        "adapter-sftp-plugin",
        "com.aperio.sync-adapter-sftp",
    ),
    (
        "adapter-dropbox-cdylib",
        "adapter-dropbox-plugin",
        "com.aperio.sync-adapter-dropbox",
    ),
    (
        "adapter-webex-cdylib",
        "adapter-webex-plugin",
        "com.aperio.vc-adapter-webex",
    ),
];

fn main() {
    tauri_build::build();
    stage_bundled_plugins();
}

/// Delete staged plugins that [`PLUGINS`] no longer lists.
///
/// Staging copies each plugin into `plugins/bundled/<plugin-id>/`, and nothing
/// ever removed one. A target directory therefore accumulated every plugin the
/// workspace had EVER produced: retiring an adapter deleted its crate, its
/// manifest and its registration, and the app went on loading yesterday's
/// cdylib from disk — with a manifest whose kind nothing serves any more.
///
/// It is not hypothetical. Folding Drive into Google and folder sync into the
/// built-in store removed two plugins, and the bundled-plugin test failed on
/// the next run with 14 loaded against 12 expected, purely from leftovers.
///
/// Only directories named like a plugin id we could plausibly have written are
/// touched — `com.aperio.*`. A third-party plugin a developer dropped in by
/// hand is somebody else's, and a build script is the wrong thing to be
/// deleting it.
fn prune_unlisted_plugins(bundled_dir: &Path) {
    let expected: Vec<&str> = PLUGINS.iter().map(|(_, _, id)| *id).collect();
    let entries = match fs::read_dir(bundled_dir) {
        Ok(entries) => entries,
        // Nothing staged yet: the first build in a clean target.
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("com.aperio.") || expected.contains(&name) {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => println!(
                "cargo:warning=removed stale bundled plugin {name}: no longer part of the \
                 workspace",
            ),
            Err(err) => println!(
                "cargo:warning=stale bundled plugin {name} could not be removed from {}: \
                 {err}",
                path.display(),
            ),
        }
    }
}

fn stage_bundled_plugins() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .expect("src-tauri lives one level below the workspace root")
        .to_path_buf();
    let crates_dir = workspace_root.join("crates");

    // OUT_DIR is `<target>/<profile>/build/<crate-hash>/out` —
    // three ancestors up lands on `<target>/<profile>/`, which
    // is also where cargo drops the plugin cdylibs alongside the
    // aperio binary.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has the expected target/<profile>/build/<hash>/out shape")
        .to_path_buf();
    let bundled_dir = profile_dir.join("plugins").join("bundled");

    // Before staging, not after: a plugin that was renamed appears under two
    // ids otherwise, and the old one would be loaded alongside the new for the
    // rest of that build.
    prune_unlisted_plugins(&bundled_dir);

    for (cdylib_crate, plugin_crate, plugin_id) in PLUGINS {
        // plugin.json + the watched source live in the rlib
        // `-plugin` crate; the loadable cdylib is produced by the
        // companion `-cdylib` shell.
        let plugin_src_dir = crates_dir.join(plugin_crate);

        // Cargo emits cdylibs as:
        //   Windows: <name>.dll
        //   macOS:   lib<name>.dylib
        //   Linux:   lib<name>.so
        // where <name> is the cdylib crate name with `-` replaced
        // by `_`.
        let cdylib_name_underscore = cdylib_crate.replace('-', "_");
        let cdylib_src = if cfg!(target_os = "windows") {
            profile_dir.join(format!("{cdylib_name_underscore}.dll"))
        } else if cfg!(target_os = "macos") {
            profile_dir.join(format!("lib{cdylib_name_underscore}.dylib"))
        } else {
            profile_dir.join(format!("lib{cdylib_name_underscore}.so"))
        };

        // Re-run when any of these change:
        //   - the plugin's source / manifest / Cargo.toml
        //   - the cdylib file itself appearing / changing —
        //     critical because cargo builds workspace members
        //     in parallel; this script may run before the
        //     cdylib lands on the first pass + needs to pick
        //     it up on the next build round.
        println!("cargo:rerun-if-changed={}/src", plugin_src_dir.display());
        println!(
            "cargo:rerun-if-changed={}/plugin.json",
            plugin_src_dir.display(),
        );
        println!(
            "cargo:rerun-if-changed={}/Cargo.toml",
            plugin_src_dir.display(),
        );
        println!("cargo:rerun-if-changed={}", cdylib_src.display());

        if !cdylib_src.is_file() {
            println!(
                "cargo:warning=bundled plugin {plugin_id}: cdylib missing at \
                 {} — run `cargo build --workspace` to build it",
                cdylib_src.display(),
            );
            continue;
        }

        let dst_subdir = bundled_dir.join(plugin_id);
        if let Err(err) = fs::create_dir_all(&dst_subdir) {
            println!(
                "cargo:warning=bundled plugin {plugin_id}: mkdir {} failed: {err}",
                dst_subdir.display(),
            );
            continue;
        }

        // Rename the cdylib on copy to match the plugin-id-
        // prefixed canonical filename `plugin_core`'s
        // `locate_library` checks first. Avoids relying on the
        // last-ditch "any file with the right extension"
        // fallback.
        let cdylib_ext = if cfg!(target_os = "windows") {
            "dll"
        } else if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        };
        let cdylib_dst = dst_subdir.join(format!("{plugin_id}.{cdylib_ext}"));
        if let Err(err) = fs::copy(&cdylib_src, &cdylib_dst) {
            println!(
                "cargo:warning=bundled plugin {plugin_id}: copy {} → {}: {err}",
                cdylib_src.display(),
                cdylib_dst.display(),
            );
            continue;
        }

        let manifest_src = plugin_src_dir.join("plugin.json");
        let manifest_dst = dst_subdir.join("plugin.json");
        if let Err(err) = fs::copy(&manifest_src, &manifest_dst) {
            println!("cargo:warning=bundled plugin {plugin_id}: copy plugin.json: {err}",);
            continue;
        }
    }
}
