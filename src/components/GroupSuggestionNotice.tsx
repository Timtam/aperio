import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  findGroupSuggestions,
  memberFromEvent,
  type EventGroup,
  type SuggestionDecline,
} from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import {
  declineGroupSuggestion,
  groupEvents,
  groupSuggestionDeclines,
} from '../api/client';
import type { CalendarEvent } from '../api/types';
import { seriesIdOf } from '../intl/recurrence';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';

/**
 * "These two look like the same appointment — are they?"
 * (`DESIGN-event-groups.md`, Stufe 3.)
 *
 * ONE row, above the day, and only when there is something to ask. It is the
 * whole proactive surface of the feature, and its size is the point: an offer
 * that cannot be dismissed for good is a daily interruption, and for a
 * screen-reader user it is one more thing to walk past every morning before
 * reaching the actual day.
 *
 * So both answers are final. **Group** makes the group; **Not the same** is
 * remembered (migration 0037) and the pair is never offered again — on any
 * device, because the record syncs.
 *
 * Nothing is decided by Aperio: the row states what it noticed and waits.
 */
export function GroupSuggestionNotice({
  events,
  groups,
}: {
  /** The day's events as the view renders them.
   *
   *  Folded or not makes no difference here: folding only removes copies that
   *  are ALREADY in a group, and those are exactly the ones no suggestion is
   *  ever made about. */
  events: readonly CalendarEvent[];
  /** What is already grouped, so the obvious is not offered again. */
  groups: readonly EventGroup[];
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { calendars } = useCalendarStore();
  const { invalidateData, dataVersion } = useDialogState();
  // `null` until the declines are known — NOT an empty list. Suggesting a pair
  // the user already refused is the one failure this feature must not have, so
  // it stays quiet until it knows, and stays quiet for good if the read fails.
  const [declines, setDeclines] = useState<SuggestionDecline[] | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void groupSuggestionDeclines()
      .then((rows) => {
        if (!cancelled) setDeclines(rows);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [dataVersion]);

  const suggestion = useMemo(
    () =>
      declines == null
        ? null
        : (findGroupSuggestions(events, groups, declines, seriesIdOf)[0] ?? null),
    [events, groups, declines],
  );

  const calendarName = useCallback(
    (id: string) => calendars.find((c) => c.id === id)?.name ?? id,
    [calendars],
  );

  if (!suggestion) return null;
  const { first, second } = suggestion;

  const refOf = (ev: CalendarEvent) => ({
    calendar_id: ev.calendar_id,
    event_id: seriesIdOf(ev),
  });

  const accept = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await groupEvents([
        memberFromEvent({ ...first, id: seriesIdOf(first) }),
        memberFromEvent({ ...second, id: seriesIdOf(second) }),
      ]);
      announce(t('views.groupSuggestion.grouped', { title: first.title }));
      invalidateData();
    } catch {
      announce(t('views.groupSuggestion.failed'));
    } finally {
      setBusy(false);
    }
  };

  const refuse = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await declineGroupSuggestion(refOf(first), refOf(second));
      announce(t('views.groupSuggestion.declined'));
      invalidateData();
    } catch {
      announce(t('views.groupSuggestion.failed'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="day-suggestion" aria-label={t('views.groupSuggestion.title')}>
      <FocusableNote className="form__message">
        {t('views.groupSuggestion.message', {
          title: first.title,
          first: calendarName(first.calendar_id),
          second: calendarName(second.calendar_id),
        })}
      </FocusableNote>
      <div className="form__actions">
        <button
          type="button"
          className="form__action form__action--primary"
          onClick={() => void accept()}
          aria-disabled={busy || undefined}
        >
          {t('views.groupSuggestion.accept')}
        </button>
        <button
          type="button"
          className="form__action"
          onClick={() => void refuse()}
          aria-disabled={busy || undefined}
        >
          {t('views.groupSuggestion.refuse')}
        </button>
      </div>
    </section>
  );
}
