import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';
import { DeleteEventScopeDialog } from './DeleteEventScopeDialog';

/**
 * Deleting one occurrence of a series (decisions 80a, 83b).
 *
 * On someone else's meeting the dialog says that the organizer gets a
 * decline, and it does not offer to end their series early — the provider
 * refuses that, and a button that can only fail is not a choice.
 */

const CALENDARS: Calendar[] = [
  {
    id: 'cal-icloud',
    name: 'iCloud',
    read_only: false,
    supports_scheduling: true,
    always_notifies_attendees: true,
    invitations_reply_only: true,
    notifier_name: 'iCloud',
  } as unknown as Calendar,
  {
    id: 'cal-exchange',
    name: 'Exchange',
    read_only: false,
    supports_scheduling: true,
  } as unknown as Calendar,
];
const STORE = { calendars: CALENDARS };
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));

const occurrence = (calendarId: string, organizedElsewhere: boolean) =>
  ({
    id: 'ev-1@2026-11-16T15:00:00.000Z',
    series_id: 'ev-1',
    calendar_id: calendarId,
    title: 'Planung',
    attendees: ['bob@example.net'],
    organizer: organizedElsewhere ? 'boss@example.net' : 'me@example.com',
    organized_elsewhere: organizedElsewhere,
  }) as unknown as CalendarEvent;

afterEach(() => {
  document.body.innerHTML = '';
});

function open(event: CalendarEvent) {
  render(
    <DeleteEventScopeDialog
      isOpen
      onClose={() => {}}
      title={event.title}
      event={event}
      onOccurrence={() => {}}
      onThisAndFuture={() => {}}
      onSeries={() => {}}
    />,
  );
}

const thisAndFuture = () =>
  screen.queryByRole('button', {
    name: /diesen und alle folgenden|this and all following/i,
  });

describe('DeleteEventScopeDialog', () => {
  it('says the organizer gets a decline, and offers no truncation (77a, 83b)', () => {
    open(occurrence('cal-icloud', true));
    expect(
      screen.getByText(/Organisator bekommt eine Absage|organizer gets a decline/i),
    ).toBeInTheDocument();
    expect(thisAndFuture()).toBeNull();
  });

  it('keeps the full choice for a meeting the account organizes', () => {
    open(occurrence('cal-exchange', false));
    expect(
      screen.queryByText(/Organisator bekommt eine Absage|organizer gets a decline/i),
    ).toBeNull();
    expect(thisAndFuture()).not.toBeNull();
  });
});
