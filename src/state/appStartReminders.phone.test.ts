import { describe, expect, it } from 'vitest';

/**
 * The phone's "on next app start" reminders (`mobile/src/reminders/appStart.ts`):
 * the phone offered this kind and never fired it. Now every opening rings what
 * the core says is due and remembers what it showed. The phone has no test
 * runner of its own, so its module runs here.
 */

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_APP_START = '../../mobile/src/reminders/appStart';

interface Reminder {
  item_id: string;
}
interface Deps {
  permitted: () => Promise<boolean>;
  due: () => Promise<Reminder[]>;
  show: (r: Reminder) => Promise<void>;
  mark: (shown: Reminder[]) => Promise<void>;
}
async function ring(deps: Deps): Promise<number> {
  const phone = (await import(/* @vite-ignore */ PHONE_APP_START)) as {
    ringAppStartReminders: (deps: Deps) => Promise<number>;
  };
  return phone.ringAppStartReminders(deps);
}

function fixture(due: Reminder[], opts: { permitted?: boolean; refuse?: string } = {}) {
  const shown: string[] = [];
  const marked: string[][] = [];
  const deps: Deps = {
    permitted: async () => opts.permitted ?? true,
    due: async () => due,
    show: async (r) => {
      if (r.item_id === opts.refuse) throw new Error('refused');
      shown.push(r.item_id);
    },
    mark: async (rs) => {
      marked.push(rs.map((r) => r.item_id));
    },
  };
  return { deps, shown, marked };
}

describe('ringAppStartReminders', () => {
  it('rings what is due and remembers it', async () => {
    const f = fixture([{ item_id: 'a' }, { item_id: 'b' }]);
    expect(await ring(f.deps)).toBe(2);
    expect(f.shown).toEqual(['a', 'b']);
    expect(f.marked).toEqual([['a', 'b']]);
  });

  it('remembers nothing without permission, so a later opening rings it', async () => {
    const f = fixture([{ item_id: 'a' }], { permitted: false });
    expect(await ring(f.deps)).toBe(0);
    expect(f.shown).toEqual([]);
    expect(f.marked).toEqual([]);
  });

  it('remembers only what was shown', async () => {
    const f = fixture([{ item_id: 'a' }, { item_id: 'b' }], { refuse: 'a' });
    expect(await ring(f.deps)).toBe(1);
    expect(f.marked).toEqual([['b']]);
  });

  it('marks nothing when nothing is due', async () => {
    const f = fixture([]);
    expect(await ring(f.deps)).toBe(0);
    expect(f.marked).toEqual([]);
  });
});
