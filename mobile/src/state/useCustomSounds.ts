import { useCallback, useEffect, useState } from 'react';
import * as DocumentPicker from 'expo-document-picker';

import {
  CustomSound,
  deleteCustomSound,
  importSound,
  listCustomSounds,
} from '../api/sounds';
import { refreshRemindersSoon } from '../reminders/scheduler';

// The custom-sound library (§14.4 / §19.2.2): list, import (via the OS document
// picker), and delete. Backs the SoundSelect picker. Import/delete kick a
// reminder reschedule so already-scheduled OS notifications re-resolve their
// sound against the new library (the mobile twin of the desktop
// invalidateReminders()).

export interface CustomSoundsBinding {
  sounds: CustomSound[];
  loading: boolean;
  reload: () => Promise<void>;
  /** Open the OS audio picker and import the choice; returns the imported sound
   *  (or null if the user cancelled). Throws on an unsupported format / size. */
  importFromPicker: () => Promise<CustomSound | null>;
  remove: (sha256: string) => Promise<void>;
}

export function useCustomSounds(): CustomSoundsBinding {
  const [sounds, setSounds] = useState<CustomSound[]>([]);
  const [loading, setLoading] = useState(true);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setSounds(await listCustomSounds());
    } catch {
      setSounds([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const importFromPicker = useCallback(async (): Promise<CustomSound | null> => {
    const res = await DocumentPicker.getDocumentAsync({
      type: 'audio/*',
      copyToCacheDirectory: true,
      multiple: false,
    });
    const asset = res.canceled ? undefined : res.assets[0];
    if (asset == null) return null;
    // The picker returns a `file://` URI (copyToCacheDirectory), but the Rust
    // importer reads a filesystem PATH via std::fs, which has no URI awareness —
    // strip the scheme (and percent-decode) before handing it over, else every
    // import fails NotFound.
    const local = asset.uri.startsWith('file://')
      ? decodeURIComponent(asset.uri.replace(/^file:\/\//, ''))
      : asset.uri;
    // importSound validates format + size Rust-side; let a rejection propagate
    // so the caller can announce it.
    const imported = await importSound(local);
    await reload();
    refreshRemindersSoon();
    return imported;
  }, [reload]);

  const remove = useCallback(
    async (sha256: string) => {
      await deleteCustomSound(sha256);
      await reload();
      refreshRemindersSoon();
    },
    [reload],
  );

  return { sounds, loading, reload, importFromPicker, remove };
}
