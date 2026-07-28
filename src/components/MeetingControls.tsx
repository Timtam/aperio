import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import { useAnnouncer } from '../a11y/announcerContext';
import {
  attachMeeting,
  detachMeeting,
  eventMeeting,
  isCommandError,
  listAccounts,
  type EventMeetingBinding,
} from '../api/client';
import type { Account, CalendarEvent } from '../api/types';

/**
 * Creating and removing the meeting for an event.
 *
 * Distinct from `ConferenceSection`, which shows the meeting an event *has* —
 * from any tool, in any language. This is about the meeting Aperio *owns*: one
 * it created, recorded, and can therefore take back down. An event carrying a
 * colleague's Webex link gets a Join button and no remove button, which is
 * correct — it is not ours to delete.
 *
 * Only offered for a saved event. A meeting minted for an event that is then
 * cancelled would stand on the provider with nothing pointing at it, so the
 * button waits until there is something to attach it to.
 */
export function MeetingControls({
  event,
  onEventChanged,
}: {
  /** The saved event, or `null` while it is still being composed. */
  event: CalendarEvent | null;
  /** Called with the event as saved once a meeting is attached or removed. */
  onEventChanged: (event: CalendarEvent) => void;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [binding, setBinding] = useState<EventMeetingBinding | null>(null);
  const [accountId, setAccountId] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Which accounts can mint a meeting: the ones whose adapter has no calendars
  // of its own. Asking the account list rather than naming providers means a
  // videoconference adapter added later shows up here without a change.
  useEffect(() => {
    let cancelled = false;
    listAccounts()
      .then((all) => {
        if (cancelled) return;
        const vc = all.filter((acc) => acc.is_videoconference);
        setAccounts(vc);
        setAccountId((current) =>
          vc.some((acc) => acc.id === current) ? current : (vc[0]?.id ?? ''),
        );
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('listAccounts failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const eventId = event?.id ?? null;
  useEffect(() => {
    if (!eventId) {
      setBinding(null);
      return;
    }
    let cancelled = false;
    eventMeeting(eventId)
      .then((found) => {
        if (!cancelled) setBinding(found);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('event_meeting failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, [eventId]);

  const create = useCallback(async () => {
    if (!event || !accountId) return;
    setBusy(true);
    setError(null);
    try {
      const attached = await attachMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
        account_id: accountId,
      });
      setBinding({
        event_id: event.id,
        account_id: accountId,
        meeting_id: attached.meeting.id,
        join_url: attached.meeting.join_url,
        created_at: new Date().toISOString(),
      });
      onEventChanged(attached.event);
      announce(t('conferencing.meetingCreated'));
    } catch (err) {
      const message = isCommandError(err) ? err.message : String(err);
      setError(message);
      announce(t('conferencing.meetingFailed', { message }));
    } finally {
      setBusy(false);
    }
  }, [accountId, announce, event, onEventChanged, t]);

  const remove = useCallback(async () => {
    if (!event) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await detachMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
      });
      setBinding(null);
      if (saved) onEventChanged(saved);
      announce(t('conferencing.meetingRemoved'));
    } catch (err) {
      const message = isCommandError(err) ? err.message : String(err);
      setError(message);
      announce(t('conferencing.meetingFailed', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, event, onEventChanged, t]);

  // No videoconference account, nothing to offer. Silent rather than a
  // disabled control explaining an absence — Settings is where accounts are
  // added, and a dead button in the editor teaches nothing.
  if (accounts.length === 0) return null;

  if (!event) {
    return (
      <FocusableNote className="form__hint">
        {t('conferencing.saveEventFirst')}
      </FocusableNote>
    );
  }

  return (
    <div className="form__field">
      {binding ? (
        <>
          <FocusableNote className="form__hint">
            {t('conferencing.meetingOwned')}
          </FocusableNote>
          <button
            type="button"
            className="form__action"
            onClick={() => void remove()}
            aria-disabled={busy || undefined}
          >
            {t('conferencing.removeMeeting')}
          </button>
        </>
      ) : (
        <>
          {accounts.length > 1 && (
            <label className="form__field">
              <span className="form__label">
                {t('conferencing.meetingAccount')}
              </span>
              <select
                value={accountId}
                onChange={(e) => setAccountId(e.target.value)}
              >
                {accounts.map((acc) => (
                  <option key={acc.id} value={acc.id}>
                    {acc.display_name}
                  </option>
                ))}
              </select>
            </label>
          )}
          <button
            type="button"
            className="form__action"
            onClick={() => void create()}
            aria-disabled={busy || !accountId || undefined}
          >
            {t('conferencing.createMeeting')}
          </button>
        </>
      )}
      {error && (
        <FocusableNote className="form__error">{error}</FocusableNote>
      )}
    </div>
  );
}
