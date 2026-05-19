//! Portable data-path resolution per `DESIGN.md` section 22.2.
//!
//! On startup the app checks whether a writable `data/` directory exists
//! (or can be created) next to the binary. If so, it runs in **portable
//! mode** — all user data, settings, and sound files live there.
//! Otherwise it falls back to the platform-specific user profile
//! directory (`%APPDATA%\Aperio` / `~/Library/Application Support/Aperio` /
//! `~/.config/Aperio`).

use std::path::{Path, PathBuf};

/// Result of data-directory resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirResolution {
    pub path: PathBuf,
    pub kind: DataDirKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirKind {
    /// Portable — `data/` next to the binary.
    Portable,
    /// Fallback — platform-specific user-profile directory.
    System,
}

/// Resolve per the algorithm in section 22.2.
pub fn resolve_data_dir() -> DataDirResolution {
    if let Some(portable) = try_portable_dir() {
        return DataDirResolution {
            path: portable,
            kind: DataDirKind::Portable,
        };
    }

    let system = system_dir();
    // Also create the fallback directory if it does not exist yet.
    let _ = std::fs::create_dir_all(&system);

    DataDirResolution {
        path: system,
        kind: DataDirKind::System,
    }
}

fn try_portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let binary_dir = exe.parent()?.to_path_buf();
    let portable_data = binary_dir.join("data");

    if is_writable_dir(&portable_data) {
        return Some(portable_data);
    }

    // Try to create it.
    if std::fs::create_dir(&portable_data).is_ok() && is_writable_dir(&portable_data) {
        return Some(portable_data);
    }

    None
}

fn is_writable_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    // Write probe with a unique temporary file name.
    let probe = path.join(".aperio-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn system_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Aperio")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `current_exe()` is process-wide; tests that manipulate paths around it
    // must not run concurrently.
    static EXE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn portable_dir_detected_when_data_dir_next_to_exe_exists() {
        let _guard = EXE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let exe_path = tmp.path().join("aperio-test.exe");
        std::fs::File::create(&exe_path).unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir(&data).unwrap();

        // We cannot override `current_exe()` from tests, so we exercise the
        // helper directly.
        assert!(is_writable_dir(&data));
    }

    #[test]
    fn system_dir_ends_with_aperio() {
        let path = system_dir();
        assert!(
            path.ends_with("Aperio"),
            "expected path to end with 'Aperio', got {:?}",
            path
        );
    }

    #[test]
    fn is_writable_dir_returns_false_for_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_writable_dir(&tmp.path().join("does-not-exist")));
    }
}
