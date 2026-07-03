import type { NativeStackNavigationProp } from '@react-navigation/native-stack';
import { useCallback, useEffect, useMemo, useState } from 'react';

import { localDateKey } from '@aperio/shared';

import { listCalendars, type Calendar } from '../api/calendar';
import { useCalendarVisibility } from '../state/calendarVisibility';
import type { RootStackParamList } from '../navigation/types';

// "New event on this day" — the create flow shared by the CalendarActions
// toolbar button and the calendar screens' VoiceOver MAGIC TAP (two-finger
// double-tap on the screen root). Extracted from CalendarActions so both
// callers seed the SAME target: the event quick-add, anchored on the view's
// focused day, on the first writable calendar the user hasn't hidden (falling
// back to any writable one — the picker still lets them change it). Lives in
// its own file (not CalendarActions.tsx) per the react-refresh
// only-export-components rule.

export function useNewEventOnDay(
  navigation: NativeStackNavigationProp<RootStackParamList>,
  anchorDay: Date,
): { addEvent: () => void; enabled: boolean } {
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

  return { addEvent, enabled: firstCalendarId != null };
}
