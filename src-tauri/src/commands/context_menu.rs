//! Native context menu plumbing.
//!
//! Sidebar rows, event chips, and task rows all reach their
//! per-item actions through the OS-native context menu rather than a
//! custom React overlay — Win32 / NSMenu / GTK each give us their
//! real menus with the platform's keyboard model, accessibility
//! wiring, and visual style for free.
//!
//! The flow is:
//!
//!   1. Frontend calls `show_context_menu({ items, position })` with
//!      a tree of `{kind, id, label, …}` entries and (optionally) the
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
//!
//! The item shape supports four kinds:
//!
//!   - `Text` — a plain action row (the default).
//!   - `Check` — a row whose check-mark state is supplied by the
//!     caller. The OS draws its own glyph.
//!   - `Submenu` — a nested menu with its own `items` list. Used by
//!     task chips for the Status > {open, in_progress, …} cascade.
//!   - `Separator` — a horizontal divider, no id.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tauri::menu::{
    CheckMenuItemBuilder, ContextMenu, MenuBuilder, MenuItemBuilder, MenuItemKind,
    PredefinedMenuItem, SubmenuBuilder,
};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::oneshot;

use super::{CommandError, CommandResult};

/// Per-app state holding the in-flight popup's reply channel, tagged with
/// a monotonic generation id.
///
/// Only one popup is visible at a time, but a *dismissed* popup fires no
/// menu event (Escape / click-away are silent on every platform we
/// target), so its `show_context_menu` keeps awaiting until a newer popup
/// replaces — and thereby drops — its sender. When that stale call finally
/// wakes (closed channel), it must NOT clear the slot the newer popup just
/// installed; doing so would swallow the next selection and the command
/// would return `None`. The generation tag lets each call clean up only
/// its own slot. (Symptom before the tag: open a menu, dismiss it, then the
/// very next menu does nothing.)
pub struct ContextMenuState {
    pub pending: std::sync::Mutex<Option<(u64, oneshot::Sender<String>)>>,
    next_generation: AtomicU64,
}

impl ContextMenuState {
    pub fn new() -> Self {
        Self {
            pending: std::sync::Mutex::new(None),
            next_generation: AtomicU64::new(0),
        }
    }

    /// Allocate a unique generation id for a new popup.
    fn next_gen(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::Relaxed)
    }
}

impl Default for ContextMenuState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MenuKind {
    /// Plain action row — the default for backward compat with
    /// callers that omit `kind`.
    #[default]
    Text,
    /// Check-mark row. The frontend supplies the initial `checked`
    /// state; the OS handles the visual glyph.
    Check,
    /// Nested menu. `items` carries the children, `label` the
    /// visible parent text.
    Submenu,
    /// Horizontal divider. No id, no label, never selected.
    Separator,
}

#[derive(Debug, serde::Deserialize)]
pub struct ContextMenuItemRequest {
    /// Optional kind discriminator. Defaults to `Text` so existing
    /// `{id, label}` callers (sidebar) keep working unchanged.
    #[serde(default)]
    pub kind: MenuKind,
    /// Required for text / check / submenu items in spirit, but kept
    /// optional here so a malformed payload fails with a clear error
    /// at build time instead of a serde decode rejection.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub checked: Option<bool>,
    /// Children for `Submenu` items. Recursively typed so deeper
    /// nesting works, though we only use one level today.
    #[serde(default)]
    pub items: Vec<ContextMenuItemRequest>,
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

/// Build one menu item from its request. Recurses through submenu
/// children so a `Submenu { items: [...] }` becomes a real native
/// `Submenu` with its kids attached.
fn build_item<R: Runtime>(
    app: &AppHandle<R>,
    req: &ContextMenuItemRequest,
) -> CommandResult<MenuItemKind<R>> {
    // Implicit-check fallback: TypeScript's structural typing happily
    // accepts a payload like `{id, label, checked}` (without `kind`)
    // against the `text` branch of the union because `checked` is
    // valid in *another* branch. Without this guard we'd silently
    // build a plain row, drop the `checked` field on the floor, and
    // never render the OS check-mark glyph. Promote to `Check` when
    // the caller provided a `checked` value alongside no explicit
    // kind — semantically that's what they meant.
    if req.kind == MenuKind::Text && req.checked.is_some() {
        let id = req.id.as_deref().ok_or_else(|| CommandError {
            code: "internal",
            message: "check menu item missing id".into(),
        })?;
        let label = req.label.as_deref().ok_or_else(|| CommandError {
            code: "internal",
            message: "check menu item missing label".into(),
        })?;
        let item = CheckMenuItemBuilder::with_id(id, label)
            .checked(req.checked.unwrap_or(false))
            .build(app)
            .map_err(|e| CommandError {
                code: "internal",
                message: format!("check item build: {e}"),
            })?;
        return Ok(MenuItemKind::Check(item));
    }

    match req.kind {
        MenuKind::Text => {
            let id = req.id.as_deref().ok_or_else(|| CommandError {
                code: "internal",
                message: "text menu item missing id".into(),
            })?;
            let label = req.label.as_deref().ok_or_else(|| CommandError {
                code: "internal",
                message: "text menu item missing label".into(),
            })?;
            let item = MenuItemBuilder::with_id(id, label)
                .build(app)
                .map_err(|e| CommandError {
                    code: "internal",
                    message: format!("text item build: {e}"),
                })?;
            Ok(MenuItemKind::MenuItem(item))
        }
        MenuKind::Check => {
            let id = req.id.as_deref().ok_or_else(|| CommandError {
                code: "internal",
                message: "check menu item missing id".into(),
            })?;
            let label = req.label.as_deref().ok_or_else(|| CommandError {
                code: "internal",
                message: "check menu item missing label".into(),
            })?;
            let item = CheckMenuItemBuilder::with_id(id, label)
                .checked(req.checked.unwrap_or(false))
                .build(app)
                .map_err(|e| CommandError {
                    code: "internal",
                    message: format!("check item build: {e}"),
                })?;
            Ok(MenuItemKind::Check(item))
        }
        MenuKind::Submenu => {
            let label = req.label.as_deref().ok_or_else(|| CommandError {
                code: "internal",
                message: "submenu missing label".into(),
            })?;
            let mut sb = SubmenuBuilder::new(app, label);
            // Recurse so deeper nesting works without special-casing
            // it here. The frontend never goes deeper than one level
            // today; the recursion is a future-proofing freebie.
            for child in &req.items {
                let child_kind = build_item(app, child)?;
                sb = sb.item(&child_kind);
            }
            let submenu = sb.build().map_err(|e| CommandError {
                code: "internal",
                message: format!("submenu build: {e}"),
            })?;
            Ok(MenuItemKind::Submenu(submenu))
        }
        MenuKind::Separator => {
            let sep = PredefinedMenuItem::separator(app).map_err(|e| CommandError {
                code: "internal",
                message: format!("separator build: {e}"),
            })?;
            Ok(MenuItemKind::Predefined(sep))
        }
    }
}

#[tauri::command]
pub async fn show_context_menu(
    app: AppHandle,
    state: State<'_, ContextMenuState>,
    request: ShowContextMenuRequest,
) -> CommandResult<Option<String>> {
    if request.items.is_empty() {
        return Ok(None);
    }

    // Build the menu. Each item gets the caller-supplied id which
    // travels back unchanged when the user activates it. Submenus
    // recurse via `build_item`; checkboxes draw their own glyph;
    // plain text rows are the cheapest path.
    let mut builder = MenuBuilder::new(&app);
    for item in &request.items {
        let kind = build_item(&app, item)?;
        builder = builder.item(&kind);
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
    let generation = state.next_gen();
    *state.pending.lock().expect("context menu state poisoned") = Some((generation, tx));

    // Aperio is a single-window app; the main window is always
    // present. The menu API wants the lower-level `Window` (the
    // OS-window handle), not the `WebviewWindow` (window + embedded
    // webview). `as_ref().clone()` peels the wrapper off cheaply —
    // `Window` is a `Clone` newtype around an `Arc`.
    let webview = app.get_webview_window("main").ok_or_else(|| CommandError {
        code: "internal",
        message: "main window missing".into(),
    })?;
    let window = webview.as_ref().window().clone();

    let popup_result = match &request.position {
        Some(pos) => menu.popup_at(window, tauri::LogicalPosition::new(pos.x, pos.y)),
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
    let result = match tokio::time::timeout(Duration::from_secs(60), rx).await {
        Ok(Ok(id)) => Some(id),
        // Closed channel (Ok(Err(_))) or timeout (Err(_)) — either way this
        // call didn't get a selection.
        _ => None,
    };
    // Clear the slot only if it is still OURS. A dismissed popup fires no
    // menu event, so this call may only have woken because a NEWER popup
    // replaced (and dropped) our sender — taking unconditionally here would
    // rip out that newer popup's sender, and its selection would never be
    // delivered (the "dismiss a menu, the next one does nothing" bug).
    {
        let mut guard = state.pending.lock().expect("context menu state poisoned");
        if matches!(guard.as_ref(), Some((g, _)) if *g == generation) {
            guard.take();
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduces the "dismiss a menu, the next one does nothing" bug at the
    /// state-machine level: a stale popup that woke up (its sender dropped
    /// by a newer popup) must clean up ONLY its own slot, leaving the newer
    /// popup's sender intact so `on_menu_event` can still deliver its pick.
    #[test]
    fn stale_popup_cleanup_keeps_a_newer_popups_sender() {
        let state = ContextMenuState::new();

        // Popup A installs its sender (then gets dismissed → no event).
        let gen_a = state.next_gen();
        let (tx_a, _rx_a) = oneshot::channel::<String>();
        *state.pending.lock().unwrap() = Some((gen_a, tx_a));

        // Popup B supersedes A: installing B drops tx_a, which would wake
        // A's awaiting command.
        let gen_b = state.next_gen();
        let (tx_b, mut rx_b) = oneshot::channel::<String>();
        *state.pending.lock().unwrap() = Some((gen_b, tx_b));
        assert_ne!(gen_a, gen_b);

        // A wakes and runs its generation-guarded cleanup — must be a no-op
        // because the slot now belongs to B.
        {
            let mut guard = state.pending.lock().unwrap();
            if matches!(guard.as_ref(), Some((g, _)) if *g == gen_a) {
                guard.take();
            }
        }

        // B's sender survives, so a selection still reaches its receiver.
        {
            let mut guard = state.pending.lock().unwrap();
            let Some((_, tx)) = guard.take() else {
                panic!("newer popup's sender was evicted by the stale one");
            };
            tx.send("members".to_string()).expect("receiver alive");
        }
        assert_eq!(rx_b.try_recv().ok(), Some("members".to_string()));
    }

    /// The ordinary path: a popup that is still the current one cleans up
    /// its own slot on timeout/close so it doesn't linger.
    #[test]
    fn current_popup_cleans_up_its_own_slot() {
        let state = ContextMenuState::new();
        let generation = state.next_gen();
        let (tx, _rx) = oneshot::channel::<String>();
        *state.pending.lock().unwrap() = Some((generation, tx));

        {
            let mut guard = state.pending.lock().unwrap();
            if matches!(guard.as_ref(), Some((g, _)) if *g == generation) {
                guard.take();
            }
        }
        assert!(state.pending.lock().unwrap().is_none());
    }
}
