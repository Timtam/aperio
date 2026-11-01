import { beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The phone's two week prefs (`mobile/src/settings/weekNumbers.ts`,
 * `weekStart.ts`) keep the last value read or written, and the app reads
 * both at launch: the week screen then opens on the stored choice instead of
 * correcting its heading a moment later, which the VoiceOver pager would
 * announce as if the user had paged (TODO B15). The phone has no test runner
 * of its own, so its modules run here.
 */

const h = vi.hoisted(() => ({ prefs: new Map<string, string>() }));

vi.mock('../../mobile/src/api/prefs', () => ({
  getUserPref: (key: string) => Promise.resolve(h.prefs.get(key) ?? null),
  setUserPref: (key: string, value: string) => {
    h.prefs.set(key, value);
    return Promise.resolve();
  },
}));

/** Through paths the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_WEEK_NUMBERS = '../../mobile/src/settings/weekNumbers';
const PHONE_WEEK_START = '../../mobile/src/settings/weekStart';

interface WeekNumbersModule {
  peekWeekNumbers(): boolean;
  readWeekNumbers(): Promise<boolean>;
  writeWeekNumbers(value: boolean): Promise<void>;
}
interface WeekStartModule {
  peekWeekStart(): number;
  readWeekStart(): Promise<number>;
  writeWeekStart(value: number): Promise<void>;
}

async function fresh() {
  vi.resetModules();
  const numbers = (await import(/* @vite-ignore */ PHONE_WEEK_NUMBERS)) as WeekNumbersModule;
  const start = (await import(/* @vite-ignore */ PHONE_WEEK_START)) as WeekStartModule;
  return { numbers, start };
}

beforeEach(() => {
  h.prefs.clear();
});

describe("the phone's week prefs", () => {
  it('start on the defaults before anything is read', async () => {
    const { numbers, start } = await fresh();
    expect(numbers.peekWeekNumbers()).toBe(true);
    expect(start.peekWeekStart()).toBe(1);
  });

  it('remember what was read', async () => {
    h.prefs.set('view.weekNumbers', 'false');
    h.prefs.set('view.weekStart', '0');
    const { numbers, start } = await fresh();
    expect(await numbers.readWeekNumbers()).toBe(false);
    expect(await start.readWeekStart()).toBe(0);
    expect(numbers.peekWeekNumbers()).toBe(false);
    expect(start.peekWeekStart()).toBe(0);
  });

  it('remember what was written, in the synced keys', async () => {
    const { numbers, start } = await fresh();
    await numbers.writeWeekNumbers(false);
    await start.writeWeekStart(6);
    expect(numbers.peekWeekNumbers()).toBe(false);
    expect(start.peekWeekStart()).toBe(6);
    expect(h.prefs.get('view.weekNumbers')).toBe('false');
    expect(h.prefs.get('view.weekStart')).toBe('6');
  });
});
