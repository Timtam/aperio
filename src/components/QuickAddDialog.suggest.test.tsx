import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Accepting an offer in the quick-add hands the editor the earlier
 * appointment AND says whether its calendar may travel. `targetPinned` means
 * "the user picked a calendar HERE, don't overrule it" — so an untouched
 * default must never set it.
 */

const invokeMock = vi.hoisted(() => vi.fn(() => Promise.resolve([])));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

const CALENDARS: Calendar[] = [
  { id: 'cal-local', name: 'Kalender', read_only: false } as unknown as Calendar,
  { id: 'cal-work', name: 'Arbeit', read_only: false } as unknown as Calendar,
];

const SOURCE: CalendarEvent = {
  id: 'ev-thomas',
  calendar_id: 'cal-work',
  title: 'Thomas Meeting',
  description: null,
  location: null,
  start: '2026-06-15T09:00:00.000Z',
  end: '2026-06-15T10:00:00.000Z',
  all_day: false,
  recurrence: null,
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

// Mutable so a test can open the dialog on a COLD store and fill it after —
// which is what a real launch does, the calendar list arriving a beat late.
const STORE = {
  calendars: CALENDARS as Calendar[],
  selectedCalendarIds: new Set(['cal-local', 'cal-work']),
};
const EMPTY_SELECTION = new Set<string>();
const VIEW_STATE = {
  anchor: new Date('2026-06-20T08:00:00'),
  showHiddenCalendarTargets: false,
};

const openEventDialog = vi.hoisted(() => vi.fn());

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => STORE,
}));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ openEventDialog }),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => [SOURCE] };
});

afterEach(() => {
  document.body.innerHTML = '';
  openEventDialog.mockClear();
  STORE.calendars = CALENDARS;
  STORE.selectedCalendarIds = new Set(['cal-local', 'cal-work']);
});

async function acceptTheOffer() {
  const { QuickAddDialog } = await import('./QuickAddDialog');
  render(<QuickAddDialog isOpen onClose={() => {}} />);
  const title = screen.getByRole('combobox', { name: /titel/i });
  fireEvent.change(title, { target: { value: 'thomas' } });
  fireEvent.keyDown(title, { key: 'ArrowDown' });
  fireEvent.keyDown(title, { key: 'Enter' });
  await waitFor(() => expect(openEventDialog).toHaveBeenCalled());
  return openEventDialog.mock.calls[0][1] as {
    prefillFrom?: CalendarEvent;
    targetPinned?: boolean;
  };
}

describe('QuickAddDialog → accepting a title offer', () => {
  it('hands the earlier appointment over', async () => {
    const opts = await acceptTheOffer();
    expect(opts.prefillFrom?.id).toBe('ev-thomas');
  });

  it('does NOT pin the calendar when the user never picked one', async () => {
    // The whole bug: an untouched default picker counted as a choice, so the
    // editor kept the local calendar and silently disagreed with the hint the
    // offer had just shown ("Arbeit").
    const opts = await acceptTheOffer();
    expect(opts.targetPinned).toBe(false);
  });

  it('does not pin when the DEFAULT moves under an untouched picker', async () => {
    // The release-build regression. Nothing about the picker changes; the
    // default it would be compared against does. Here the sidebar selection
    // drops the first calendar while the dialog is open, so `initial`
    // re-derives to another one and slides under the untouched picker. Judging
    // "did the user pick?" by comparing the two then answers yes, and the
    // offer's calendar stops travelling into the editor.
    const { QuickAddDialog } = await import('./QuickAddDialog');
    const { rerender } = render(<QuickAddDialog isOpen onClose={() => {}} />);
    const select = screen.getByRole('combobox', {
      name: /kalender/i,
    }) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe('cal-local'));

    STORE.selectedCalendarIds = new Set(['cal-work']);
    rerender(<QuickAddDialog isOpen onClose={() => {}} />);
    await waitFor(() => expect(select.value).toBe('cal-work'));

    const title = screen.getByRole('combobox', { name: /titel/i });
    fireEvent.change(title, { target: { value: 'thomas' } });
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    fireEvent.keyDown(title, { key: 'Enter' });
    await waitFor(() => expect(openEventDialog).toHaveBeenCalled());
    const opts = openEventDialog.mock.calls[0][1] as { targetPinned?: boolean };
    expect(opts.targetPinned).toBe(false);
  });

  it("keeps the user's pick even after the default catches up with it", async () => {
    // The other half, and the one that loses data: the user DID pick a
    // calendar, and afterwards the default moves onto the same one — the
    // sidebar selection changes, a refresh reorders the catalog. Comparing
    // picker against default now says "unchanged", so the offer's calendar
    // overrules a choice the user actually made.
    const { QuickAddDialog } = await import('./QuickAddDialog');
    const { rerender } = render(<QuickAddDialog isOpen onClose={() => {}} />);
    const select = screen.getByRole('combobox', { name: /kalender/i });
    fireEvent.change(select, { target: { value: 'cal-work' } });

    STORE.selectedCalendarIds = new Set(['cal-work']);
    rerender(<QuickAddDialog isOpen onClose={() => {}} />);

    const title = screen.getByRole('combobox', { name: /titel/i });
    fireEvent.change(title, { target: { value: 'thomas' } });
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    fireEvent.keyDown(title, { key: 'Enter' });
    await waitFor(() => expect(openEventDialog).toHaveBeenCalled());
    const opts = openEventDialog.mock.calls[0][1] as { targetPinned?: boolean };
    expect(opts.targetPinned).toBe(true);
  });

  it('does not pin either when the calendars arrive a beat late', async () => {
    // The real launch order: the dialog opens before the calendar list has
    // resolved, so its picker starts empty and adopts the default afterwards.
    // That adoption is not a choice either.
    STORE.calendars = [];
    STORE.selectedCalendarIds = EMPTY_SELECTION;
    const { QuickAddDialog } = await import('./QuickAddDialog');
    const { rerender } = render(<QuickAddDialog isOpen onClose={() => {}} />);

    STORE.calendars = CALENDARS;
    STORE.selectedCalendarIds = new Set(['cal-local', 'cal-work']);
    rerender(<QuickAddDialog isOpen onClose={() => {}} />);

    const title = screen.getByRole('combobox', { name: /titel/i });
    fireEvent.change(title, { target: { value: 'thomas' } });
    fireEvent.keyDown(title, { key: 'ArrowDown' });
    fireEvent.keyDown(title, { key: 'Enter' });
    await waitFor(() => expect(openEventDialog).toHaveBeenCalled());
    const opts = openEventDialog.mock.calls[0][1] as { targetPinned?: boolean };
    expect(opts.targetPinned).toBe(false);
  });
});
