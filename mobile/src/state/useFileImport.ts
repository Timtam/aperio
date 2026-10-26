import { useCallback, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import * as DocumentPicker from 'expo-document-picker';
import { importFailureSentence, type ImportFormat } from '@aperio/shared';

import { listCalendars } from '../api/calendar';
import { fileImportFormats, readImportFile } from '../api/fileImport';
import { logLine } from '../api/logs';
import type { RootStackScreenProps } from '../navigation/types';
import { useShowHiddenCalendarTargets } from '../settings/hiddenTargets';
import { useCalendarVisibility } from './calendarVisibility';
import { importTarget, pathOfPicked, readAndPresent } from './fileImportFlow';
import { readLastUsedCalendar } from './lastUsedCalendar';

type Navigation = RootStackScreenProps<'Calendars'>['navigation'];

/**
 * Bring a calendar file in from the document picker (DESIGN-file-import.md,
 * I5): the calendar list's "Import appointments …". What the plugin read
 * decides what opens (`readAndPresent`); a sentence — a file that failed, or
 * held nothing — is `notice`, until `clearNotice`. Mirrors the desktop's
 * `useFileImport`.
 */
export function useFileImport(navigation: Navigation): {
  importFromPicker: () => Promise<void>;
  busy: boolean;
  notice: string | null;
  clearNotice: () => void;
} {
  const { t, i18n } = useTranslation();
  const { hidden } = useCalendarVisibility();
  const includeHidden = useShowHiddenCalendarTargets();
  const [busy, setBusy] = useState(false);
  // A second tap before the render that disables the button opens no second
  // picker.
  const busyRef = useRef(false);
  const [notice, setNotice] = useState<string | null>(null);

  const importFromPicker = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setNotice(null);
    try {
      let formats: ImportFormat[] = [];
      try {
        formats = await fileImportFormats(i18n.language);
      } catch {
        // No filter then: the picker offers every file, and the plugin says
        // whether it reads the one picked.
      }
      const types = [...new Set(formats.flatMap((format) => format.mime_types))];
      const picked = await DocumentPicker.getDocumentAsync({
        type: types.length > 0 ? types : '*/*',
        copyToCacheDirectory: true,
        multiple: false,
      });
      const asset = picked.canceled ? undefined : picked.assets[0];
      if (asset == null) return;
      await readAndPresent(asset.name, pathOfPicked(asset.uri), {
        t,
        read: readImportFile,
        target: async () => {
          const [calendars, lastUsed] = await Promise.all([
            listCalendars().catch(() => []),
            readLastUsedCalendar().catch(() => null),
          ]);
          return importTarget(calendars, lastUsed, hidden, includeHidden);
        },
        openEditor: (calendarId, event, note) =>
          navigation.navigate('EventEditor', {
            eventId: null,
            calendarId,
            importFrom: event,
            importNote: note,
          }),
        openList: (fileName, read) => navigation.navigate('FileImport', { fileName, read }),
        say: setNotice,
        log: (message) => void logLine('warn', message),
      });
    } catch (err) {
      // The picker itself failed: no file to name, as on the desktop.
      setNotice(importFailureSentence('', err, t));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, [i18n.language, t, hidden, includeHidden, navigation]);

  const clearNotice = useCallback(() => setNotice(null), []);
  return { importFromPicker, busy, notice, clearNotice };
}
