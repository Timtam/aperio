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

import {
  dateInput,
  defaultNewEventTimes,
  selectableEventCalendars,
  timeInput,
  toIso,
} from '@aperio/shared';

import { createEvent, listCalendars, type Calendar } from '../api/calendar';
import { DateTimeFieldButton } from '../components/DateTimeFieldButton';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { useCancelHeader } from '../components/useCancelHeader';
import { useShowHiddenCalendarTargets } from '../settings/hiddenTargets';
import { useCalendarVisibility } from '../state/calendarVisibility';
import {
  readLastUsedCalendar,
  writeLastUsedCalendar,
} from '../state/lastUsedCalendar';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// One-tap EVENT capture — the RN twin of the desktop QuickAddDialog. Minimal
// form (title + date + time + calendar); the event runs one hour from the
// chosen start. "More details …" hands the in-progress title/day/calendar to
// the full EventEditor. Mobile had no event quick-add before (events always
// went straight to the full editor); this brings event parity with the task
// quick-add and the day-activation create flow.

export default function QuickAddEventModal({
  navigation,
  route,
}: RootStackScreenProps<'QuickAddEvent'>) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { hidden } = useCalendarVisibility();
  const includeHidden = useShowHiddenCalendarTargets();

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [title, setTitle] = useState('');
  // Default slot, same policy as the desktop quick-add + the full editor
  // (shared defaultNewEventTimes): a tapped calendar day seeds the start day,
  // and the time is the next :00/:30 slot on today / 09:00 on another day —
  // not the current minute, which put every quick-added event in the past.
  const [initialSlot] = useState(() =>
    defaultNewEventTimes(route.params.anchor, new Date()),
  );
  const [date, setDate] = useState(() => dateInput(initialSlot.start));
  const [time, setTime] = useState(() => timeInput(initialSlot.start));
  const [calId, setCalId] = useState(route.params.calendarId);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const titleRef = useRef<TextInput | null>(null);
  // Live mirrors for the mount-only default-calendar resolution below — refs so
  // the effect reads the CURRENT visibility rules without re-running (which
  // would re-adopt the stored calendar over a pick the user already made).
  const hiddenRef = useRef(hidden);
  hiddenRef.current = hidden;
  const includeHiddenRef = useRef(includeHidden);
  includeHiddenRef.current = includeHidden;
  const calIdTouchedRef = useRef(false);

  // Cancel button in the header (first element) so the user can back out fast.
  useCancelHeader(navigation);

  // Load the calendars for the picker (best-effort; the route already passed a
  // sensible default calId so the form works before this resolves). Once the
  // list is in, prefer the calendar the user last created on — the surfaces
  // seed us with the FIRST writable one, which lands every event in the same
  // calendar for anyone with more than two. Desktop does this in the event
  // form's default chain; here it's the one create entry point, so it lives
  // here. Only while the picker is still untouched (the read is async).
  useEffect(() => {
    void (async () => {
      const [cals, lastUsed] = await Promise.all([
        listCalendars().catch(() => [] as Calendar[]),
        readLastUsedCalendar(),
      ]);
      setCalendars(cals);
      // Only while the picker is still untouched — the read is async, so the
      // user may already have chosen by the time it lands.
      if (calIdTouchedRef.current || lastUsed == null) return;
      const usable = cals.find(
        (c) =>
          c.id === lastUsed &&
          !c.read_only &&
          (includeHiddenRef.current || !hiddenRef.current.has(c.id)),
      );
      if (usable) setCalId(usable.id);
    })();
  }, []);

  // Drive SR focus + the keyboard into the title field on open, so a new event
  // is ready to type immediately (a modal must drive focus or VoiceOver lingers
  // on the trigger row).
  useEffect(() => {
    const tag = findNodeHandle(titleRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    titleRef.current?.focus();
  }, []);

  // Offer only writable, non-hidden calendars (plus the pre-selected one so it
  // always shows). Mirrors the full editor's calendar picker.
  const calendarOptions = useMemo(
    () =>
      selectableEventCalendars(calendars, {
        selectedIds: new Set(
          calendars.filter((c) => !hidden.has(c.id)).map((c) => c.id),
        ),
        currentId: calId,
        includeHidden,
      }).map((c) => ({ value: c.id, label: c.name })),
    [calendars, hidden, calId, includeHidden],
  );

  const fail = useCallback((message: string) => {
    setError(message);
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  const onCreate = useCallback(async () => {
    const trimmed = title.trim();
    if (!trimmed) {
      fail(t('dialogs.event.titleRequired'));
      return;
    }
    if (!calId) {
      fail(t('dialogs.event.calendarRequired'));
      return;
    }
    const start = toIso(date, time, false);
    if (!start) {
      fail(t('mobile.invalidDateTime'));
      return;
    }
    const end = new Date(
      new Date(start).getTime() + 60 * 60 * 1000,
    ).toISOString();
    setError(null);
    setSubmitting(true);
    try {
      await createEvent({
        calendar_id: calId,
        title: trimmed,
        description: null,
        location: null,
        start,
        end,
        all_day: false,
        recurrence: null,
        color_label: null,
        reminders: [],
        sound: null,
        attendees: [],
        send_invitations: false,
      });
      // Remember the calendar for the next new-event open (see the editor).
      void writeLastUsedCalendar(calId);
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.event.created', { title: trimmed }),
      );
      navigation.goBack();
    } catch (err) {
      fail(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }, [calId, date, fail, navigation, t, time, title]);

  // Hand off to the full editor, carrying the title/day/TIME/calendar. Replace
  // (not push) so the quick-add doesn't linger behind the editor. The picked
  // time rides along so the editor keeps it instead of re-deriving its own
  // default slot (the desktop QuickAddDialog does the same via defaultTime).
  const openFullEditor = useCallback(() => {
    navigation.replace('EventEditor', {
      eventId: null,
      calendarId: calId,
      anchor: date.trim() || undefined,
      initialTitle: title.trim() || undefined,
      initialTime: time.trim() || undefined,
    });
  }, [calId, date, navigation, time, title]);

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      {error != null && (
        <Text
          style={styles.error}
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
        >
          {error}
        </Text>
      )}

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.event.fields.title')}</Text>
        <TextInput
          ref={titleRef}
          style={styles.input}
          value={title}
          onChangeText={setTitle}
          placeholder={t('dialogs.event.fields.title')}
          accessibilityLabel={t('dialogs.event.fields.title')}
          returnKeyType="done"
          onSubmitEditing={() => void onCreate()}
          autoComplete="off"
        />
      </View>

      {/* Date/time as accessible field buttons (value in the label, picker in
          a dialog) — the inline compact picker never joined the VoiceOver
          swipe order; the visible label is SR-hidden (folded into the button). */}
      <View style={styles.field}>
        <Text
          style={styles.legend}
          accessibilityElementsHidden
          importantForAccessibility="no"
        >
          {t('dialogs.event.fields.startDate')}
        </Text>
        <DateTimeFieldButton
          label={t('dialogs.event.fields.startDate')}
          mode="date"
          value={date}
          onChange={setDate}
        />
      </View>

      <View style={styles.field}>
        <Text
          style={styles.legend}
          accessibilityElementsHidden
          importantForAccessibility="no"
        >
          {t('dialogs.event.fields.startTime')}
        </Text>
        <DateTimeFieldButton
          label={t('dialogs.event.fields.startTime')}
          mode="time"
          value={time}
          onChange={setTime}
        />
      </View>

      {calendarOptions.length > 0 ? (
        <RadioGroup<string>
          label={t('dialogs.event.fields.calendar')}
          value={calId}
          options={calendarOptions}
          onChange={(next) => {
            // Their pick wins over a late-landing last-used adoption.
            calIdTouchedRef.current = true;
            setCalId(next);
          }}
        />
      ) : (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.event.pickCalendar')}
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
          style={({ pressed }) => [
            styles.ghostButton,
            pressed && styles.ghostPressed,
          ]}
        >
          <Text style={styles.ghostButtonText}>
            {t('dialogs.quickAdd.moreDetails')}
          </Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.cancel')}
          onPress={() => navigation.goBack()}
          style={({ pressed }) => [
            styles.ghostButton,
            pressed && styles.ghostPressed,
          ]}
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
