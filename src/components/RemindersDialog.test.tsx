import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { RemindersDialog } from './RemindersDialog';

/**
 * Enter on a row of the reminders overview (Ctrl+Shift+R) opens what a click
 * on its notification opens (TODO A5): THAT occurrence of a series, an
 * external appointment through its calendar, and never the create editor in
 * place of one that cannot be found.
 */

const ROW = {
  item_id: 'series-1',
  item_kind: 'event',
  container_id: 'dav-cal',
  title: 'Standup',
  trigger_at: '2026-11-16T08:45:00+00:00',
  start: '2026-11-16T09:00:00+00:00',
};

const SERIES = {
  id: 'series-1',
  calendar_id: 'dav-cal',
  title: 'Standup',
  start: '2026-11-02T09:00:00.000Z',
  end: '2026-11-02T09:15:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [] },
};

const h = vi.hoisted(() => {
  const backend = { rows: [] as unknown[], event: null as unknown };
  const invokeMock = vi.fn((cmd: string, _args?: unknown) => {
    switch (cmd) {
      case 'list_upcoming_reminders':
        return Promise.resolve(backend.rows);
      case 'get_event_by_id':
        return Promise.resolve(backend.event);
      case 'get_task_by_id':
        return Promise.resolve(null);
      default:
        return Promise.resolve([]);
    }
  });
  const dialog = {
    openTappedEvent: vi.fn(),
    openTaskDialog: vi.fn(),
    openEventDialog: vi.fn(),
  };
  const store = { calendars: [{ id: 'dav-cal' }] };
  const announce = vi.fn();
  return { backend, invokeMock, dialog, store, announce };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: h.invokeMock }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => h.dialog }));
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => h.store }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => h.announce }));

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 20)));

async function enterOnFirstRow() {
  render(<RemindersDialog isOpen onClose={() => {}} />);
  await settle();
  fireEvent.keyDown(screen.getByRole('listbox'), { key: 'Enter' });
  await settle();
}

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  h.backend.rows = [ROW];
  h.backend.event = SERIES;
  h.invokeMock.mockClear();
  h.dialog.openTappedEvent.mockClear();
  h.dialog.openTaskDialog.mockClear();
  h.dialog.openEventDialog.mockClear();
  h.announce.mockClear();
});

describe('the reminders overview opens a row', () => {
  it('as THAT occurrence, read through its calendar', async () => {
    await enterOnFirstRow();
    const read = h.invokeMock.mock.calls.find((c) => c[0] === 'get_event_by_id');
    expect(read?.[1]).toEqual({ id: 'series-1', calendarId: 'dav-cal' });
    expect(h.dialog.openTappedEvent).toHaveBeenCalledTimes(1);
    const [event, options] = h.dialog.openTappedEvent.mock.calls[0];
    expect(event.id).toBe('series-1@2026-11-16T09:00:00.000Z');
    expect(event.start).toBe('2026-11-16T09:00:00.000Z');
    expect(options).toEqual({ initialScope: undefined, shownOccurrence: undefined });
  });

  it('says it was not found, and opens no create editor', async () => {
    h.backend.event = null;
    await enterOnFirstRow();
    expect(h.dialog.openTappedEvent).not.toHaveBeenCalled();
    expect(h.dialog.openEventDialog).not.toHaveBeenCalled();
    expect(h.announce).toHaveBeenCalledTimes(1);
  });

  it('a task that cannot be found opens no create editor either', async () => {
    h.backend.rows = [{ ...ROW, item_id: 't-1', item_kind: 'task', container_id: 'vikunja-3' }];
    await enterOnFirstRow();
    expect(h.dialog.openTaskDialog).not.toHaveBeenCalled();
    expect(h.announce).toHaveBeenCalledTimes(1);
    expect(h.invokeMock.mock.calls.some((c) => c[0] === 'get_tasks')).toBe(true);
  });
});
