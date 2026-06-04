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

use sha2::{Digest, Sha256};
use tauri::State;

use super::{CommandError, CommandResult};
use crate::audio::AudioPlayer;
use crate::sound_assets::{list_local_sounds, local_sound_path};

/// Max size of an imported sound. Mirrors the §19.2.2 cap — large
/// enough for any realistic notification chime, small enough that
/// syncing the blob across devices stays cheap.
const MAX_SOUND_BYTES: u64 = 5 * 1024 * 1024;

/// Audio container extensions we accept on import. Kept in sync with
/// `sound_assets::FETCH_EXTENSION_CANDIDATES` (the formats the player's
/// `symphonia-all` decoders handle); anything else is rejected up front
/// rather than failing silently at play time.
const ALLOWED_EXTENSIONS: &[&str] = &["mp3", "ogg", "wav", "m4a", "aac", "flac"];

/// Newtype Tauri state carrying the resolved
/// `<data_dir>/assets/sounds/` path, so the sound commands don't
/// re-probe the data dir on every call. Wrapped so the State lookup
/// doesn't collide with other `PathBuf` state.
#[derive(Clone)]
pub struct SoundsDir(pub PathBuf);

/// A custom sound on disk, identified by content hash + container
/// extension. The frontend only persists `sha256` (in a `SoundConfig`);
/// `ext` is informational for the picker's list UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportedSound {
    pub sha256: String,
    pub ext: String,
}

/// Import an audio file into the custom-sound store. Reads the file at
/// `path`, enforces the size + extension limits, content-hashes it, and
/// copies it to `<sounds_dir>/<sha256>.<ext>` (a no-op if an identical
/// file is already there). Returns the hash + extension so the caller
/// can write `SoundSource::Custom { sha256 }` into the relevant pref.
#[tauri::command]
pub async fn import_sound(
    sounds_dir: State<'_, SoundsDir>,
    path: String,
) -> CommandResult<ImportedSound> {
    let src = PathBuf::from(&path);

    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| ALLOWED_EXTENSIONS.contains(&e.as_str()))
        .ok_or_else(|| CommandError {
            code: "invalid_input",
            message: format!(
                "unsupported sound format; allowed: {}",
                ALLOWED_EXTENSIONS.join(", ")
            ),
        })?;

    // Size-gate before reading the whole file into memory.
    let meta = std::fs::metadata(&src).map_err(|e| CommandError {
        code: "invalid_input",
        message: format!("cannot read sound file: {e}"),
    })?;
    if meta.len() > MAX_SOUND_BYTES {
        return Err(CommandError {
            code: "invalid_input",
            message: format!(
                "sound file too large ({} bytes); limit is {MAX_SOUND_BYTES} bytes",
                meta.len()
            ),
        });
    }

    let bytes = std::fs::read(&src).map_err(|e| CommandError {
        code: "invalid_input",
        message: format!("cannot read sound file: {e}"),
    })?;

    let sha256 = hex_digest(&bytes);

    let dir = sounds_dir.0.clone();
    std::fs::create_dir_all(&dir).map_err(|e| CommandError {
        code: "internal",
        message: format!("cannot create sounds dir: {e}"),
    })?;
    let dest = dir.join(format!("{sha256}.{ext}"));
    // Content-addressed: if the exact bytes are already stored under any
    // extension, don't write a second copy.
    if local_sound_path(&dir, &sha256).is_none() {
        std::fs::write(&dest, &bytes).map_err(|e| CommandError {
            code: "internal",
            message: format!("cannot write sound file: {e}"),
        })?;
    }

    Ok(ImportedSound { sha256, ext })
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

/// Lowercase hex SHA-256 of `bytes`.
fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_digest_of_empty_is_known_sha256() {
        // SHA-256("") = e3b0c442…b855.
        assert_eq!(
            hex_digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }

    #[test]
    fn allowed_extensions_match_player_candidates() {
        // Guards against the import allowlist drifting from the formats
        // the playback layer can actually decode.
        for ext in ALLOWED_EXTENSIONS {
            assert!(crate::sound_assets::FETCH_EXTENSION_CANDIDATES.contains(ext));
        }
    }
}
