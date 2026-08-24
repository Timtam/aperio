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
import { warmCacheOnForeground } from '../api/sync';
import { dayStartPreschedulesOsNotification } from '../reminders/dayStartSchedule';
import { notify } from './notify';
import { currentUserForList } from './currentUser';
import { readFiredDayKey, writeFiredDayKey } from './dayStartFired';
import {
  getCacheRefreshProgress,
  subscribeCacheRefreshProgress,
} from './cacheRefreshProgress';
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
async function runDeadlinePin(invalidateData: () => void): Promise<void> {
  const behaviour = await readTaskBehaviour();
  const todayKey = todayIsoKey();
  const fired = await readFiredDayKey('deadlinePin');
  if (!shouldFireToday(behaviour.dayStartTrigger, fired, todayKey)) return;
  const all = await loadAllTasks();
  await writeFiredDayKey('deadlinePin', todayKey);
  const targets = filterDeadlinePinTargets(all, await meForTasks(all));
  if (targets.length === 0) return;
  for (const task of targets) {
    // Pin to today; leave scheduled_time untouched ("by 14:30" ≠ "at 14:30").
    await updateTask({ ...task, scheduled_date: todayKey });
  }
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
    // Nothing in scope; still record the fire so we don't keep re-checking.
    await writeFiredDayKey('dayStartReview', todayKey);
    return;
  }

  const all = await loadTasksForLists(selectedIds);
  // Mark fired BEFORE applying — even an empty day records the fire (the gate's
  // only job is "review for this day"); a partial run isn't re-fired.
  await writeFiredDayKey('dayStartReview', todayKey);

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
  if (reminderTotal > 0 && !dayStartPreschedulesOsNotification(behaviour.dayStartTrigger)) {
    // One combined OS notification for the "you're not looking at Aperio"
    // reach — but ONLY for the modes the reminder scheduler does NOT
    // pre-schedule ('app-start' / the '00:00' default). For an explicit
    // morning HH:MM the ahead-of-time OS notification already fired at the
    // trigger instant; posting another here would double-notify minutes
    // apart. The live announcement above (the assistive-tech channel) and the
    // review modal always run regardless.
    void notify(
      i18n.t('dialogs.dayStartReview.reminders.notificationTitle'),
      i18n.t('dialogs.dayStartReview.reminders.notificationBody', { count: reminderTotal }),
      'day-start reminders notification',
    );
  }

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

  if (todayRows.length > 0) {
    await runAutoCarryOverBatch('today', todayRows, all, behaviour);
  }
  if (backlogRows.length > 0) {
    await runAutoCarryOverBatch('backlog', backlogRows, all, behaviour);
  }
  if (todayRows.length + backlogRows.length > 0) invalidateData();

  // Open the modal iff there's still a decision to make OR a reminder to show
  // (the reminders section is informational but still a reason to surface the
  // modal). Bump the data version first so the modal's own `useTasks` re-reads
  // from the bridge rather than a possibly-stale warm cache — the checker read
  // the bridge directly (a separate fan-out), so this keeps the modal
  // authoritative over what it acts on and makes its loading-guard meaningful.
  if (overdue.length + askRows.length + reminderTotal > 0) {
    invalidateData();
    openReview();
  }
}

// One run at a time across launch + the foreground listener.
let inFlight = false;

/** Cap on waiting for the external warm pass before the day-start checks
 *  run anyway. Offline (no pass, or a failing one) must not block the
 *  checks forever — and offline, the pre-branch blocking live reads
 *  degraded to empty too, so running local-only there is parity. */
const CACHE_SETTLE_CAP_MS = 60_000;
/** How long "not refreshing" still means "the pass has not STARTED yet".
 *  The warm kick returns before the pass emits anything, so an immediate
 *  status read says `refreshing: false` — indistinguishable from
 *  "finished". After the grace, "not refreshing" is taken at face value,
 *  which is also the honest answer on a device with no external accounts.
 *  (Same reasoning, constants and shape as backgroundSync's
 *  waitForExternalRefresh — that path polls because it runs headless; this
 *  one subscribes because the JS observer is alive in the foreground.) */
const WARM_START_GRACE_MS = 2_000;

/**
 * Kick an (unforced) external warm pass and resolve once it has finished.
 *
 * The day-start checks burn once-a-day fire-markers; with the read path
 * cache-only, evaluating against a cold external cache would burn the
 * markers against EMPTY data — silently dropping the day's deadline-pin,
 * carry-over, review and spoken reminders for every external task. The
 * device-reminders account was the visible victim: its bridge installs
 * right after the Host opens, so the launch warm pass can enumerate its
 * targets BEFORE that account exists, and the old "wait only if a pass is
 * already running" check then saw `refreshing: false` and evaluated
 * against nothing. Kicking our OWN pass here (unforced — fresh containers
 * cost nothing) guarantees every registered account, including the device
 * bridge, has been offered one refresh before anything is decided.
 */
function settleExternalCaches(): Promise<void> {
  return new Promise((resolve) => {
    let done = false;
    let unsub: () => void = () => {};
    let graceTimer: ReturnType<typeof setTimeout> | null = null;
    const finish = () => {
      if (done) return;
      done = true;
      unsub();
      if (graceTimer != null) clearTimeout(graceTimer);
      clearTimeout(cap);
      resolve();
    };
    const cap = setTimeout(finish, CACHE_SETTLE_CAP_MS);
    let seenRunning = getCacheRefreshProgress().refreshing;
    unsub = subscribeCacheRefreshProgress((p) => {
      if (p.refreshing) {
        seenRunning = true;
        if (graceTimer != null) {
          clearTimeout(graceTimer);
          graceTimer = null;
        }
      } else if (seenRunning) {
        finish();
      }
    });
    // The kick is fire-and-forget on the Host worker — its promise resolving
    // says nothing about the pass. Only a rejected bridge call ends the wait
    // early (degraded parity: no pass will ever report back).
    void warmCacheOnForeground().catch(() => finish());
    if (!seenRunning) {
      graceTimer = setTimeout(() => {
        if (!seenRunning) finish();
      }, WARM_START_GRACE_MS);
    }
  });
}

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

  const openReview = useCallback(() => setReviewOpen(true), []);
  const closeReview = useCallback(() => setReviewOpen(false), []);

  const runInner = useCallback(() => {
    void (async () => {
      if (inFlight) return;
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
        await settleExternalCaches();
        await refreshTaskLists().catch(() => {});
        // The reconciled selection lands via setState; yield one macrotask so
        // the provider commits and `selectionRef` reflects it. (React flushes
        // batched updates in a microtask — a timer runs strictly after.)
        await new Promise((resolve) => setTimeout(resolve, 0));
        await runDeadlinePin(invalidateData);
        // The review reads the SELECTED lists, so it must wait for the
        // store to hydrate (an empty pre-hydration selection would mark
        // the day fired with nothing to review). The catalog-ready
        // effect below re-runs us then.
        if (!loadingRef.current) {
          await runDayStartReview([...selectionRef.current], invalidateData, openReview);
        }
      } catch {
        // Best-effort — a bridge hiccup must never crash launch/foreground.
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

  return { reviewOpen, closeReview };
}
