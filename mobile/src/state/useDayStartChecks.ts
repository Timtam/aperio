import { useCallback, useEffect, useRef, useState } from 'react';
import { AccessibilityInfo, AppState } from 'react-native';

import {
  actionableDescendants,
  buildReminderGroups,
  filterCarriedOver,
  filterDeadlinePinTargets,
  filterOverdue,
  reminderCount,
  shouldFireToday,
  todayIsoKey,
} from '@aperio/shared';
import type { Task, TaskUser } from '@aperio/shared';
import i18n from '../../i18n';

import { getTasks, listTaskLists, updateTask } from '../api/client';
import { logLine } from '../api/logs';
import { dayStartPreschedulesOsNotification } from '../reminders/dayStartSchedule';
import { useAppLockLocked } from './appLockContext';
import { settleExternalCaches } from './cacheSettle';
import { notify } from './notify';
import { currentUserForList } from './currentUser';
import { readFiredDayKey, writeFiredDayKey } from './dayStartFired';
import { isDayStartReviewSnoozed } from './dayStartSnooze';
import { whenStartupSettled } from './startupGate';
import { effectiveForList, readTaskBehaviour, type TaskBehaviour } from './taskBehaviour';
import { useTaskStore } from './taskStoreContext';

// The mobile day-start checks — the screen-reader-first twin of the desktop's
// DeadlinePinChecker + DayStartReviewChecker. The desktop fires from a live
// minute-poller; iOS suspends background JS, so mobile runs the checks on launch
// + every foreground-resume (the same model as the reminder rescheduler), gated
// by the synced dayStartTrigger pref + a per-device per-slot fire-marker so a
// day's batch runs at most once. All best-effort + silent on failure.

/** Every task across the user's lists (for the cross-list deadline-pin). */
async function loadAllTasks(): Promise<Task[]> {
  const lists = await listTaskLists();
  const per = await Promise.all(lists.map((l) => getTasks(l.id).catch(() => [] as Task[])));
  return per.flat();
}

/** Tasks across the given lists — the day-start review's scope mirrors the
 *  desktop's (it reads `useTasks`, i.e. the SELECTED lists), so the checker's
 *  decision and the modal's re-derivation see the same set. */
async function loadTasksForLists(ids: string[]): Promise<Task[]> {
  const per = await Promise.all(ids.map((id) => getTasks(id).catch(() => [] as Task[])));
  return per.flat();
}

/** Resolve "me" per list for `tasks` (session-cached) as a sync lookup for the
 *  day-start ownership filter — only my own / unassigned tasks are offered or
 *  auto-acted-on; a colleague's task is theirs to handle (DESIGN §9.7). */
async function meForTasks(
  tasks: Task[],
): Promise<(listId: string) => TaskUser | null> {
  const ids = Array.from(new Set(tasks.map((task) => task.list_id)));
  const entries = await Promise.all(
    ids.map(async (id) => [id, await currentUserForList(id)] as const),
  );
  const map = Object.fromEntries(entries) as Record<string, TaskUser | null>;
  return (listId: string) => map[listId] ?? null;
}

/**
 * Silent "by"-deadline auto-pin: tasks whose deadline is today (and aren't
 * already scheduled for today) get pinned to today so they surface on today's
 * calendar lanes. Gated by dayStartTrigger + the 'deadlinePin' fire-marker. The
 * marker is written BEFORE applying (idempotent — a partial run isn't re-fired).
 */
async function runDeadlinePin(
  invalidateData: () => void,
  /** See runDayStartReview: a "nothing to pin" verdict only burns the marker
   *  on a CONFIRMED cache settle; a capped one retries next foreground. */
  settleConfirmed: boolean,
  /** Live app-lock probe — see runDayStartReview. */
  isLocked: () => boolean,
): Promise<void> {
  const behaviour = await readTaskBehaviour();
  const todayKey = todayIsoKey();
  const fired = await readFiredDayKey('deadlinePin');
  if (!shouldFireToday(behaviour.dayStartTrigger, fired, todayKey)) return;
  const all = await loadAllTasks();
  const targets = filterDeadlinePinTargets(all, await meForTasks(all));
  // Re-probe the app lock AFTER the loads: the runs bail at entry too, but a
  // re-lock during the settle/fan-out must not let the pin mutate + announce
  // through the cover. No marker burned yet — the unlock re-run picks it up.
  if (isLocked()) {
    void logLine('info', 'day-start: pin deferred (app locked) — unlock re-runs');
    return;
  }
  if (targets.length === 0) {
    if (!settleConfirmed) {
      void logLine(
        'info',
        `day-start: pin found nothing on an UNSETTLED cache (tasks=${all.length}) — marker kept, next foreground retries`,
      );
      return;
    }
    await writeFiredDayKey('deadlinePin', todayKey);
    return;
  }
  // Mark BEFORE applying (idempotent — a partial run isn't re-fired).
  await writeFiredDayKey('deadlinePin', todayKey);
  for (const task of targets) {
    // Pin to today; leave scheduled_time untouched ("by 14:30" ≠ "at 14:30").
    await updateTask({ ...task, scheduled_date: todayKey });
  }
  void logLine('info', `day-start: pinned ${targets.length} by-deadline task(s) to today`);
  AccessibilityInfo.announceForAccessibility(
    i18n.t('dialogs.deadlinePin.announce', { count: targets.length }),
  );
  invalidateData();
}

/**
 * Silent carry-over batch (one action) — the mobile twin of the desktop
 * runAutoCarryOverBatch. Collects every slipped root plus, when cascade is on
 * for THAT root's list, its actionable descendants, and shifts each one's
 * scheduled_date (today or null). Announces the ROOT count (descendants are an
 * implementation detail). No visual undo-toast: mobile has no toast surface yet,
 * and the screen-reader announce — the channel that matters here — is covered.
 */
async function runAutoCarryOverBatch(
  action: 'today' | 'backlog',
  slippedRoots: Task[],
  allTasks: Task[],
  behaviour: TaskBehaviour,
): Promise<void> {
  const collected = new Map<string, Task>();
  for (const root of slippedRoots) {
    collected.set(root.id, root);
    if (!effectiveForList(behaviour, root.list_id).cascade) continue;
    for (const desc of actionableDescendants(root.id, allTasks)) {
      collected.set(desc.id, desc);
    }
  }
  const targets = [...collected.values()];
  if (targets.length === 0) return;
  const newDate = action === 'today' ? todayIsoKey() : null;
  // Sequential so a first-row failure surfaces without a half-applied family.
  for (const task of targets) {
    await updateTask({ ...task, scheduled_date: newDate });
  }
  AccessibilityInfo.announceForAccessibility(
    i18n.t(
      action === 'today'
        ? 'dialogs.dayStartReview.carryOver.autoToday'
        : 'dialogs.dayStartReview.carryOver.autoBacklog',
      { count: slippedRoots.length },
    ),
  );
}

/**
 * The unified day-start review gate — the mobile twin of DayStartReviewChecker.
 * Reads overdue (lapsed deadline) + slipped (lapsed scheduled day) across the
 * SELECTED lists, splits slipped rows by each list's carry-over default, runs
 * the silent batch for the today/backlog lists, and opens the review modal iff
 * there's still something to decide (an overdue row or a slipped row whose list
 * voted 'ask'). Gated by dayStartTrigger + the 'dayStartReview' fire-marker +
 * the 4-hour snooze. A snooze bail does NOT mark fired — the gate re-runs once
 * the snooze expires.
 */
async function runDayStartReview(
  selectedIds: string[],
  invalidateData: () => void,
  openReview: () => void,
  /** Whether the cache settle CONFIRMED the external caches are warm. A
   *  "nothing to review" verdict is only believed — and only burns the
   *  once-a-day marker — on a confirmed settle; on a capped one the next
   *  foreground retries instead of silencing the review for the day. */
  settleConfirmed: boolean,
  /** Live app-lock probe. The run bails at ENTRY while locked, but the
   *  settle + fan-outs above this point can span a minute — long enough to
   *  background the app and re-lock. Checked again right before anything
   *  surfaces (marker burn, announcements, the modal): the review is an RN
   *  Modal, which would present ABOVE the lock cover fully interactive. */
  isLocked: () => boolean,
): Promise<void> {
  const behaviour = await readTaskBehaviour();
  const todayKey = todayIsoKey();
  const fired = await readFiredDayKey('dayStartReview');
  if (!shouldFireToday(behaviour.dayStartTrigger, fired, todayKey)) return;
  // Snooze respects the user's "remind me later" choice. Do NOT mark fired —
  // the next eligible tick should run the gate once the snooze expires. NB: this
  // bail also defers the day's TASK REMINDERS (announcement + notification +
  // modal) — they're computed below this gate, so a snooze suppresses them too,
  // and they re-surface together with the review once the snooze expires.
  if (await isDayStartReviewSnoozed()) return;
  if (selectedIds.length === 0) {
    // An empty selection is USUALLY a transient dip — a shrunken catalog read
    // mid-refresh trims the reconciled selection until the lists reappear —
    // not "this user has no lists". Burning the marker here silenced the
    // review for the WHOLE day at exactly the moment the data was at its
    // worst. Keep the marker and let the next foreground retry; a genuinely
    // list-less user pays one cheap re-check per foreground.
    void logLine(
      'info',
      'day-start: review skipped (empty selection) — marker kept, next foreground retries',
    );
    return;
  }

  const all = await loadTasksForLists(selectedIds);
  const meFor = await meForTasks(all);

  // ── Day-start TASK REMINDERS ────────────────────────────────────────────
  // Three read-only nudges, each gated by its own toggle, sharing this same
  // 'dayStartReview' fire-marker so they surface once a day with the review.
  // Built via the SHARED `buildReminderGroups` so a task lands in exactly ONE
  // group (due-today > planned-today > countdown) and the spoken count, the OS
  // notification, and the modal's rendered rows all agree. The predicates skip
  // settled tasks, project parents, and other-user tasks (via `meFor`).
  const reminders = buildReminderGroups(
    all,
    {
      remindUntimedToday: behaviour.remindUntimedToday,
      remindDeadlineArrived: behaviour.remindDeadlineArrived,
      remindDeadlineCountdown: behaviour.remindDeadlineCountdown,
      deadlineCountdownDays: behaviour.deadlineCountdownDays,
    },
    meFor,
  );
  const reminderTotal = reminderCount(reminders);

  const overdue = filterOverdue(all, meFor);
  const slipped = filterCarriedOver(all, {
    cascadeEnabledFor: (listId) => effectiveForList(behaviour, listId).cascade,
    meFor,
  });

  // Split slipped rows by each list's carry-over default: 'today' / 'backlog'
  // run silently, 'ask' surfaces in the modal. A mix produces a hybrid.
  const askRows: Task[] = [];
  const todayRows: Task[] = [];
  const backlogRows: Task[] = [];
  for (const row of slipped) {
    const def = effectiveForList(behaviour, row.list_id).carryOverDefault;
    if (def === 'today') todayRows.push(row);
    else if (def === 'backlog') backlogRows.push(row);
    else askRows.push(row);
  }

  // Late lock re-probe: a re-lock during the settle/fan-out must not burn the
  // marker, speak task details through the cover, or present the review Modal
  // above it. Nothing surfaced yet — the unlock effect re-runs the whole pass.
  if (isLocked()) {
    void logLine('info', 'day-start: review deferred (app locked) — unlock re-runs');
    return;
  }

  // ── The decision, and the ONLY honest places to burn the day marker ──────
  // `surfaced` opens the modal; `autoRows` acts silently. A day with neither
  // is only BELIEVED (and marked done) when the settle confirmed the caches
  // were warm — a zero verdict on cold/partial data used to burn the marker
  // and silence the review for the whole day, which is exactly the failure
  // a screen-reader user cannot see happening.
  const surfaced = overdue.length + askRows.length + reminderTotal;
  const autoRows = todayRows.length + backlogRows.length;
  if (surfaced + autoRows === 0) {
    if (!settleConfirmed) {
      void logLine(
        'info',
        `day-start: review found nothing on an UNSETTLED cache (tasks=${all.length}, selection=${selectedIds.length}) — marker kept, next foreground retries`,
      );
      return;
    }
    await writeFiredDayKey('dayStartReview', todayKey);
    void logLine(
      'info',
      `day-start: review found nothing (tasks=${all.length}, selection=${selectedIds.length}) — day marked done`,
    );
    return;
  }
  // Something to say or to do: mark the day BEFORE announcing/acting, so a
  // partial failure below can't replay the silent batch or the announcements
  // on the next foreground.
  await writeFiredDayKey('dayStartReview', todayKey);

  // Coalesce the per-group lines into ONE polite live announcement: three
  // back-to-back `announceForAccessibility` calls would each interrupt the
  // previous, so a screen-reader user would only ever hear the last group.
  // Joined with ". " they read as a single utterance.
  const reminderParts: string[] = [];
  if (reminders.untimed.length > 0) {
    reminderParts.push(
      i18n.t('dialogs.dayStartReview.reminders.untimedToday', {
        count: reminders.untimed.length,
      }),
    );
  }
  if (reminders.dueToday.length > 0) {
    reminderParts.push(
      i18n.t('dialogs.dayStartReview.reminders.deadlineArrived', {
        count: reminders.dueToday.length,
      }),
    );
  }
  if (reminders.countdown.length > 0) {
    // Summary only — the per-task remaining days (1..window) differ, so the
    // spoken roll-up stays generic ("N tasks with an upcoming deadline").
    reminderParts.push(
      i18n.t('dialogs.dayStartReview.reminders.countdown', {
        count: reminders.countdown.length,
      }),
    );
  }
  if (reminderParts.length > 0) {
    AccessibilityInfo.announceForAccessibility(reminderParts.join('. '));
  }
  if (surfaced > 0 && !dayStartPreschedulesOsNotification(behaviour.dayStartTrigger)) {
    // One combined OS notification for the "you're not looking at Aperio"
    // reach — but ONLY for the modes the reminder scheduler does NOT
    // pre-schedule ('app-start' / the '00:00' default). For an explicit
    // morning HH:MM the ahead-of-time OS notification already fired at the
    // trigger instant; posting another here would double-notify minutes
    // apart. The live announcement above (the assistive-tech channel) and the
    // review modal always run regardless. The count is `surfaced` — the same
    // sum that opens the dialog — so the notification never claims fewer
    // tasks than the review it points at.
    void notify(
      i18n.t('dialogs.dayStartReview.reminders.notificationTitle'),
      i18n.t('dialogs.dayStartReview.reminders.notificationBody', { count: surfaced }),
      'day-start reminders notification',
    );
  }

  if (todayRows.length > 0) {
    await runAutoCarryOverBatch('today', todayRows, all, behaviour);
  }
  if (backlogRows.length > 0) {
    await runAutoCarryOverBatch('backlog', backlogRows, all, behaviour);
  }
  if (autoRows > 0) invalidateData();

  void logLine(
    'info',
    `day-start: review tasks=${all.length} overdue=${overdue.length} ask=${askRows.length} auto=${autoRows} reminders=${reminderTotal} -> ${surfaced > 0 ? 'open modal' : 'silent batch only'}`,
  );

  // Open the modal iff there's still a decision to make OR a reminder to show
  // (the reminders section is informational but still a reason to surface the
  // modal). Bump the data version first so the modal's own `useTasks` re-reads
  // from the bridge rather than a possibly-stale warm cache — the checker read
  // the bridge directly (a separate fan-out), so this keeps the modal
  // authoritative over what it acts on and makes its loading-guard meaningful.
  if (surfaced > 0) {
    invalidateData();
    openReview();
  }
}

// One run at a time across launch + the foreground listener.
let inFlight = false;

// `settleExternalCaches` moved to ./cacheSettle — the reminder scheduler's
// launch pass needs the identical wait, and two copies of a timing-sensitive
// primitive would drift.

/**
 * Mount once inside the TaskStore provider: run the day-start checks on launch
 * (once the catalog + selection have hydrated) + every foreground-resume (the
 * latter catches a date rollover while away). Returns the review-modal state so
 * the mounting component can render the modal — the modal must overlay any tab,
 * and the checker lives above the navigator, so a navigation screen would be the
 * wrong tool; an app-level modal driven by this flag is the fit.
 */
export function useDayStartChecks(): { reviewOpen: boolean; closeReview: () => void } {
  const { invalidateData, selectedTaskListIds, taskListsLoading, refreshTaskLists } =
    useTaskStore();
  const [reviewOpen, setReviewOpen] = useState(false);

  // The AppState listener registers once; it reads the live selection +
  // catalog-loading state through refs so it never needs re-subscribing when
  // either changes.
  const selectionRef = useRef(selectedTaskListIds);
  selectionRef.current = selectedTaskListIds;
  const loadingRef = useRef(taskListsLoading);
  loadingRef.current = taskListsLoading;
  // While the app lock covers the app, the checks HOLD: the review is an RN
  // Modal, which would present ABOVE the lock cover fully interactive, and
  // even the spoken announcements would read task details through the lock.
  // The unlock flips this false and the effect below runs the deferred pass.
  const appLocked = useAppLockLocked();
  const appLockedRef = useRef(appLocked);
  appLockedRef.current = appLocked;

  const openReview = useCallback(() => setReviewOpen(true), []);
  const closeReview = useCallback(() => setReviewOpen(false), []);

  const runInner = useCallback(() => {
    void (async () => {
      if (inFlight) return;
      if (appLockedRef.current) return;
      inFlight = true;
      try {
        // A cheap marker precheck FIRST, so the settle below (which kicks a
        // warm pass) only ever runs when a day-start slot is actually still
        // due — not on every foreground-resume all day long.
        const behaviour = await readTaskBehaviour();
        const todayKey = todayIsoKey();
        const pinDue = shouldFireToday(
          behaviour.dayStartTrigger,
          await readFiredDayKey('deadlinePin'),
          todayKey,
        );
        const reviewDue =
          shouldFireToday(
            behaviour.dayStartTrigger,
            await readFiredDayKey('dayStartReview'),
            todayKey,
          ) &&
          // A snoozed review would bail inside runDayStartReview anyway (without
          // marking fired); checking here keeps a snoozed morning from kicking a
          // full warm pass on every foreground-resume for nothing.
          !(await isDayStartReviewSnoozed());
        if (!pinDue && !reviewDue) return;
        // The settle below takes a couple of seconds at best and a slow-network
        // morning at worst — say so, politely, or the review modal's focus grab
        // lands mid-task for a screen-reader user with no warning that anything
        // was still pending.
        AccessibilityInfo.announceForAccessibility(
          i18n.t('dialogs.dayStartReview.checking'),
        );
        // Warm every registered external account (unforced) and wait it out,
        // so the cache-only reads below see today's data — see
        // settleExternalCaches for why waiting on an ALREADY-running pass is
        // not enough. Then re-read the catalog: the pass may have surfaced
        // lists (a cold device-reminders account) that the launch read
        // missed, and the reconciler must adopt them into the selection
        // before the review decides what today holds.
        const settleStart = Date.now();
        const settle = await settleExternalCaches();
        await refreshTaskLists().catch(() => {});
        // The reconciled selection lands via setState; yield one macrotask so
        // the provider commits and `selectionRef` reflects it. (React flushes
        // batched updates in a microtask — a timer runs strictly after.)
        await new Promise((resolve) => setTimeout(resolve, 0));
        void logLine(
          'info',
          `day-start: due (pin=${pinDue} review=${reviewDue}), settle ${settle} in ${
            Date.now() - settleStart
          }ms, selection=${selectionRef.current.size}`,
        );
        const confirmed = settle === 'confirmed';
        const isLocked = () => appLockedRef.current;
        await runDeadlinePin(invalidateData, confirmed, isLocked);
        // The review reads the SELECTED lists, so it must wait for the
        // store to hydrate (an empty pre-hydration selection would mark
        // the day fired with nothing to review). The catalog-ready
        // effect below re-runs us then.
        if (!loadingRef.current) {
          await runDayStartReview(
            [...selectionRef.current],
            invalidateData,
            openReview,
            confirmed,
            isLocked,
          );
        } else {
          void logLine(
            'info',
            'day-start: catalog still hydrating — review deferred to the catalog-ready effect',
          );
        }
      } catch (err) {
        // Best-effort — a bridge hiccup must never crash launch/foreground.
        // But leave a trace: this catch used to swallow the whole morning
        // without a word, which made "the dialog just never came" undiagnosable.
        void logLine(
          'warn',
          `day-start: run failed: ${err instanceof Error ? err.message : String(err)}`,
        );
      } finally {
        inFlight = false;
      }
    })();
  }, [invalidateData, openReview, refreshTaskLists]);

  const run = useCallback(() => {
    // Startup-gated: the deadline-pin + review passes fan out over every
    // list, and at launch that queued ahead of the visible screen's first
    // read on the serial native queue. Pre-gate triggers coalesce into one
    // deferred run (the fire-markers make repeats no-ops anyway); once the
    // gate is open this is a plain pass-through (foreground resumes).
    // The cache-settle wait lives INSIDE runInner, after its marker
    // precheck, so it costs a warm kick only while a day-start slot is due.
    whenStartupSettled('dayStart', runInner);
  }, [runInner]);

  // Launch + catalog-ready: fire once the task-list catalog + selection have
  // hydrated (taskListsLoading flips false). Re-firing here is harmless — the
  // inFlight guard + the fire-marker make repeat runs no-ops.
  useEffect(() => {
    if (taskListsLoading) return;
    run();
  }, [taskListsLoading, run]);

  // Foreground-resume: catches a date rollover (or a snooze expiry) while away.
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') run();
    });
    return () => sub.remove();
  }, [run]);

  // Unlock: the passes above bailed while the app lock was up — run the
  // morning's checks now that someone proved they may see them.
  useEffect(() => {
    if (!appLocked) run();
  }, [appLocked, run]);

  return { reviewOpen, closeReview };
}
