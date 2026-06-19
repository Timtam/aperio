//! Custom notification-sound commands (DESIGN.md §14.4 / §19.11.7).
//!
//! The sound *configuration* (which sound plays at which level) lives
//! in `user_prefs` and goes through the generic `set_user_pref`
//! command. This module is only the binary side: importing a user's
//! audio file into the content-addressed store, listing/deleting what's
//! there, and previewing a sound (the SoundPicker's "Test" button).
//!
//! Imported files live at `<data_dir>/assets/sounds/<sha256>.<ext>`.
//! Content-addressing means identical audio dedupes automatically and
//! the §19.10 sync asset store can fetch a referenced sound by hash on
//! another device. The actual cross-device propagation is handled by
//! `sound_assets::sync_assets`; this module just gets bytes onto disk
//! with the right name.

use std::path::PathBuf;

use tauri::State;

use super::{CommandError, CommandResult};
use crate::audio::AudioPlayer;
use crate::sound_assets::{import_sound as core_import_sound, ImportSoundError};
use crate::sound_assets::{list_local_sounds, local_sound_path, ImportedSound};

/// Newtype Tauri state carrying the resolved
/// `<data_dir>/assets/sounds/` path, so the sound commands don't
/// re-probe the data dir on every call. Wrapped so the State lookup
/// doesn't collide with other `PathBuf` state.
#[derive(Clone)]
pub struct SoundsDir(pub PathBuf);

/// Map the shared importer's error onto the command layer's `CommandError`.
fn map_import_err(err: ImportSoundError) -> CommandError {
    match err {
        ImportSoundError::UnsupportedFormat(_) | ImportSoundError::TooLarge { .. } => {
            CommandError {
                code: "invalid_input",
                message: err.to_string(),
            }
        }
        ImportSoundError::Io(_) => CommandError {
            code: "internal",
            message: err.to_string(),
        },
    }
}

/// Import an audio file into the custom-sound store. Validates + content-hashes
/// it and copies it to `<sounds_dir>/<sha256>.<ext>` (a no-op if an identical
/// file is already there). Returns the hash + extension so the caller can write
/// `SoundSource::Custom { sha256 }` into the relevant pref. The validation +
/// hashing live in `host_core::sound_assets::import_sound` so the desktop and
/// the mobile cal-ffi Host import through the same path.
#[tauri::command]
pub async fn import_sound(
    sounds_dir: State<'_, SoundsDir>,
    path: String,
) -> CommandResult<ImportedSound> {
    core_import_sound(&sounds_dir.0, &PathBuf::from(&path)).map_err(map_import_err)
}

/// List every custom sound currently in the store.
#[tauri::command]
pub async fn list_custom_sounds(
    sounds_dir: State<'_, SoundsDir>,
) -> CommandResult<Vec<ImportedSound>> {
    let list = list_local_sounds(&sounds_dir.0).map_err(|e| CommandError {
        code: "internal",
        message: format!("cannot list sounds: {e}"),
    })?;
    Ok(list
        .into_iter()
        .map(|(sha256, ext)| ImportedSound { sha256, ext })
        .collect())
}

/// Play a sound once for the SoundPicker's "Test" button. Only
/// `Custom` sources do anything here — System/Silent are the OS's job
/// (or the absence of one) and there's nothing for us to play.
#[tauri::command]
pub async fn preview_sound(
    audio: State<'_, AudioPlayer>,
    sounds_dir: State<'_, SoundsDir>,
    config: cal_core::SoundConfig,
) -> CommandResult<()> {
    if let cal_core::SoundSource::Custom { sha256 } = &config.source {
        match local_sound_path(&sounds_dir.0, sha256) {
            Some(p) => audio.play_file(p),
            None => {
                return Err(CommandError {
                    code: "not_found",
                    message: "custom sound file not found".into(),
                })
            }
        }
    }
    Ok(())
}

/// Delete a custom sound from the store by hash. Idempotent — a missing
/// file is treated as already-gone. The pref(s) still referencing it
/// (if any) fall back to System at resolve time.
#[tauri::command]
pub async fn delete_custom_sound(
    sounds_dir: State<'_, SoundsDir>,
    sha256: String,
) -> CommandResult<()> {
    if let Some(path) = local_sound_path(&sounds_dir.0, &sha256) {
        std::fs::remove_file(&path).map_err(|e| CommandError {
            code: "internal",
            message: format!("cannot delete sound file: {e}"),
        })?;
    }
    Ok(())
}
