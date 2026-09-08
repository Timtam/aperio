//! A plugin crate names ONE Aperio crate: the SDK.
//!
//! `plugin-sdk` re-exports `plugin_core`, `cal_core`, `sync_core` and
//! `vc_core`, so everything a plugin needs is reachable through a single
//! dependency. That is convenience in this repository and something more once
//! an adapter lives in its own: **every crate an adapter NAMES is a version it
//! has to track.** Four names mean four things to keep in step — by git tag, by
//! `[patch]`, by submodule path, whichever mechanism wins. One name means one.
//!
//! Nothing stops a plugin crate from adding `cal-core` back to its own
//! `Cargo.toml` and reaching past the SDK; it compiles perfectly well while all
//! the crates are neighbours, and it is only the day of the move that the extra
//! name turns into an extra pin. So this asks the manifests, not the sources: a
//! `use` can be rewritten, a dependency has to be declared.
//!
//! ## Which crates this is about
//!
//! The `*-plugin` rlibs and the `*-cdylib` shells — the two halves of an
//! adapter that exist BECAUSE of the plugin system.
//!
//! Not the `adapter-X` logic crate beneath them. That one implements
//! `cal_core`'s traits and speaks HTTP; it is a provider client that happens to
//! be wrapped as a plugin, and making it depend on an FFI SDK to reach the
//! vocabulary would be the same inversion as folding `cal-core` into
//! `plugin-core`. It names its domain crate directly, and so do `adapter-local`
//! and `adapter-device-calendar`, which are part of the app binary and are
//! never loaded as plugins at all.

use std::fs;
use std::path::PathBuf;

/// The crates a plugin crate must reach THROUGH the SDK rather than name.
const BEHIND_THE_FACADE: [&str; 4] = ["cal-core", "sync-core", "vc-core", "plugin-core"];

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("host-plugins lives under crates/")
        .to_path_buf()
}

#[test]
fn a_plugin_crate_names_only_the_sdk() {
    let mut checked = Vec::new();
    let mut offenders = Vec::new();

    for entry in fs::read_dir(crates_dir()).expect("read crates/").flatten() {
        let dir = entry.path();
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !name.ends_with("-plugin") && !name.ends_with("-cdylib") {
            continue;
        }
        let manifest = dir.join("Cargo.toml");
        let text = fs::read_to_string(&manifest)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
        checked.push(name.to_string());

        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            for behind in BEHIND_THE_FACADE {
                // A dependency line starts with the crate name: `cal-core = …`
                // or `cal-core.workspace = true`. A mention inside a comment or
                // a feature list is not a dependency.
                let declares = line
                    .strip_prefix(behind)
                    .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with('.'));
                if declares {
                    offenders.push(format!("{name}: {line}"));
                }
            }
        }
    }

    // Named anchors rather than a count: the number of adapters is exactly what
    // the extraction changes, so `checked.len() >= 24` would break on the one
    // event it should survive. These two are the shapes — an rlib and a shell —
    // and if the walk stops seeing them it is not reading the tree it claims to.
    for anchor in ["adapter-vikunja-plugin", "adapter-vikunja-cdylib"] {
        assert!(
            checked.iter().any(|c| c == anchor),
            "the walk missed {anchor} — it is not covering the crates it claims to",
        );
    }

    assert!(
        offenders.is_empty(),
        "a plugin crate names an Aperio crate the SDK already re-exports. Reach \
         it through `plugin_sdk::` instead and drop the dependency — every name \
         here is a version an out-of-tree adapter would have to pin \
         separately:\n  {}",
        offenders.join("\n  "),
    );
}
