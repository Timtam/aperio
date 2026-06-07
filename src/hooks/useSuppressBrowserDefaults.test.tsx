import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { useSuppressBrowserDefaults } from './useSuppressBrowserDefaults';

function Harness({ children }: { children?: React.ReactNode }) {
  useSuppressBrowserDefaults();
  return <div>{children}</div>;
}

describe('useSuppressBrowserDefaults', () => {
  it('blocks F5 outside editable targets', () => {
    render(
      <Harness>
        <button type="button">x</button>
      </Harness>,
    );
    const ev = new KeyboardEvent('keydown', {
      key: 'F5',
      bubbles: true,
      cancelable: true,
    });
    const defaultPrevented = !window.dispatchEvent(ev);
    expect(defaultPrevented).toBe(true);
  });

  it('does not block keys inside <input>', () => {
    render(
      <Harness>
        <input aria-label="search" />
      </Harness>,
    );
    const input = screen.getByRole('textbox', { name: 'search' });
    input.focus();
    const ev = new KeyboardEvent('keydown', {
      key: 'F5',
      bubbles: true,
      cancelable: true,
    });
    // Dispatch on the input so the event target is the editable element.
    input.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
  });

  it('blocks contextmenu by default', () => {
    render(
      <Harness>
        <button type="button">x</button>
      </Harness>,
    );
    const ev = new MouseEvent('contextmenu', {
      bubbles: true,
      cancelable: true,
    });
    const defaultPrevented = !window.dispatchEvent(ev);
    expect(defaultPrevented).toBe(true);
  });

  it('allows drag from elements marked data-drag-source', () => {
    render(
      <Harness>
        <div data-drag-source data-testid="src">
          item
        </div>
        <div data-testid="not-src">other</div>
      </Harness>,
    );
    const src = screen.getByTestId('src');
    const notSrc = screen.getByTestId('not-src');

    const allowed = new Event('dragstart', { bubbles: true, cancelable: true });
    src.dispatchEvent(allowed);
    expect(allowed.defaultPrevented).toBe(false);

    const blocked = new Event('dragstart', { bubbles: true, cancelable: true });
    notSrc.dispatchEvent(blocked);
    expect(blocked.defaultPrevented).toBe(true);
  });

  it('allows drag from explicitly draggable elements', () => {
    // The app's chips (backlog tasks, event/task chips, sidebar rows) opt in
    // via the native `draggable` attribute — the suppressor must let those
    // start a drag, otherwise schedule-by-drag is dead app-wide.
    render(
      <Harness>
        <div draggable data-testid="drag">
          item
        </div>
      </Harness>,
    );
    const el = screen.getByTestId('drag');
    const ev = new Event('dragstart', { bubbles: true, cancelable: true });
    el.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
  });

  it('blocks Ctrl+R', () => {
    render(<Harness />);
    const ev = new KeyboardEvent('keydown', {
      key: 'r',
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    const defaultPrevented = !window.dispatchEvent(ev);
    expect(defaultPrevented).toBe(true);
  });
});
