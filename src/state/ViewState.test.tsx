import { describe, expect, it, beforeEach } from 'vitest';
import { act, render, screen } from '@testing-library/react';

import { ViewStateProvider, useViewShortcuts, useViewState } from './ViewState';

function Probe() {
  useViewShortcuts();
  const v = useViewState();
  return (
    <div>
      <span data-testid="view">{v.view}</span>
      <span data-testid="anchor">{v.anchor.toISOString()}</span>
      <button type="button" onClick={() => v.setView('month')}>
        set-month
      </button>
    </div>
  );
}

beforeEach(() => {
  localStorage.clear();
});

describe('ViewStateProvider', () => {
  it('defaults to the week view on first run', () => {
    render(
      <ViewStateProvider>
        <Probe />
      </ViewStateProvider>,
    );
    expect(screen.getByTestId('view').textContent).toBe('week');
  });

  it('reads a persisted view from localStorage', () => {
    localStorage.setItem(
      'aperio.view.v1',
      JSON.stringify({ view: 'agenda', anchor: '2026-01-15T00:00:00Z' }),
    );
    render(
      <ViewStateProvider>
        <Probe />
      </ViewStateProvider>,
    );
    expect(screen.getByTestId('view').textContent).toBe('agenda');
    expect(screen.getByTestId('anchor').textContent).toContain('2026-01-15');
  });

  it('ignores an invalid persisted view', () => {
    localStorage.setItem('aperio.view.v1', JSON.stringify({ view: 'galaxy' }));
    render(
      <ViewStateProvider>
        <Probe />
      </ViewStateProvider>,
    );
    expect(screen.getByTestId('view').textContent).toBe('week');
  });
});

describe('useViewShortcuts', () => {
  function dispatch(key: string, opts: KeyboardEventInit = {}) {
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent('keydown', {
          key,
          ctrlKey: true,
          bubbles: true,
          cancelable: true,
          ...opts,
        }),
      );
    });
  }

  it('Ctrl+3 switches to month view', () => {
    render(
      <ViewStateProvider>
        <Probe />
      </ViewStateProvider>,
    );
    dispatch('3');
    expect(screen.getByTestId('view').textContent).toBe('month');
  });

  it('Ctrl+T jumps to today', () => {
    localStorage.setItem(
      'aperio.view.v1',
      JSON.stringify({ view: 'week', anchor: '2000-01-01T00:00:00Z' }),
    );
    render(
      <ViewStateProvider>
        <Probe />
      </ViewStateProvider>,
    );
    expect(screen.getByTestId('anchor').textContent).toContain('2000-01-01');
    dispatch('t');
    expect(screen.getByTestId('anchor').textContent).not.toContain('2000-01-01');
  });

  it('does not fire shortcuts inside form controls', () => {
    render(
      <ViewStateProvider>
        <Probe />
      </ViewStateProvider>,
    );
    const input = document.createElement('input');
    document.body.appendChild(input);
    input.focus();
    act(() => {
      input.dispatchEvent(
        new KeyboardEvent('keydown', {
          key: '3',
          ctrlKey: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    expect(screen.getByTestId('view').textContent).toBe('week');
    input.remove();
  });

  it('starts with no calendar focus', () => {
    render(
      <ViewStateProvider>
        <FocusProbe />
      </ViewStateProvider>,
    );
    expect(screen.getByTestId('focused').textContent).toBe('none');
  });

  it('enterFocus / exitFocus toggle the focused calendar', () => {
    render(
      <ViewStateProvider>
        <FocusProbe />
      </ViewStateProvider>,
    );
    act(() => {
      screen.getByTestId('enter').click();
    });
    expect(screen.getByTestId('focused').textContent).toBe('cal-42');
    act(() => {
      screen.getByTestId('exit').click();
    });
    expect(screen.getByTestId('focused').textContent).toBe('none');
  });

  it('persists the focused calendar across remounts', () => {
    const { unmount } = render(
      <ViewStateProvider>
        <FocusProbe />
      </ViewStateProvider>,
    );
    act(() => {
      screen.getByTestId('enter').click();
    });
    unmount();
    render(
      <ViewStateProvider>
        <FocusProbe />
      </ViewStateProvider>,
    );
    expect(screen.getByTestId('focused').textContent).toBe('cal-42');
  });
});

function FocusProbe() {
  const v = useViewState();
  return (
    <div>
      <span data-testid="focused">{v.focusedCalendarId ?? 'none'}</span>
      <button
        type="button"
        data-testid="enter"
        onClick={() => v.enterFocus('cal-42')}
      >
        enter
      </button>
      <button type="button" data-testid="exit" onClick={() => v.exitFocus()}>
        exit
      </button>
    </div>
  );
}
