//! `cargo xtask <task>` — repository automation that needs to run at a moment
//! no build script can.
//!
//! ## Why staging is not a build script any more
//!
//! Each bundled plugin ships as two crates: an rlib `*-plugin` crate holding
//! the adapter and its `plugin.json`, and a thin `*-cdylib` shell emitting the
//! `#[no_mangle]` C-ABI exports the desktop's `dlopen` loader resolves. Neither
//! is a cargo dependency of `aperio` — depending on the cdylibs would make the
//! host link twelve copies of `aperio_plugin_create` and collide.
//!
//! So the app's build script had to copy those cdylibs into
//! `target/<profile>/plugins/bundled/<plugin-id>/` after cargo produced them.
//! A build script cannot do that, because it runs DURING the build: cargo
//! compiles workspace members in parallel, and the cdylibs may not exist yet
//! when `aperio`'s script runs. The workaround was visible in
//! `tauri.conf.json`, which built the app twice —
//! `cargo build --workspace && cargo build -p aperio` — the second one purely
//! to make the script rerun once the libraries were on disk. A missing cdylib
//! was a `cargo:warning` and the build carried on, so the failure mode was a
//! release binary that starts and can connect to nothing.
//!
//! Staging after the build removes the race rather than working around it, and
//! lets a missing cdylib be what it is: an error.
//!
//! ## Why the plugin list is not written down
//!
//! It used to be a table of twelve triples in `src-tauri/build.rs`, and that
//! table was read by four other things — two `sed`/`grep` steps in the release
//! workflow, and a Rust test that scraped `build.rs` with `include_str!` to
//! check the shell one-liner still agreed with it. All of them had to be edited
//! together, and once they were not: a helper filtering on the `com.aperio.`
//! prefix added one string in a function that stages nothing, the workflow's
//! count went up by one, and every artifact build went red against a tree that
//! was fine.
//!
//! The workspace already knows. A plugin shell is a member producing a `cdylib`
//! that depends on `plugin-sdk` — the crate whose `declare_cdylib_exports!`
//! emits the very symbols the loader resolves, which nothing else here has a
//! reason to use. Twelve today; `cal-ffi`, the phone's own cdylib, does not use
//! the SDK and is correctly not one. Adding or retiring an adapter changes the
//! answer with no list to remember.
//!
//! Asked positively, on purpose. The first version of this rule was "a cdylib
//! with a `*-plugin` dependency", and anything else was skipped in silence — so
//! a shell that stopped matching, for any reason, stopped being a plugin, with
//! every gate green and the adapter simply absent from the shipped artifact.
//! Now a cdylib that uses the SDK and then fails to name exactly one adapter is
//! an error that says which crate and why.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let task = args.first().map(String::as_str);
    match task {
        Some("stage-plugins") => run(stage_plugins(&args[1..])),
        Some("pack-plugins") => run(pack_plugins(&args[1..])),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run(outcome: Result<String, String>) -> ExitCode {
    match outcome {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "\
cargo xtask <task>

Tasks:
  stage-plugins [--release] [--target <triple>] [--check [--dir <path>]]
        Copy every bundled plugin's cdylib and plugin.json into
        target/<profile>/plugins/bundled/<plugin-id>/, where the desktop host
        scans for them. Run it AFTER `cargo build --workspace`, which is what
        produces the cdylibs.

        --check   verify what is staged and change nothing. For CI after a
                  packaging step, to prove the artifact carries its adapters.
        --dir     the staged directory to check, instead of the one under
                  target/. For a layout assembled somewhere else — the macOS
                  universal build lipo-fuses two arches into a staging tree of
                  its own, and that tree needs proving too.

  pack-plugins [--release] [--target <triple>] [--out <dir>]
        Pack every staged plugin into a `.aperio` archive — the format the
        plugin installer reads. Run it after `stage-plugins`.

        Archives land in target/<profile>/plugins/packaged/ unless --out says
        otherwise, named `<plugin-id>-<version>-<triple>.aperio`. The triple is
        in the name because it is NOT in the archive: the installer picks a
        library by file extension alone, so a Windows arm64 build and a Windows
        x64 build produce archives that look alike and are not
        interchangeable.";

/// One bundled plugin, as the workspace describes it.
struct Bundled {
    /// The cdylib crate's name. With `-` replaced by `_` it is the library's
    /// filename stem, which is how cargo writes it.
    cdylib_crate: String,
    /// Where the `-plugin` crate lives — the manifest is `plugin.json` beside
    /// its `Cargo.toml`.
    plugin_dir: PathBuf,
    /// The id from that manifest. It names the staged directory and is what the
    /// host keys the loaded plugin on.
    plugin_id: String,
}

fn stage_plugins(args: &[String]) -> Result<String, String> {
    let mut release = false;
    let mut check = false;
    let mut target: Option<String> = None;
    let mut dir: Option<PathBuf> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--release" => release = true,
            "--check" => check = true,
            "--target" => {
                target = Some(
                    rest.next()
                        .ok_or_else(|| "--target needs a triple".to_string())?
                        .clone(),
                )
            }
            "--dir" => {
                dir = Some(PathBuf::from(
                    rest.next()
                        .ok_or_else(|| "--dir needs a path".to_string())?,
                ))
            }
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }

    let metadata = metadata()?;
    let workspace_root = PathBuf::from(
        metadata["workspace_root"]
            .as_str()
            .ok_or("cargo metadata has no workspace_root")?,
    );
    let target_dir = PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .ok_or("cargo metadata has no target_directory")?,
    );

    let bundled = discover(&metadata)?;
    // A discovery that found nothing would stage nothing and report success.
    if bundled.is_empty() {
        return Err(format!(
            "no bundled plugins found in {}. A bundled plugin is a workspace member \
             that produces a cdylib and depends on a `*-plugin` crate; if that is no \
             longer how they are built, this task needs to know",
            workspace_root.display(),
        ));
    }

    let profile_dir = match &target {
        Some(triple) => target_dir.join(triple),
        None => target_dir.clone(),
    }
    .join(if release { "release" } else { "debug" });
    let bundled_dir = dir
        .clone()
        .unwrap_or_else(|| profile_dir.join("plugins").join("bundled"));

    if check {
        return verify(&bundled, &bundled_dir);
    }
    if dir.is_some() {
        return Err(
            "--dir only makes sense with --check; staging writes beside the binaries it copies"
                .to_string(),
        );
    }

    // Before staging, not after: a plugin that was renamed would otherwise sit
    // under two ids, and the host would load yesterday's alongside today's.
    let pruned = prune(&bundled, &bundled_dir)?;

    for plugin in &bundled {
        let src = cdylib_path(&profile_dir, &plugin.cdylib_crate);
        if !src.is_file() {
            return Err(format!(
                "{}: no cdylib at {}. Run `cargo build --workspace{}` first — that is \
                 what produces it",
                plugin.plugin_id,
                src.display(),
                if release { " --release" } else { "" },
            ));
        }
        let dst_dir = bundled_dir.join(&plugin.plugin_id);
        fs::create_dir_all(&dst_dir)
            .map_err(|e| format!("{}: mkdir {}: {e}", plugin.plugin_id, dst_dir.display()))?;

        // Renamed on the way in, to the plugin-id-prefixed name
        // `plugin_core::locate_library` looks for first. Without it the loader
        // falls back to "any file with the right extension", which is a guess.
        let dst = dst_dir.join(format!("{}.{}", plugin.plugin_id, cdylib_extension()));
        fs::copy(&src, &dst).map_err(|e| {
            format!(
                "{}: copy {} -> {}: {e}",
                plugin.plugin_id,
                src.display(),
                dst.display()
            )
        })?;

        let manifest_src = plugin.plugin_dir.join("plugin.json");
        fs::copy(&manifest_src, dst_dir.join("plugin.json"))
            .map_err(|e| format!("{}: copy {}: {e}", plugin.plugin_id, manifest_src.display()))?;
    }

    // Staging that reported success without producing the tree it describes
    // would be the same silence this replaced.
    verify(&bundled, &bundled_dir)?;

    let pruned_note = if pruned.is_empty() {
        String::new()
    } else {
        format!(
            " (removed {}: no longer in the workspace)",
            pruned.join(", ")
        )
    };
    Ok(format!(
        "staged {} bundled plugins into {}{pruned_note}",
        bundled.len(),
        bundled_dir.display(),
    ))
}

/// Pack every staged plugin into a `.aperio` archive.
///
/// The format has had a complete reader since the plugin installer was written
/// — inspect, install, a path-traversal guard, a confirmation dialog — and
/// until now nothing that produced one. The only `.aperio` file that had ever
/// existed was built by a unit-test helper, which meant the first real archive
/// anyone made would have been an out-of-tree adapter author's, discovering
/// whatever was wrong with the shape on their own time.
///
/// Twelve adapters are staged in this repository. Packing them is the cheapest
/// possible proof that the writer and the reader agree about the format, and it
/// costs a few seconds after a build that already happened.
fn pack_plugins(args: &[String]) -> Result<String, String> {
    let mut release = false;
    let mut target: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--release" => release = true,
            "--target" => {
                target = Some(
                    rest.next()
                        .ok_or_else(|| "--target needs a triple".to_string())?
                        .clone(),
                )
            }
            "--out" => {
                out = Some(PathBuf::from(
                    rest.next()
                        .ok_or_else(|| "--out needs a path".to_string())?,
                ))
            }
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }

    let metadata = metadata()?;
    let target_dir = PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .ok_or("cargo metadata has no target_directory")?,
    );
    let bundled = discover(&metadata)?;
    if bundled.is_empty() {
        return Err("no bundled plugins found; see `stage-plugins`".to_string());
    }

    let profile_dir = match &target {
        Some(triple) => target_dir.join(triple),
        None => target_dir.clone(),
    }
    .join(if release { "release" } else { "debug" });
    let staged_dir = profile_dir.join("plugins").join("bundled");
    let out_dir = out.unwrap_or_else(|| profile_dir.join("plugins").join("packaged"));

    // Named for what it is, because the archive cannot say it. `locate_library`
    // finds the library by extension, so nothing inside distinguishes an arm64
    // build from an x64 one.
    let triple = match &target {
        Some(triple) => triple.clone(),
        None => host_triple()?,
    };

    let mut packed = Vec::new();
    for plugin in &bundled {
        let dir = staged_dir.join(&plugin.plugin_id);
        if !dir.is_dir() {
            return Err(format!(
                "{} is not staged at {}. Run `cargo xtask stage-plugins{}` first",
                plugin.plugin_id,
                dir.display(),
                if release { " --release" } else { "" },
            ));
        }
        let version = manifest_string(&dir, "version")?;
        let dest = out_dir.join(format!("{}-{version}-{triple}.aperio", plugin.plugin_id));
        plugin_core::pack_archive(&dir, &dest).map_err(|e| e.to_string())?;
        packed.push(dest);
    }

    // Every archive read back through the reader that will meet it in the
    // wild. Producing a file nobody has opened is how the format got here.
    for archive in &packed {
        plugin_core::inspect_archive(archive)
            .map_err(|e| format!("{} does not read back: {e}", archive.display()))?;
    }

    Ok(format!(
        "packed {} plugins into {}",
        packed.len(),
        out_dir.display(),
    ))
}

/// The target triple this build runs on, asked of rustc rather than guessed.
fn host_triple() -> Result<String, String> {
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let out = Command::new(rustc)
        .arg("-vV")
        .output()
        .map_err(|e| format!("running `rustc -vV`: {e}"))?;
    if !out.status.success() {
        return Err("`rustc -vV` failed".to_string());
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(|host| host.trim().to_string())
        .ok_or_else(|| "`rustc -vV` printed no host line".to_string())
}

/// What `cargo metadata` says about this workspace, with dependencies left out:
/// every member is listed either way, and resolving the graph costs a network
/// round trip this task has no use for.
fn metadata() -> Result<serde_json::Value, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .map_err(|e| format!("running `cargo metadata`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`cargo metadata` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim(),
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("parsing `cargo metadata`: {e}"))
}

/// Every workspace member that produces a cdylib and depends on a `*-plugin`
/// crate — the shape a bundled plugin has, rather than a list of their names.
fn discover(metadata: &serde_json::Value) -> Result<Vec<Bundled>, String> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?;

    // Where each member lives, so a `-plugin` dependency can be turned into the
    // directory holding its manifest.
    let dirs: std::collections::BTreeMap<&str, PathBuf> = packages
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?;
            let manifest = Path::new(p["manifest_path"].as_str()?);
            Some((name, manifest.parent()?.to_path_buf()))
        })
        .collect();

    let mut found = Vec::new();
    for package in packages {
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        let makes_cdylib = package["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|t| {
                t["crate_types"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|k| k.as_str() == Some("cdylib"))
            });
        if !makes_cdylib {
            continue;
        }

        // Only NORMAL dependencies. cargo reports `kind` as null for those and
        // "dev" / "build" otherwise, and the difference matters here:
        // `host-core` alone dev-depends on eight `-plugin` crates so its tests
        // can read real manifests, and a dev-dependency is not linked into
        // anything.
        let deps: Vec<&str> = package["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["kind"].is_null())
            .filter_map(|d| d["name"].as_str())
            .collect();

        // What makes a cdylib a PLUGIN SHELL, positively: it depends on
        // `plugin-sdk`, which is the crate whose `declare_cdylib_exports!`
        // emits the `#[no_mangle] aperio_plugin_*` symbols the loader resolves.
        // Nothing else in the workspace has a reason to.
        //
        // Asking it this way round matters. The rule used to be "a cdylib with
        // a `-plugin` dependency", and everything else fell into a silent
        // `continue` — so a shell that stopped matching, for any reason, simply
        // stopped being a plugin, with every gate still green and the adapter
        // missing from the shipped artifact. `cal-ffi` is the only other cdylib
        // here, it does not use the SDK, and it is skipped for a reason that is
        // stated rather than assumed.
        if !deps.contains(&"plugin-sdk") {
            continue;
        }

        let plugin_deps: Vec<&str> = deps
            .iter()
            .copied()
            .filter(|d| d.ends_with("-plugin"))
            .collect();
        match plugin_deps.as_slice() {
            [plugin] if dirs.contains_key(plugin) => {
                let plugin_dir = dirs[plugin].clone();
                let plugin_id = plugin_id(&plugin_dir)?;
                found.push(Bundled {
                    cdylib_crate: name.to_string(),
                    plugin_dir,
                    plugin_id,
                });
            }
            // The adapter crates are meant to move into their own repositories
            // (DESIGN.md section 20.4). On the day one does, its `-plugin` crate
            // stops being a workspace member and `cargo metadata --no-deps`
            // stops describing it — so this is where that lands, loudly, rather
            // than as an adapter quietly absent from the build.
            [plugin] => {
                return Err(format!(
                    "{name} is a plugin shell for `{plugin}`, which is not a member of \
                     this workspace. Staging reads the manifest from the crate's own \
                     directory, and only a member has one here; if plugin crates now \
                     come from elsewhere, this task needs to learn how to find them",
                ))
            }
            [] => {
                return Err(format!(
                    "{name} uses plugin-sdk but depends on no `*-plugin` crate, so there \
                     is no manifest to stage with it. Either it is not a plugin shell, \
                     or its adapter dependency is missing",
                ))
            }
            many => {
                return Err(format!(
                    "{name} depends on {} plugin crates ({}); a cdylib shell emits the \
                     C-ABI exports for exactly one, or its symbols collide",
                    many.len(),
                    many.join(", "),
                ))
            }
        }
    }
    found.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
    Ok(found)
}

/// The `id` a crate's `plugin.json` declares — the name of its staged directory
/// and the key the host loads it under.
fn plugin_id(plugin_dir: &Path) -> Result<String, String> {
    manifest_string(plugin_dir, "id")
}

/// One top-level string out of a directory's `plugin.json`.
fn manifest_string(plugin_dir: &Path, field: &str) -> Result<String, String> {
    let path = plugin_dir.join("plugin.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    value[field]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{} declares no {field}", path.display()))
}

/// Cargo's filename for a cdylib, which differs per platform.
fn cdylib_path(profile_dir: &Path, cdylib_crate: &str) -> PathBuf {
    let stem = cdylib_crate.replace('-', "_");
    if cfg!(target_os = "windows") {
        profile_dir.join(format!("{stem}.dll"))
    } else if cfg!(target_os = "macos") {
        profile_dir.join(format!("lib{stem}.dylib"))
    } else {
        profile_dir.join(format!("lib{stem}.so"))
    }
}

fn cdylib_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// Remove staged directories for plugins the workspace no longer has.
///
/// Nothing used to, and a target directory accumulated every plugin the
/// workspace had EVER produced: retiring an adapter deleted its crate, its
/// manifest and its registration, and the app went on loading yesterday's
/// cdylib from disk. Folding Drive into Google and folder sync into the
/// built-in store did exactly that — the next run loaded fourteen where twelve
/// were expected, purely from leftovers.
///
/// Only `com.aperio.*` is touched. A third-party plugin somebody dropped in by
/// hand is theirs, and this is the wrong thing to be deleting it.
fn prune(bundled: &[Bundled], bundled_dir: &Path) -> Result<Vec<String>, String> {
    let expected: BTreeSet<&str> = bundled.iter().map(|b| b.plugin_id.as_str()).collect();
    let Ok(entries) = fs::read_dir(bundled_dir) else {
        // Nothing staged yet — the first build into a clean target.
        return Ok(Vec::new());
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("com.aperio.") || expected.contains(name) {
            continue;
        }
        fs::remove_dir_all(&path)
            .map_err(|e| format!("removing stale plugin {name} from {}: {e}", path.display()))?;
        removed.push(name.to_string());
    }
    Ok(removed)
}

/// Every discovered plugin is on disk with both halves, and nothing else of
/// ours is.
fn verify(bundled: &[Bundled], bundled_dir: &Path) -> Result<String, String> {
    let mut problems = Vec::new();
    for plugin in bundled {
        let dir = bundled_dir.join(&plugin.plugin_id);
        for file in [
            dir.join(format!("{}.{}", plugin.plugin_id, cdylib_extension())),
            dir.join("plugin.json"),
        ] {
            if !file.is_file() {
                problems.push(format!("missing {}", file.display()));
            }
        }
    }

    let expected: BTreeSet<&str> = bundled.iter().map(|b| b.plugin_id.as_str()).collect();
    if let Ok(entries) = fs::read_dir(bundled_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.is_dir() && name.starts_with("com.aperio.") && !expected.contains(name) {
                problems.push(format!("{name} is staged but not part of the workspace"));
            }
        }
    } else {
        problems.push(format!("{} does not exist", bundled_dir.display()));
    }

    if problems.is_empty() {
        Ok(format!(
            "{} bundled plugins staged in {}",
            bundled.len(),
            bundled_dir.display(),
        ))
    } else {
        Err(format!(
            "the staged plugins do not match the workspace:\n  {}",
            problems.join("\n  "),
        ))
    }
}
