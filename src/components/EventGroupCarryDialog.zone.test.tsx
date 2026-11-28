import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import {
  carryTargetKey,
  nextZoneAnswer,
  seriesClockChange,
  zoneAppliedSeries,
  zoneMoveOf,
  zoneVerdictFor,
  type CarryableFields,
  type EventGroup,
  type ZoneCarry,
  type ZoneMove,
} from '@aperio/shared';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * A series' zone carried to its copies (stage 13; 143a to 146a): each copy is
 * asked of the core as it is now and written once — a true copy ends as the
 * series; one already right is left alone; a series that cannot take the
 * zone gets its text only and is named, outside retry; a copy that is no
 * series gets every change but the zone. The offer says how far the copies
 * move and how many do not get the zone, and where no copy can take the
 * change it can only close.
 */

const { invokeMock, rows, announced, refusals } = vi.hoisted(() => {
  /** The copies as `get_event_by_id` reads them, by id. */
  const rows: Record<string, unknown> = {};
  const announced: string[] = [];
  /** What the next `update_event` calls fail with, in order. */
  const refusals: unknown[] = [];
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'get_event_by_id') {
      return Promise.resolve(rows[(payload as { id: string }).id] ?? null);
    }
    if (command === 'update_event') {
      const refusal = refusals.shift();
      if (refusal !== undefined) return Promise.reject(refusal);
      return Promise.resolve((payload as { event: unknown }).event);
    }
    if (command === 'event_groups_for_events') return Promise.resolve([]);
    return Promise.resolve(null);
  });
  return { invokeMock, rows, announced, refusals };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const CALENDARS = [
  { id: 'work', name: 'Arbeit', read_only: false, account_id: 'acc-a' },
  { id: 'private', name: 'Privat', read_only: false, account_id: 'acc-b' },
  {
    id: 'exchange',
    name: 'Exchange',
    read_only: false,
    account_id: 'acc-c',
    recurrence_capabilities: { time_zones: ['America/New_York'] },
  },
  { id: 'device', name: 'Gerät', read_only: false, account_id: 'acc-d' },
] as unknown as Calendar[];
const STORE = { calendars: CALENDARS, colorLabels: [], selectedCalendarIds: new Set(['work']) };
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../a11y/announcerContext', () => ({
  useAnnouncer: () => (message: string) => {
    announced.push(message);
  },
}));

/** Sundays and Wednesdays 23:30 UTC, no zone: Monday and Thursday 00:30 in
 *  Berlin, where the core asks. */
const STORED = {
  id: 'ev-a',
  calendar_id: 'work',
  title: 'Abendrunde',
  description: null,
  location: null,
  start: '2026-11-22T23:30:00.000Z',
  end: '2026-11-23T00:30:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=WEEKLY;BYDAY=SU,WE',
    exceptions: ['2026-11-25T23:30:00.000Z'],
    tzid: null,
  },
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

/** The same series in another calendar, as `get_event_by_id` reads it. */
const copyIn = (calendar_id: string, id: string, extra: Partial<CalendarEvent> = {}) =>
  ({ ...STORED, id, calendar_id, ...extra }) as CalendarEvent;

const member = (calendar_id: string, event_id: string) => ({
  calendar_id,
  event_id,
  title: 'Abendrunde',
  starts_at: STORED.start,
  added_at: '2026-11-01T00:00:00Z',
});

/** The series moved to Berlin with one of the core's answers, the save the
 *  editor makes, and what it hands the dialog. */
function moved(which: 'asRule' | 'asNow', title = STORED.title) {
  const change = seriesClockChange({
    rrule: STORED.recurrence!.rrule,
    start: STORED.start,
    exceptions: STORED.recurrence!.exceptions,
    fromTzid: null,
    toTzid: 'Europe/Berlin',
    endsByDate: false,
  });
  const answer = change?.[which];
  if (answer?.outcome !== 'changed') throw new Error('the core refused the test series');
  const zoneAnswer = nextZoneAnswer(null, answer);
  const saved = { ...zoneAppliedSeries(STORED, zoneAnswer, 'Europe/Berlin'), title };
  const move = zoneMoveOf(STORED, saved, zoneAnswer) as ZoneMove;
  const fields = (ev: CalendarEvent): CarryableFields => ({
    title: ev.title,
    start: ev.start,
    end: ev.end,
    all_day: ev.all_day,
    location: ev.location,
    description: ev.description,
  });
  const before: CarryableFields = {
    ...fields(STORED),
    start: move.appliedStart,
    end: saved.end,
    zone: { tzid: null },
  };
  const after: CarryableFields = { ...fields(saved), zone: { tzid: 'Europe/Berlin' } };
  return { answer, saved, move, before, after };
}

const TODAY = '2026-11-28T12:00:00.000Z';

/** The verdicts the editor reads before it opens the dialog. */
function verdictsFor(move: ZoneMove, copies: CalendarEvent[]): Record<string, ZoneCarry> {
  return Object.fromEntries(
    copies.map((copy) => [
      carryTargetKey({ calendar_id: copy.calendar_id, event_id: copy.id }),
      zoneVerdictFor(
        move,
        copy,
        CALENDARS.find((c) => c.id === copy.calendar_id),
        TODAY,
      ),
    ]),
  );
}

async function openDialog(
  copies: CalendarEvent[],
  { move, before, after }: ReturnType<typeof moved>,
) {
  for (const copy of copies) rows[copy.id] = copy;
  const group: EventGroup = {
    id: 'g1',
    created_at: '2026-11-01T00:00:00Z',
    updated_at: '2026-11-01T00:00:00Z',
    members: [member('work', 'ev-a'), ...copies.map((c) => member(c.calendar_id, c.id))],
  };
  const { EventGroupCarryDialog } = await import('./EventGroupCarryDialog');
  render(
    <EventGroupCarryDialog
      isOpen
      onClose={() => {}}
      group={group}
      anchor={{ calendar_id: 'work', event_id: 'ev-a' }}
      before={before}
      after={after}
      zoneMove={move}
      zoneVerdicts={verdictsFor(move, copies)}
    />,
  );
}

const sent = () =>
  invokeMock.mock.calls
    .filter((call) => call[0] === 'update_event')
    .map((call) => call[1] as { event: CalendarEvent; calendarId?: string });
const carryButton = () => screen.findByRole('button', { name: /^mitziehen$/i });

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  for (const id of Object.keys(rows)) delete rows[id];
  announced.length = 0;
  refusals.length = 0;
});

describe('EventGroupCarryDialog → a series\' zone carried to its copies (stage 13)', () => {
  it('names the zone first, says the day move, and ends a true copy as the series', async () => {
    const m = moved('asRule');
    await openDialog([copyIn('private', 'ev-b')], m);
    const message = screen.getByText(/^Diese Verabredung liegt noch ein weiteres Mal vor/);
    expect(message.textContent).toContain('Die Änderung (Zeitzone: Berlin) mitziehen?');
    expect(message.textContent).toContain('Bei einer Kopie rücken die Termine dabei einen Tag vor.');
    fireEvent.click(await carryButton());
    await waitFor(() => expect(sent()).toHaveLength(1));
    const { event } = sent()[0];
    expect(event.recurrence?.rrule).toBe(m.answer.rrule);
    expect(Date.parse(event.start)).toBe(Date.parse(m.answer.start));
    expect(Date.parse(event.end)).toBe(Date.parse(m.saved.end));
    // The deletion moved with its occurrence, as the series' did.
    expect(m.answer.exceptions).toHaveLength(1);
    expect(Date.parse(m.answer.exceptions[0])).not.toBe(
      Date.parse(STORED.recurrence!.exceptions[0]),
    );
    expect(event.recurrence?.exceptions).toEqual(m.answer.exceptions);
    expect(event.recurrence?.tzid).toBe('Europe/Berlin');
    expect(event.occurrence_shift_minutes).toBe(m.answer.movedMinutes);
  });

  it('carries a changed title with the zone to a copy that takes it', async () => {
    const m = moved('asRule', 'Abendrunde neu');
    await openDialog([copyIn('private', 'ev-b')], m);
    const message = screen.getByText(/^Diese Verabredung liegt noch ein weiteres Mal vor/);
    expect(message.textContent).toContain('Die Änderung (Zeitzone: Berlin, Titel) mitziehen?');
    fireEvent.click(await carryButton());
    await waitFor(() => expect(sent()).toHaveLength(1));
    const { event } = sent()[0];
    expect(event.title).toBe('Abendrunde neu');
    expect(event.recurrence?.tzid).toBe('Europe/Berlin');
    expect(event.recurrence?.rrule).toBe(m.answer.rrule);
  });

  it('asks the loss question for a copy that takes the zone, and writes it again with the consent', async () => {
    const m = moved('asRule');
    refusals.push({ code: 'forbidden', message: 'exceptions-would-be-lost: slot:1:0' });
    await openDialog([copyIn('private', 'ev-b')], m);
    fireEvent.click(await carryButton());
    const question = await screen.findByRole('dialog', { name: 'Vorkommen gehen verloren' });
    fireEvent.click(within(question).getByRole('button', { name: /^trotzdem übertragen$/i }));
    await waitFor(() => expect(sent()).toHaveLength(2));
    const [first, second] = sent().map((call) => call.event);
    expect(first.accepts_exception_loss).toBeUndefined();
    expect(second.accepts_exception_loss).toBe(true);
    expect(second.recurrence?.tzid).toBe('Europe/Berlin');
    expect(second.occurrence_shift_minutes).toBe(m.answer.movedMinutes);
  });

  it('leaves a copy alone that already stands as the series does, and does not count it', async () => {
    const m = moved('asNow');
    // Vienna is Berlin's clock (120a); this copy already repeats where the
    // series now does.
    const vienna = copyIn('private', 'ev-v', {
      start: m.saved.start,
      end: m.saved.end,
      recurrence: { ...m.saved.recurrence!, tzid: 'Europe/Vienna' },
    } as Partial<CalendarEvent>);
    const trueCopy = copyIn('device', 'ev-b');
    await openDialog([vienna, trueCopy], m);
    const message = screen.getByText(/^Diese Verabredung liegt noch ein weiteres Mal vor/);
    expect(message.textContent).toContain('(Zeitzone: Berlin)');
    // As they fall now, the occurrences keep their instants: no day move.
    expect(message.textContent).not.toMatch(/rücken/);
    fireEvent.click(await carryButton());
    await waitFor(() => expect(sent()).toHaveLength(1));
    expect(sent()[0].event.id).toBe('ev-b');
  });

  it('names a copy that cannot take the zone, gives it the title only, and offers no retry', async () => {
    const m = moved('asRule', 'Abendrunde neu');
    await openDialog([copyIn('exchange', 'ev-x')], m);
    const message = screen.getByText(/^Diese Verabredung liegt noch/);
    expect(message.textContent).toContain('Eine Kopie bekommt die Zeitzone nicht.');
    expect(
      screen.getByText(
        'Die Kopie in Exchange bekommt die Zeitzone nicht: dieser Kalender kann die Zeitzone Berlin nicht speichern. Ihre Zeiten bleiben.',
      ),
    ).toBeTruthy();
    fireEvent.click(await carryButton());
    await waitFor(() => expect(sent()).toHaveLength(1));
    const { event } = sent()[0];
    expect(event.title).toBe('Abendrunde neu');
    expect(event.start).toBe(STORED.start);
    expect(event.recurrence?.tzid ?? null).toBeNull();
    expect(event.occurrence_shift_minutes).toBeUndefined();
    const note =
      'Die Kopie in Exchange wurde ohne die Zeitzone geändert: dieser Kalender kann die Zeitzone Berlin nicht speichern.';
    expect(await screen.findByText(note)).toBeTruthy();
    expect(announced.at(-1)).toContain(note);
    expect(screen.queryByRole('button', { name: /erneut/i })).toBeNull();
  });

  it('gives a copy that is no series the other changes, and says the zone does not apply', async () => {
    const m = moved('asNow', 'Abendrunde neu');
    const single = copyIn('device', 'ev-s', { recurrence: null } as Partial<CalendarEvent>);
    await openDialog([single], m);
    expect(
      screen.getAllByText(
        'Die Kopie in Gerät ist dort keine Serie; die Zeitzone gilt für sie nicht.',
      ),
    ).toHaveLength(1);
    fireEvent.click(await carryButton());
    await waitFor(() => expect(sent()).toHaveLength(1));
    const { event } = sent()[0];
    expect(event.title).toBe('Abendrunde neu');
    expect(event.recurrence ?? null).toBeNull();
    // The series' start is an instant of the series, not this event's.
    expect(event.start).toBe(STORED.start);
  });

  it('offers only to close where nothing but the zone changed and the copy is no series', async () => {
    const m = moved('asNow');
    const single = copyIn('device', 'ev-s', { recurrence: null } as Partial<CalendarEvent>);
    await openDialog([single], m);
    const close = screen.getByRole('button', { name: /^schließen$/i });
    await waitFor(() => expect(document.activeElement).toBe(close));
    expect(screen.queryByRole('button', { name: /^mitziehen$/i })).toBeNull();
    expect(screen.getByText(/^Keine Kopie kann die Änderung bekommen\./).textContent).toContain(
      'Die Kopie in Gerät ist dort keine Serie; die Zeitzone gilt für sie nicht.',
    );
    expect(sent()).toHaveLength(0);
  });

  it('can only close where no copy can take the change, with the reason in its message', async () => {
    const m = moved('asRule');
    await openDialog([copyIn('exchange', 'ev-x')], m);
    const close = screen.getByRole('button', { name: /^schließen$/i });
    await waitFor(() => expect(document.activeElement).toBe(close));
    expect(screen.queryByRole('button', { name: /^mitziehen$/i })).toBeNull();
    const message = screen.getByText(/^Keine Kopie kann die Änderung bekommen\./);
    expect(message.textContent).toContain(
      'Die Kopie in Exchange bekommt die Zeitzone nicht: dieser Kalender kann die Zeitzone Berlin nicht speichern.',
    );
    // The reason is said once, in the message, not again as its own note.
    expect(screen.queryAllByText(/^Die Kopie in Exchange bekommt die Zeitzone nicht/)).toHaveLength(0);
  });
});
