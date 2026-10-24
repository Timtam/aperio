import { afterEach, describe, expect, it, vi } from 'vitest';

import type { CalendarEvent, Task } from '../api/types';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

import {
  duplicateEvent,
  duplicatedSentence,
  duplicateFailedSentence,
  duplicateTaskAndSay,
} from './duplicateActions';
import { SeriesNotLoadedError } from '../state/moveActions';

/**
 * Ctrl+D duplicates the whole series from its own start (decision 260), with
 * its real deletions only (259) and its zone as the source has it (262); a
 * failure is said in words (261).
 */

/** Daily from 1 June at 09:00 UTC: the 3 June deleted, the 5 June changed
 *  elsewhere (Exchange lists its slot and keeps its row). */
const MASTER = {
  id: 'e1',
  calendar_id: 'c1',
  title: 'Standup',
  description: null,
  location: null,
  start: '2026-06-01T09:00:00.000Z',
  end: '2026-06-01T09:30:00.000Z',
  all_day: false,
  recurrence: {
    rrule: 'FREQ=DAILY;COUNT=10',
    exceptions: ['2026-06-03T09:00:00Z', '2026-06-05T09:00:00Z'],
    tzid: null,
  },
  color_label: null,
  reminders: [],
  sound: null,
  attendees: [],
} as unknown as CalendarEvent;
const CHANGED = {
  ...MASTER,
  id: 'e1::rid::2026-06-05T09:00:00Z',
  start: '2026-06-05T11:00:00.000Z',
  end: '2026-06-05T11:30:00.000Z',
  recurrence: null,
} as unknown as CalendarEvent;
/** What the views hand in: the 8 June occurrence, expanded. */
const OCCURRENCE = {
  ...MASTER,
  id: 'e1@2026-06-08T09:00:00.000Z',
  series_id: 'e1',
  occurrence_start: '2026-06-08T09:00:00.000Z',
  start: '2026-06-08T09:00:00.000Z',
  end: '2026-06-08T09:30:00.000Z',
} as unknown as CalendarEvent;

function serving(master: CalendarEvent | null) {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === 'get_event_by_id') return Promise.resolve(master);
    if (cmd === 'get_series_rows') {
      return Promise.resolve({ rows: [CHANGED], reach: { kind: 'complete' } });
    }
    return Promise.resolve({});
  });
}
const callsTo = (cmd: string) => invokeMock.mock.calls.filter((call) => call[0] === cmd);

afterEach(() => {
  invokeMock.mockReset();
});

describe('duplicateEvent', () => {
  it('duplicates the whole series from its own start, with its real deletions only', async () => {
    serving(MASTER);
    await duplicateEvent(OCCURRENCE);
    const creates = callsTo('create_event');
    expect(creates).toHaveLength(1);
    const { request } = creates[0][1];
    expect(request.calendar_id).toBe('c1');
    expect(request.start).toBe('2026-06-01T09:00:00.000Z');
    expect(request.recurrence.rrule).toBe('FREQ=DAILY;COUNT=10');
    // The changed occurrence comes back at its time; the deleted one stays away.
    expect(request.recurrence.exceptions).toEqual(['2026-06-03T09:00:00Z']);
  });

  it("keeps the series without a zone, as the source has it, on a device with one", async () => {
    serving(MASTER);
    const zone = vi
      .spyOn(Intl.DateTimeFormat.prototype, 'resolvedOptions')
      .mockReturnValue({ timeZone: 'Europe/Berlin' } as Intl.ResolvedDateTimeFormatOptions);
    try {
      await duplicateEvent(OCCURRENCE);
    } finally {
      zone.mockRestore();
    }
    expect(callsTo('create_event')[0][1].request.recurrence.tzid).toBeNull();
  });

  it('duplicates the whole series from the row of a changed occurrence too, and says the series', async () => {
    serving(MASTER);
    // The row's own title differs from the series'.
    const written = await duplicateEvent({ ...CHANGED, title: 'Standup (verlegt)' } as CalendarEvent);
    const { request } = callsTo('create_event')[0][1];
    expect(request.start).toBe('2026-06-01T09:00:00.000Z');
    expect(request.recurrence).not.toBeNull();
    expect(request.title).toBe('Standup');
    const t = (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values)}`;
    expect(duplicatedSentence(written, t)).toBe('actions.duplicatedSeries {"title":"Standup"}');
  });

  it('says a single appointment as itself', () => {
    const t = (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values)}`;
    const single = { ...MASTER, recurrence: null } as CalendarEvent;
    expect(duplicatedSentence(single, t)).toBe('actions.duplicated {"title":"Standup"}');
  });

  it('writes nothing when the series cannot be loaded, and says so', async () => {
    serving(null);
    const failure = await duplicateEvent(OCCURRENCE).catch((err: unknown) => err);
    expect(failure).toBeInstanceOf(SeriesNotLoadedError);
    expect(callsTo('create_event')).toHaveLength(0);
    const t = (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values)}`;
    expect(duplicateFailedSentence(failure, 'Standup', t)).toMatch(/^dialogs\.event\.seriesLoadFailed/);
  });

  it('says another failure with its reason', () => {
    const t = (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values)}`;
    const said = duplicateFailedSentence(new Error('Der Server antwortet nicht'), 'Standup', t);
    expect(said).toMatch(/^actions\.duplicateFailed/);
    expect(said).toMatch(/Der Server antwortet nicht/);
  });

  it('says a copy that may stand only for a new series that may stand', () => {
    const t = (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values)}`;
    const mayStand = { code: 'network', message: 'created-may-stand: time' };
    expect(duplicateFailedSentence(mayStand, 'Standup', t)).toMatch(/^actions\.copyMayStand/);
    const inPart = { code: 'network', message: 'partly-saved: children' };
    expect(duplicateFailedSentence(inPart, 'Standup', t)).toMatch(
      /^dialogs\.event\.writeError\.partlySaved/,
    );
  });
});

describe('duplicateTaskAndSay (Ctrl+D on a task)', () => {
  const t = (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values)}`;
  const TASK = { id: 't1', list_id: 'L1', title: 'Einkaufen', assignees: [] } as unknown as Task;

  it('says a busy provider in words, and reads the list again (273)', async () => {
    invokeMock.mockReset();
    invokeMock.mockRejectedValue({ code: 'forbidden', message: 'provider-busy: http-429' });
    const announce = vi.fn();
    const invalidateData = vi.fn();
    // Resolves: a failure is said, never an unhandled rejection.
    await expect(duplicateTaskAndSay(TASK, { announce, invalidateData, t })).resolves.toBeUndefined();
    expect(announce).toHaveBeenCalledTimes(1);
    expect(announce.mock.calls[0][0]).toMatch(/^actions\.duplicateFailed /);
    expect(announce.mock.calls[0][0]).toContain('dialogs.event.writeError.reason.providerBusy');
    // A busy sentence always says to try later (decisions 272, 278).
    expect(announce.mock.calls[0][0]).toMatch(/ common\.tryAgainLater /);
    expect(invalidateData).toHaveBeenCalled();
  });

  it('says the copy when it lands', async () => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({ ...TASK, id: 't2' });
    const announce = vi.fn();
    await duplicateTaskAndSay(TASK, { announce, invalidateData: () => {}, t });
    expect(announce.mock.calls[0][0]).toMatch(/^actions\.duplicated /);
  });
});
