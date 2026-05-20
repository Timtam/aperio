import { useCallback, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { addDays, isSameDay, startOfWeek } from 'date-fns';

import { useAnnouncer } from '../../a11y/Announcer';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useEventTabNavigation } from '../../hooks/useEventTabNavigation';
import { localDateKey } from '../../intl/dateKey';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveEventColor } from '../../intl/eventColor';
import { useCalendarStore } from '../../state/CalendarStore';
import { useDialogState } from '../../state/DialogState';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';
import type { CalendarEvent } from '../../api/types';
import { ConfirmDialog } from '../ConfirmDialog';
import { deleteEventById, isCommandError } from '../../api/client';

/**
 * Week view — the workhorse calendar surface.
 *
 * Layout: a 7-cell grid (Mon–Sun, ISO weeks). Each cell shows the events
 * scheduled on that day. The KW number lives in the header next to the
 * date range (DESIGN.md section 5.2).
 *
 * Screen-reader model (section 3.3, Wochenansicht):
 *  - `role="grid"` on the container, which holds focus permanently and
 *    announces the active cell via `aria-activedescendant`.
 *  - `role="gridcell"` per day, with `aria-selected` on the active cell.
 *  - `aria-current="date"` on today's cell.
 *
 * Why `aria-activedescendant` instead of a roving tabindex: with roving
 * tabindex the focused cell's `tabIndex` toggles between `0` and `-1`,
 * and the DOM focus has to be moved explicitly on each arrow press. If
 * the focus isn't moved the cell shows as selected (ARIA) but loses the
 * visual focus ring. The active-descendant pattern keeps DOM focus on
 * the grid; the cell highlight is pure CSS driven by a class, so there
 * is no window where "selected" and "highlighted" can disagree.
 *
 * Keyboard model:
 *  - Left/Right move the focused day inside the visible week.
 *  - Up/Down (and PageUp/PageDown) scroll the week — Outlook convention.
 *  - Home / End jump to the first / last day of the visible week.
 *  - Ctrl-modified arrows are handled by the global shortcut layer.
 *
 * `anchor` is the single source of truth; the focused cell index is
 * derived from it. One state update per key press, one render commit.
 */
export function WeekView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { anchor, setAnchor, goPrev, goNext } = useViewState();
  const { openEventDialog } = useDialogState();

  const range = useMemo(() => visibleRange('week', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);
  const { colorLabels } = useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  const weekStart = useMemo(
    () => startOfWeek(anchor, { weekStartsOn: 1 }),
    [anchor],
  );
  const days = useMemo(
    () => Array.from({ length: 7 }, (_, i) => addDays(weekStart, i)),
    [weekStart],
  );

  const eventsByDay = useMemo(
    () => groupEventsByDay(events, days),
    [events, days],
  );

  const focusIndex = useMemo(() => {
    const i = days.findIndex((d) => isSameDay(d, anchor));
    return i >= 0 ? i : 0;
  }, [days, anchor]);

  // Unique prefix per WeekView instance, in case there's ever more than
  // one on screen (e.g. a future side-by-side comparison).
  const idPrefix = useId();
  const cellId = (i: number) => `${idPrefix}-cell-${i}`;
  const eventOptionId = (dayIdx: number, evIdx: number) =>
    `${idPrefix}-cell-${dayIdx}-ev-${evIdx}`;

  // Two-level focus: `null` means the day cell itself is focused (arrow
  // keys move the day). A number means the user has tabbed into the
  // day and is focused on the n-th event of that day — Enter opens it,
  // Delete removes it, Escape returns to the day cell.
  //
  // Tab is handled by the shared hook below: it crosses day boundaries
  // and moves the anchor for us, so the visual day selection follows
  // the focused event the way it does in Outlook.
  const buckets = useMemo(
    () => days.map((d) => ({ events: eventsByDay.get(keyOf(d)) ?? [] })),
    [days, eventsByDay],
  );
  const focusedDayEvents = buckets[focusIndex]?.events ?? [];

  const dayChangeAnnouncer = useCallback(
    (newDayIdx: number, ev: CalendarEvent) => {
      announce(
        t('views.week.tabAnnounce', {
          day: fmt.format(days[newDayIdx], 'PPPP'),
          title: ev.title,
        }),
      );
    },
    [announce, days, fmt, t],
  );

  const {
    eventIndex,
    clear: clearEventIndex,
    handleTab,
  } = useEventTabNavigation({
    buckets,
    dayIndex: focusIndex,
    setDayIndex: (next) => setAnchor(days[next]),
    onDayChange: dayChangeAnnouncer,
  });

  // Delete confirmation. `confirmTarget` carries the event that the
  // user is about to delete; rendering the dialog conditionally keeps
  // its DOM out of the tree until needed.
  const [confirmTarget, setConfirmTarget] = useState<CalendarEvent | null>(
    null,
  );
  const performDelete = useCallback(
    async (ev: CalendarEvent) => {
      try {
        // Strip the synthetic occurrence suffix — delete always targets
        // the master row. Single-occurrence delete lives in EventDialog
        // where the user can pick scope explicitly.
        const id = ev.id.includes('@') ? ev.id.split('@')[0] : ev.id;
        await deleteEventById(id);
        announce(t('dialogs.event.deleted', { title: ev.title }));
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      }
    },
    [announce, t],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Tab / Shift+Tab walk through *all* events of the visible week
      // chronologically, crossing day boundaries — the hook moves the
      // anchor and announces the new day when it does.
      if (e.key === 'Tab' && !e.ctrlKey && !e.metaKey && !e.altKey) {
        const consumed = handleTab(e.shiftKey);
        if (consumed) e.preventDefault();
        return;
      }
      if (e.ctrlKey || e.metaKey || e.altKey) {
        return;
      }

      // Event-level shortcuts when an event inside the cell is focused.
      if (eventIndex !== null) {
        const ev = focusedDayEvents[eventIndex];
        if (e.key === 'Escape') {
          e.preventDefault();
          clearEventIndex();
          return;
        }
        if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') {
          e.preventDefault();
          if (ev) openEventDialog(ev);
          return;
        }
        if (e.key === 'Delete' || e.key === 'Backspace') {
          e.preventDefault();
          if (ev) setConfirmTarget(ev);
          return;
        }
        // Arrow keys fall through to day navigation below, which will
        // also reset eventIndex via the focusIndex effect.
      }

      switch (e.key) {
        case 'ArrowLeft':
          e.preventDefault();
          setAnchor(addDays(anchor, -1));
          return;
        case 'ArrowRight':
          e.preventDefault();
          setAnchor(addDays(anchor, 1));
          return;
        case 'ArrowUp':
        case 'PageUp':
          e.preventDefault();
          goPrev();
          return;
        case 'ArrowDown':
        case 'PageDown':
          e.preventDefault();
          goNext();
          return;
        case 'Home':
          e.preventDefault();
          setAnchor(weekStart);
          return;
        case 'End':
          e.preventDefault();
          setAnchor(addDays(weekStart, 6));
          return;
        case 'Enter':
        case ' ':
        case 'Spacebar': {
          // Enter on a focused cell with events: open the first one.
          // Pressing Enter on an empty cell opens the create-event
          // dialog pre-seeded with that day so the form reflects what
          // the user is looking at.
          e.preventDefault();
          const focusedDay = days[focusIndex];
          const evs = eventsByDay.get(keyOf(focusedDay)) ?? [];
          if (evs.length > 0) {
            openEventDialog(evs[0]);
          } else {
            openEventDialog(null, {
              defaultDate: keyOf(focusedDay),
            });
          }
          return;
        }
        default:
          return;
      }
    },
    [
      anchor,
      weekStart,
      setAnchor,
      goPrev,
      goNext,
      days,
      focusIndex,
      eventIndex,
      eventsByDay,
      focusedDayEvents,
      openEventDialog,
      handleTab,
      clearEventIndex,
    ],
  );

  const today = useMemo(() => new Date(), []);
  const isoWeek = fmt.isoWeek(weekStart);
  // Wait for the first fetch before focusing so the screen reader
  // announces the real day-event count, not the initial empty one.
  const gridRef = useAutoFocus<HTMLDivElement>(!loading);

  return (
    <section className="view view--week" aria-label={t('views.week.title')}>
      <header className="view__header">
        <h2>
          {t('views.week.kw', { week: isoWeek })} ·{' '}
          {fmt.format(weekStart, 'PP')} – {fmt.format(days[6], 'PP')}
        </h2>
      </header>

      <div
        ref={gridRef}
        role="grid"
        aria-label={t('views.week.gridLabel')}
        tabIndex={0}
        aria-activedescendant={
          eventIndex !== null
            ? eventOptionId(focusIndex, eventIndex)
            : cellId(focusIndex)
        }
        onKeyDown={handleKeyDown}
        className="week-grid"
      >
        <div role="row" className="week-grid__head">
          {days.map((day) => (
            <div
              key={day.toISOString()}
              role="columnheader"
              className="week-grid__col-head"
              aria-current={isSameDay(day, today) ? 'date' : undefined}
            >
              <span className="week-grid__dow">{fmt.format(day, 'EEE')}</span>
              <span className="week-grid__date">{fmt.format(day, 'd')}</span>
            </div>
          ))}
        </div>

        <div role="row" className="week-grid__body">
          {days.map((day, i) => {
            const dayEvents = eventsByDay.get(keyOf(day)) ?? [];
            const focused = i === focusIndex;
            return (
              <div
                key={day.toISOString()}
                id={cellId(i)}
                role="gridcell"
                aria-selected={focused}
                aria-current={isSameDay(day, today) ? 'date' : undefined}
                aria-label={t('views.week.dayAnnounce', {
                  day: fmt.format(day, 'PPPP'),
                  count: dayEvents.length,
                })}
                className={
                  'week-grid__cell' +
                  (focused ? ' week-grid__cell--focused' : '') +
                  (isSameDay(day, today) ? ' week-grid__cell--today' : '')
                }
                onClick={() => setAnchor(day)}
              >
                <ul role="list" className="week-grid__events">
                  {dayEvents.map((ev, evIdx) => {
                    const cal = calendarById.get(ev.calendar_id);
                    const color = resolveEventColor(ev, calendarById, labelById);
                    const time = ev.all_day
                      ? t('views.allDay')
                      : `${fmt.format(new Date(ev.start), 'p')}`;
                    const aria = color.labelName
                      ? t('views.week.eventLabelWithLabel', {
                          title: ev.title,
                          time,
                          calendar: cal?.name ?? '—',
                          label: color.labelName,
                        })
                      : t('views.week.eventLabel', {
                          title: ev.title,
                          time,
                          calendar: cal?.name ?? '—',
                        });
                    const isFocusedEvent =
                      focused && eventIndex === evIdx;
                    return (
                      <li key={ev.id} role="listitem">
                        <span
                          id={eventOptionId(i, evIdx)}
                          className={
                            'week-event' +
                            (isFocusedEvent ? ' week-event--focused' : '')
                          }
                          aria-label={aria}
                          aria-selected={isFocusedEvent}
                          style={
                            color.hex
                              ? ({ '--event-color': color.hex } as React.CSSProperties)
                              : undefined
                          }
                        >
                          <span className="week-event__time">{time}</span>
                          <span className="week-event__title">{ev.title}</span>
                        </span>
                      </li>
                    );
                  })}
                </ul>
              </div>
            );
          })}
        </div>
      </div>

      <ConfirmDialog
        isOpen={confirmTarget !== null}
        onClose={() => setConfirmTarget(null)}
        onConfirm={() => {
          if (confirmTarget) void performDelete(confirmTarget);
        }}
        title={t('dialogs.confirm.deleteEventTitle')}
        message={t('dialogs.confirm.deleteEventMessage', {
          title: confirmTarget?.title ?? '',
        })}
      />
    </section>
  );
}

function keyOf(d: Date): string {
  // Local YYYY-MM-DD — see `localDateKey` for why this can't be
  // toISOString().slice(0, 10).
  return localDateKey(d);
}

function groupEventsByDay(
  events: CalendarEvent[],
  days: Date[],
): Map<string, CalendarEvent[]> {
  const map = new Map<string, CalendarEvent[]>();
  days.forEach((d) => map.set(keyOf(d), []));
  events.forEach((ev) => {
    const k = keyOf(new Date(ev.start));
    const bucket = map.get(k);
    if (bucket) bucket.push(ev);
  });
  return map;
}
