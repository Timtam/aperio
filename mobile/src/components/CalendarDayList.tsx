import type { NativeStackNavigationProp } from '@react-navigation/native-stack';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import type {
  ColorLabel,
  DayGridItem,
  MultiDayInfo,
  Section,
  Task,
  TaskList,
} from '@aperio/shared';
import {
  assigneeSuffix,
  daysCoveredKeys,
  expandAll,
  filterTasksOnDay,
  isDeadlineChip,
  localDateKey,
  mergeDayItems,
  multiDayInfo,
  occurrenceIsoOf,
  prioritySuffix,
  seriesIdOf,
  statusI18nKey,
  statusMarker,
  subtaskParentSuffix,
  subtaskProgressSuffix,
  taskTimeOnDay,
} from '@aperio/shared';

import {
  Calendar,
  CalendarEvent,
  getEvents,
  listCalendars,
} from '../api/calendar';
import {
  deleteTask as apiDeleteTask,
  getSections,
  getTasks,
  listTaskLists,
} from '../api/client';
import { listColorLabels } from '../api/colorLabels';
import { useTabBarInset } from '../hooks/useTabBarInset';
import { resolveEventColor } from '../intl/eventColor';
import { resolveTaskColor, sectionColorMap } from '../intl/taskColor';
import type { RootStackParamList } from '../navigation/types';
import { useCacheReload } from '../state/cacheObserver';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { useCurrentUserByList } from '../state/currentUser';
import { confirmDeleteEvent } from '../state/eventDeleteScope';
import { applyTaskToggle, statusAnnounce } from '../state/taskToggle';
import { useTaskListShowCompleted } from '../state/useTaskListShowCompleted';
import { useThemedStyles, type ThemeColors } from '../theme';

// The shared, screen-reader-first calendar day list — the rendering + data
// engine behind both the Week and Month views (and any future day-range view).
// Given the visible `days` and the `range` covering them, it loads everything
// (calendars + palette + lists + tasks + sections), expands recurring events,
// and renders one accessible section per day: a header announcing the day's
// item count, then that day's all-day events, its timed events + timed tasks
// merged chronologically (mergeDayItems), then its untimed tasks. Behaviour
// parity with the desktop comes from reusing the SAME shared domain logic
// (expandAll, daysCoveredKeys/multiDayInfo, filterTasksOnDay/mergeDayItems/
// taskTimeOnDay). Both audiences: coloured dots for sighted users, the bound
// label's NAME folded into every accessible label (WCAG 1.4.1). Event rows
// offer edit + delete; task rows complete (shared status cascade) / edit /
// delete. The owning screen supplies the day window + the chrome (nav/header).

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Local Date at `key`'s `HH:MM[:SS]` time-of-day (for localized formatting). */
function buildTimeDate(key: string, time: string): Date {
  const [hh, mm, ss] = time.split(':').map((n) => Number(n));
  const [y, mo, d] = key.split('-').map((n) => Number(n));
  return new Date(y, mo - 1, d, hh ?? 0, mm ?? 0, ss ?? 0);
}

interface DayBucket {
  key: string;
  date: Date;
  allDay: CalendarEvent[];
  timed: DayGridItem<CalendarEvent, Task>[];
  untimed: Task[];
  count: number;
}

export interface CalendarDayListProps {
  /** The owning screen's navigation (for the editor routes + focus reload). */
  navigation: NativeStackNavigationProp<RootStackParamList>;
  /** The visible days (local midnights), in order. */
  days: Date[];
  /** The instant range covering `days` (for the event/task fetch + expansion). */
  range: { start: Date; end: Date };
  /** accessibilityLabel for the list (e.g. "Week grid" / "Month grid"). */
  gridLabel: string;
  /** Shown when the window has no events or tasks. */
  emptyText: string;
  /** i18n key for the per-day header announce (`{{day}}, {{count}} items`). */
  dayAnnounceKey: string;
  /**
   * Render the per-day header row (the date + item count). Default `true`.
   * The single-day view passes `false`: it already shows the date in its own
   * nav-bar heading, so the list's per-day header would be a redundant second
   * heading for a screen-reader user.
   */
  showDayHeaders?: boolean;
}

export function CalendarDayList({
  navigation,
  days,
  range,
  gridLabel,
  emptyText,
  dayAnnounceKey,
  showDayHeaders = true,
}: CalendarDayListProps) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const tabBarInset = useTabBarInset();
  const { hidden: hiddenCalendars } = useCalendarVisibility();

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [taskLists, setTaskLists] = useState<TaskList[]>([]);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [sections, setSections] = useState<Section[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const tr = useCallback(
    (key: string, vars?: Record<string, unknown>): string => t(key, vars) as string,
    [t],
  );
  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const dayKeys = useMemo(() => days.map(localDateKey), [days]);

  const calendarsById = useMemo(
    () => new Map(calendars.map((c) => [c.id, c])),
    [calendars],
  );
  const labelsById = useMemo(
    () => new Map(colorLabels.map((l) => [l.id, l])),
    [colorLabels],
  );
  const listsById = useMemo(
    () => new Map(taskLists.map((l) => [l.id, l])),
    [taskLists],
  );
  const sectionColorById = useMemo(
    () => sectionColorMap(sections, labelsById),
    [sections, labelsById],
  );
  const readOnlyIds = useMemo(
    () => new Set(calendars.filter((c) => c.read_only).map((c) => c.id)),
    [calendars],
  );

  const fmtFullDate = useCallback(
    (d: Date) =>
      d.toLocaleDateString(i18n.language, {
        weekday: 'long',
        year: 'numeric',
        month: 'long',
        day: 'numeric',
      }),
    [i18n.language],
  );
  const fmtDateOnly = useMemo(() => {
    const f = new Intl.DateTimeFormat(i18n.language, {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    });
    return (iso: string) => f.format(new Date(iso));
  }, [i18n.language]);
  const fmtTime = useCallback(
    (d: Date) =>
      d.toLocaleTimeString(i18n.language, { hour: '2-digit', minute: '2-digit' }),
    [i18n.language],
  );

  const eventTimeLabel = useCallback(
    (ev: CalendarEvent): string => {
      if (ev.all_day) return t('views.allDay');
      return `${fmtTime(new Date(ev.start))}–${fmtTime(new Date(ev.end))}`;
    },
    [fmtTime, t],
  );

  // A request-epoch guard: the latest load wins. Changing the day window (e.g.
  // the week-start pref resolving async, or a prev/next step) recomputes `range`
  // and re-fires load while an earlier fetch may still be in flight; without
  // this, a slow earlier resolution could overwrite the newer window's data and
  // leave events mismatched against the day headers (derived from `days`).
  const reqToken = useRef(0);
  const load = useCallback(async () => {
    const token = (reqToken.current += 1);
    setLoading(true);
    setError(null);
    try {
      // listCalendars also primes the Host's route map (getEvents routes by
      // calendar id), so it must resolve before the per-calendar fetch. Palette,
      // lists are best-effort — a failure just drops the colour/task overlay.
      const [cals, labels, lists] = await Promise.all([
        listCalendars(),
        listColorLabels().catch(() => [] as ColorLabel[]),
        listTaskLists().catch(() => [] as TaskList[]),
      ]);
      const startIso = range.start.toISOString();
      const endIso = range.end.toISOString();
      const [perCalendar, perList, perListSections] = await Promise.all([
        Promise.all(
          cals.map((c) =>
            getEvents({ calendar_id: c.id, start: startIso, end: endIso }).catch(() => []),
          ),
        ),
        Promise.all(lists.map((l) => getTasks(l.id).catch(() => [] as Task[]))),
        Promise.all(lists.map((l) => getSections(l.id).catch(() => [] as Section[]))),
      ]);
      // A newer load superseded this one — drop these stale results.
      if (reqToken.current !== token) return;
      setCalendars(cals);
      setColorLabels(labels);
      setTaskLists(lists);
      // Expand recurring series across the whole window so an event recurring
      // mid-window isn't invisible after its first occurrence (rrule + EXDATE).
      setEvents(expandAll(perCalendar.flat(), { start: range.start, end: range.end }));
      setTasks(perList.flat());
      setSections(perListSections.flat());
    } catch (err) {
      if (reqToken.current !== token) return;
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      if (reqToken.current === token) setLoading(false);
    }
  }, [announce, range, t]);

  // Reload when the window changes or the screen regains focus (after an editor).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // Live-update while focused: this view loads BOTH events and tasks per day, so
  // re-read on either an external calendar- or task-cache refresh (the root
  // observer already announced it politely; same `load` covers both).
  useCacheReload('calendar', load);
  useCacheReload('tasks', load);

  // Per-list "show completed in calendar" opt-in (default hide). Shared store, so
  // a toggle on the Lists screen reflects here on the next render.
  const { shouldShow: showCompletedForList } = useTaskListShowCompleted();
  const currentUserByList = useCurrentUserByList(tasks);
  // Hide tasks assigned to a concrete OTHER user from MY calendar (mine +
  // unassigned stay) — the day-start review's ownership filter (DESIGN §9.7).
  const meFor = useCallback(
    (listId: string) => currentUserByList[listId] ?? null,
    [currentUserByList],
  );

  // Bucket each day's events + tasks. Completed tasks stay hidden by default;
  // per list, the "show completed in calendar" opt-in (toggled on the Lists
  // screen, shared with the desktop's `tasks.showCompletedInCalendar` pref)
  // keeps them visible.
  // Calendars the user hid (the Calendars-screen toggles) drop out of the view.
  const visibleEvents = useMemo(
    () => events.filter((ev) => !hiddenCalendars.has(ev.calendar_id)),
    [events, hiddenCalendars],
  );
  const buckets = useMemo<DayBucket[]>(() => {
    return days.map((date, i) => {
      const key = dayKeys[i];
      const allDay = visibleEvents.filter(
        (ev) => ev.all_day && daysCoveredKeys(ev).includes(key),
      );
      const timedEvents = visibleEvents.filter(
        (ev) => !ev.all_day && localDateKey(new Date(ev.start)) === key,
      );
      const dayTasks = filterTasksOnDay(tasks, key, showCompletedForList, meFor);
      const { timed, untimed } = mergeDayItems(
        timedEvents,
        dayTasks,
        key,
        (ev) => new Date(ev.start).getTime(),
      );
      return {
        key,
        date,
        allDay,
        timed,
        untimed,
        count: allDay.length + timed.length + untimed.length,
      };
    });
  }, [days, dayKeys, visibleEvents, tasks, showCompletedForList, meFor]);

  const totalItems = useMemo(
    () => buckets.reduce((sum, b) => sum + b.count, 0),
    [buckets],
  );

  const editEvent = useCallback(
    (ev: CalendarEvent) =>
      navigation.navigate('EventEditor', {
        eventId: seriesIdOf(ev),
        calendarId: ev.calendar_id,
        occurrence: occurrenceIsoOf(ev),
      }),
    [navigation],
  );

  // Move / copy to another calendar — pass the full (possibly expanded) row so
  // the modal can offer the occurrence-vs-series scope.
  const moveCopyEvent = useCallback(
    (ev: CalendarEvent) => navigation.navigate('MoveCopy', { kind: 'event', event: ev }),
    [navigation],
  );

  const removeEvent = useCallback(
    (ev: CalendarEvent) =>
      confirmDeleteEvent(
        ev,
        t,
        (message) => {
          announce(message);
          void load();
        },
        (message) => {
          setError(message);
          announce(t('mobile.error', { message }));
        },
      ),
    [announce, load, t],
  );

  const openTask = useCallback(
    (task: Task) =>
      navigation.navigate('TaskEditor', { taskId: task.id, listId: task.list_id }),
    [navigation],
  );

  const moveCopyTask = useCallback(
    (task: Task) =>
      navigation.navigate('MoveCopy', {
        kind: 'task',
        taskId: task.id,
        listId: task.list_id,
      }),
    [navigation],
  );

  // Per-day "+ new event": seed a new event on that day (first writable
  // calendar) — the mobile twin of the desktop's day-anchored create.
  const firstWritableCalendarId = useMemo(
    () =>
      calendars.find((c) => !c.read_only && !hiddenCalendars.has(c.id))?.id ??
      calendars.find((c) => !c.read_only)?.id ??
      null,
    [calendars, hiddenCalendars],
  );
  const addEventOnDay = useCallback(
    (dayKey: string) => {
      if (firstWritableCalendarId == null) return;
      navigation.navigate('EventEditor', {
        eventId: null,
        calendarId: firstWritableCalendarId,
        anchor: dayKey,
      });
    },
    [firstWritableCalendarId, navigation],
  );

  // Day-anchored task create — seed the new task's scheduled day from the tapped
  // calendar day (the task twin of addEventOnDay). Lands in the first writable
  // task list; the editor's list picker can move it.
  const firstWritableTaskListId = useMemo(
    () => taskLists.find((l) => !l.read_only)?.id ?? null,
    [taskLists],
  );
  const addTaskOnDay = useCallback(
    (dayKey: string) => {
      if (firstWritableTaskListId == null) return;
      navigation.navigate('TaskEditor', {
        taskId: null,
        listId: firstWritableTaskListId,
        initialScheduledDate: dayKey,
      });
    },
    [firstWritableTaskListId, navigation],
  );

  // Check off a task via the shared toggle path (honours the synced
  // task-behaviour knobs), then reload. Like the other calendar screens, focus
  // is not forcibly restored across the reload.
  const toggleTask = useCallback(
    async (task: Task) => {
      try {
        const next = await applyTaskToggle(task, listsById.get(task.list_id), tasks);
        if (next == null) return;
        announce(statusAnnounce(t, next, task.title));
        await load();
      } catch (err) {
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, listsById, load, t, tasks],
  );

  const removeTask = useCallback(
    (task: Task) => {
      Alert.alert(
        t('dialogs.confirm.deleteTaskTitle'),
        t('dialogs.confirm.deleteTaskMessage', { title: task.title }),
        [
          { text: t('dialogs.confirm.cancel'), style: 'cancel' },
          {
            text: t('mobile.delete'),
            style: 'destructive',
            onPress: () => {
              void (async () => {
                try {
                  await apiDeleteTask(task.id, task.list_id);
                  announce(t('mobile.deleted', { title: task.title }));
                  await load();
                } catch (err) {
                  announce(t('mobile.error', { message: errorMessage(err) }));
                }
              })();
            },
          },
        ],
      );
    },
    [announce, load, t],
  );

  // ── Accessible labels ──────────────────────────────────────────────────────

  const eventLabel = useCallback(
    (ev: CalendarEvent, span: MultiDayInfo | null): string => {
      let label = t('views.week.eventLabel', {
        title: ev.title,
        time: eventTimeLabel(ev),
        calendar: calendarsById.get(ev.calendar_id)?.name ?? '—',
      });
      if (span) {
        label += t('views.multiDaySuffix', { day: span.dayIndex, total: span.totalDays });
      }
      const colour = resolveEventColor(ev, calendarsById, labelsById);
      if (colour.labelName) {
        label += t('mobile.colorLabelSuffix', { name: colour.labelName });
      }
      return label;
    },
    [calendarsById, eventTimeLabel, labelsById, t],
  );

  const taskLabel = useCallback(
    (task: Task, key: string, colourName: string | null): string => {
      const time = taskTimeOnDay(task, key);
      const common = {
        title: task.title,
        state: t(statusI18nKey(task.status)),
        priority: prioritySuffix(tr, task.priority),
        progress: subtaskProgressSuffix(tr, task.id, tasks),
        assignee: assigneeSuffix(tr, task.assignees),
      };
      let label: string;
      if (time) {
        label = t('views.week.taskChipTimed', {
          ...common,
          time: fmtTime(buildTimeDate(key, time)),
        });
      } else if (isDeadlineChip(task, key) && task.deadline_date) {
        label = t('views.week.taskChipBy', {
          ...common,
          deadline: fmtDateOnly(task.deadline_date),
        });
      } else {
        label = t('views.week.taskChip', common);
      }
      if (colourName) {
        label += t('mobile.colorLabelSuffix', { name: colourName });
      }
      return label + subtaskParentSuffix(tr, task, tasks);
    },
    [fmtDateOnly, fmtTime, t, tasks, tr],
  );

  const renderEventRow = (ev: CalendarEvent, day: Date, span: MultiDayInfo | null) => {
    const rowKey = `e-${ev.id}@${localDateKey(day)}`;
    const hex = resolveEventColor(ev, calendarsById, labelsById).hex;
    const dot =
      hex != null ? (
        <View
          accessible={false}
          importantForAccessibility="no"
          style={[styles.colorDot, { backgroundColor: hex }]}
        />
      ) : null;
    const badge = span
      ? ` ${t('views.multiDayCompact', { day: span.dayIndex, total: span.totalDays })}`
      : '';
    if (readOnlyIds.has(ev.calendar_id)) {
      return (
        <View
          key={rowKey}
          accessible
          accessibilityRole="text"
          accessibilityLabel={eventLabel(ev, span)}
          style={styles.row}
        >
          {dot}
          <View style={styles.rowText}>
            <Text style={styles.itemTitle} importantForAccessibility="no">
              {ev.title}
              {badge}
            </Text>
            <Text style={styles.itemMeta} importantForAccessibility="no">
              {eventTimeLabel(ev)}
            </Text>
          </View>
        </View>
      );
    }
    return (
      <View
        key={rowKey}
        accessible
        accessibilityRole="button"
        accessibilityLabel={eventLabel(ev, span)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityActions={[
          { name: 'activate', label: t('mobile.editTaskLabel') },
          { name: 'moveCopy', label: t('mobile.moveCopy') },
          { name: 'delete', label: t('dialogs.event.delete') },
        ]}
        onAccessibilityAction={(e) => {
          if (e.nativeEvent.actionName === 'delete') removeEvent(ev);
          else if (e.nativeEvent.actionName === 'moveCopy') moveCopyEvent(ev);
          else editEvent(ev);
        }}
        style={styles.row}
      >
        {dot}
        <Pressable accessible={false} onPress={() => editEvent(ev)} style={styles.rowText}>
          <Text style={styles.itemTitle} importantForAccessibility="no">
            {ev.title}
            {badge}
          </Text>
          <Text style={styles.itemMeta} importantForAccessibility="no">
            {eventTimeLabel(ev)}
          </Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${t('dialogs.event.delete')}: ${ev.title}`}
          onPress={() => removeEvent(ev)}
          style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
        >
          <Text style={styles.deleteButtonText}>{t('dialogs.event.delete')}</Text>
        </Pressable>
      </View>
    );
  };

  const renderTaskRow = (task: Task, key: string) => {
    const done = task.status === 'completed';
    const resolved = resolveTaskColor(task, listsById, labelsById, sectionColorById);
    const hex = resolved.hex;
    // Day-aware visible meta (the row's reason for being on THIS day): its time
    // if timed here, else a "due"/"planned" marker for this day. (Task-level
    // describeDue would show the scheduled day on a deadline-day row.)
    const time = taskTimeOnDay(task, key);
    const meta = time
      ? fmtTime(buildTimeDate(key, time))
      : isDeadlineChip(task, key)
        ? t('views.tasks.dueDeadline', { date: fmtDateOnly(key) })
        : t('views.tasks.dueScheduled', { date: fmtDateOnly(key) });
    return (
      <View
        key={`t-${task.id}@${key}`}
        accessible
        accessibilityRole="button"
        accessibilityLabel={taskLabel(task, key, resolved.labelName)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityActions={[
          { name: 'toggle', label: done ? t('mobile.reopen') : t('mobile.complete') },
          { name: 'edit', label: t('mobile.rename') },
          { name: 'moveCopy', label: t('mobile.moveCopy') },
          { name: 'delete', label: t('mobile.delete') },
        ]}
        onAccessibilityAction={(e) => {
          const name = e.nativeEvent.actionName;
          if (name === 'toggle') void toggleTask(task);
          else if (name === 'delete') removeTask(task);
          else if (name === 'moveCopy') moveCopyTask(task);
          else openTask(task);
        }}
        style={styles.row}
      >
        {/* Sighted tap target to complete/reopen the task; the row otherwise
            opens the task on tap, so the marker needs its own Pressable. SR
            users use the row's "toggle" custom action, so this stays out of the
            accessibility tree. */}
        <Pressable
          accessible={false}
          importantForAccessibility="no"
          onPress={() => void toggleTask(task)}
          hitSlop={10}
          style={({ pressed }) => [styles.taskCheckButton, pressed && styles.pressed]}
        >
          <Text style={styles.taskCheck} importantForAccessibility="no">
            {statusMarker(task.status)}
          </Text>
        </Pressable>
        {hex != null && (
          <View
            accessible={false}
            importantForAccessibility="no"
            style={[styles.colorDot, { backgroundColor: hex }]}
          />
        )}
        <Pressable accessible={false} onPress={() => openTask(task)} style={styles.rowText}>
          <Text
            style={[styles.itemTitle, done && styles.itemTitleDone]}
            importantForAccessibility="no"
          >
            {task.parent_id ? '↳ ' : ''}
            {task.title}
          </Text>
          <Text style={styles.itemMeta} importantForAccessibility="no">
            {meta}
          </Text>
        </Pressable>
      </View>
    );
  };

  // The error line rides above whatever else renders (the list still shows when
  // a reload after a mutation fails but stale data remains).
  return (
    <>
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('views.loading')}>
          {t('views.loading')}
        </Text>
      ) : totalItems === 0 ? (
        <Text style={styles.muted} accessibilityRole="text">
          {emptyText}
        </Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          accessibilityLabel={gridLabel}
          style={styles.scroll}
          contentContainerStyle={[styles.list, { paddingBottom: tabBarInset }]}
          keyboardShouldPersistTaps="handled"
        >
          {buckets.map((b) => {
            const rows: ReactNode[] = [];
            if (showDayHeaders) {
              rows.push(
                <Text
                  key={`h-${b.key}`}
                  accessibilityRole="header"
                  accessibilityLabel={t(dayAnnounceKey, {
                    day: fmtFullDate(b.date),
                    count: b.count,
                  })}
                  style={styles.dayHeader}
                >
                  {fmtFullDate(b.date)}
                </Text>,
              );
            }
            for (const ev of b.allDay) {
              rows.push(renderEventRow(ev, b.date, multiDayInfo(ev, b.date)));
            }
            for (const item of b.timed) {
              rows.push(
                item.kind === 'event'
                  ? renderEventRow(item.event, b.date, null)
                  : renderTaskRow(item.task, b.key),
              );
            }
            for (const task of b.untimed) {
              rows.push(renderTaskRow(task, b.key));
            }
            return (
              <View key={b.key} style={styles.daySection}>
                {rows}
                {firstWritableCalendarId != null && (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={`${t('toolbar.newEvent')}, ${fmtFullDate(b.date)}`}
                    onPress={() => addEventOnDay(b.key)}
                    style={({ pressed }) => [
                      styles.newEventButton,
                      pressed && styles.pressed,
                    ]}
                  >
                    <Text style={styles.newEventText}>
                      {t('toolbar.newEvent')}
                    </Text>
                  </Pressable>
                )}
                {firstWritableTaskListId != null && (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={`${t('toolbar.newTask')}, ${fmtFullDate(b.date)}`}
                    onPress={() => addTaskOnDay(b.key)}
                    style={({ pressed }) => [
                      styles.newEventButton,
                      pressed && styles.pressed,
                    ]}
                  >
                    <Text style={styles.newEventText}>
                      {t('toolbar.newTask')}
                    </Text>
                  </Pressable>
                )}
              </View>
            );
          })}
        </ScrollView>
      )}
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    scroll: { flex: 1 },
    list: { gap: 8, padding: 16 },
    daySection: { gap: 8 },
    newEventButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
      alignItems: 'center',
    },
    newEventText: { fontSize: 15, fontWeight: '600', color: c.link },
    dayHeader: {
      fontSize: 15,
      fontWeight: '700',
      color: c.textLabel,
      marginTop: 8,
      marginBottom: 2,
    },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowText: { flex: 1, gap: 2 },
    taskCheckButton: { borderRadius: 8, padding: 4 },
    taskCheck: { fontSize: 20, width: 26, textAlign: 'center', color: c.textPrimary },
    colorDot: {
      width: 12,
      height: 12,
      borderRadius: 6,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    itemTitle: { fontSize: 18, fontWeight: '600', color: c.textPrimary },
    itemTitleDone: { textDecorationLine: 'line-through', color: c.textSecondary },
    itemMeta: { fontSize: 14, color: c.textSecondary },
    deleteButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    deleteButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
  });
