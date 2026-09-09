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
        Some("ts-types") => run(ts_types(&args[1..])),
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
        interchangeable.

  ts-types [--check]
        Regenerate shared/generated/ — the TypeScript declarations both
        frontends parse — from the Rust types carrying `#[derive(ts_rs::TS)]`.
        Run it after changing any of them.

        --check   generate into a scratch directory and compare, changing
                  nothing. For CI, so a Rust field that never reached the
                  frontends is a red build rather than a value nobody reads.";

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
    check_against_what_the_repo_declares(&bundled, &metadata)?;

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
    // Same blind spot, same check: this task asks the workspace too, so an
    // adapter that left it would be quietly absent from the archives as well as
    // from the staging tree.
    check_against_what_the_repo_declares(&bundled, &metadata)?;

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
        // WITH dependencies. `--no-deps` would list only workspace members, and
        // an adapter that has moved into its own repository is then invisible —
        // its `plugin.json` sits in cargo's git checkout, which only the full
        // graph names. See `discover`.
        .args(["metadata", "--format-version", "1"])
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

/// `ts-types` — regenerate `shared/generated/` from the Rust declarations.
///
/// # Why the frontends do not restate the domain
///
/// `shared/types.ts` used to be 285 lines of hand-written TypeScript whose own
/// header said "the crate `cal-core` is the source of truth; if a field changes
/// there, mirror it here". Nothing checked that anyone did, and the mirror had
/// drifted: a reminder's `minutes_before` was `number` where the wire carries an
/// i64, `Task.recurrence` had given up and said `unknown`, and half of
/// `TaskCapabilities` was optional for fields the backend always sends.
///
/// Now the shapes are generated, so a field added in Rust is a TypeScript
/// compile error in the same commit rather than a value that arrives and is
/// never read.
///
/// # Which crates
///
/// Not a list. Every workspace member that declares a `ts-export` feature is
/// one — the feature exists for exactly this purpose, so a fourth crate joins
/// by declaring it and nothing here has to remember. Zero of them is an error,
/// not a quiet success.
fn ts_types(args: &[String]) -> Result<String, String> {
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }

    let meta = metadata()?;
    let root = PathBuf::from(
        meta["workspace_root"]
            .as_str()
            .ok_or("`cargo metadata` printed no workspace_root")?,
    );
    let committed = root.join("shared").join("generated");
    let crates = ts_export_members(&meta)?;

    // Generated somewhere else first, always. Writing in place would leave the
    // file of a type that has been RENAMED or removed sitting in the tree,
    // still compiling, describing something that no longer exists.
    let staging = env::temp_dir().join(format!("aperio-ts-types-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("creating {}: {e}", staging.display()))?;

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(&root)
        .env("TS_RS_EXPORT_DIR", &staging)
        .arg("test");
    for name in &crates {
        cmd.args(["-p", name]);
    }
    cmd.arg("--features")
        .arg(
            crates
                .iter()
                .map(|c| format!("{c}/ts-export"))
                .collect::<Vec<_>>()
                .join(","),
        )
        // The generator IS a test: `#[ts(export)]` emits one per type, and
        // running it writes the file.
        .arg("export_bindings");
    let out = cmd
        .output()
        .map_err(|e| format!("running `cargo test … export_bindings`: {e}"))?;
    if !out.status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "generating the bindings failed:\n{}",
            String::from_utf8_lossy(&out.stderr).trim(),
        ));
    }

    let fresh = read_ts_dir(&staging)?;
    if fresh.is_empty() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "the generator wrote no files at all, from {} crate(s): {}. Every \
             `#[derive(ts_rs::TS)]` needs `#[ts(export)]` beside it, or nothing is \
             written and this task would report success over an empty tree.",
            crates.len(),
            crates.join(", "),
        ));
    }

    if check {
        let have = read_ts_dir(&committed)?;
        let report = describe_drift(&have, &fresh);
        let _ = fs::remove_dir_all(&staging);
        return match report {
            None => Ok(format!(
                "shared/generated is current — {} type(s) from {}.",
                fresh.len(),
                crates.join(", "),
            )),
            Some(drift) => Err(format!(
                "shared/generated does not match the Rust declarations:\n{drift}\n\nRun \
                 `cargo xtask ts-types` and commit the result."
            )),
        };
    }

    let _ = fs::remove_dir_all(&committed);
    fs::create_dir_all(&committed).map_err(|e| format!("creating {}: {e}", committed.display()))?;
    for (name, body) in &fresh {
        fs::write(committed.join(name), body)
            .map_err(|e| format!("writing {}: {e}", committed.join(name).display()))?;
    }
    let _ = fs::remove_dir_all(&staging);
    Ok(format!(
        "shared/generated: {} type(s) written from {}.",
        fresh.len(),
        crates.join(", "),
    ))
}

/// Workspace members that declare a `ts-export` feature.
fn ts_export_members(meta: &serde_json::Value) -> Result<Vec<String>, String> {
    let members: BTreeSet<&str> = meta["workspace_members"]
        .as_array()
        .ok_or("`cargo metadata` printed no workspace_members")?
        .iter()
        .filter_map(|m| m.as_str())
        .collect();
    let mut out: Vec<String> = meta["packages"]
        .as_array()
        .ok_or("`cargo metadata` printed no packages")?
        .iter()
        .filter(|p| p["id"].as_str().is_some_and(|id| members.contains(id)))
        .filter(|p| p["features"].get("ts-export").is_some())
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect();
    out.sort();
    if out.is_empty() {
        return Err(
            "no workspace member declares a `ts-export` feature, so there is nothing \
                    to generate. That feature is how a crate says it owns part of the \
                    frontend domain; if one were renamed, this task would otherwise report \
                    success and leave shared/generated frozen."
                .to_string(),
        );
    }
    Ok(out)
}

/// Every `.ts` file in `dir`, by name, with line endings normalised — a Windows
/// checkout may carry CRLF where the generator writes LF, and that is not drift.
fn read_ts_dir(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(format!("reading {}: {e}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ts") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let body = fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?
            .replace("\r\n", "\n");
        out.push((name, body));
    }
    out.sort();
    Ok(out)
}

/// What changed, by NAME. A count would say "31 files, expected 32" and leave
/// the reader to work out which one.
fn describe_drift(have: &[(String, String)], fresh: &[(String, String)]) -> Option<String> {
    let mut lines = Vec::new();
    for (name, body) in fresh {
        match have.iter().find(|(n, _)| n == name) {
            None => lines.push(format!("  missing:  {name}")),
            Some((_, old)) if old != body => lines.push(format!("  outdated: {name}")),
            Some(_) => {}
        }
    }
    for (name, _) in have {
        if !fresh.iter().any(|(n, _)| n == name) {
            lines.push(format!(
                "  stale:    {name} (no Rust type generates it any more)"
            ));
        }
    }
    if lines.is_empty() {
        None
    } else {
        lines.sort();
        Some(lines.join("\n"))
    }
}

/// Every workspace member that produces a cdylib and depends on a `*-plugin`
/// crate — the shape a bundled plugin has, rather than a list of their names.
///
/// # The shell is a member; the plugin crate need not be
///
/// Those two are asked separately, because they answer differently once an
/// adapter moves into its own repository.
///
/// The SHELL has to stay a workspace member. Nothing can depend on a cdylib, so
/// it is a leaf cargo builds only by virtue of membership; if it moved out too,
/// no repository would ever build it.
///
/// The `-plugin` rlib behind it can come from anywhere, and its `plugin.json`
/// comes with it — a git dependency is checked out under
/// `~/.cargo/git/checkouts/…`, manifest and all, and cargo reports that path
/// like any other. So the directory map is built from EVERY package in the
/// graph while the shell scan stays on members. Reading both from
/// `--no-deps` was the one line that made staging refuse an out-of-tree
/// adapter; it is also why that refusal was written to say what it needed.
fn discover(metadata: &serde_json::Value) -> Result<Vec<Bundled>, String> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?;

    // Where each package lives, so a `-plugin` dependency can be turned into the
    // directory holding its manifest — wherever cargo put it.
    let dirs: std::collections::BTreeMap<&str, PathBuf> = packages
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?;
            let manifest = Path::new(p["manifest_path"].as_str()?);
            Some((name, manifest.parent()?.to_path_buf()))
        })
        .collect();

    // The shells, and only the shells. With dependencies in the graph,
    // `packages` holds every crate the build touches; a cdylib among them that
    // this workspace does not own is not something to stage.
    let members: std::collections::BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .ok_or("cargo metadata has no workspace_members")?
        .iter()
        .filter_map(|id| id.as_str())
        .collect();

    let mut found = Vec::new();
    for package in packages {
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        if !package["id"]
            .as_str()
            .is_some_and(|id| members.contains(id))
        {
            continue;
        }
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
/// Every adapter the mobile build links must also be one the desktop stages,
/// and the reverse.
///
/// `discover` asks the workspace what exists, which is the right question and
/// has one blind spot: an adapter that LEAVES the workspace stops existing, so
/// discovery finds eleven instead of twelve, stages eleven, and reports
/// success. The release artifact then ships without that adapter and every gate
/// is green — the exact silence that moving an adapter into its own repository
/// is going to cause, on the day it is least expected.
///
/// So discovery is checked against a list that does NOT come from workspace
/// membership: `host-plugins`' `static` feature, which names the twelve
/// adapters the mobile host links statically. That list is not a tally kept for
/// this check — it is load-bearing already (`cal-ffi` enables those features one
/// by one to drop adapters it does not ship), so it cannot rot quietly.
///
/// Using it here buys a second thing beyond the blind spot: the two platforms
/// are made to agree. An adapter added to one side and forgotten on the other
/// now fails, by name, in whichever direction it happened.
fn check_against_what_the_repo_declares(
    bundled: &[Bundled],
    metadata: &serde_json::Value,
) -> Result<(), String> {
    let features = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"].as_str() == Some("host-plugins"))
        .map(|p| &p["features"])
        .ok_or("no `host-plugins` package in cargo metadata — that is where the mobile host declares which adapters it links")?;

    // `static = ["caldav", "ical", …]`, and each of those is
    // `caldav = ["registry", "dep:adapter-caldav-plugin"]`. The `dep:` entry is
    // the plugin crate, which is the name discovery works in too.
    let declared: BTreeSet<String> = features["static"]
        .as_array()
        .ok_or("`host-plugins` has no `static` feature; the mobile host no longer declares its adapters in the place this check reads")?
        .iter()
        .filter_map(|f| f.as_str())
        .flat_map(|feature| {
            features[feature]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e.as_str())
                .filter_map(|e| e.strip_prefix("dep:"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    if declared.is_empty() {
        return Err("`host-plugins`' `static` feature names no adapter crates; this check would pass on anything".to_string());
    }

    let staged: BTreeSet<String> = bundled
        .iter()
        .map(|b| {
            b.plugin_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect();

    let declared_only: Vec<&String> = declared.difference(&staged).collect();
    let staged_only: Vec<&String> = staged.difference(&declared).collect();
    if declared_only.is_empty() && staged_only.is_empty() {
        return Ok(());
    }

    let mut message = String::from("the workspace and the declared adapter list disagree");
    for name in declared_only {
        message.push_str(&format!(
            "\n  {name}: declared by `host-plugins`' `static` feature, but no workspace cdylib \
             stages it. If this adapter moved to its own repository, stage-plugins has to be \
             taught where to find it — leaving it out would ship a build without it, quietly"
        ));
    }
    for name in staged_only {
        message.push_str(&format!(
            "\n  {name}: staged from the workspace, but `host-plugins`' `static` feature does not \
             name it. Either add it there, or it is an adapter nothing declares"
        ));
    }
    Err(message)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pair(name: &str, body: &str) -> (String, String) {
        (name.to_string(), body.to_string())
    }

    /// The drift report NAMES what moved. A count ("31 files, expected 32")
    /// leaves the reader to find which one, which is how a guard becomes a
    /// thing people re-run until it goes green.
    #[test]
    fn drift_names_every_file_that_moved() {
        let have = vec![
            pair("Task.ts", "old"),
            pair("Ghost.ts", "left over"),
            pair("Section.ts", "same"),
        ];
        let fresh = vec![
            pair("Task.ts", "new"),
            pair("Section.ts", "same"),
            pair("Weekday.ts", "added"),
        ];
        let report = describe_drift(&have, &fresh).expect("three of these differ");
        assert!(report.contains("outdated: Task.ts"), "{report}");
        assert!(report.contains("missing:  Weekday.ts"), "{report}");
        assert!(report.contains("stale:    Ghost.ts"), "{report}");
        assert!(
            !report.contains("Section.ts"),
            "an unchanged file has nothing to report: {report}"
        );
    }

    #[test]
    fn an_identical_tree_is_no_drift() {
        let files = vec![pair("Task.ts", "x"), pair("Section.ts", "y")];
        assert!(describe_drift(&files, &files).is_none());
    }

    /// Line endings are not drift. A Windows checkout can hand back CRLF where
    /// the generator wrote LF; without normalising, every file would read as
    /// outdated on every Windows run.
    #[test]
    fn crlf_in_the_checkout_is_not_drift() {
        let dir = std::env::temp_dir().join(format!("aperio-ts-crlf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("Task.ts"), "export type Task = {\r\n};\r\n").expect("write");
        // A file that is not TypeScript is not part of the answer.
        fs::write(dir.join("notes.txt"), "ignore me").expect("write");
        let read = read_ts_dir(&dir).expect("read");
        assert_eq!(read, vec![pair("Task.ts", "export type Task = {\n};\n")]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A crate that declares `ts-export` is one of the sources; every other
    /// workspace member is not, and a package outside the workspace never is.
    #[test]
    fn the_sources_are_the_crates_that_declare_the_feature() {
        let meta = json!({
            "workspace_members": ["cal-core 0.1.0 (path+file:///c)", "aperio-db 0.1.0 (path+file:///d)"],
            "packages": [
                {"id": "cal-core 0.1.0 (path+file:///c)", "name": "cal-core",
                 "features": {"ts-export": ["dep:ts-rs"]}},
                {"id": "aperio-db 0.1.0 (path+file:///d)", "name": "aperio-db",
                 "features": {}},
                // Not a member: a dependency that happens to have such a feature.
                {"id": "elsewhere 1.0.0 (registry+…)", "name": "elsewhere",
                 "features": {"ts-export": []}},
            ]
        });
        assert_eq!(
            ts_export_members(&meta).unwrap(),
            vec!["cal-core".to_string()]
        );
    }

    /// Nobody declaring it is an ERROR, not an empty success. Otherwise a
    /// renamed feature leaves `shared/generated/` frozen while the check keeps
    /// reporting that it is current.
    #[test]
    fn no_source_at_all_is_an_error() {
        let meta = json!({
            "workspace_members": ["aperio-db 0.1.0 (path+file:///d)"],
            "packages": [
                {"id": "aperio-db 0.1.0 (path+file:///d)", "name": "aperio-db", "features": {}},
            ]
        });
        let err = ts_export_members(&meta).unwrap_err();
        assert!(err.contains("ts-export"), "{err}");
    }

    /// A crate directory with a `plugin.json` in it, as cargo would report.
    ///
    /// Real directories rather than a fabricated path: `discover` READS the
    /// manifest to learn the plugin id, so a test that only checks path
    /// arithmetic would not be testing the thing that broke.
    fn crate_dir(root: &Path, rel: &str, id: &str) -> PathBuf {
        let dir = root.join(rel);
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            dir.join("plugin.json"),
            format!(r#"{{"id":"{id}","name":"X","version":"0.1.0"}}"#),
        )
        .expect("write plugin.json");
        dir
    }

    /// A scratch root that cleans itself up.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("aperio-xtask-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("mkdir scratch");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const SHELL_ID: &str = "path+file:///repo/crates/adapter-x-cdylib#0.1.0";

    fn metadata_for(shell_dir: &Path, plugin_dir: &Path, plugin_id: &str) -> serde_json::Value {
        json!({
            "workspace_members": [SHELL_ID],
            "packages": [
                {
                    "name": "adapter-x-cdylib",
                    "id": SHELL_ID,
                    "manifest_path": shell_dir.join("Cargo.toml").to_string_lossy(),
                    "targets": [{ "crate_types": ["cdylib"] }],
                    "dependencies": [
                        { "name": "plugin-sdk", "kind": null },
                        { "name": "adapter-x-plugin", "kind": null },
                    ],
                },
                {
                    "name": "adapter-x-plugin",
                    "id": plugin_id,
                    "manifest_path": plugin_dir.join("Cargo.toml").to_string_lossy(),
                    "targets": [{ "crate_types": ["rlib"] }],
                    "dependencies": [{ "name": "plugin-sdk", "kind": null }],
                },
            ],
        })
    }

    /// The case that already worked, kept so the change is a widening rather
    /// than a swap.
    #[test]
    fn a_shell_finds_a_plugin_crate_in_the_same_workspace() {
        let scratch = Scratch::new("same-workspace");
        let shell = crate_dir(&scratch.0, "crates/adapter-x-cdylib", "unused");
        let plugin = crate_dir(&scratch.0, "crates/adapter-x-plugin", "com.example.x");

        let found = discover(&metadata_for(
            &shell,
            &plugin,
            "path+file:///repo/crates/adapter-x-plugin#0.1.0",
        ))
        .expect("discovery");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].plugin_id, "com.example.x");
        assert_eq!(found[0].plugin_dir, plugin);
    }

    /// The case the extraction creates, and the reason `metadata()` stopped
    /// passing `--no-deps`.
    ///
    /// The shell stays a workspace member — a cdylib is a leaf nothing can
    /// depend on, so it has to. The rlib behind it comes from another
    /// repository, checked out by cargo with its `plugin.json` beside its
    /// `Cargo.toml`. Read the graph without dependencies and that crate has no
    /// directory at all as far as this task is concerned, and staging refuses
    /// the very adapter it exists to stage.
    #[test]
    fn a_shell_finds_a_plugin_crate_that_lives_in_another_repository() {
        let scratch = Scratch::new("another-repo");
        let shell = crate_dir(&scratch.0, "crates/adapter-x-cdylib", "unused");
        // The shape cargo really produces for a git dependency.
        let plugin = crate_dir(
            &scratch.0,
            "git/checkouts/adapter-x-abc123/deadbee/crates/adapter-x-plugin",
            "com.example.x",
        );

        let found = discover(&metadata_for(
            &shell,
            &plugin,
            "git+file:///elsewhere?branch=main#adapter-x-plugin@0.1.0",
        ))
        .expect(
            "an out-of-tree plugin crate has a directory too — cargo's git \
             checkout — and its plugin.json is in it",
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].plugin_id, "com.example.x");
        assert!(
            found[0]
                .plugin_dir
                .components()
                .any(|c| c.as_os_str() == "checkouts"),
            "the manifest should be read from cargo's checkout, got {}",
            found[0].plugin_dir.display(),
        );
    }

    /// A cdylib that is not ours is not a plugin shell.
    ///
    /// With dependencies in the graph the package list holds every crate the
    /// build touches, so the scan has to say which ones this workspace owns
    /// instead of assuming the list already is the workspace. Without that,
    /// widening the graph would start staging other people's libraries.
    #[test]
    fn a_cdylib_from_a_dependency_is_not_mistaken_for_a_shell() {
        let scratch = Scratch::new("foreign-cdylib");
        let shell = crate_dir(&scratch.0, "crates/adapter-x-cdylib", "unused");
        let plugin = crate_dir(&scratch.0, "crates/adapter-x-plugin", "com.example.x");
        let stranger = crate_dir(&scratch.0, "registry/stranger-cdylib", "com.stranger.x");

        let mut metadata = metadata_for(
            &shell,
            &plugin,
            "path+file:///repo/crates/adapter-x-plugin#0.1.0",
        );
        // Identical in every respect except membership.
        metadata["packages"].as_array_mut().unwrap().push(json!({
            "name": "stranger-cdylib",
            "id": "registry+https://github.com/rust-lang/crates.io-index#stranger-cdylib@1.0.0",
            "manifest_path": stranger.join("Cargo.toml").to_string_lossy(),
            "targets": [{ "crate_types": ["cdylib"] }],
            "dependencies": [
                { "name": "plugin-sdk", "kind": null },
                { "name": "adapter-x-plugin", "kind": null },
            ],
        }));

        let found = discover(&metadata).expect("discovery");
        assert_eq!(
            found.len(),
            1,
            "only this workspace's own shell should be staged, got {:?}",
            found.iter().map(|b| &b.cdylib_crate).collect::<Vec<_>>(),
        );
        assert_eq!(found[0].cdylib_crate, "adapter-x-cdylib");
    }
}
