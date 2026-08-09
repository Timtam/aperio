import { describe, expect, it } from 'vitest';

import {
  carryOnto,
  planCarry,
  worthCarrying,
  type CarryableFields,
  type EventGroup,
} from '@aperio/shared';

const group: EventGroup = {
  id: 'g1',
  created_at: '2026-08-09T12:00:00Z',
  updated_at: '2026-08-09T12:00:00Z',
  members: [
    {
      calendar_id: 'work',
      event_id: 'ev-a',
      title: 'Wochenplanung',
      starts_at: '2026-08-10T08:00:00Z',
      added_at: '2026-08-09T12:00:00Z',
    },
    {
      calendar_id: 'private',
      event_id: 'ev-b',
      title: 'Wochenplanung',
      starts_at: '2026-08-10T08:00:00Z',
      added_at: '2026-08-09T12:00:01Z',
    },
    {
      calendar_id: 'colleague',
      event_id: 'ev-c',
      title: 'Wochenplanung',
      starts_at: '2026-08-10T08:00:00Z',
      added_at: '2026-08-09T12:00:02Z',
    },
  ],
};

const fields = (over: Partial<CarryableFields> = {}): CarryableFields => ({
  title: 'Wochenplanung',
  start: '2026-08-10T08:00:00Z',
  end: '2026-08-10T09:00:00Z',
  all_day: false,
  location: null,
  description: null,
  ...over,
});

const anchor = { calendar_id: 'work', event_id: 'ev-a' };
// The colleague's calendar is the read-only one, as it usually is.
const writable = (id: string) => id !== 'colleague';
const titleOf = (_cal: string, ev: string) => `Kopie ${ev}`;

describe('carrying an edit to the other copies', () => {
  it('names what changed, who gets it, and who cannot', () => {
    const plan = planCarry(
      group,
      anchor,
      fields(),
      fields({ start: '2026-08-10T10:00:00Z', end: '2026-08-10T11:00:00Z' }),
      writable,
      titleOf,
    );

    expect(plan.changed).toEqual(['start', 'end']);
    expect(plan.targets.map((tg) => tg.event_id)).toEqual(['ev-b']);
    // The one it must not write is reported, never quietly dropped — that
    // silence is what produces the contradiction groups exist to prevent.
    expect(plan.skipped.map((tg) => tg.event_id)).toEqual(['ev-c']);
    expect(worthCarrying(plan)).toBe(true);
  });

  it('leaves the edited copy out of its own carry', () => {
    const plan = planCarry(group, anchor, fields(), fields({ title: 'Neu' }), writable, titleOf);
    expect(plan.targets.some((tg) => tg.event_id === 'ev-a')).toBe(false);
    expect(plan.skipped.some((tg) => tg.event_id === 'ev-a')).toBe(false);
  });

  it('is not worth asking about when nothing carried changed', () => {
    // The user only changed the reminder — a property of THIS copy, and very
    // often the whole reason the copy exists.
    const plan = planCarry(group, anchor, fields(), fields(), writable, titleOf);
    expect(plan.changed).toEqual([]);
    expect(worthCarrying(plan)).toBe(false);
  });

  it('is not worth asking about when every other copy is read-only', () => {
    const readOnlyRest = () => false;
    const plan = planCarry(
      group,
      anchor,
      fields(),
      fields({ title: 'Neu' }),
      readOnlyRest,
      titleOf,
    );
    expect(plan.targets).toEqual([]);
    expect(plan.skipped).toHaveLength(2);
    expect(worthCarrying(plan)).toBe(false);
  });

  it('treats empty and absent as the same value', () => {
    // Providers disagree about which they return for a field nobody filled
    // in; a null-to-empty flip is not a change anyone made.
    const plan = planCarry(
      group,
      anchor,
      fields({ location: null }),
      fields({ location: '' }),
      writable,
      titleOf,
    );
    expect(plan.changed).toEqual([]);
  });

  it('carries only what changed and leaves the copy its own everything else', () => {
    const member = {
      ...fields({ title: 'Wochenplanung', location: 'Raum 3' }),
      reminders: ['-PT30M'],
      calendar_id: 'private',
    };
    const carried = carryOnto(
      member,
      fields({ title: 'Wochenplanung neu', location: 'Raum 4' }),
      ['title'],
    );

    expect(carried.title).toBe('Wochenplanung neu');
    // The location was not among the changed fields, so it stays.
    expect(carried.location).toBe('Raum 3');
    // And the reason this copy exists at all survives untouched.
    expect(carried.reminders).toEqual(['-PT30M']);
    expect(carried.calendar_id).toBe('private');
  });
});
