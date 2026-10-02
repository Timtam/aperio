import { describe, expect, it } from 'vitest';

import {
  eventPrefillFrom,
  findOffer,
  joinSuggestionPasses,
  offerKey,
  offerOptions,
  offerUsable,
  pickedOverOffer,
  prefillTarget,
  rankTitleSuggestions,
  taskPrefillFrom,
  type PrefillableEvent,
  type PrefillableTask,
} from '@aperio/shared';
import type { CalendarEvent } from '../api/types';
import { rankEventSuggestions } from '../state/useTitleSuggestions';

const item = (id: string, title: string, at: string) => ({ id, title, at });
const recency = (i: { at: string }) => i.at;

describe('offering what was written before', () => {
  it('offers a title that begins with what is typed, before one that merely contains it', () => {
    const out = rankTitleSuggestions(
      [
        item('a', 'Team Standup', '2026-08-01T09:00:00Z'),
        item('b', 'Standup Vertrieb', '2026-08-01T09:00:00Z'),
      ],
      'standup',
      recency,
    );
    expect(out.map((s) => s.title)).toEqual(['Standup Vertrieb', 'Team Standup']);
  });

  it('does not match inside a word', () => {
    // "stand" must not drag in "Verstandene Themen" — mid-word noise is the
    // fastest way to make a list of offers useless.
    const out = rankTitleSuggestions(
      [item('a', 'Verstandene Themen', '2026-08-01T09:00:00Z')],
      'stand',
      recency,
    );
    expect(out).toEqual([]);
  });

  it('offers one entry per title, the most recent of its kind', () => {
    // A weekly appointment is in the history dozens of times; offering it
    // dozens of times would spend the whole list on one answer.
    const out = rankTitleSuggestions(
      [
        item('old', 'Physio', '2025-01-06T09:00:00Z'),
        item('new', 'physio', '2026-07-06T09:00:00Z'),
        item('mid', 'Physio', '2026-01-06T09:00:00Z'),
      ],
      'phys',
      recency,
    );
    expect(out).toHaveLength(1);
    expect(out[0].item.id).toBe('new');
    // The title comes back as it was WRITTEN, not folded.
    expect(out[0].title).toBe('physio');
  });

  it('says nothing for an empty query', () => {
    const out = rankTitleSuggestions(
      [item('a', 'Physio', '2026-07-06T09:00:00Z')],
      '   ',
      recency,
    );
    expect(out).toEqual([]);
  });

  it('still offers an item with no date, but last', () => {
    const out = rankTitleSuggestions(
      [
        item('undated', 'Physio Termin', ''),
        item('dated', 'Physio Sitzung', '2026-07-06T09:00:00Z'),
      ],
      'physio ',
      recency,
    );
    expect(out.map((s) => s.item.id)).toEqual(['dated', 'undated']);
  });
});

const pastEvent: PrefillableEvent = {
  id: 'ev-1',
  calendar_id: 'private',
  title: 'Physio',
  description: 'Rezept mitnehmen',
  location: 'Praxis Nord',
  start: '2026-03-02T09:00:00.000Z',
  end: '2026-03-02T09:45:00.000Z',
  all_day: false,
  recurrence: null,
  color_label: 'blue',
  reminders: ['-PT30M'],
  attendees: [],
};

describe('what an earlier event lends a new one', () => {
  it('lends its LENGTH, not its end', () => {
    // The new event is on another day; an end lifted verbatim would put it in
    // the past.
    const fill = eventPrefillFrom(pastEvent);
    expect(fill.durationMinutes).toBe(45);
    expect(fill).not.toHaveProperty('start');
    expect(fill).not.toHaveProperty('end');
  });

  it('lends what makes the appointment itself', () => {
    const fill = eventPrefillFrom(pastEvent);
    expect(fill.location).toBe('Praxis Nord');
    expect(fill.description).toBe('Rezept mitnehmen');
    expect(fill.color_label).toBe('blue');
    expect(fill.reminders).toEqual(['-PT30M']);
    expect(fill.calendar_id).toBe('private');
  });

  it('lends the repetition, but never the old series holes', () => {
    // EXDATEs name instants of the OLD series; on a new one they would punch
    // holes in days the user never touched.
    const weekly: PrefillableEvent = {
      ...pastEvent,
      recurrence: {
        rrule: 'FREQ=WEEKLY;COUNT=10',
        exceptions: ['2026-03-09T09:00:00.000Z'],
        tzid: null,
      },
    };
    const fill = eventPrefillFrom(weekly);
    expect(fill.rrule).toBe('FREQ=WEEKLY;COUNT=10');
    expect(fill).not.toHaveProperty('exceptions');
  });

  it('drops a repetition that has already ended', () => {
    // An UNTIL in the past would create a series with nothing in it, which
    // reads as the app having quietly dropped the repetition.
    const ended: PrefillableEvent = {
      ...pastEvent,
      recurrence: {
        rrule: 'FREQ=WEEKLY;UNTIL=20260401T090000Z',
        exceptions: [],
        tzid: null,
      },
    };
    expect(eventPrefillFrom(ended, new Date('2026-08-10T00:00:00Z')).rrule).toBeNull();
    // …but one that has not is kept.
    expect(eventPrefillFrom(ended, new Date('2026-03-10T00:00:00Z')).rrule).toBe(
      'FREQ=WEEKLY;UNTIL=20260401T090000Z',
    );
  });

  it('keeps a COUNT-bounded repetition, which counts from wherever it starts', () => {
    const counted: PrefillableEvent = {
      ...pastEvent,
      recurrence: { rrule: 'FREQ=WEEKLY;COUNT=4', exceptions: [], tzid: null },
    };
    expect(eventPrefillFrom(counted, new Date('2030-01-01T00:00:00Z')).rrule).toBe(
      'FREQ=WEEKLY;COUNT=4',
    );
  });
});

describe('what an earlier task lends a new one', () => {
  const pastTask: PrefillableTask = {
    id: 't-1',
    list_id: 'privat',
    title: 'Steuer sortieren',
    description: 'Belege aus dem Ordner',
    priority: 'high',
    effort: 'large',
    color_label: 'red',
    reminders: ['-P1D'],
    recurrence: 'FREQ=MONTHLY',
    deadline_reminder_days: 3,
  };

  it('lends everything but when it is due', () => {
    const fill = taskPrefillFrom(pastTask);
    expect(fill).toEqual({
      title: 'Steuer sortieren',
      list_id: 'privat',
      description: 'Belege aus dem Ordner',
      priority: 'high',
      effort: 'large',
      color_label: 'red',
      reminders: ['-P1D'],
      recurrence: 'FREQ=MONTHLY',
      deadline_reminder_days: 3,
    });
    // Explicitly NOT: dates, status, or who it was assigned to — a task copied
    // from one assigned to a colleague would quietly put work on their plate.
    expect(fill).not.toHaveProperty('scheduled_date');
    expect(fill).not.toHaveProperty('deadline_date');
    expect(fill).not.toHaveProperty('status');
    expect(fill).not.toHaveProperty('assignees');
  });
});

describe('which of two rows with one title', () => {
  /** The shape the tier callback judges: a row that is finished, or not. */
  const row = (id: string, title: string, at: string, done: boolean) => ({
    id,
    title,
    at,
    done,
  });
  const liveFirst = (i: { done: boolean }) => (i.done ? 1 : 0);

  it('offers the living task, not the newer completion record', () => {
    // THE bug. A repeating task on a provider with native recurrence
    // (Vikunja) leaves a completion record behind on every tick: a finished,
    // deliberately non-repeating copy under the same title, written just now.
    // Being the newest row of its name it won every time — so accepting the
    // offer filled the editor from the one copy that is guaranteed to have no
    // repetition and no reminders, and the repetition the user wanted was
    // silently absent.
    const out = rankTitleSuggestions(
      [
        row('live', 'Handtücher wechseln', '2026-08-01T07:00:00Z', false),
        row('record', 'Handtücher wechseln', '2026-08-11T19:30:00Z', true),
      ],
      'handtücher',
      recency,
      undefined,
      liveFirst,
    );
    expect(out).toHaveLength(1);
    expect((out[0].item as { id: string }).id).toBe('live');
  });

  it('still offers a finished task when that is all there is', () => {
    // Most tasks are one-offs done once. The history is exactly what makes
    // them worth offering; only the tiebreak changed.
    const out = rankTitleSuggestions(
      [row('done', 'Steuer abgeben', '2026-05-01T07:00:00Z', true)],
      'steuer',
      recency,
      undefined,
      liveFirst,
    );
    expect(out.map((s) => s.title)).toEqual(['Steuer abgeben']);
  });

  it('falls back to the most recent within one tier', () => {
    const out = rankTitleSuggestions(
      [
        row('old', 'Müll rausbringen', '2026-01-01T07:00:00Z', false),
        row('new', 'Müll rausbringen', '2026-08-01T07:00:00Z', false),
      ],
      'müll',
      recency,
      undefined,
      liveFirst,
    );
    expect((out[0].item as { id: string }).id).toBe('new');
  });

  it('behaves exactly as before when the caller names no tiers', () => {
    // Events pass none today; the newest still wins there.
    const out = rankTitleSuggestions(
      [
        row('a', 'Zahnarzt', '2026-01-01T07:00:00Z', false),
        row('b', 'Zahnarzt', '2026-08-01T07:00:00Z', true),
      ],
      'zahnarzt',
      recency,
    );
    expect((out[0].item as { id: string }).id).toBe('b');
  });
});

describe('joining the two passes the task offers run', () => {
  const row = (id: string, title: string) => ({ id, title });

  it('keeps the unfinished pass first and drops the duplicate', () => {
    // The live task appears in BOTH passes — it matches the query and it is
    // unfinished. Offering it twice would spend two of six slots on one
    // answer.
    const out = joinSuggestionPasses(
      [row('live', 'Handtücher wechseln')],
      [row('record-1', 'Handtücher wechseln'), row('live', 'Handtücher wechseln')],
    );
    expect(out.map((r) => r.id)).toEqual(['live', 'record-1']);
  });

  it('brings the history when nothing is unfinished', () => {
    // The ordinary case: a task done once, months ago. The first pass is
    // empty and the offer comes entirely from the second.
    const out = joinSuggestionPasses([], [row('done', 'Steuer abgeben')]);
    expect(out.map((r) => r.id)).toEqual(['done']);
  });

  it('survives a history that would have crowded the live row out', () => {
    // What the second pass looks like for a weekly task after a few years:
    // 200 completion records and no room for the task itself. The separate
    // pass is the only reason it is here at all.
    const history = Array.from({ length: 200 }, (_, i) =>
      row(`record-${i}`, 'Handtücher wechseln'),
    );
    const out = joinSuggestionPasses([row('live', 'Handtücher wechseln')], history);
    expect(out[0].id).toBe('live');
    expect(out).toHaveLength(201);
  });
});

describe('which offer was accepted (decisions 160 and 161)', () => {
  const CONTAINERS = [
    { id: 'cal-local', name: 'Kalender', read_only: false },
    { id: 'cal-work', name: 'Arbeit', read_only: false },
    { id: 'cal-feed', name: 'Abo', read_only: true },
  ];
  const ev = (id: string, calendar_id: string, start: string, cancelled = false) =>
    ({ id, calendar_id, title: 'Thomas Meeting', start, cancelled }) as unknown as CalendarEvent;

  it('names an offer by its container and its id', () => {
    expect(offerKey('cal-work', 'ev-1')).not.toBe(offerKey('cal-local', 'ev-1'));
    // No two spellings collide, whatever the parts contain.
    expect(offerKey('a', 'b c')).not.toBe(offerKey('a b', 'c'));
  });

  it('finds the copy the offer named, not the first with that id', () => {
    const rows = [
      ev('ev-1', 'cal-local', '2026-05-15T09:00:00Z'),
      ev('ev-1', 'cal-work', '2026-06-15T09:00:00Z'),
    ];
    const found = findOffer(rows, offerKey('cal-work', 'ev-1'), (e) => e.calendar_id);
    expect(found?.calendar_id).toBe('cal-work');
  });

  it('hints a container that takes nothing new as read-only', () => {
    const ranked = [{ item: ev('ev-1', 'cal-feed', '2026-06-15T09:00:00Z'), title: 'Thomas Meeting' }];
    const [option] = offerOptions(ranked, (e) => e.calendar_id, CONTAINERS, (n) => `${n}, nur lesbar`);
    expect(option).toEqual({
      id: offerKey('cal-feed', 'ev-1'),
      title: 'Thomas Meeting',
      hint: 'Abo, nur lesbar',
    });
  });

  it('offers a writable copy before a newer one in a read-only calendar', () => {
    const rows = [
      ev('feed', 'cal-feed', '2026-06-15T09:00:00Z'),
      ev('own', 'cal-work', '2026-05-15T09:00:00Z'),
    ];
    const [best] = rankEventSuggestions(rows, 'thomas', offerUsable(CONTAINERS));
    expect(best.item.id).toBe('own');
  });

  it('still offers the read-only copy when it is the only one', () => {
    const rows = [ev('feed', 'cal-feed', '2026-06-15T09:00:00Z')];
    const [best] = rankEventSuggestions(rows, 'thomas', offerUsable(CONTAINERS));
    expect(best.item.id).toBe('feed');
  });

  it('counts an unknown calendar as one that takes nothing new', () => {
    expect(offerUsable(CONTAINERS)('cal-gone')).toBe(false);
    expect(offerUsable(CONTAINERS)('cal-work')).toBe(true);
  });

  it('decides where a filled-in item goes', () => {
    expect(prefillTarget('cal-work', CONTAINERS, false)).toEqual({ kind: 'offer', id: 'cal-work' });
    expect(prefillTarget('cal-work', CONTAINERS, true)).toEqual({ kind: 'pinned' });
    expect(prefillTarget('cal-feed', CONTAINERS, false)).toEqual({ kind: 'readOnly', name: 'Abo' });
    expect(prefillTarget('cal-gone', CONTAINERS, false)).toEqual({ kind: 'unknown' });
    expect(prefillTarget('', CONTAINERS, false)).toEqual({ kind: 'none' });
  });

  it('counts a picker as chosen only when it was left off its default', () => {
    expect(pickedOverOffer(false, 'cal-work', 'cal-local')).toBe(false);
    // Arrowed through and back: moved, but not chosen.
    expect(pickedOverOffer(true, 'cal-local', 'cal-local')).toBe(false);
    expect(pickedOverOffer(true, 'cal-work', 'cal-local')).toBe(true);
    expect(pickedOverOffer(true, 'cal-work', null)).toBe(true);
  });

  it('keeps two tasks with one id from two servers apart when joining', () => {
    const a = { id: '1', list_id: 'vikunja-a', title: 'Einkaufen' };
    const b = { id: '1', list_id: 'vikunja-b', title: 'Einkaufen' };
    const out = joinSuggestionPasses([a], [b], (t) => offerKey(t.list_id, t.id));
    expect(out).toEqual([a, b]);
  });
});
