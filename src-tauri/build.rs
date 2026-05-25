//! Aperio's build script.
//!
//! Two responsibilities:
//!
//!   1. The usual [`tauri_build::build`] hook that wires up the
//!      Tauri command surface, icon resources, etc.
//!   2. Plugin staging (DESIGN.md §20.6 + §22.2). After `cargo
//!      build` produces the 13 bundled plugins' cdylibs in
//!      `target/<profile>/`, this script copies each one + its
//!      sibling `plugin.json` into
//!      `target/<profile>/plugins/bundled/<plugin-id>/` so the
//!      running aperio binary can `dlopen` them via
//!      [`plugin_core::PluginManager::scan_dir`].
//!
//! ## Workflow
//!
//! The plugin crates are workspace members but NOT cargo deps of
//! `aperio` (that would force the host to link the plugin rlibs,
//! which would collide on `#[no_mangle] aperio_plugin_create`
//! across 13 plugins). Cargo therefore only builds the plugin
//! cdylibs when something else triggers it.
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
use std::path::PathBuf;

/// `(crate-name, plugin-id)` for every bundled plugin. The
/// crate name follows Cargo's snake-case-via-underscore
/// filename convention; the plugin id matches the value the
/// plugin's `declare_lifecycle!` invocation emits + the value
/// in the plugin's `plugin.json` manifest.
const PLUGINS: &[(&str, &str)] = &[
    ("cal-adapter-caldav-plugin", "com.aperio.cal-adapter-caldav"),
    ("cal-adapter-ical-plugin", "com.aperio.cal-adapter-ical"),
    ("cal-adapter-google-plugin", "com.aperio.cal-adapter-google"),
    (
        "cal-adapter-microsoft-graph-plugin",
        "com.aperio.cal-adapter-microsoft-graph",
    ),
    ("cal-adapter-ews-plugin", "com.aperio.cal-adapter-ews"),
    ("cal-adapter-vikunja-plugin", "com.aperio.cal-adapter-vikunja"),
    ("cal-adapter-todoist-plugin", "com.aperio.cal-adapter-todoist"),
    ("sync-adapter-local-plugin", "com.aperio.sync-adapter-local"),
    ("sync-adapter-webdav-plugin", "com.aperio.sync-adapter-webdav"),
    ("sync-adapter-ftp-plugin", "com.aperio.sync-adapter-ftp"),
    ("sync-adapter-sftp-plugin", "com.aperio.sync-adapter-sftp"),
    ("sync-adapter-dropbox-plugin", "com.aperio.sync-adapter-dropbox"),
    (
        "sync-adapter-googledrive-plugin",
        "com.aperio.sync-adapter-googledrive",
    ),
];

fn main() {
    tauri_build::build();
    stage_bundled_plugins();
}

fn stage_bundled_plugins() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
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

    for (crate_name, plugin_id) in PLUGINS {
        let src_dir = crates_dir.join(crate_name);

        // Cargo emits cdylibs as:
        //   Windows: <name>.dll
        //   macOS:   lib<name>.dylib
        //   Linux:   lib<name>.so
        // where <name> is the crate name with `-` replaced by `_`.
        let crate_name_underscore = crate_name.replace('-', "_");
        let cdylib_src = if cfg!(target_os = "windows") {
            profile_dir.join(format!("{crate_name_underscore}.dll"))
        } else if cfg!(target_os = "macos") {
            profile_dir.join(format!("lib{crate_name_underscore}.dylib"))
        } else {
            profile_dir.join(format!("lib{crate_name_underscore}.so"))
        };

        // Re-run when any of these change:
        //   - the plugin's source / manifest / Cargo.toml
        //   - the cdylib file itself appearing / changing —
        //     critical because cargo builds workspace members
        //     in parallel; this script may run before the
        //     cdylib lands on the first pass + needs to pick
        //     it up on the next build round.
        println!("cargo:rerun-if-changed={}/src", src_dir.display());
        println!(
            "cargo:rerun-if-changed={}/plugin.json",
            src_dir.display(),
        );
        println!(
            "cargo:rerun-if-changed={}/Cargo.toml",
            src_dir.display(),
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

        let manifest_src = src_dir.join("plugin.json");
        let manifest_dst = dst_subdir.join("plugin.json");
        if let Err(err) = fs::copy(&manifest_src, &manifest_dst) {
            println!(
                "cargo:warning=bundled plugin {plugin_id}: copy plugin.json: {err}",
            );
            continue;
        }
    }
}
