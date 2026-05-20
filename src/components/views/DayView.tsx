import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isSameDay } from 'date-fns';

import { useAnnouncer } from '../../a11y/Announcer';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveEventColor } from '../../intl/eventColor';
import { useCalendarStore } from '../../state/CalendarStore';
import { useDialogState } from '../../state/DialogState';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';
import { duplicateEvent } from '../MoveCopyDialog';
import { ConfirmDialog } from '../ConfirmDialog';
import { DeleteEventScopeDialog } from '../DeleteEventScopeDialog';
import {
  addEventExdate,
  deleteEventById,
  isCommandError,
} from '../../api/client';
import type { CalendarEvent } from '../../api/types';

/**
 * Day view — flat listbox of the focused day's events.
 *
 * Phase 3 keeps it deliberately simple: the events of the active day are
 * rendered as `role="option"` items inside a `role="listbox"`, with
 * keyboard navigation via `aria-activedescendant`. The 15-minute slot
 * grid + slot focus from DESIGN.md section 3.3 returns alongside the
 * event-creation dialog in a later phase.
 *
 * Why listbox rather than a non-interactive list: with the listbox
 * pattern the screen reader stays in focus mode and reads the active
 * option as it changes — just like Week/Month view. A bare
 * `tabIndex=-1` section lacks a clear interactive role and lets NVDA
 * fall back to browse mode, which would break arrow navigation.
 */
export function DayView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { anchor } = useViewState();
  const { openEventDialog, openMoveCopy, invalidateData } = useDialogState();

  const range = useMemo(() => visibleRange('day', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);
  const { colorLabels } = useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  const dayEvents = useMemo(
    () => events.filter((ev) => isSameDay(new Date(ev.start), anchor)),
    [events, anchor],
  );

  const [focusIndex, setFocusIndex] = useState(0);

  // If the day changes (or events arrive) and the previous focus index
  // is past the end of the new list, snap back to the last valid item.
  useEffect(() => {
    if (focusIndex >= dayEvents.length) {
      setFocusIndex(Math.max(0, dayEvents.length - 1));
    }
  }, [dayEvents.length, focusIndex]);

  const idPrefix = useId();
  const itemId = (i: number) => `${idPrefix}-item-${i}`;
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const [confirmTarget, setConfirmTarget] = useState<CalendarEvent | null>(
    null,
  );
  const [scopeTarget, setScopeTarget] = useState<CalendarEvent | null>(null);

  const performDelete = useCallback(
    async (ev: CalendarEvent, scope: 'occurrence' | 'series') => {
      try {
        if (scope === 'occurrence' && ev.id.includes('@')) {
          const [seriesId, occIso] = ev.id.split('@');
          await addEventExdate(seriesId, occIso, ev.calendar_id);
          announce(
            t('dialogs.event.occurrenceDeleted', { title: ev.title }),
          );
        } else {
          const id = ev.id.includes('@') ? ev.id.split('@')[0] : ev.id;
          await deleteEventById(id, ev.calendar_id);
          announce(t('dialogs.event.deleted', { title: ev.title }));
        }
        // The DeleteEventScope / Confirm dialogs are local view state,
        // not part of DialogState, so closing them won't trigger a
        // refetch automatically. Bump the data version ourselves so
        // useEvents re-reads after the mutation lands on the server.
        invalidateData();
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      }
    },
    [announce, t, invalidateData],
  );

  const requestDelete = useCallback((ev: CalendarEvent) => {
    if (ev.id.includes('@') || ev.recurrence) {
      setScopeTarget(ev);
    } else {
      setConfirmTarget(ev);
    }
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Ctrl+D duplicates the focused event in place; Shift+M opens
      // the move/copy dialog. Bare keys cover the rest of the listbox
      // navigation.
      if (e.ctrlKey || e.metaKey) {
        if (e.key.toLowerCase() === 'd' && !e.shiftKey && !e.altKey) {
          e.preventDefault();
          const ev = dayEvents[focusIndex];
          if (ev) {
            void duplicateEvent(ev).then(() =>
              announce(
                t('actions.duplicated', { title: ev.title }),
              ),
            );
          }
        }
        return;
      }
      if (e.altKey) return;
      if (e.shiftKey && e.key.toLowerCase() === 'm') {
        e.preventDefault();
        const ev = dayEvents[focusIndex];
        if (ev) openMoveCopy({ kind: 'event', event: ev });
        return;
      }
      if (dayEvents.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusIndex((i) => Math.min(i + 1, dayEvents.length - 1));
          return;
        case 'ArrowUp':
          e.preventDefault();
          setFocusIndex((i) => Math.max(i - 1, 0));
          return;
        case 'Home':
          e.preventDefault();
          setFocusIndex(0);
          return;
        case 'End':
          e.preventDefault();
          setFocusIndex(dayEvents.length - 1);
          return;
        case 'Enter':
        case ' ':
        case 'Spacebar': {
          e.preventDefault();
          const ev = dayEvents[focusIndex];
          if (ev) openEventDialog(ev);
          return;
        }
        case 'Delete':
        case 'Backspace': {
          e.preventDefault();
          const ev = dayEvents[focusIndex];
          if (ev) requestDelete(ev);
          return;
        }
        default:
          return;
      }
    },
    [
      dayEvents,
      focusIndex,
      openEventDialog,
      openMoveCopy,
      announce,
      t,
      requestDelete,
    ],
  );

  const today = useMemo(() => new Date(), []);
  const isToday = isSameDay(today, anchor);

  return (
    <section className="view view--day" aria-label={fmt.format(anchor, 'PPPP')}>
      <header className="view__header">
        <h2 aria-current={isToday ? 'date' : undefined}>
          {fmt.format(anchor, 'PPPP')}
        </h2>
      </header>

      <ul
        ref={listRef}
        role="listbox"
        tabIndex={0}
        aria-label={t('views.day.eventList')}
        aria-activedescendant={
          dayEvents.length > 0 ? itemId(focusIndex) : undefined
        }
        onKeyDown={handleKeyDown}
        className="day-list"
      >
        {dayEvents.length === 0 ? (
          <li role="presentation" className="day-list__empty">
            {t('views.day.empty')}
          </li>
        ) : (
          dayEvents.map((ev, i) => {
            const cal = calendarById.get(ev.calendar_id);
            const color = resolveEventColor(ev, calendarById, labelById);
            const startStr = fmt.format(new Date(ev.start), 'p');
            const endStr = fmt.format(new Date(ev.end), 'p');
            const aria = color.labelName
              ? t('views.day.eventLabelWithLabel', {
                  title: ev.title,
                  start: startStr,
                  end: endStr,
                  calendar: cal?.name ?? '—',
                  label: color.labelName,
                })
              : t('views.day.eventLabel', {
                  title: ev.title,
                  start: startStr,
                  end: endStr,
                  calendar: cal?.name ?? '—',
                });
            const focused = i === focusIndex;
            return (
              <li
                key={ev.id}
                id={itemId(i)}
                role="option"
                aria-selected={focused}
                aria-label={aria}
                className={
                  'day-list__item' +
                  (focused ? ' day-list__item--focused' : '')
                }
                style={
                  color.hex
                    ? ({ '--event-color': color.hex } as React.CSSProperties)
                    : undefined
                }
                onClick={() => setFocusIndex(i)}
              >
                <span className="day-list__time">
                  {startStr} – {endStr}
                </span>
                <span className="day-list__title">{ev.title}</span>
              </li>
            );
          })
        )}
      </ul>

      <ConfirmDialog
        isOpen={confirmTarget !== null}
        onClose={() => setConfirmTarget(null)}
        onConfirm={() => {
          if (confirmTarget) void performDelete(confirmTarget, 'series');
        }}
        title={t('dialogs.confirm.deleteEventTitle')}
        message={t('dialogs.confirm.deleteEventMessage', {
          title: confirmTarget?.title ?? '',
        })}
      />

      <DeleteEventScopeDialog
        isOpen={scopeTarget !== null}
        onClose={() => setScopeTarget(null)}
        title={scopeTarget?.title ?? ''}
        onOccurrence={() => {
          if (scopeTarget) void performDelete(scopeTarget, 'occurrence');
        }}
        onSeries={() => {
          if (scopeTarget) void performDelete(scopeTarget, 'series');
        }}
      />
    </section>
  );
}
