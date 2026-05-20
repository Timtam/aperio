//! Native context menu plumbing.
//!
//! The sidebar's per-row rename / delete actions are reachable
//! through the OS-native context menu rather than a custom React
//! overlay — that gives us real Win32 / NSMenu / GTK menus with
//! the platform's keyboard model, accessibility wiring, and visual
//! style for free.
//!
//! The flow is:
//!
//!   1. Frontend calls `show_sidebar_context_menu({ items, position })`
//!      with a list of `{id, label}` entries and (optionally) the
//!      logical-pixel position the menu should anchor at.
//!   2. This command builds a `tauri::menu::Menu`, installs a
//!      one-shot sender into the shared `ContextMenuState`, calls
//!      `popup()` / `popup_at()` on the main window.
//!   3. A global `on_menu_event` handler (registered once at
//!      startup in `lib.rs`) consumes the sender when an item is
//!      activated and sends the selected id back through the
//!      one-shot.
//!   4. If the user dismisses the menu without selecting (clicks
//!      outside, presses Escape), no event fires — the timeout
//!      drops the sender and the command returns `None`.

use std::time::Duration;

use tauri::menu::{ContextMenu, MenuBuilder};
use tauri::{AppHandle, Manager, State};
use tokio::sync::oneshot;

use super::{CommandError, CommandResult};

/// Per-app state holding the in-flight popup's reply channel.
///
/// Only one popup is expected to be open at a time (the user can't
/// physically have two native menus visible). If a second popup
/// arrives while one is pending, the previous sender is dropped —
/// the previous command's `recv` resolves to a closed-channel error
/// and the command returns `None`, which is the right semantics
/// ("the user moved on without picking anything").
pub struct ContextMenuState {
    pub pending: std::sync::Mutex<Option<oneshot::Sender<String>>>,
}

impl ContextMenuState {
    pub fn new() -> Self {
        Self {
            pending: std::sync::Mutex::new(None),
        }
    }
}

impl Default for ContextMenuState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct ContextMenuItemRequest {
    pub id: String,
    pub label: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ContextMenuPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct ShowContextMenuRequest {
    pub items: Vec<ContextMenuItemRequest>,
    /// Optional anchor in window-logical coordinates. `None` ⇒ the
    /// menu pops at the current cursor position (right-click case).
    /// `Some` ⇒ for keyboard-triggered menus where the cursor is
    /// unrelated to the focused row.
    pub position: Option<ContextMenuPosition>,
}

#[tauri::command]
pub async fn show_sidebar_context_menu(
    app: AppHandle,
    state: State<'_, ContextMenuState>,
    request: ShowContextMenuRequest,
) -> CommandResult<Option<String>> {
    if request.items.is_empty() {
        return Ok(None);
    }

    // Build the menu. Each item gets the caller-supplied id which
    // travels back unchanged when the user activates it.
    let mut builder = MenuBuilder::new(&app);
    for item in &request.items {
        builder = builder.text(item.id.clone(), &item.label);
    }
    let menu = builder.build().map_err(|e| CommandError {
        code: "internal",
        message: format!("menu build: {e}"),
    })?;

    // Install the reply channel before showing the menu. If a
    // previous popup is still pending, its sender drops here —
    // the corresponding await over there will receive a closed
    // channel and convert it to `None`.
    let (tx, rx) = oneshot::channel::<String>();
    *state.pending.lock().expect("context menu state poisoned") = Some(tx);

    // Aperio is a single-window app; the main window is always
    // present. The menu API wants the lower-level `Window` (the
    // OS-window handle), not the `WebviewWindow` (window + embedded
    // webview). `as_ref().clone()` peels the wrapper off cheaply —
    // `Window` is a `Clone` newtype around an `Arc`.
    let webview = app
        .get_webview_window("main")
        .ok_or_else(|| CommandError {
            code: "internal",
            message: "main window missing".into(),
        })?;
    let window = webview.as_ref().window().clone();

    let popup_result = match &request.position {
        Some(pos) => menu.popup_at(
            window,
            tauri::LogicalPosition::new(pos.x, pos.y),
        ),
        None => menu.popup(window),
    };
    popup_result.map_err(|e| CommandError {
        code: "internal",
        message: format!("popup: {e}"),
    })?;

    // Wait for the user's choice, with a generous timeout in case
    // they walk away. 60s is more than enough — most users either
    // pick within seconds or dismiss with Escape (which fires no
    // event on any platform we target). Without the timeout this
    // task could leak forever after a quiet dismiss.
    match tokio::time::timeout(Duration::from_secs(60), rx).await {
        Ok(Ok(id)) => Ok(Some(id)),
        // Closed channel (Ok(Err(_))) or timeout (Err(_)) — either
        // way the user didn't pick. Clean up the sender slot so the
        // next popup starts fresh.
        _ => {
            state
                .pending
                .lock()
                .expect("context menu state poisoned")
                .take();
            Ok(None)
        }
    }
}
