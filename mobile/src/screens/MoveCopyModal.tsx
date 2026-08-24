import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { Section, Task } from '@aperio/shared';
import { isSeriesOccurrence, selectableTaskLists } from '@aperio/shared';

import { Calendar, listCalendars } from '../api/calendar';
import {
  createSection,
  getSections,
  getTaskById,
  getTasks,
} from '../api/client';
import { FormScrollView } from '../components/FormScrollView';
import { SelectFieldButton } from '../components/SelectFieldButton';
import { SegmentedSelect } from '../components/SegmentedSelect';
import { useCancelHeader } from '../components/useCancelHeader';
import {
  moveOrCopyEvent,
  moveOrCopyTask,
  type MoveCopyMode,
  type MoveCopyScope,
} from '../state/moveActions';
import {
  useShowHiddenCalendarTargets,
  useShowHiddenTaskListTargets,
} from '../settings/hiddenTargets';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { canAssignSection } from '../state/taskBehaviour';
import { useTaskStore } from '../state/taskStoreContext';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// Move / copy a task to another list or an event to another calendar — the RN
// port of the desktop MoveCopyDialog. Subtasks always travel with their parent
// (the app treats a split family as impossible), so there's no opt-out — just a
// one-line info so an N-row mutation isn't a surprise. A move into the same
// container is a no-op the user can still confirm. Screen-reader-first: mode +
// scope as segmented controls, the target as a labelled radio list with the
// current container marked.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function MoveCopyModal({
  route,
  navigation,
}: RootStackScreenProps<'MoveCopy'>) {
  const params = route.params;
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { taskLists, selectedTaskListIds, invalidateData } = useTaskStore();
  const { hidden: hiddenCalendars } = useCalendarVisibility();
  const showHiddenCalendarTargets = useShowHiddenCalendarTargets();
  const showHiddenTaskListTargets = useShowHiddenTaskListTargets();
  useCancelHeader(navigation);

  const [task, setTask] = useState<Task | null>(null);
  const [children, setChildren] = useState<Task[]>([]);
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [mode, setMode] = useState<MoveCopyMode>('move');
  const [scope, setScope] = useState<MoveCopyScope>('occurrence');
  const [targetContainerId, setTargetContainerId] = useState(
    params.kind === 'task' ? params.listId : params.event.calendar_id,
  );
  /** The section inside the target list; `''` is "no section". */
  const [targetSectionId, setTargetSectionId] = useState('');
  const [sections, setSections] = useState<Section[]>([]);
  /** Non-null while the "new section" name box is open. */
  const [sectionDraft, setSectionDraft] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const initialContainerId =
    params.kind === 'task' ? params.listId : params.event.calendar_id;
  const isRecurringOccurrence =
    params.kind === 'event' && isSeriesOccurrence(params.event);

  // Load: a task is re-fetched by id (+ its direct children); event surfaces just
  // need the calendar list (the event itself rode in via params).
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        if (params.kind === 'task') {
          const [loaded, siblings] = await Promise.all([
            getTaskById(params.taskId, params.listId),
            getTasks(params.listId),
          ]);
          if (cancelled) return;
          if (loaded == null) {
            setError(t('mobile.taskMissing'));
            AccessibilityInfo.announceForAccessibility(t('mobile.taskMissing'));
            return;
          }
          setTask(loaded);
          setChildren(siblings.filter((row) => row.parent_id === params.taskId));
        } else {
          const cals = await listCalendars();
          if (!cancelled) setCalendars(cals);
        }
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [params, t]);

  // Sections belong to the TARGET list: what is being chosen is where the
  // task lands, not where it came from.
  const targetList = useMemo(
    () => taskLists.find((l) => l.id === targetContainerId),
    [taskLists, targetContainerId],
  );
  const sectionsEnabled =
    params.kind === 'task' && canAssignSection(targetList);
  // Creating one needs the adapter to say it can; a provider that exposes
  // sections read-only still gets the picker.
  const sectionsManageable =
    sectionsEnabled && !!targetList?.task_capabilities?.manageable_sections;

  useEffect(() => {
    // A section id means something only inside its own list, so switching the
    // target drops the choice rather than carrying a stale one across.
    setTargetSectionId('');
    setSectionDraft(null);
    if (!sectionsEnabled) {
      setSections([]);
      return;
    }
    let cancelled = false;
    void getSections(targetContainerId)
      .then((rows) => {
        if (!cancelled) setSections(rows);
      })
      .catch(() => {
        // No sections offered this round; the move itself still works.
        if (!cancelled) setSections([]);
      });
    return () => {
      cancelled = true;
    };
  }, [targetContainerId, sectionsEnabled]);

  /**
   * Make a section in the target list and select it.
   *
   * Immediate, like the editor's: the section is a real row the moment it is
   * named, and the move that follows files into it. Position 0 puts one made
   * in passing at the top, where the thing being filed is about to be.
   */
  const createTargetSection = useCallback(async () => {
    const name = (sectionDraft ?? '').trim();
    if (!name) {
      setSectionDraft(null);
      return;
    }
    try {
      const created = await createSection({
        list_id: targetContainerId,
        name,
        position: 0,
      });
      setSections(await getSections(targetContainerId));
      setTargetSectionId(created.id);
      setSectionDraft(null);
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.task.section.created', { name: created.name }),
      );
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [sectionDraft, targetContainerId, t]);

  const sectionOptions = useMemo(
    () => [
      { value: '', label: t('dialogs.task.noSection') },
      ...sections.map((section) => ({ value: section.id, label: section.name })),
    ],
    [sections, t],
  );

  const containerOptions = useMemo(() => {
    // Targets must accept writes AND still be checked in the catalog — the
    // same set the editors' pickers offer (shared `selectableTaskLists` for
    // lists, the hidden-set filter for calendars, both matching desktop).
    const writable =
      params.kind === 'task'
        ? selectableTaskLists(taskLists, {
            selectedIds: selectedTaskListIds,
            includeHidden: showHiddenTaskListTargets,
          }).map((l) => ({ id: l.id, name: l.name }))
        : calendars
            .filter(
              (c) =>
                !c.read_only &&
                (showHiddenCalendarTargets || !hiddenCalendars.has(c.id)),
            )
            .map((c) => ({ id: c.id, name: c.name }));
    return writable.map((c) => ({
      value: c.id,
      label:
        c.id === initialContainerId
          ? `${c.name} ${t('dialogs.moveCopy.currentSuffix')}`
          : c.name,
    }));
  }, [
    params.kind,
    taskLists,
    selectedTaskListIds,
    calendars,
    hiddenCalendars,
    showHiddenCalendarTargets,
    showHiddenTaskListTargets,
    initialContainerId,
    t,
  ]);

  const itemTitle = params.kind === 'task' ? (task?.title ?? '') : params.event.title;

  const modeOptions = useMemo(
    () => [
      { value: 'move' as const, label: t('dialogs.moveCopy.modeMove') },
      { value: 'copy' as const, label: t('dialogs.moveCopy.modeCopy') },
    ],
    [t],
  );
  const scopeOptions = useMemo(
    () => [
      { value: 'occurrence' as const, label: t('dialogs.moveCopy.scopeOccurrence') },
      { value: 'series' as const, label: t('dialogs.moveCopy.scopeSeries') },
    ],
    [t],
  );

  const submit = useCallback(async () => {
    setError(null);
    if (!targetContainerId) {
      setError(t('dialogs.moveCopy.targetRequired'));
      AccessibilityInfo.announceForAccessibility(t('dialogs.moveCopy.targetRequired'));
      return;
    }
    if (mode === 'move' && targetContainerId === initialContainerId) {
      navigation.goBack(); // same container — a confirmable no-op
      return;
    }
    setSubmitting(true);
    try {
      if (params.kind === 'event') {
        await moveOrCopyEvent(params.event, targetContainerId, mode, scope);
      } else if (task != null) {
        await moveOrCopyTask(
          task,
          targetContainerId,
          mode,
          children,
          sectionsEnabled ? targetSectionId || null : null,
        );
      }
      invalidateData();
      AccessibilityInfo.announceForAccessibility(
        t(mode === 'move' ? 'dialogs.moveCopy.moved' : 'dialogs.moveCopy.copied', {
          title: itemTitle,
        }),
      );
      navigation.goBack();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      AccessibilityInfo.announceForAccessibility(t('mobile.error', { message }));
    } finally {
      setSubmitting(false);
    }
  }, [
    targetContainerId,
    mode,
    scope,
    initialContainerId,
    params,
    task,
    children,
    itemTitle,
    invalidateData,
    navigation,
    t,
    sectionsEnabled,
    targetSectionId,
  ]);

  // A task move/copy isn't actionable until its row has loaded.
  const blocked = submitting || (params.kind === 'task' && task == null);

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      <Text style={styles.heading} accessibilityRole="header">
        {t(
          params.kind === 'event'
            ? 'dialogs.moveCopy.titleEvent'
            : 'dialogs.moveCopy.titleTask',
          { title: itemTitle },
        )}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      <SegmentedSelect<MoveCopyMode>
        label={t('dialogs.moveCopy.modeLabel')}
        value={mode}
        options={modeOptions}
        onChange={setMode}
      />

      {isRecurringOccurrence && (
        <SegmentedSelect<MoveCopyScope>
          label={t('dialogs.moveCopy.scopeLabel')}
          value={scope}
          options={scopeOptions}
          onChange={setScope}
        />
      )}

      {containerOptions.length > 0 ? (
        <SelectFieldButton<string>
          label={t(
            params.kind === 'event'
              ? 'dialogs.moveCopy.targetCalendar'
              : 'dialogs.moveCopy.targetList',
          )}
          value={targetContainerId}
          options={containerOptions}
          onChange={setTargetContainerId}
        />
      ) : (
        <Text style={styles.hint} accessibilityRole="text">
          {t(
            params.kind === 'event'
              ? 'dialogs.moveCopy.targetCalendar'
              : 'dialogs.moveCopy.targetList',
          )}
        </Text>
      )}

      {sectionsEnabled && (
        <SelectFieldButton<string>
          label={t('dialogs.task.fields.section')}
          value={targetSectionId}
          options={sectionOptions}
          onChange={setTargetSectionId}
        />
      )}

      {/* Naming a section here rather than in a detour through the editor:
          filing something into a section that does not exist yet is one
          thought. */}
      {sectionsManageable && sectionDraft === null && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.task.section.add')}
          onPress={() => setSectionDraft('')}
          style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
        >
          <Text style={styles.buttonText}>{t('dialogs.task.section.add')}</Text>
        </Pressable>
      )}
      {sectionsManageable && sectionDraft !== null && (
        <View style={styles.field}>
          <Text style={styles.legend}>{t('dialogs.task.section.nameField')}</Text>
          <TextInput
            style={styles.input}
            value={sectionDraft}
            autoFocus
            accessibilityLabel={t('dialogs.task.section.nameField')}
            placeholder={t('dialogs.task.section.namePlaceholder')}
            returnKeyType="done"
            onChangeText={setSectionDraft}
            onSubmitEditing={() => void createTargetSection()}
          />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.task.section.create')}
            onPress={() => void createTargetSection()}
            style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
          >
            <Text style={styles.buttonText}>
              {t('dialogs.task.section.create')}
            </Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.task.section.cancel')}
            onPress={() => setSectionDraft(null)}
            style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
          >
            <Text style={styles.buttonText}>
              {t('dialogs.task.section.cancel')}
            </Text>
          </Pressable>
        </View>
      )}

      {params.kind === 'task' && children.length > 0 && (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.moveCopy.subtasksIncluded', { count: children.length })}
        </Text>
      )}

      <View style={styles.buttons}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={
            mode === 'move'
              ? t('dialogs.moveCopy.submitMove')
              : t('dialogs.moveCopy.submitCopy')
          }
          accessibilityState={{ disabled: blocked }}
          disabled={blocked}
          onPress={() => void submit()}
          style={({ pressed }) => [
            styles.button,
            pressed && styles.buttonPressed,
            blocked && styles.buttonDisabled,
          ]}
        >
          <Text style={styles.buttonText}>
            {mode === 'move'
              ? t('dialogs.moveCopy.submitMove')
              : t('dialogs.moveCopy.submitCopy')}
          </Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.cancel')}
          onPress={() => navigation.goBack()}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
        >
          <Text style={styles.ghostButtonText}>{t('dialogs.cancel')}</Text>
        </Pressable>
      </View>
    </FormScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 20, gap: 18 },
    heading: { fontSize: 20, fontWeight: '700', color: c.textPrimary },
    hint: { fontSize: 14, color: c.textSecondary },
    // The inline "new section" box, borrowed in shape from the editors' fields.
    field: { gap: 8 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textPrimary },
    input: {
      borderWidth: 1,
      borderColor: c.border,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 16,
      color: c.textPrimary,
      backgroundColor: c.surface,
    },
    buttons: { flexDirection: 'row', gap: 10, marginTop: 8 },
    button: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    buttonPressed: { backgroundColor: c.accentPressed },
    buttonDisabled: { opacity: 0.5 },
    buttonText: { fontSize: 17, fontWeight: '700', color: c.textOnAccent },
    ghostButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostPressed: { backgroundColor: c.surfacePressed },
    ghostButtonText: { fontSize: 17, fontWeight: '600', color: c.link },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
