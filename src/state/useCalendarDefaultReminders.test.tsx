import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../api/client', () => ({
  getUserPref: vi.fn(() => Promise.resolve(null)),
  setUserPref: vi.fn(() => Promise.resolve()),
  invalidateReminders: vi.fn(() => Promise.resolve()),
}));

/** Captures the `user-prefs-changed` listener so a test can play the event a
 *  sync round emits when the OTHER device changed a calendar's defaults. */
let prefsChanged: ((event: { payload: string[] }) => void) | null = null;
vi.mock('@tauri-apps/api/event', () => ({
  listen: (name: string, handler: (event: { payload: string[] }) => void) => {
    if (name === 'user-prefs-changed') prefsChanged = handler;
    return Promise.resolve(() => {
      prefsChanged = null;
    });
  },
}));

import type { DefaultReminder } from '@aperio/shared';

import { getUserPref, setUserPref } from '../api/client';
import {
  useCalendarDefaultReminders,
  type CalendarDefaultReminders,
} from './useCalendarDefaultReminders';

const IDS = ['work'];
const HOUR: DefaultReminder = {
  kind: { type: 'relative', minutes_before: 60 },
  sound: null,
  attach: true,
};
const DAY: DefaultReminder = {
  kind: { type: 'relative', minutes_before: 1440 },
  sound: null,
};

/** Hands the latest hook result to the test through `onRender`. */
function Probe({ onRender }: { onRender: (api: CalendarDefaultReminders) => void }) {
  onRender(useCalendarDefaultReminders(IDS));
  return null;
}

async function mount(): Promise<() => CalendarDefaultReminders> {
  let latest: CalendarDefaultReminders | null = null;
  await act(async () => {
    render(
      <Probe
        onRender={(api) => {
          latest = api;
        }}
      />,
    );
  });
  return () => {
    if (!latest) throw new Error('hook never rendered');
    return latest;
  };
}

beforeEach(() => {
  vi.mocked(getUserPref).mockReset();
  vi.mocked(getUserPref).mockImplementation(() => Promise.resolve(null));
  vi.mocked(setUserPref).mockClear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useCalendarDefaultReminders — where each default lives', () => {
  it('keeps each entry\'s placement flag through hydration', async () => {
    vi.mocked(getUserPref).mockImplementation((key: string) =>
      Promise.resolve(
        key === 'calendar.work.defaultReminders' ? JSON.stringify([HOUR, DAY]) : null,
      ),
    );
    const api = await mount();
    expect(api().hydrating).toBe(false);
    const [hour, day] = api().getDefaultsFor('work');
    expect(hour.attach).toBe(true);
    // An entry stored before the choice existed carries no flag: in Aperio.
    expect(day.attach).toBeUndefined();
  });

  it('re-reads when a sync round wrote this calendar\'s defaults', async () => {
    const api = await mount();
    expect(api().getDefaultsFor('work')).toEqual([]);
    // The other device attached an hour-before default; the round announces it.
    vi.mocked(getUserPref).mockImplementation((key: string) =>
      Promise.resolve(
        key === 'calendar.work.defaultReminders' ? JSON.stringify([HOUR]) : null,
      ),
    );
    expect(prefsChanged).not.toBeNull();
    await act(async () => {
      prefsChanged?.({ payload: ['calendar.work.defaultReminders'] });
    });
    expect(api().getDefaultsFor('work')).toEqual([HOUR]);
  });

  it('ignores a round that touched somebody else\'s keys', async () => {
    const api = await mount();
    const readsAfterMount = vi.mocked(getUserPref).mock.calls.length;
    await act(async () => {
      prefsChanged?.({ payload: ['locale', 'calendar.other.defaultReminders'] });
    });
    expect(vi.mocked(getUserPref).mock.calls.length).toBe(readsAfterMount);
    expect(api().getDefaultsFor('work')).toEqual([]);
  });

  it('writes the placement flag inside the same synced list', async () => {
    vi.useFakeTimers();
    const api = await mount();
    act(() => {
      api().setDefaultsFor('work', [HOUR, DAY]);
    });
    expect(api().getDefaultsFor('work')).toEqual([HOUR, DAY]);
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(vi.mocked(setUserPref)).toHaveBeenCalledWith(
      'calendar.work.defaultReminders',
      JSON.stringify([HOUR, DAY]),
    );
  });
});
