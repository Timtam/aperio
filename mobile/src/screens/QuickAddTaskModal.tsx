import { DateTimePicker } from '@expo/ui/community/datetime-picker';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { createTask } from '../api/client';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { useCancelHeader } from '../components/useCancelHeader';
import { formatLocalDate, parseLocalDate } from '../intl/dateTimeField';
import { readLastUsedTaskList, writeLastUsedTaskList } from '../state/lastUsedTaskList';
import { useTaskStore } from '../state/taskStoreContext';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// One-tap task capture — the RN twin of the desktop QuickAddTaskDialog. Minimal
// form (title + optional scheduled day + list); empty day ⇒ the task lands in
// the backlog. "More details …" hands the in-progress title/day/list to the full
// TaskEditor. The default list is the last-used one (if still present + writable)
// so a multi-list setup doesn't reset to the first list each time.

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;

/** True when a non-empty date is malformed in shape OR calendar value. Empty is
 *  valid (no day ⇒ backlog). Mirrors the TaskEditor's date guard so the user
 *  gets a localized message instead of a raw bridge error. */
function dateInvalid(date: string): boolean {
  const d = date.trim();
  if (!d) return false;
  if (!DATE_RE.test(d)) return true;
  const [y, m, day] = d.split('-').map(Number);
  const probe = new Date(y, m - 1, day);
  return probe.getFullYear() !== y || probe.getMonth() !== m - 1 || probe.getDate() !== day;
}

export default function QuickAddTaskModal({
  navigation,
  route,
}: RootStackScreenProps<'QuickAdd'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { taskLists, invalidateData } = useTaskStore();

  const writable = useMemo(() => taskLists.filter((l) => !l.read_only), [taskLists]);

  const [title, setTitle] = useState('');
  // A tapped calendar day pre-schedules the task; otherwise dateless (backlog).
  const [date, setDate] = useState(route.params?.initialScheduledDate ?? '');
  const [listId, setListId] = useState<string>(
    () => writable[0]?.id ?? taskLists[0]?.id ?? '',
  );
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const titleRef = useRef<TextInput | null>(null);
  // Don't let the async last-used read clobber a list the user already picked.
  const userPicked = useRef(false);

  // Default to the last-used list (if still present + writable). Async read, so
  // it lands a tick after first paint — guarded against a manual pick.
  useEffect(() => {
    let cancelled = false;
    void readLastUsedTaskList().then((id) => {
      if (cancelled || userPicked.current) return;
      if (id && writable.some((l) => l.id === id)) setListId(id);
    });
    return () => {
      cancelled = true;
    };
  }, [writable]);

  // Cancel button in the header (first element) so the user can back out fast.
  useCancelHeader(navigation);

  // Move SR focus into the title field on open AND open the keyboard, so a new
  // task is ready to type immediately (a modal must drive focus or VoiceOver
  // lingers on the trigger row).
  useEffect(() => {
    const tag = findNodeHandle(titleRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    titleRef.current?.focus();
  }, []);

  // Any dismissal refetches the list (the desktop DialogState.close behaviour).
  useEffect(() => () => invalidateData(), [invalidateData]);

  const pickList = useCallback((id: string) => {
    userPicked.current = true;
    setListId(id);
  }, []);

  const listOptions = useMemo(
    () => writable.map((l) => ({ value: l.id, label: l.name })),
    [writable],
  );

  const fail = useCallback((message: string) => {
    setError(message);
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  const onCreate = useCallback(async () => {
    const trimmed = title.trim();
    if (!trimmed) {
      fail(t('dialogs.task.titleRequired'));
      return;
    }
    if (!listId) {
      fail(t('dialogs.task.listRequired'));
      return;
    }
    if (dateInvalid(date)) {
      fail(t('mobile.invalidDateTime'));
      return;
    }
    setError(null);
    setSubmitting(true);
    try {
      await createTask({
        list_id: listId,
        title: trimmed,
        description: null,
        status: 'open',
        priority: 'medium',
        effort: 'medium',
        // Empty day → backlog; a chosen day schedules it.
        scheduled_date: date.trim() || null,
        scheduled_time: null,
        deadline_date: null,
        deadline_time: null,
        // No per-task countdown override on quick-add → use the global.
        deadline_reminder_days: null,
        recurrence: null,
        parent_id: null,
        section_id: null,
        color_label: null,
        reminders: [],
        assignees: [],
        sound: null,
      });
      await writeLastUsedTaskList(listId);
      AccessibilityInfo.announceForAccessibility(t('dialogs.task.created', { title: trimmed }));
      navigation.goBack();
    } catch (err) {
      fail(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }, [date, fail, listId, navigation, t, title]);

  // Hand off to the full editor, carrying the typed title/day + the picked list.
  // Replace (not push) so the quick-add doesn't linger behind the editor.
  const openFullEditor = useCallback(() => {
    navigation.replace('TaskEditor', {
      taskId: null,
      listId,
      initialTitle: title.trim() || undefined,
      initialScheduledDate: date.trim() || undefined,
    });
  }, [date, listId, navigation, title]);

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.task.fields.title')}</Text>
        <TextInput
          ref={titleRef}
          style={styles.input}
          value={title}
          onChangeText={setTitle}
          placeholder={t('mobile.newTaskPlaceholder')}
          accessibilityLabel={t('dialogs.task.fields.title')}
          returnKeyType="done"
          onSubmitEditing={() => void onCreate()}
          autoComplete="off"
        />
      </View>

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.task.fields.scheduled.legend')}</Text>
        {date.trim() === '' ? (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.task.fields.scheduled.addDate')}
            onPress={() => setDate(formatLocalDate(new Date()))}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.task.fields.scheduled.addDate')}
            </Text>
          </Pressable>
        ) : (
          <View style={styles.pickerRow}>
            <DateTimePicker
              mode="date"
              display="compact"
              value={parseLocalDate(date)}
              onValueChange={(_, d) => setDate(formatLocalDate(d))}
              locale={i18n.language}
            />
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={t('dialogs.task.fields.scheduled.clear')}
              onPress={() => setDate('')}
              style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
            >
              <Text style={styles.ghostButtonText}>
                {t('dialogs.task.fields.scheduled.clear')}
              </Text>
            </Pressable>
          </View>
        )}
      </View>

      {listOptions.length > 0 ? (
        <RadioGroup<string>
          label={t('dialogs.task.fields.list')}
          value={listId}
          options={listOptions}
          onChange={pickList}
        />
      ) : (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.task.pickList')}
        </Text>
      )}

      <View style={styles.buttons}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.create')}
          accessibilityState={{ disabled: submitting }}
          disabled={submitting}
          onPress={() => void onCreate()}
          style={({ pressed }) => [
            styles.button,
            pressed && styles.buttonPressed,
            submitting && styles.buttonDisabled,
          ]}
        >
          <Text style={styles.buttonText}>{t('dialogs.create')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.quickAdd.moreDetails')}
          onPress={openFullEditor}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
        >
          <Text style={styles.ghostButtonText}>{t('dialogs.quickAdd.moreDetails')}</Text>
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
    field: { gap: 6 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    pickerRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      paddingVertical: 4,
      flexWrap: 'wrap',
    },
    hint: { fontSize: 13, color: c.textSecondary },
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
    buttons: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, marginTop: 8 },
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
