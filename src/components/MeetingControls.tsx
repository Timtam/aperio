import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import { useAnnouncer } from '../a11y/announcerContext';
import {
  adoptMeeting,
  attachMeeting,
  detachMeeting,
  inspectEventMeeting,
  isCommandError,
  listAccounts,
  type EventMeetingInspection,
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
  const [found, setFound] = useState<EventMeetingInspection | null>(null);
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
  const calendarId = event?.calendar_id ?? null;
  useEffect(() => {
    if (!eventId || !calendarId) {
      setFound(null);
      return;
    }
    let cancelled = false;
    inspectEventMeeting({ event_id: eventId, calendar_id: calendarId })
      .then((result) => {
        if (!cancelled) setFound(result);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('inspect_event_meeting failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, [eventId, calendarId]);

  const create = useCallback(
    async (usePersonalRoom: boolean) => {
    if (!event || !accountId) return;
    setBusy(true);
    setError(null);
    try {
      const attached = await attachMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
        account_id: accountId,
        use_personal_room: usePersonalRoom,
      });
      setFound({
        binding: {
          event_id: event.id,
          account_id: accountId,
          meeting_id: attached.meeting.id,
          join_url: attached.meeting.join_url,
          created_at: new Date().toISOString(),
        },
        meeting: attached.meeting,
        account_id: accountId,
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
    },
    [accountId, announce, event, onEventChanged, t],
  );

  const remove = useCallback(async () => {
    if (!event) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await detachMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
      });
      setFound(null);
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

  /** Take over a meeting that is already on the event but not yet ours. */
  const adopt = useCallback(async () => {
    if (!event || !found?.meeting || !found.account_id) return;
    setBusy(true);
    setError(null);
    try {
      const binding = await adoptMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
        account_id: found.account_id,
        meeting_id: found.meeting.id,
        join_url: found.meeting.join_url,
      });
      setFound({ ...found, binding });
      announce(t('conferencing.meetingAdopted'));
    } catch (err) {
      const message = isCommandError(err) ? err.message : String(err);
      setError(message);
      announce(t('conferencing.meetingFailed', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, event, found, t]);

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
      {/* Who the PROVIDER says is invited. Kept apart from the event's own
          attendee list rather than merged into it: an event auto-created from
          an invitation mail often lists only the recipient and the provider's
          sending address, and quietly replacing one list with the other would
          misstate what the calendar entry actually holds. */}
      {found?.meeting?.invitees && found.meeting.invitees.length > 0 && (
        <>
          <FocusableNote className="form__label">
            {t('conferencing.meetingInvitees')}
          </FocusableNote>
          {found.meeting.invitees.map((invitee) => (
            <FocusableNote key={invitee.email} className="form__hint">
              {invitee.co_host
                ? t('conferencing.inviteeCoHost', {
                    name: invitee.display_name ?? invitee.email,
                    email: invitee.email,
                  })
                : t('conferencing.invitee', {
                    name: invitee.display_name ?? invitee.email,
                    email: invitee.email,
                  })}
            </FocusableNote>
          ))}
        </>
      )}

      {found?.binding ? (
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
      ) : found?.meeting ? (
        <>
          {/* The event already has a meeting. Offering "create" here would mint
              a SECOND one and write its link in alongside the first. */}
          <FocusableNote className="form__hint">
            {t('conferencing.meetingNotOwned')}
          </FocusableNote>
          <button
            type="button"
            className="form__action"
            onClick={() => void adopt()}
            aria-disabled={busy || undefined}
          >
            {t('conferencing.adoptMeeting')}
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
          {/* Two buttons rather than one button and a dialog. Which kind of
              meeting this should be is a real choice, but it is a choice
              between two named things — so naming them IS the control, and it
              costs one stop instead of four. */}
          <button
            type="button"
            className="form__action"
            onClick={() => void create(false)}
            aria-disabled={busy || !accountId || undefined}
          >
            {t('conferencing.createMeeting')}
          </button>
          <button
            type="button"
            className="form__action"
            onClick={() => void create(true)}
            aria-disabled={busy || !accountId || undefined}
          >
            {t('conferencing.usePersonalRoom')}
          </button>
          <FocusableNote className="form__hint">
            {t('conferencing.personalRoomHint')}
          </FocusableNote>
        </>
      )}
      {error && (
        <FocusableNote className="form__error">{error}</FocusableNote>
      )}
    </div>
  );
}
