import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { Task } from '@aperio/shared';
import { isoNextMonday, isoToday, isoTomorrow } from '@aperio/shared';

import { getTaskById, updateTask } from '../api/client';
import { useTaskStore } from '../state/taskStoreContext';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// Plan-from-backlog quick scheduler — the RN twin of the desktop PlanTaskDialog
// (DESIGN.md §9.3). Picks a scheduled day three ways: quick presets (Today /
// Tomorrow / Next Monday), a custom YYYY-MM-DD field, or "back to backlog"
// (clears scheduled+deadline and reopens a completed task). The full-overwrite
// update round-trips the task exactly as read so the store-managed
// series_id/resurface_date survive; the store then refetches via invalidateData.

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;

/** True when a date string is malformed in shape OR calendar value. Mirrors the
 *  QuickAdd/TaskEditor guard so the user gets a localized message, not a raw
 *  bridge error. */
function dateInvalid(date: string): boolean {
  const d = date.trim();
  if (!DATE_RE.test(d)) return true;
  const [y, m, day] = d.split('-').map(Number);
  const probe = new Date(y, m - 1, day);
  return probe.getFullYear() !== y || probe.getMonth() !== m - 1 || probe.getDate() !== day;
}

type Choice = { kind: 'date'; iso: string } | { kind: 'backlog' };

export default function PlanTaskModal({ route, navigation }: RootStackScreenProps<'PlanTask'>) {
  const { taskId } = route.params;
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { invalidateData } = useTaskStore();

  const [task, setTask] = useState<Task | null>(null);
  // Preload the custom field with the task's current scheduled date so
  // editing-after-planning is one keypress less work (desktop parity).
  const [customDate, setCustomDate] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const todayRef = useRef<View | null>(null);

  // Load the task fresh, then drive SR focus onto the Today preset — the most
  // frequently-used action (desktop auto-focuses it).
  useEffect(() => {
    let cancelled = false;
    void getTaskById(taskId).then((loaded) => {
      if (cancelled) return;
      if (!loaded) {
        setError(t('mobile.taskMissing'));
        AccessibilityInfo.announceForAccessibility(t('mobile.taskMissing'));
        return;
      }
      setTask(loaded);
      setCustomDate(loaded.scheduled_date ?? '');
      const tag = findNodeHandle(todayRef.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    });
    return () => {
      cancelled = true;
    };
  }, [taskId, t]);

  // Any dismissal refetches the tasks (the desktop DialogState.close behaviour).
  useEffect(() => () => invalidateData(), [invalidateData]);

  const fmtDate = useCallback(
    (iso: string): string => {
      const d = new Date(`${iso}T00:00:00`);
      return Number.isNaN(d.getTime())
        ? iso
        : d.toLocaleDateString(i18n.language, {
            weekday: 'long',
            year: 'numeric',
            month: 'long',
            day: 'numeric',
          });
    },
    [i18n.language],
  );

  const commit = useCallback(
    async (choice: Choice) => {
      if (!task) return;
      setSubmitting(true);
      setError(null);
      try {
        const isBacklog = choice.kind === 'backlog';
        // A backlogged completed task reopens; otherwise the status is untouched.
        const nextStatus =
          isBacklog && task.status === 'completed' ? 'open' : task.status;
        const updated: Task = {
          ...task,
          scheduled_date: choice.kind === 'date' ? choice.iso : null,
          // Picking a specific day drops the per-day time — the task moves as a
          // whole, so the previously planned minute doesn't transfer.
          scheduled_time: choice.kind === 'date' ? null : task.scheduled_time,
          // "Back to backlog" also clears the deadline so the task is truly
          // unscheduled (a lone "by" deadline would keep pulling it forward).
          deadline_date: isBacklog ? null : task.deadline_date,
          deadline_time: isBacklog ? null : task.deadline_time,
          status: nextStatus,
          // Keep the completed/completed_at invariant in lock-step (the mobile
          // editor's convention): a reopened task has no completion stamp.
          completed_at: nextStatus === 'completed' ? task.completed_at : null,
        };
        await updateTask(updated);
        AccessibilityInfo.announceForAccessibility(
          choice.kind === 'date'
            ? t('dialogs.plan.plannedAnnouncement', {
                title: task.title,
                date: fmtDate(choice.iso),
              })
            : t('dialogs.plan.backloggedAnnouncement', { title: task.title }),
        );
        navigation.goBack();
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setError(message);
        AccessibilityInfo.announceForAccessibility(t('mobile.error', { message }));
      } finally {
        setSubmitting(false);
      }
    },
    [task, t, fmtDate, navigation],
  );

  const applyCustom = useCallback(() => {
    if (dateInvalid(customDate)) {
      const message = t('dialogs.plan.customDateRequired');
      setError(message);
      AccessibilityInfo.announceForAccessibility(message);
      return;
    }
    void commit({ kind: 'date', iso: customDate.trim() });
  }, [commit, customDate, t]);

  const presets: { key: string; iso: string; label: string; ref?: typeof todayRef }[] = [
    { key: 'today', iso: isoToday(), label: t('dialogs.plan.today'), ref: todayRef },
    { key: 'tomorrow', iso: isoTomorrow(), label: t('dialogs.plan.tomorrow') },
    { key: 'nextWeek', iso: isoNextMonday(), label: t('dialogs.plan.nextWeek') },
  ];

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
      accessibilityViewIsModal
    >
      {task != null && (
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.plan.title', { title: task.title })}
        </Text>
      )}
      <Text style={styles.hint} accessibilityRole="text">
        {t('dialogs.plan.hint')}
      </Text>
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      <View style={styles.field}>
        <Text style={styles.legend} accessibilityRole="header">
          {t('dialogs.plan.quickPresets')}
        </Text>
        {presets.map((p) => (
          <Pressable
            key={p.key}
            ref={p.ref}
            accessibilityRole="button"
            accessibilityLabel={p.label}
            accessibilityState={{ disabled: submitting }}
            disabled={submitting}
            onPress={() => void commit({ kind: 'date', iso: p.iso })}
            style={({ pressed }) => [
              styles.button,
              pressed && styles.buttonPressed,
              submitting && styles.buttonDisabled,
            ]}
          >
            <Text style={styles.buttonText}>{p.label}</Text>
          </Pressable>
        ))}
      </View>

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.plan.customDate')}</Text>
        <TextInput
          style={styles.input}
          value={customDate}
          onChangeText={setCustomDate}
          placeholder="YYYY-MM-DD"
          accessibilityLabel={t('dialogs.plan.customDate')}
          autoCapitalize="none"
          autoCorrect={false}
          returnKeyType="go"
          onSubmitEditing={applyCustom}
        />
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.plan.applyCustom')}
          accessibilityState={{ disabled: submitting }}
          disabled={submitting}
          onPress={applyCustom}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
        >
          <Text style={styles.ghostButtonText}>{t('dialogs.plan.applyCustom')}</Text>
        </Pressable>
      </View>

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.plan.backToBacklog')}
        accessibilityState={{ disabled: submitting }}
        disabled={submitting}
        onPress={() => void commit({ kind: 'backlog' })}
        style={({ pressed }) => [styles.secondaryButton, pressed && styles.ghostPressed]}
      >
        <Text style={styles.secondaryButtonText}>{t('dialogs.plan.backToBacklog')}</Text>
      </Pressable>

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.plan.cancel')}
        onPress={() => navigation.goBack()}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
      >
        <Text style={styles.ghostButtonText}>{t('dialogs.plan.cancel')}</Text>
      </Pressable>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 20, gap: 16 },
    heading: { fontSize: 20, fontWeight: '700', color: c.textPrimary },
    hint: { fontSize: 14, color: c.textSecondary },
    field: { gap: 10 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
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
    secondaryButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
      alignItems: 'center',
    },
    secondaryButtonText: { fontSize: 16, fontWeight: '600', color: c.textPrimary },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
