import { describe, expect, it } from 'vitest';

import type { Task } from '../api/types';
import {
  assigneeSuffix,
  normalPriority,
  priorityMarker,
  priorityRank,
  prioritySuffix,
  statusI18nKey,
  statusMarker,
  subtaskProgress,
  subtaskProgressSuffix,
} from './taskStatus';

const baseTask: Task = {
  id: 't',
  list_id: 'list',
  title: 'thing',
  description: null,
  status: 'open',
  priority: 'medium',
  effort: 'medium',
  scheduled_date: null,
  scheduled_time: null,
  deadline_date: null,
  deadline_time: null,
  deadline_reminder_days: null,
  recurrence: null,
  resurface_date: null,
  series_id: null,
  parent_id: null,
  section_id: null,
  color_label: null,
  reminders: [],
  assignees: [],
  sound: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  completed_at: null,
  etag: null,
};

describe('statusMarker', () => {
  it('maps every TaskStatus to a distinct glyph', () => {
    const seen = new Set([
      statusMarker('open'),
      statusMarker('in_progress'),
      statusMarker('completed'),
      statusMarker('cancelled'),
    ]);
    // Four statuses → four distinct glyphs. If two ever collide,
    // sighted users can't tell rows apart at a glance.
    expect(seen.size).toBe(4);
  });
});

describe('statusI18nKey', () => {
  it('returns the canonical key under views.tasks.state*', () => {
    expect(statusI18nKey('open')).toBe('views.tasks.stateOpen');
    expect(statusI18nKey('in_progress')).toBe('views.tasks.stateInProgress');
    expect(statusI18nKey('completed')).toBe('views.tasks.stateDone');
    expect(statusI18nKey('cancelled')).toBe('views.tasks.stateCancelled');
  });
});

describe('subtaskProgress', () => {
  it('returns null when the parent has no children', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p' },
      { ...baseTask, id: 'orphan' },
    ];
    expect(subtaskProgress('p', tasks)).toBeNull();
  });

  it('counts completed children, ignoring cancelled', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p' },
      { ...baseTask, id: 'c1', parent_id: 'p', status: 'completed' },
      { ...baseTask, id: 'c2', parent_id: 'p', status: 'open' },
      { ...baseTask, id: 'c3', parent_id: 'p', status: 'in_progress' },
      { ...baseTask, id: 'c4', parent_id: 'p', status: 'cancelled' },
    ];
    // 4 children total, but cancelled drops out of the denominator
    // — the fraction reflects what's actually left to do.
    expect(subtaskProgress('p', tasks)).toEqual({ done: 1, total: 3 });
  });

  it('only counts direct children, not grand-children', () => {
    const tasks: Task[] = [
      { ...baseTask, id: 'p' },
      { ...baseTask, id: 'c', parent_id: 'p', status: 'open' },
      { ...baseTask, id: 'gc', parent_id: 'c', status: 'completed' },
    ];
    expect(subtaskProgress('p', tasks)).toEqual({ done: 0, total: 1 });
  });
});

describe('subtaskProgressSuffix', () => {
  it('returns the empty string when there are no children', () => {
    const t = (k: string, v?: Record<string, unknown>) =>
      `${k}(${JSON.stringify(v)})`;
    expect(subtaskProgressSuffix(t, 'no-kids', [])).toBe('');
  });

  it('passes done and total to the i18n function', () => {
    const t = (k: string, v?: Record<string, unknown>) =>
      `${k}(${JSON.stringify(v)})`;
    const tasks: Task[] = [
      { ...baseTask, id: 'p' },
      { ...baseTask, id: 'c1', parent_id: 'p', status: 'completed' },
      { ...baseTask, id: 'c2', parent_id: 'p', status: 'open' },
    ];
    expect(subtaskProgressSuffix(t, 'p', tasks)).toBe(
      'views.tasks.subtaskProgress({"done":1,"total":2})',
    );
  });
});

describe('assigneeSuffix', () => {
  const t = (k: string, v?: Record<string, unknown>) =>
    `${k}(${JSON.stringify(v)})`;

  it('returns the empty string when there are no assignees', () => {
    expect(assigneeSuffix(t, [])).toBe('');
  });

  it('joins assignee names and passes them to the i18n function', () => {
    expect(
      assigneeSuffix(t, [
        { id: 'u1', name: 'Anna', email: null },
        { id: 'u2', name: 'Ben', email: 'ben@example.test' },
      ]),
    ).toBe('views.tasks.assigneeSuffix({"names":"Anna, Ben"})');
  });
});

describe('the two-level priority system', () => {
  const t = (k: string) => k;

  it('marks only the top level, and marks nothing else at all', () => {
    // The whole point of the second system: normal is the ABSENCE of a mark,
    // not a quieter one, so low and medium must both render empty.
    expect(priorityMarker('high', 'two')).toBe('★');
    expect(priorityMarker('medium', 'two')).toBe('');
    expect(priorityMarker('low', 'two')).toBe('');
    // Three levels are untouched.
    expect(priorityMarker('high', 'three')).toBe('!!!');
    expect(priorityMarker('medium', 'three')).toBe('!!');
    expect(priorityMarker('low', 'three')).toBe('!');
  });

  it('says "important", and says nothing for the rest', () => {
    expect(prioritySuffix(t, 'high', 'two')).toBe(', views.tasks.priorityImportant');
    expect(prioritySuffix(t, 'medium', 'two')).toBe('');
    // `low` announced "niedrige Priorität" in the three-level system; in this
    // one it is indistinguishable from medium and must stay silent.
    expect(prioritySuffix(t, 'low', 'two')).toBe('');
    expect(prioritySuffix(t, 'low', 'three')).toBe(', views.tasks.priorityLow');
  });

  it('sorts low and medium into ONE band', () => {
    // Ranking them apart would order a list by something the reader cannot
    // see — and would break the A→Z tiebreak inside the band.
    expect(priorityRank('high', 'two')).toBeLessThan(priorityRank('medium', 'two'));
    expect(priorityRank('medium', 'two')).toBe(priorityRank('low', 'two'));
    // Three levels keep their three ranks.
    expect(priorityRank('medium', 'three')).toBeLessThan(priorityRank('low', 'three'));
  });

  it('keeps the stored value when "important" is cleared', () => {
    // A `low` from a provider stays `low`: rewriting it to `medium` would
    // change nothing the user can see and everything another client sees.
    expect(normalPriority('low')).toBe('low');
    expect(normalPriority('medium')).toBe('medium');
    // Nothing to restore → the neutral middle.
    expect(normalPriority('high')).toBe('medium');
    expect(normalPriority()).toBe('medium');
    expect(normalPriority(null)).toBe('medium');
  });
});
