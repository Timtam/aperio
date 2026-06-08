//! Device-local window-state persistence (DESIGN.md §15.3).
//!
//! The main window's size + position are remembered across restarts in
//! `app_config.json` inside the resolved data directory (the portable
//! `data/` next to the binary, or the per-user fallback — always via
//! [`crate::paths::resolve_data_dir`], never an ad-hoc path).
//!
//! This is deliberately NOT a synced `user_prefs` value: a window rectangle
//! captured on one monitor layout is meaningless on another device.
//!
//! Mechanics: the latest geometry is kept in an in-memory [`Store`] that the
//! window-event handler refreshes on every move/resize (cheap, no I/O), and
//! is flushed to disk on close. The stored size is always the last
//! *non-maximized* rectangle, so un-maximizing after a restore returns to a
//! sensible size; a separate `maximized` flag drives re-maximizing on start.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow, Window};

use crate::paths::resolve_data_dir;

const CONFIG_FILE: &str = "app_config.json";
/// Never restore a window smaller than this — guards against a corrupt file
/// trapping the user in an unusable sliver.
const MIN_W: u32 = 320;
const MIN_H: u32 = 240;

/// Saved outer geometry of the main window, in physical pixels.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    #[serde(default)]
    pub maximized: bool,
}

/// On-disk shape of `app_config.json`. A wrapper struct (rather than a bare
/// geometry) so future device-local settings can join without a format break.
#[derive(Debug, Default, Serialize, Deserialize)]
struct AppConfig {
    #[serde(default)]
    window: Option<WindowGeometry>,
}

/// In-memory latest geometry, registered in Tauri state. Refreshed on every
/// move/resize; flushed to disk on close.
pub type Store = Mutex<Option<WindowGeometry>>;

fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

/// Read the saved geometry from `app_config.json` in `dir`. `None` when the
/// file is absent or unparseable (first run, corrupt file).
pub fn load_from(dir: &Path) -> Option<WindowGeometry> {
    let raw = std::fs::read_to_string(config_path(dir)).ok()?;
    serde_json::from_str::<AppConfig>(&raw).ok()?.window
}

/// Write `geom` to `app_config.json` in `dir`.
pub fn save_to(dir: &Path, geom: &WindowGeometry) -> std::io::Result<()> {
    let cfg = AppConfig {
        window: Some(*geom),
    };
    let json = serde_json::to_string_pretty(&cfg).map_err(std::io::Error::other)?;
    std::fs::write(config_path(dir), json)
}

/// Load from the resolved data directory.
pub fn load() -> Option<WindowGeometry> {
    load_from(&resolve_data_dir().path)
}

/// Persist to the resolved data directory (best effort).
pub fn save(geom: &WindowGeometry) {
    let _ = save_to(&resolve_data_dir().path, geom);
}

/// Refresh the in-memory [`Store`] from the live geometry of `window`. Skips
/// while minimized (the geometry is then meaningless) and keeps the last
/// non-maximized size as the restore size while still tracking the maximized
/// flag. No-ops if the store isn't registered yet (an event arriving before
/// setup finished).
pub fn remember(window: &Window) {
    if matches!(window.is_minimized(), Ok(true)) {
        return;
    }
    let Some(store) = window.app_handle().try_state::<Store>() else {
        return;
    };
    let maximized = window.is_maximized().unwrap_or(false);
    let mut guard = store.lock().expect("window-state mutex poisoned");
    let had = guard.is_some();
    let mut geom = (*guard).unwrap_or_default();
    // Only refresh size/position from a NORMAL window, so the stored size is
    // the "restore" size — but seed once even if the first event is maximized.
    if !maximized || !had {
        if let Ok(size) = window.outer_size() {
            if size.width > 0 && size.height > 0 {
                geom.width = size.width;
                geom.height = size.height;
            }
        }
        if let Ok(pos) = window.outer_position() {
            geom.x = pos.x;
            geom.y = pos.y;
        }
    }
    geom.maximized = maximized;
    *guard = Some(geom);
}

/// Capture the latest geometry and write it to disk. Called on window close.
pub fn flush(window: &Window) {
    remember(window);
    let Some(store) = window.app_handle().try_state::<Store>() else {
        return;
    };
    let geom = *store.lock().expect("window-state mutex poisoned");
    if let Some(geom) = geom {
        save(&geom);
    }
}

/// Apply a saved geometry to the main window at startup. Validates the size
/// and that the position lands on a currently-connected monitor, so a
/// disconnected second screen never strands the window off-canvas.
pub fn restore(window: &WebviewWindow, geom: &WindowGeometry) {
    if geom.width >= MIN_W && geom.height >= MIN_H {
        let _ = window.set_size(PhysicalSize::new(geom.width, geom.height));
        if position_visible(window, geom.x, geom.y) {
            let _ = window.set_position(PhysicalPosition::new(geom.x, geom.y));
        }
    }
    if geom.maximized {
        let _ = window.maximize();
    }
}

/// Whether `(x, y)` (the window's top-left) sits inside a connected monitor —
/// keeps the title bar (the only drag handle) reachable.
fn position_visible(window: &WebviewWindow, x: i32, y: i32) -> bool {
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let pos = m.position();
        let size = m.size();
        x >= pos.x && x < pos.x + size.width as i32 && y >= pos.y && y < pos.y + size.height as i32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_round_trips_through_app_config() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from(dir.path()).is_none(), "no file yet → None");

        let geom = WindowGeometry {
            width: 1280,
            height: 800,
            x: 100,
            y: 60,
            maximized: false,
        };
        save_to(dir.path(), &geom).unwrap();

        let back = load_from(dir.path()).expect("saved geometry reads back");
        assert_eq!(back.width, 1280);
        assert_eq!(back.height, 800);
        assert_eq!(back.x, 100);
        assert_eq!(back.y, 60);
        assert!(!back.maximized);

        // Overwrite with a maximized state.
        save_to(
            dir.path(),
            &WindowGeometry {
                maximized: true,
                ..geom
            },
        )
        .unwrap();
        assert!(load_from(dir.path()).unwrap().maximized);
    }

    #[test]
    fn load_from_tolerates_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(config_path(dir.path()), b"{ not json").unwrap();
        assert!(load_from(dir.path()).is_none());
    }
}
