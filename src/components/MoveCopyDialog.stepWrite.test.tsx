import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Copying or moving a series into a calendar that creates a new series in
 * steps (decisions 202-213, 227, 265): the "please wait" dialog while it runs,
 * the outcome in words, and a new series that may stand said on screen with
 * only Close (210). A whole-series move writes the series in place with its
 * new calendar (`update_event`); the sidebar drop shares that path.
 */

const { invokeMock, onFile, invalidateData, announced } = vi.hoisted(() => {
  const onFile: {
    createFails: unknown;
    createGate: Promise<void> | null;
    updateFails: unknown;
    updateGate: Promise<void> | null;
  } = {
    createFails: null,
    createGate: null,
    updateFails: null,
    updateGate: null,
  };
  const announced: string[] = [];
  const invalidateData = vi.fn();
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'create_event') {
      const request = (payload as { request: Record<string, unknown> }).request;
      if (onFile.createFails != null) return Promise.reject(onFile.createFails);
      if (onFile.createGate) return onFile.createGate.then(() => ({ ...request, id: 'copy-1' }));
      return Promise.resolve({ ...request, id: 'copy-1' });
    }
    if (command === 'update_event') {
      const event = (payload as { event: Record<string, unknown> }).event;
      if (onFile.updateFails != null) return Promise.reject(onFile.updateFails);
      if (onFile.updateGate) return onFile.updateGate.then(() => event);
      return Promise.resolve(event);
    }
    if (command === 'get_series_rows') {
      return Promise.resolve({ rows: [], reach: { kind: 'complete' } });
    }
    return Promise.resolve([]);
  });
  return { invokeMock, onFile, invalidateData, announced };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS = [
  { id: 'cal-home', name: 'Privat', read_only: false, account_id: 'acc-icloud' },
  {
    id: 'cal-exchange',
    name: 'Arbeit',
    read_only: false,
    account_id: 'acc-ews',
    recurrence_capabilities: { deletions_in_steps: true },
  },
] as unknown as Calendar[];
const STORE = {
  calendars: CALENDARS,
  taskLists: [],
  selectedCalendarIds: new Set(['cal-home', 'cal-exchange']),
  selectedTaskListIds: new Set<string>(),
  sectionsByList: {},
  loadSections: () => Promise.resolve(),
};
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({
  useViewState: () => ({ showHiddenCalendarTargets: false, showHiddenTaskListTargets: false }),
}));
vi.mock('../state/useTasks', () => ({ useTasks: () => ({ tasks: [] }) }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => ({ invalidateData }) }));
vi.mock('../a11y/announcerContext', () => ({
  useAnnouncer: () => (message: string) => {
    announced.push(message);
  },
}));

/** A weekly series with one deleted Monday, the series' own row. */
const SERIES = {
  id: 'ev-series',
  calendar_id: 'cal-home',
  title: 'Teamrunde',
  description: null,
  location: null,
  start: '2026-11-02T09:00:00.000Z',
  end: '2026-11-02T10:00:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;BYDAY=MO',
    exceptions: ['2026-11-16T09:00:00.000Z'],
    tzid: 'Europe/Berlin',
  },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  invalidateData.mockClear();
  onFile.createFails = null;
  onFile.createGate = null;
  onFile.updateFails = null;
  onFile.updateGate = null;
  announced.length = 0;
});

async function copyTo(
  calendarName: RegExp,
  onClose: () => void = () => {},
  defaultMode: 'copy' | 'move' = 'copy',
) {
  const { MoveCopyDialog } = await import('./MoveCopyDialog');
  render(
    <div id="app-root">
      <MoveCopyDialog
        isOpen
        onClose={onClose}
        target={{ kind: 'event', event: SERIES, defaultMode }}
      />
    </div>,
  );
  const picker = await screen.findByRole('combobox');
  const option = screen.getByRole('option', { name: calendarName }) as HTMLOptionElement;
  fireEvent.change(picker, { target: { value: option.value } });
}

const submit = () => fireEvent.click(screen.getByRole('button', { name: /^kopieren$|^copy$/i }));
const submitMove = () =>
  fireEvent.click(screen.getByRole('button', { name: /^verschieben$|^move$/i }));
const updates = () => invokeMock.mock.calls.filter((c) => c[0] === 'update_event');
const waiting = () => screen.queryByRole('dialog', { name: /Vorgang läuft|please wait/i });

describe('MoveCopyDialog → a series into a calendar that creates it in steps', () => {
  it('waits in its dialog while the copy is written, then says it was copied', async () => {
    let release = () => {};
    onFile.createGate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await copyTo(/Arbeit/);
    submit();
    await screen.findByRole('dialog', { name: /Vorgang läuft|please wait/i });
    // One write at a time.
    submit();
    release();
    await waitFor(() => expect(waiting()).toBeNull());
    await waitFor(() => expect(announced.at(-1)).toMatch(/kopiert|copied/i));
    expect(invokeMock.mock.calls.filter((c) => c[0] === 'create_event')).toHaveLength(1);
  });

  it('does not wait where the calendar takes the deletions with the series', async () => {
    onFile.createGate = new Promise<void>(() => {});
    await copyTo(/Privat/);
    submit();
    await waitFor(() =>
      expect(invokeMock.mock.calls.filter((c) => c[0] === 'create_event')).toHaveLength(1),
    );
    expect(waiting()).toBeNull();
  });

  it('says a copy that may stand on screen, focused and only closed, and reads again (210)', async () => {
    onFile.createFails = { code: 'network', message: 'created-may-stand: no-occurrence' };
    await copyTo(/Arbeit/);
    submit();
    const said = await screen.findByText(/möglicherweise zusätzlich|may now stand there as well/);
    await waitFor(() => expect(document.activeElement).toBe(said));
    expect(screen.queryByRole('button', { name: /^kopieren$|^copy$/i })).toBeNull();
    expect(screen.getByRole('button', { name: /^schließen$/i })).toBeTruthy();
    expect(invalidateData).toHaveBeenCalled();
  });

  it('says a copy taken back again in words, with Copy still there', async () => {
    onFile.createFails = { code: 'forbidden', message: 'deletions-not-kept: no-occurrence' };
    await copyTo(/Arbeit/);
    submit();
    const said = await screen.findByRole('alert');
    expect(said.textContent).toMatch(
      /nicht mit ihren gelöschten Terminen übernommen|together with its deleted occurrences/,
    );
    expect(said.textContent).not.toMatch(/forbidden|deletions-not-kept/);
    expect(screen.getByRole('button', { name: /^kopieren$|^copy$/i })).toBeTruthy();
  });

  it('does not close while it writes: Escape and Cancel wait for the outcome', async () => {
    onFile.createGate = new Promise<void>(() => {});
    const onClose = vi.fn();
    await copyTo(/Privat/, onClose);
    submit();
    await waitFor(() =>
      expect(invokeMock.mock.calls.filter((c) => c[0] === 'create_event')).toHaveLength(1),
    );
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });
    fireEvent.click(screen.getByRole('button', { name: /^abbrechen$|^cancel$/i }));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('waits while the whole series is moved, writes it once, then says it moved', async () => {
    let release = () => {};
    onFile.updateGate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await copyTo(/Arbeit/, () => {}, 'move');
    submitMove();
    await screen.findByRole('dialog', { name: /Vorgang läuft|please wait/i });
    // The series is moved in place, into its new calendar.
    expect(updates()).toHaveLength(1);
    expect((updates()[0][1] as { event: { calendar_id: string } }).event.calendar_id).toBe(
      'cal-exchange',
    );
    // One write at a time.
    submitMove();
    release();
    await waitFor(() => expect(waiting()).toBeNull());
    await waitFor(() => expect(announced.at(-1)).toMatch(/verschoben|moved/i));
    expect(updates()).toHaveLength(1);
  });

  it('says a moved series that may stand on screen, focused and only closed (210)', async () => {
    onFile.updateFails = { code: 'network', message: 'created-may-stand: no-occurrence' };
    await copyTo(/Arbeit/, () => {}, 'move');
    submitMove();
    const said = await screen.findByText(/möglicherweise zusätzlich|may now stand there as well/);
    await waitFor(() => expect(document.activeElement).toBe(said));
    expect(screen.queryByRole('button', { name: /^verschieben$|^move$/i })).toBeNull();
    expect(screen.getByRole('button', { name: /^schließen$/i })).toBeTruthy();
    expect(invalidateData).toHaveBeenCalled();
  });
});
