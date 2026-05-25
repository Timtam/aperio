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
            "plugin_type": "calendar-adapter",
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
