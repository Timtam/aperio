import { useCallback, useEffect, useRef } from 'react';
import { AccessibilityInfo, findNodeHandle } from 'react-native';
import type { Text, TextInput, View } from 'react-native';

/** An element whose accessibility focus we can move to (a row's first control
 *  or the Add button). The union covers the host components the editors use. */
type FocusableNode =
  | React.ElementRef<typeof View>
  | React.ElementRef<typeof Text>
  | React.ElementRef<typeof TextInput>;

/** Ref callback a row's first focusable control receives (from `registerRow`).
 *  Accepts the wide focusable union, so it's assignable to any host element's
 *  `ref` (Text / TextInput / View) by parameter contravariance. */
export type RowRefCallback = (node: FocusableNode | null) => void;

/**
 * Screen-reader focus management for a dynamic add/remove list.
 *
 * RN does not move VoiceOver/TalkBack focus when a list grows or shrinks, so
 * after the user adds or removes a row the cursor is stranded (usually back at
 * the top) and they must swipe to find their place again. This hook moves focus
 * to a sensible target instead: the newly-added row on add, and a surviving
 * sibling (or the Add button, when the last row goes) on remove.
 *
 * Usage: pass the current row `count`; attach `registerRow(i)` as the `ref` of
 * each row's first focusable control and `registerAdd` to the Add button; call
 * `onAdd()` / `onRemove(i)` right before the corresponding `onChange`. The
 * pending focus is applied in an effect once the list has re-rendered at its
 * new length.
 */
export function useListFocusManager(count: number) {
  const rowRefs = useRef(new Map<number, FocusableNode>());
  const addRef = useRef<FocusableNode | null>(null);
  const pending = useRef<number | 'add' | null>(null);

  const registerRow = useCallback(
    (index: number) => (node: FocusableNode | null) => {
      if (node) rowRefs.current.set(index, node);
      else rowRefs.current.delete(index);
    },
    [],
  );

  const registerAdd = useCallback((node: FocusableNode | null) => {
    addRef.current = node;
  }, []);

  const focusNode = useCallback((node: FocusableNode | null | undefined) => {
    if (!node) return;
    const tag = findNodeHandle(node);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  // Apply the pending focus once the list has re-rendered at its new length.
  // Keyed on `count`, so it runs after the add/remove that changed it. A null
  // `pending` (initial load, or an unrelated re-render) is a no-op.
  useEffect(() => {
    const target = pending.current;
    if (target == null) return;
    pending.current = null;
    if (target === 'add') {
      focusNode(addRef.current);
    } else {
      // Fall back to the Add button if the intended sibling has gone.
      focusNode(rowRefs.current.get(target) ?? addRef.current);
    }
  }, [count, focusNode]);

  /** Call before appending a row: focus lands on the new last row. */
  const onAdd = useCallback(() => {
    pending.current = count;
  }, [count]);

  /** Call before removing row `i`: focus lands on the previous row, or the Add
   *  button when the removed row was the only one. */
  const onRemove = useCallback(
    (i: number) => {
      pending.current = count <= 1 ? 'add' : Math.max(0, i - 1);
    },
    [count],
  );

  /** Move SR focus to row `i` directly (the count is unchanged, so the
   *  count-keyed effect won't fire). For in-place edits like an inline rename
   *  that swap a row's control without adding/removing a row: call it once the
   *  row has re-rendered (e.g. from an effect keyed on the edit flag) so the
   *  re-registered ref is current. */
  const focusRow = useCallback(
    (i: number) => {
      focusNode(rowRefs.current.get(i));
    },
    [focusNode],
  );

  return { registerRow, registerAdd, onAdd, onRemove, focusRow };
}
