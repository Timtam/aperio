import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render } from '@testing-library/react';

import { focusActiveView } from '../a11y/focusView';
import { useRegionFocus } from './useRegionFocus';

/**
 * F6, Ctrl+Tab and their Shift twins cycle the shell's regions (Toni,
 * 2026-10-31): the sidebar, the active view's button, the backlog's list, the
 * view's own widget — the backlog before the view though it sits inside it,
 * and never the check-in button in the view's header.
 */

function Shell({ sidebarInert = false }: { sidebarInert?: boolean }) {
  useRegionFocus();
  return (
    <>
      <nav data-region="sidebar" {...(sidebarInert ? { inert: '' } : {})}>
        <button type="button">Kalender</button>
      </nav>
      <div role="toolbar" data-region="toolbar">
        <button type="button" aria-pressed="false">
          Tag
        </button>
        <button type="button" aria-pressed="true" data-region-target>
          Woche
        </button>
      </div>
      <div data-region="view" data-active-view-root tabIndex={-1}>
        <header>
          <button type="button">Check-in</button>
        </header>
        <section data-region="backlog">
          <button type="button">Listen</button>
          <div role="listbox" aria-label="Backlog" tabIndex={0} data-region-target />
        </section>
        <div role="grid" aria-label="Wochenraster" tabIndex={0} data-region-target />
      </div>
      <div role="dialog" aria-label="Dialog">
        <button type="button">Im Dialog</button>
      </div>
    </>
  );
}

const name = () => {
  const el = document.activeElement as HTMLElement | null;
  return el?.getAttribute('aria-label') ?? el?.textContent ?? '';
};

afterEach(() => {
  document.body.innerHTML = '';
});

describe('the region cycle', () => {
  it('goes sidebar, active view, backlog, view, and round again with F6', () => {
    render(<Shell />);
    const seen: string[] = [];
    for (let i = 0; i < 5; i += 1) {
      fireEvent.keyDown(document.activeElement ?? document.body, { key: 'F6' });
      seen.push(name());
    }
    // The grid, not the check-in button in the view's header.
    expect(seen).toEqual(['Kalender', 'Woche', 'Backlog', 'Wochenraster', 'Kalender']);
  });

  it('goes the other way with Shift+F6', () => {
    render(<Shell />);
    const seen: string[] = [];
    for (let i = 0; i < 4; i += 1) {
      fireEvent.keyDown(document.activeElement ?? document.body, { key: 'F6', shiftKey: true });
      seen.push(name());
    }
    expect(seen).toEqual(['Wochenraster', 'Backlog', 'Woche', 'Kalender']);
  });

  it('does the same with Ctrl+Tab and Ctrl+Shift+Tab', () => {
    render(<Shell />);
    fireEvent.keyDown(document.body, { key: 'Tab', ctrlKey: true });
    expect(name()).toBe('Kalender');
    fireEvent.keyDown(document.activeElement!, { key: 'Tab', ctrlKey: true });
    expect(name()).toBe('Woche');
    fireEvent.keyDown(document.activeElement!, { key: 'Tab', ctrlKey: true, shiftKey: true });
    expect(name()).toBe('Kalender');
  });

  it('leaves Ctrl+Tab to a dialog', () => {
    render(<Shell />);
    const inside = document.querySelector<HTMLElement>('[role="dialog"] button')!;
    inside.focus();
    const event = new KeyboardEvent('keydown', { key: 'Tab', ctrlKey: true, bubbles: true, cancelable: true });
    inside.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(inside);
  });

  it('skips a region that is inert', () => {
    render(<Shell sidebarInert />);
    fireEvent.keyDown(document.body, { key: 'F6' });
    expect(name()).toBe('Woche');
  });

  it("returns into the view on its widget, not the header's button", () => {
    render(<Shell />);
    focusActiveView();
    expect(document.activeElement?.getAttribute('role')).toBe('grid');
  });
});
