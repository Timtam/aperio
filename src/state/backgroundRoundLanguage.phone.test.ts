// @vitest-environment node
// Node, not jsdom: under jsdom Vite resolves every import of the module under
// test as it transforms it, and CI's frontend job has no mobile/node_modules,
// so `react-native` could not be found there even though it is mocked.
import { beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The phone's background round (`mobile/src/state/backgroundSync.ts`) words
 * the reminders, the widget and the Live Activities. A round in a background
 * launch runs without the app's screens (iOS starts React Native without a
 * window there, `modules/background-launch`; Android's headless runs never had
 * one), and only the screens applied the language the user chose. So the
 * round applies it first, and a failure to apply it costs nothing else. The
 * phone has no test runner of its own, so its module runs here.
 */

const state = vi.hoisted(() => ({
  calls: [] as string[],
  handler: undefined as undefined | (() => Promise<unknown>),
  applyFails: false,
}));

const phonePackages = vi.hoisted(() => ({
  reactNative: () => ({ Platform: { OS: 'ios' } }),
  asyncStorage: () => ({
    default: { getItem: async () => null, setItem: async () => {}, removeItem: async () => {} },
  }),
  backgroundTask: () => ({
    BackgroundTaskResult: { Success: 1, Failed: 2 },
    BackgroundTaskStatus: { Restricted: 1, Available: 2 },
  }),
  taskManager: () => ({
    defineTask: (_name: string, run: () => Promise<unknown>) => {
      state.handler = run;
    },
  }),
}));

// The phone's own packages, under both spellings. Where mobile/node_modules
// is installed (a developer's machine), the module under test resolves them
// there, which only the path reaches; where it is not (CI's frontend job),
// neither resolves and the bare name is what matches.
vi.mock('react-native', phonePackages.reactNative);
vi.mock('../../mobile/node_modules/react-native', phonePackages.reactNative);
vi.mock('@react-native-async-storage/async-storage', phonePackages.asyncStorage);
vi.mock(
  '../../mobile/node_modules/@react-native-async-storage/async-storage',
  phonePackages.asyncStorage,
);
vi.mock('expo-background-task', phonePackages.backgroundTask);
vi.mock('../../mobile/node_modules/expo-background-task', phonePackages.backgroundTask);
vi.mock('expo-task-manager', phonePackages.taskManager);
vi.mock('../../mobile/node_modules/expo-task-manager', phonePackages.taskManager);
vi.mock('../../mobile/modules/cal-ffi', () => ({ default: {} }));
vi.mock('../../mobile/src/api/prefs', () => ({ getUserPref: async () => null }));
vi.mock('../../mobile/src/settings/language', () => ({
  readLanguageChoice: async () => {
    state.calls.push('read language');
    return 'en';
  },
  applyLanguageChoice: async (choice: string) => {
    if (state.applyFails) throw new Error('changeLanguage failed');
    state.calls.push(`apply ${choice}`);
  },
}));
vi.mock('../../mobile/src/api/sync', () => ({
  // A failed status read ends the provider wait at once.
  cacheRefreshStatus: () => Promise.reject(new Error('no status')),
  refreshExternalCacheInBackground: async () => {
    state.calls.push('refresh providers');
  },
  syncNow: async () => ({ applied: 0 }),
  syncStatus: async () => ({ configured: false }),
}));
vi.mock('../../mobile/src/api/logs', () => ({ logLine: async () => undefined }));
vi.mock('../../mobile/src/liveActivities', () => ({
  endFromLastPlan: async () => {
    state.calls.push('end live activities');
  },
}));
vi.mock('../../mobile/src/reminders/scheduler', () => ({
  duringBackgroundRound: (run: () => Promise<unknown>) => run(),
  markExternalCachesSettled: () => undefined,
  rescheduleReminders: async () => {
    state.calls.push('reschedule reminders');
  },
}));
vi.mock('../../mobile/src/state/widgetSnapshot', () => ({
  refreshWidgetSnapshot: async () => {
    state.calls.push('write widget');
  },
}));

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_BACKGROUND_SYNC = '../../mobile/src/state/backgroundSync';

/** The round the module defines as its background task. */
async function backgroundRound(): Promise<() => Promise<unknown>> {
  await import(/* @vite-ignore */ PHONE_BACKGROUND_SYNC);
  if (state.handler == null) throw new Error('no background task defined');
  return state.handler;
}

beforeEach(() => {
  state.calls = [];
  state.applyFails = false;
});

describe('the background round', () => {
  it('applies the stored language before it words reminders, the widget or Live Activities', async () => {
    const round = await backgroundRound();
    await round();
    expect(state.calls).toEqual([
      'read language',
      'apply en',
      'end live activities',
      'refresh providers',
      'reschedule reminders',
      'write widget',
    ]);
  });

  it('still runs when the language cannot be applied', async () => {
    state.applyFails = true;
    const round = await backgroundRound();
    await expect(round()).resolves.toBe(1);
    expect(state.calls).toEqual([
      'read language',
      'end live activities',
      'refresh providers',
      'reschedule reminders',
      'write widget',
    ]);
  });
});
