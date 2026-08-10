import { describe, expect, it } from 'vitest';

import {
  eventPrefillFrom,
  rankTitleSuggestions,
  taskPrefillFrom,
  type PrefillableEvent,
  type PrefillableTask,
} from '@aperio/shared';

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
