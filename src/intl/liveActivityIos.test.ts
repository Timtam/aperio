import { describe, expect, it } from 'vitest';

import {
  EMPTY_ACTIVITY_MEMORY,
  MAX_PENDING,
  activityContent,
  activityEnds,
  activityPassSummary,
  nextWake,
  reconcileIosActivities,
  remember,
  triggerKeyId,
  type ActivityMemory,
  type ReconcileInput,
  type ReconcileResult,
  type ReportedActivity,
} from '../../shared/liveActivityIos';
import type { LiveActivityChain, LiveActivitySection } from '../../shared/types';

const at = (hhmm: string) => `2026-11-09T${hhmm}:00Z`;
const WORDS = { begins: 'Beginn', ends: 'Ende', followUp: 'Danach: {title} um {time}', over: 'Vorbei' };

function section(
  key: string,
  from: string,
  until: string,
  opts: { start?: string; end?: string; signal?: boolean; followUp?: [string, string] } = {},
): LiveActivitySection {
  return {
    from: at(from),
    until: at(until),
    event: {
      key,
      item_id: key,
      calendar_id: 'work',
      original_start: null,
      title: key,
      location: null,
      start: at(opts.start ?? until),
      end: at(opts.end ?? until),
    },
    follow_up: opts.followUp ? { title: opts.followUp[0], start: at(opts.followUp[1]) } : null,
    started: false,
    promoted: true,
    signal: opts.signal
      ? [{ item_id: key, calendar_id: 'work', occurrence_start: at(opts.start ?? until), fire_at: at(from) }]
      : [],
  };
}

/** A at 10:00-11:00 (reminder 09:50), crossfading into B at 10:55-11:30
 *  (reminder 10:45). */
const CROSSFADE: LiveActivityChain = {
  keys: ['A', 'B'],
  start: at('09:50'),
  end: at('11:30'),
  sections: [
    section('A', '09:50', '10:45', { start: '10:00', end: '11:00', signal: true, followUp: ['B', '10:55'] }),
    section('B', '10:45', '11:30', { start: '10:55', end: '11:30', signal: true }),
  ],
};

const single = (key: string, from: string, until: string): LiveActivityChain => ({
  keys: [key],
  start: at(from),
  end: at(until),
  sections: [section(key, from, until, { signal: true })],
});

function input(overrides: Partial<ReconcileInput>): ReconcileInput {
  return {
    now: new Date(at('08:00')),
    chains: [],
    reported: [],
    memory: EMPTY_ACTIVITY_MEMORY,
    foreground: true,
    canSchedule: true,
    planAhead: true,
    enabled: true,
    isSilent: () => false,
    locale: 'de',
    words: WORDS,
    hideOnLockScreen: false,
    build: '68',
    endedByWake: [],
    ...overrides,
  };
}

const types = (actions: { type: string }[]) => actions.map((a) => a.type);

/** The wake a pass hands over once iOS took every start and schedule: what
 *  the native side arms. */
function wakeAfter(result: ReconcileResult, now: string): string | null {
  let memory = result.memory;
  result.actions.forEach((action, i) => {
    if (action.type === 'start' || action.type === 'schedule') {
      memory = remember(memory, `new${i}`, action, new Date(at(now)));
    }
  });
  return nextWake(activityEnds(memory), new Date(at(now)));
}

describe('activityContent — the one switch iOS makes alone', () => {
  it('goes to the next appointment of the chain', () => {
    const content = activityContent(CROSSFADE, new Date(at('09:55')), 'de', WORDS);
    expect(content.current.key).toBe('A');
    expect(content.current.followUpTitle).toBe('B');
    expect(content.next?.key).toBe('B');
    expect(content.staleAt).toBe(at('10:45'));
  });

  it('goes to the end when nothing follows, where the appointment reads as over', () => {
    const content = activityContent(single('A', '09:50', '11:00'), new Date(at('09:55')), 'de', WORDS);
    expect(content.next).toBeNull();
    expect(content.staleAt).toBe(at('11:00'));
  });

  it('shows the running section, and a later chain outranks an earlier one', () => {
    const later = activityContent(CROSSFADE, new Date(at('10:50')), 'de', WORDS);
    expect(later.current.key).toBe('B');
    expect(later.next).toBeNull();
    const early = activityContent(single('X', '07:00', '08:00'), new Date(at('07:10')), 'de', WORDS);
    expect(later.relevance).toBeGreaterThan(early.relevance);
  });
});

describe('reconcileIosActivities — starting and scheduling', () => {
  it('starts a running chain in front, and does nothing from the background', () => {
    const running = input({ now: new Date(at('10:00')), chains: [CROSSFADE] });
    expect(types(reconcileIosActivities(running).actions)).toEqual(['start']);
    expect(reconcileIosActivities({ ...running, foreground: false }).actions).toEqual([]);
  });

  it('schedules the next chains ahead on iOS 26, with the reminder it stands for', () => {
    const result = reconcileIosActivities(input({ chains: [CROSSFADE] }));
    expect(result.actions).toHaveLength(1);
    const action = result.actions[0];
    expect(action.type).toBe('schedule');
    if (action.type !== 'schedule') return;
    expect(action.at).toBe(at('09:50'));
    expect(action.signal.map((k) => k.item_id)).toEqual(['A']);
    // Covered only once the schedule has succeeded.
    expect(result.covered).toEqual([]);
  });

  it('schedules nothing below iOS 26, nor a silent reminder, nor more than the limit', () => {
    expect(reconcileIosActivities(input({ chains: [CROSSFADE], canSchedule: false })).actions).toEqual([]);
    expect(reconcileIosActivities(input({ chains: [CROSSFADE], isSilent: () => true })).actions).toEqual([]);
    const many = [single('A', '09:00', '10:00'), single('B', '11:00', '12:00'), single('C', '13:00', '14:00')];
    expect(types(reconcileIosActivities(input({ chains: many })).actions)).toEqual(
      Array.from({ length: MAX_PENDING }, () => 'schedule'),
    );
  });

  it('ends everything when switched off, and starts nothing', () => {
    const reported: ReportedActivity[] = [{ id: 'a1', chainKey: 'A', state: 'active' }];
    const result = reconcileIosActivities(
      input({ now: new Date(at('10:00')), chains: [CROSSFADE], reported, enabled: false }),
    );
    expect(result.actions).toEqual([{ type: 'end', id: 'a1' }]);
  });
});

describe('reconcileIosActivities — what already runs or waits', () => {
  // What a schedule at 09:50 sent: the content as of the start.
  const SENT = JSON.stringify(activityContent(CROSSFADE, new Date(at('09:50')), 'de', WORDS));
  const scheduled = (
    memory: Partial<ActivityMemory['activities'][string]> = {},
    id = 'p1',
  ): ActivityMemory => ({
    activities: {
      [id]: { keys: ['A', 'B'], shownKey: 'A', startsAt: at('09:50'), content: SENT, ...memory },
    },
    dismissed: {},
  });

  it('keeps a waiting activity that still fits, and only its reminder is covered', () => {
    const result = reconcileIosActivities(
      input({
        chains: [CROSSFADE],
        reported: [{ id: 'p1', chainKey: 'A', state: 'pending' }],
        memory: scheduled(),
      }),
    );
    expect(result.actions).toEqual([]);
    expect(result.covered.map((k) => k.item_id)).toEqual(['A']);
  });

  it('ends a waiting activity whose start moved, and plans it again only in front', () => {
    const moved = { ...CROSSFADE, start: at('09:40') };
    moved.sections = [{ ...CROSSFADE.sections[0], from: at('09:40') }, CROSSFADE.sections[1]];
    const args = input({
      chains: [moved],
      reported: [{ id: 'p1', chainKey: 'A', state: 'pending' }],
      memory: scheduled(),
    });
    expect(types(reconcileIosActivities(args).actions)).toEqual(['end', 'schedule']);
    const background = reconcileIosActivities({ ...args, foreground: false });
    expect(types(background.actions)).toEqual(['end']);
    // Nothing waits any more, so the reminder rings as a notification.
    expect(background.covered).toEqual([]);
  });

  it('finds a running activity by a later key once its first appointment is over', () => {
    const onlyB: LiveActivityChain = { ...CROSSFADE, keys: ['B'], sections: [CROSSFADE.sections[1]] };
    const result = reconcileIosActivities(
      input({
        now: new Date(at('11:10')),
        chains: [onlyB],
        reported: [{ id: 'a1', chainKey: 'A', state: 'active' }],
        memory: scheduled({ startsAt: at('09:50') }, 'a1'),
      }),
    );
    expect(types(result.actions)).toEqual(['update']);
    // The chain's keys as they are now, not every key it ever had.
    expect(result.memory.activities.a1.keys).toEqual(['B']);
  });

  it('never lets a running activity jump ahead to a chain that has not begun', () => {
    // B left the chain (moved to the afternoon) while A ran, and nothing
    // reconciled before A ended.
    const later: LiveActivityChain = {
      keys: ['B'],
      start: at('14:50'),
      end: at('16:00'),
      sections: [section('B', '14:50', '16:00', { start: '15:00', signal: true })],
    };
    const result = reconcileIosActivities(
      input({
        now: new Date(at('11:30')),
        chains: [later],
        reported: [{ id: 'a1', chainKey: 'A', state: 'active' }],
        memory: scheduled({ startsAt: at('09:50') }, 'a1'),
      }),
    );
    expect(types(result.actions)).toEqual(['end', 'schedule']);
  });

  it('ends a waiting activity whose reminder has become silent', () => {
    const result = reconcileIosActivities(
      input({
        chains: [CROSSFADE],
        reported: [{ id: 'p1', chainKey: 'A', state: 'pending' }],
        memory: scheduled(),
        isSilent: () => true,
      }),
    );
    expect(result.actions).toEqual([{ type: 'end', id: 'p1' }]);
    expect(result.covered).toEqual([]);
  });

  it('plans a waiting activity again when what it will show has changed', () => {
    const args = input({
      chains: [CROSSFADE],
      reported: [{ id: 'p1', chainKey: 'A', state: 'pending' }],
      memory: scheduled({ content: '{"title":"old"}' }),
    });
    expect(types(reconcileIosActivities(args).actions)).toEqual(['end', 'schedule']);
    // From the background it cannot be planned again: it stays, and covers.
    const background = reconcileIosActivities({ ...args, foreground: false });
    expect(background.actions).toEqual([]);
    expect(background.covered.map((k) => k.item_id)).toEqual(['A']);
  });

  it('keeps the earliest chains waiting when an earlier one appears', () => {
    const b = single('B', '11:00', '12:00');
    const c = single('C', '13:00', '14:00');
    const d = single('D', '15:00', '16:00');
    const sent = (chain: LiveActivityChain) =>
      JSON.stringify(activityContent(chain, new Date(chain.start), 'de', WORDS));
    const result = reconcileIosActivities(
      input({
        chains: [b, c, d],
        reported: [
          { id: 'pc', chainKey: 'C', state: 'pending' },
          { id: 'pd', chainKey: 'D', state: 'pending' },
        ],
        memory: {
          activities: {
            pc: { keys: ['C'], shownKey: 'C', startsAt: c.start, content: sent(c) },
            pd: { keys: ['D'], shownKey: 'D', startsAt: d.start, content: sent(d) },
          },
          dismissed: {},
        },
      }),
    );
    expect(result.actions.map((a) => (a.type === 'schedule' ? `schedule ${a.chainKey}` : `${a.type} ${'id' in a ? a.id : ''}`))).toEqual([
      'end pd',
      'schedule B',
    ]);
  });

  it('sends no update when nothing changed', () => {
    const now = new Date(at('10:00'));
    const content = JSON.stringify(activityContent(CROSSFADE, now, 'de', WORDS));
    const result = reconcileIosActivities(
      input({
        now,
        chains: [CROSSFADE],
        reported: [{ id: 'a1', chainKey: 'A', state: 'active' }],
        memory: scheduled({ content }, 'a1'),
      }),
    );
    expect(result.actions).toEqual([]);
  });

  it('ends an activity whose chain is gone, and a second one for the same chain', () => {
    const result = reconcileIosActivities(
      input({
        now: new Date(at('10:00')),
        chains: [CROSSFADE],
        reported: [
          { id: 'a1', chainKey: 'A', state: 'active' },
          { id: 'a2', chainKey: 'A', state: 'active' },
          { id: 'x', chainKey: 'X', state: 'active' },
        ],
      }),
    );
    expect(result.actions.filter((a) => a.type === 'end')).toEqual([
      { type: 'end', id: 'a2' },
      { type: 'end', id: 'x' },
    ]);
  });
});

describe('reconcileIosActivities — a swipe, inferred', () => {
  const started: ActivityMemory = {
    activities: { a1: { keys: ['A', 'B'], shownKey: 'A', startsAt: at('09:50'), content: '' } },
    dismissed: {},
    build: '68',
  };

  it('remembers the appointment swiped away and does not start it again', () => {
    const result = reconcileIosActivities(
      input({ now: new Date(at('10:10')), chains: [CROSSFADE], memory: started }),
    );
    expect(Object.keys(result.memory.dismissed)).toEqual(['A']);
    expect(result.actions).toEqual([]);
  });

  it('starts the next appointment of the chain once it shows', () => {
    const later = reconcileIosActivities(
      input({
        now: new Date(at('10:50')),
        chains: [CROSSFADE],
        memory: { activities: {}, dismissed: { A: at('10:10') } },
      }),
    );
    expect(types(later.actions)).toEqual(['start']);
  });

  it('is no swipe when Live Activities are off, as iOS then removes them all', () => {
    const result = reconcileIosActivities(
      input({ now: new Date(at('10:10')), chains: [CROSSFADE], memory: started, enabled: false }),
    );
    expect(result.memory.dismissed).toEqual({});
  });

  it('is no swipe after an update of the app, and the appointment starts again', () => {
    // Toni's iPhone, 2026-11-08: the update to build 67 took the running
    // activity away, the next pass read that as a swipe, and it stayed away.
    for (const memory of [{ ...started, build: '67' }, { ...started, build: undefined }]) {
      const result = reconcileIosActivities(
        input({ now: new Date(at('10:10')), chains: [CROSSFADE], memory }),
      );
      expect(result.memory.dismissed).toEqual({});
      expect(types(result.actions)).toEqual(['start']);
      expect(result.memory.build).toBe('68');
    }
  });

  it("is no swipe when iOS's eight hours ran out", () => {
    const result = reconcileIosActivities(
      input({ now: new Date('2026-11-09T18:00:00Z'), chains: [], memory: started }),
    );
    expect(result.memory.dismissed).toEqual({});
  });
});

describe('remember and triggerKeyId', () => {
  it('records a started activity under its id', () => {
    const result = reconcileIosActivities(input({ now: new Date(at('10:00')), chains: [CROSSFADE] }));
    const action = result.actions[0];
    if (action.type !== 'start') throw new Error('expected a start');
    const memory = remember(result.memory, 'a1', action, new Date(at('10:00')));
    expect(memory.activities.a1).toMatchObject({
      keys: ['A', 'B'],
      shownKey: 'A',
      startsAt: new Date(at('10:00')).toISOString(),
    });
  });

  it('reads one instant the same in both spellings', () => {
    const plan = { item_id: 'e', calendar_id: 'c', occurrence_start: '2026-11-09T10:00:00Z', fire_at: '2026-11-09T09:50:00Z' };
    const list = { ...plan, occurrence_start: '2026-11-09T10:00:00+00:00', fire_at: '2026-11-09T09:50:00.000+00:00' };
    expect(triggerKeyId(plan)).toBe(triggerKeyId(list));
  });
});

describe('reconcileIosActivities — when to ask iOS for a background round', () => {
  // Only a running Aperio ends an activity (Toni's iPhone, 2026-11-10: "Vorbei"
  // on the lock screen all evening). The wake is the earliest end of what keeps
  // or gets an activity.
  it('is the end of the running chain, and of a chain started now', () => {
    const running = input({
      now: new Date(at('10:00')),
      chains: [CROSSFADE],
      reported: [{ id: 'a1', chainKey: 'A', state: 'active' }],
    });
    expect(wakeAfter(reconcileIosActivities(running), '10:00')).toBe(at('11:30'));
    const started = input({ now: new Date(at('10:00')), chains: [CROSSFADE] });
    expect(wakeAfter(reconcileIosActivities(started), '10:00')).toBe(at('11:30'));
  });

  it('is the earliest end among a running and a waiting one', () => {
    const later = single('C', '13:00', '14:00');
    const result = reconcileIosActivities(
      input({
        now: new Date(at('10:00')),
        chains: [CROSSFADE, later],
        reported: [{ id: 'a1', chainKey: 'A', state: 'active' }],
      }),
    );
    expect(types(result.actions)).toEqual(['update', 'schedule']);
    expect(wakeAfter(result, '10:00')).toBe(at('11:30'));
  });

  it('is none once the chain is over and its activity ended, and none when off', () => {
    const over = reconcileIosActivities(
      input({
        now: new Date(at('12:00')),
        chains: [CROSSFADE],
        reported: [{ id: 'a1', chainKey: 'A', state: 'stale' }],
      }),
    );
    expect(over.actions).toEqual([{ type: 'end', id: 'a1' }]);
    expect(wakeAfter(over, '12:00')).toBeNull();
    const off = reconcileIosActivities(
      input({
        now: new Date(at('10:00')),
        chains: [CROSSFADE],
        reported: [{ id: 'a1', chainKey: 'A', state: 'active' }],
        enabled: false,
      }),
    );
    expect(wakeAfter(off, '10:00')).toBeNull();
  });

  it('is none for a chain that is only planned, never started, from the background', () => {
    const result = reconcileIosActivities(
      input({ now: new Date(at('10:00')), chains: [CROSSFADE], foreground: false }),
    );
    expect(result.actions).toEqual([]);
    expect(wakeAfter(result, '10:00')).toBeNull();
  });

  it('is never an end already past', () => {
    expect(nextWake({ a: at('09:00'), b: at('11:00'), c: at('10:30') }, new Date(at('10:00')))).toBe(
      at('10:30'),
    );
    expect(nextWake({ a: at('09:00') }, new Date(at('10:00')))).toBeNull();
  });
});

describe('activityPassSummary — one line per pass', () => {
  it('says nothing when there was nothing before and nothing was done', () => {
    expect(
      activityPassSummary({ origin: 'background', before: 0, done: [], failed: 0, wakeAt: null }),
    ).toBeNull();
  });

  it('counts what was done, a running activity kept included, with no title or id', () => {
    expect(
      activityPassSummary({ origin: 'background', before: 1, done: [], failed: 0, wakeAt: at('20:00') }),
    ).toBe(
      `live activities (background): before=1 ended=0 updated=0 started=0 scheduled=0 failed=0 wake=${at('20:00')}`,
    );
    expect(
      activityPassSummary({
        origin: 'foreground',
        before: 2,
        done: ['end', 'update', 'schedule'],
        failed: 1,
        wakeAt: null,
      }),
    ).toBe(
      'live activities (foreground): before=2 ended=1 updated=1 started=0 scheduled=1 failed=1 wake=none',
    );
  });
});

describe('reconcileIosActivities — each activity knows its end (Toni, 2026-11-17)', () => {
  const content = (chain: LiveActivityChain, now: string) =>
    JSON.stringify(activityContent(chain, new Date(at(now)), 'de', WORDS));

  it('keeps the end of the chain with every activity, and the native wake gets them', () => {
    const result = reconcileIosActivities(
      input({
        now: new Date(at('10:00')),
        chains: [CROSSFADE],
        reported: [{ id: 'a1', chainKey: 'A', state: 'active' }],
        memory: {
          activities: {
            a1: { keys: ['A', 'B'], shownKey: 'A', startsAt: at('09:50'), content: content(CROSSFADE, '10:00') },
          },
          dismissed: {},
        },
      }),
    );
    expect(result.memory.activities.a1.endsAt).toBe(at('11:30'));
    expect(activityEnds(result.memory)).toEqual({ a1: at('11:30') });
  });

  it('gives a started or scheduled activity the end of its chain', () => {
    const started = reconcileIosActivities(input({ now: new Date(at('10:00')), chains: [CROSSFADE] }));
    const action = started.actions[0];
    if (action.type !== 'start') throw new Error('expected a start');
    expect(action.endsAt).toBe(at('11:30'));
    expect(remember(started.memory, 'a1', action, new Date(at('10:00'))).activities.a1.endsAt).toBe(
      at('11:30'),
    );
    const planned = reconcileIosActivities(input({ chains: [CROSSFADE] }));
    expect(planned.actions[0]).toMatchObject({ type: 'schedule', endsAt: at('11:30') });
  });

  // The native wake ended it at 11:30; meanwhile the appointment was
  // extended, which the next pass in front finds.
  const extendedAfterTheEnd = (endedByWake: string[]) =>
    reconcileIosActivities(
      input({
        now: new Date(at('11:40')),
        chains: [
          {
            ...CROSSFADE,
            end: at('12:00'),
            sections: [
              CROSSFADE.sections[0],
              section('B', '10:45', '12:00', { start: '10:55', end: '12:00', signal: true }),
            ],
          },
        ],
        memory: {
          activities: {
            a1: { keys: ['A', 'B'], shownKey: 'B', startsAt: at('09:50'), content: '', endsAt: at('11:30') },
          },
          dismissed: {},
          build: '68',
        },
        endedByWake,
      }),
    );

  it('is no swipe when the native wake ended it, and starts again if the chain runs on', () => {
    const result = extendedAfterTheEnd(['a1']);
    expect(result.memory.dismissed).toEqual({});
    expect(types(result.actions)).toEqual(['start']);
  });

  it('is still a swipe when it is merely found gone after its end', () => {
    // Swiped at 11:10, extended since, and no pass in between: the swipe holds.
    const result = extendedAfterTheEnd([]);
    expect(Object.keys(result.memory.dismissed)).toEqual(['B']);
    expect(result.actions).toEqual([]);
  });

  it('leaves a waiting activity to the full pass when the pass runs from the last plan', () => {
    // Two earlier chains whose reminders are silent: the full pass leaves
    // them out and keeps the CROSSFADE waiting. A pass from the last plan has
    // no reminders, takes every reminder for loud, and used to end it.
    const x = single('X', '08:30', '08:40');
    const y = single('Y', '08:45', '08:55');
    const sent = JSON.stringify(activityContent(CROSSFADE, new Date(at('09:50')), 'de', WORDS));
    const result = reconcileIosActivities(
      input({
        chains: [x, y, CROSSFADE],
        reported: [{ id: 'p1', chainKey: 'A', state: 'pending' }],
        memory: {
          activities: { p1: { keys: ['A', 'B'], shownKey: 'A', startsAt: at('09:50'), content: sent } },
          dismissed: {},
        },
        planAhead: false,
        foreground: false,
      }),
    );
    expect(result.actions).toEqual([]);
    expect(result.memory.activities.p1.endsAt).toBe(at('11:30'));
  });

  it('still ends a waiting activity from the last plan when its chain is gone', () => {
    const result = reconcileIosActivities(
      input({
        chains: [],
        reported: [{ id: 'p1', chainKey: 'A', state: 'pending' }],
        memory: {
          activities: { p1: { keys: ['A', 'B'], shownKey: 'A', startsAt: at('09:50'), content: '' } },
          dismissed: {},
        },
        planAhead: false,
        foreground: false,
      }),
    );
    expect(result.actions).toEqual([{ type: 'end', id: 'p1' }]);
  });

  it('ends an activity whose chain is over from the last plan in the background', () => {
    const result = reconcileIosActivities(
      input({
        now: new Date(at('11:40')),
        chains: [CROSSFADE],
        reported: [{ id: 'a1', chainKey: 'A', state: 'stale' }],
        memory: {
          activities: {
            a1: { keys: ['A', 'B'], shownKey: 'B', startsAt: at('09:50'), content: '', endsAt: at('11:30') },
          },
          dismissed: {},
          build: '68',
        },
        planAhead: false,
        foreground: false,
      }),
    );
    expect(result.actions).toEqual([{ type: 'end', id: 'a1' }]);
    expect(wakeAfter(result, '11:40')).toBeNull();
  });
});
