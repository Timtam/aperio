import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import { DEFAULT_RECURRENCE_CAPABILITIES } from '@aperio/shared';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * A whole series saved into another calendar is written there as a NEW
 * series (decisions 224, 259): with its real deletions only. An occurrence
 * changed elsewhere — Exchange lists its slot among the exceptions and keeps
 * its row — comes back at its usual time instead of travelling as a deletion.
 * The same save in its own calendar leaves the exceptions as they are.
 */

const { invokeMock, rowsFail, rowsOnFile, onUpdate } = vi.hoisted(() => {
  const rowsFail = { current: false };
  /** How the update answers: held until a gate opens, or refused. */
  const onUpdate: { gate: Promise<void> | null; fails: unknown } = { gate: null, fails: null };
  /** The series' rows besides its master; the changed 16 November unless a test says otherwise. */
  const rowsOnFile: { current: unknown[] | null } = { current: null };
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      const event = (payload as { event: unknown }).event;
      if (onUpdate.fails != null) return Promise.reject(onUpdate.fails);
      if (onUpdate.gate) return onUpdate.gate.then(() => event);
      return Promise.resolve(event);
    }
    if (command === 'get_series_rows') {
      if (rowsFail.current) return Promise.reject({ code: 'network', message: 'offline' });
      return Promise.resolve({ rows: rowsOnFile.current ?? [CHANGED], reach: { kind: 'complete' } });
    }
    if (command === 'calendar_current_user_email' || command === 'get_user_pref') {
      return Promise.resolve(null);
    }
    return Promise.resolve([]);
  });
  return { invokeMock, rowsFail, rowsOnFile, onUpdate };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  { id: 'cal-work', name: 'Arbeit', read_only: false, account_id: 'acc-ews' } as unknown as Calendar,
  { id: 'cal-home', name: 'Privat', read_only: false, account_id: 'acc-icloud' } as unknown as Calendar,
];

/** Mondays at 09:00 UTC from 2 November, four times: the 9 November deleted,
 *  the 16 November changed in Outlook. */
const SERIES: CalendarEvent = {
  id: 'M:series|CK',
  calendar_id: 'cal-work',
  title: 'Teamrunde',
  description: null,
  location: null,
  start: '2026-11-02T09:00:00.000Z',
  end: '2026-11-02T10:00:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;BYDAY=MO;COUNT=4',
    exceptions: ['2026-11-09T09:00:00.000Z', '2026-11-16T09:00:00.000Z'],
    tzid: 'Europe/Berlin',
  },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;
/** The row Exchange keeps for the changed 16 November, moved to 11:00. */
const CHANGED = {
  ...SERIES,
  id: 'M:series|CK::rid::2026-11-16T09:00:00.000Z',
  start: '2026-11-16T11:00:00.000Z',
  end: '2026-11-16T12:00:00.000Z',
  recurrence: null,
} as unknown as CalendarEvent;

const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work', 'cal-home']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry: () => {} };
const REMINDERS = { getDefaultsFor: () => [] };
const announce = () => {};

vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => DIALOG_STATE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
vi.mock('../state/useCalendarDefaultReminders', () => ({
  useCalendarDefaultReminders: () => REMINDERS,
}));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<typeof import('../state/useTitleSuggestions')>(
    '../state/useTitleSuggestions',
  );
  return { ...actual, useTitleSuggestions: () => [] };
});

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  rowsFail.current = false;
  rowsOnFile.current = null;
  onUpdate.gate = null;
  onUpdate.fails = null;
  for (const calendar of CALENDARS) {
    delete (calendar as { recurrence_capabilities?: unknown }).recurrence_capabilities;
  }
  vi.restoreAllMocks();
});

function updatesSent(): { event: CalendarEvent; previousCalendarId: string | null }[] {
  return invokeMock.mock.calls
    .filter((call) => call[0] === 'update_event')
    .map((call) => call[1] as { event: CalendarEvent; previousCalendarId: string | null });
}

async function open() {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={SERIES} />
    </StrictMode>,
  );
  return screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
}

describe('EventDialog → a whole series saved into another calendar (224, 259)', () => {
  it('carries the real deletions only: the changed occurrence comes back', async () => {
    const picker = await open();
    fireEvent.change(picker, { target: { value: 'cal-home' } });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() => expect(updatesSent()).toHaveLength(1));
    const [{ event, previousCalendarId }] = updatesSent();
    expect(event.calendar_id).toBe('cal-home');
    expect(previousCalendarId).toBe('cal-work');
    expect(event.recurrence?.exceptions).toEqual(['2026-11-09T09:00:00.000Z']);
    expect(event.recurrence?.tzid).toBe('Europe/Berlin');
  });

  /** The start time an hour off what the form shows, on this machine's clock,
   *  within the same day: an hour later, or an hour earlier from 23:00 on, so
   *  no zone wraps it past midnight. Answers the move in milliseconds. */
  function anHourOff(): number {
    const input = screen.getByLabelText(/startzeit|start time/i) as HTMLInputElement;
    const [h, m] = input.value.split(':').map(Number);
    const by = h >= 23 ? -1 : 1;
    fireEvent.change(input, {
      target: { value: `${String(h + by).padStart(2, '0')}:${String(m).padStart(2, '0')}` },
    });
    return by * 3_600_000;
  }
  const instants = (xs: readonly string[] | undefined) => (xs ?? []).map((x) => Date.parse(x));

  it('with a new time as well, carries the real deletion along, and not the changed one', async () => {
    const picker = await open();
    fireEvent.change(picker, { target: { value: 'cal-home' } });
    const moved = anHourOff();
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() => expect(updatesSent()).toHaveLength(1));
    const [{ event }] = updatesSent();
    expect(event.calendar_id).toBe('cal-home');
    expect(instants(event.recurrence?.exceptions)).toEqual([
      Date.parse('2026-11-09T09:00:00Z') + moved,
    ]);
  });

  it('with a new time and a plan that cannot read the series as a whole, still only the real deletion', async () => {
    // A row of the series before its start (another client moved the start
    // later) makes the plan a cut: the editor falls back to moving the
    // exceptions to the new time — the real deletions only.
    const before = {
      ...CHANGED,
      id: 'M:series|CK::rid::2026-10-26T09:00:00.000Z',
      start: '2026-10-26T09:00:00.000Z',
      end: '2026-10-26T10:00:00.000Z',
    };
    rowsOnFile.current = [CHANGED, before];
    const picker = await open();
    fireEvent.change(picker, { target: { value: 'cal-home' } });
    const moved = anHourOff();
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() => expect(updatesSent()).toHaveLength(1));
    const [{ event }] = updatesSent();
    expect(instants(event.recurrence?.exceptions)).toEqual([
      Date.parse('2026-11-09T09:00:00Z') + moved,
    ]);
  });

  it('writes nothing when the series rows cannot be read', async () => {
    rowsFail.current = true;
    const picker = await open();
    fireEvent.change(picker, { target: { value: 'cal-home' } });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.some((call) => call[0] === 'get_series_rows')).toBe(true),
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(updatesSent()).toHaveLength(0);
  });

  it('leaves the exceptions as they are when the series stays in its calendar', async () => {
    await open();
    fireEvent.change(screen.getByRole('combobox', { name: /titel|title/i }), {
      target: { value: 'Teamrunde neu' },
    });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() => expect(updatesSent()).toHaveLength(1));
    expect(updatesSent()[0].event.recurrence?.exceptions).toEqual([
      '2026-11-09T09:00:00.000Z',
      '2026-11-16T09:00:00.000Z',
    ]);
  });
});

describe('EventDialog → a whole series into a calendar that creates it in steps (227, 265)', () => {
  const inSteps = (id: string) => {
    (CALENDARS.find((c) => c.id === id) as { recurrence_capabilities?: unknown }).recurrence_capabilities = {
      ...DEFAULT_RECURRENCE_CAPABILITIES,
      deletions_in_steps: true,
    };
  };
  const waiting = () => screen.queryByRole('dialog', { name: /Vorgang läuft|please wait/i });

  it('waits in its dialog while the series is created in the other calendar', async () => {
    inSteps('cal-home');
    let release = () => {};
    onUpdate.gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const picker = await open();
    fireEvent.change(picker, { target: { value: 'cal-home' } });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await screen.findByRole('dialog', { name: /Vorgang läuft|please wait/i });
    release();
    await waitFor(() => expect(waiting()).toBeNull());
  });

  it('does not wait for a save in its own calendar, even one that creates in steps', async () => {
    inSteps('cal-work');
    onUpdate.gate = new Promise<void>(() => {});
    await open();
    fireEvent.change(screen.getByRole('combobox', { name: /^titel$|^title$/i }), {
      target: { value: 'Teamrunde neu' },
    });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    await waitFor(() => expect(updatesSent()).toHaveLength(1));
    expect(waiting()).toBeNull();
  });

  it('says a series that may stand in the other calendar, focused and only closed (210)', async () => {
    inSteps('cal-home');
    onUpdate.fails = { code: 'network', message: 'created-may-stand: no-occurrence' };
    const picker = await open();
    fireEvent.change(picker, { target: { value: 'cal-home' } });
    fireEvent.click(screen.getByRole('button', { name: /^speichern$|^save$/i }));
    const said = await screen.findByText(/möglicherweise zusätzlich|may now stand there as well/);
    await waitFor(() => expect(document.activeElement).toBe(said));
    expect(screen.queryByRole('button', { name: /^speichern$|^save$/i })).toBeNull();
  });
});
