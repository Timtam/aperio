//! System-tray integration — optional close/minimize-to-tray.
//!
//! On platforms that provide a tray (Windows notification area, macOS menu
//! bar, Linux StatusNotifierItem/AppIndicator where the desktop exposes
//! one) the user can opt to have the close and/or minimize buttons tuck
//! Aperio into the tray instead of quitting / going to the taskbar. The
//! reminder scheduler keeps running in the background, so the app can sit
//! quietly in the tray and still fire reminders.
//!
//! The two behaviours are independent prefs — [`CLOSE_TO_TRAY_PREF`] and
//! [`MINIMIZE_TO_TRAY_PREF`], both default off — and are only honoured when
//! a tray actually exists. Building the tray is best-effort: on a desktop
//! without one (e.g. GNOME with no AppIndicator extension) [`build`]
//! reports `available = false`, the Settings toggles disable themselves,
//! and close/minimize fall back to their normal behaviour.

use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, Wry};
use tracing::warn;

use crate::user_prefs::UserPrefsRepo;
use crate::DbHandle;

/// `"true"` ⇒ the close button hides to tray instead of quitting.
pub const CLOSE_TO_TRAY_PREF: &str = "window.closeToTray";
/// `"true"` ⇒ the minimize button hides to tray instead of minimizing.
pub const MINIMIZE_TO_TRAY_PREF: &str = "window.minimizeToTray";

const MAIN_WINDOW: &str = "main";

/// Whether a system tray is present, plus the live tray handle kept alive
/// for the app's lifetime (dropping the [`TrayIcon`] removes it from the
/// tray). Stored in Tauri state so the read commands can answer the
/// Settings gate.
pub struct TrayHandles {
    pub available: bool,
    /// The menu-item handles, kept so the frontend can push localized labels
    /// onto them (via [`set_tray_labels`]) once i18n is up and on every
    /// language change. `None` when no tray was built. muda menu items are
    /// shared handles, so re-labelling these updates the live menu.
    show: Mutex<Option<MenuItem<Wry>>>,
    quit: Mutex<Option<MenuItem<Wry>>>,
    _icon: Option<TrayIcon<Wry>>,
}

/// Build the tray (best-effort). Returns handles with `available = false`
/// when the platform/desktop has no tray.
pub fn build(app: &AppHandle) -> TrayHandles {
    match try_build(app) {
        Ok((icon, show, quit)) => TrayHandles {
            available: true,
            show: Mutex::new(Some(show)),
            quit: Mutex::new(Some(quit)),
            _icon: Some(icon),
        },
        Err(err) => {
            warn!(
                target: "aperio::tray",
                %err,
                "system tray unavailable; close/minimize-to-tray fall back to normal behaviour",
            );
            TrayHandles {
                available: false,
                show: Mutex::new(None),
                quit: Mutex::new(None),
                _icon: None,
            }
        }
    }
}

fn try_build(app: &AppHandle) -> tauri::Result<(TrayIcon<Wry>, MenuItem<Wry>, MenuItem<Wry>)> {
    // Placeholder labels in the app's fallback language. The frontend pushes
    // the localized labels via `set_tray_labels` as soon as i18n is ready
    // (and again on every language change), so these are only visible in the
    // brief window before the UI mounts.
    let show = MenuItem::with_id(app, "tray-show", "Show Aperio", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray-quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    // Embed the icon so it doesn't depend on bundle config or a file next to
    // a portable .exe. `image-png` decodes it.
    let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;

    let tray = TrayIconBuilder::with_id("aperio-main-tray")
        .tooltip("Aperio")
        .icon(icon)
        // Left-click toggles the window; the menu is right-click only.
        .show_menu_on_left_click(false)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-show" => show_main_window(app),
            // `app.exit` raises RunEvent::ExitRequested, so the app-exit
            // sync push (in `run()`'s run-loop) flushes pending changes here
            // too — no special handling needed.
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok((tray, show, quit))
}

/// Reveal + focus the main window (un-hide from the tray).
pub fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// True when the user-pref `key` holds the string `"true"`.
pub fn pref_is_true(app: &AppHandle, key: &str) -> bool {
    let shared = app.state::<DbHandle>().shared();
    matches!(
        UserPrefsRepo::new(&shared)
            .get(key)
            .ok()
            .flatten()
            .as_deref(),
        Some("true"),
    )
}

/// Whether a system tray exists — drives the Settings toggles' enabled state.
#[tauri::command]
pub fn tray_available(tray: State<'_, TrayHandles>) -> bool {
    tray.available
}

/// Push localized labels onto the tray menu items. The frontend calls this
/// once i18n is ready and again whenever the language changes, so the tray
/// menu follows the app language instead of the hard-coded placeholders.
///
/// Native menu mutation must happen on the main (UI) thread; Tauri commands
/// run off it, so we hop via `run_on_main_thread`. No-op when there's no
/// tray (the handles are `None`).
#[tauri::command]
pub fn set_tray_labels(app: AppHandle, show: String, quit: String) {
    let target = app.clone();
    let _ = app.run_on_main_thread(move || {
        let tray = target.state::<TrayHandles>();
        relabel(&tray.show, &show);
        relabel(&tray.quit, &quit);
    });
}

/// Set a tray menu item's text, locking its slot. No-op when the slot is
/// empty (no tray) or the lock is poisoned.
fn relabel(slot: &Mutex<Option<MenuItem<Wry>>>, text: &str) {
    if let Ok(guard) = slot.lock() {
        if let Some(item) = guard.as_ref() {
            let _ = item.set_text(text);
        }
    }
}

/// Close button: hide to tray when `closeToTray` is on AND a tray exists;
/// otherwise close the window (which exits the app + flushes the sync push).
#[tauri::command]
pub fn request_window_close(app: AppHandle, tray: State<'_, TrayHandles>) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    if tray.available && pref_is_true(&app, CLOSE_TO_TRAY_PREF) {
        let _ = win.hide();
    } else {
        let _ = win.close();
    }
}

/// Minimize button: hide to tray when `minimizeToTray` is on AND a tray
/// exists; otherwise minimize to the taskbar/dock.
#[tauri::command]
pub fn request_window_minimize(app: AppHandle, tray: State<'_, TrayHandles>) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    if tray.available && pref_is_true(&app, MINIMIZE_TO_TRAY_PREF) {
        let _ = win.hide();
    } else {
        let _ = win.minimize();
    }
}
