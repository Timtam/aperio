import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { useGridNavigation } from './useGridNavigation';

/**
 * Test harness: renders a 7-cell row by default (one week) and exposes the
 * current focus index in the DOM. We assert against `data-focus-index`
 * rather than poking the hook directly, because the contract we care
 * about is "the right cell is highlighted after the right key".
 */
function Harness({
  itemCount = 14,
  rowSize = 7,
  initialIndex = 0,
  onCrossRow,
}: Partial<Parameters<typeof useGridNavigation>[0]> = {}) {
  const grid = useGridNavigation({
    itemCount,
    rowSize,
    initialIndex,
    onCrossRow,
  });
  return (
    <div
      role="grid"
      tabIndex={0}
      onKeyDown={grid.handleKeyDown}
      data-focus-index={grid.focusIndex}
    />
  );
}

function focusIndex() {
  return Number(screen.getByRole('grid').getAttribute('data-focus-index'));
}

function fireKey(key: string) {
  fireEvent.keyDown(screen.getByRole('grid'), { key });
}

describe('useGridNavigation', () => {
  it('starts at initialIndex', () => {
    render(<Harness initialIndex={3} />);
    expect(focusIndex()).toBe(3);
  });

  it('Arrow keys move within a row', () => {
    render(<Harness initialIndex={3} />);
    fireKey('ArrowRight');
    expect(focusIndex()).toBe(4);
    fireKey('ArrowLeft');
    fireKey('ArrowLeft');
    expect(focusIndex()).toBe(2);
  });

  it('Arrow Up/Down crosses rows by rowSize', () => {
    render(<Harness initialIndex={3} />);
    fireKey('ArrowDown');
    expect(focusIndex()).toBe(10);
    fireKey('ArrowUp');
    expect(focusIndex()).toBe(3);
  });

  it('clamps at the left boundary', () => {
    render(<Harness initialIndex={0} />);
    fireKey('ArrowLeft');
    expect(focusIndex()).toBe(0);
  });

  it('clamps at the right boundary', () => {
    render(<Harness initialIndex={13} itemCount={14} />);
    expect(focusIndex()).toBe(13);
    fireKey('ArrowRight');
    expect(focusIndex()).toBe(13);
  });

  it('Home jumps to start of current row, End to last in row', () => {
    render(<Harness initialIndex={10} itemCount={14} />);
    fireKey('Home');
    expect(focusIndex()).toBe(7);
    fireKey('End');
    expect(focusIndex()).toBe(13);
  });

  it('End is capped by itemCount when the last row is partial', () => {
    render(<Harness itemCount={10} rowSize={7} initialIndex={8} />);
    fireKey('End');
    // Row [7,8,9] — last valid item is 9.
    expect(focusIndex()).toBe(9);
  });

  it('fires onCrossRow with direction when crossing rows', () => {
    const spy = vi.fn();
    render(<Harness initialIndex={3} onCrossRow={spy} />);
    fireKey('ArrowDown');
    expect(spy).toHaveBeenCalledWith('down');
    fireKey('ArrowUp');
    expect(spy).toHaveBeenCalledWith('up');
  });

  it('does not fire onCrossRow on horizontal moves', () => {
    const spy = vi.fn();
    render(<Harness initialIndex={3} onCrossRow={spy} />);
    fireKey('ArrowRight');
    fireKey('ArrowLeft');
    expect(spy).not.toHaveBeenCalled();
  });

  it('ignores key presses inside form controls', () => {
    function Combined() {
      const grid = useGridNavigation({ itemCount: 14, rowSize: 7, initialIndex: 3 });
      return (
        <div
          role="grid"
          onKeyDown={grid.handleKeyDown}
          data-focus-index={grid.focusIndex}
        >
          <input aria-label="search" />
        </div>
      );
    }
    render(<Combined />);
    const input = screen.getByRole('textbox', { name: 'search' });
    input.focus();
    fireEvent.keyDown(input, { key: 'ArrowRight' });
    expect(focusIndex()).toBe(3);
  });
});
