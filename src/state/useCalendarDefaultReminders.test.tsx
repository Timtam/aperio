import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../api/client', () => ({
  getUserPref: vi.fn(() => Promise.resolve(null)),
  setUserPref: vi.fn(() => Promise.resolve()),
  invalidateReminders: vi.fn(() => Promise.resolve()),
}));

import { getUserPref, setUserPref } from '../api/client';
import {
  useCalendarDefaultReminders,
  type CalendarDefaultReminders,
} from './useCalendarDefaultReminders';

const IDS = ['work', 'home'];

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

describe('useCalendarDefaultReminders — where the defaults live', () => {
  it('reads each calendar\'s mode beside its defaults and is local until chosen', async () => {
    vi.mocked(getUserPref).mockImplementation((key: string) =>
      Promise.resolve(key === 'calendar.work.defaultRemindersMode' ? 'attach' : null),
    );
    const api = await mount();
    expect(api().hydrating).toBe(false);
    expect(api().getModeFor('work')).toBe('attach');
    expect(api().getModeFor('home')).toBe('local');
    // One read per calendar for the list, one for the mode.
    expect(vi.mocked(getUserPref)).toHaveBeenCalledWith('calendar.home.defaultRemindersMode');
    expect(vi.mocked(getUserPref)).toHaveBeenCalledWith('calendar.home.defaultReminders');
  });

  it('only the exact attach marker attaches', async () => {
    vi.mocked(getUserPref).mockImplementation((key: string) =>
      Promise.resolve(key === 'calendar.work.defaultRemindersMode' ? 'Attach' : null),
    );
    const api = await mount();
    expect(api().getModeFor('work')).toBe('local');
  });

  it('writes the choice at once, under the synced per-calendar key', async () => {
    const api = await mount();
    act(() => {
      api().setModeFor('home', 'attach');
    });
    expect(api().getModeFor('home')).toBe('attach');
    expect(vi.mocked(setUserPref)).toHaveBeenCalledWith(
      'calendar.home.defaultRemindersMode',
      'attach',
    );
    // Back to local is a stored answer too, not a deletion.
    act(() => {
      api().setModeFor('home', 'local');
    });
    expect(api().getModeFor('home')).toBe('local');
    expect(vi.mocked(setUserPref)).toHaveBeenLastCalledWith(
      'calendar.home.defaultRemindersMode',
      'local',
    );
  });
});
