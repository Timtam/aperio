import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * "This and all following", edited or deleted in the editor.
 *
 * At a later occurrence the series is cut in two: the head ends just before
 * it, and a new series carries the change from there — with the repeat rule
 * the user set, if they changed it (decision 121), and with its exceptions at
 * a new time of day.
 *
 * Where nothing comes before — the first occurrence — there is no head
 * (decision 118). Cutting anyway wrote a rule that ends before it starts: the
 * views showed nothing, and the reminders fell back to the series start and
 * went on ringing. So an edit rewrites the whole series in place, keeping its
 * id, and a delete deletes it — and the sentence says why.
 */

const { invokeMock, onFile, announced, announce, openEventGroupCarry } = vi.hoisted(() => {
  const announced: string[] = [];
  const announce = (message: string) => {
    announced.push(message);
  };
  const onFile: { series: unknown; groups: unknown[] } = { series: null, groups: [] };
  const openEventGroupCarry = vi.fn();
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'update_event') {
      return Promise.resolve((payload as { event: unknown }).event);
    }
    if (command === 'create_event') {
      const request = (payload as { request: Record<string, unknown> }).request;
      return Promise.resolve({ ...request, id: 'tail-1' });
    }
    if (command === 'get_event_by_id') {
      return Promise.resolve(onFile.series);
    }
    if (command === 'get_series_rows') {
      return Promise.resolve({ rows: [], reach: { kind: 'complete' } });
    }
    if (command === 'event_groups_for_events') {
      return Promise.resolve(onFile.groups);
    }
    if (command === 'calendar_current_user_email' || command === 'get_user_pref') {
      return Promise.resolve(null);
    }
    return Promise.resolve([]);
  });
  return { invokeMock, onFile, announced, announce, openEventGroupCarry };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS: Calendar[] = [
  { id: 'cal-work', name: 'Arbeit', read_only: false, account_id: 'acc-icloud' } as unknown as Calendar,
];

/** A weekly Monday 09:00 series in Berlin summer time; one July Monday excluded. */
const SERIES: CalendarEvent = {
  id: 'ev-series',
  calendar_id: 'cal-work',
  title: 'Teamrunde',
  description: null,
  location: null,
  start: '2026-06-15T07:00:00.000Z',
  end: '2026-06-15T08:00:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;BYDAY=MO',
    exceptions: ['2026-07-13T07:00:00.000Z'],
    tzid: 'Europe/Berlin',
  },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

/** An occurrence of SERIES, as the views expand it. */
const occurrenceAt = (iso: string, end: string) =>
  ({
    ...SERIES,
    id: `ev-series@${iso}`,
    series_id: 'ev-series',
    occurrence_start: iso,
    start: iso,
    end,
  }) as unknown as CalendarEvent;
const FIRST = occurrenceAt(SERIES.start, SERIES.end);
const JULY = occurrenceAt('2026-07-06T07:00:00.000Z', '2026-07-06T08:00:00.000Z');

const STORE = {
  calendars: CALENDARS as Calendar[],
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-work']),
};
const VIEW_STATE = { showHiddenCalendarTargets: false, anchor: new Date() };
const DIALOG_STATE = { openEventGroupCarry };
const REMINDERS = { getDefaultsFor: () => [] };

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
// The rule picker, reduced to two buttons: a plain weekly rule, and a
// fortnightly one that ends after ten times.
vi.mock('./RecurrenceSelector', () => ({
  RecurrenceSelector: ({ onChange }: { onChange: (rrule: string | null) => void }) => (
    <>
      <button type="button" onClick={() => onChange('FREQ=WEEKLY')}>
        weekly
      </button>
      <button type="button" onClick={() => onChange('FREQ=WEEKLY;INTERVAL=2;COUNT=10')}>
        fortnightly ten
      </button>
    </>
  ),
}));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  onFile.series = null;
  onFile.groups = [];
  openEventGroupCarry.mockClear();
  announced.length = 0;
  vi.restoreAllMocks();
});

/** Pretend the device is in Berlin, whatever zone the test machine is in. */
function deviceInBerlin() {
  const real = new Intl.DateTimeFormat().resolvedOptions();
  vi.spyOn(Intl.DateTimeFormat.prototype, 'resolvedOptions').mockReturnValue({
    ...real,
    timeZone: 'Europe/Berlin',
  });
}

const calls = (command: string) => invokeMock.mock.calls.filter((call) => call[0] === command);

async function open(event: CalendarEvent) {
  const { EventDialog } = await import('./EventDialog');
  render(
    <StrictMode>
      <EventDialog isOpen onClose={() => {}} event={event} initialScope="this_and_future" />
    </StrictMode>,
  );
  await screen.findByRole('combobox', { name: /kalender/i }, { timeout: 8000 });
}

const setStartTime = (value: string) =>
  fireEvent.change(screen.getByLabelText(/startzeit|start time/i), { target: { value } });

const save = () => fireEvent.click(screen.getByRole('button', { name: /speichern|save/i }));

const WHOLE = /Davor war kein Termin mehr übrig|No earlier occurrence was left/;

describe('EventDialog → "this and all following" at the first occurrence', () => {
  it('rewrites the whole series in place, keeping its id', async () => {
    deviceInBerlin();
    onFile.series = SERIES;
    await open(FIRST);
    fireEvent.change(screen.getByRole('combobox', { name: /^titel$|^title$/i }), {
      target: { value: 'Teamrunde neu' },
    });
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));

    expect(calls('create_event')).toHaveLength(0);
    const sent = (calls('update_event')[0][1] as {
      event: CalendarEvent & { truncate_tail_overrides?: boolean };
    }).event;
    expect(sent.id).toBe('ev-series');
    expect(sent.title).toBe('Teamrunde neu');
    expect(sent.start).toBe(SERIES.start);
    expect(sent.recurrence).toEqual(SERIES.recurrence);
    expect(sent.truncate_tail_overrides).toBeUndefined();
    await waitFor(() => expect(announced.some((m) => WHOLE.test(m))).toBe(true));
  });

  it('takes a new time for the whole series, and its exceptions along', async () => {
    deviceInBerlin();
    onFile.series = SERIES;
    await open(FIRST);
    setStartTime('10:30');
    save();
    await waitFor(() => expect(calls('update_event')).toHaveLength(1));

    // The time field is read on this machine's clock: the series' own day at
    // 10:30 there, and the excluded Monday moved by as much.
    const sent = (calls('update_event')[0][1] as { event: CalendarEvent }).event;
    expect(sent.start).toBe(new Date(2026, 5, 15, 10, 30).toISOString());
    const moved = Date.parse(sent.start) - Date.parse(SERIES.start);
    expect(sent.recurrence?.exceptions).toEqual([
      new Date(Date.parse('2026-07-13T07:00:00.000Z') + moved).toISOString(),
    ]);
  });

  it('offers the copies the change from the cut, not a move nobody made', async () => {
    deviceInBerlin();
    onFile.series = SERIES;
    onFile.groups = [
      {
        id: 'g1',
        created_at: '2026-06-01T00:00:00Z',
        updated_at: '2026-06-01T00:00:00Z',
        members: [
          { calendar_id: 'cal-work', event_id: 'ev-series', title: 'Teamrunde', starts_at: SERIES.start, added_at: '2026-06-01T00:00:00Z' },
          { calendar_id: 'cal-work', event_id: 'ev-copy', title: 'Teamrunde', starts_at: SERIES.start, added_at: '2026-06-01T00:00:01Z' },
        ],
      },
    ];
    await open(FIRST);
    fireEvent.change(screen.getByRole('combobox', { name: /^titel$|^title$/i }), {
      target: { value: 'Teamrunde neu' },
    });
    save();
    await waitFor(() => expect(openEventGroupCarry).toHaveBeenCalledTimes(1));

    const offer = openEventGroupCarry.mock.calls[0][0] as {
      scope: string;
      occurrence: string;
      before: { start: string; title: string };
      after: { start: string; title: string };
      successor: { event_id: string } | null;
    };
    expect(offer.scope).toBe('future');
    expect(offer.occurrence).toBe(SERIES.start);
    expect(offer.before.start).toBe(SERIES.start);
    expect(offer.after.start).toBe(SERIES.start);
    expect(offer.after.title).toBe('Teamrunde neu');
    expect(offer.successor?.event_id).toBe('ev-series');
  });

  it('deletes the whole series, and says why', async () => {
    deviceInBerlin();
    onFile.series = SERIES;
    await open(FIRST);
    fireEvent.click(screen.getByRole('button', { name: /^(löschen|delete)$/i }));
    await waitFor(() => expect(calls('delete_event')).toHaveLength(1));

    expect(calls('update_event')).toHaveLength(0);
    expect((calls('delete_event')[0][1] as { id: string }).id).toBe('ev-series');
    await waitFor(() => expect(announced.some((m) => WHOLE.test(m))).toBe(true));
  });
});

describe('EventDialog → "this and all following" at a later occurrence', () => {
  it('gives the new series the rule the user set (121)', async () => {
    deviceInBerlin();
    onFile.series = SERIES;
    await open(JULY);
    fireEvent.click(screen.getByRole('button', { name: 'weekly' }));
    save();
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));

    const truncated = (calls('update_event')[0][1] as { event: CalendarEvent }).event;
    expect(truncated.recurrence?.rrule).toContain('UNTIL=');
    const tail = (calls('create_event')[0][1] as { request: CalendarEvent }).request;
    expect(tail.recurrence?.rrule).toBe('FREQ=WEEKLY');
    expect(tail.recurrence?.exceptions).toEqual(['2026-07-13T07:00:00.000Z']);
  });

  it('keeps the series pattern when the rule is untouched', async () => {
    deviceInBerlin();
    onFile.series = SERIES;
    await open(JULY);
    save();
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));

    const tail = (calls('create_event')[0][1] as { request: CalendarEvent }).request;
    expect(tail.recurrence?.rrule).toBe('FREQ=WEEKLY;BYDAY=MO');
  });

  /** SERIES, ending after ten times; its July occurrence is the fourth. */
  const COUNTED = {
    ...SERIES,
    recurrence: { ...SERIES.recurrence!, rrule: 'FREQ=WEEKLY;BYDAY=MO;COUNT=10' },
  } as CalendarEvent;
  const COUNTED_JULY = { ...JULY, recurrence: COUNTED.recurrence } as CalendarEvent;

  it('continues with what is left of a COUNT when the rule is untouched', async () => {
    deviceInBerlin();
    onFile.series = COUNTED;
    await open(COUNTED_JULY);
    save();
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));

    const tail = (calls('create_event')[0][1] as { request: CalendarEvent }).request;
    expect(tail.recurrence?.rrule).toBe('FREQ=WEEKLY;BYDAY=MO;COUNT=7');
  });

  it('reads a COUNT the user set as the series total, as the field showed it', async () => {
    // "Ends after 10 times" on the fourth occurrence: seven from here, not ten
    // more.
    deviceInBerlin();
    onFile.series = COUNTED;
    await open(COUNTED_JULY);
    fireEvent.click(screen.getByRole('button', { name: 'fortnightly ten' }));
    save();
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));

    const tail = (calls('create_event')[0][1] as { request: CalendarEvent }).request;
    expect(tail.recurrence?.rrule).toBe('FREQ=WEEKLY;INTERVAL=2;COUNT=7');
  });

  it('says a series that no longer repeats cannot be split, and writes nothing', async () => {
    // Changed on another device since the row was drawn: "could not be loaded"
    // would send the user to retry what a retry cannot fix.
    deviceInBerlin();
    onFile.series = { ...SERIES, recurrence: null };
    await open(JULY);
    save();
    await screen.findByText(/wiederholt sich nicht mehr|no longer repeats/i);

    expect(calls('update_event')).toHaveLength(0);
    expect(calls('create_event')).toHaveLength(0);
  });

  it("moves the new series' exceptions to its new time", async () => {
    // Left at 09:00, the excluded Monday came back at 10:30.
    deviceInBerlin();
    onFile.series = SERIES;
    await open(JULY);
    setStartTime('10:30');
    save();
    await waitFor(() => expect(calls('create_event')).toHaveLength(1));

    // Read on this machine's clock, as the field is.
    const tail = (calls('create_event')[0][1] as { request: CalendarEvent }).request;
    expect(tail.start).toBe(new Date(2026, 6, 6, 10, 30).toISOString());
    const moved = Date.parse(tail.start) - Date.parse(JULY.start);
    expect(moved).not.toBe(0);
    expect(tail.recurrence?.exceptions).toEqual([
      new Date(Date.parse('2026-07-13T07:00:00.000Z') + moved).toISOString(),
    ]);
  });
});
