import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import {
  moveDayMarker,
  reorderDayMarkers,
  sameDayMarkerOrder,
  type DayMarker,
} from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  createDayMarker,
  deleteDayMarker,
  isCommandError,
  updateDayMarker,
} from '../api/client';
import { useAutoFocus } from '../hooks/useAutoFocus';
import { useCalendarStore } from '../state/calendarStoreContext';
import { duringDayMarkerBurst } from '../state/dayMarkersChanged';
import { useDayMarkers } from '../state/useDayMarkers';

/**
 * The day-marker vocabulary, managed from Settings.
 *
 * This is where the feature's whole flexibility lives: the user decides what
 * is worth noting about a day and how much to say about it — a word, a
 * sentence, an emoji. Aperio supplies no starter set, because a guessed one
 * would be somebody else's habits.
 *
 * Two modes, the same shape `ColorLabelsPanel` next door uses so the Settings
 * dialog has one idiom rather than two:
 *  - *List mode*: a listbox of markers. ONE tab stop, Arrow keys move within
 *    it, Enter opens the focused one for editing.
 *  - *Edit mode*: name, short symbol, colour, order, and Delete.
 *
 * Underneath either, the "add a marker" form stays mounted so creating
 * several in a row never costs a mode switch.
 */
export function DayMarkersPanel() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { colorLabels } = useCalendarStore();
  const { markers, loading, error: loadError, refresh, replace } = useDayMarkers();
  const namedLabels = colorLabels.filter((l) => !l.ad_hoc);

  const [newName, setNewName] = useState('');
  const [newSymbol, setNewSymbol] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);

  const listId = useId();
  const [activeId, setActiveId] = useState<string | null>(null);
  // Drag-and-drop is a MOUSE affordance layered on top; it adds nothing to the
  // accessibility tree and takes nothing from it. `dragId` is the row being
  // carried, `dropIndex` the slot it would land in — kept apart so the drop
  // marker can sit BELOW the last row (index === length) without pretending
  // there is a row there.
  const [dragId, setDragId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  // Leaving edit mode should land focus back on the listbox — but NOT on the
  // panel's first mount, including a Settings tab switch: the tab keeps focus
  // there, and grabbing it would make NVDA read the first marker the moment
  // the user arrows onto the tab.
  const focusListOnNextMountRef = useRef(false);
  const exitEditMode = useCallback(() => {
    focusListOnNextMountRef.current = true;
    setEditingId(null);
  }, []);
  const listRef = useRef<HTMLUListElement>(null);
  const sectionRef = useRef<HTMLDivElement>(null);
  // Not `useAutoFocus`: that hook fires at most once per MOUNT, and this hook
  // instance lives on the panel while the list mounts and unmounts under it —
  // so the first exit from edit mode worked and every later one stranded focus
  // on <body>. The flag is cleared here, and the section catches the case
  // where the deleted marker was the last one and no list renders at all.
  useEffect(() => {
    if (!focusListOnNextMountRef.current || editingId !== null) return;
    focusListOnNextMountRef.current = false;
    (listRef.current ?? sectionRef.current)?.focus({ preventScroll: true });
  }, [editingId, markers.length]);

  // Clamp the active option when the list shrinks. A stale id is not just a
  // dangling aria-activedescendant: the reorder buttons resolve `undefined`
  // and do nothing while still rendering, and Enter opens an editor for a
  // marker that no longer exists.
  useEffect(() => {
    if (markers.length === 0) {
      if (activeId !== null) setActiveId(null);
      return;
    }
    if (activeId && !markers.some((m) => m.id === activeId)) {
      setActiveId(markers[markers.length - 1].id);
    }
  }, [markers, activeId]);

  const reportError = useCallback(
    (err: unknown) => {
      setError(isCommandError(err) ? `${err.code}: ${err.message}` : String(err));
    },
    [],
  );

  const editing = markers.find((m) => m.id === editingId) ?? null;
  /** The row the action buttons below the list act on. */
  const active = markers.find((m) => m.id === activeId) ?? null;

  const onAdd = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      const name = newName.trim();
      if (!name) {
        setError(t('dialogs.settings.dayMarkers.nameRequired'));
        return;
      }
      setBusy(true);
      setError(null);
      try {
        await createDayMarker({
          name,
          symbol: newSymbol.trim() || null,
          color_label: null,
        });
        setNewName('');
        setNewSymbol('');
        await refresh();
        announce(t('dialogs.settings.dayMarkers.added', { name }));
      } catch (err) {
        reportError(err);
      } finally {
        setBusy(false);
      }
    },
    [newName, newSymbol, refresh, announce, t, reportError],
  );

  const onSave = useCallback(
    async (next: DayMarker) => {
      setBusy(true);
      setError(null);
      try {
        await updateDayMarker(next);
        await refresh();
        announce(t('dialogs.settings.dayMarkers.saved', { name: next.name }));
        exitEditMode();
      } catch (err) {
        reportError(err);
      } finally {
        setBusy(false);
      }
    },
    [refresh, announce, t, exitEditMode, reportError],
  );

  const onDelete = useCallback(
    async (marker: DayMarker) => {
      setBusy(true);
      setError(null);
      try {
        await deleteDayMarker(marker.id);
        await refresh();
        // Worth saying plainly: the days keep their record, the marker just
        // stops resolving. Nobody should fear losing history to a rename.
        announce(t('dialogs.settings.dayMarkers.deleted', { name: marker.name }));
        exitEditMode();
      } catch (err) {
        reportError(err);
      } finally {
        setBusy(false);
      }
    },
    [refresh, announce, t, exitEditMode, reportError],
  );

  /** Move one marker and write back every position that shifted. */
  const onMove = useCallback(
    async (marker: DayMarker, delta: number) => {
      const reordered = moveDayMarker(markers, marker.id, delta);
      if (sameDayMarkerOrder(markers, reordered)) return;
      replace(reordered);
      setBusy(true);
      try {
        // Only the rows whose position actually changed — a reorder near the
        // top of a long list must not rewrite the whole vocabulary.
        const before = new Map(markers.map((m) => [m.id, m.position ?? 0]));
        // One burst: the rows shift together, so the readers hear about it
        // once, at the end, rather than between each pair.
        await duringDayMarkerBurst(async () => {
          for (const m of reordered) {
            if (before.get(m.id) !== m.position) await updateDayMarker(m);
          }
        });
        announce(
          t('dialogs.settings.dayMarkers.moved', {
            name: marker.name,
            position: (reordered.findIndex((m) => m.id === marker.id) ?? 0) + 1,
            count: reordered.length,
          }),
        );
      } catch (err) {
        reportError(err);
        await refresh();
      } finally {
        setBusy(false);
      }
    },
    [markers, replace, refresh, announce, t, reportError],
  );

  /** Move `marker` to `toIndex` and write back every row that shifted. The
   *  drop half of drag-and-drop; `onMove` is the same thing by a delta. */
  const onDropAt = useCallback(
    async (id: string, toIndex: number) => {
      const reordered = reorderDayMarkers(markers, id, toIndex);
      if (sameDayMarkerOrder(markers, reordered)) return;
      const marker = markers.find((m) => m.id === id);
      if (!marker) return;
      replace(reordered);
      setBusy(true);
      try {
        const before = new Map(markers.map((m) => [m.id, m.position ?? 0]));
        await duringDayMarkerBurst(async () => {
          for (const m of reordered) {
            if (before.get(m.id) !== m.position) await updateDayMarker(m);
          }
        });
        // Announced like the button move, because a drag can end somewhere the
        // user did not intend and this is the only confirmation of where it
        // actually landed.
        announce(
          t('dialogs.settings.dayMarkers.moved', {
            name: marker.name,
            position: reordered.findIndex((m) => m.id === id) + 1,
            count: reordered.length,
          }),
        );
      } catch (err) {
        reportError(err);
        await refresh();
      } finally {
        setBusy(false);
      }
    },
    [markers, replace, refresh, announce, t, reportError],
  );

  const onListKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (markers.length === 0) return;
      const idx = Math.max(
        0,
        markers.findIndex((m) => m.id === activeId),
      );
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const next = e.key === 'ArrowDown' ? idx + 1 : idx - 1;
        if (next >= 0 && next < markers.length) setActiveId(markers[next].id);
      } else if (e.key === 'Home') {
        e.preventDefault();
        setActiveId(markers[0].id);
      } else if (e.key === 'End') {
        e.preventDefault();
        setActiveId(markers[markers.length - 1].id);
      } else if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        setEditingId(activeId ?? markers[0].id);
      }
    },
    [markers, activeId],
  );

  return (
    // `tabIndex={-1}` so focus has somewhere to land when the list itself is
    // gone — deleting the last marker leaves no listbox to return to.
    <div className="settings-panel" ref={sectionRef} tabIndex={-1}>
      <h3>{t('dialogs.settings.dayMarkers.heading')}</h3>
      <p className="form__hint">{t('dialogs.settings.dayMarkers.intro')}</p>

      {(error || loadError) && (
        <p className="form__error" role="alert">
          {error ?? loadError}
        </p>
      )}

      {editing ? (
        <DayMarkerEditor
          marker={editing}
          labels={namedLabels}
          busy={busy}
          onSave={onSave}
          onDelete={onDelete}
          onCancel={exitEditMode}
        />
      ) : (
        <>
          {loading ? (
            <p className="form__hint">{t('dialogs.settings.dayMarkers.loading')}</p>
          ) : markers.length === 0 ? (
            <p className="form__hint">{t('dialogs.settings.dayMarkers.empty')}</p>
          ) : (
            <ul
              ref={listRef}
              id={listId}
              className="settings-list"
              role="listbox"
              tabIndex={0}
              aria-label={t('dialogs.settings.dayMarkers.listLabel')}
              aria-activedescendant={activeId ? `${listId}-${activeId}` : undefined}
              onKeyDown={onListKeyDown}
              onFocus={() => {
                if (!activeId && markers.length > 0) setActiveId(markers[0].id);
              }}
            >
              {markers.map((m, i) => (
                <li
                  key={m.id}
                  id={`${listId}-${m.id}`}
                  role="option"
                  aria-selected={m.id === activeId}
                  className={
                    'settings-list__row settings-list__row--grabbable' +
                    (m.id === activeId ? ' settings-list__row--active' : '') +
                    (m.id === dragId ? ' settings-list__row--dragging' : '') +
                    (dropIndex === i ? ' settings-list__row--drop-before' : '') +
                    (dropIndex === markers.length && i === markers.length - 1
                      ? ' settings-list__row--drop-after'
                      : '')
                  }
                  // A row a mouse can select. Without this the only way to
                  // move the selection was the arrow keys, so every button
                  // below acted on the first marker forever and the rest of
                  // the list was unreachable for anyone using a mouse.
                  onClick={() => {
                    setActiveId(m.id);
                    // Keep the real focus on the listbox: `aria-activedescendant`
                    // only means anything while the box itself is focused, and
                    // the arrow keys have to keep working from wherever the
                    // click left off.
                    listRef.current?.focus({ preventScroll: true });
                  }}
                  // The mouse idiom for "open this", the same one the calendar
                  // uses on a day.
                  onDoubleClick={() => {
                    setActiveId(m.id);
                    setEditingId(m.id);
                  }}
                  draggable={!busy}
                  onDragStart={(e) => {
                    setDragId(m.id);
                    e.dataTransfer.effectAllowed = 'move';
                    // Some browsers refuse to start a drag without payload.
                    e.dataTransfer.setData('text/plain', m.id);
                  }}
                  onDragEnd={() => {
                    setDragId(null);
                    setDropIndex(null);
                  }}
                  onDragOver={(e) => {
                    if (dragId == null) return;
                    e.preventDefault();
                    e.dataTransfer.dropEffect = 'move';
                    // Past the midpoint means "after this row" — otherwise the
                    // bottom half of the last row could never be a target and
                    // the end of the list would be unreachable.
                    const box = e.currentTarget.getBoundingClientRect();
                    const after = e.clientY > box.top + box.height / 2;
                    setDropIndex(after ? i + 1 : i);
                  }}
                  onDrop={(e) => {
                    e.preventDefault();
                    const id = dragId;
                    const target = dropIndex;
                    setDragId(null);
                    setDropIndex(null);
                    if (id == null || target == null) return;
                    // Removing the row first shifts every later slot down by
                    // one, so a drop below its own position has to compensate.
                    const from = markers.findIndex((x) => x.id === id);
                    const to = target > from ? target - 1 : target;
                    setActiveId(id);
                    void onDropAt(id, to);
                  }}
                >
                  {/* The symbol is decoration; the row's name carries the
                      meaning, and a screen reader must not read an emoji's
                      own name in place of what the user called this. */}
                  {m.symbol && <span aria-hidden="true">{m.symbol} </span>}
                  {t('dialogs.settings.dayMarkers.rowLabel', {
                    name: m.name,
                    position: i + 1,
                    count: markers.length,
                  })}
                </li>
              ))}
            </ul>
          )}
          {markers.length > 0 && (
            <p className="form__hint">{t('dialogs.settings.dayMarkers.listHint')}</p>
          )}

          {/* The actions live outside the listbox rather than in every row:
              four buttons per row would cost five tab stops per marker, and
              these are things the user does once rather than while reading.
              They act on the SELECTED row — which a mouse can now set — and
              every label names it, so a screen reader never has to remember
              which one is selected to know what a button will do. */}
          {active && (
            <div className="settings-panel__actions">
              <button
                type="button"
                className="form__action"
                aria-disabled={busy}
                aria-label={`${t('dialogs.settings.dayMarkers.edit')}: ${active.name}`}
                onClick={() => {
                  if (!busy) setEditingId(active.id);
                }}
              >
                {t('dialogs.settings.dayMarkers.edit')}
              </button>
              <button
                type="button"
                className="form__action form__action--destructive"
                aria-disabled={busy}
                aria-label={`${t('dialogs.settings.dayMarkers.delete')}: ${active.name}`}
                onClick={() => {
                  if (!busy) void onDelete(active);
                }}
              >
                {t('dialogs.settings.dayMarkers.delete')}
              </button>
              {markers.length > 1 && (
                <>
                  <button
                    type="button"
                    className="form__action"
                    aria-disabled={busy}
                    aria-label={`${t('dialogs.settings.dayMarkers.moveUp')}: ${active.name}`}
                    onClick={() => {
                      if (!busy) void onMove(active, -1);
                    }}
                  >
                    {t('dialogs.settings.dayMarkers.moveUp')}
                  </button>
                  <button
                    type="button"
                    className="form__action"
                    aria-disabled={busy}
                    aria-label={`${t('dialogs.settings.dayMarkers.moveDown')}: ${active.name}`}
                    onClick={() => {
                      if (!busy) void onMove(active, 1);
                    }}
                  >
                    {t('dialogs.settings.dayMarkers.moveDown')}
                  </button>
                </>
              )}
            </div>
          )}

          <form className="form" onSubmit={onAdd}>
            <h4>{t('dialogs.settings.dayMarkers.addHeading')}</h4>
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.settings.dayMarkers.nameLabel')}
              </span>
              <input
                type="text"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                autoComplete="off"
              />
            </label>
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.settings.dayMarkers.symbolLabel')}
              </span>
              <input
                type="text"
                value={newSymbol}
                onChange={(e) => setNewSymbol(e.target.value)}
                autoComplete="off"
              />
              <span className="form__hint">
                {t('dialogs.settings.dayMarkers.symbolHint')}
              </span>
            </label>
            <button type="submit" className="form__action" aria-disabled={busy}>
              {t('dialogs.settings.dayMarkers.add')}
            </button>
          </form>
        </>
      )}
    </div>
  );
}

/** The edit form for one marker. Split out so the list stays readable. */
function DayMarkerEditor({
  marker,
  labels,
  busy,
  onSave,
  onDelete,
  onCancel,
}: {
  marker: DayMarker;
  labels: { id: string; name: string }[];
  busy: boolean;
  onSave: (next: DayMarker) => void;
  onDelete: (marker: DayMarker) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(marker.name);
  const [symbol, setSymbol] = useState(marker.symbol ?? '');
  const [colorLabel, setColorLabel] = useState(marker.color_label ?? '');
  const nameRef = useAutoFocus<HTMLInputElement>(true);

  return (
    <form
      className="form"
      onSubmit={(e) => {
        e.preventDefault();
        if (busy) return;
        onSave({
          ...marker,
          name: name.trim() || marker.name,
          symbol: symbol.trim() || null,
          color_label: colorLabel || null,
        });
      }}
    >
      <h4>{t('dialogs.settings.dayMarkers.editHeading', { name: marker.name })}</h4>
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.dayMarkers.nameLabel')}
        </span>
        <input
          ref={nameRef}
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoComplete="off"
        />
      </label>
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.dayMarkers.symbolLabel')}
        </span>
        <input
          type="text"
          value={symbol}
          onChange={(e) => setSymbol(e.target.value)}
          autoComplete="off"
        />
      </label>
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.dayMarkers.colorLabel')}
        </span>
        <select value={colorLabel} onChange={(e) => setColorLabel(e.target.value)}>
          <option value="">{t('dialogs.settings.dayMarkers.colorNone')}</option>
          {labels.map((l) => (
            <option key={l.id} value={l.id}>
              {l.name}
            </option>
          ))}
        </select>
      </label>
      <div className="settings-panel__actions">
        <button type="submit" className="form__action" aria-disabled={busy}>
          {t('dialogs.settings.dayMarkers.save')}
        </button>
        <button type="button" className="form__action" onClick={onCancel}>
          {t('dialogs.settings.dayMarkers.cancel')}
        </button>
        <button
          type="button"
          className="form__action form__action--danger"
          aria-disabled={busy}
          onClick={() => !busy && onDelete(marker)}
        >
          {t('dialogs.settings.dayMarkers.delete')}
        </button>
      </div>
      <p className="form__hint">{t('dialogs.settings.dayMarkers.deleteHint')}</p>
    </form>
  );
}
