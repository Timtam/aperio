import type { NativeStackNavigationProp } from '@react-navigation/native-stack';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text } from 'react-native';

import { localDateKey } from '@aperio/shared';
import type { RootStackParamList } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';
import { chrome } from '../theme/uiScale';
import { DayLogDialog } from './DayLogDialog';
import { useNewEventOnDay } from './useNewEventOnDay';
import { useNewTaskOnDay } from './useNewTaskOnDay';

// The shared calendar action bar — Calendars (toggle which calendars show),
// Search, New Event, New Task, and the day's Check-in. These are cross-cutting
// (they apply to every calendar view), but historically only the Day view
// (EventsScreen) rendered them, so on Week/Month/Agenda/Year a sighted user saw
// nothing where they should be. This component renders them visibly AND
// accessibly for any screen.
//
// Check-in rides here rather than on each day row: a button per day would cost
// a swipe per day to get past, and the day-header actions it also lives in are
// skipped entirely by the single-day screen. One button, on the day the view is
// focused on, is reachable from every view including that one.
//
// It self-loads the first calendar id so a screen needn't wire the calendar
// list just to enable "New Event"; the button seeds the editor on `anchorDay`
// (the day the view is focused on) and is disabled (greyed + announced) until a
// calendar exists.

interface Props {
  navigation: NativeStackNavigationProp<RootStackParamList>;
  /** The day a new event seeds on — the view's currently-focused day. */
  anchorDay: Date;
}

export function CalendarActions({ navigation, anchorDay }: Props) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  // The shared "new event on this day" flow — the same hook feeds the host
  // screens' VoiceOver magic tap, so both entry points seed identically.
  const { addEvent, enabled } = useNewEventOnDay(navigation, anchorDay);
  // Its task twin. This button is what makes task creation DISCOVERABLE from
  // the calendar for a sighted user: the day headers' create actions are
  // rotor/long-press only, and the desktop toolbar has always carried both.
  const { addTask, enabled: taskEnabled } = useNewTaskOnDay(navigation, anchorDay);
  const [checkInOpen, setCheckInOpen] = useState(false);
  const dayKey = localDateKey(anchorDay);
  const dayLabel = anchorDay.toLocaleDateString(i18n.language, {
    weekday: 'long',
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  });

  // A Fragment (not a wrapping View) so the buttons flow with the host
  // screen's action bar (which already lays out + wraps Today / Jump-to-date).
  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('mobile.calendarsButtonLabel')}
        onPress={() => navigation.navigate('Calendars')}
        hitSlop={8}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>{t('mobile.calendarsButtonLabel')}</Text>
      </Pressable>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.search.title')}
        onPress={() => navigation.navigate('Search')}
        hitSlop={8}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>{t('toolbar.search')}</Text>
      </Pressable>
      <Pressable
        accessibilityRole="button"
        // The date is in the NAME, not in the visible text: which day this
        // acts on is obvious to somebody who can see the focused cell and
        // invisible to somebody who cannot.
        accessibilityLabel={t('dialogs.dayLog.openButtonOnDay', { day: dayLabel })}
        onPress={() => setCheckInOpen(true)}
        hitSlop={8}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>{t('dialogs.dayLog.openButton')}</Text>
      </Pressable>
      <DayLogDialog
        visible={checkInOpen}
        onClose={() => setCheckInOpen(false)}
        day={dayKey}
        dayLabel={dayLabel}
      />
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: !enabled }}
        accessibilityLabel={t('toolbar.newEvent')}
        disabled={!enabled}
        onPress={addEvent}
        hitSlop={8}
        style={({ pressed }) => [
          styles.primaryButton,
          pressed && styles.primaryPressed,
          !enabled && styles.primaryDisabled,
        ]}
      >
        <Text style={styles.primaryButtonText}>{t('toolbar.newEvent')}</Text>
      </Pressable>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: !taskEnabled }}
        accessibilityLabel={t('toolbar.newTask')}
        disabled={!taskEnabled}
        onPress={addTask}
        hitSlop={8}
        style={({ pressed }) => [
          styles.primaryButton,
          pressed && styles.primaryPressed,
          !taskEnabled && styles.primaryDisabled,
        ]}
      >
        <Text style={styles.primaryButtonText}>{t('toolbar.newTask')}</Text>
      </Pressable>
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    ghostButton: {
      paddingVertical: chrome(6),
      paddingHorizontal: chrome(12),
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    primaryButton: {
      paddingVertical: chrome(6),
      paddingHorizontal: chrome(12),
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryPressed: { backgroundColor: c.accentPressed },
    primaryDisabled: { backgroundColor: c.accentDisabled },
    primaryButtonText: { fontSize: 15, fontWeight: '700', color: c.textOnAccent },
    pressed: { opacity: 0.7 },
  });
