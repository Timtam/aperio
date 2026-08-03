import { useCallback, useEffect } from 'react';
import { AppState } from 'react-native';

import { buildWidgetSnapshot } from '@aperio/shared';
import type { ColorLabel, Task, TaskList, TaskUser } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import i18n from '../../i18n';
import { getEvents, listCalendars, type Calendar, type CalendarEvent } from '../api/calendar';
import { getTasks, listTaskLists } from '../api/client';
import { listColorLabels } from '../api/colorLabels';
import { resolveEventColor } from '../intl/eventColor';
import { useCurrentDayKey } from '../hooks/useCurrentDayKey';
import { getHiddenCalendars } from './calendarVisibility';
import { currentUserForList } from './currentUser';
import { consumeQueuedActionsApplied, drainQueuedActions } from './queuedActions';
import { useCacheReload } from './cacheObserver';
import { subscribeCalendarChanged } from './calendarMutations';
import { whenStartupSettled } from './startupGate';
import { useTaskStore } from './taskStoreContext';

// Keeps the home-screen widgets fed. The app derives what they show — it is the
// only process that can, holding both the per-device visibility settings and
// the shared helpers the views themselves render from — and drops the result in
// the App Group as one small JSON document.
//
// Deliberately built on the SAME triggers as the app-icon badge and the reminder
// scheduler: local mutation, external-cache refresh, foreground-resume, the
// midnight rollover, and the OS background-sync round. Those are exactly the
// moments the answer can have changed, and they are already proven to fire.
//
// Everything here is best-effort and silent. A widget is a convenience; a
// failure to refresh one must never be visible in the app.

/** How far ahead the snapshot reaches. The widget re-renders on the system's
 *  schedule long after this ran, so it needs enough future to answer "what is
 *  next" without the app — 7 days matches the reminder scheduler's horizon. */
const HORIZON_DAYS = 7;
/** Rows to carry. Well past what any widget family can show; the surplus is what
 *  lets the timeline advance through the day on its own. */
const MAX_ITEMS = 25;

async function collectEvents(now: Date, calendars: Calendar[]): Promise<CalendarEvent[]> {
  const start = now.toISOString();
  const end = new Date(now.getTime() + HORIZON_DAYS * 86_400_000).toISOString();
  const per = await Promise.all(
    calendars.map((c) =>
      getEvents({ calendar_id: c.id, start, end }).catch(() => [] as CalendarEvent[]),
    ),
  );
  return per.flat();
}

async function collectTasks(): Promise<{ tasks: Task[]; lists: TaskList[] }> {
  const lists = await listTaskLists();
  // Across ALL lists, not just the ones selected in the task view. Hiding a
  // CALENDAR is an explicit "don't show me this"; the task-list selection is a
  // focus control inside one screen, and honouring it here would silently drop
  // items from a surface whose whole job is to be complete. Same call the
  // app-icon badge makes.
  const per = await Promise.all(lists.map((l) => getTasks(l.id).catch(() => [] as Task[])));
  return { tasks: per.flat(), lists };
}

/** `list_id → connected user`, for the ownership filter. Session-memoized per
 *  list by `currentUserForList`, so this costs a bridge call once per list per
 *  app run and nothing thereafter. */
async function resolveMe(lists: TaskList[]): Promise<Record<string, TaskUser | null>> {
  const entries = await Promise.all(
    lists.map(async (l) => [l.id, await currentUserForList(l.id)] as const),
  );
  return Object.fromEntries(entries);
}

async function computeSnapshot(): Promise<string> {
  const now = new Date();
  const [calendars, labels, hidden] = await Promise.all([
    listCalendars(),
    listColorLabels().catch(() => [] as ColorLabel[]),
    getHiddenCalendars(),
  ]);
  const [events, collected] = await Promise.all([
    collectEvents(now, calendars),
    collectTasks(),
  ]);
  const { tasks, lists } = collected;
  const meByList = await resolveMe(lists);

  const calendarsById = new Map(calendars.map((c) => [c.id, c]));
  const labelsById = new Map(labels.map((l) => [l.id, l]));

  return JSON.stringify(
    buildWidgetSnapshot<CalendarEvent>({
      events,
      tasks,
      now,
      horizonDays: HORIZON_DAYS,
      limit: MAX_ITEMS,
      // Translated HERE, not in the extension. The language is the one chosen in
      // the app's settings, which can differ from the device locale — and an
      // extension has no way to read that choice. The tag travels too, because
      // the widget still has dates and countdowns of its own to format.
      locale: i18n.language,
      strings: {
        empty: i18n.t('widgets.upcoming.empty'),
        noTimed: i18n.t('widgets.upcoming.noTimed'),
        stale: i18n.t('widgets.upcoming.stale'),
        allDay: i18n.t('widgets.upcoming.allDay'),
        today: i18n.t('widgets.upcoming.today'),
        runningUntil: i18n.t('widgets.upcoming.runningUntil'),
        kindEvent: i18n.t('widgets.upcoming.kindEvent'),
        kindTask: i18n.t('widgets.upcoming.kindTask'),
        // The canonical status wording, shared with the editor and the
        // chip menu — a widget inventing its own would be a third spelling.
        statusOpen: i18n.t('dialogs.task.status.open'),
        statusInProgress: i18n.t('dialogs.task.status.inProgress'),
      },
      // Someone else's work is not what is next for YOU — the same rule the
      // calendar views apply.
      meFor: (listId) => meByList[listId] ?? null,
      hiddenContainers: hidden,
      // The same resolver the day view paints with, so a widget row is never a
      // different colour from the row behind it in the app.
      eventColorOf: (ev) => resolveEventColor(ev, calendarsById, labelsById).hex,
      taskColorOf: (task) =>
        (task.color_label ? labelsById.get(task.color_label)?.hex : null) ?? null,
      calendarIdOf: (ev) => ev.calendar_id,
      titleOf: (ev) => ev.title,
      allDayOf: (ev) => ev.all_day,
    }),
  );
}

// One pass at a time; a trigger arriving mid-pass sets `rerun` so the guard
// loops once more with fresh data rather than dropping the update. Same shape as
// the badge's guard, and load-bearing for the same reason: every pass carries a
// full result, so a dropped one genuinely loses the latest state.
let inFlight = false;
let rerun = false;

/** Drain anything the widget queued, then recompute and hand over the snapshot.
 *  Never throws.
 *
 *  Draining FIRST is the whole ordering: a tap the widget queued has to become a
 *  completed task before the snapshot is built, or the pass would helpfully
 *  write the task back out as still pending. */
export async function refreshWidgetSnapshot(): Promise<void> {
  if (inFlight) {
    rerun = true;
    return;
  }
  inFlight = true;
  try {
    do {
      rerun = false;
      try {
        await drainQueuedActions();
        await CalFfi.writeWidgetSnapshot(await computeSnapshot());
      } catch {
        // A bridge hiccup, a missing container, an account that failed to list:
        // the widget keeps its previous snapshot, which is stale but coherent.
      }
    } while (rerun);
  } finally {
    inFlight = false;
  }
}

/**
 * Mount ONCE inside the TaskStore provider (it reads dataVersion), next to
 * `useAppBadge`. Keeps the widgets in step with the app on every trigger that
 * can change what is next.
 */
export function useWidgetSnapshot(): void {
  const { dataVersion, invalidateData } = useTaskStore();
  const dayKey = useCurrentDayKey();

  // Through the startup gate like every other app-global scan: during launch the
  // many mount + cache-flush triggers collapse into ONE deferred pass, so this
  // full-catalog fan-out never queues ahead of the visible screen's first read
  // on the serial native queue.
  const refresh = useCallback(() => {
    whenStartupSettled('widgetSnapshot', () => {
      void refreshWidgetSnapshot().then(() => {
        // A tap performed here — or by the background pass, which had no React
        // to tell — is a task the open views are now wrong about.
        if (consumeQueuedActionsApplied()) invalidateData();
      });
    });
  }, [invalidateData]);

  // Local task mutation + the local-midnight flip (yesterday's rows are no
  // longer next).
  useEffect(() => {
    refresh();
  }, [dataVersion, dayKey, refresh]);

  // Foreground-resume: time has moved on while iOS had the JS suspended, and
  // this is the moment before the user goes back to the home screen — the one
  // place the widget is actually looked at.
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') refresh();
    });
    return () => sub.remove();
  }, [refresh]);

  // Local EVENT writes (the task store's dataVersion does not cover the event
  // editor / delete paths) + external-cache warm passes.
  useEffect(() => subscribeCalendarChanged(refresh), [refresh]);
  useCacheReload('tasks', refresh);
  useCacheReload('calendar', refresh);
}
