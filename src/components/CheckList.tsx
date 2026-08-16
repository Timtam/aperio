import { useCallback, useEffect, useRef, useState } from 'react';

/**
 * A list of checkboxes that costs ONE tab stop.
 *
 * A stop per row is the default a plain `<ul>` of checkboxes gives you, and it
 * is the wrong default the moment the list is more than a handful: ten rows
 * mean ten presses to walk past something the user did not want to touch. A
 * roving tabindex fixes that — only the active box is in the tab order, the
 * arrow keys move within the list, Home and End jump to the ends.
 *
 * Real `<input type="checkbox">` elements with real focus, deliberately, rather
 * than a multi-selectable listbox of options: the announcement stays "checkbox,
 * checked" instead of "selected", Space keeps its native meaning, and nothing
 * rests on how well a given screen reader handles `aria-checked` on an option.
 *
 * A symbol is decoration and is hidden from the accessibility tree — a reader
 * announcing an emoji's own name in place of what the user called the row
 * would be worse than silence.
 */

export interface CheckListItem {
  id: string;
  /** What gets read aloud, and what is shown. */
  name: string;
  /** Optional short stand-in shown beside the name; never announced. */
  symbol?: string | null;
  checked: boolean;
}

export function CheckList({
  items,
  onToggle,
  className = 'form__check-list',
}: {
  items: readonly CheckListItem[];
  onToggle: (item: CheckListItem) => void;
  className?: string;
}) {
  const [activeIndex, setActiveIndex] = useState(0);
  const boxes = useRef<(HTMLInputElement | null)[]>([]);

  // Clamp when the list shrinks under an open dialog — a sync round can remove
  // a row another device deleted.
  useEffect(() => {
    if (activeIndex >= items.length && items.length > 0) {
      setActiveIndex(items.length - 1);
    }
  }, [items.length, activeIndex]);

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (items.length === 0) return;
      let next: number | null = null;
      if (e.key === 'ArrowDown') next = Math.min(activeIndex + 1, items.length - 1);
      else if (e.key === 'ArrowUp') next = Math.max(activeIndex - 1, 0);
      else if (e.key === 'Home') next = 0;
      else if (e.key === 'End') next = items.length - 1;
      if (next === null) return;
      e.preventDefault();
      setActiveIndex(next);
      // Move the REAL focus, not just the tab stop: the point is that the next
      // arrow press announces the row it landed on.
      boxes.current[next]?.focus();
    },
    [activeIndex, items.length],
  );

  return (
    <ul className={className} onKeyDown={onKeyDown}>
      {items.map((item, i) => (
        <li key={item.id}>
          <label className="form__field form__field--inline">
            <input
              ref={(el) => {
                boxes.current[i] = el;
              }}
              type="checkbox"
              checked={item.checked}
              tabIndex={i === activeIndex ? 0 : -1}
              onFocus={() => setActiveIndex(i)}
              onChange={() => onToggle(item)}
            />
            {item.symbol && <span aria-hidden="true">{item.symbol}</span>}
            <span>{item.name}</span>
          </label>
        </li>
      ))}
    </ul>
  );
}
