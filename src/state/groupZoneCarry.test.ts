import { describe, expect, it } from 'vitest';

import contract from '../../crates/cal-core/tests/fixtures/groupZoneCarry.json';
import {
  carryTargetKey,
  planCarry,
  textFields,
  zoneCarry,
  zoneOffer,
  zoneRefusalText,
  type CarryableFields,
  type CarryPlan,
  type CarryTarget,
  type EventGroup,
  type ZoneCarry,
  type ZoneCarryQuestion,
  type ZoneSeriesRefused,
} from '@aperio/shared';

/**
 * Carrying a series' zone change to its copies (stage 13 of
 * DESIGN-series-time-zone.md, 143a to 146a), pinned by
 * `crates/cal-core/tests/fixtures/groupZoneCarry.json`: every row through the
 * door this surface installs, the real WebAssembly module. The core and the
 * phone's bridge read the same file.
 */
describe('zoneCarry contract (WebAssembly door and shared shell)', () => {
  // Anti-silence: the rows the decisions and the closed holes turn on.
  it('still carries the rows the rule turns on', () => {
    const names = new Set(contract.rows.map((row) => row.name));
    for (const needed of [
      'a-true-copy-as-they-fall-now',
      'a-true-copy-as-the-rule-says',
      'a-copy-in-the-other-half-year-comes-to-the-series-hour',
      'copy-in-june-anchor-in-january-as-they-fall-now',
      'a-copy-an-hour-later-keeps-its-distance',
      'a-copy-already-repaired-is-unchanged',
      'a-copy-already-repaired-gets-a-new-end',
      'a-copy-in-vienna-keeps-vienna',
      'troll-on-a-calendar-that-cannot-store-it',
      'a-copy-without-a-rule',
      'every-other-week-in-the-other-parity',
      'a-move-into-the-spring-gap',
      'zone-then-a-date-says-only-the-zone-days',
    ]) {
      expect(names, `fixture lost ${needed}`).toContain(needed);
    }
  });

  for (const row of contract.rows) {
    it(`answers ${row.name}`, () => {
      expect(zoneCarry(row.question as ZoneCarryQuestion), row.note).toEqual(row.expect);
    });
  }
});

const target = (event_id: string, writable = true): CarryTarget => ({
  calendar_id: `cal-${event_id}`,
  event_id,
  title: `Kopie ${event_id}`,
  writable,
});

const plan = (changed: CarryPlan['changed'], targets: CarryTarget[], skipped: CarryTarget[] = []): CarryPlan => ({
  changed,
  targets,
  skipped,
  worth_carrying: true,
});

const changed = (zoneDays: number): ZoneCarry => ({
  outcome: 'changed',
  rrule: 'FREQ=WEEKLY;BYDAY=MO',
  start: '2027-01-04T08:00:00Z',
  end: '2027-01-04T09:00:00Z',
  exceptions: [],
  tzid: 'Europe/Berlin',
  shiftMinutes: 60,
  zoneDays,
});
const unchanged: ZoneCarry = { outcome: 'unchanged' };
const notASeries: ZoneCarry = { outcome: 'refused', reason: 'not_a_series' };
const offPattern: ZoneCarry = { outcome: 'refused', reason: 'off_pattern' };
const skippedTime: ZoneCarry = { outcome: 'refused', reason: 'rule_cannot_move', rule: 'skipped_time' };

const verdicts = (pairs: [CarryTarget, ZoneCarry][]) =>
  new Map(pairs.map(([copy, verdict]) => [carryTargetKey(copy), verdict]));

describe('the offer after a zone change (zoneOffer)', () => {
  const [a, b, c, d] = ['a', 'b', 'c', 'd'].map((id) => target(id));

  it('carries to the copies that move and leaves out the ones already right', () => {
    const offer = zoneOffer(
      plan(['zone'], [a, b, c]),
      verdicts([
        [a, changed(0)],
        [b, unchanged],
        [c, changed(-1)],
      ]),
    );
    expect(offer.targets).toEqual([a, c]);
    expect(offer.days).toEqual([{ days: -1, count: 1 }]);
    expect(offer.nothingToCarry).toBe(false);
    expect(offer.silent).toBe(false);
  });

  it('keeps a copy already right when a text field changed, for that field', () => {
    const offer = zoneOffer(plan(['zone', 'title'], [b]), verdicts([[b, unchanged]]));
    expect(offer.targets).toEqual([b]);
    expect(offer.nothingToCarry).toBe(false);
    expect(offer.silent).toBe(false);
  });

  it('says nothing when every copy already stands as the series does', () => {
    const offer = zoneOffer(
      plan(['zone'], [a, b]),
      verdicts([
        [a, unchanged],
        [b, unchanged],
      ]),
    );
    expect(offer.targets).toEqual([]);
    expect(offer.nothingToCarry).toBe(true);
    expect(offer.silent).toBe(true);
  });

  it('is not silent while a copy was skipped: it has to be named', () => {
    const offer = zoneOffer(plan(['zone'], [a], [target('r', false)]), verdicts([[a, unchanged]]));
    expect(offer.silent).toBe(false);
    expect(offer.nothingToCarry).toBe(true);
  });

  it('counts a copy without a verdict as a target that is written', () => {
    const offer = zoneOffer(plan(['zone'], [a]), new Map());
    expect(offer.targets).toEqual([a]);
    expect(offer.nothingToCarry).toBe(false);
    expect(offer.silent).toBe(false);
  });

  it('names the series that refuse the zone and still carries their text fields (146a)', () => {
    const offer = zoneOffer(
      plan(['zone', 'title'], [a, b]),
      verdicts([
        [a, offPattern],
        [b, skippedTime],
      ]),
    );
    expect(offer.targets).toEqual([a, b]);
    expect(offer.refused).toEqual([
      { target: a, verdict: offPattern },
      { target: b, verdict: skippedTime },
    ]);
    expect(offer.nothingToCarry).toBe(false);
  });

  it('writes nothing to a refusing series when only the zone changed', () => {
    const offer = zoneOffer(plan(['zone'], [a]), verdicts([[a, offPattern]]));
    expect(offer.refused).toEqual([{ target: a, verdict: offPattern }]);
    expect(offer.nothingToCarry).toBe(true);
    expect(offer.silent).toBe(false);
  });

  // Hole 2: a single-event copy on a zone-only save stayed a target, the
  // carry wrote it unchanged and reported "Eine Kopie aktualisiert".
  it('leaves a copy that is no series out of the targets when only the zone changed', () => {
    const offer = zoneOffer(plan(['zone'], [d]), verdicts([[d, notASeries]]));
    expect(offer.notASeries).toEqual([d]);
    expect(offer.targets).toEqual([]);
    expect(offer.nothingToCarry).toBe(true);
    expect(offer.silent).toBe(false);
  });

  it('carries everything but the zone to a copy that is no series there', () => {
    const offer = zoneOffer(plan(['zone', 'start', 'title'], [d]), verdicts([[d, notASeries]]));
    expect(offer.notASeries).toEqual([d]);
    expect(offer.targets).toEqual([d]);
    expect(offer.nothingToCarry).toBe(false);
  });

  it('groups the day moves by how far, earliest first', () => {
    const offer = zoneOffer(
      plan(['zone'], [a, b, c, d]),
      verdicts([
        [a, changed(1)],
        [b, changed(-1)],
        [c, changed(-1)],
        [d, changed(0)],
      ]),
    );
    expect(offer.days).toEqual([
      { days: -1, count: 2 },
      { days: 1, count: 1 },
    ]);
  });
});

// Hole 0: the day move is read on the new clock before and after, so the
// offer says what the user's own answer said (138a): nothing as they fall
// now, a day earlier ("vor") as the rule says.
describe('the day move the offer says, through the door', () => {
  const rowNamed = (name: string) => {
    const row = contract.rows.find((r) => r.name === name);
    if (!row) throw new Error(`fixture lost ${name}`);
    return row.question as ZoneCarryQuestion;
  };
  const copy = target('copy');

  it('says no day move as they fall now', () => {
    const verdict = zoneCarry(rowNamed('a-true-copy-as-they-fall-now'));
    expect(zoneOffer(plan(['zone'], [copy]), verdicts([[copy, verdict]])).days).toEqual([]);
  });

  it('says a day earlier as the rule says', () => {
    const verdict = zoneCarry(rowNamed('a-true-copy-as-the-rule-says'));
    expect(zoneOffer(plan(['zone'], [copy]), verdicts([[copy, verdict]])).days).toEqual([
      { days: -1, count: 1 },
    ]);
  });
});

describe('what goes to a copy whatever its zone does (textFields)', () => {
  it('is the title, the location and the description, in the plan order', () => {
    expect(textFields(['zone', 'title', 'start', 'end', 'all_day', 'location', 'description'])).toEqual([
      'title',
      'location',
      'description',
    ]);
    expect(textFields(['zone', 'start', 'end'])).toEqual([]);
  });
});

describe('the reason a series does not get the zone (zoneRefusalText)', () => {
  const t = (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values ?? {})}`;
  const refusal = (verdict: ZoneCarry) => verdict as ZoneSeriesRefused;

  it("names the calendar's reason with the zone's city", () => {
    expect(
      zoneRefusalText(refusal({ outcome: 'refused', reason: 'zone_not_storable' }), 'America/New_York', t),
    ).toBe('dialogs.eventGroupCarry.zoneRefusal.zone_not_storable {"zone":"New York"}');
    expect(zoneRefusalText(refusal(offPattern), 'Europe/Berlin', t)).toBe(
      'dialogs.eventGroupCarry.zoneRefusal.off_pattern {"zone":"Berlin"}',
    );
  });

  it("says a rule's own reason as the zone question does", () => {
    expect(zoneRefusalText(refusal(skippedTime), 'Europe/Berlin', t)).toBe(
      'dialogs.moveScope.refusal.skipped_time {"clock":"Berlin"}',
    );
  });
});

describe('a zone change in the plan (143a)', () => {
  const group: EventGroup = {
    id: 'g1',
    created_at: '2026-11-20T12:00:00Z',
    updated_at: '2026-11-20T12:00:00Z',
    members: ['a', 'b', 'c'].map((id, i) => ({
      calendar_id: `cal-${id}`,
      event_id: id,
      title: 'Wochenplanung',
      starts_at: '2026-11-02T08:00:00Z',
      added_at: `2026-11-20T12:00:0${i}Z`,
    })),
  };
  const fields = (tzid: string | null, over: Partial<CarryableFields> = {}): CarryableFields => ({
    title: 'Wochenplanung',
    start: '2026-11-02T08:00:00Z',
    end: '2026-11-02T09:00:00Z',
    all_day: false,
    location: null,
    description: null,
    zone: { tzid },
    ...over,
  });
  const anchor = { calendar_id: 'cal-a', event_id: 'a' };

  it('names the zone first, and offers a zone-only change', () => {
    const plan = planCarry(
      group,
      anchor,
      fields(null),
      fields('America/New_York', { title: 'Neu' }),
      () => true,
      (_cal, ev) => ev,
    );
    expect(plan.changed).toEqual(['zone', 'title']);
    const zoneOnly = planCarry(group, anchor, fields(null), fields('Europe/Berlin'), () => true, (_c, ev) => ev);
    expect(zoneOnly.changed).toEqual(['zone']);
    expect(zoneOnly.worth_carrying).toBe(true);
  });

  it('offers a zone change with every other copy read-only, to name them', () => {
    const plan = planCarry(group, anchor, fields(null), fields('Europe/Berlin'), () => false, (_c, ev) => ev);
    expect(plan.targets).toEqual([]);
    expect(plan.skipped).toHaveLength(2);
    expect(plan.worth_carrying).toBe(true);
  });

  it('reads one zone under two names as no change', () => {
    const plan = planCarry(group, anchor, fields('Asia/Calcutta'), fields('Asia/Kolkata'), () => true, (_c, ev) => ev);
    expect(plan.changed).toEqual([]);
  });
});
