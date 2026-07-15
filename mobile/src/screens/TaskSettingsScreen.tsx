import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, ScrollView, StyleSheet, Switch, Text, View } from 'react-native';

import { RadioGroup } from '../components/RadioGroup';
import {
  readShowHiddenTaskListTargets,
  writeShowHiddenTaskListTargets,
} from '../settings/hiddenTargets';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';
import {
  readTaskBehaviour,
  writeAutoDate,
  writeAutoSelfAssign,
  writeCarryOverDefault,
  writeCascadeEnabled,
  writeCheckoffMode,
  writeDayStartTrigger,
  writeDeadlineCountdownDays,
  writeRemindDeadlineArrived,
  writeRemindDeadlineCountdown,
  writeRemindUntimedToday,
  writeVisualEffortSizing,
  type CarryOverDefault,
  type CheckoffMode,
  type DayStartTrigger,
} from '../state/taskBehaviour';

// Tasks settings — the global task-behaviour knobs (the mobile twin of the
// desktop TasksPanel's global section). Three controls: check-off mode (a radio
// group), status coupling, and auto-date (accessible switches). Each carries the
// desktop's hint text. Per-list overrides, carry-over, and the day-start trigger
// are deferred (the latter two need the day-start review checkers mobile lacks).
// Screen-reader-first: the radio group is one focus stop per option; each switch
// row is a single switch node (role + checked + label on the Pressable, the
// visual Switch hidden + non-interactive), with its hint as the next element.

/** Sensible "X days before a deadline" presets (the pref clamps to 1..30). A
 *  RadioGroup of numbers is the SR-faithful single-select on RN (no <select>). */
const DEADLINE_COUNTDOWN_DAYS_OPTIONS: readonly number[] = [1, 2, 3, 5, 7, 14];

/** One accessible switch row: the Pressable owns role/checked/label/tap; the
 *  inner Switch is the visual indicator only (hidden from SR, tap routed to the
 *  Pressable). Matches the EventEditorModal all-day-switch pattern. */
function SwitchRow({
  label,
  value,
  onToggle,
}: {
  label: string;
  value: boolean;
  onToggle: () => void;
}) {
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityState={{ checked: value }}
      accessibilityLabel={label}
      onPress={onToggle}
      style={({ pressed }) => [styles.switchRow, pressed && styles.pressed]}
    >
      <Text style={styles.switchLabel} importantForAccessibility="no">
        {label}
      </Text>
      <View pointerEvents="none">
        <Switch
          value={value}
          trackColor={{ false: colors.border, true: colors.accent }}
          importantForAccessibility="no"
          accessibilityElementsHidden
        />
      </View>
    </Pressable>
  );
}

export default function TaskSettingsScreen() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [checkoffMode, setCheckoffMode] = useState<CheckoffMode>('toggle');
  const [cascade, setCascade] = useState(true);
  const [autoDate, setAutoDate] = useState(true);
  const [autoSelfAssign, setAutoSelfAssign] = useState(true);
  const [visualEffortSizing, setVisualEffortSizing] = useState(true);
  // Synced (default on): offer hidden task lists as targets in the pickers.
  const [showHiddenListTargets, setShowHiddenListTargets] = useState(true);
  const [remindUntimedToday, setRemindUntimedToday] = useState(true);
  const [remindDeadlineArrived, setRemindDeadlineArrived] = useState(true);
  const [remindDeadlineCountdown, setRemindDeadlineCountdown] = useState(true);
  const [deadlineCountdownDays, setDeadlineCountdownDays] = useState(3);
  const [carryOver, setCarryOver] = useState<CarryOverDefault>('ask');
  const [dayStart, setDayStart] = useState<DayStartTrigger>('00:00');

  // Reflect the stored knobs whenever focused (they may have been set on
  // another device, or just hydrated after this screen first mounted).
  useFocusEffect(
    useCallback(() => {
      void readTaskBehaviour().then((b) => {
        setCheckoffMode(b.checkoffMode);
        setCascade(b.cascadeEnabled);
        setAutoDate(b.autoDate);
        setAutoSelfAssign(b.autoSelfAssign);
        setVisualEffortSizing(b.visualEffortSizing);
        setRemindUntimedToday(b.remindUntimedToday);
        setRemindDeadlineArrived(b.remindDeadlineArrived);
        setRemindDeadlineCountdown(b.remindDeadlineCountdown);
        setDeadlineCountdownDays(b.deadlineCountdownDays);
        setCarryOver(b.carryOverDefault);
        setDayStart(b.dayStartTrigger);
      });
      void readShowHiddenTaskListTargets().then(setShowHiddenListTargets);
    }, []),
  );

  const onCheckoffChange = useCallback((next: CheckoffMode) => {
    setCheckoffMode(next);
    void writeCheckoffMode(next);
  }, []);

  const onShowHiddenListTargetsToggle = useCallback(() => {
    setShowHiddenListTargets((prev) => {
      const next = !prev;
      void writeShowHiddenTaskListTargets(next);
      return next;
    });
  }, []);

  const onCarryOverChange = useCallback((next: CarryOverDefault) => {
    setCarryOver(next);
    void writeCarryOverDefault(next);
  }, []);

  const onDayStartChange = useCallback((next: DayStartTrigger) => {
    setDayStart(next);
    void writeDayStartTrigger(next);
  }, []);

  const onCascadeToggle = useCallback(() => {
    setCascade((prev) => {
      const next = !prev;
      void writeCascadeEnabled(next);
      return next;
    });
  }, []);

  const onAutoDateToggle = useCallback(() => {
    setAutoDate((prev) => {
      const next = !prev;
      void writeAutoDate(next);
      return next;
    });
  }, []);

  const onAutoSelfAssignToggle = useCallback(() => {
    setAutoSelfAssign((prev) => {
      const next = !prev;
      void writeAutoSelfAssign(next);
      return next;
    });
  }, []);

  const onVisualEffortSizingToggle = useCallback(() => {
    setVisualEffortSizing((prev) => {
      const next = !prev;
      void writeVisualEffortSizing(next);
      return next;
    });
  }, []);

  const onRemindUntimedTodayToggle = useCallback(() => {
    setRemindUntimedToday((prev) => {
      const next = !prev;
      void writeRemindUntimedToday(next);
      return next;
    });
  }, []);

  const onRemindDeadlineArrivedToggle = useCallback(() => {
    setRemindDeadlineArrived((prev) => {
      const next = !prev;
      void writeRemindDeadlineArrived(next);
      return next;
    });
  }, []);

  const onRemindDeadlineCountdownToggle = useCallback(() => {
    setRemindDeadlineCountdown((prev) => {
      const next = !prev;
      void writeRemindDeadlineCountdown(next);
      return next;
    });
  }, []);

  const onCountdownDaysChange = useCallback((next: number) => {
    setDeadlineCountdownDays(next);
    void writeDeadlineCountdownDays(next);
  }, []);

  // Always keep the current (possibly synced-from-another-device) value
  // selectable even if it's not a preset; splice it in, keep sorted.
  const countdownDayOptions = useMemo(() => {
    const set = new Set<number>(DEADLINE_COUNTDOWN_DAYS_OPTIONS);
    set.add(deadlineCountdownDays);
    return [...set]
      .sort((a, b) => a - b)
      .map((days) => ({
        value: days,
        label: t('dialogs.tasks.reminders.countdownDays.option', {
          count: days,
        }),
      }));
  }, [deadlineCountdownDays, t]);

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      <View style={styles.section}>
        <RadioGroup<CheckoffMode>
          label={t('dialogs.tasks.checkoffMode.heading')}
          labelAsHeading
          value={checkoffMode}
          options={[
            { value: 'toggle', label: t('dialogs.tasks.checkoffMode.options.toggle') },
            { value: 'cycle', label: t('dialogs.tasks.checkoffMode.options.cycle') },
          ]}
          onChange={onCheckoffChange}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.checkoffMode.hint')}
        </Text>
      </View>

      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.tasks.statusCoupling.heading')}
        </Text>
        <SwitchRow
          label={t('dialogs.tasks.statusCoupling.label')}
          value={cascade}
          onToggle={onCascadeToggle}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.statusCoupling.hint')}
        </Text>
      </View>

      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.tasks.autoDate.heading')}
        </Text>
        <SwitchRow
          label={t('dialogs.tasks.autoDate.label')}
          value={autoDate}
          onToggle={onAutoDateToggle}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.autoDate.hint')}
        </Text>
      </View>

      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.tasks.autoSelfAssign.heading')}
        </Text>
        <SwitchRow
          label={t('dialogs.tasks.autoSelfAssign.label')}
          value={autoSelfAssign}
          onToggle={onAutoSelfAssignToggle}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.autoSelfAssign.hint')}
        </Text>
      </View>

      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.tasks.visualEffortSizing.heading')}
        </Text>
        <SwitchRow
          label={t('dialogs.tasks.visualEffortSizing.label')}
          value={visualEffortSizing}
          onToggle={onVisualEffortSizingToggle}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.visualEffortSizing.hint')}
        </Text>
      </View>

      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.tasks.showHiddenTaskListTargets.heading')}
        </Text>
        <SwitchRow
          label={t('dialogs.tasks.showHiddenTaskListTargets.label')}
          value={showHiddenListTargets}
          onToggle={onShowHiddenListTargetsToggle}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.showHiddenTaskListTargets.hint')}
        </Text>
      </View>

      {/* Day-start reminders. Three independent switches + the shared
          "X days before" lead time. Prefs only — the reminder LOGIC that
          consumes them lands in a later step. */}
      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.tasks.reminders.heading')}
        </Text>
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.reminders.hint')}
        </Text>

        <SwitchRow
          label={t('dialogs.tasks.reminders.untimedToday.label')}
          value={remindUntimedToday}
          onToggle={onRemindUntimedTodayToggle}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.reminders.untimedToday.hint')}
        </Text>

        <SwitchRow
          label={t('dialogs.tasks.reminders.deadlineArrived.label')}
          value={remindDeadlineArrived}
          onToggle={onRemindDeadlineArrivedToggle}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.reminders.deadlineArrived.hint')}
        </Text>

        <SwitchRow
          label={t('dialogs.tasks.reminders.deadlineCountdown.label')}
          value={remindDeadlineCountdown}
          onToggle={onRemindDeadlineCountdownToggle}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.reminders.deadlineCountdown.hint')}
        </Text>

        <RadioGroup<number>
          label={t('dialogs.tasks.reminders.countdownDays.label')}
          labelAsHeading
          value={deadlineCountdownDays}
          options={countdownDayOptions}
          onChange={onCountdownDaysChange}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.reminders.countdownDays.hint')}
        </Text>
      </View>

      <View style={styles.section}>
        <RadioGroup<CarryOverDefault>
          label={t('dialogs.tasks.carryOverDefault.heading')}
          labelAsHeading
          value={carryOver}
          options={[
            { value: 'ask', label: t('dialogs.tasks.carryOverDefault.options.ask') },
            { value: 'today', label: t('dialogs.tasks.carryOverDefault.options.today') },
            { value: 'backlog', label: t('dialogs.tasks.carryOverDefault.options.backlog') },
          ]}
          onChange={onCarryOverChange}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.carryOverDefault.hint')}
        </Text>
      </View>

      <View style={styles.section}>
        <RadioGroup<DayStartTrigger>
          label={t('dialogs.tasks.dayStartTrigger.heading')}
          labelAsHeading
          value={dayStart}
          options={[
            { value: '00:00', label: t('dialogs.tasks.dayStartTrigger.options.midnight') },
            { value: '06:00', label: t('dialogs.tasks.dayStartTrigger.options.morning0600') },
            { value: '08:00', label: t('dialogs.tasks.dayStartTrigger.options.morning0800') },
            { value: '12:00', label: t('dialogs.tasks.dayStartTrigger.options.morning1200') },
            { value: 'app-start', label: t('dialogs.tasks.dayStartTrigger.options.appStart') },
          ]}
          onChange={onDayStartChange}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.tasks.dayStartTrigger.hint')}
        </Text>
      </View>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 24 },
    section: { gap: 8 },
    heading: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 14, color: c.textSecondary, lineHeight: 20 },
    switchRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    switchLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
    pressed: { opacity: 0.7 },
  });
