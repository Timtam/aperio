import {
  isBirthdayEventId,
  isProviderOverride,
  occurrenceIsoOf,
  seriesIdOf,
} from '@aperio/shared';

import { showEventScopeDialog } from './eventScopeDialog';
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
  /** The row's title, passed only for a SYNTHETIC birthday event: its id has no
   *  fetchable row behind it, so the editor's read-only birthday summary has no
   *  other way to name the person. */
  initialTitle?: string;
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
  /** Without `scoped` the editor opens the series itself. `eventId` is the row
   *  the editor loads: the series, unless the row is edited in place. */
  const open = (
    scoped?: {
      // None for the whole series, which opens the series itself.
      occurrence?: string;
      initialScope: 'occurrence' | 'series' | 'this_and_future';
    },
    eventId: string = seriesIdOf(ev),
  ) => {
    navigate({
      eventId,
      calendarId: ev.calendar_id,
      occurrence: scoped?.occurrence,
      initialScope: scoped?.initialScope,
      // A synthetic birthday event opens a read-only summary that can't
      // re-fetch its own name — hand it over from the row.
      initialTitle: isBirthdayEventId(eventId) ? ev.title : undefined,
    });
  };

  if (occurrence == null) {
    open();
    return;
  }

  // In-app dialog (NOT Alert): three scope buttons don't fit a native Android
  // Alert once Cancel is included (it keeps only the first three buttons), so
  // "whole series" would be dropped. Editing isn't destructive, so no notify
  // radio and no danger styling — mirrors the desktop EditEventScopeDialog.
  showEventScopeDialog({
    title: t('dialogs.editScope.title'),
    message: t('dialogs.editScope.message', { title: ev.title }),
    cancelLabel: t('dialogs.editScope.cancel'),
    options: [
      {
        key: 'occurrence',
        label: t('dialogs.editScope.occurrence'),
        // A provider override IS the occurrence: the editor opens it by its own
        // id and saves it in place, as the desktop does. Opened as the series,
        // "just this one" deleted the provider's exception and made a copy.
        run: () =>
          open(
            { occurrence, initialScope: 'occurrence' },
            isProviderOverride(ev) ? ev.id : seriesIdOf(ev),
          ),
      },
      {
        key: 'thisAndFuture',
        label: t('dialogs.editScope.thisAndFuture'),
        run: () => open({ occurrence, initialScope: 'this_and_future' }),
      },
      {
        key: 'series',
        label: t('dialogs.editScope.series'),
        // The whole series opens as the series — its own start, end, rule and
        // exceptions, like a search hit. Seeded from the occurrence, saving
        // moved the series start to that occurrence and the earlier ones
        // disappeared. The scope rides along so the editor can name it.
        run: () => open({ initialScope: 'series' }),
      },
    ],
  });
}
