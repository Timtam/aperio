//! The desktop's OS notifications, and what a click on one opens (DESIGN
//! §14.3, TODO A5).
//!
//! On Windows Aperio builds the toast itself with tauri-winrt-notification,
//! the crate tauri-plugin-notification uses underneath. The plugin (2.3.3)
//! has no click hook on the desktop: its `show()` reads title, body, icon and
//! sound and nothing else (`desktop.rs:26-53`). A toast built here carries an
//! `Activated` handler instead. A click on the banner while Aperio runs,
//! hidden in the tray included, shows the window and hands the page what the
//! reminder was for: the keys `reminderTapData` writes on the phone
//! (`host_core::reminders::TapData`), or `{dayStart: true}`.
//!
//! What this does not reach yet: a click in the notification center (Win+N)
//! after the banner went away, and any click once Aperio has quit. Windows
//! delivers those only to a COM activator registered under the shortcut's
//! `ToastActivatorCLSID` (stage 2, TODO A5).
//!
//! macOS and Linux keep the plugin, and a click there does what it did
//! before.

use std::sync::Mutex;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tracing::info;

/// What a click emits to the page, which then takes the tap with
/// [`take_reminder_tap`]. Mirrored in `src/api/client.ts`.
pub const REMINDER_TAP_EVENT: &str = "reminder-tap";

/// One OS notification.
pub struct ToastRequest {
    pub title: String,
    /// `None` shows the title alone: an all-day reminder before the page has
    /// pushed its labels has no body, and an empty line would read as one.
    pub body: Option<String>,
    /// The sound name `os_notification` chose (§14.4). `None` is silent.
    pub sound: Option<&'static str>,
    /// What a click opens. `None`: a click shows the window and nothing more.
    pub tap: Option<Value>,
}

/// The tap of the last click, until the page takes it. The page may not be
/// listening yet (a click while the webview loads), so the click leaves it
/// here and the page asks.
#[derive(Default)]
pub struct ReminderTapState(Mutex<Option<Value>>);

impl ReminderTapState {
    /// Keep a click's tap. A later click replaces an earlier one the page has
    /// not taken: it is the one the user just asked for. Off Windows nothing
    /// clicks yet (see the module note), so only the tests call it there.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn store(&self, tap: Value) {
        *self.0.lock().expect("reminder tap poisoned") = Some(tap);
    }

    /// Hand the tap out once, so a reload of the page never reopens it.
    fn take(&self) -> Option<Value> {
        self.0.lock().expect("reminder tap poisoned").take()
    }
}

/// The page takes the tap of the last click, if there is one.
#[tauri::command]
pub fn take_reminder_tap(state: tauri::State<'_, ReminderTapState>) -> Option<Value> {
    state.take()
}

/// Show an OS notification. Never fails the caller: a toast that cannot be
/// shown is logged, as the plugin did.
pub fn show_toast<R: Runtime>(app: &AppHandle<R>, request: ToastRequest) {
    #[cfg(windows)]
    windows_toast::show(app, request);
    #[cfg(not(windows))]
    plugin_toast(app, request);
}

/// A click on a toast: bring Aperio forward and tell the page. Runs on a
/// WinRT thread, not the main one; window calls and `emit` post to the event
/// loop from there. No store work here. Only Windows' toast has a click to
/// report; the plugin's on macOS and Linux has none.
#[cfg_attr(not(windows), allow(dead_code))]
fn on_click<R: Runtime>(app: &AppHandle<R>, tap: Option<Value>) {
    crate::tray::show_main_window(app);
    let Some(tap) = tap else {
        info!("notification clicked");
        return;
    };
    // The kind only: ids name calendars, which stay out of the log.
    info!(kind = tap_kind(&tap), "reminder notification clicked");
    if let Some(state) = app.try_state::<ReminderTapState>() {
        state.store(tap);
    }
    let _ = app.emit(REMINDER_TAP_EVENT, ());
}

/// What a tap is for, for the log.
#[cfg_attr(not(windows), allow(dead_code))]
fn tap_kind(tap: &Value) -> &'static str {
    if tap.get("dayStart") == Some(&Value::Bool(true)) {
        return "dayStart";
    }
    match tap.get("itemKind").and_then(Value::as_str) {
        Some("event") => "event",
        Some("task") => "task",
        _ => "unknown",
    }
}

#[cfg(windows)]
mod windows_toast {
    use std::str::FromStr;

    use tauri::{AppHandle, Runtime};
    use tauri_winrt_notification::{Duration, Sound, Toast};
    use tracing::warn;

    use super::{on_click, ToastRequest};

    /// The id a toast goes out under: the identifier, or PowerShell's for a
    /// development build, which has no Start menu shortcut to carry it. The
    /// rule is the plugin's (`platform::toasts_use_powershell_id`), so a toast
    /// of Aperio's own goes out where the plugin's went.
    pub(super) fn app_id(identifier: &str, dev_build: bool) -> String {
        if dev_build {
            Toast::POWERSHELL_APP_ID.to_string()
        } else {
            identifier.to_string()
        }
    }

    /// The sound a toast asks for, from the name `os_notification` chose.
    /// What notify-rust did with the same name, and it matters: a fresh
    /// `Toast` has empty audio, which Windows plays as its default sound, so a
    /// silent reminder (or one whose custom sound Aperio already played) must
    /// say `None` out loud or it rings.
    pub(super) fn sound(name: Option<&str>) -> Option<Sound> {
        name.and_then(|name| Sound::from_str(name).ok())
    }

    pub(super) fn show<R: Runtime>(app: &AppHandle<R>, request: ToastRequest) {
        let app = app.clone();
        let app_id = app_id(&app.config().identifier, crate::platform::is_dev_build());
        // Built and shown on a blocking thread, from plain data: the toast
        // holds COM delegates and is never moved across threads, and `show`
        // does COM work and sleeps, which the caller must not wait for.
        tauri::async_runtime::spawn_blocking(move || {
            let ToastRequest {
                title,
                body,
                sound: sound_name,
                tap,
            } = request;
            let mut toast = Toast::new(&app_id).title(&title);
            if let Some(body) = body.as_deref() {
                toast = toast.text1(body);
            }
            let clicked = app.clone();
            let toast = toast
                .sound(sound(sound_name))
                .duration(Duration::Short)
                .on_activated(move |_arguments| {
                    on_click(&clicked, tap.clone());
                    Ok(())
                });
            if let Err(err) = toast.show() {
                warn!(?err, "failed to show the notification");
            }
        });
    }
}

#[cfg(not(windows))]
fn plugin_toast<R: Runtime>(app: &AppHandle<R>, request: ToastRequest) {
    use tauri_plugin_notification::NotificationExt;
    use tracing::warn;
    let builder = app.notification().builder().title(&request.title);
    let builder = match request.body {
        Some(body) => builder.body(body),
        None => builder,
    };
    let builder = match request.sound {
        Some(name) => builder.sound(name),
        None => builder,
    };
    if let Err(err) = builder.show() {
        warn!(?err, "failed to dispatch notification");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tap_is_taken_once() {
        let state = ReminderTapState::default();
        assert_eq!(state.take(), None);
        state.store(json!({"itemId": "a"}));
        assert_eq!(state.take(), Some(json!({"itemId": "a"})));
        assert_eq!(state.take(), None, "a reload must not reopen it");
    }

    #[test]
    fn a_later_click_replaces_one_not_taken_yet() {
        let state = ReminderTapState::default();
        state.store(json!({"itemId": "a"}));
        state.store(json!({"itemId": "b"}));
        assert_eq!(state.take(), Some(json!({"itemId": "b"})));
        assert_eq!(state.take(), None);
    }

    #[test]
    fn the_log_names_a_taps_kind_only() {
        assert_eq!(tap_kind(&json!({"dayStart": true})), "dayStart");
        assert_eq!(
            tap_kind(&json!({"itemKind": "event", "itemId": "x"})),
            "event"
        );
        assert_eq!(
            tap_kind(&json!({"itemKind": "task", "itemId": "x"})),
            "task"
        );
        assert_eq!(tap_kind(&json!({"title": "x"})), "unknown");
    }

    #[cfg(windows)]
    #[test]
    fn a_release_build_sends_under_its_identifier_a_dev_build_under_powershells() {
        assert_eq!(
            windows_toast::app_id("com.aperio.app", false),
            "com.aperio.app"
        );
        assert_eq!(
            windows_toast::app_id("com.aperio.app", true),
            tauri_winrt_notification::Toast::POWERSHELL_APP_ID
        );
    }

    /// Each §14.4 sound source, through the name `os_notification` picks, to
    /// what the toast asks Windows for. A `None` that turned into the default
    /// sound would ring a silent reminder, or ring a custom one twice.
    #[cfg(windows)]
    #[test]
    fn every_sound_source_reaches_the_toast_as_before() {
        use crate::os_notification::reminder_sound_name;
        use cal_core::SoundSource;
        use tauri_winrt_notification::Sound;
        let custom = SoundSource::Custom {
            sha256: "abc".into(),
        };
        let toast = |source: &SoundSource, found: bool| {
            windows_toast::sound(reminder_sound_name(source, found))
        };
        assert!(matches!(
            toast(&SoundSource::System, false),
            Some(Sound::Default)
        ));
        assert!(matches!(toast(&custom, false), Some(Sound::Default)));
        assert!(toast(&SoundSource::Silent, false).is_none());
        assert!(toast(&custom, true).is_none());
    }
}
