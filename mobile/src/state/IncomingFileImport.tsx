import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AppState, Platform } from 'react-native';
import * as Linking from 'expo-linking';
import { errorMessageText } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import { listCalendars } from '../api/calendar';
import { readImportFile } from '../api/fileImport';
import { logLine } from '../api/logs';
import { AppDialog } from '../components/AppDialog';
import { navigateNested, navigationRef } from '../navigation/navigationRef';
import { useShowHiddenCalendarTargets } from '../settings/hiddenTargets';
import { useCalendarVisibility } from './calendarVisibility';
import { importTarget, incomingFileOfUrl, readAndPresent } from './fileImportFlow';
import { readLastUsedCalendar } from './lastUsedCalendar';

/** Until the navigation can take the editor or the list: at once on any
 *  open after the first, a moment when the file started the app. */
async function whenNavigationReady(): Promise<void> {
  for (let tries = 0; tries < 100 && !navigationRef.isReady(); tries += 1) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

/**
 * A calendar file shared into Aperio or opened in it from another app
 * (DESIGN-file-import.md, I5): iOS hands an "Open in" to Linking as a
 * `file://` URL — at launch, or while the app runs —; Android hands a share
 * or an "open with" to the activity as an intent, which the native module
 * copies into the cache (`takeIncomingFileJson`), read at launch and on every
 * return to the foreground. Either opens what the file holds in the calendar
 * tab, as the calendar list's "Import appointments …" does; a sentence — a
 * file that failed, or held nothing — shows here, over whatever is open.
 */
export function IncomingFileImport() {
  const { t } = useTranslation();
  const { hidden } = useCalendarVisibility();
  const includeHidden = useShowHiddenCalendarTargets();
  // Read when a file arrives, as they are then.
  const hiddenRef = useRef(hidden);
  hiddenRef.current = hidden;
  const includeHiddenRef = useRef(includeHidden);
  includeHiddenRef.current = includeHidden;
  const [notice, setNotice] = useState<string | null>(null);

  const present = useCallback(
    async (file: { path: string; name: string }) => {
      await whenNavigationReady();
      await readAndPresent(file.name, file.path, {
        t,
        read: readImportFile,
        target: async () => {
          const [calendars, lastUsed] = await Promise.all([
            listCalendars().catch(() => []),
            readLastUsedCalendar().catch(() => null),
          ]);
          return importTarget(calendars, lastUsed, hiddenRef.current, includeHiddenRef.current);
        },
        openEditor: (calendarId, event, note) =>
          navigateNested('CalendarTab', 'EventEditor', {
            eventId: null,
            calendarId,
            importFrom: event,
            importNote: note,
          }),
        openList: (fileName, read) =>
          navigateNested('CalendarTab', 'FileImport', { fileName, read }),
        say: setNotice,
        log: (message) => void logLine('warn', message),
      });
    },
    [t],
  );

  useEffect(() => {
    if (Platform.OS === 'ios') {
      const open = (url: string | null) => {
        const file = url ? incomingFileOfUrl(url) : null;
        if (file) void present(file);
      };
      void Linking.getInitialURL().then(open);
      const subscription = Linking.addEventListener('url', ({ url }) => open(url));
      return () => subscription.remove();
    }
    const take = () => {
      CalFfi.takeIncomingFileJson()
        .then((json) => {
          if (json) void present(JSON.parse(json) as { path: string; name: string });
        })
        .catch((err: unknown) => {
          void logLine('warn', `taking a shared calendar file failed: ${errorMessageText(err)}`);
        });
    };
    take();
    const subscription = AppState.addEventListener('change', (state) => {
      if (state === 'active') take();
    });
    return () => subscription.remove();
  }, [present]);

  return (
    <AppDialog
      visible={notice != null}
      title={t('fileImport.dialogTitle')}
      message={notice ?? ''}
      cancelLabel={t('dialogs.close')}
      onCancel={() => setNotice(null)}
    />
  );
}
