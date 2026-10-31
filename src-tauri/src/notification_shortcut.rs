//! The Start menu shortcut Windows needs before it shows Aperio's
//! notifications (DESIGN.md §14.3).
//!
//! Windows shows a toast only when a Start menu shortcut carries the toast's
//! app id (System.AppUserModel.ID); without one it stores every toast and
//! shows none (see `platform`). Aperio is portable, so it never creates that
//! shortcut on its own: Settings > General, the first-launch wizard, and a
//! one-time hint offer it, and this module does the work.
//!
//! - **The shortcut** is `<Start menu Programs>\{productName}.lnk`, pointing
//!   at the running exe and carrying the identifier as its app id, like the
//!   one Tauri's NSIS installer writes. A link of that name without our id is
//!   the user's own (`Foreign`): setting up replaces it on request, nothing
//!   else touches it.
//! - **At start** a shortcut of ours that points at another exe (the folder
//!   moved, or another copy ran last) is pointed at this one. A missing
//!   shortcut is never created there.
//! - **The hint**: the first reminder of an app start that fires while
//!   Windows would hide it raises one event for the frontend. Kept in memory
//!   only, so it can come again after a restart (Toni, 2026-10-31).

#[cfg(any(windows, test))]
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{CommandError, CommandResult};

#[cfg(windows)]
mod win;

/// Raised for the frontend when a reminder fired while Windows would hide
/// it. No payload: the frontend asks for the status before it shows the
/// hint.
pub const HINT_EVENT: &str = "notification-shortcut-hint";

/// Where the shortcut stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NotificationShortcut {
    /// Not Windows: nothing to set up.
    Unsupported,
    /// A development build (`platform::toasts_use_powershell_id`): its
    /// toasts go out under PowerShell's id and show as Windows PowerShell.
    DevBuild,
    /// No shortcut.
    NotSetUp,
    /// A shortcut of that name that does not carry our id.
    Foreign,
    /// Ours, pointing at this exe.
    SetUp,
    /// Ours, pointing at another exe.
    PointsElsewhere { target: String },
}

impl NotificationShortcut {
    /// Whether Windows hides Aperio's notifications in this state, so that
    /// setting up would bring them. The rule the frontend reads, once.
    pub fn needs_set_up(&self) -> bool {
        matches!(self, Self::NotSetUp | Self::Foreign)
    }
}

/// What the commands return: the state and the rule read off it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShortcutStatus {
    #[serde(flatten)]
    pub state: NotificationShortcut,
    pub needs_set_up: bool,
}

impl From<NotificationShortcut> for ShortcutStatus {
    fn from(state: NotificationShortcut) -> Self {
        let needs_set_up = state.needs_set_up();
        Self {
            state,
            needs_set_up,
        }
    }
}

/// A shortcut as read from disk.
#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
pub(crate) struct LinkInfo {
    pub target: PathBuf,
    pub aumid: Option<String>,
}

/// Where a shortcut stands, decided from what is on disk.
#[cfg(any(windows, test))]
pub(crate) fn classify(
    link: Option<&LinkInfo>,
    exe: &Path,
    identifier: &str,
    dev: bool,
) -> NotificationShortcut {
    if dev {
        return NotificationShortcut::DevBuild;
    }
    let Some(link) = link else {
        return NotificationShortcut::NotSetUp;
    };
    if link.aumid.as_deref() != Some(identifier) {
        return NotificationShortcut::Foreign;
    }
    if same_exe(&link.target, exe) {
        NotificationShortcut::SetUp
    } else {
        NotificationShortcut::PointsElsewhere {
            target: link.target.display().to_string(),
        }
    }
}

/// Whether two paths name the same exe. Resolved through the file system
/// when both exist (case, 8.3 names, junctions); compared as text when one
/// does not, say a deleted copy: separators unified, case ignored, as
/// Windows' own paths are.
#[cfg(any(windows, test))]
pub(crate) fn same_exe(a: &Path, b: &Path) -> bool {
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        return a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase();
    }
    let lexical = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    lexical(a) == lexical(b)
}

/// When the one-time hint is due. Pure, so its rules are tested on every
/// system.
#[derive(Debug, Default)]
pub(crate) struct HintGate {
    status: Option<NotificationShortcut>,
    fired_while_unknown: bool,
    due: bool,
    shown: bool,
}

impl HintGate {
    /// A reminder fired. True when that raises the hint.
    pub fn on_fired(&mut self) -> bool {
        if self.shown || self.due {
            return false;
        }
        match &self.status {
            None => {
                self.fired_while_unknown = true;
                false
            }
            Some(status) if status.needs_set_up() => {
                self.due = true;
                true
            }
            Some(_) => false,
        }
    }

    /// The status is known (again). True when a reminder that fired before
    /// the first status was known raises the hint now.
    pub fn on_status(&mut self, status: NotificationShortcut) -> bool {
        let needs = status.needs_set_up();
        let fired_before = std::mem::take(&mut self.fired_while_unknown);
        self.status = Some(status);
        if !needs {
            self.due = false;
            return false;
        }
        if fired_before && !self.shown && !self.due {
            self.due = true;
            return true;
        }
        false
    }

    /// Whether the hint waits to be shown.
    pub fn pending(&self) -> bool {
        self.due && !self.shown
    }

    /// The hint was shown: not again in this app start.
    pub fn on_shown(&mut self) {
        self.shown = true;
        self.due = false;
    }
}

/// The module's Tauri state.
pub struct NotificationShortcutState {
    identifier: String,
    product_name: String,
    gate: Mutex<HintGate>,
    /// Held across every read and write of the shortcut, so the repair at
    /// start and a status probe from the page never interleave.
    io: Mutex<()>,
}

impl NotificationShortcutState {
    pub fn new(identifier: String, product_name: String) -> Self {
        Self {
            identifier,
            product_name,
            gate: Mutex::new(HintGate::default()),
            io: Mutex::new(()),
        }
    }

    /// A reminder fired; true when the frontend should hear about the hint.
    /// Memory only: no disk, no COM.
    pub fn note_reminder_fired(&self) -> bool {
        self.gate.lock().expect("hint gate poisoned").on_fired()
    }

    fn note_status(&self, status: &NotificationShortcut) -> bool {
        self.gate
            .lock()
            .expect("hint gate poisoned")
            .on_status(status.clone())
    }

    #[cfg(windows)]
    fn paths(&self, create_folder: bool) -> CommandResult<(PathBuf, PathBuf)> {
        let link = win::link_path(&self.product_name, create_folder).map_err(internal)?;
        let exe = std::env::current_exe().map_err(internal)?;
        Ok((link, exe))
    }

    fn status(&self) -> CommandResult<NotificationShortcut> {
        #[cfg(windows)]
        {
            let _io = self.io.lock().expect("shortcut io poisoned");
            let (link, exe) = self.paths(false)?;
            win::status_at(
                &link,
                &exe,
                &self.identifier,
                crate::platform::is_dev_build(),
            )
            .map_err(internal)
        }
        #[cfg(not(windows))]
        {
            let _ = (&self.io, &self.identifier, &self.product_name);
            Ok(NotificationShortcut::Unsupported)
        }
    }

    fn create(&self) -> CommandResult<NotificationShortcut> {
        #[cfg(windows)]
        {
            let _io = self.io.lock().expect("shortcut io poisoned");
            let (link, exe) = self.paths(true)?;
            win::create_at(
                &link,
                &exe,
                &self.identifier,
                crate::platform::is_dev_build(),
            )
        }
        #[cfg(not(windows))]
        Err(unsupported())
    }

    fn remove(&self) -> CommandResult<NotificationShortcut> {
        #[cfg(windows)]
        {
            let _io = self.io.lock().expect("shortcut io poisoned");
            let (link, exe) = self.paths(false)?;
            win::remove_at(
                &link,
                &exe,
                &self.identifier,
                crate::platform::is_dev_build(),
            )
        }
        #[cfg(not(windows))]
        Err(unsupported())
    }

    #[cfg(windows)]
    fn repair_at_startup(&self) -> CommandResult<NotificationShortcut> {
        let _io = self.io.lock().expect("shortcut io poisoned");
        let (link, exe) = self.paths(false)?;
        win::refresh_at(
            &link,
            &exe,
            &self.identifier,
            crate::platform::is_dev_build(),
        )
        .map_err(internal)
    }
}

fn internal(err: impl std::fmt::Display) -> CommandError {
    CommandError {
        code: "internal",
        message: err.to_string(),
    }
}

#[cfg(any(not(windows), test))]
fn unsupported() -> CommandError {
    CommandError {
        code: "unsupported",
        message: "the Start menu shortcut exists only on Windows".into(),
    }
}

/// Look at the shortcut at start, off the main thread (COM). Repairs one of
/// ours that points elsewhere, and raises the hint for a reminder that fired
/// before the status was known. Elsewhere than Windows it only records that
/// there is nothing to set up.
pub fn spawn_startup_check(app: AppHandle) {
    let state = app
        .state::<Arc<NotificationShortcutState>>()
        .inner()
        .clone();
    #[cfg(windows)]
    {
        let spawned = std::thread::Builder::new()
            .name("aperio-start-menu".into())
            .spawn(move || match state.repair_at_startup() {
                Ok(status) => {
                    tracing::info!(target: "aperio::notifications", ?status, "start menu shortcut");
                    if state.note_status(&status) {
                        let _ = app.emit(HINT_EVENT, ());
                    }
                }
                Err(err) => tracing::warn!(
                    target: "aperio::notifications",
                    code = err.code,
                    message = %err.message,
                    "checking the start menu shortcut failed"
                ),
            });
        if let Err(err) = spawned {
            tracing::warn!(target: "aperio::notifications", ?err, "could not start the shortcut check");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        state.note_status(&NotificationShortcut::Unsupported);
    }
}

/// Run blocking shortcut work off the async runtime's workers (COM wants a
/// thread of its own; never the main thread).
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> CommandResult<T> + Send + 'static,
) -> CommandResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(internal)?
}

/// Read the status from disk; the page's probe. Also raises the hint when a
/// reminder fired before the status was known.
#[tauri::command]
pub async fn notification_shortcut_status(app: AppHandle) -> CommandResult<ShortcutStatus> {
    let state = app
        .state::<Arc<NotificationShortcutState>>()
        .inner()
        .clone();
    let status = blocking({
        let state = state.clone();
        move || state.status()
    })
    .await?;
    if state.note_status(&status) {
        let _ = app.emit(HINT_EVENT, ());
    }
    Ok(status.into())
}

/// Create the shortcut (or point ours, or replace a foreign one, at this
/// exe). `unsupported` in a development build and off Windows.
#[tauri::command]
pub async fn notification_shortcut_create(app: AppHandle) -> CommandResult<ShortcutStatus> {
    let state = app
        .state::<Arc<NotificationShortcutState>>()
        .inner()
        .clone();
    let status = blocking({
        let state = state.clone();
        move || state.create()
    })
    .await?;
    state.note_status(&status);
    tracing::info!(target: "aperio::notifications", ?status, "start menu shortcut set up");
    Ok(status.into())
}

/// Remove our shortcut. `conflict` for a foreign one, which stays.
#[tauri::command]
pub async fn notification_shortcut_remove(app: AppHandle) -> CommandResult<ShortcutStatus> {
    let state = app
        .state::<Arc<NotificationShortcutState>>()
        .inner()
        .clone();
    let status = blocking({
        let state = state.clone();
        move || state.remove()
    })
    .await?;
    state.note_status(&status);
    tracing::info!(target: "aperio::notifications", "start menu shortcut removed");
    Ok(status.into())
}

/// Whether the hint waits: for a page that started listening after the
/// event went out.
#[tauri::command]
pub fn notification_shortcut_hint_pending(app: AppHandle) -> bool {
    app.state::<Arc<NotificationShortcutState>>()
        .gate
        .lock()
        .expect("hint gate poisoned")
        .pending()
}

/// The hint is on screen: not again in this app start.
#[tauri::command]
pub fn notification_shortcut_hint_shown(app: AppHandle) {
    app.state::<Arc<NotificationShortcutState>>()
        .gate
        .lock()
        .expect("hint gate poisoned")
        .on_shown();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(target: &str, aumid: Option<&str>) -> LinkInfo {
        LinkInfo {
            target: PathBuf::from(target),
            aumid: aumid.map(str::to_string),
        }
    }

    const ID: &str = "com.aperio.app";

    #[test]
    fn a_development_build_is_one_whatever_is_on_disk() {
        let here = Path::new(r"C:\a\aperio.exe");
        assert_eq!(
            classify(None, here, ID, true),
            NotificationShortcut::DevBuild
        );
        assert_eq!(
            classify(Some(&link(r"C:\a\aperio.exe", Some(ID))), here, ID, true),
            NotificationShortcut::DevBuild
        );
    }

    #[test]
    fn the_shortcut_is_classified_by_its_id_and_its_target() {
        let here = Path::new(r"C:\a\aperio.exe");
        assert_eq!(
            classify(None, here, ID, false),
            NotificationShortcut::NotSetUp
        );
        assert_eq!(
            classify(Some(&link(r"C:\a\aperio.exe", None)), here, ID, false),
            NotificationShortcut::Foreign
        );
        assert_eq!(
            classify(
                Some(&link(r"C:\a\aperio.exe", Some("other.id"))),
                here,
                ID,
                false
            ),
            NotificationShortcut::Foreign
        );
        assert_eq!(
            classify(Some(&link(r"C:\a\aperio.exe", Some(ID))), here, ID, false),
            NotificationShortcut::SetUp
        );
        assert_eq!(
            classify(Some(&link(r"D:\b\aperio.exe", Some(ID))), here, ID, false),
            NotificationShortcut::PointsElsewhere {
                target: r"D:\b\aperio.exe".into()
            }
        );
    }

    #[test]
    fn paths_that_do_not_exist_compare_as_windows_text() {
        assert!(same_exe(
            Path::new(r"D:\Aperio-windows-x86_64\aperio.exe"),
            Path::new("d:/aperio-windows-x86_64/APERIO.EXE"),
        ));
        assert!(!same_exe(
            Path::new(r"D:\one\aperio.exe"),
            Path::new(r"D:\two\aperio.exe"),
        ));
    }

    #[test]
    fn only_not_set_up_and_foreign_need_setting_up() {
        let needs: Vec<bool> = [
            NotificationShortcut::Unsupported,
            NotificationShortcut::DevBuild,
            NotificationShortcut::NotSetUp,
            NotificationShortcut::Foreign,
            NotificationShortcut::SetUp,
            NotificationShortcut::PointsElsewhere { target: "x".into() },
        ]
        .iter()
        .map(NotificationShortcut::needs_set_up)
        .collect();
        assert_eq!(needs, [false, false, true, true, false, false]);
    }

    #[test]
    fn a_reminder_while_not_set_up_raises_the_hint_once() {
        let mut gate = HintGate::default();
        gate.on_status(NotificationShortcut::NotSetUp);
        assert!(gate.on_fired());
        assert!(gate.pending());
        assert!(!gate.on_fired());
        gate.on_shown();
        assert!(!gate.pending());
        assert!(!gate.on_fired());
    }

    #[test]
    fn a_reminder_before_the_first_status_raises_the_hint_when_it_comes() {
        let mut gate = HintGate::default();
        assert!(!gate.on_fired());
        assert!(gate.on_status(NotificationShortcut::Foreign));
        assert!(gate.pending());
    }

    #[test]
    fn a_reminder_before_a_status_that_needs_nothing_raises_nothing_later() {
        let mut gate = HintGate::default();
        gate.on_fired();
        assert!(!gate.on_status(NotificationShortcut::SetUp));
        // Removed afterwards: the old reminder does not count.
        assert!(!gate.on_status(NotificationShortcut::NotSetUp));
        assert!(!gate.pending());
    }

    #[test]
    fn no_hint_when_windows_shows_the_reminders_or_nothing_can_be_set_up() {
        for status in [
            NotificationShortcut::SetUp,
            NotificationShortcut::PointsElsewhere { target: "x".into() },
            NotificationShortcut::DevBuild,
            NotificationShortcut::Unsupported,
        ] {
            let mut gate = HintGate::default();
            gate.on_status(status.clone());
            assert!(!gate.on_fired(), "{status:?}");
        }
    }

    #[test]
    fn setting_up_while_the_hint_waits_takes_it_back() {
        let mut gate = HintGate::default();
        gate.on_status(NotificationShortcut::NotSetUp);
        gate.on_fired();
        gate.on_status(NotificationShortcut::SetUp);
        assert!(!gate.pending());
    }

    #[test]
    fn removing_does_not_silence_the_hint() {
        let mut gate = HintGate::default();
        gate.on_status(NotificationShortcut::SetUp);
        gate.on_status(NotificationShortcut::NotSetUp);
        assert!(gate.on_fired());
    }

    /// The wire shape the frontend reads, from the same fixture its own
    /// contract test reads.
    #[test]
    fn the_status_reaches_the_frontend_in_the_shape_of_the_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/notificationShortcut.json"))
                .expect("fixture parses");
        let written: Vec<serde_json::Value> = [
            NotificationShortcut::Unsupported,
            NotificationShortcut::DevBuild,
            NotificationShortcut::NotSetUp,
            NotificationShortcut::Foreign,
            NotificationShortcut::SetUp,
            NotificationShortcut::PointsElsewhere {
                target: r"D:\Aperio\aperio.exe".into(),
            },
        ]
        .into_iter()
        .map(|state| serde_json::to_value(ShortcutStatus::from(state)).unwrap())
        .collect();
        assert_eq!(serde_json::Value::Array(written), fixture);
    }

    #[test]
    fn off_windows_there_is_nothing_to_create() {
        assert_eq!(unsupported().code, "unsupported");
    }
}
