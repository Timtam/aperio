//! Every `plugin.json` in the tree survives the loader's own parser.
//!
//! `PluginManifest::from_bytes` is what the host runs against a manifest before
//! it will load the plugin, and it is more than a serde call: it validates the
//! declared `account` block. A manifest that serialises fine but declares, say,
//! a secret field with no slot is rejected there — at load time, on a user's
//! machine, as a plugin that silently fails to appear.
//!
//! Nothing guarded that until now. The bundled-plugin test only covers plugins
//! this build actually stages, so an unbundled one (Zoom today) could carry a
//! broken manifest indefinitely, and the six sync manifests are about to grow
//! `account` blocks written by hand.
//!
//! Deliberately walks the directory rather than listing crates: a manifest
//! added tomorrow is covered without anyone remembering this file.

use std::fs;
use std::path::PathBuf;

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("host-plugins lives under crates/")
        .to_path_buf()
}

#[test]
fn every_plugin_manifest_in_the_tree_parses_and_validates() {
    let crates = crates_dir();
    let mut checked = Vec::new();

    for entry in fs::read_dir(&crates).expect("read crates/") {
        let path = entry.expect("dir entry").path().join("plugin.json");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let manifest = plugin_core::PluginManifest::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("{} is not a loadable manifest: {e}", path.display()));

        // An id is what everything else keys on; an empty one would produce a
        // plugin the host can load but nothing can reference.
        assert!(
            !manifest.id.trim().is_empty(),
            "{} declares an empty id",
            path.display(),
        );
        checked.push(manifest.id);
    }

    // A path bug that found no manifests would otherwise pass silently.
    assert!(
        checked.len() >= 10,
        "only found {} manifests under {} — the walk is probably wrong",
        checked.len(),
        crates.display(),
    );

    checked.sort();
    let before = checked.len();
    checked.dedup();
    assert_eq!(
        checked.len(),
        before,
        "two plugins in the tree share an id; the loader keys on it",
    );
}
