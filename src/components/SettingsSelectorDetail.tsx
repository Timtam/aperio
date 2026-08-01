import { Fragment, useEffect, useId, useMemo, useState } from 'react';

/** A visually sub-headed group of selectable items (e.g. one account). */
export interface SettingsSelectorGroup<T> {
  /** Stable group id (React key). */
  id: string;
  /** Visible + spoken group label (e.g. the account name). */
  label: string;
  items: T[];
}

export interface SettingsSelectorDetailProps<T> {
  groups: SettingsSelectorGroup<T>[];
  /**
   * Item → stable id. Keep this referentially stable (module scope or
   * `useCallback`): it's the only accessor that feeds the navigation memos.
   */
  getItemId: (item: T) => string;
  /** Visible item name. */
  getItemName: (item: T) => string;
  /** Spoken state folded into the option's accessible name (e.g. "2 default reminders"). */
  getItemSummary: (item: T) => string;
  /** Optional visible, aria-hidden trailing badge (e.g. a count). The span always renders. */
  getItemBadge?: (item: T) => React.ReactNode;
  /** Optional leading colour-swatch hex; only consulted when `withSwatch`. */
  getItemSwatchHex?: (item: T) => string | null | undefined;
  /** Render a leading colour swatch per option (calendars). Omit for none (task lists). */
  withSwatch?: boolean;
  /** Listbox accessible name. */
  selectorLabel: string;
  /** Build each option's accessible name from group label + item name + spoken summary. */
  optionLabel: (args: { account: string; name: string; summary: string }) => string;
  /** Build the detail region heading from group label + item name. */
  detailHeading: (args: { account: string; name: string }) => string;
  /**
   * Heading level for the detail region's heading. Defaults to `3`.
   *
   * Pass `4` when the selector already sits inside a `<section>` that a
   * caller-owned `<h3>` heads: two `<h3>`s in the same section read as
   * SIBLINGS in a screen reader's heading list, so the detail looks like a
   * second top-level topic rather than the selected item's editor.
   */
  detailHeadingLevel?: 3 | 4;
  /**
   * Item to select when there is no valid selection yet (mount, or the
   * selected item disappeared). Falls back to the first item when absent or
   * not in the list. Use it when one row is materially different from the
   * others — the account that already holds the sync dataset — so the detail
   * pane does not open on an item the user has to arrow away from.
   */
  preferredItemId?: string | null;
  /**
   * Called whenever the selection moves, with the newly selected id (`null`
   * when the list emptied). Keep it referentially stable (module scope or
   * `useCallback`) — it is an effect dependency. Callers use it to drop state
   * that belonged to the previously selected item, e.g. a refusal whose repair
   * button lives on that item's detail pane and is gone the moment the
   * selection moves.
   */
  onSelectionChange?: (id: string | null) => void;
  /**
   * Render the detail editor for the selected item. The returned content is
   * keyed by the selected item's id, so it remounts (resetting any internal
   * state) whenever the selection changes — callers need not key it themselves.
   */
  renderDetail: (item: T, group: SettingsSelectorGroup<T>) => React.ReactNode;
}

/**
 * Accessible master/detail selector for per-entity settings.
 *
 * Extracted from the Calendars panel (commit 7ab1b0a) so the Tasks panel can
 * reuse the same keyboard-first idiom. Rather than render every entity's full
 * editor inline — which produced dozens of tab stops, none of them announcing
 * WHICH entity a focused control belonged to — this is a single `role="listbox"`
 * (one tab stop, Arrow/Home/End, grouped visually by account) whose options each
 * carry "{account} › {name}, {summary}" as their accessible name, plus a detail
 * region that hosts the editor for ONLY the selected entity and is headed after
 * it. Selection follows focus (aria-activedescendant), so arrowing live-swaps
 * the detail; one Tab lands in the selected entity's editor and nothing else.
 *
 * The emitted class names are the shared `calendars-panel__*` selector/option/
 * detail styles in styles.css (also borrowed by the Tasks panel).
 */
export function SettingsSelectorDetail<T>({
  groups,
  getItemId,
  getItemName,
  getItemSummary,
  getItemBadge,
  getItemSwatchHex,
  withSwatch = false,
  selectorLabel,
  optionLabel,
  detailHeading,
  detailHeadingLevel = 3,
  preferredItemId,
  onSelectionChange,
  renderDetail,
}: SettingsSelectorDetailProps<T>) {
  // Flat id order for arrow-key navigation across the account groups.
  const orderedIds = useMemo(
    () => groups.flatMap((g) => g.items.map((it) => getItemId(it))),
    [groups, getItemId],
  );

  const [selectedId, setSelectedId] = useState<string | null>(null);

  // Keep a valid selection: default to the caller's preferred item (the first
  // one when it names none, or names one that isn't here) and recover if the
  // selected one disappears (group removed mid-session, etc.).
  useEffect(() => {
    if (orderedIds.length === 0) {
      if (selectedId !== null) setSelectedId(null);
      return;
    }
    if (selectedId === null || !orderedIds.includes(selectedId)) {
      setSelectedId(
        preferredItemId && orderedIds.includes(preferredItemId)
          ? preferredItemId
          : orderedIds[0],
      );
    }
  }, [orderedIds, preferredItemId, selectedId]);

  // Tell the caller where the selection went, so it can drop state that
  // belonged to the item the user just left.
  useEffect(() => {
    onSelectionChange?.(selectedId);
  }, [onSelectionChange, selectedId]);

  const idPrefix = useId();
  const optionId = (id: string) => `${idPrefix}-opt-${id}`;
  const detailHeadingId = `${idPrefix}-detail-h`;

  // Keep the active option visible: with aria-activedescendant the browser
  // doesn't move DOM focus, so it won't auto-scroll the selection into view
  // inside the (capped-height, scrollable) listbox. `block: 'nearest'`
  // minimises movement. Optional-chained so it's a no-op where scrollIntoView
  // isn't implemented (jsdom).
  useEffect(() => {
    if (!selectedId) return;
    const el = document.getElementById(`${idPrefix}-opt-${selectedId}`);
    el?.scrollIntoView?.({ block: 'nearest' });
  }, [selectedId, idPrefix]);

  const selected = useMemo(() => {
    for (const g of groups) {
      const item = g.items.find((it) => getItemId(it) === selectedId);
      if (item) return { item, group: g };
    }
    return null;
  }, [groups, selectedId, getItemId]);

  const selectAt = (index: number) => {
    if (orderedIds.length === 0) return;
    const clamped = Math.min(orderedIds.length - 1, Math.max(0, index));
    setSelectedId(orderedIds[clamped]);
  };

  const handleKey = (e: React.KeyboardEvent<HTMLUListElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const cur = selectedId ? orderedIds.indexOf(selectedId) : -1;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        selectAt(cur + 1);
        return;
      case 'ArrowUp':
        e.preventDefault();
        selectAt(cur - 1);
        return;
      case 'Home':
        e.preventDefault();
        selectAt(0);
        return;
      case 'End':
        e.preventDefault();
        selectAt(orderedIds.length - 1);
        return;
      default:
        return;
    }
  };

  const DetailHeading = detailHeadingLevel === 4 ? 'h4' : 'h3';

  return (
    <div className="calendars-panel__master-detail">
      {/* Master: one keyboard-navigable list of entities. The visual account
          sub-headers are presentational (each option's own accessible name
          already carries the account), so arrow keys walk the options
          uninterrupted. */}
      <ul
        role="listbox"
        tabIndex={0}
        aria-label={selectorLabel}
        // Derive the active id from what actually renders (`selected`) rather
        // than raw `selectedId`, so the attribute can never dangle at an option
        // that was removed this render (recovery runs in the next commit).
        aria-activedescendant={
          selected ? optionId(getItemId(selected.item)) : undefined
        }
        onKeyDown={handleKey}
        className="calendars-panel__selector"
      >
        {groups.map((group) => (
          <li
            key={group.id}
            role="presentation"
            className="calendars-panel__selector-group"
          >
            <span className="calendars-panel__account" aria-hidden="true">
              {group.label}
            </span>
            <ul
              role="presentation"
              className="calendars-panel__selector-sublist"
            >
              {group.items.map((item) => {
                const id = getItemId(item);
                const isSel = id === selectedId;
                const hex = withSwatch ? getItemSwatchHex?.(item) : undefined;
                return (
                  <li
                    key={id}
                    id={optionId(id)}
                    role="option"
                    aria-selected={isSel}
                    aria-label={optionLabel({
                      account: group.label,
                      name: getItemName(item),
                      summary: getItemSummary(item),
                    })}
                    className={
                      'calendars-panel__option' +
                      (isSel ? ' calendars-panel__option--selected' : '')
                    }
                    onClick={() => setSelectedId(id)}
                  >
                    {withSwatch && (
                      <span
                        className="calendars-panel__swatch"
                        aria-hidden="true"
                        style={hex ? { background: hex } : undefined}
                      />
                    )}
                    <span className="calendars-panel__name">
                      {getItemName(item)}
                    </span>
                    <span
                      className="calendars-panel__option-summary"
                      aria-hidden="true"
                    >
                      {getItemBadge?.(item) ?? ''}
                    </span>
                  </li>
                );
              })}
            </ul>
          </li>
        ))}
      </ul>

      {/* Detail: editor for the selected entity only. The region is named
          after the entity so focus entering it (Tab from the list) announces
          the heading. */}
      {selected && (
        <section
          className="calendars-panel__detail"
          aria-labelledby={detailHeadingId}
        >
          <DetailHeading
            id={detailHeadingId}
            className="calendars-panel__detail-heading"
          >
            {detailHeading({
              account: selected.group.label,
              name: getItemName(selected.item),
            })}
          </DetailHeading>
          {/* Keyed by the selection so the editor remounts (and its internal
              state resets) when the selected entity changes. */}
          <Fragment key={getItemId(selected.item)}>
            {renderDetail(selected.item, selected.group)}
          </Fragment>
        </section>
      )}
    </div>
  );
}
