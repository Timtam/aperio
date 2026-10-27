import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { Alert, AppState, Platform } from 'react-native';
import * as Linking from 'expo-linking';
import { errorMessageText, importFailureSentence } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import { listCalendars } from '../api/calendar';
import { readImportFile } from '../api/fileImport';
import { logLine } from '../api/logs';
import { navigateNested, navigationRef } from '../navigation/navigationRef';
import { useShowHiddenCalendarTargets } from '../settings/hiddenTargets';
import { useCalendarVisibility } from './calendarVisibility';
import { importTarget, incomingFileOfUrl, readAndPresent } from './fileImportFlow';
import { newImportToken } from './importProgress';
import { readLastUsedCalendar } from './lastUsedCalendar';

/** Until the navigation can take the editor or the list: at once on any
 *  open after the first, a moment when the file started the app. */
async function whenNavigationReady(): Promise<void> {
  for (let tries = 0; tries < 100 && !navigationRef.isReady(); tries += 1) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

/** Whether the URL the app was launched with was taken: once per run of the
 *  JavaScript, as `Linking.getInitialURL` answers with it for the whole run. */
let initialUrlTaken = false;

/**
 * A calendar file shared into Aperio or opened in it from another app
 * (DESIGN-file-import.md, I5): iOS hands an "Open in" to Linking as a
 * `file://` URL — at launch, or while the app runs —; Android hands a share
 * or an "open with" to the activity as an intent, which the native module
 * copies into the cache (`takeIncomingFileJson`), read at launch and on every
 * return to the foreground. Either opens what the file holds in the calendar
 * tab, as the calendar list's "Import appointments …" does, each import a
 * route of its own.
 *
 * A sentence — a file that failed, or held nothing — is a native alert: it
 * presents over whatever is open, a sheet included, where a modal rendered
 * at the root cannot on iOS.
 */
export function IncomingFileImport() {
  const { t } = useTranslation();
  const { hidden } = useCalendarVisibility();
  const includeHidden = useShowHiddenCalendarTargets();
  // Read when a file arrives, as they are then: the listeners below are set
  // up once and outlive a change of language or of the hidden calendars.
  const latest = useRef({ t, hidden, includeHidden });
  latest.current = { t, hidden, includeHidden };

  useEffect(() => {
    const say = (sentence: string) => {
      const { t: translate } = latest.current;
      Alert.alert(translate('fileImport.dialogTitle'), sentence, [
        { text: translate('dialogs.close') },
      ]);
    };
    const present = async (file: { path: string; name: string }) => {
      await whenNavigationReady();
      const { t: translate } = latest.current;
      await readAndPresent(file.name, file.path, {
        t: translate,
        read: readImportFile,
        target: async () => {
          const [calendars, lastUsed] = await Promise.all([
            listCalendars().catch(() => []),
            readLastUsedCalendar().catch(() => null),
          ]);
          const { hidden: shown, includeHidden: offerHidden } = latest.current;
          return importTarget(calendars, lastUsed, shown, offerHidden);
        },
        openEditor: (calendarId, event, note) =>
          navigateNested('CalendarTab', 'EventEditor', {
            eventId: null,
            calendarId,
            importFrom: event,
            importNote: note,
            importToken: newImportToken(),
          }),
        openList: (fileName, read) =>
          navigateNested('CalendarTab', 'FileImport', {
            fileName,
            read,
            token: newImportToken(),
          }),
        say,
        log: (message) => void logLine('warn', message),
      });
    };

    if (Platform.OS === 'ios') {
      const open = (url: string | null) => {
        const file = url ? incomingFileOfUrl(url) : null;
        if (file) void present(file);
      };
      if (!initialUrlTaken) {
        initialUrlTaken = true;
        void Linking.getInitialURL().then(open);
      }
      const subscription = Linking.addEventListener('url', ({ url }) => open(url));
      return () => subscription.remove();
    }
    const take = () => {
      CalFfi.takeIncomingFileJson()
        .then((json) => {
          if (json) void present(JSON.parse(json) as { path: string; name: string });
        })
        .catch((err: unknown) => {
          // A file that could not be opened: the error names it.
          void logLine('warn', `taking a shared calendar file failed: ${errorMessageText(err)}`);
          say(importFailureSentence(errorMessageText(err), err, latest.current.t));
        });
    };
    take();
    const subscription = AppState.addEventListener('change', (state) => {
      if (state === 'active') take();
    });
    return () => subscription.remove();
  }, []);

  return null;
}
