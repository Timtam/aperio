import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  calendarCurrentUserEmail,
  isCommandError,
  respondToEvent,
} from '../api/client';
import type { AttendeeStatus, CalendarEvent } from '../api/types';
import { seriesIdOf } from '../intl/recurrence';

/** The three respondable statuses, in the order we render the buttons. */
const RESPONSE_ACTIONS: AttendeeStatus[] = [
  'accepted',
  'tentative',
  'declined',
];

/** Lower-case, `mailto:`-stripped form for comparing addresses. */
function normalizeEmail(value: string | null | undefined): string {
  if (!value) return '';
  return value.trim().replace(/^mailto:/i, '').toLowerCase();
}

export interface EventRsvpProps {
  event: CalendarEvent;
  /** Called after a successful response so the host can refresh + close. */
  onResponded: () => void;
}

/**
 * RSVP affordance for an existing meeting (DESIGN.md §7.3). Shown only
 * when the event carries per-attendee response data (external,
 * scheduling-capable providers):
 *
 *  - If the connected account's user is a **non-organizer attendee**,
 *    renders Accept / Tentative / Decline buttons with the current
 *    status pressed.
 *  - If the user is the **organizer**, renders read-only per-attendee
 *    status chips.
 *
 * "Who am I" comes from `calendarCurrentUserEmail`; when it's unknown
 * (local/iCal, or a provider that can't report it) the component
 * renders nothing.
 */
export function EventRsvp({ event, onResponded }: EventRsvpProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const responses = event.attendee_responses ?? [];

  const [myEmail, setMyEmail] = useState<string | null>(null);
  const [pending, setPending] = useState<AttendeeStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (responses.length === 0) {
      setMyEmail(null);
      return;
    }
    calendarCurrentUserEmail(event.calendar_id)
      .then((email) => {
        if (!cancelled) setMyEmail(email);
      })
      .catch(() => {
        if (!cancelled) setMyEmail(null);
      });
    return () => {
      cancelled = true;
    };
  }, [event.calendar_id, responses.length]);

  if (responses.length === 0) return null;

  const me = normalizeEmail(myEmail);
  if (!me) return null;
  const isOrganizer = normalizeEmail(event.organizer) === me;
  const myResponse = responses.find((r) => normalizeEmail(r.email) === me);

  // Organizer view: read-only per-attendee status chips.
  if (isOrganizer) {
    return (
      <div className="rsvp">
        <span className="form__label">
          {t('dialogs.event.rsvp.attendeeStatusLabel')}
        </span>
        <ul className="rsvp__chips">
          {responses.map((r) => (
            <li key={r.email} className={`rsvp__chip rsvp__chip--${r.status}`}>
              <span className="rsvp__chip-name">{r.name ?? r.email}</span>
              <span className="rsvp__chip-status">
                {t(`dialogs.event.rsvp.status.${r.status}`)}
              </span>
            </li>
          ))}
        </ul>
      </div>
    );
  }

  // Only a (non-organizer) attendee can respond.
  if (!myResponse) return null;

  const respond = async (status: AttendeeStatus) => {
    setPending(status);
    setError(null);
    try {
      // Respond against the series id so a recurring-occurrence's
      // synthetic `@ISO` suffix doesn't reach the provider.
      await respondToEvent(event.calendar_id, seriesIdOf(event), status, true);
      announce(
        t('dialogs.event.rsvp.responded', {
          status: t(`dialogs.event.rsvp.status.${status}`),
        }),
      );
      onResponded();
    } catch (err) {
      setError(
        isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
      );
    } finally {
      setPending(null);
    }
  };

  return (
    <div className="rsvp">
      <span className="form__label">
        {t('dialogs.event.rsvp.yourResponseLabel')}
      </span>
      <div
        className="rsvp__buttons"
        role="group"
        aria-label={t('dialogs.event.rsvp.yourResponseLabel')}
      >
        {RESPONSE_ACTIONS.map((status) => {
          const current = myResponse.status === status;
          return (
            <button
              key={status}
              type="button"
              className={`rsvp__button rsvp__button--${status}${
                current ? ' rsvp__button--current' : ''
              }`}
              aria-pressed={current}
              disabled={pending !== null}
              onClick={() => respond(status)}
            >
              {t(`dialogs.event.rsvp.action.${status}`)}
            </button>
          );
        })}
      </div>
      {error && (
        <p className="form__error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
