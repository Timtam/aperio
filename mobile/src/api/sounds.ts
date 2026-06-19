// Custom-sound api-client — the content-addressed audio store behind
// SoundSource::Custom (§14.4 / §19.2.2), over the cal-ffi Host. Files live at
// <data_dir>/assets/sounds/<sha256>.<ext> and sync between devices through the
// regular sync round (host-core's DesktopSyncRoundHooks). Bytes never cross the
// bridge — each method returns the on-disk PATH, which the UI plays (expo-audio)
// and the scheduler turns into the Android notification-channel sound.

import CalFfi from '../../modules/cal-ffi';

/** One custom sound on disk: its content hash, container extension, and the
 *  absolute local path (for preview + the Android notification channel). */
export interface CustomSound {
  sha256: string;
  ext: string;
  path: string;
}

/** Import an audio file (a local `path`/uri from the document picker) into the
 *  store; returns the imported sound. Rejects (throws) on an unsupported format
 *  or an over-cap size — the validation lives in the shared Rust importer. */
export const importSound = async (path: string): Promise<CustomSound> =>
  JSON.parse(await CalFfi.importSoundJson(path)) as CustomSound;

/** Every custom sound currently in the store. */
export const listCustomSounds = async (): Promise<CustomSound[]> =>
  JSON.parse(await CalFfi.listCustomSoundsJson()) as CustomSound[];

/** The absolute on-disk path of a custom sound by hash, or null when it isn't
 *  present locally (not yet synced / deleted). */
export const customSoundPath = (sha256: string): Promise<string | null> =>
  CalFfi.customSoundPath(sha256);

/** Delete a custom sound from the store by hash (idempotent). Any pref still
 *  referencing it falls back to the system sound at resolve time. */
export const deleteCustomSound = (sha256: string): Promise<void> =>
  CalFfi.deleteCustomSound(sha256);
