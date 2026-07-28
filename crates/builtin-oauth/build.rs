//! Feed the OAuth client credentials into `option_env!` at compile time.
//!
//! Two sources, in this order of precedence:
//!
//!  1. **Environment variables** — what CI uses. The release workflows export
//!     them from repository secrets; nothing is written to disk.
//!  2. **A local file** — what a developer uses. Same variable names, one
//!     `NAME=value` per line. Default path is `oauth-clients.local.env` beside
//!     the workspace root; `APERIO_OAUTH_CLIENTS_FILE` overrides it (useful to
//!     keep the file outside the checkout entirely).
//!
//! Whatever is found is re-emitted with `cargo:rustc-env`, so `src/lib.rs` can
//! read it through `option_env!` without the caller having to export anything.
//! A variable that is set but EMPTY counts as absent — an empty CI secret
//! should behave like a build without credentials, not like a client id of "".
//!
//! Nothing here fails the build. A build with no credentials is a first-class
//! configuration: the app then asks the user to register their own integration
//! (see `crates/builtin-oauth/README.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Every variable the crate knows how to bake in. Adding a provider means
/// adding its names here and a matching `option_env!` pair in `src/lib.rs`.
const VARS: &[&str] = &[
    "APERIO_OAUTH_WEBEX_CLIENT_ID",
    "APERIO_OAUTH_WEBEX_CLIENT_SECRET",
    "APERIO_OAUTH_GOOGLE_CLIENT_ID",
    "APERIO_OAUTH_GOOGLE_CLIENT_SECRET",
    "APERIO_OAUTH_MICROSOFT_CLIENT_ID",
];

const FILE_VAR: &str = "APERIO_OAUTH_CLIENTS_FILE";
const DEFAULT_FILE: &str = "oauth-clients.local.env";

fn main() {
    println!("cargo:rerun-if-env-changed={FILE_VAR}");
    for var in VARS {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let file_path = resolve_file_path();
    println!("cargo:rerun-if-changed={}", file_path.display());

    let from_file = read_file(&file_path);

    let mut baked = 0usize;
    for var in VARS {
        // Environment beats file, so a CI run cannot be shadowed by a stray
        // checked-out file and a developer can override one value ad hoc.
        let value = std::env::var(var)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| from_file.get(*var).cloned());
        if let Some(value) = value {
            // A newline would let a value forge further cargo directives.
            let value = value.trim();
            if value.is_empty() || value.contains('\n') || value.contains('\r') {
                println!("cargo:warning={var} is empty or contains a newline; ignored");
                continue;
            }
            println!("cargo:rustc-env={var}={value}");
            baked += 1;
        }
    }

    // Deliberately counts, never names or echoes a value: build logs are
    // routinely pasted into issues.
    println!("cargo:rustc-env=APERIO_OAUTH_BAKED_COUNT={baked}");
}

fn resolve_file_path() -> PathBuf {
    if let Ok(explicit) = std::env::var(FILE_VAR) {
        if !explicit.trim().is_empty() {
            return PathBuf::from(explicit);
        }
    }
    // CARGO_MANIFEST_DIR is <workspace>/crates/builtin-oauth.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    manifest
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join(DEFAULT_FILE))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FILE))
}

/// Parse a `NAME=value` file. Blank lines and `#` comments are skipped, an
/// optional leading `export ` is tolerated (so the same file can be sourced by
/// a shell), and surrounding single or double quotes are stripped.
fn read_file(path: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        if !name.is_empty() && !value.is_empty() {
            out.insert(name.to_string(), value.to_string());
        }
    }
    out
}
