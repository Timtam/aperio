import { describe, expect, it } from 'vitest';

import { androidHandover, rangByExpo, reminderPairId } from '../../shared/liveActivityAndroid';
import type { LiveActivityPlan, LiveActivitySection } from '../../shared/types';

const at = (hhmm: string) => `2026-11-09T${hhmm}:00Z`;
const TEXTS = {
  beginsAt: 'Beginnt um {time}',
  runsUntil: 'Läuft bis {time}',
  followUp: 'Danach: {title} um {time}',
  separator: ' · ',
};

function section(
  key: string,
  from: string,
  until: string,
  opts: { start: string; end: string; signal?: boolean; started?: boolean },
): LiveActivitySection {
  return {
    from: at(from),
    until: at(until),
    event: {
      key,
      item_id: `id-${key}`,
      calendar_id: 'work',
      original_start: null,
      title: key,
      location: key === 'A' ? 'Raum 2.14' : null,
      start: at(opts.start),
      end: at(opts.end),
    },
    follow_up: key === 'A' ? { title: 'B', start: at('10:55') } : null,
    started: opts.started ?? false,
    promoted: true,
    signal: opts.signal
      ? [{ item_id: `id-${key}`, calendar_id: 'work', occurrence_start: at(opts.start), fire_at: at(from) }]
      : [],
  };
}

const PLAN: LiveActivityPlan = {
  chains: [
    {
      keys: ['A', 'B'],
      start: at('09:50'),
      end: at('11:30'),
      sections: [
        section('A', '09:50', '10:00', { start: '10:00', end: '11:00', signal: true }),
        section('A', '10:00', '10:45', { start: '10:00', end: '11:00', started: true }),
        section('B', '10:45', '11:30', { start: '10:55', end: '11:30', signal: true }),
      ],
    },
  ],
};

// The notification list spells the instants its own way: they meet as numbers.
const A_PAIR = reminderPairId('id-A', '2026-11-09T09:50:00+00:00');
const B_PAIR = reminderPairId('id-B', '2026-11-09T10:45:00.000+00:00');
const CHANNELS = new Map([
  [A_PAIR, 'reminders-custom-a'],
  [B_PAIR, 'reminders-custom-abc'],
]);

describe('androidHandover — the plan, as the notifier reads it', () => {
  const handover = androidHandover(PLAN, 'de', TEXTS, (id) => CHANNELS.get(id));

  it('flattens the sections in time order, instants as milliseconds', () => {
    expect(handover.version).toBe(2);
    expect(handover.language).toBe('de');
    expect(handover.texts).toEqual(TEXTS);
    expect(handover.sections.map((s) => [s.key, s.from, s.until])).toEqual([
      ['A', Date.parse(at('09:50')), Date.parse(at('10:00'))],
      ['A', Date.parse(at('10:00')), Date.parse(at('10:45'))],
      ['B', Date.parse(at('10:45')), Date.parse(at('11:30'))],
    ]);
    expect(handover.sections.every((s) => s.chainEnd === Date.parse(at('11:30')))).toBe(true);
  });

  it('names the reminders a section IS, by item and moment, and none for the others', () => {
    expect(handover.sections.map((s) => s.signal)).toEqual([[A_PAIR], null, [B_PAIR]]);
  });

  it("rings each appointment on its own reminder's channel, in all its sections", () => {
    // A's second section is no reminder, and still rings on A's channel.
    expect(handover.sections.map((s) => s.channelId)).toEqual([
      'reminders-custom-a',
      'reminders-custom-a',
      'reminders-custom-abc',
    ]);
  });

  it('carries what the sentence needs', () => {
    expect(handover.sections[0]).toMatchObject({
      eventId: 'id-A',
      calendarId: 'work',
      title: 'A',
      location: 'Raum 2.14',
      start: Date.parse(at('10:00')),
      end: Date.parse(at('11:00')),
      followUpTitle: 'B',
      followUpStart: Date.parse(at('10:55')),
      started: false,
      promoted: true,
    });
    expect(handover.sections[1].started).toBe(true);
  });

  it('leaves the channel to the notifier when no reminder names one now', () => {
    const plain = androidHandover(PLAN, 'de', TEXTS, () => undefined);
    expect(plain.sections.map((s) => s.channelId)).toEqual([null, null, null]);
  });

  it('puts the sections of all chains in time order', () => {
    const later = {
      keys: ['C'],
      start: at('14:50'),
      end: at('16:00'),
      sections: [section('C', '14:50', '16:00', { start: '15:00', end: '16:00', signal: true })],
    };
    // The plan is in time order already; the flat list must not depend on it.
    const reversed = androidHandover({ chains: [later, ...PLAN.chains] }, 'de', TEXTS, () => undefined);
    expect(reversed.sections.map((s) => s.key)).toEqual(['A', 'A', 'B', 'C']);
  });
});

describe('rangByExpo — reminders the notifier must count as rung', () => {
  const handover = androidHandover(PLAN, 'de', TEXTS, () => undefined);

  it('counts every past reminder as Expo’s on the first hand-over', () => {
    const now = Date.parse(at('10:50'));
    expect(rangByExpo(handover, new Set(), now)).toEqual([A_PAIR, B_PAIR].sort());
  });

  it('leaves out what the notifier already held, and anything still to come', () => {
    const now = Date.parse(at('10:50'));
    expect(rangByExpo(handover, new Set([A_PAIR]), now)).toEqual([B_PAIR]);
    expect(rangByExpo(handover, new Set(), Date.parse(at('09:55')))).toEqual([A_PAIR]);
  });
});
