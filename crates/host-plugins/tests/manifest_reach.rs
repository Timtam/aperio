//! A manifest is read from the crate that owns it, not from its directory.
//!
//! Every host that needed an adapter's `plugin.json` used to embed it by
//! relative path — `include_bytes!("../../adapter-caldav-plugin/plugin.json")`
//! and thirty-four more like it. That works, and it works for exactly one
//! reason: the crates are neighbours in one checkout. A cargo dependency hands
//! you a crate, not the directory it was built from, so the day an adapter
//! lives in its own repository every one of those lines stops compiling — and
//! ten of them, reaching five different crates, had no cargo dependency edge at
//! all, so nothing but the directory layout connected the two crates.
//!
//! Each manifest-owning crate now exports its own bytes as `MANIFEST`, and the
//! hosts read that. These two tests keep it that way: one refuses a reach
//! across crate boundaries, the other refuses a manifest-owning crate that
//! forgot to export its own.
//!
//! Both walk the tree on purpose. They are statements about THIS repository's
//! sources, so they have to stay meaningful after the adapters move out — which
//! is why neither guards itself with a count. A count would be the number of
//! crates there are today, and the move is precisely the event that changes it;
//! the anti-silence guards below name things that must be found instead.

use std::fs;
use std::path::{Path, PathBuf};

/// The sanctioned form, spelled out so there is exactly one of it.
///
/// `CARGO_MANIFEST_DIR` rather than a path relative to the source file: the
/// crate root is where `plugin.json` lives by definition, so the const does not
/// have to be kept in step with where in the crate it happens to be written.
const MANIFEST_CONST: &str = r#"pub const MANIFEST: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/plugin.json"));"#;

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("host-plugins lives under crates/")
        .to_path_buf()
}

fn repo_root() -> PathBuf {
    crates_dir()
        .parent()
        .expect("crates/ lives under the repository root")
        .to_path_buf()
}

/// Every `.rs` file under `dir`, recursively. `target/` is skipped: it holds
/// generated sources that no one edits.
fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if name != "target" && name != "node_modules" {
                rust_sources(&path, found);
            }
        } else if name.ends_with(".rs") {
            found.push(path);
        }
    }
}

/// The argument of the `include_bytes!` / `include_str!` invocation starting at
/// `start`, with whitespace collapsed so a call broken over several lines reads
/// the same as a one-liner.
///
/// `None` when what follows the macro name is not a `(` — prose in a doc
/// comment mentions these macros by name, and reading on from there would
/// swallow whatever paren came next.
fn include_arg(text: &str, start: usize) -> Option<String> {
    if !text[start..].trim_start().starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    let mut arg = String::new();
    for ch in text[start..].chars() {
        match ch {
            '(' => {
                depth += 1;
                if depth == 1 {
                    continue;
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        arg.push(if ch.is_whitespace() { ' ' } else { ch });
    }
    Some(arg.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// One source file per walked root that must turn up, so a root that goes dark
/// fails loudly instead of contributing nothing. Named files rather than a
/// count: the number of crates is exactly what the extraction changes.
const WALK_ANCHORS: [&str; 2] = ["crates/host-plugins/src/lib.rs", "src-tauri/src/lib.rs"];

#[test]
fn no_crate_embeds_another_crates_manifest() {
    let root = repo_root();
    let mut sources = Vec::new();
    rust_sources(&crates_dir(), &mut sources);
    rust_sources(&root.join("src-tauri").join("src"), &mut sources);

    // `rust_sources` returns quietly when a directory cannot be read, so a
    // mistyped root would otherwise make this test pass by scanning nothing —
    // and a total across both roots would hide the smaller one going dark.
    for anchor in WALK_ANCHORS {
        let expected = root.join(anchor.replace('/', std::path::MAIN_SEPARATOR_STR));
        assert!(
            sources.contains(&expected),
            "the walk missed {anchor} — it is not covering the tree it claims to",
        );
    }

    let mut offenders = Vec::new();
    for path in &sources {
        // This file quotes the shape it forbids, in the module docs above.
        if path.ends_with("manifest_reach.rs") {
            continue;
        }
        let text =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for macro_name in ["include_bytes!", "include_str!"] {
            let mut at = 0;
            while let Some(hit) = text[at..].find(macro_name) {
                let start = at + hit + macro_name.len();
                if let Some(arg) = include_arg(&text, start) {
                    if arg.contains("plugin.json") && arg.contains("../../") {
                        offenders.push(format!("{}: {macro_name}({arg})", path.display()));
                    }
                }
                at = start;
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a manifest is being read out of another crate's directory, which only \
         works while the crates are neighbours. Read the owning crate's \
         `MANIFEST` const instead (and declare the cargo dependency that makes \
         it reachable):\n  {}",
        offenders.join("\n  "),
    );
}

#[test]
fn every_manifest_owning_crate_exports_its_manifest() {
    let crates = crates_dir();
    let mut checked = Vec::new();
    let mut missing = Vec::new();

    for entry in fs::read_dir(&crates).expect("read crates/") {
        let dir = entry.expect("dir entry").path();
        if !dir.join("plugin.json").is_file() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let lib = dir.join("src").join("lib.rs");
        let text =
            fs::read_to_string(&lib).unwrap_or_else(|e| panic!("read {}: {e}", lib.display()));
        if text.contains(MANIFEST_CONST) {
            checked.push(name);
        } else {
            missing.push(name);
        }
    }

    assert!(
        missing.is_empty(),
        "these crates ship a plugin.json but do not export it as `MANIFEST`, so \
         a host can only reach it by path: {}\nAdd:\n  {MANIFEST_CONST}",
        missing.join(", "),
    );

    // The anti-silence guard, by name rather than by count. These two are the
    // adapters the host links instead of loading (`host_core::builtin_adapters`),
    // so they are the two that stay in this repository no matter how many
    // plugin crates move out — a walk that cannot find them is broken, and a
    // walk that finds only them is correct.
    for built_in in ["adapter-local", "adapter-device-calendar"] {
        assert!(
            checked.iter().any(|c| c == built_in),
            "{built_in} was not among the manifest-owning crates found under {} — \
             the walk is probably wrong (found: {})",
            crates.display(),
            checked.join(", "),
        );
    }
}
