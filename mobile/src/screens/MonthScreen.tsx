import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { CalendarDayList } from '../components/CalendarDayList';
import { CalendarViewSwitcher } from '../components/CalendarViewSwitcher';
import { CALENDAR_VIEW_ROUTE } from '../components/calendarViews';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// Accessible Month view — the screen-reader-first port of the desktop MonthView.
// The desktop's 6-week grid is a visual layout; the faithful SR equivalent is
// the shared linear CalendarDayList scoped to the calendar month (the 1st to the
// last, no adjacent-month padding — padding only serves a visual grid). This
// screen owns the month chrome: the Day/Week/Month/Agenda switcher, the month
// header + prev/next-month nav, and today / jump-to-date. (Unlike Week, the
// month-days list needs no week-start — there's no week grouping or padding.)

/** Local-midnight clone of `date`. */
function localMidnight(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

export default function MonthScreen({ navigation, route }: RootStackScreenProps<'Month'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [anchor, setAnchor] = useState(() => {
    const seed = route.params?.anchor ? new Date(route.params.anchor) : new Date();
    return localMidnight(Number.isNaN(seed.getTime()) ? new Date() : seed);
  });
  const [jumpText, setJumpText] = useState('');
  const [jumpError, setJumpError] = useState<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  // Every day of the anchor's calendar month (1st → last) + the covering range.
  const { days, range } = useMemo(() => {
    const y = anchor.getFullYear();
    const m = anchor.getMonth();
    const daysInMonth = new Date(y, m + 1, 0).getDate();
    const ds = Array.from({ length: daysInMonth }, (_, i) => new Date(y, m, i + 1));
    const end = new Date(y, m, daysInMonth, 23, 59, 59, 999);
    return { days: ds, range: { start: new Date(y, m, 1), end } };
  }, [anchor]);

  const monthLabel = useMemo(
    () => anchor.toLocaleDateString(i18n.language, { month: 'long', year: 'numeric' }),
    [anchor, i18n.language],
  );

  const goToday = useCallback(() => setAnchor(localMidnight(new Date())), []);

  const stepMonth = useCallback(
    (delta: number) =>
      setAnchor((a) => new Date(a.getFullYear(), a.getMonth() + delta, 1)),
    [],
  );

  const jumpToDate = useCallback(() => {
    const raw = jumpText.trim();
    if (raw === '') return;
    const parsed = new Date(`${raw}T00:00`);
    if (Number.isNaN(parsed.getTime())) {
      setJumpError(t('dialogs.event.dateInvalid'));
      announce(t('dialogs.event.dateInvalid'));
      return;
    }
    setJumpError(null);
    setJumpText('');
    setAnchor(localMidnight(parsed));
  }, [announce, jumpText, t]);

  return (
    <View style={styles.screen}>
      <CalendarViewSwitcher
        active="month"
        onSelect={(v) =>
          navigation.replace(CALENDAR_VIEW_ROUTE[v], { anchor: anchor.toISOString() })
        }
      />

      <View style={styles.navBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.prev')}
          onPress={() => stepMonth(-1)}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">‹</Text>
        </Pressable>
        <Text style={styles.rangeHeading} accessibilityRole="header">
          {monthLabel}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.next')}
          onPress={() => stepMonth(1)}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">›</Text>
        </Pressable>
      </View>

      <View style={styles.actionBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.today')}
          onPress={goToday}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.today')}</Text>
        </Pressable>
        <TextInput
          style={styles.jumpInput}
          value={jumpText}
          onChangeText={setJumpText}
          placeholder="YYYY-MM-DD"
          accessibilityLabel={t('mobile.jumpToDate')}
          autoCapitalize="none"
          autoCorrect={false}
          returnKeyType="go"
          onSubmitEditing={jumpToDate}
        />
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.jumpToDateAction')}
          onPress={jumpToDate}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.jumpToDateAction')}</Text>
        </Pressable>
      </View>

      {jumpError != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {jumpError}
        </Text>
      )}

      <CalendarDayList
        navigation={navigation}
        days={days}
        range={range}
        gridLabel={t('views.month.gridLabel')}
        emptyText={t('views.month.empty')}
        dayAnnounceKey="views.month.dayAnnounce"
      />
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    navBar: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingHorizontal: 12,
      paddingTop: 12,
    },
    rangeHeading: {
      flex: 1,
      fontSize: 16,
      fontWeight: '700',
      color: c.textPrimary,
      textAlign: 'center',
    },
    navButton: {
      width: 48,
      height: 48,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      alignItems: 'center',
      justifyContent: 'center',
      backgroundColor: c.surfaceAlt,
    },
    navButtonText: { fontSize: 26, color: c.textPrimary, lineHeight: 30 },
    actionBar: { flexDirection: 'row', gap: 10, padding: 12, alignItems: 'center' },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    jumpInput: {
      flex: 1,
      fontSize: 16,
      color: c.textPrimary,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
    pressed: { opacity: 0.7 },
  });
