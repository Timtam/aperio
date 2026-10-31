import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { CalendarEvent } from '../api/types';

const { invokeMock, phone } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  phone: {
    addEventExdate: vi.fn(),
    createEvent: vi.fn(),
    getEventById: vi.fn(),
    getSeriesRows: vi.fn(),
    updateEvent: vi.fn(),
  },
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('../../mobile/src/api/calendar', () => phone);
vi.mock('../../mobile/src/api/client', () => ({ createTask: vi.fn(), updateTask: vi.fn() }));

import {
  moveEventToDay,
  moveOrCopyEvent as desktopMoveOrCopy,
  OriginalKeptError,
} from './moveActions';

/**
 * The phone's module, loaded through a path the type checker does not follow,
 * as `taskSettings.contractSupport.ts` does: a static import pulls the mobile
 * sources — and through them the Expo bridge — into the desktop's `tsc`, which
 * runs without the mobile packages installed. The test runner resolves it next
 * to this file at run time.
 */
const PHONE_MOVE_ACTIONS = '../../mobile/src/state/moveActions';
type MoveOrCopy = (
  event: CalendarEvent,
  targetCalendarId: string,
  mode: 'move' | 'copy',
  scope: 'occurrence' | 'series',
) => Promise<CalendarEvent | null>;
async function phoneMoveOrCopy(...args: Parameters<MoveOrCopy>): Promise<CalendarEvent | null> {
  const phoneModule = (await import(/* @vite-ignore */ PHONE_MOVE_ACTIONS)) as {
    moveOrCopyEvent: MoveOrCopy;
  };
  return phoneModule.moveOrCopyEvent(...args);
}

/**
 * Moving and copying a whole series, on both surfaces (decisions 224, 259,
 * 262): the same series written as a new one, with its real deletions only and
 * the source's zone. The phone has no test runner of its own, so this runs its
 * module here, next to the desktop's, on every case.
 */

/** Daily from 1 June at 09:00 UTC. */
const BASE = {
  id: 'e1',
  calendar_id: 'c1',
  title: 'Standup',
  description: null,
  location: null,
  start: '2026-06-01T09:00:00.000Z',
  end: '2026-06-01T09:30:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=DAILY;COUNT=10', exceptions: [] as string[], tzid: null },
  color_label: null,
  reminders: [],
  sound: null,
  attendees: [],
};
const asEvent = (e: object) => e as unknown as CalendarEvent;
/** The row a provider keeps for one occurrence of the series. */
const rowAt = (slot: string, extra: object = {}) =>
  asEvent({
    ...BASE,
    id: `e1::rid::${slot}`,
    start: slot,
    end: slot,
    recurrence: null,
    ...extra,
  });

const SHAPES = {
  // Exchange lists the changed 5 June's slot among the exceptions and keeps
  // its row; the 3 June is deleted.
  exchange: {
    master: asEvent({
      ...BASE,
      recurrence: {
        ...BASE.recurrence,
        exceptions: ['2026-06-03T09:00:00Z', '2026-06-05T09:00:00Z'],
      },
    }),
    rows: [rowAt('2026-06-05T09:00:00Z', { start: '2026-06-05T11:00:00.000Z' })],
    carried: ['2026-06-03T09:00:00Z'],
  },
  // Google keeps the deleted 7 June as a cancelled row, with no exception.
  google: {
    master: asEvent(BASE),
    rows: [rowAt('2026-06-07T09:00:00Z', { cancelled: true })],
    carried: ['2026-06-07T09:00:00.000Z'],
  },
};
/** Where a move or copy starts from: the master, an occurrence the views
 *  expanded, or the provider's row for a changed occurrence. */
const FROM = {
  master: (shape: (typeof SHAPES)['exchange']) => shape.master,
  occurrence: (shape: (typeof SHAPES)['exchange']) =>
    asEvent({
      ...shape.master,
      id: 'e1@2026-06-08T09:00:00.000Z',
      series_id: 'e1',
      occurrence_start: '2026-06-08T09:00:00.000Z',
      start: '2026-06-08T09:00:00.000Z',
      end: '2026-06-08T09:30:00.000Z',
    }),
  override: () => rowAt('2026-06-05T09:00:00Z', { start: '2026-06-05T11:00:00.000Z' }),
};

function serve(master: CalendarEvent, rows: CalendarEvent[]) {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === 'get_event_by_id') return Promise.resolve(master);
    if (cmd === 'get_series_rows') return Promise.resolve({ rows, reach: { kind: 'complete' } });
    return Promise.resolve({});
  });
  phone.getEventById.mockResolvedValue(master);
  phone.getSeriesRows.mockResolvedValue({ rows, reach: { kind: 'complete' } });
  phone.updateEvent.mockResolvedValue({});
  phone.createEvent.mockResolvedValue({});
}

/** What each surface wrote: the series' start and recurrence. */
function desktopWrote(mode: 'move' | 'copy') {
  const call = invokeMock.mock.calls.find((c) => c[0] === (mode === 'move' ? 'update_event' : 'create_event'));
  const written = mode === 'move' ? call?.[1].event : call?.[1].request;
  return { start: written?.start, recurrence: written?.recurrence };
}
function phoneWrote(mode: 'move' | 'copy') {
  const call = (mode === 'move' ? phone.updateEvent : phone.createEvent).mock.calls[0];
  return {
    start: call?.[0].start,
    recurrence: call?.[0].recurrence,
    keepsZone: mode === 'copy' ? call?.[1]?.preserveRecurrenceZone === true : undefined,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
  for (const fn of Object.values(phone)) fn.mockReset();
});

describe('a whole series moved or copied, on the desktop and the phone alike', () => {
  for (const [shapeName, shape] of Object.entries(SHAPES)) {
    for (const [fromName, from] of Object.entries(FROM)) {
      for (const mode of ['move', 'copy'] as const) {
        it(`${mode} from the ${fromName}, ${shapeName}: the real deletions only`, async () => {
          serve(shape.master, shape.rows);
          await desktopMoveOrCopy(from(shape), 'c2', mode, 'series');
          await phoneMoveOrCopy(from(shape), 'c2', mode, 'series');
          const desktop = desktopWrote(mode);
          const onPhone = phoneWrote(mode);
          expect(desktop.start).toBe('2026-06-01T09:00:00.000Z');
          expect(desktop.recurrence?.exceptions).toEqual(shape.carried);
          expect(onPhone.start).toBe(desktop.start);
          expect(onPhone.recurrence).toEqual(desktop.recurrence);
          if (mode === 'copy') expect(onPhone.keepsZone).toBe(true);
        });
      }
    }
  }
});

/**
 * One occurrence moved to another calendar, on both surfaces, and one dragged
 * to another day where its calendar keeps no exceptions (decision 277): the
 * copy lands first, so a busy answer to the cancel of the original leaves the
 * move partly saved, never "nothing was changed" — moving again would make a
 * second copy.
 */
describe('an occurrence moved: a busy cancel after its copy landed keeps the original', () => {
  const busy = { code: 'forbidden', message: 'provider-busy: http-429' };
  const occurrence = FROM.occurrence(SHAPES.google);

  it('desktop: the copy names where the original stayed', async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === 'add_event_exdate'
        ? Promise.reject(busy)
        : Promise.resolve({ id: 'copy', title: 'Standup', calendar_id: 'c2' }),
    );
    await expect(desktopMoveOrCopy(occurrence, 'c2', 'move', 'occurrence')).resolves.toEqual(
      expect.objectContaining({ id: 'copy', original_left_in: 'c1' }),
    );
    expect(invokeMock.mock.calls.map((c) => c[0])).toEqual(['create_event', 'add_event_exdate']);
  });

  it('phone: the copy names where the original stayed', async () => {
    phone.createEvent.mockResolvedValue({ id: 'copy', title: 'Standup', calendar_id: 'c2' });
    phone.addEventExdate.mockRejectedValue(busy);
    await expect(phoneMoveOrCopy(occurrence, 'c2', 'move', 'occurrence')).resolves.toEqual(
      expect.objectContaining({ id: 'copy', original_left_in: 'c1' }),
    );
    expect(phone.createEvent).toHaveBeenCalledTimes(1);
  });

  it('desktop: any other failure of the cancel may have landed', async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === 'add_event_exdate'
        ? Promise.reject({ code: 'not_found', message: 'gone' })
        : Promise.resolve({}),
    );
    await expect(desktopMoveOrCopy(occurrence, 'c2', 'move', 'occurrence')).rejects.toEqual(
      expect.objectContaining({ code: 'network' }),
    );
  });

  it('desktop, dragged to another day', async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === 'add_event_exdate' ? Promise.reject(busy) : Promise.resolve({}),
    );
    // The drag's copy stands; the occurrence it left stayed, which the view
    // says as a move whose original stayed.
    await expect(moveEventToDay(occurrence, '2026-06-10', 'occurrence', null)).rejects.toBeInstanceOf(
      OriginalKeptError,
    );
    expect(invokeMock.mock.calls.map((c) => c[0])).toEqual(['create_event', 'add_event_exdate']);
  });
});
