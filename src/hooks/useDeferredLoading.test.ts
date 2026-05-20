/// <reference types="vitest" />
import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useDeferredLoading } from './useDeferredLoading';

describe('useDeferredLoading', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('starts false', () => {
    const { result } = renderHook(() => useDeferredLoading(true, 200));
    // Initial render: timer scheduled but not yet fired.
    expect(result.current).toBe(false);
  });

  it('flips true after the delay elapses while still loading', () => {
    const { result } = renderHook(() => useDeferredLoading(true, 200));
    act(() => {
      vi.advanceTimersByTime(199);
    });
    expect(result.current).toBe(false);
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(result.current).toBe(true);
  });

  it('stays false when loading clears within the delay window', () => {
    const { result, rerender } = renderHook(
      ({ loading }) => useDeferredLoading(loading, 200),
      { initialProps: { loading: true } },
    );
    act(() => {
      vi.advanceTimersByTime(100);
    });
    rerender({ loading: false });
    act(() => {
      // Fast-forward well past the original delay — the timer was
      // cleared on the rerender so nothing should fire.
      vi.advanceTimersByTime(500);
    });
    expect(result.current).toBe(false);
  });

  it('returns false once loading drops back to false', () => {
    const { result, rerender } = renderHook(
      ({ loading }) => useDeferredLoading(loading, 200),
      { initialProps: { loading: true } },
    );
    act(() => {
      vi.advanceTimersByTime(300);
    });
    expect(result.current).toBe(true);
    rerender({ loading: false });
    expect(result.current).toBe(false);
  });

  it('starts a fresh delay window when loading toggles true again', () => {
    const { result, rerender } = renderHook(
      ({ loading }) => useDeferredLoading(loading, 200),
      { initialProps: { loading: true } },
    );
    act(() => {
      vi.advanceTimersByTime(50);
    });
    rerender({ loading: false });
    rerender({ loading: true });
    act(() => {
      // Only 100 ms into the new window — still false.
      vi.advanceTimersByTime(100);
    });
    expect(result.current).toBe(false);
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(result.current).toBe(true);
  });
});
