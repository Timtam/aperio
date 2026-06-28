import type { NativeStackNavigationProp } from '@react-navigation/native-stack';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text } from 'react-native';

import { localDateKey } from '@aperio/shared';

import { listCalendars, type Calendar } from '../api/calendar';
import { useCalendarVisibility } from '../state/calendarVisibility';
import type { RootStackParamList } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// The shared calendar action bar — Calendars (toggle which calendars show),
// Search, and New Event. These three are cross-cutting (they apply to every
// calendar view), but historically only the Day view (EventsScreen) rendered
// them, so on Week/Month/Agenda/Year a sighted user saw nothing where they
// should be. This component renders them visibly AND accessibly for any screen.
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
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { hidden } = useCalendarVisibility();
  const [calendars, setCalendars] = useState<Calendar[]>([]);

  // Refresh the calendar list on focus (calendars can be added/removed on the
  // Calendars screen and on other devices via sync).
  useEffect(() => {
    const read = () =>
      void listCalendars()
        .then(setCalendars)
        .catch(() => setCalendars([]));
    const unsubscribe = navigation.addListener('focus', read);
    read();
    return unsubscribe;
  }, [navigation]);

  // Seed "New Event" on a writable calendar the user hasn't hidden, so a new
  // event never defaults to a read-only or hidden one; fall back to any writable
  // calendar (the picker still lets the user change it).
  const firstCalendarId = useMemo(
    () =>
      calendars.find((c) => !c.read_only && !hidden.has(c.id))?.id ??
      calendars.find((c) => !c.read_only)?.id ??
      null,
    [calendars, hidden],
  );

  const addEvent = useCallback(() => {
    if (firstCalendarId == null) return;
    // → the event quick-add (expands to the full editor via "More details …").
    navigation.navigate('QuickAddEvent', {
      calendarId: firstCalendarId,
      anchor: localDateKey(anchorDay),
    });
  }, [firstCalendarId, navigation, anchorDay]);

  // A Fragment (not a wrapping View) so the buttons flow with the host
  // screen's action bar (which already lays out + wraps Today / Jump-to-date).
  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('mobile.calendarsButtonLabel')}
        onPress={() => navigation.navigate('Calendars')}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>{t('mobile.calendarsButtonLabel')}</Text>
      </Pressable>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.search.title')}
        onPress={() => navigation.navigate('Search')}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>{t('toolbar.search')}</Text>
      </Pressable>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: firstCalendarId == null }}
        accessibilityLabel={t('toolbar.newEvent')}
        disabled={firstCalendarId == null}
        onPress={addEvent}
        style={({ pressed }) => [
          styles.primaryButton,
          pressed && styles.primaryPressed,
          firstCalendarId == null && styles.primaryDisabled,
        ]}
      >
        <Text style={styles.primaryButtonText}>{t('toolbar.newEvent')}</Text>
      </Pressable>
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    primaryButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryPressed: { backgroundColor: c.accentPressed },
    primaryDisabled: { backgroundColor: c.accentDisabled },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    pressed: { opacity: 0.7 },
  });
