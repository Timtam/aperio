//! Aperio's build script.
//!
//! One responsibility: the [`tauri_build::build`] hook that wires up the Tauri
//! command surface, icon resources and the capability files.
//!
//! ## Plugin staging is not here any more
//!
//! It used to be. This script copied each bundled plugin's cdylib and
//! `plugin.json` out of `target/<profile>/` into
//! `target/<profile>/plugins/bundled/<plugin-id>/`, from a table of twelve
//! triples kept by hand.
//!
//! A build script is the wrong place for it, because it runs DURING the build.
//! Cargo compiles workspace members in parallel, so the cdylibs frequently did
//! not exist yet when this script ran; `tauri.conf.json` worked around that by
//! building the app twice, the second pass purely to make this script rerun
//! once the libraries had landed. A cdylib that was still missing produced a
//! `cargo:warning` and the build continued — a release binary that starts and
//! can connect to nothing.
//!
//! It is `cargo xtask stage-plugins` now, run after the workspace build. See
//! `xtask/src/main.rs` for why the plugin list is derived from `cargo metadata`
//! rather than written down.

fn main() {
    tauri_build::build();
}
