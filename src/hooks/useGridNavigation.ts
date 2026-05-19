import { useCallback, useMemo, useState } from 'react';

/**
 * Two-level Outlook-style grid navigation.
 *
 * The calendar views from DESIGN.md section 3.3 share one navigation
 * model: the **arrow keys move between cells** (days, weeks, time slots
 * — the "space" axis), and **`Tab` moves between items inside the
 * focused cell** (events, tasks — the "content" axis). Keeping the two
 * axes separate is what stops users from changing day by accident while
 * tabbing through events.
 *
 * This hook is the primitive shared by the week, day, and month views.
 * Each view supplies its own `rowSize` (7 for a week, 1 for a day list,
 * varying for a month grid) and the hook hands back:
 *
 *  - `focusIndex` — current 1D cell index.
 *  - `setFocusIndex` — escape hatch (e.g. "jump to today").
 *  - `handleKeyDown` — wire this to the grid's `onKeyDown`.
 *
 * The hook intentionally does *not* manage Tab/Shift+Tab for the content
 * axis — Tab traversal between items inside a cell is a separate concern
 * and is left to the view (`role="option"` siblings handle their own Tab
 * order naturally).
 */
export interface GridNavigationOptions {
  /** Total number of cells in the grid. */
  itemCount: number;
  /** Number of cells per row (1 = pure list, 7 = a week, 31 = a month). */
  rowSize: number;
  /** Starting cell. Defaults to 0. */
  initialIndex?: number;
  /**
   * Called when navigation crosses a row boundary up or down — useful for
   * announcing "Previous week" / "Next month".
   */
  onCrossRow?: (direction: 'up' | 'down') => void;
}

export interface GridNavigationResult {
  focusIndex: number;
  setFocusIndex: (i: number) => void;
  handleKeyDown: (e: React.KeyboardEvent) => void;
}

export function useGridNavigation({
  itemCount,
  rowSize,
  initialIndex = 0,
  onCrossRow,
}: GridNavigationOptions): GridNavigationResult {
  const [focusIndex, setFocusIndex] = useState(() =>
    clamp(initialIndex, 0, Math.max(0, itemCount - 1)),
  );

  const move = useCallback(
    (delta: number, axis: 'horizontal' | 'vertical') => {
      setFocusIndex((current) => {
        const next = clamp(current + delta, 0, Math.max(0, itemCount - 1));
        if (axis === 'vertical' && next !== current && onCrossRow) {
          onCrossRow(delta > 0 ? 'down' : 'up');
        }
        return next;
      });
    },
    [itemCount, onCrossRow],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Never hijack keys when the user is typing in a form control.
      if (isEditableTarget(e.target)) return;

      switch (e.key) {
        case 'ArrowLeft':
          e.preventDefault();
          move(-1, 'horizontal');
          break;
        case 'ArrowRight':
          e.preventDefault();
          move(1, 'horizontal');
          break;
        case 'ArrowUp':
          e.preventDefault();
          move(-rowSize, 'vertical');
          break;
        case 'ArrowDown':
          e.preventDefault();
          move(rowSize, 'vertical');
          break;
        case 'Home':
          e.preventDefault();
          // Home: jump to the start of the current row.
          setFocusIndex((current) => current - (current % rowSize));
          break;
        case 'End':
          e.preventDefault();
          setFocusIndex((current) => {
            const lastInRow = current - (current % rowSize) + rowSize - 1;
            return Math.min(lastInRow, itemCount - 1);
          });
          break;
        case 'PageUp':
          e.preventDefault();
          move(-rowSize, 'vertical');
          break;
        case 'PageDown':
          e.preventDefault();
          move(rowSize, 'vertical');
          break;
        default:
          break;
      }
    },
    [itemCount, rowSize, move],
  );

  return useMemo(
    () => ({ focusIndex, setFocusIndex, handleKeyDown }),
    [focusIndex, handleKeyDown],
  );
}

function clamp(value: number, min: number, max: number): number {
  if (value < min) return min;
  if (value > max) return max;
  return value;
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  if (tag === 'input' || tag === 'textarea' || tag === 'select') return true;
  if (target.isContentEditable) return true;
  return false;
}
