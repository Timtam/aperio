//! What sound an OS notification asks for (DESIGN §14.4), in one place for
//! the reminder worker and for the notifications the frontend sends.
//!
//! tauri-plugin-notification hands the sound name to notify-rust. On Windows
//! notify-rust sends `<audio silent="true"/>` when there is none (notify-rust
//! 4.17 `windows.rs:8-11`, tauri-winrt-notification 0.7.2 `lib.rs:479-482`),
//! and only the exact name `"Default"` maps to `Sound::Default`, which leaves
//! the audio element out so Windows plays its own notification sound
//! (`lib.rs:121-123`). Without a name every Aperio notification on Windows
//! was silent, "System sound" included. macOS reads a name as a sound file
//! (mac-notification-sys 0.6 `notification.rs:53-60`), so the name is set on
//! Windows only.

use cal_core::SoundSource;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// The sound name that asks for the system's default notification sound.
#[cfg(windows)]
pub const SYSTEM_SOUND: Option<&str> = Some("Default");
/// The sound name that asks for the system's default notification sound.
#[cfg(not(windows))]
pub const SYSTEM_SOUND: Option<&str> = None;

/// The sound name a reminder's notification carries. A custom sound Aperio
/// plays itself, so that notification stays silent. A custom sound whose file
/// is missing falls back to the system sound: no reminder goes off silently
/// because a file is gone (§14.4).
pub fn reminder_sound_name(source: &SoundSource, custom_file_found: bool) -> Option<&'static str> {
    match source {
        SoundSource::System => SYSTEM_SOUND,
        SoundSource::Silent => None,
        SoundSource::Custom { .. } if custom_file_found => None,
        SoundSource::Custom { .. } => SYSTEM_SOUND,
    }
}

/// An OS notification the frontend sends (the day-start review, sync
/// conflicts), with the system sound. Through here rather than the plugin's
/// own JavaScript call, so its sound follows the same rule as a reminder's.
#[tauri::command]
pub fn notify_system(app: AppHandle, title: String, body: String) -> Result<(), String> {
    let builder = app.notification().builder().title(title).body(body);
    let builder = match SYSTEM_SOUND {
        Some(name) => builder.sound(name),
        None => builder,
    };
    builder.show().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_system_sound_is_windows_default_sound() {
        assert_eq!(SYSTEM_SOUND, Some("Default"));
    }

    #[cfg(not(windows))]
    #[test]
    fn the_system_sound_names_nothing_off_windows() {
        assert_eq!(SYSTEM_SOUND, None);
    }

    #[test]
    fn a_reminder_with_the_system_sound_asks_for_it() {
        assert_eq!(reminder_sound_name(&SoundSource::System, false), SYSTEM_SOUND);
    }

    #[test]
    fn a_silent_reminder_and_a_played_custom_sound_ask_for_none() {
        assert_eq!(reminder_sound_name(&SoundSource::Silent, false), None);
        let custom = SoundSource::Custom {
            sha256: "abc".into(),
        };
        assert_eq!(reminder_sound_name(&custom, true), None);
    }

    #[test]
    fn a_missing_custom_sound_falls_back_to_the_system_sound() {
        let custom = SoundSource::Custom {
            sha256: "abc".into(),
        };
        assert_eq!(reminder_sound_name(&custom, false), SYSTEM_SOUND);
    }

    /// The name is a string three crates pass along, and a name the last one
    /// does not know turns silently into no sound (`.ok()` in notify-rust).
    #[cfg(windows)]
    #[test]
    fn windows_reads_only_the_exact_default_name_as_its_sound() {
        use std::str::FromStr;
        use tauri_winrt_notification::Sound;
        assert!(matches!(Sound::from_str("Default"), Ok(Sound::Default)));
        assert!(Sound::from_str("default").is_err());
    }

    /// The comments above, and the dev-build rule in `platform.rs`, quote
    /// these crates' sources by line. A version bump has to send someone back
    /// to read them again.
    #[test]
    fn the_notification_crates_are_the_versions_whose_code_was_read() {
        let lock = include_str!("../../Cargo.lock");
        let version_of = |name: &str| {
            let mut lines = lock.lines();
            while let Some(line) = lines.next() {
                if line == format!("name = \"{name}\"") {
                    return lines.next().map(|v| v.to_string());
                }
            }
            None
        };
        for (name, version) in [
            ("tauri-plugin-notification", "2.3.3"),
            ("notify-rust", "4.17.0"),
            ("tauri-winrt-notification", "0.7.2"),
        ] {
            assert_eq!(
                version_of(name).as_deref(),
                Some(format!("version = \"{version}\"").as_str()),
                "{name} changed: read its notification code again (src-tauri/src/os_notification.rs, platform.rs)",
            );
        }
    }
}
