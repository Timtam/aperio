import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isSameDay } from 'date-fns';

import { useAnnouncer } from '../../a11y/announcerContext';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import { useDateFormat } from '../../intl/dateFormat';
import {
  labelsLookup,
  resolveEventColor,
  resolveTaskColor,
} from '../../intl/eventColor';
import { eventCoversDay, multiDayInfo } from '../../intl/multiDay';
import {
  isExpandedOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '../../intl/recurrence';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { setEventDrag, setTaskDrag } from '../../state/moveActions';
import { useDialogState } from '../../state/dialogStateContext';
import { useEvents } from '../../state/useEvents';
import { useTaskListShowCompleted } from '../../state/useTaskListShowCompleted';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useTaskStatusToggle } from '../../state/useTaskStatusToggle';
import { useTasks } from '../../state/useTasks';
import { useViewState } from '../../state/viewStateContext';
import { visibleRange } from '../../state/viewMath';
import { localDateKey } from '../../intl/dateKey';
import {
  filterTasksOnDay,
  isDeadlineChip,
  mergeDayItems,
  taskTimeOnDay,
} from '../../intl/taskDay';
import { useCurrentUserByList } from '../../state/currentUser';
import {
  assigneeSuffix,
  priorityMarker,
  prioritySuffix,
  statusI18nKey,
  statusMarker,
  subtaskParentSuffix,
  subtaskProgressSuffix,
} from '../../intl/taskStatus';
import { duplicateEvent } from '../duplicateActions';
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
  const currentUserByList = useCurrentUserByList(tasks);
  // Hide tasks assigned to a concrete OTHER user from MY calendar (mine +
  // unassigned stay) — the day-start review's ownership filter (DESIGN §9.7).
  const meFor = useCallback(
    (listId: string) => currentUserByList[listId] ?? null,
    [currentUserByList],
  );
  const toggleTaskStatus = useTaskStatusToggle();
  const { shouldShow: shouldShowCompletedForList } =
    useTaskListShowCompleted();
  const { openForEvent: openEventMenu, openForTask: openTaskMenu } =
    useChipContextMenu();
  const { colorLabels, sectionColorById, sectionsByList, loadSections } =
    useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  // Load sections for the lists with tasks here so a colored section
  // cascades to its tasks in this view too (cached + cheap; empty for
  // section-less backends). Mirrors TaskView.
  const listIdsWithTasks = useMemo(
    () => Array.from(new Set(tasks.map((task) => task.list_id))),
    [tasks],
  );
  useEffect(() => {
    for (const listId of listIdsWithTasks) {
      if (!(listId in sectionsByList)) void loadSections(listId);
    }
  }, [listIdsWithTasks, sectionsByList, loadSections]);

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
        shouldShowCompletedForList,
        meFor,
      ),
    [tasks, anchor, shouldShowCompletedForList, meFor],
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
  const itemId = useCallback(
    (i: number) => `${idPrefix}-item-${i}`,
    [idPrefix],
  );
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const [confirmTarget, setConfirmTarget] = useState<CalendarEvent | null>(
    null,
  );
  const [scopeTarget, setScopeTarget] = useState<CalendarEvent | null>(null);

  const performDelete = useCallback(
    async (ev: CalendarEvent, scope: 'occurrence' | 'series') => {
      try {
        const occIso = occurrenceIsoOf(ev);
        if (scope === 'occurrence' && occIso) {
          await addEventExdate(seriesIdOf(ev), occIso, ev.calendar_id);
          announce(
            t('dialogs.event.occurrenceDeleted', { title: ev.title }),
          );
        } else {
          await deleteEventById(seriesIdOf(ev), ev.calendar_id);
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
    if (isExpandedOccurrence(ev) || ev.recurrence) {
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
        case 'ContextMenu':
        case 'F10': {
          if (e.key === 'F10' && !e.shiftKey) return;
          // Shift+F10 / Menu key: open the chip context menu near
          // the focused row, mirroring the platform convention.
          e.preventDefault();
          if (focusedItem) {
            const target = e.currentTarget as HTMLElement;
            const id = itemId(focusIndex);
            const node = target.ownerDocument?.getElementById(id);
            const rect = node?.getBoundingClientRect();
            const pos = rect
              ? { x: rect.left, y: rect.bottom }
              : undefined;
            if (focusedItem.kind === 'event') {
              void openEventMenu(focusedItem.event, pos);
            } else {
              void openTaskMenu(focusedItem.task, pos);
            }
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
      openEventMenu,
      openTaskMenu,
      itemId,
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
              // Pull the effective time-of-day via the shared helper;
              // it returns scheduled_time when on the scheduled day,
              // deadline_time when on the deadline day, with the
              // schedule winning on a same-day collision.
              const timeOnDay = taskTimeOnDay(task, dayKey);
              const timeStr = timeOnDay
                ? fmt.format(
                    new Date(`${dayKey}T${timeOnDay}`),
                    'p',
                  )
                : '';
              const color = resolveTaskColor(
                task,
                taskListById,
                labelById,
                sectionColorById,
              );
              const state = t(statusI18nKey(task.status));
              const priorityGlyph = priorityMarker(task.priority);
              return (
                <li
                  key={`task-${task.id}`}
                  id={itemId(i)}
                  role="option"
                  aria-selected={focused}
                  aria-label={
                    t('views.day.taskLabel', {
                      title: task.title,
                      time: timeStr,
                      state,
                      priority: prioritySuffix(t, task.priority),
                      progress: subtaskProgressSuffix(t, task.id, tasks),
                      assignee: assigneeSuffix(t, task.assignees),
                    }) + subtaskParentSuffix(t, task, tasks)
                  }
                  className={
                    'day-list__item day-list__item--task' +
                    (focused ? ' day-list__item--focused' : '') +
                    ` day-list__item--${task.status.replace('_', '-')}`
                  }
                  style={
                    color.hex
                      ? ({
                          '--event-color': color.hex,
                        } as React.CSSProperties)
                      : undefined
                  }
                  draggable
                  onDragStart={(dev) => {
                    setTaskDrag(
                      dev.dataTransfer,
                      task,
                      tasks.filter((c) => c.parent_id === task.id),
                    );
                  }}
                  onClick={() => setFocusIndex(i)}
                  onContextMenu={(ev) => {
                    ev.preventDefault();
                    ev.stopPropagation();
                    setFocusIndex(i);
                    void openTaskMenu(task);
                  }}
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
                      {statusMarker(task.status)}{' '}
                    </span>
                    {task.title}
                    {priorityGlyph && (
                      <span className="day-task__priority" aria-hidden="true">
                        {' '}
                        {priorityGlyph}
                      </span>
                    )}
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
            const ariaBase = t('views.day.eventLabel', {
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
                draggable
                onDragStart={(dev) => {
                  setEventDrag(dev.dataTransfer, ev);
                }}
                onClick={() => setFocusIndex(i)}
                onDoubleClick={(dcev) => {
                  // Open the editor, mirroring the task chips (single click
                  // just moves focus; the keyboard path is Enter).
                  dcev.stopPropagation();
                  setFocusIndex(i);
                  openEventDialog(ev);
                }}
                onContextMenu={(cmev) => {
                  cmev.preventDefault();
                  cmev.stopPropagation();
                  setFocusIndex(i);
                  void openEventMenu(ev);
                }}
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
              // "Due here" when the task is on this day because of its
              // deadline (not its scheduled day) — that chip is the
              // deadline marker ("fällig bis …"). A task scheduled today
              // stays a plain work chip even with a later deadline.
              const isBy = isDeadlineChip(task, dayKey);
              const labelKey = isBy
                ? 'views.week.taskChipBy'
                : 'views.week.taskChip';
              const color = resolveTaskColor(
                task,
                taskListById,
                labelById,
                sectionColorById,
              );
              const state = t(statusI18nKey(task.status));
              const priorityGlyph = priorityMarker(task.priority);
              return (
                <li key={task.id} className="day-tasks__item">
                  <button
                    type="button"
                    className={
                      'day-task' +
                      ` day-task--${task.status.replace('_', '-')}` +
                      (isBy ? ' day-task--by' : '')
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
                      } else if (
                        ev.key === 'ContextMenu' ||
                        (ev.shiftKey && ev.key === 'F10')
                      ) {
                        ev.preventDefault();
                        const rect = (
                          ev.currentTarget as HTMLElement
                        ).getBoundingClientRect();
                        void openTaskMenu(task, {
                          x: rect.left,
                          y: rect.bottom,
                        });
                      }
                    }}
                    onClick={() => openTaskDialog(task)}
                    onContextMenu={(ev) => {
                      ev.preventDefault();
                      ev.stopPropagation();
                      void openTaskMenu(task);
                    }}
                    style={
                      color.hex
                        ? ({
                            '--event-color': color.hex,
                          } as React.CSSProperties)
                        : undefined
                    }
                    aria-label={t(labelKey, {
                      title: task.title,
                      deadline: task.deadline_date
                        ? fmt.format(
                            new Date(`${task.deadline_date}T00:00:00`),
                            'PPP',
                          )
                        : '',
                      state,
                      priority: prioritySuffix(t, task.priority),
                      progress: subtaskProgressSuffix(t, task.id, tasks),
                      assignee: assigneeSuffix(t, task.assignees),
                    }) + subtaskParentSuffix(t, task, tasks)}
                  >
                    <span
                      className="day-task__marker day-task__marker--clickable"
                      aria-hidden="true"
                      onClick={(ev) => {
                        ev.stopPropagation();
                        void toggleTaskStatus(task);
                      }}
                    >
                      {statusMarker(task.status)}
                    </span>
                    <span className="day-task__title">{task.title}</span>
                    {priorityGlyph && (
                      <span className="day-task__priority" aria-hidden="true">
                        {priorityGlyph}
                      </span>
                    )}
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
