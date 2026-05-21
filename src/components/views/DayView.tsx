import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isSameDay } from 'date-fns';

import { useAnnouncer } from '../../a11y/Announcer';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import { useDateFormat } from '../../intl/dateFormat';
import {
  labelsLookup,
  resolveEventColor,
  resolveTaskColor,
} from '../../intl/eventColor';
import { eventCoversDay, multiDayInfo } from '../../intl/multiDay';
import { useCalendarStore } from '../../state/CalendarStore';
import { useDialogState } from '../../state/DialogState';
import { useEvents } from '../../state/useEvents';
import { useTaskListShowCompleted } from '../../state/useTaskListShowCompleted';
import { useTaskStatusToggle } from '../../state/useTaskStatusToggle';
import { useTasks } from '../../state/useTasks';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';
import { localDateKey } from '../../intl/dateKey';
import {
  filterTasksOnDay,
  mergeDayItems,
  todayIsoKey,
} from '../../intl/taskDay';
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
  const { openEventDialog, openTaskDialog, openMoveCopy, invalidateData } =
    useDialogState();

  const range = useMemo(() => visibleRange('day', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);
  const { tasks, taskListById } = useTasks();
  const toggleTaskStatus = useTaskStatusToggle();
  const { shouldShow: shouldShowCompletedForList } =
    useTaskListShowCompleted();
  const { colorLabels } = useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  // Pick up multi-day all-day events on every day of their span — see
  // intl/multiDay for the rationale.
  const dayEvents = useMemo(
    () => events.filter((ev) => eventCoversDay(ev, anchor)),
    [events, anchor],
  );

  // Tasks visible on this day (§9.4): scheduled, On-deadline, or
  // By-deadline window. Same filter as WeekView, including the
  // per-list "show completed" sidebar toggle.
  const dayTasks = useMemo(
    () =>
      filterTasksOnDay(
        tasks,
        localDateKey(anchor),
        todayIsoKey(),
        shouldShowCompletedForList,
      ),
    [tasks, anchor, shouldShowCompletedForList],
  );

  // Split tasks into "timed" (carry a deadline_time on this specific
  // day) and "untimed" (everything else). Timed tasks slot into the
  // events listbox sorted by time so a 14:00 task deadline appears
  // between a 13:00 meeting and a 15:00 standup — the bug fix the
  // user asked for. Untimed tasks still render in the dedicated
  // section below the listbox.
  const dayKey = useMemo(() => localDateKey(anchor), [anchor]);
  const { timedItems, untimedTasks } = useMemo(() => {
    const { timed, untimed } = mergeDayItems(
      dayEvents,
      dayTasks,
      dayKey,
      (ev) => new Date(ev.start).getTime(),
    );
    return { timedItems: timed, untimedTasks: untimed };
  }, [dayEvents, dayTasks, dayKey]);

  const [focusIndex, setFocusIndex] = useState(0);

  // If the day changes (or events arrive) and the previous focus index
  // is past the end of the new list, snap back to the last valid item.
  useEffect(() => {
    if (focusIndex >= timedItems.length) {
      setFocusIndex(Math.max(0, timedItems.length - 1));
    }
  }, [timedItems.length, focusIndex]);

  // Loading indicator is gated on `showLoading` (the deferred
  // variant), not the raw `loading` flag. That way a sub-200ms
  // local fetch — which is what happens whenever the user switches
  // views with cached data — never flashes "Lädt …". Only genuine
  // waits (CalDAV cold start, slow iCal feed) cross the threshold
  // and surface the indicator + the SR announcement.
  const showLoading = useDeferredLoading(loading);
  useEffect(() => {
    if (showLoading) announce(t('views.loading'));
  }, [showLoading, announce, t]);

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
      const focusedItem = timedItems[focusIndex];
      // Ctrl+D duplicates the focused event in place; Shift+M opens
      // the move/copy dialog. Bare keys cover the rest of the listbox
      // navigation. Both shortcuts are event-only — duplicating a
      // task deadline doesn't have a clear meaning, and Move/Copy of
      // tasks lives in TaskView.
      if (e.ctrlKey || e.metaKey) {
        if (e.key.toLowerCase() === 'd' && !e.shiftKey && !e.altKey) {
          e.preventDefault();
          if (focusedItem?.kind === 'event') {
            const ev = focusedItem.event;
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
        if (focusedItem?.kind === 'event') {
          openMoveCopy({ kind: 'event', event: focusedItem.event });
        }
        return;
      }
      if (timedItems.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusIndex((i) => Math.min(i + 1, timedItems.length - 1));
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
          setFocusIndex(timedItems.length - 1);
          return;
        case 'Enter': {
          // Enter always opens the editor — for events the
          // EventDialog, for tasks the TaskDialog.
          e.preventDefault();
          if (focusedItem?.kind === 'event') {
            openEventDialog(focusedItem.event);
          } else if (focusedItem?.kind === 'task') {
            openTaskDialog(focusedItem.task);
          }
          return;
        }
        case ' ':
        case 'Spacebar': {
          // Space opens events (no other meaningful action) but
          // toggles done/open on tasks. Matches TaskView's existing
          // Space-to-check convention and the user-visible
          // checkbox marker on the chip.
          e.preventDefault();
          if (focusedItem?.kind === 'event') {
            openEventDialog(focusedItem.event);
          } else if (focusedItem?.kind === 'task') {
            void toggleTaskStatus(focusedItem.task);
          }
          return;
        }
        case 'Delete':
        case 'Backspace': {
          e.preventDefault();
          // Tasks go through their own delete flow via TaskDialog;
          // the listbox Delete shortcut only nukes events, matching
          // WeekView semantics.
          if (focusedItem?.kind === 'event') {
            requestDelete(focusedItem.event);
          }
          return;
        }
        default:
          return;
      }
    },
    [
      timedItems,
      focusIndex,
      openEventDialog,
      openTaskDialog,
      openMoveCopy,
      announce,
      t,
      requestDelete,
      toggleTaskStatus,
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

      {showLoading && (
        <p className="view__loading" aria-hidden="true">
          {t('views.loading')}
        </p>
      )}

      <ul
        ref={listRef}
        role="listbox"
        tabIndex={0}
        aria-label={t('views.day.eventList')}
        aria-activedescendant={
          timedItems.length > 0 ? itemId(focusIndex) : undefined
        }
        onKeyDown={handleKeyDown}
        className="day-list"
      >
        {timedItems.length === 0 ? (
          <li role="presentation" className="day-list__empty">
            {t('views.day.empty')}
          </li>
        ) : (
          timedItems.map((item, i) => {
            const focused = i === focusIndex;
            if (item.kind === 'task') {
              const task = item.task;
              const timeStr = task.deadline_time
                ? fmt.format(
                    new Date(`${dayKey}T${task.deadline_time}`),
                    'p',
                  )
                : '';
              const color = resolveTaskColor(task, taskListById, labelById);
              const isCompleted = task.status === 'completed';
              const state = t(
                isCompleted
                  ? 'views.week.taskStateDone'
                  : 'views.week.taskStateOpen',
              );
              return (
                <li
                  key={`task-${task.id}`}
                  id={itemId(i)}
                  role="option"
                  aria-selected={focused}
                  aria-label={
                    color.labelName
                      ? t('views.day.taskLabelWithLabel', {
                          title: task.title,
                          time: timeStr,
                          label: color.labelName,
                          state,
                        })
                      : t('views.day.taskLabel', {
                          title: task.title,
                          time: timeStr,
                          state,
                        })
                  }
                  className={
                    'day-list__item day-list__item--task' +
                    (focused ? ' day-list__item--focused' : '') +
                    (isCompleted ? ' day-list__item--completed' : '')
                  }
                  style={
                    color.hex
                      ? ({
                          '--event-color': color.hex,
                        } as React.CSSProperties)
                      : undefined
                  }
                  onClick={() => setFocusIndex(i)}
                >
                  <span className="day-list__time">{timeStr}</span>
                  <span className="day-list__title">
                    <span
                      className="day-task__marker day-task__marker--clickable"
                      aria-hidden="true"
                      // Mouse: clicking the marker toggles the task
                      // without selecting the row, so users don't
                      // have to round-trip through the dialog just
                      // to check something off.
                      onClick={(ev) => {
                        ev.stopPropagation();
                        void toggleTaskStatus(task);
                      }}
                    >
                      {isCompleted ? '☑ ' : '☐ '}
                    </span>
                    {task.title}
                  </span>
                </li>
              );
            }
            const ev = item.event;
            const cal = calendarById.get(ev.calendar_id);
            const color = resolveEventColor(ev, calendarById, labelById);
            const startStr = fmt.format(new Date(ev.start), 'p');
            const endStr = fmt.format(new Date(ev.end), 'p');
            const span = multiDayInfo(ev, anchor);
            const ariaBase = color.labelName
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
            const aria = span
              ? ariaBase +
                t('views.multiDaySuffix', {
                  day: span.dayIndex,
                  total: span.totalDays,
                })
              : ariaBase;
            return (
              <li
                key={ev.id}
                id={itemId(i)}
                role="option"
                aria-selected={focused}
                aria-label={aria}
                className={
                  'day-list__item' +
                  (focused ? ' day-list__item--focused' : '') +
                  (span ? ' day-list__item--multiday' : '')
                }
                style={
                  color.hex
                    ? ({ '--event-color': color.hex } as React.CSSProperties)
                    : undefined
                }
                onClick={() => setFocusIndex(i)}
              >
                <span className="day-list__time">
                  {ev.all_day ? t('views.allDay') : `${startStr} – ${endStr}`}
                </span>
                <span className="day-list__title">
                  {ev.title}
                  {span && (
                    <span className="day-list__span">
                      {' '}
                      {t('views.multiDayCompact', {
                        day: span.dayIndex,
                        total: span.totalDays,
                      })}
                    </span>
                  )}
                </span>
              </li>
            );
          })
        )}
      </ul>

      {/* §9.4: untimed tasks on this day, rendered as natural-Tab-
          order buttons below the listbox. Tasks with a concrete
          deadline_time were already interleaved with events in the
          listbox above (sorted by time), so only scheduled-only
          tasks and By-window intermediate days surface here. Click
          / Enter / Space opens the TaskDialog; status toggles live
          in TaskView (the dedicated keyboard surface). */}
      {untimedTasks.length > 0 && (
        <section
          className="day-tasks"
          aria-label={t('views.day.tasksHeading')}
        >
          <h3 className="day-tasks__heading">
            {t('views.day.tasksHeading')}
          </h3>
          <ul className="day-tasks__list">
            {untimedTasks.map((task) => {
              const labelKey =
                task.deadline_type === 'by' && task.deadline_date
                  ? 'views.week.taskChipBy'
                  : 'views.week.taskChip';
              const color = resolveTaskColor(task, taskListById, labelById);
              const isCompleted = task.status === 'completed';
              const state = t(
                isCompleted
                  ? 'views.week.taskStateDone'
                  : 'views.week.taskStateOpen',
              );
              return (
                <li key={task.id} className="day-tasks__item">
                  <button
                    type="button"
                    className={
                      'day-task' +
                      (isCompleted ? ' day-task--completed' : '') +
                      (task.deadline_type === 'by'
                        ? ' day-task--by'
                        : '')
                    }
                    // Default <button> would fire onClick on both
                    // Space and Enter. We need different actions:
                    // Space toggles done (matches the visual ☐/☑),
                    // Enter opens the editor. Intercept here.
                    onKeyDown={(ev) => {
                      if (ev.key === ' ' || ev.key === 'Spacebar') {
                        ev.preventDefault();
                        void toggleTaskStatus(task);
                      } else if (ev.key === 'Enter') {
                        ev.preventDefault();
                        openTaskDialog(task);
                      }
                    }}
                    onClick={() => openTaskDialog(task)}
                    style={
                      color.hex
                        ? ({
                            '--event-color': color.hex,
                          } as React.CSSProperties)
                        : undefined
                    }
                    aria-label={
                      color.labelName
                        ? t(`${labelKey}WithLabel`, {
                            title: task.title,
                            deadline: task.deadline_date ?? '',
                            label: color.labelName,
                            state,
                          })
                        : t(labelKey, {
                            title: task.title,
                            deadline: task.deadline_date ?? '',
                            state,
                          })
                    }
                  >
                    <span
                      className="day-task__marker day-task__marker--clickable"
                      aria-hidden="true"
                      onClick={(ev) => {
                        ev.stopPropagation();
                        void toggleTaskStatus(task);
                      }}
                    >
                      {isCompleted ? '☑' : '☐'}
                    </span>
                    <span className="day-task__title">{task.title}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      )}

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
