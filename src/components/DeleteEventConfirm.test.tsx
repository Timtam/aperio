import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';
import { DeleteEventConfirm } from './DeleteEventConfirm';

/**
 * The views' Delete key asks what the editor, the chip menu and the phone ask
 * (decisions 70a, 80a): nothing about attendees for a plain event, "notify or
 * not" where the provider can delete silently, and where it cannot, the
 * sentence that says who informs the attendees.
 */

const CALENDARS: Calendar[] = [
  { id: 'cal-plain', name: 'Privat', read_only: false } as unknown as Calendar,
  {
    id: 'cal-exchange',
    name: 'Exchange',
    read_only: false,
    supports_scheduling: true,
  } as unknown as Calendar,
  {
    id: 'cal-icloud',
    name: 'iCloud',
    read_only: false,
    supports_scheduling: true,
    always_notifies_attendees: true,
    notifier_name: 'iCloud',
  } as unknown as Calendar,
];
const STORE = { calendars: CALENDARS };
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));

const meeting = (calendarId: string, organizedElsewhere = false) =>
  ({
    id: 'ev-1',
    calendar_id: calendarId,
    title: 'Planung',
    start: '2026-06-15T07:00:00.000Z',
    end: '2026-06-15T08:00:00.000Z',
    all_day: false,
    recurrence: null,
    attendees: ['bob@example.com'],
    organizer: 'me@example.com',
    organized_elsewhere: organizedElsewhere,
  }) as unknown as CalendarEvent;

afterEach(() => {
  document.body.innerHTML = '';
});

function open(event: CalendarEvent) {
  const onDelete = vi.fn();
  render(<DeleteEventConfirm event={event} onClose={() => {}} onDelete={onDelete} />);
  return onDelete;
}

const cancelButton = /absagen & teilnehmer benachrichtigen|cancel & notify attendees/i;
const silentButton = /ohne benachrichtigung entfernen|remove without notifying/i;

describe('DeleteEventConfirm', () => {
  it('confirms a plain delete of an event without guests to tell', () => {
    const onDelete = open({ ...meeting('cal-plain'), attendees: [] } as CalendarEvent);
    expect(screen.getByText(/Planung/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: cancelButton })).toBeNull();
    expect(screen.queryByRole('button', { name: silentButton })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /^(löschen|delete|bestätigen|confirm)$/i }));
    expect(onDelete).toHaveBeenCalledWith(expect.objectContaining({ id: 'ev-1' }), false);
  });

  it("asks nothing about someone else's meeting (70a)", () => {
    const onDelete = open(meeting('cal-icloud', true));
    expect(screen.queryByText(/iCloud informiert|iCloud informs/i)).toBeNull();
    expect(screen.queryByRole('button', { name: cancelButton })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /^(löschen|delete|bestätigen|confirm)$/i }));
    expect(onDelete).toHaveBeenCalledWith(expect.anything(), false);
  });

  it('offers to notify or not where the provider can delete silently', () => {
    const onDelete = open(meeting('cal-exchange'));
    fireEvent.click(screen.getByRole('button', { name: silentButton }));
    expect(onDelete).toHaveBeenLastCalledWith(expect.anything(), false);
    fireEvent.click(screen.getByRole('button', { name: cancelButton }));
    expect(onDelete).toHaveBeenLastCalledWith(expect.anything(), true);
  });

  it('says who informs the attendees where the provider always does, and cancels', () => {
    const onDelete = open(meeting('cal-icloud'));
    const sentence = /iCloud informiert die Teilnehmer über die Absage|iCloud informs the attendees of the cancellation/i;
    expect(screen.getByText(sentence)).toBeInTheDocument();
    // The message describes the buttons, so the sentence is heard on open.
    const describedBy = screen.getByRole('button', { name: cancelButton }).getAttribute('aria-describedby');
    expect(document.getElementById(describedBy!.split(' ')[0])?.textContent).toMatch(sentence);
    expect(screen.queryByRole('button', { name: silentButton })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: cancelButton }));
    expect(onDelete).toHaveBeenCalledWith(expect.anything(), true);
  });
});
