import { Alert } from 'react-native';

import { occurrenceIsoOf, seriesIdOf } from '@aperio/shared';

import { CalendarEvent } from '../api/calendar';

// Shared "edit this occurrence vs the whole series" prompt — the mobile analogue
// of the desktop EditEventScopeDialog, and the edit twin of confirmDeleteEvent.
// A concrete occurrence of a recurring series pops an Outlook-style choice before
// the editor opens; a single event / master row opens the editor directly. Used
// by every calendar surface so the scope choice lives in one place and can't be
// missed in a control buried inside the editor.

type Tr = (key: string, vars?: Record<string, unknown>) => string;

/** Params the editor navigation accepts, narrowed to what this helper sets. */
export interface EditEventParams {
  eventId: string | null;
  calendarId: string;
  occurrence?: string | null;
  /** Scope the up-front prompt resolved to; seeds the editor's edit scope. */
  initialScope?: 'occurrence' | 'series' | 'this_and_future';
}

/** Open the event editor for `ev`. For a recurring OCCURRENCE, first pops the
 *  "this occurrence vs whole series" prompt, then opens the editor locked to the
 *  chosen scope; a non-recurring / master row opens directly. `navigate` performs
 *  the actual `navigation.navigate('EventEditor', …)`. */
export function editEventWithScope(
  ev: CalendarEvent,
  t: Tr,
  navigate: (params: EditEventParams) => void,
): void {
  const occurrence = occurrenceIsoOf(ev);
  const open = (initialScope?: 'occurrence' | 'series' | 'this_and_future') =>
    navigate({
      eventId: seriesIdOf(ev),
      calendarId: ev.calendar_id,
      occurrence,
      initialScope,
    });

  if (occurrence == null) {
    open();
    return;
  }

  Alert.alert(
    t('dialogs.editScope.title'),
    t('dialogs.editScope.message', { title: ev.title }),
    [
      { text: t('dialogs.editScope.cancel'), style: 'cancel' },
      {
        text: t('dialogs.editScope.occurrence'),
        onPress: () => open('occurrence'),
      },
      {
        text: t('dialogs.editScope.thisAndFuture'),
        onPress: () => open('this_and_future'),
      },
      { text: t('dialogs.editScope.series'), onPress: () => open('series') },
    ],
  );
}
