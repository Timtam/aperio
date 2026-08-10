import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../api/client', () => ({
  setUserPref: vi.fn(() => Promise.resolve()),
}));

import { setUserPref } from '../api/client';
import { useDebouncedPrefWrite } from './useDebouncedPrefWrite';

const KEY = 'tasks.example';
const DEBOUNCE = 150;

function Probe({
  value,
  hydrating,
  revision = 0,
}: {
  value: string;
  hydrating: boolean;
  revision?: number;
}) {
  useDebouncedPrefWrite(KEY, value, hydrating, DEBOUNCE, revision);
  return null;
}

/** Let the debounce elapse. */
function settle() {
  act(() => {
    vi.advanceTimersByTime(DEBOUNCE + 10);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(setUserPref).mockClear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useDebouncedPrefWrite', () => {
  it('writes nothing when hydration produced the value it is holding', () => {
    // THE bug this hook exists for. The old write-back fired the moment
    // hydration finished, stamping a fresh sync timestamp on an old choice —
    // enough to overrule a genuine change another device made while this one
    // was closed.
    const { rerender } = render(<Probe value="true" hydrating />);
    rerender(<Probe value="true" hydrating={false} />);
    settle();
    expect(setUserPref).not.toHaveBeenCalled();
  });

  it('writes a real change, once, after the debounce', () => {
    const { rerender } = render(<Probe value="true" hydrating />);
    rerender(<Probe value="true" hydrating={false} />);
    settle();
    rerender(<Probe value="false" hydrating={false} />);
    expect(setUserPref).not.toHaveBeenCalled(); // still inside the quiet period
    settle();
    expect(setUserPref).toHaveBeenCalledTimes(1);
    expect(setUserPref).toHaveBeenCalledWith(KEY, 'false');
  });

  it('collapses a flurry of changes into the last value', () => {
    const { rerender } = render(<Probe value="a" hydrating />);
    rerender(<Probe value="a" hydrating={false} />);
    settle();
    rerender(<Probe value="b" hydrating={false} />);
    rerender(<Probe value="c" hydrating={false} />);
    rerender(<Probe value="d" hydrating={false} />);
    settle();
    expect(setUserPref).toHaveBeenCalledTimes(1);
    expect(setUserPref).toHaveBeenCalledWith(KEY, 'd');
  });

  it('says nothing when a change is undone before the debounce elapses', () => {
    const { rerender } = render(<Probe value="a" hydrating />);
    rerender(<Probe value="a" hydrating={false} />);
    settle();
    rerender(<Probe value="b" hydrating={false} />);
    rerender(<Probe value="a" hydrating={false} />);
    settle();
    expect(setUserPref).not.toHaveBeenCalled();
  });

  it('writes again when the value comes BACK to what was stored', () => {
    // The baseline follows what we wrote, not what we read at startup —
    // otherwise turning a setting off and on again would leave storage on
    // "off" while the app shows "on".
    const { rerender } = render(<Probe value="a" hydrating />);
    rerender(<Probe value="a" hydrating={false} />);
    settle();
    rerender(<Probe value="b" hydrating={false} />);
    settle();
    rerender(<Probe value="a" hydrating={false} />);
    settle();
    expect(setUserPref).toHaveBeenCalledTimes(2);
    expect(vi.mocked(setUserPref).mock.calls.map((c) => c[1])).toEqual(['b', 'a']);
  });

  it('does not write while hydration is still running', () => {
    const { rerender } = render(<Probe value="a" hydrating />);
    rerender(<Probe value="b" hydrating />);
    settle();
    expect(setUserPref).not.toHaveBeenCalled();
  });
});

describe('useDebouncedPrefWrite with a value from another device', () => {
  it('adopts a re-read value without writing it back', () => {
    // Otherwise every device restates every change it receives, with its own
    // newer timestamp — the launch echo again, one round later.
    const { rerender } = render(<Probe value="a" hydrating />);
    rerender(<Probe value="a" hydrating={false} />);
    settle();
    rerender(<Probe value="b" hydrating={false} revision={1} />);
    settle();
    expect(setUserPref).not.toHaveBeenCalled();
  });

  it('writes the next edit against the NEW baseline', () => {
    const { rerender } = render(<Probe value="a" hydrating />);
    rerender(<Probe value="a" hydrating={false} />);
    settle();
    rerender(<Probe value="b" hydrating={false} revision={1} />);
    settle();
    // Back to what this device had before the sync: a real change now.
    rerender(<Probe value="a" hydrating={false} revision={1} />);
    settle();
    expect(setUserPref).toHaveBeenCalledTimes(1);
    expect(setUserPref).toHaveBeenCalledWith(KEY, 'a');
  });

  it('drops a write still in the quiet period when a re-read overtakes it', () => {
    // The stored value is newer than what the user was in the middle of
    // saying; re-stating theirs would undo a change that already won.
    const { rerender } = render(<Probe value="a" hydrating />);
    rerender(<Probe value="a" hydrating={false} />);
    settle();
    rerender(<Probe value="b" hydrating={false} />);
    rerender(<Probe value="c" hydrating={false} revision={1} />);
    settle();
    expect(setUserPref).not.toHaveBeenCalled();
  });
});
