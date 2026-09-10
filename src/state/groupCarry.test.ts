import { describe, expect, it } from 'vitest';

import {
  carryOnto,
  futureCarryRow,
  occurrenceCarryRow,
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

  it('does not call a re-serialised start a change', () => {
    // The editor rebuilds start/end through `toIso`, which is not the spelling
    // the backend sent. Compared as strings, EVERY save looked like a move: a
    // reminder-only edit asked to carry, and carrying rewrote start and end
    // onto every copy.
    const plan = planCarry(
      group,
      anchor,
      fields({ start: '2026-08-10T08:00:00Z', end: '2026-08-10T09:00:00Z' }),
      fields({
        start: '2026-08-10T08:00:00.000Z',
        end: '2026-08-10T09:00:00.000Z',
      }),
      writable,
      titleOf,
    );
    expect(plan.changed).toEqual([]);
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

  it('builds the copy own occurrence, with only the change laid over it', () => {
    // The member's master: an hour long, with a reminder that is the reason
    // this copy exists at all.
    const master = {
      ...fields({ title: 'Wochenplanung', location: 'Raum 3' }),
      start: '2026-08-03T08:00:00.000Z',
      end: '2026-08-03T09:00:00.000Z',
      reminders: ['-PT30M'],
    };
    // The user renamed ONE occurrence, three weeks later. The time did not
    // change, so the row must land on that occurrence — not on the master's
    // own start, and not at the anchor's master time either.
    const row = occurrenceCarryRow(
      master,
      '2026-08-24T08:00:00.000Z',
      fields({ title: 'Wochenplanung kurz' }),
      ['title'],
    );
    if (row == null) throw new Error('the occurrence instant is readable');

    expect(row.start).toBe('2026-08-24T08:00:00.000Z');
    expect(row.end).toBe('2026-08-24T09:00:00.000Z');
    expect(row.title).toBe('Wochenplanung kurz');
    expect(row.location).toBe('Raum 3');
    expect(row.reminders).toEqual(['-PT30M']);
  });

  it('lands a MOVED occurrence where the user put it', () => {
    const master = {
      ...fields(),
      start: '2026-08-03T08:00:00.000Z',
      end: '2026-08-03T09:00:00.000Z',
    };
    const row = occurrenceCarryRow(
      master,
      '2026-08-24T08:00:00.000Z',
      fields({
        start: '2026-08-24T10:00:00.000Z',
        end: '2026-08-24T11:00:00.000Z',
      }),
      ['start', 'end'],
    );
    if (row == null) throw new Error('the fixture has a readable instant');
    expect(row.start).toBe('2026-08-24T10:00:00.000Z');
    expect(row.end).toBe('2026-08-24T11:00:00.000Z');
  });
});

/**
 * "This and all following", carried to a copy.
 *
 * These cases did not exist. `futureCarryRow` was the only function in the
 * module with no test at all — and it is the one whose own doc describes the
 * bug it was written against: the head was truncated before the copy's own
 * occurrence while the tail began at the anchor's, so the two halves did not
 * meet, the copy lost a real appointment, gained one on a day it never had,
 * and every occurrence after it fell out of phase.
 *
 * Every expectation below was MEASURED from the implementation as it stands,
 * not chosen. They are here so the behaviour is pinned before the rule crosses
 * into `cal-core`, where a mistranslation would be silent.
 */
describe('carrying "this and all following" to a copy', () => {
  /** The copy's own master: a week later than the anchor's, an hour long. */
  const copyMaster = () => ({
    ...fields({ start: '2026-08-12T14:00:00Z', end: '2026-08-12T15:00:00Z' }),
    reminders: ['-PT30M'],
  });
  /** The copy cuts at ITS own next occurrence, which is not the anchor's. */
  const copyCut = '2026-08-12T14:00:00Z';

  it('leaves the copy its own instants when only the title changed', () => {
    const row = futureCarryRow(
      copyMaster(),
      copyCut,
      fields(),
      fields({ title: 'Neuer Name' }),
      ['title'],
    );
    if (row == null) throw new Error('the fixture has a readable instant');
    expect(row.start).toBe('2026-08-12T14:00:00.000Z');
    expect(row.end).toBe('2026-08-12T15:00:00.000Z');
    expect(row.title).toBe('Neuer Name');
    // The reason this copy exists at all is untouched.
    expect(row.reminders).toEqual(['-PT30M']);
  });

  it('carries the SHIFT, applied to the copy own cut point', () => {
    // The user moved the anchor an hour later. The copy is cut a week on, at
    // 14:00 — writing the anchor's new instant here would put the tail three
    // days before the head.
    const row = futureCarryRow(
      copyMaster(),
      copyCut,
      fields(),
      fields({ start: '2026-08-10T09:00:00Z', end: '2026-08-10T10:00:00Z' }),
      ['start', 'end'],
    );
    if (row == null) throw new Error('the fixture has a readable instant');
    expect(row.start).toBe('2026-08-12T15:00:00.000Z');
    expect(row.end).toBe('2026-08-12T16:00:00.000Z');
  });

  it('carries a backward move the same way', () => {
    const row = futureCarryRow(
      copyMaster(),
      copyCut,
      fields(),
      fields({ start: '2026-08-09T08:00:00Z', end: '2026-08-09T09:00:00Z' }),
      ['start', 'end'],
    );
    if (row == null) throw new Error('the fixture has a readable instant');
    expect(row.start).toBe('2026-08-11T14:00:00.000Z');
    expect(row.end).toBe('2026-08-11T15:00:00.000Z');
  });

  it('adopts a new duration without a move', () => {
    const row = futureCarryRow(
      copyMaster(),
      copyCut,
      fields(),
      fields({ end: '2026-08-10T11:00:00Z' }),
      ['end'],
    );
    if (row == null) throw new Error('the fixture has a readable instant');
    expect(row.start).toBe('2026-08-12T14:00:00.000Z');
    expect(row.end).toBe('2026-08-12T17:00:00.000Z');
  });

  it('carries the other fields while deciding the instants itself', () => {
    const row = futureCarryRow(
      copyMaster(),
      copyCut,
      fields(),
      fields({
        location: 'Raum 3',
        start: '2026-08-10T10:00:00Z',
        end: '2026-08-10T11:30:00Z',
      }),
      ['location', 'start', 'end'],
    );
    if (row == null) throw new Error('the fixture has a readable instant');
    expect(row.location).toBe('Raum 3');
    expect(row.start).toBe('2026-08-12T16:00:00.000Z');
    expect(row.end).toBe('2026-08-12T17:30:00.000Z');
  });

  const allDayCopy = () =>
    fields({
      start: '2026-08-12T00:00:00Z',
      end: '2026-08-13T00:00:00Z',
      all_day: true,
    });

  it('moves an ALL-DAY copy in whole days, whatever the anchor did to the minute', () => {
    const row = futureCarryRow(
      allDayCopy(),
      '2026-08-12T00:00:00Z',
      fields(),
      fields({ start: '2026-08-10T20:00:00Z', end: '2026-08-10T21:00:00Z' }),
      ['start', 'end'],
    );
    if (row == null) throw new Error('the fixture has a readable instant');
    // Twelve hours, rounded to a day. A start that is not local midnight is
    // not an all-day event.
    expect(row.start).toBe('2026-08-13T00:00:00.000Z');
    // And an hour-long edit must not shrink it to an hour: the two disagree
    // about being all-day, so the copy keeps its own duration.
    expect(row.end).toBe('2026-08-14T00:00:00.000Z');
  });

  /**
   * The rounding is NOT symmetric, and that is measured rather than chosen.
   *
   * `Math.round` breaks a tie towards +∞, so exactly twelve hours forward is a
   * day and exactly twelve hours back is nothing. Rust's `f64::round` breaks
   * the same tie AWAY from zero, which would move a copy a whole day further
   * back on every exact half-day rewind — silently. Pinned here so the port
   * has to reproduce the tie rather than discover it.
   */
  it('breaks an exact half-day tie towards the future, in both directions', () => {
    const back = futureCarryRow(
      allDayCopy(),
      '2026-08-12T00:00:00Z',
      fields(),
      fields({ start: '2026-08-09T20:00:00Z', end: '2026-08-09T21:00:00Z' }),
      ['start', 'end'],
    );
    if (back == null) throw new Error('the fixture has a readable instant');
    expect(back.start).toBe('2026-08-12T00:00:00.000Z');

    const further = futureCarryRow(
      allDayCopy(),
      '2026-08-12T00:00:00Z',
      fields(),
      fields({ start: '2026-08-08T20:00:00Z', end: '2026-08-08T21:00:00Z' }),
      ['start', 'end'],
    );
    if (further == null) throw new Error('the fixture has a readable instant');
    expect(further.start).toBe('2026-08-11T00:00:00.000Z');
  });

  it('adopts the new duration when both agree about being all-day', () => {
    const row = futureCarryRow(
      allDayCopy(),
      '2026-08-12T00:00:00Z',
      fields({ all_day: true }),
      fields({
        start: '2026-08-10T00:00:00Z',
        end: '2026-08-13T00:00:00Z',
        all_day: true,
      }),
      ['start', 'end'],
    );
    if (row == null) throw new Error('the fixture has a readable instant');
    expect(row.start).toBe('2026-08-12T00:00:00.000Z');
    expect(row.end).toBe('2026-08-15T00:00:00.000Z');
  });

  /**
   * An unreadable cut point carries NOTHING, and says so.
   *
   * It used to THROW: `new Date(NaN).toISOString()` raises, in the middle of
   * the loop that carries to each member, so one bad row abandoned the rest of
   * the carry instead of reporting the one copy. The rule lives in
   * `cal_core::group_carry` now, where it cannot throw at all, and the answer
   * is the one the callers were already shaped for — they keep a list of
   * members they could not write, because a copy that silently kept its old
   * shape is the contradiction the group exists to prevent.
   *
   * Nothing produces an unreadable cut point today; it is the cut point of a
   * real occurrence. The case is here so the shape is a decision rather than
   * an accident.
   */
  it('carries nothing when the cut point cannot be read', () => {
    expect(
      futureCarryRow(
        copyMaster(),
        'irgendwann',
        fields(),
        fields({ start: '2026-08-10T09:00:00Z', end: '2026-08-10T10:00:00Z' }),
        ['start', 'end'],
      ),
    ).toBeNull();
  });
});
