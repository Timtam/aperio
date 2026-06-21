import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityActionEvent,
  AccessibilityInfo,
  ActivityIndicator,
  Alert,
  FlatList,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import AsyncStorage from '@react-native-async-storage/async-storage';

import type { Entry, Task } from '@aperio/shared';
import {
  assigneeSuffix,
  buildEntries,
  DEFERRED_GROUP_ID,
  DONE_GROUP_ID,
  prioritySuffix,
  statusI18nKey,
  statusMarker,
  subtaskProgressSuffix,
} from '@aperio/shared';

import { expandedA11y } from '../a11y/roles';
import { deleteTask, duplicateTask } from '../api/client';
import { useCurrentDayKey } from '../hooks/useCurrentDayKey';
import { useTabBarInset } from '../hooks/useTabBarInset';
import { describeDue } from '../intl/describeDue';
import { resolveTaskColor, sectionColorMap } from '../intl/taskColor';
import { useCacheReload } from '../state/cacheObserver';
import { useCurrentUserByList } from '../state/currentUser';
import { useTaskStore } from '../state/taskStoreContext';
import { surfaceTaskNow } from '../state/moveActions';
import { applyTaskToggle, recomputeAncestors, statusAnnounce } from '../state/taskToggle';
import { useTasks } from '../state/useTasks';
import { useThemedStyles, type ThemeColors } from '../theme';
import type { RootStackScreenProps } from '../navigation/types';

// The grouped tasks tree — a faithful port of the desktop TaskView, adapted to
// the RN per-element + custom-actions a11y idiom (no aria-tree / roving focus).
// Grouping + labels are NOT reimplemented: buildEntries + the status/label
// helpers come from @aperio/shared, exactly as the desktop calls them. Group
// headers (Backlog / per-list / per-section / Upcoming / Done) are collapsible
// buttons; task rows carry the rich shared label + complete/edit/delete (and a
// subtask-collapse action for parents). The rich editor is sub-4, section/list
// management sub-5.

// Only Done + Upcoming collapse state persists (per the desktop); per-list,
// per-section and subtask twisties stay session-local.
const DONE_COLLAPSE_KEY = 'aperio.tasks.doneCollapsed';
const DEFERRED_COLLAPSE_KEY = 'aperio.tasks.deferredCollapsed';

export default function TasksScreen({
  navigation,
}: RootStackScreenProps<'Tasks'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const tabBarInset = useTabBarInset();
  const { tasks, loading, taskListById } = useTasks();
  const {
    selectedTaskListIds,
    taskLists,
    sectionsByList,
    loadSections,
    colorLabels,
    invalidateData,
  } = useTaskStore();
  // A cross-device sync round applies a peer's task changes to the local store;
  // reload (bump dataVersion) when the sync layer signals a data change, so joined
  // / synced tasks appear without an app restart. Other screens reload via their
  // own useCacheReload; the Tasks screen drives reloads through dataVersion.
  useCacheReload('tasks', invalidateData);

  // The shared helpers + buildEntries take a plain (key, vars) => string; adapt
  // i18next's t (whose overload union isn't directly assignable).
  const tr = useCallback(
    (key: string, vars?: Record<string, unknown>): string => t(key, vars) as string,
    [t],
  );

  // Localized, time-free date formatter — Intl approximates the desktop's
  // date-fns 'PP' (no date-fns dependency on mobile).
  const formatDate = useMemo(() => {
    const f = new Intl.DateTimeFormat(i18n.language, {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    });
    return (iso: string) => f.format(new Date(iso));
  }, [i18n.language]);

  const today = useCurrentDayKey();

  // Collapsed group/subtask ids. Done + Upcoming start collapsed (desktop
  // default); hydrated from storage, which can only EXPAND them (explicit
  // 'false') so a not-yet-loaded read never flickers them open.
  const [collapsed, setCollapsed] = useState<Set<string>>(
    () => new Set([DONE_GROUP_ID, DEFERRED_GROUP_ID]),
  );
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const [doneRaw, deferredRaw] = await Promise.all([
        AsyncStorage.getItem(DONE_COLLAPSE_KEY),
        AsyncStorage.getItem(DEFERRED_COLLAPSE_KEY),
      ]);
      if (cancelled) return;
      if (doneRaw === 'false' || deferredRaw === 'false') {
        setCollapsed((prev) => {
          const next = new Set(prev);
          if (doneRaw === 'false') next.delete(DONE_GROUP_ID);
          if (deferredRaw === 'false') next.delete(DEFERRED_GROUP_ID);
          return next;
        });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Spoken feedback (plain announce, not a live region).
  const announce = useCallback((message: string) => {
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  // Managed screen-reader focus: rows + headers register their node handles so
  // focus can be restored after a mutation refetch (rows) or a collapse
  // (headers stay mounted). pendingEmptyFocus lands on the empty-state message
  // when a delete empties the list.
  const rowTags = useRef<Record<string, number | null>>({});
  const headerTags = useRef<Record<string, number | null>>({});
  const emptyRef = useRef<Text>(null);
  const pendingFocusId = useRef<string | null>(null);
  const pendingEmptyFocus = useRef(false);

  // Load sections for every list that has tasks so buildEntries can group by
  // section. Mirrors TaskView's sections-loading effect; lazy + sticky.
  const listIdsWithTasks = useMemo(
    () => Array.from(new Set(tasks.map((task) => task.list_id))),
    [tasks],
  );
  useEffect(() => {
    listIdsWithTasks.forEach((listId) => {
      if (!(listId in sectionsByList)) void loadSections(listId);
    });
  }, [listIdsWithTasks, sectionsByList, loadSections]);

  const currentUserByList = useCurrentUserByList(tasks);
  const entries = useMemo(
    () =>
      buildEntries(
        tasks,
        taskListById,
        tr,
        collapsed,
        sectionsByList,
        today,
        currentUserByList,
      ).entries,
    [tasks, taskListById, tr, collapsed, sectionsByList, today, currentUserByList],
  );

  // What the virtualized list renders: only the NON-hidden entries (children of
  // a collapsed group/parent are out of the tree entirely). buildEntries keeps
  // the hidden entries in `entries` for the focus-walk helpers below, so filter
  // here. The FlatList then mounts only the on-screen window — so a list with
  // thousands of (e.g. device-reminder) rows no longer balloons the VoiceOver
  // accessibility tree the way the old all-rows-mounted ScrollView did.
  const visibleEntries = useMemo(
    () => entries.filter((entry) => !entry.hidden),
    [entries],
  );

  // Colour resolution (task own → section → list), matching the desktop. The
  // palette + every section's bound hex feed resolveTaskColor; recolouring a
  // label recolours every task that inherits it, with no per-task write.
  const labelsById = useMemo(
    () => new Map(colorLabels.map((l) => [l.id, l])),
    [colorLabels],
  );
  const sectionColorById = useMemo(
    () => sectionColorMap(Object.values(sectionsByList).flat(), labelsById),
    [sectionsByList, labelsById],
  );

  // Restore focus to a sibling row after a mutation refetch remounts the list.
  useEffect(() => {
    if (pendingFocusId.current != null) {
      const id = pendingFocusId.current;
      pendingFocusId.current = null;
      pendingEmptyFocus.current = false;
      const tag = rowTags.current[id] ?? headerTags.current[id];
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
      return;
    }
    if (pendingEmptyFocus.current) {
      pendingEmptyFocus.current = false;
      const tag = findNodeHandle(emptyRef.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    }
  }, [entries]);

  const targetListId = [...selectedTaskListIds][0] ?? taskLists[0]?.id ?? null;

  const openEditor = useCallback(
    (task: Task) =>
      navigation.navigate('TaskEditor', { taskId: task.id, listId: task.list_id }),
    [navigation],
  );

  // Plan-from-backlog quick scheduler (Today / Tomorrow / Next Monday / custom /
  // back-to-backlog) — the desktop's Shift+D affordance as a row action.
  const openPlan = useCallback(
    (task: Task) =>
      navigation.navigate('PlanTask', { taskId: task.id, listId: task.list_id }),
    [navigation],
  );

  // Move / copy the task (and its subtasks) to another list — the desktop's
  // Shift+M affordance as a row action.
  const openMoveCopy = useCallback(
    (task: Task) =>
      navigation.navigate('MoveCopy', {
        kind: 'task',
        taskId: task.id,
        listId: task.list_id,
      }),
    [navigation],
  );

  // Bring a deferred task back into the active backlog now (clears its future
  // resurface date) — the desktop's "Ins Backlog holen" chip action.
  const surfaceNow = useCallback(
    async (task: Task) => {
      try {
        await surfaceTaskNow(task);
        invalidateData();
        announce(t('chipMenu.broughtToBacklog', { title: task.title }));
      } catch (err) {
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, invalidateData, t],
  );

  const newTask = useCallback(() => {
    // No list at all → send the user to create one first. Otherwise open the
    // quick-add (it resolves its own default list — last-used, else first
    // writable — and offers "More details …" for the full editor).
    if (targetListId == null) {
      navigation.navigate('Lists');
      return;
    }
    navigation.navigate('QuickAdd');
  }, [navigation, targetListId]);

  // Create a child task under `task` (same list, locked). buildEntries nests it
  // by parent_id, which round-trips through create.
  const addSubtask = useCallback(
    (task: Task) =>
      navigation.navigate('TaskEditor', {
        taskId: null,
        listId: task.list_id,
        parentId: task.id,
      }),
    [navigation],
  );

  const toggleDone = useCallback(
    async (task: Task) => {
      // The check-off honours the synced task-behaviour knobs (mode, status
      // coupling, auto-date) via the shared toggle path; it returns the new
      // status (or null if nothing changed).
      try {
        const next = await applyTaskToggle(task, taskListById.get(task.list_id), tasks);
        if (next == null) return;
        // Keep SR focus sensible after the refetch remounts the list: a task
        // that became completed slips into the (collapsed) Done group, so land
        // focus on a surviving sibling — or the Done header when it was the
        // only visible task; otherwise the row stays visible, so keep focus on
        // it. Set just before the refetch so the pending-focus effect picks it.
        pendingFocusId.current =
          next === 'completed'
            ? (focusTargetAfterRemoving(entries, task.id) ?? DONE_GROUP_ID)
            : task.id;
        invalidateData();
        announce(statusAnnounce(t, next, task.title));
      } catch (err) {
        pendingFocusId.current = null;
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, entries, invalidateData, t, tasks, taskListById],
  );

  const removeTask = useCallback(
    (task: Task) => {
      const performDelete = async () => {
        // Land focus on a surviving sibling (excluding the deleted task's own
        // subtree, which cascades away with it), or the empty-state message.
        const siblingId = focusTargetAfterRemoving(entries, task.id);
        if (siblingId) {
          pendingFocusId.current = siblingId;
        } else {
          pendingEmptyFocus.current = true;
        }
        try {
          await deleteTask(task.id, task.list_id);
          // A subtask's removal can change the parent's derived status (e.g. the
          // last open child gone → parent completes). Recompute ancestors against
          // the post-deletion snapshot, honouring the coupling knob.
          if (task.parent_id != null) {
            await recomputeAncestors(
              task.parent_id,
              tasks.filter((t) => t.id !== task.id),
            );
          }
          invalidateData();
          announce(t('mobile.deleted', { title: task.title }));
        } catch (err) {
          pendingFocusId.current = null;
          pendingEmptyFocus.current = false;
          announce(t('mobile.error', { message: errorMessage(err) }));
        }
      };
      // Confirm the irreversible delete — matches the desktop ConfirmDialog.
      // Alert is screen-reader-accessible and moves focus to itself.
      Alert.alert(
        t('dialogs.confirm.deleteTaskTitle'),
        t('dialogs.confirm.deleteTaskMessage', { title: task.title }),
        [
          { text: t('dialogs.confirm.cancel'), style: 'cancel' },
          {
            text: t('mobile.delete'),
            style: 'destructive',
            onPress: () => void performDelete(),
          },
        ],
      );
    },
    [announce, entries, invalidateData, t, tasks],
  );

  // Duplicate a task (flat copy in the same list); land SR focus on the copy
  // once the refetch remounts the list.
  const duplicate = useCallback(
    async (task: Task) => {
      try {
        const created = await duplicateTask(task);
        pendingFocusId.current = created.id;
        invalidateData();
        announce(t('actions.duplicated', { title: task.title }));
      } catch (err) {
        pendingFocusId.current = null;
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, invalidateData, t],
  );

  // Toggle a group header (sentinel/synthetic id) or a subtask parent (task id).
  const toggleCollapsed = useCallback(
    (id: string, title: string) => {
      const willExpand = collapsed.has(id);
      setCollapsed((prev) => {
        const next = new Set(prev);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        if (id === DONE_GROUP_ID) {
          void AsyncStorage.setItem(DONE_COLLAPSE_KEY, String(next.has(id)));
        } else if (id === DEFERRED_GROUP_ID) {
          void AsyncStorage.setItem(DEFERRED_COLLAPSE_KEY, String(next.has(id)));
        }
        return next;
      });
      announce(
        willExpand
          ? t('mobile.groupExpanded', { name: title })
          : t('mobile.groupCollapsed', { name: title }),
      );
      // The toggled header/row stays mounted (stable key) — only its children
      // unmount — so TalkBack/VoiceOver retain focus on it. We deliberately do
      // NOT force focus back: refocusing the already-focused node re-speaks its
      // title on iOS and would step on the expanded/collapsed announcement.
    },
    [announce, collapsed, t],
  );

  const onAction = useCallback(
    (entry: Entry, event: AccessibilityActionEvent) => {
      const task = entry.task;
      switch (event.nativeEvent.actionName) {
        case 'toggle':
          void toggleDone(task);
          break;
        case 'edit':
          openEditor(task);
          break;
        case 'delete':
          void removeTask(task);
          break;
        case 'duplicate':
          void duplicate(task);
          break;
        case 'plan':
          openPlan(task);
          break;
        case 'moveCopy':
          openMoveCopy(task);
          break;
        case 'surface':
          void surfaceNow(task);
          break;
        case 'addSubtask':
          addSubtask(task);
          break;
        case 'toggleCollapse':
          toggleCollapsed(task.id, task.title);
          break;
      }
    },
    [
      addSubtask,
      duplicate,
      openEditor,
      openPlan,
      openMoveCopy,
      surfaceNow,
      removeTask,
      toggleCollapsed,
      toggleDone,
    ],
  );

  const taskLabel = (task: Task, colourName: string | null): string => {
    const base = t('views.tasks.optionLabel', {
      title: task.title,
      state: t(statusI18nKey(task.status)),
      priority: prioritySuffix(tr, task.priority),
      progress: subtaskProgressSuffix(tr, task.id, tasks),
      due: describeDue(task, tr, today, formatDate),
      assignee: assigneeSuffix(tr, task.assignees),
    });
    // A bound colour is meaningless to a screen reader as a colour, so announce
    // its label NAME instead — only the task's OWN explicit label is named (an
    // inherited section/list tint is a grouping cue, not a per-task signal).
    return colourName
      ? `${base}${t('mobile.colorLabelSuffix', { name: colourName })}`
      : base;
  };

  const renderHeader = (entry: Entry) => {
    const id = entry.task.id;
    const isCollapsed = collapsed.has(id);
    return (
      <Pressable
        key={id}
        ref={(node) => {
          headerTags.current[id] = node ? findNodeHandle(node) : null;
        }}
        accessible
        // A VoiceOver/TalkBack HEADING so the group regions (Backlog / lists /
        // sections / Upcoming / Done) are reachable via the headings rotor —
        // swipe-jump between them instead of scrolling through every task. It
        // still toggles on double-tap (onPress) + announces its expanded/
        // collapsed state (via expandedA11y — RN speaks the iOS state word in
        // English regardless of locale, so we localize it ourselves); a header
        // isn't a "button", so it carries the explicit toggle hint itself rather
        // than relying on the native button hint.
        accessibilityRole="header"
        accessibilityLabel={entry.task.title}
        accessibilityHint={t('mobile.groupHeaderHint')}
        {...(entry.hasChildren
          ? expandedA11y(
              !isCollapsed,
              t(isCollapsed ? 'mobile.collapsedState' : 'mobile.expandedState'),
            )
          : {})}
        onPress={() => toggleCollapsed(id, entry.task.title)}
        style={({ pressed }) => [
          styles.header,
          { paddingLeft: 16 + entry.depth * 16 },
          pressed && styles.rowPressed,
        ]}
      >
        <Text style={styles.twisty} importantForAccessibility="no">
          {isCollapsed ? '▸' : '▾'}
        </Text>
        <Text style={styles.headerTitle} importantForAccessibility="no">
          {entry.task.title}
        </Text>
      </Pressable>
    );
  };

  const renderTask = (entry: Entry) => {
    const task = entry.task;
    const done = task.status === 'completed';
    // The task's resolved colour (own label → section → list) — a coloured dot
    // for sighted users; SR users get the OWN label's name in taskLabel().
    const resolved = resolveTaskColor(task, taskListById, labelsById, sectionColorById);
    const taskHex = resolved.hex ?? undefined;
    const actions = [
      { name: 'toggle', label: done ? t('mobile.reopen') : t('mobile.complete') },
      { name: 'edit', label: t('mobile.rename') },
      { name: 'delete', label: t('mobile.delete') },
      { name: 'duplicate', label: t('mobile.duplicate') },
      { name: 'plan', label: t('mobile.plan') },
      { name: 'moveCopy', label: t('mobile.moveCopy') },
    ];
    // "Bring to backlog" — only for a deferred task (a future resurface date);
    // clearing it pulls the task back into the active backlog now.
    if (task.resurface_date != null) {
      actions.push({ name: 'surface', label: t('chipMenu.bringToBacklog') });
    }
    // Subtasks ride parent_id on the local store; offer "Add subtask" only for
    // local lists (external subtask support is provider-dependent — deferred).
    if (taskListById.get(task.list_id)?.account_id === 'local') {
      actions.push({ name: 'addSubtask', label: t('mobile.addSubtask') });
    }
    if (entry.hasChildren) {
      actions.push({
        name: 'toggleCollapse',
        label: collapsed.has(task.id)
          ? t('views.tasks.expand', { title: task.title })
          : t('views.tasks.collapse', { title: task.title }),
      });
    }
    return (
      <Pressable
        key={task.id}
        ref={(node) => {
          rowTags.current[task.id] = node ? findNodeHandle(node) : null;
        }}
        accessible
        accessibilityRole="button"
        accessibilityLabel={taskLabel(task, resolved.labelName)}
        accessibilityHint={t('mobile.taskHint')}
        {...(entry.hasChildren
          ? expandedA11y(
              !collapsed.has(task.id),
              t(collapsed.has(task.id) ? 'mobile.collapsedState' : 'mobile.expandedState'),
            )
          : {})}
        accessibilityActions={actions}
        onAccessibilityAction={(event) => onAction(entry, event)}
        onPress={() => openEditor(task)}
        style={({ pressed }) => [
          styles.task,
          { marginLeft: entry.depth * 16 },
          pressed && styles.rowPressed,
        ]}
      >
        {/* Sighted tap target to complete/reopen the task. The whole row's
            onPress opens the editor, so the status marker needs its own
            Pressable — otherwise tapping it just opened the editor and there was
            no way to check a task off by sight. SR users use the row's "toggle"
            custom action instead, so this stays out of the accessibility tree. */}
        <Pressable
          accessible={false}
          importantForAccessibility="no"
          onPress={() => void toggleDone(task)}
          hitSlop={10}
          style={({ pressed }) => [styles.taskCheckButton, pressed && styles.rowPressed]}
        >
          <Text style={styles.taskCheck} importantForAccessibility="no">
            {statusMarker(task.status)}
          </Text>
        </Pressable>
        {taskHex != null && (
          <View
            accessible={false}
            importantForAccessibility="no"
            style={[styles.colorDot, { backgroundColor: taskHex }]}
          />
        )}
        <View style={styles.taskBody}>
          <Text
            style={[styles.taskTitle, done && styles.taskTitleDone]}
            importantForAccessibility="no"
          >
            {task.title}
          </Text>
          <Text style={styles.taskMeta} importantForAccessibility="no">
            {describeDue(task, tr, today, formatDate)}
          </Text>
        </View>
      </Pressable>
    );
  };

  return (
    <View style={styles.screen}>
      <View style={styles.toolbar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.newTaskLabel')}
          onPress={newTask}
          style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
        >
          <Text style={styles.buttonText}>{t('mobile.add')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.listsButtonLabel')}
          onPress={() => navigation.navigate('Lists')}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.rowPressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.listsButtonLabel')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.search.title')}
          onPress={() => navigation.navigate('Search')}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.rowPressed]}
        >
          <Text style={styles.ghostButtonText}>{t('toolbar.search')}</Text>
        </Pressable>
        {/* Calendar / Contacts / Settings are now bottom tabs, not toolbar
            buttons — the Tasks toolbar keeps only its own actions (Add + Lists). */}
      </View>

      {loading ? (
        <View
          style={styles.center}
          accessible
          accessibilityRole="text"
          accessibilityLabel={t('mobile.loadingLabel')}
        >
          <ActivityIndicator />
          <Text style={styles.muted}>{t('mobile.loading')}</Text>
        </View>
      ) : entries.length === 0 ? (
        <Text ref={emptyRef} accessibilityRole="text" style={styles.muted}>
          {t('mobile.empty')}
        </Text>
      ) : (
        <FlatList
          accessibilityRole="list"
          data={visibleEntries}
          keyExtractor={(entry) => entry.task.id}
          renderItem={({ item }) =>
            item.group ? renderHeader(item) : renderTask(item)
          }
          contentContainerStyle={[styles.list, { paddingBottom: tabBarInset }]}
          keyboardShouldPersistTaps="handled"
          // Virtualization budget mirrors the contacts SectionList: keep the
          // mounted window small so the screen-reader tree stays light even with
          // a few thousand rows; the focus-restoration targets (the adjacent
          // sibling after a delete/complete) sit well inside the window.
          initialNumToRender={20}
          windowSize={11}
          removeClippedSubviews
        />
      )}
    </View>
  );
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * The id of the next (or previous) VISIBLE task row to land screen-reader focus
 * on after `taskId` leaves the visible list (deleted, or completed into a
 * collapsed group). Excludes `taskId` AND its whole subtree — a parent's
 * subtasks move/cascade away with it, so they're never valid focus targets.
 * Returns `null` when no other visible task remains.
 */
function focusTargetAfterRemoving(entries: Entry[], taskId: string): string | null {
  const excluded = new Set<string>([taskId]);
  const idx = entries.findIndex((e) => !e.group && e.task.id === taskId);
  if (idx >= 0) {
    const depth = entries[idx].depth;
    for (let i = idx + 1; i < entries.length; i += 1) {
      if (entries[i].depth <= depth) break;
      if (!entries[i].group) excluded.add(entries[i].task.id);
    }
  }
  const visible = entries.filter((e) => !e.hidden && !e.group);
  const pos = visible.findIndex((e) => e.task.id === taskId);
  if (pos < 0) return null;
  const after = visible.slice(pos + 1).find((e) => !excluded.has(e.task.id));
  if (after) return after.task.id;
  for (let i = pos - 1; i >= 0; i -= 1) {
    if (!excluded.has(visible[i].task.id)) return visible[i].task.id;
  }
  return null;
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    toolbar: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 10,
      paddingHorizontal: 16,
      paddingVertical: 12,
    },
    list: { gap: 8, padding: 16 },
    button: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    buttonPressed: { backgroundColor: c.accentPressed },
    buttonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    center: { alignItems: 'center', gap: 8, paddingVertical: 24 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    header: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 12,
      paddingRight: 16,
      borderRadius: 8,
      backgroundColor: c.surfaceSubtle,
    },
    twisty: { fontSize: 16, width: 18, textAlign: 'center', color: c.textLabel },
    headerTitle: { flex: 1, fontSize: 16, fontWeight: '700', color: c.textPrimary },
    task: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 14,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowPressed: { backgroundColor: c.surfacePressed },
    taskCheckButton: { borderRadius: 8, padding: 4 },
    taskCheck: { fontSize: 20, width: 26, textAlign: 'center', color: c.textPrimary },
    // A small colour dot for the task's bound colour label (sighted users).
    colorDot: {
      width: 12,
      height: 12,
      borderRadius: 6,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    taskBody: { flex: 1 },
    taskTitle: { fontSize: 18, color: c.textPrimary },
    taskTitleDone: { textDecorationLine: 'line-through', color: c.textSecondary },
    taskMeta: { fontSize: 14, color: c.textSecondary, marginTop: 2 },
  });
