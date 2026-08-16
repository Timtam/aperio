import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';

import { useBacklogLists } from './useBacklogLists';

const KEY = 'aperio.backlog.hiddenLists';

describe('useBacklogLists (the backlog-only list filter)', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('shows every list until one is hidden', () => {
    // It stores what is HIDDEN, not what is shown: a list created on another
    // device has to appear by default. A "these are shown" set would make it
    // silently invisible, which is the failure that loses work.
    const { result } = renderHook(() => useBacklogLists());
    expect(result.current.shows('brand-new-list')).toBe(true);
  });

  it('hides a list and remembers it', () => {
    const { result } = renderHook(() => useBacklogLists());
    act(() => result.current.setShown('household', false));
    expect(result.current.shows('household')).toBe(false);
    expect(JSON.parse(localStorage.getItem(KEY) ?? '[]')).toEqual(['household']);
  });

  it('brings one back, and brings them all back', () => {
    const { result } = renderHook(() => useBacklogLists());
    act(() => {
      result.current.setShown('a', false);
      result.current.setShown('b', false);
    });
    act(() => result.current.setShown('a', true));
    expect(result.current.shows('a')).toBe(true);
    expect(result.current.shows('b')).toBe(false);

    act(() => result.current.showAll());
    expect(result.current.shows('b')).toBe(true);
    expect(JSON.parse(localStorage.getItem(KEY) ?? '[]')).toEqual([]);
  });

  it('keeps two mounted rails in step', () => {
    // The dialog that edits the filter and the rail that renders it are
    // different trees; without the in-process notification the rail would keep
    // showing what the user just switched off.
    const rail = renderHook(() => useBacklogLists());
    const dialog = renderHook(() => useBacklogLists());
    act(() => dialog.result.current.setShown('household', false));
    expect(rail.result.current.shows('household')).toBe(false);
  });

  it('degrades to "show everything" on a corrupt blob', () => {
    // A display filter must never be the reason a backlog looks empty.
    localStorage.setItem(KEY, '{"not":"an array"}');
    const { result } = renderHook(() => useBacklogLists());
    expect(result.current.shows('anything')).toBe(true);

    localStorage.setItem(KEY, 'not json at all');
    const second = renderHook(() => useBacklogLists());
    expect(second.result.current.shows('anything')).toBe(true);
  });

  it('ignores non-string entries rather than trusting the blob', () => {
    localStorage.setItem(KEY, JSON.stringify(['real', 42, null]));
    const { result } = renderHook(() => useBacklogLists());
    expect(result.current.shows('real')).toBe(false);
    expect(result.current.hidden.size).toBe(1);
  });
});
