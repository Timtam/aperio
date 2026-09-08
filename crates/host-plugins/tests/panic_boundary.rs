//! Every `extern "C"` function a bundled plugin exposes catches its panics.
//!
//! A function with C linkage that unwinds is a process abort — Rust inserts the
//! abort rather than let the unwind cross — so a panic anywhere in a plugin
//! takes the whole of Aperio down with it: every other account, mid-sentence,
//! with nothing on screen and nothing in the log naming the adapter that did
//! it.
//!
//! The SDK closes that at every boundary it owns. This test exists because the
//! CONTRACT — the C header, the plugin docs, DESIGN §20.3 — promises it for
//! every entry point, and a promise the tree cannot check is a promise that
//! decays. It decayed once already: the first version of the guard wrapped six
//! dispatch helpers while the docs said "every vtable slot, every named
//! export", leaving seventeen hand-written synchronous slots and twelve
//! `close_instance` shims aborting exactly as before.
//!
//! ## What counts as covered
//!
//! One of three, all of which end in `catch_unwind`:
//!
//! - a `*_dispatch` / `*_dispatch_unit` helper — the async vtable slots;
//! - `plugin_sdk::guarded` — the synchronous slots that marshal their own
//!   result (`capabilities`, `calendar_color`, `fetch_sound_asset`);
//! - `plugin_sdk::guarded_void` — the ones with no channel to answer through.
//!
//! Or the body delegates to an SDK helper that is itself guarded
//! (`open_instance_with`, `discover_with`, `interactive_auth_with`,
//! `probe_host_key_with`), which is why those names count too.
//!
//! No allowlist. There is nothing here that is allowed to be uncovered, and if
//! something has to be, it should be an argued exception in a diff rather than
//! a line in a table nobody re-reads.

use std::fs;
use std::path::{Path, PathBuf};

/// Ways a body can reach a `catch_unwind`.
///
/// Substring matching, deliberately coarse. `dispatch` covers the six helpers
/// under all the local aliases the adapters import them as — most write
/// `dispatch(h, …)` after a `use … as dispatch`, so matching the full names
/// would miss every one of them. The cost of coarseness is that a comment
/// mentioning one of these words inside an `extern "C"` fn would excuse it;
/// the cost of precision would be a parser, and a parser that drifts is how
/// this kind of test goes quiet.
const GUARDS: [&str; 4] = [
    // `guarded` and `guarded_void`.
    "guarded",
    // `cal_dispatch`, `sync_dispatch_unit`, and the aliases they arrive under.
    "dispatch",
    // The macro-emitted exports.
    "catching_panics_into(",
    // `open_instance_with`, `discover_with`, `interactive_auth_with`,
    // `probe_host_key_with`, `strings_with` — SDK helpers that catch inside.
    "_with(",
];

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("host-plugins lives under crates/")
        .to_path_buf()
}

/// The `-plugin` crates: the rlibs that hold an adapter's FFI surface. The
/// `-cdylib` shells are three lines that expand `declare_cdylib_exports!`,
/// whose expansion is guarded in the SDK itself.
fn plugin_crate_sources() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(crates_dir()).expect("read crates/").flatten() {
        let dir = entry.path();
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !name.ends_with("-plugin") {
            continue;
        }
        collect_rs(&dir.join("src"), &mut found);
    }
    found
}

fn collect_rs(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, found);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            found.push(path);
        }
    }
}

/// The body of the `extern "C"` fn whose signature starts at `at`, from the
/// brace that opens it to the matching close.
fn body_after(text: &str, at: usize) -> Option<(String, usize)> {
    let open = text[at..].find('{')? + at;
    let mut depth = 0usize;
    for (offset, ch) in text[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((text[open..open + offset].to_string(), open + offset));
                }
            }
            _ => {}
        }
    }
    None
}

#[test]
fn every_plugin_entry_point_catches_its_panics() {
    let sources = plugin_crate_sources();

    // The walk has to have walked. A mistyped directory would leave `sources`
    // empty and this test would pass having read nothing — which is the failure
    // shape the whole file is about.
    let anchor = crates_dir()
        .join("adapter-caldav-plugin")
        .join("src")
        .join("lib.rs");
    assert!(
        sources.contains(&anchor),
        "the walk missed {} — it is not covering the crates it claims to",
        anchor.display(),
    );

    let mut unguarded = Vec::new();
    let mut checked = 0usize;
    for path in &sources {
        let text = fs::read_to_string(path).expect("read source");
        let mut at = 0;
        while let Some(hit) = text[at..].find("extern \"C\" fn ") {
            let start = at + hit;
            let name: String = text[start + "extern \"C\" fn ".len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let Some((body, end)) = body_after(&text, start) else {
                break;
            };
            checked += 1;
            if !GUARDS.iter().any(|g| body.contains(g)) {
                unguarded.push(format!("{}: {name}", path.display()));
            }
            at = end;
        }
    }

    // Named, because the number of adapters is exactly what the extraction
    // changes — but a floor of zero is not a floor, it is the whole claim.
    assert!(
        checked > 0,
        "no `extern \"C\"` functions were found at all, so this proved nothing",
    );

    assert!(
        unguarded.is_empty(),
        "these plugin entry points can unwind across the FFI boundary, which \
         aborts Aperio — every account, not just this adapter's. Wrap the body \
         in `plugin_sdk::guarded` (or `guarded_void` when it returns nothing), \
         or route it through a dispatch helper:\n  {}",
        unguarded.join("\n  "),
    );
}
