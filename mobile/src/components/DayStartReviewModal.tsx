import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  ActivityIndicator,
  Alert,
  findNodeHandle,
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import {
  actionableDescendants,
  buildReminderGroups,
  daysUntilDeadline,
  filterCarriedOver,
  filterOverdue,
  occurrenceMoveTarget,
  priorityMarker,
  prioritySuffix,
  reminderCount,
  deadlineMovedToToday,
  todayIsoKey,
} from '@aperio/shared';
import type { Task } from '@aperio/shared';

import { deleteTask as apiDeleteTask, updateTask } from '../api/client';
import { navigateNested } from '../navigation/navigationRef';
import { refreshRemindersSoon } from '../reminders/scheduler';
import { snoozeDayStartReview } from '../state/dayStartSnooze';
import {
  effectiveForList,
  priorityScaleFor,
  readTaskBehaviour,
  TASK_BEHAVIOUR_DEFAULTS,
  type TaskBehaviour,
} from '../state/taskBehaviour';
import { setTaskStatusTo, statusAnnounce } from '../state/taskToggle';
import { useCurrentUserByList } from '../state/currentUser';
import { useTaskStore } from '../state/taskStoreContext';
import { useTasks } from '../state/useTasks';
import { useThemedStyles, type ThemeColors } from '../theme';

// The day-start review — the screen-reader-first twin of the desktop
// DayStartReviewDialog (DESIGN.md § 9.5). Two sections: missed deadlines
// (Done / Back to backlog) and carried-over tasks (Today / Tomorrow / Backlog /
// Done), plus per-section bulk actions and a snooze. Cascade-coupling resolves
// PER-LIST: a date action on a parent drags its actionable descendants along
// iff that row's list cascades; the slipped filter hides a subtask whose
// same-list ancestor also slipped (the user decides at the subtree root).
//
// Unlike the desktop (a Modal in the dialog stack) this is an RN <Modal> driven
// by a flag from useDayStartChecks: the review must overlay ANY tab and it's
// triggered from above the navigator, so a per-stack navigation screen would be
// the wrong tool. Every action announces its result; managed focus lands on the
// next row so a screen-reader user keeps their place as rows resolve.

export interface DayStartReviewModalProps {
  visible: boolean;
  onClose: () => void;
}

export default function DayStartReviewModal({ visible, onClose }: DayStartReviewModalProps) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const insets = useSafeAreaInsets();
  const { tasks, loading, taskListById } = useTasks();
  const { invalidateData } = useTaskStore();

  const tr = useCallback(
    (key: string, vars?: Record<string, unknown>): string => t(key, vars) as string,
    [t],
  );

  // Localized, time-free date formatter (Intl, no date-fns on mobile).
  const formatDate = useMemo(() => {
    const f = new Intl.DateTimeFormat(i18n.language, {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    });
    return (iso: string) => f.format(new Date(`${iso}T00:00:00`));
  }, [i18n.language]);

  const announce = useCallback((message: string) => {
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  // The synced task-behaviour (cascade / carry-over defaults) drives the same
  // per-list cascade decisions the checker used. Loaded fresh each time the
  // modal opens; until then we render a loading state (cascade grouping needs
  // the real values). resolvedIds also resets per opening.
  const [behaviour, setBehaviour] = useState<TaskBehaviour | null>(null);
  // Until the prefs land, read the world as three levels — the default.
  const priorityScale = priorityScaleFor(behaviour?.twoLevelPriority ?? false);
  const [busy, setBusy] = useState(false);
  // Rows the user just handled vanish instantly even before the update_task
  // round-trip + refetch land — otherwise every tap feels laggy.
  const [resolvedIds, setResolvedIds] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (!visible) return;
    setResolvedIds(new Set());
    setBehaviour(null);
    let cancelled = false;
    void readTaskBehaviour().then((b) => {
      if (!cancelled) setBehaviour(b);
    });
    return () => {
      cancelled = true;
    };
  }, [visible]);

  const beh = behaviour ?? TASK_BEHAVIOUR_DEFAULTS;
  const cascadeFor = useCallback(
    (listId: string) => effectiveForList(beh, listId).cascade,
    [beh],
  );
  // Only my own / unassigned tasks are offered; a task owned by a concrete other
  // user is theirs to handle (DESIGN §9.7). `meFor` resolves the connected user
  // per list from the session cache (null = no identity ⇒ keep the task).
  const currentUserByList = useCurrentUserByList(tasks);
  const meFor = useCallback(
    (listId: string) => currentUserByList[listId] ?? null,
    [currentUserByList],
  );

  const overdue = useMemo(() => filterOverdue(tasks, meFor), [tasks, meFor]);
  const slipped = useMemo(
    () => filterCarriedOver(tasks, { cascadeEnabledFor: cascadeFor, meFor }),
    [tasks, cascadeFor, meFor],
  );

  // ── Read-only reminder groups ───────────────────────────────────────────────
  // Gated by the same Settings toggles the checker uses (read off the loaded
  // behaviour). Built via the SHARED `buildReminderGroups` so the rendered rows
  // are de-duplicated EXACTLY like the checker's count (a task lands in one
  // group only: due-today > planned-today > countdown). Informational: the rows
  // open the task editor but never mutate state from the modal, so they sit
  // outside the resolved/snooze bookkeeping.
  const reminders = useMemo(
    () =>
      buildReminderGroups(
        tasks,
        {
          remindUntimedToday: beh.remindUntimedToday,
          remindDeadlineArrived: beh.remindDeadlineArrived,
          remindDeadlineCountdown: beh.remindDeadlineCountdown,
          deadlineCountdownDays: beh.deadlineCountdownDays,
        },
        meFor,
      ),
    [
      tasks,
      beh.remindUntimedToday,
      beh.remindDeadlineArrived,
      beh.remindDeadlineCountdown,
      beh.deadlineCountdownDays,
      meFor,
    ],
  );
  const hasReminders = reminderCount(reminders) > 0;

  // Render-ready reminder groups: each task set paired with its count-aware
  // summary line and the per-row "why" suffix for the row's accessible label.
  // The countdown "why" is PER TASK — the set spans the whole 1..window range,
  // so each task announces its OWN remaining days — hence `why` is a function.
  // Empty groups are filtered out at render time.
  const reminderGroups = useMemo(
    () => [
      {
        key: 'untimed',
        tasks: reminders.untimed,
        summary: t('dialogs.dayStartReview.reminders.untimedToday', {
          count: reminders.untimed.length,
        }),
        why: () => t('dialogs.dayStartReview.reminders.whyUntimed'),
      },
      {
        key: 'dueToday',
        tasks: reminders.dueToday,
        summary: t('dialogs.dayStartReview.reminders.deadlineArrived', {
          count: reminders.dueToday.length,
        }),
        why: () => t('dialogs.dayStartReview.reminders.whyDeadlineToday'),
      },
      {
        key: 'countdown',
        tasks: reminders.countdown,
        summary: t('dialogs.dayStartReview.reminders.countdown', {
          count: reminders.countdown.length,
        }),
        why: (task: Task) =>
          t('dialogs.dayStartReview.reminders.whyCountdown', {
            lead: t('dialogs.dayStartReview.reminders.inDays', {
              count: daysUntilDeadline(task) ?? beh.deadlineCountdownDays,
            }),
          }),
      },
    ],
    [reminders, beh.deadlineCountdownDays, t],
  );

  // Open a reminder task in the editor: close the review first (it overlays the
  // navigator as an RN Modal, so the editor would otherwise mount behind it),
  // then navigate via the app-level container ref — this modal renders OUTSIDE
  // any navigator, so it has no `useNavigation` context of its own. Target the
  // Tasks tab's stack EXPLICITLY (it registers TaskEditor): a bare-name navigate
  // from the root is unhandled when the focused tab's stack has no TaskEditor
  // (e.g. the Contacts tab), so the row would be a dead button there.
  const openTaskEditor = useCallback(
    (task: Task) => {
      onClose();
      navigateNested('TasksTab', 'TaskEditor', { taskId: task.id, listId: task.list_id });
    },
    [onClose],
  );

  const remainingOverdue = useMemo(
    () => overdue.filter((task) => !resolvedIds.has(task.id)),
    [overdue, resolvedIds],
  );
  const remainingSlipped = useMemo(
    () => slipped.filter((task) => !resolvedIds.has(task.id)),
    [slipped, resolvedIds],
  );
  const totalRemaining = remainingOverdue.length + remainingSlipped.length;

  // Reminders-only review: nothing left to decide, just today's read-only
  // nudges. "Remind me later" is meaningless here (no work to defer, and the
  // day is already marked reviewed before the modal opens), so the footer
  // offers a plain acknowledge, hardware-back closes without burning a snooze,
  // and the intro copy swaps. Mirrors the desktop dialog.
  const remindersOnly = totalRemaining === 0 && hasReminders;

  // ── Managed screen-reader focus ─────────────────────────────────────────────
  // Each row registers its title node; after an action we land focus on the
  // next remaining row so the user keeps their place. The title gets focus on
  // open.
  const titleRef = useRef<Text>(null);
  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  const focusTitle = useCallback(() => {
    const tag = findNodeHandle(titleRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  // The ordered visible row ids, for picking the next focus target.
  const orderedIds = useMemo(
    () => [...remainingOverdue, ...remainingSlipped].map((task) => task.id),
    [remainingOverdue, remainingSlipped],
  );

  // After a resolution remounts the rows, move focus to the queued next row.
  useEffect(() => {
    if (pendingFocusId.current == null) return;
    const id = pendingFocusId.current;
    pendingFocusId.current = null;
    const tag = rowTags.current[id];
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [orderedIds]);

  /** Queue focus on the row that should take over once `removedIds` leave. */
  const queueFocusAfter = useCallback(
    (removedIds: string[]) => {
      const removed = new Set(removedIds);
      const survivor = orderedIds.find((id) => !removed.has(id));
      pendingFocusId.current = survivor ?? null;
    },
    [orderedIds],
  );

  // Close + snooze when the user has cleared everything mid-session. The
  // defensive empty-on-open (checker + modal raced) closes WITHOUT snoozing —
  // there's no reason to suppress a real future trigger. Both wait for the
  // initial fetch + behaviour load so a transient empty state can't close us.
  // Reminders keep the modal up — whether the review opened that way or the
  // user just cleared the last actionable row. The day's fire marker is written
  // BEFORE the modal opens, so closing here is FINAL for today: auto-dismissing
  // on the last "done" tap would silently eat reminders the user never read.
  // It falls through to the reminders-only state instead (announced below).
  useEffect(() => {
    if (!visible || behaviour == null || loading) return;
    if (totalRemaining > 0) return;
    if (hasReminders) return;
    if (resolvedIds.size > 0) {
      announce(t('dialogs.dayStartReview.allHandled'));
      void snoozeDayStartReview(4);
      // Re-plan the pre-scheduled day-start notifications so none fires into
      // the fresh snooze window (the scheduler reads the snooze end).
      refreshRemindersSoon();
    }
    onClose();
  }, [
    visible,
    behaviour,
    loading,
    totalRemaining,
    resolvedIds.size,
    hasReminders,
    announce,
    t,
    onClose,
  ]);

  // Speak the switch INTO the reminders-only state: the intro copy and the
  // footer button's identity both change, and VoiceOver doesn't re-read an
  // element whose label changed under it. Either the user cleared the last
  // actionable row, or the rows were never really there (a stale read on the
  // opening render) — the first wants "all handled", the second the new hint.
  const wasRemindersOnly = useRef(false);
  useEffect(() => {
    if (!visible) {
      wasRemindersOnly.current = false;
      return;
    }
    if (remindersOnly && !wasRemindersOnly.current) {
      announce(
        resolvedIds.size > 0
          ? t('dialogs.dayStartReview.allHandled')
          : t('dialogs.dayStartReview.hintRemindersOnly'),
      );
    }
    wasRemindersOnly.current = remindersOnly;
  }, [visible, remindersOnly, resolvedIds.size, announce, t]);

  // ── Deadline-section actions ────────────────────────────────────────────────

  const markCompleted = useCallback(
    async (task: Task): Promise<void> => {
      setBusy(true);
      queueFocusAfter([task.id]);
      try {
        // Completing a parent cascades to its open descendants; surface the
        // count the same way a coupled status flip does elsewhere, so a
        // screen-reader user hears that more than one task changed.
        const extra = await setTaskStatusTo(
          task,
          'completed',
          taskListById.get(task.list_id),
          tasks,
        );
        setResolvedIds((s) => new Set(s).add(task.id));
        const base = statusAnnounce(t, 'completed', task.title);
        announce(
          extra > 0 ? `${base} ${t('views.tasks.cascadeSuffix', { count: extra })}` : base,
        );
        invalidateData();
      } finally {
        setBusy(false);
      }
    },
    [announce, invalidateData, queueFocusAfter, t, tasks, taskListById],
  );

  /**
   * Move a lapsed deadline to today, keeping the time of day.
   *
   * The section's only other way out of an overdue deadline was Backlog, which
   * clears the deadline AND the scheduling — everything, when the answer is
   * usually "it is still due, just not yesterday".
   *
   * The TIME is kept on purpose: a deadline of 14:00 that slipped is still a
   * 14:00 deadline, and dropping it to "sometime today" would quietly discard
   * what the user wrote. The SCHEDULING is left alone for the same reason —
   * what lapsed is the deadline, not the plan.
   *
   * One task, no cascade over subtasks, matching `backToBacklog` right beside
   * it. Twin of the desktop dialog's action of the same name.
   */
  const deadlineToToday = useCallback(
    async (task: Task): Promise<void> => {
      setBusy(true);
      queueFocusAfter([task.id]);
      try {
        await updateTask(deadlineMovedToToday(task));
        setResolvedIds((s) => new Set(s).add(task.id));
        announce(
          t('dialogs.dayStartReview.deadlines.announceDeadlineToday', {
            title: task.title,
          }),
        );
        invalidateData();
      } finally {
        setBusy(false);
      }
    },
    [announce, invalidateData, queueFocusAfter, t],
  );

  const backToBacklog = useCallback(
    async (task: Task): Promise<void> => {
      setBusy(true);
      queueFocusAfter([task.id]);
      try {
        await updateTask({
          ...task,
          scheduled_date: null,
          scheduled_time: null,
          deadline_date: null,
          deadline_time: null,
          status: 'open',
        });
        setResolvedIds((s) => new Set(s).add(task.id));
        announce(
          t('dialogs.dayStartReview.deadlines.announceBackToBacklog', { title: task.title }),
        );
        invalidateData();
      } finally {
        setBusy(false);
      }
    },
    [announce, invalidateData, queueFocusAfter, t],
  );

  const completeAllOverdue = useCallback(async () => {
    if (remainingOverdue.length === 0) return;
    setBusy(true);
    const ids = remainingOverdue.map((task) => task.id);
    queueFocusAfter(ids);
    try {
      const now = new Date().toISOString();
      // Bulk = a plain status flip (no cascade) — mirrors the desktop's
      // completeAllOverdue, which intentionally skips the per-row cascade here.
      for (const task of remainingOverdue) {
        await updateTask({
          ...task,
          status: 'completed',
          completed_at: task.completed_at ?? now,
        });
      }
      setResolvedIds((s) => {
        const next = new Set(s);
        ids.forEach((id) => next.add(id));
        return next;
      });
      announce(
        t('dialogs.dayStartReview.deadlines.announceAllCompleted', {
          count: remainingOverdue.length,
        }),
      );
      invalidateData();
    } finally {
      setBusy(false);
    }
  }, [announce, invalidateData, queueFocusAfter, remainingOverdue, t]);

  // ── Carry-over section actions ──────────────────────────────────────────────

  /** Apply a date patch to a root and, when its list cascades, its actionable
   *  descendants too. The three date buttons funnel through here; Done goes via
   *  the shared status path so the status cascade stays centralised. */
  const applyDateAction = useCallback(
    async (root: Task, newDate: string | null, announcementKey: string): Promise<void> => {
      setBusy(true);
      const followers = cascadeFor(root.list_id) ? actionableDescendants(root.id, tasks) : [];
      const targets: Task[] = [root, ...followers];
      const ids = targets.map((task) => task.id);
      queueFocusAfter(ids);
      try {
        // Sequential so a first-row failure surfaces without a half-applied family.
        let advanced = false;
        for (const target of targets) {
          // What the SOURCE can actually do with "move this one to that day".
          // iOS Reminders keeps the due date as the series anchor, so writing an
          // arbitrary day to a repeating reminder simply does not stick — and it
          // used to not stick SILENTLY: this dialog said "moved to today", the
          // reminder kept its old date, and today went on showing a read-only
          // preview with no checkbox.
          const move = occurrenceMoveTarget(
            target,
            newDate,
            taskListById.get(target.list_id)?.task_capabilities
              ?.reschedule_single_occurrence ?? true,
          );
          if (move.advanced) advanced = true;
          await updateTask({ ...target, scheduled_date: move.date });
        }
        setResolvedIds((s) => {
          const next = new Set(s);
          ids.forEach((id) => next.add(id));
          return next;
        });
        // Says what HAPPENED, not what was asked for. Advancing a series is not
        // the day that was tapped, and announcing the tap would be the same lie
        // in a politer form.
        const base = advanced
          ? t('dialogs.dayStartReview.carryOver.announceAdvancedSeries')
          : t(announcementKey, { title: root.title });
        announce(
          followers.length > 0
            ? `${base} ${t('views.tasks.cascadeSuffix', { count: followers.length })}`
            : base,
        );
        invalidateData();
      } finally {
        setBusy(false);
      }
    },
    [announce, cascadeFor, invalidateData, queueFocusAfter, t, taskListById, tasks],
  );

  const carryToToday = useCallback(
    (task: Task) =>
      applyDateAction(task, todayIsoKey(), 'dialogs.dayStartReview.carryOver.announceToday'),
    [applyDateAction],
  );
  const carryToTomorrow = useCallback(
    (task: Task) =>
      applyDateAction(task, tomorrowIsoKey(), 'dialogs.dayStartReview.carryOver.announceTomorrow'),
    [applyDateAction],
  );
  const sendToBacklog = useCallback(
    (task: Task) =>
      applyDateAction(task, null, 'dialogs.dayStartReview.carryOver.announceBacklog'),
    [applyDateAction],
  );

  /** Every visible carry-over row plus, when its list cascades, its actionable
   *  descendants — the bulk target set. The Map dedups overlapping branches. */
  const collectBulkCarryTargets = useCallback((): Task[] => {
    const collected = new Map<string, Task>();
    for (const row of remainingSlipped) {
      collected.set(row.id, row);
      if (!cascadeFor(row.list_id)) continue;
      for (const desc of actionableDescendants(row.id, tasks)) {
        collected.set(desc.id, desc);
      }
    }
    return [...collected.values()];
  }, [cascadeFor, remainingSlipped, tasks]);

  const bulkCarry = useCallback(
    async (newDate: string | null, announcementKey: string) => {
      if (remainingSlipped.length === 0) return;
      setBusy(true);
      const targets = collectBulkCarryTargets();
      const ids = targets.map((task) => task.id);
      queueFocusAfter(ids);
      try {
        for (const task of targets) {
          await updateTask({ ...task, scheduled_date: newDate });
        }
        setResolvedIds((s) => {
          const next = new Set(s);
          ids.forEach((id) => next.add(id));
          return next;
        });
        announce(t(announcementKey, { count: remainingSlipped.length }));
        invalidateData();
      } finally {
        setBusy(false);
      }
    },
    [announce, collectBulkCarryTargets, invalidateData, queueFocusAfter, remainingSlipped, t],
  );

  const allCarryToToday = useCallback(
    () => bulkCarry(todayIsoKey(), 'dialogs.dayStartReview.carryOver.announceAllToday'),
    [bulkCarry],
  );
  const allCarryToBacklog = useCallback(
    () => bulkCarry(null, 'dialogs.dayStartReview.carryOver.announceAllBacklog'),
    [bulkCarry],
  );

  // ── Delete ───────────────────────────────────────────────────────────────────
  // The only irreversible row action (done / backlog / today / tomorrow all
  // undo), so it confirms first — same as the calendar list's task delete.
  const deleteTaskAction = useCallback(
    (task: Task) => {
      Alert.alert(
        t('dialogs.dayStartReview.delete'),
        t('dialogs.dayStartReview.confirmDelete', { title: task.title }),
        [
          { text: t('dialogs.confirm.cancel'), style: 'cancel' },
          {
            text: t('dialogs.dayStartReview.delete'),
            style: 'destructive',
            onPress: () => {
              void (async () => {
                setBusy(true);
                queueFocusAfter([task.id]);
                try {
                  await apiDeleteTask(task.id, task.list_id);
                  setResolvedIds((s) => new Set(s).add(task.id));
                  announce(
                    t('dialogs.dayStartReview.announceDeleted', { title: task.title }),
                  );
                  invalidateData();
                } finally {
                  setBusy(false);
                }
              })();
            },
          },
        ],
      );
    },
    [announce, invalidateData, queueFocusAfter, t],
  );

  // ── Snooze ───────────────────────────────────────────────────────────────────

  const snoozeLater = useCallback(() => {
    // Reachable as the hardware-back handler even while the behaviour read is
    // still in flight (the visible "Remind me later" button only renders once
    // loaded). Don't burn a 4-hour snooze before the user saw a single row —
    // the day is already marked reviewed, so a plain close suffices.
    if (behaviour != null) {
      void snoozeDayStartReview(4);
      announce(t('dialogs.dayStartReview.snoozed'));
      // Re-plan the pre-scheduled day-start notifications so none fires into
      // the fresh snooze window (the scheduler reads the snooze end).
      refreshRemindersSoon();
    }
    onClose();
  }, [announce, behaviour, onClose, t]);

  // ── Render helpers ────────────────────────────────────────────────────────────

  const dateMeta = (task: Task, kind: 'deadline' | 'carryOver'): string | null => {
    const iso = kind === 'deadline' ? task.deadline_date : task.scheduled_date;
    if (!iso) return null;
    return t(
      kind === 'deadline'
        ? 'dialogs.dayStartReview.deadlines.dateLabel'
        : 'dialogs.dayStartReview.carryOver.dateLabel',
      { date: formatDate(iso) },
    );
  };

  const rowLabel = (task: Task, meta: string | null): string => {
    const base = `${task.title}${prioritySuffix(tr, task.priority, priorityScale)}`;
    return meta ? `${base}, ${meta}` : base;
  };

  const renderRow = (
    task: Task,
    kind: 'deadline' | 'carryOver',
    actions: { label: string; primary?: boolean; destructive?: boolean; onPress: () => void }[],
  ) => {
    const meta = dateMeta(task, kind);
    const marker = priorityMarker(task.priority, priorityScale);
    return (
      <View key={task.id} style={styles.row}>
        <Text
          ref={(node) => {
            rowTags.current[task.id] = node ? findNodeHandle(node) : null;
          }}
          accessible
          accessibilityRole="text"
          accessibilityLabel={rowLabel(task, meta)}
          style={styles.rowTitle}
        >
          {task.title}
          {marker !== '' ? (
            <Text style={styles.rowPriority} importantForAccessibility="no">
              {'  '}
              {marker}
            </Text>
          ) : null}
        </Text>
        {meta != null && (
          <Text style={styles.rowMeta} importantForAccessibility="no">
            {meta}
          </Text>
        )}
        <View style={styles.rowActions}>
          {actions.map((action) => (
            <Pressable
              key={action.label}
              accessibilityRole="button"
              accessibilityLabel={action.label}
              accessibilityState={{ disabled: busy }}
              disabled={busy}
              onPress={action.onPress}
              style={({ pressed }) => [
                styles.actionButton,
                action.primary
                  ? styles.actionPrimary
                  : action.destructive
                    ? styles.actionDestructive
                    : styles.actionGhost,
                pressed && !busy && styles.actionPressed,
                busy && styles.actionDisabled,
              ]}
            >
              <Text
                style={[
                  styles.actionText,
                  action.primary
                    ? styles.actionTextPrimary
                    : action.destructive
                      ? styles.actionTextDestructive
                      : styles.actionTextGhost,
                ]}
                importantForAccessibility="no"
              >
                {action.label}
              </Text>
            </Pressable>
          ))}
        </View>
      </View>
    );
  };

  return (
    <Modal
      visible={visible}
      animationType="slide"
      onRequestClose={remindersOnly ? onClose : snoozeLater}
      onShow={focusTitle}
    >
      <View style={[styles.screen, { paddingTop: insets.top + 12 }]}>
        <Text ref={titleRef} accessibilityRole="header" style={styles.title}>
          {t('dialogs.dayStartReview.title')}
        </Text>
        <Text style={styles.hint}>
          {t(
            remindersOnly
              ? 'dialogs.dayStartReview.hintRemindersOnly'
              : 'dialogs.dayStartReview.hint',
          )}
        </Text>

        {behaviour == null || (loading && totalRemaining === 0) ? (
          <View
            style={styles.center}
            accessible
            accessibilityRole="text"
            accessibilityLabel={t('mobile.loadingLabel')}
          >
            <ActivityIndicator />
            <Text style={styles.muted}>{t('mobile.loading')}</Text>
          </View>
        ) : (
          <ScrollView
            contentContainerStyle={[styles.list, { paddingBottom: insets.bottom + 24 }]}
            keyboardShouldPersistTaps="handled"
          >
            {hasReminders && (
              <View style={styles.section}>
                <Text accessibilityRole="header" style={styles.sectionHeading}>
                  {t('dialogs.dayStartReview.reminders.heading')}
                </Text>
                {reminderGroups.map((group) =>
                  group.tasks.length === 0 ? null : (
                    <View key={group.key} style={styles.reminderGroup}>
                      <Text style={styles.reminderSummary}>{group.summary}</Text>
                      {group.tasks.map((task) => (
                        <Pressable
                          key={task.id}
                          accessibilityRole="button"
                          accessibilityLabel={`${task.title}, ${group.why(task)}`}
                          onPress={() => openTaskEditor(task)}
                          style={({ pressed }) => [
                            styles.reminderRow,
                            pressed && styles.actionPressed,
                          ]}
                        >
                          <Text style={styles.reminderTaskTitle} importantForAccessibility="no">
                            {task.title}
                          </Text>
                        </Pressable>
                      ))}
                    </View>
                  ),
                )}
              </View>
            )}

            {remainingOverdue.length > 0 && (
              <View style={styles.section}>
                <Text accessibilityRole="header" style={styles.sectionHeading}>
                  {t('dialogs.dayStartReview.deadlines.heading', {
                    count: remainingOverdue.length,
                  })}
                </Text>
                {remainingOverdue.map((task) =>
                  renderRow(task, 'deadline', [
                    {
                      label: t('dialogs.dayStartReview.deadlines.actions.done'),
                      primary: true,
                      onPress: () => void markCompleted(task),
                    },
                    {
                      label: t('dialogs.dayStartReview.deadlines.actions.today'),
                      onPress: () => void deadlineToToday(task),
                    },
                    {
                      label: t('dialogs.dayStartReview.deadlines.actions.backlog'),
                      onPress: () => void backToBacklog(task),
                    },
                    {
                      label: t('dialogs.dayStartReview.delete'),
                      destructive: true,
                      onPress: () => deleteTaskAction(task),
                    },
                  ]),
                )}
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={t('dialogs.dayStartReview.deadlines.bulk.allDone')}
                  accessibilityState={{ disabled: busy }}
                  disabled={busy}
                  onPress={() => void completeAllOverdue()}
                  style={({ pressed }) => [
                    styles.bulkButton,
                    pressed && !busy && styles.actionPressed,
                    busy && styles.actionDisabled,
                  ]}
                >
                  <Text style={styles.bulkText} importantForAccessibility="no">
                    {t('dialogs.dayStartReview.deadlines.bulk.allDone')}
                  </Text>
                </Pressable>
              </View>
            )}

            {remainingSlipped.length > 0 && (
              <View style={styles.section}>
                <Text accessibilityRole="header" style={styles.sectionHeading}>
                  {t('dialogs.dayStartReview.carryOver.heading', {
                    count: remainingSlipped.length,
                  })}
                </Text>
                {remainingSlipped.map((task) =>
                  renderRow(task, 'carryOver', [
                    {
                      label: t('dialogs.dayStartReview.carryOver.actions.today'),
                      primary: true,
                      onPress: () => void carryToToday(task),
                    },
                    {
                      label: t('dialogs.dayStartReview.carryOver.actions.tomorrow'),
                      onPress: () => void carryToTomorrow(task),
                    },
                    {
                      label: t('dialogs.dayStartReview.carryOver.actions.backlog'),
                      onPress: () => void sendToBacklog(task),
                    },
                    {
                      label: t('dialogs.dayStartReview.carryOver.actions.done'),
                      onPress: () => void markCompleted(task),
                    },
                    {
                      label: t('dialogs.dayStartReview.delete'),
                      destructive: true,
                      onPress: () => deleteTaskAction(task),
                    },
                  ]),
                )}
                <View style={styles.bulkRow}>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('dialogs.dayStartReview.carryOver.bulk.allToday')}
                    accessibilityState={{ disabled: busy }}
                    disabled={busy}
                    onPress={() => void allCarryToToday()}
                    style={({ pressed }) => [
                      styles.bulkButton,
                      styles.bulkButtonFlex,
                      pressed && !busy && styles.actionPressed,
                      busy && styles.actionDisabled,
                    ]}
                  >
                    <Text style={styles.bulkText} importantForAccessibility="no">
                      {t('dialogs.dayStartReview.carryOver.bulk.allToday')}
                    </Text>
                  </Pressable>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('dialogs.dayStartReview.carryOver.bulk.allBacklog')}
                    accessibilityState={{ disabled: busy }}
                    disabled={busy}
                    onPress={() => void allCarryToBacklog()}
                    style={({ pressed }) => [
                      styles.bulkButton,
                      styles.bulkButtonFlex,
                      pressed && !busy && styles.actionPressed,
                      busy && styles.actionDisabled,
                    ]}
                  >
                    <Text style={styles.bulkText} importantForAccessibility="no">
                      {t('dialogs.dayStartReview.carryOver.bulk.allBacklog')}
                    </Text>
                  </Pressable>
                </View>
              </View>
            )}

            <Pressable
              accessibilityRole="button"
              accessibilityLabel={t(
                remindersOnly
                  ? 'dialogs.dayStartReview.acknowledge'
                  : 'dialogs.dayStartReview.snooze',
              )}
              onPress={remindersOnly ? onClose : snoozeLater}
              style={({ pressed }) => [
                styles.snoozeButton,
                remindersOnly && styles.acknowledgeButton,
                pressed && styles.actionPressed,
              ]}
            >
              <Text
                style={[styles.snoozeText, remindersOnly && styles.acknowledgeText]}
                importantForAccessibility="no"
              >
                {t(
                  remindersOnly
                    ? 'dialogs.dayStartReview.acknowledge'
                    : 'dialogs.dayStartReview.snooze',
                )}
              </Text>
            </Pressable>
          </ScrollView>
        )}
      </View>
    </Modal>
  );
}

/** Local `YYYY-MM-DD` for tomorrow — the "Tomorrow" carry-over target. */
function tomorrowIsoKey(): string {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background, paddingHorizontal: 16 },
    title: { fontSize: 24, fontWeight: '800', color: c.textPrimary },
    hint: { fontSize: 15, color: c.textSecondary, marginTop: 8, marginBottom: 8 },
    center: { alignItems: 'center', gap: 8, paddingVertical: 32 },
    muted: { fontSize: 15, color: c.textSecondary },
    list: { gap: 20, paddingTop: 8 },
    section: { gap: 10 },
    sectionHeading: { fontSize: 18, fontWeight: '700', color: c.textPrimary },
    // Read-only reminders: a count summary per group, then each task as a tap
    // target that opens its editor. Lighter chrome than the actionable rows —
    // these inform, they don't carry per-row buttons.
    reminderGroup: { gap: 6 },
    reminderSummary: { fontSize: 15, fontWeight: '600', color: c.textSecondary },
    reminderRow: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      minHeight: 44,
      justifyContent: 'center',
    },
    reminderTaskTitle: { fontSize: 16, fontWeight: '600', color: c.link },
    row: {
      gap: 8,
      padding: 14,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowTitle: { fontSize: 17, fontWeight: '600', color: c.textPrimary },
    // Neutral marker — the glyph count (! vs !!!) conveys the level; colour
    // stays neutral so "low" isn't mis-signalled as danger. The SR label spells
    // the priority out via prioritySuffix.
    rowPriority: { fontSize: 15, color: c.textSecondary, fontWeight: '700' },
    rowMeta: { fontSize: 14, color: c.textSecondary },
    rowActions: { flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginTop: 4 },
    actionButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      minHeight: 44,
      justifyContent: 'center',
      alignItems: 'center',
    },
    actionPrimary: { backgroundColor: c.accent },
    actionGhost: { borderWidth: 1, borderColor: c.border, backgroundColor: c.surface },
    actionDestructive: { borderWidth: 1, borderColor: c.danger, backgroundColor: c.surface },
    actionPressed: { backgroundColor: c.surfacePressed },
    actionDisabled: { opacity: 0.5 },
    actionText: { fontSize: 15, fontWeight: '600' },
    actionTextPrimary: { color: c.textOnAccent },
    actionTextGhost: { color: c.link },
    actionTextDestructive: { color: c.danger },
    bulkRow: { flexDirection: 'row', gap: 8 },
    bulkButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceSubtle,
      minHeight: 44,
      justifyContent: 'center',
      alignItems: 'center',
    },
    bulkButtonFlex: { flex: 1 },
    bulkText: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    snoozeButton: {
      paddingVertical: 14,
      paddingHorizontal: 16,
      borderRadius: 10,
      alignItems: 'center',
      marginTop: 8,
    },
    snoozeText: { fontSize: 16, fontWeight: '600', color: c.link },
    // Reminders-only: OK is the modal's PRIMARY and only explicit exit, so it
    // gets a filled button instead of the borderless link styling — otherwise
    // it reads as just another link like the reminder rows above it.
    acknowledgeButton: {
      backgroundColor: c.accent,
      minHeight: 44,
      justifyContent: 'center',
    },
    acknowledgeText: { color: c.textOnAccent },
  });
