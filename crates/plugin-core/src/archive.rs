//! `.aperio` community-plugin archive support (DESIGN.md §20.7).
//!
//! A `.aperio` file is a ZIP archive that contains, at minimum:
//!
//!   - `plugin.json` — the manifest, parsed verbatim by the
//!     manager. Identical to the `plugin.json` shipped next to
//!     a bundled plugin's cdylib.
//!   - One or more platform-specific shared libraries. The
//!     bundled plugins ship as `<plugin_id>.{dll,dylib,so}`;
//!     community archives should follow the same naming so
//!     [`crate::manager::locate_library`]-style lookups find
//!     them without a fallback scan.
//!
//! Two entry points:
//!
//!   - [`inspect_archive`] — cheap, read-only. Opens the zip,
//!     extracts `plugin.json` into memory, validates the
//!     basics. The install dialog uses this to render the
//!     "Plugin installieren?" preview before the user
//!     commits.
//!   - [`install_archive`] — extracts every entry into
//!     `<target_root>/<plugin_id>/`. The host then calls
//!     `PluginManager::load_from_dir` against that path. Any
//!     pre-existing directory under the same id is removed
//!     first so the install is a clean replace (the update
//!     flow in §20.9 routes through the same path).
//!
//! Signature verification is intentionally NOT implemented —
//! per DESIGN.md §20.4 + the install dialog spec in §20.7,
//! every community plugin is treated as unsigned in this
//! phase. The host surfaces the "unsigned, install from
//! trusted sources only" warning via its own UI.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::error::{PluginError, PluginResult};
use crate::manifest::{PluginManifest, MANIFEST_FILENAME};

/// The modification time stamped into every archive entry: the zip epoch,
/// 1980-01-01, which is what `zip::DateTime::default()` is.
///
/// Fixed rather than the file's own mtime, so the same directory always packs
/// to the same bytes. Two builds of one commit are then comparable, and a
/// rebuild that changed nothing does not look like a new release.
fn archive_timestamp() -> zip::DateTime {
    zip::DateTime::default()
}

/// Build a `.aperio` archive out of a staged plugin directory.
///
/// The counterpart to [`install_archive`], and deliberately its mirror image:
/// installing extracts an archive into `<root>/<plugin-id>/`, and a staged
/// bundled plugin already IS that directory — `plugin.json` beside
/// `<plugin-id>.{dll,dylib,so}`. So packing is "zip this directory" and the two
/// halves cannot drift into different ideas of the layout.
///
/// Until this existed the format had a complete reader and no writer at all:
/// `inspect_archive`, `install_archive`, the path-traversal guard, the install
/// dialog — all of it against a shape that nothing in the tree produced. The
/// only `.aperio` file that had ever existed was built by a test helper. An
/// adapter author's first archive would have been the first real one.
///
/// Returns the manifest it packed, parsed and validated. Two refusals, for
/// different reasons. A directory with no `plugin.json` would pack into an
/// archive the reader rejects outright — nothing is lost by saying so at the
/// packing end, where the person who can fix it is standing. One with no
/// library is worse: it installs cleanly and loads nothing, and
/// `PluginManager::load_from_dir` reports that as one failed load among others,
/// on the user's machine, after they chose to install it.
pub fn pack_archive(
    plugin_dir: impl AsRef<Path>,
    dest: impl AsRef<Path>,
) -> PluginResult<PluginManifest> {
    let plugin_dir = plugin_dir.as_ref();
    let dest = dest.as_ref();

    let manifest_path = plugin_dir.join(MANIFEST_FILENAME);
    let manifest_bytes = fs::read(&manifest_path).map_err(|e| {
        PluginError::Io(format!(
            "read {}: {e} — a plugin directory is its manifest plus its library",
            manifest_path.display(),
        ))
    })?;
    let manifest = PluginManifest::from_bytes(&manifest_bytes)?;

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    let dir = fs::read_dir(plugin_dir)
        .map_err(|e| PluginError::Io(format!("read {}: {e}", plugin_dir.display())))?;
    for entry in dir {
        let entry = entry.map_err(|e| PluginError::Io(format!("read dir entry: {e}")))?;
        let path = entry.path();
        // One level, no recursion: the layout `install_archive` writes is flat,
        // and a nested tree here would be one this reader has never seen.
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return Err(PluginError::Io(format!(
                "{} has a name that is not valid UTF-8, which a zip entry must be",
                path.display(),
            )));
        };
        entries.push((name.to_string(), path));
    }
    // Sorted so the same directory always produces byte-identical archives.
    // Whoever compares two builds should be comparing the plugins, not the
    // order a filesystem happened to enumerate them in.
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let has_library = entries.iter().any(|(name, _)| {
        Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "dll" | "dylib" | "so"))
    });
    if !has_library {
        return Err(PluginError::Manifest(format!(
            "{} holds no shared library, so the archive would install and load \
             nothing. Stage the plugin first — see `cargo xtask stage-plugins`",
            plugin_dir.display(),
        )));
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| PluginError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    let file = fs::File::create(dest)
        .map_err(|e| PluginError::Io(format!("create {}: {e}", dest.display())))?;
    let mut writer = zip::ZipWriter::new(file);
    // Deflate: an adapter's cdylib is ten megabytes of mostly-compressible
    // code, and this is a file someone downloads.
    //
    // The timestamp is pinned to the zip epoch, and that is what makes two
    // packs of one directory byte-identical. Left at its default it is not:
    // `SimpleFileOptions::default()` calls the zip crate's
    // `DateTime::default_for_write`, which returns the WALL CLOCK when that
    // crate's `time` feature is on and 1980 when it is not. `time` is in its
    // default set; this workspace happens to switch defaults off for an
    // unrelated reason, and cargo unifies features across the whole graph — so
    // one new dependency asking for `zip` with defaults would silently start
    // stamping the hour into every archive. Naming it here does not depend on
    // anyone noticing that.
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(archive_timestamp());
    for (name, path) in &entries {
        writer
            .start_file(name.as_str(), options)
            .map_err(zip_to_plugin_error)?;
        let bytes =
            fs::read(path).map_err(|e| PluginError::Io(format!("read {}: {e}", path.display())))?;
        io::Write::write_all(&mut writer, &bytes)
            .map_err(|e| PluginError::Io(format!("write {name} into {}: {e}", dest.display())))?;
    }
    writer.finish().map_err(zip_to_plugin_error)?;
    Ok(manifest)
}

/// Read + parse the `plugin.json` from a `.aperio` archive
/// without writing anything to disk. Used by the install
/// dialog to render the preview + perform the ABI / min-app-
/// version checks before the user confirms.
pub fn inspect_archive(archive_path: impl AsRef<Path>) -> PluginResult<PluginManifest> {
    let manifest_bytes = read_manifest_bytes(archive_path.as_ref())?;
    PluginManifest::from_bytes(&manifest_bytes)
}

/// Extract every entry from a `.aperio` archive into
/// `<target_root>/<plugin_id>/`. The plugin id is read from
/// the manifest first; if a directory with that id already
/// exists under `target_root` it gets removed before
/// extraction so the install is a clean replace (the update
/// flow in §20.9 routes through the same path — install over
/// an existing id IS the update verb).
///
/// Returns the absolute path of the plugin's freshly-staged
/// directory; the host immediately follows up with
/// `PluginManager::load_from_dir` against it.
pub fn install_archive(
    archive_path: impl AsRef<Path>,
    target_root: impl AsRef<Path>,
) -> PluginResult<InstalledArchive> {
    let archive_path = archive_path.as_ref();
    let target_root = target_root.as_ref();
    let manifest_bytes = read_manifest_bytes(archive_path)?;
    let manifest = PluginManifest::from_bytes(&manifest_bytes)?;
    let plugin_dir = target_root.join(&manifest.id);

    // Clean-replace: if the dir exists we wipe it first so a
    // re-install / update lands on the same path without
    // stale files from the previous version. This is
    // deliberately destructive — the host's command surface
    // should only call this fn after the user confirms.
    if plugin_dir.exists() {
        fs::remove_dir_all(&plugin_dir).map_err(|e| {
            PluginError::Io(format!(
                "remove existing plugin dir {}: {e}",
                plugin_dir.display()
            ))
        })?;
    }
    fs::create_dir_all(&plugin_dir)
        .map_err(|e| PluginError::Io(format!("mkdir {}: {e}", plugin_dir.display())))?;

    // Open + iterate.
    let file = fs::File::open(archive_path)
        .map_err(|e| PluginError::Io(format!("open {}: {e}", archive_path.display(),)))?;
    let mut zip = zip::ZipArchive::new(file).map_err(zip_to_plugin_error)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(zip_to_plugin_error)?;
        // Defensive: zip's `enclosed_name` strips leading
        // separators + rejects anything containing `..` so a
        // hostile archive can't write outside the plugin dir.
        let Some(rel_path) = entry.enclosed_name() else {
            return Err(PluginError::Manifest(format!(
                "archive contains unsafe path: {}",
                entry.name(),
            )));
        };
        // Skip empty path-only entries (some zip writers emit
        // these as separators).
        if rel_path.as_os_str().is_empty() {
            continue;
        }
        let out_path = plugin_dir.join(&rel_path);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| PluginError::Io(format!("mkdir {}: {e}", out_path.display(),)))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| PluginError::Io(format!("mkdir {}: {e}", parent.display(),)))?;
        }
        let mut out_file = fs::File::create(&out_path)
            .map_err(|e| PluginError::Io(format!("create {}: {e}", out_path.display(),)))?;
        io::copy(&mut entry, &mut out_file)
            .map_err(|e| PluginError::Io(format!("write {}: {e}", out_path.display(),)))?;
    }

    Ok(InstalledArchive {
        plugin_dir,
        manifest,
    })
}

/// Result of a successful [`install_archive`] call.
#[derive(Debug, Clone)]
pub struct InstalledArchive {
    /// Absolute path of the freshly-extracted plugin
    /// directory (`<target_root>/<plugin_id>/`). The host
    /// hands this straight to
    /// [`crate::manager::PluginManager::load_from_dir`].
    pub plugin_dir: PathBuf,
    /// Parsed manifest from the archive. Hands the install
    /// command's success path a populated PluginInfo without
    /// re-parsing.
    pub manifest: PluginManifest,
}

/// Internal helper: open the archive, locate + read the
/// `plugin.json` into a Vec, return the bytes. Shared by
/// [`inspect_archive`] (which stops here) and
/// [`install_archive`] (which uses the bytes to early-validate
/// the manifest before laying down any files).
fn read_manifest_bytes(archive_path: &Path) -> PluginResult<Vec<u8>> {
    let file = fs::File::open(archive_path)
        .map_err(|e| PluginError::Io(format!("open {}: {e}", archive_path.display(),)))?;
    let mut zip = zip::ZipArchive::new(file).map_err(zip_to_plugin_error)?;
    let mut manifest_entry = zip.by_name(MANIFEST_FILENAME).map_err(|err| {
        if matches!(err, zip::result::ZipError::FileNotFound) {
            PluginError::Manifest(format!("archive is missing the {MANIFEST_FILENAME} entry",))
        } else {
            zip_to_plugin_error(err)
        }
    })?;
    let mut bytes = Vec::with_capacity(manifest_entry.size() as usize);
    manifest_entry
        .read_to_end(&mut bytes)
        .map_err(|e| PluginError::Io(format!("read {MANIFEST_FILENAME}: {e}")))?;
    Ok(bytes)
}

fn zip_to_plugin_error(err: zip::result::ZipError) -> PluginError {
    use zip::result::ZipError::*;
    match err {
        Io(io) => PluginError::Io(io.to_string()),
        InvalidArchive(msg) | UnsupportedArchive(msg) => {
            PluginError::Manifest(format!("invalid plugin archive: {msg}"))
        }
        FileNotFound => PluginError::Manifest("archive missing expected entry".to_string()),
        _ => PluginError::Manifest(format!("zip error: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    /// Build a minimal valid manifest body for tests.
    fn sample_manifest_json() -> Vec<u8> {
        br#"{
            "id": "com.example.test-plugin",
            "name": "Test Plugin",
            "version": "1.0.0",
            "plugin_type": "adapter",
            "capabilities": ["calendar"],
            "abi_version": 1,
            "min_app_version": "0.1.0",
            "author": "Tester"
        }"#
        .to_vec()
    }

    /// Construct an in-memory ZIP archive on disk that mimics
    /// the .aperio shape: plugin.json at the root + one
    /// dummy shared-library entry per platform suffix.
    fn make_archive(path: &Path, manifest: &[u8]) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file(MANIFEST_FILENAME, opts).unwrap();
        writer.write_all(manifest).unwrap();
        writer
            .start_file("com.example.test-plugin.dll", opts)
            .unwrap();
        writer.write_all(b"dummy windows cdylib").unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn inspect_returns_parsed_manifest() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.aperio");
        make_archive(&path, &sample_manifest_json());
        let manifest = inspect_archive(&path).expect("inspect should succeed");
        assert_eq!(manifest.id, "com.example.test-plugin");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.author.as_deref(), Some("Tester"));
    }

    /// The two halves are one shape: what `pack_archive` writes is what
    /// `install_archive` lays back down, file for file.
    ///
    /// The format had a reader and no writer, so this is the first thing that
    /// has ever asked whether the two agree.
    #[test]
    fn packing_a_plugin_dir_round_trips_through_installing_it() {
        let dir = tempdir().unwrap();
        let staged = dir.path().join("com.example.test-plugin");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join(MANIFEST_FILENAME), sample_manifest_json()).unwrap();
        fs::write(staged.join("com.example.test-plugin.dll"), b"a library").unwrap();
        fs::write(staged.join("README.txt"), b"carried along").unwrap();

        let archive = dir.path().join("plugin.aperio");
        let packed = pack_archive(&staged, &archive).expect("packs");
        assert_eq!(packed.id, "com.example.test-plugin");

        // The reader agrees about the manifest without unpacking anything.
        let inspected = inspect_archive(&archive).expect("inspects");
        assert_eq!(inspected.id, packed.id);
        assert_eq!(inspected.version, packed.version);

        let root = dir.path().join("installed");
        let installed = install_archive(&archive, &root).expect("installs");
        assert_eq!(installed.plugin_dir, root.join("com.example.test-plugin"));

        // Every file, and its contents — not just the two the format names.
        for name in [
            "com.example.test-plugin.dll",
            "README.txt",
            MANIFEST_FILENAME,
        ] {
            let before = fs::read(staged.join(name)).unwrap();
            let after = fs::read(installed.plugin_dir.join(name))
                .unwrap_or_else(|e| panic!("{name} did not survive the round trip: {e}"));
            assert_eq!(before, after, "{name} changed on the way through");
        }
    }

    /// A directory with a manifest and no library packs into an archive that
    /// installs cleanly and loads nothing.
    ///
    /// Refused here rather than discovered there: the host reports a missing
    /// library as one failed load among others, on the user's machine, after
    /// they chose to install it.
    #[test]
    fn packing_refuses_a_plugin_with_no_library() {
        let dir = tempdir().unwrap();
        let staged = dir.path().join("com.example.test-plugin");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join(MANIFEST_FILENAME), sample_manifest_json()).unwrap();

        let err = pack_archive(&staged, dir.path().join("plugin.aperio"))
            .expect_err("a manifest alone is not a plugin");
        assert!(
            matches!(&err, PluginError::Manifest(m) if m.contains("no shared library")),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn packing_refuses_a_directory_with_no_manifest() {
        let dir = tempdir().unwrap();
        let staged = dir.path().join("com.example.test-plugin");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("com.example.test-plugin.dll"), b"a library").unwrap();

        let err = pack_archive(&staged, dir.path().join("plugin.aperio"))
            .expect_err("a library alone is not a plugin either");
        assert!(
            matches!(err, PluginError::Io(_)),
            "unexpected error: {err:?}"
        );
    }

    /// Packing the same directory twice produces the same bytes.
    ///
    /// So two builds can be compared, and so a rebuild that changes nothing
    /// does not look like a new release.
    #[test]
    fn packing_is_deterministic() {
        let dir = tempdir().unwrap();
        let staged = dir.path().join("com.example.test-plugin");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join(MANIFEST_FILENAME), sample_manifest_json()).unwrap();
        fs::write(staged.join("com.example.test-plugin.dll"), b"a library").unwrap();
        fs::write(staged.join("a-second-file.txt"), b"and another").unwrap();

        let first = dir.path().join("first.aperio");
        let second = dir.path().join("second.aperio");
        pack_archive(&staged, &first).unwrap();
        pack_archive(&staged, &second).unwrap();
        assert_eq!(
            fs::read(&first).unwrap(),
            fs::read(&second).unwrap(),
            "the same directory packed twice should be byte-identical",
        );

        // Comparing two packs is nearly useless on its own: they happen
        // microseconds apart, and a zip timestamp has two-second granularity,
        // so a writer stamping the wall clock would land both in the same
        // bucket and pass this about 1999 times in 2000. What actually has to
        // hold is that no clock is consulted at all — so the stamp is read back
        // and checked against the epoch it is pinned to.
        let file = fs::File::open(&first).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        assert!(zip.len() >= 2, "the fixture packs a manifest and a library");
        for i in 0..zip.len() {
            let entry = zip.by_index(i).unwrap();
            let stamped = entry.last_modified().expect("every entry carries a stamp");
            assert_eq!(
                (stamped.year(), stamped.month(), stamped.day()),
                (1980, 1, 1),
                "{} was stamped {stamped:?} — something is reading the clock, and \
                 two builds of one commit have stopped being comparable",
                entry.name(),
            );
        }
    }

    #[test]
    fn inspect_rejects_archive_without_manifest() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bogus.aperio");
        let file = fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        writer.start_file("README.md", opts).unwrap();
        writer.write_all(b"no manifest").unwrap();
        writer.finish().unwrap();
        let err = inspect_archive(&path).expect_err("manifest-less archive");
        assert!(matches!(err, PluginError::Manifest(_)));
    }

    #[test]
    fn inspect_rejects_malformed_manifest_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.aperio");
        make_archive(&path, b"{not json");
        let err = inspect_archive(&path).expect_err("bad manifest");
        // PluginManifest::from_bytes wraps serde errors as
        // Manifest variants.
        assert!(matches!(err, PluginError::Manifest(_)));
    }

    #[test]
    fn install_extracts_under_plugin_id() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("test.aperio");
        make_archive(&archive_path, &sample_manifest_json());

        let target_root = dir.path().join("user_plugins");
        fs::create_dir_all(&target_root).unwrap();
        let installed =
            install_archive(&archive_path, &target_root).expect("install should succeed");
        assert_eq!(installed.manifest.id, "com.example.test-plugin");
        assert_eq!(
            installed.plugin_dir,
            target_root.join("com.example.test-plugin"),
        );
        assert!(installed.plugin_dir.join(MANIFEST_FILENAME).is_file());
        assert!(installed
            .plugin_dir
            .join("com.example.test-plugin.dll")
            .is_file());
    }

    #[test]
    fn install_replaces_existing_plugin_dir() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("test.aperio");
        make_archive(&archive_path, &sample_manifest_json());

        let target_root = dir.path().join("user_plugins");
        fs::create_dir_all(&target_root).unwrap();
        let existing = target_root.join("com.example.test-plugin");
        fs::create_dir_all(&existing).unwrap();
        // Stale file from a previous install that should not
        // survive the re-install.
        fs::write(existing.join("OLD.txt"), b"stale").unwrap();

        install_archive(&archive_path, &target_root).expect("install ok");
        assert!(
            !existing.join("OLD.txt").exists(),
            "old files must be removed before extraction",
        );
        assert!(existing.join(MANIFEST_FILENAME).is_file());
    }

    /// Hostile archive containing a `..`-prefixed entry name
    /// must not be allowed to write outside the plugin dir.
    /// `zip`'s `enclosed_name` already filters these; we just
    /// confirm the surface error we surface to callers.
    #[test]
    fn install_rejects_path_traversal() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("evil.aperio");
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        writer.start_file(MANIFEST_FILENAME, opts).unwrap();
        writer.write_all(&sample_manifest_json()).unwrap();
        // Path-traversal entry. zip-rs canonicalises this and
        // enclosed_name returns None, which we map to an
        // explicit Manifest error.
        writer.start_file("../escape.txt", opts).unwrap();
        writer.write_all(b"hostile").unwrap();
        writer.finish().unwrap();

        let target_root = dir.path().join("user_plugins");
        fs::create_dir_all(&target_root).unwrap();
        let err =
            install_archive(&archive_path, &target_root).expect_err("traversal must be rejected");
        match err {
            PluginError::Manifest(msg) => {
                assert!(msg.contains("unsafe path"), "got: {msg}");
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }
}
