import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import type { ColorLabel, DayOccurrence, MultiDayInfo } from '@aperio/shared';
import { eventInstanceKey } from '@aperio/shared';
import {
  expandAll,
  expandToDayOccurrences,
  localDateKey,
} from '@aperio/shared';

import {
  Calendar,
  CalendarEvent,
  getEvents,
  listCalendars,
} from '../api/calendar';
import { listColorLabels } from '../api/colorLabels';
import { ActionsMenu, type MenuAction } from '../components/ActionsMenu';
import { CalendarActions } from '../components/CalendarActions';
import { useNewEventOnDay } from '../components/useNewEventOnDay';
import { CalendarPager } from '../components/CalendarPager';
import { useCalendarPagerOwnsHeading } from '../components/useCalendarPagerOwnsHeading';
import { CalendarViewSwitcher } from '../components/CalendarViewSwitcher';
import { JumpToDateButton } from '../components/JumpToDateButton';
import { CALENDAR_VIEW_ROUTE } from '../components/calendarViews';
import { useTabBarInset } from '../hooks/useTabBarInset';
import { resolveEventColor } from '../intl/eventColor';
import { useCacheReload } from '../state/cacheObserver';
import { hapticLoadBegin, hapticLoadEnd } from '../state/haptics';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { confirmDeleteEvent } from '../state/eventDeleteScope';
import { editEventWithScope } from '../state/eventEditScope';
import type { RootStackScreenProps } from '../navigation/types';
import { MagicTapView } from '../components/MagicTapView';
import { useThemedStyles, type ThemeColors } from '../theme';
import { chrome } from '../theme/uiScale';

// Accessible Agenda view — a flat ~30-day-forward list of events grouped by
// day, the screen-reader-natural sibling of the day view (EventsScreen). Same
// engine pipeline (listCalendars + palette + getEvents per calendar + expandAll
// recurrence), then expandToDayOccurrences spreads multi-day all-day events
// into one row per covered day (shared with desktop). Each day gets an
// accessible header row; the full day is also folded into every event row's
// label so a row read in isolation still announces its date.

const AGENDA_DAYS = 30;

/** Local-midnight clone of `date`. */
function localMidnight(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

function addMonths(date: Date, months: number): Date {
  const next = new Date(date);
  next.setMonth(next.getMonth() + months);
  return next;
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function AgendaScreen({
  route,
  navigation,
}: RootStackScreenProps<'Agenda'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { hidden } = useCalendarVisibility();
  const tabBarInset = useTabBarInset();

  // Window anchor (local midnight); seeded from the switcher's `anchor` param so
  // switching Day⇄Agenda keeps the selected date, else today.
  const [anchor, setAnchor] = useState(() => {
    const seed = route.params?.anchor ? new Date(route.params.anchor) : new Date();
    return localMidnight(Number.isNaN(seed.getTime()) ? new Date() : seed);
  });
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [occurrences, setOccurrences] = useState<DayOccurrence<CalendarEvent>[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // First load blanks; later reloads (focus return, delete/edit, cache refresh)
  // keep the current list on screen and refresh in place — the view stays open
  // instead of flashing the loading screen.
  const hasLoadedRef = useRef(false);
  // Long-press action menu — the sighted twin of the rows'/headers' SR custom
  // actions (one shared action list feeds both).
  const [menu, setMenu] = useState<{
    title: string;
    actions: MenuAction[];
    onAction: (name: string) => void;
  } | null>(null);
  // Request epoch — focus, a cache-refresh push and an anchor change can each
  // fire load() while another is in flight; a slower OLDER load must not
  // overwrite a newer window's results (mirrors CalendarDayList). With
  // keep-view-open there's no loading flash to mask such a lost update, so the
  // guard matters more.
  const reqToken = useRef(0);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const calendarsById = useMemo(
    () => new Map(calendars.map((c) => [c.id, c])),
    [calendars],
  );
  const labelsById = useMemo(
    () => new Map(colorLabels.map((l) => [l.id, l])),
    [colorLabels],
  );

  // The visible window: anchor 00:00 → end of (anchor + 30 days).
  const range = useMemo(() => {
    const start = localMidnight(anchor);
    const end = addDays(start, AGENDA_DAYS);
    end.setHours(23, 59, 59, 999);
    return { start, end };
  }, [anchor]);

  const fmtFullDate = useCallback(
    (d: Date) =>
      d.toLocaleDateString(i18n.language, {
        weekday: 'long',
        year: 'numeric',
        month: 'long',
        day: 'numeric',
      }),
    [i18n.language],
  );
  const fmtShortDate = useCallback(
    (d: Date) =>
      d.toLocaleDateString(i18n.language, { year: 'numeric', month: 'long', day: 'numeric' }),
    [i18n.language],
  );
  const timeLabel = useCallback(
    (ev: CalendarEvent): string => {
      if (ev.all_day) return t('views.allDay');
      const fmt = (iso: string) =>
        new Date(iso).toLocaleTimeString(i18n.language, { hour: '2-digit', minute: '2-digit' });
      return `${fmt(ev.start)}–${fmt(ev.end)}`;
    },
    [i18n.language, t],
  );

  const load = useCallback(async () => {
    const token = (reqToken.current += 1);
    if (!hasLoadedRef.current) setLoading(true);
    setError(null);
    hapticLoadBegin();
    try {
      const [cals, labels] = await Promise.all([
        listCalendars(),
        listColorLabels().catch(() => [] as ColorLabel[]),
      ]);
      const startIso = range.start.toISOString();
      const endIso = range.end.toISOString();
      const perCalendar = await Promise.all(
        cals.map((c) =>
          getEvents({ calendar_id: c.id, start: startIso, end: endIso }).catch(() => []),
        ),
      );
      // A newer load superseded this one — drop these stale results so a slow
      // older window can't overwrite the fresh one.
      if (reqToken.current !== token) return;
      setCalendars(cals);
      setColorLabels(labels);
      // Expand recurring series across the whole window first, then spread
      // multi-day all-day events into one occurrence per covered day.
      const expanded = expandAll(perCalendar.flat(), { start: range.start, end: range.end });
      // Drop events from calendars the user hid (the Calendars-screen toggles).
      const visible = expanded.filter((e) => !hidden.has(e.calendar_id));
      setOccurrences(expandToDayOccurrences(visible, range));
      hasLoadedRef.current = true;
    } catch (err) {
      if (reqToken.current !== token) return;
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      if (reqToken.current === token) setLoading(false);
      // Balances hapticLoadBegin above — always, even for a superseded load.
      hapticLoadEnd();
    }
  }, [announce, range, t, hidden]);

  // Reload when the window changes or the screen regains focus (after the editor).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // Live-update while focused when an external calendar-cache refresh lands (the
  // root observer already announced it politely; this just re-reads the window).
  useCacheReload('calendar', load);

  // Per-day event counts for the accessible day-header labels.
  const dayCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const occ of occurrences) {
      const k = localDateKey(occ.day);
      counts.set(k, (counts.get(k) ?? 0) + 1);
    }
    return counts;
  }, [occurrences]);

  const readOnlyIds = useMemo(
    () => new Set(calendars.filter((c) => c.read_only).map((c) => c.id)),
    [calendars],
  );

  // First writable calendar — the seed target for the per-day "+ new event"
  // affordance (the day-anchored create, mirroring the desktop calendar views).
  const firstWritableCalendarId = useMemo(
    () =>
      calendars.find((c) => !c.read_only && !hidden.has(c.id))?.id ??
      calendars.find((c) => !c.read_only)?.id ??
      null,
    [calendars, hidden],
  );
  const addEventOnDay = useCallback(
    (dayKey: string) => {
      if (firstWritableCalendarId == null) return;
      // → the event quick-add (expands to the full editor via "More details …").
      navigation.navigate('QuickAddEvent', {
        calendarId: firstWritableCalendarId,
        anchor: dayKey,
      });
    },
    [firstWritableCalendarId, navigation],
  );

  const goToday = useCallback(() => setAnchor(localMidnight(new Date())), []);

  const stepMonths = useCallback(
    (delta: number) => setAnchor((a) => localMidnight(addMonths(a, delta))),
    [],
  );

  // VoiceOver MAGIC TAP (two-finger double-tap) creates a new event on the
  // window's anchor day — the same flow as the toolbar's New Event.
  const { addEvent: magicTapCreate } = useNewEventOnDay(navigation, anchor);

  const rangeLabel = useMemo(
    () =>
      `${fmtShortDate(range.start)} – ${fmtShortDate(localMidnight(addDays(anchor, AGENDA_DAYS)))}`,
    [anchor, fmtShortDate, range.start],
  );

  // Announce the period on navigation (the three-finger swipe / prev-next shift
  // the window silently otherwise). Skip the first render — nothing changed yet.
  // Mirrors MonthScreen.
  const pagerOwnsHeading = useCalendarPagerOwnsHeading();
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    if (pagerOwnsHeading) return;
    announce(rangeLabel);
  }, [rangeLabel, announce, pagerOwnsHeading]);

  const editEvent = useCallback(
    (ev: CalendarEvent) =>
      // A recurring occurrence pops the "this occurrence vs whole series" prompt
      // first, then opens the editor locked to the choice (shared helper).
      editEventWithScope(ev, t, (params) =>
        navigation.navigate('EventEditor', params),
      ),
    [navigation, t],
  );

  // Move / copy to another calendar — pass the full (possibly expanded) row so
  // the modal can offer the occurrence-vs-series scope.
  const moveCopyEvent = useCallback(
    (ev: CalendarEvent) => navigation.navigate('MoveCopy', { kind: 'event', event: ev }),
    [navigation],
  );

  // Delete with recurrence scope: an occurrence offers "this occurrence" (exdate)
  // vs "whole series"; a single event a plain delete (shared helper).
  const removeEvent = useCallback(
    (ev: CalendarEvent) =>
      confirmDeleteEvent(
        ev,
        t,
        (message) => {
          announce(message);
          void load();
        },
        (message) => {
          setError(message);
          announce(t('mobile.error', { message }));
        },
        {
          supportsScheduling:
            calendars.find((c) => c.id === ev.calendar_id)
              ?.supports_scheduling ?? false,
        },
      ),
    [announce, calendars, load, t],
  );

  const rowLabel = useCallback(
    (ev: CalendarEvent, day: Date, span: MultiDayInfo | null): string => {
      let label = t('views.agenda.eventLabel', {
        day: fmtFullDate(day),
        title: ev.title,
        time: timeLabel(ev),
        calendar: calendarsById.get(ev.calendar_id)?.name ?? '—',
      });
      if (span) {
        label += t('views.multiDaySuffix', { day: span.dayIndex, total: span.totalDays });
      }
      if (ev.cancelled) {
        label += t('views.eventCancelledSuffix');
      }
      const colour = resolveEventColor(ev, calendarsById, labelsById);
      if (colour.labelName) {
        label += t('mobile.colorLabelSuffix', { name: colour.labelName });
      }
      return label;
    },
    [calendarsById, fmtFullDate, labelsById, t, timeLabel],
  );

  return (
    <MagicTapView style={styles.screen} onMagicTap={magicTapCreate}>
      {/* Day ⇄ Week ⇄ Month ⇄ Agenda, carrying the anchor so the date survives
          the switch. replace (not push): sibling views swap in place, keeping
          the stack flat. Pressing the active view is suppressed by the switcher. */}
      <CalendarViewSwitcher
        active="agenda"
        onSelect={(v) =>
          navigation.replace(CALENDAR_VIEW_ROUTE[v], { anchor: anchor.toISOString() })
        }
      />

      <View style={styles.navBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.prev')}
          onPress={() => stepMonths(-1)}
          hitSlop={8}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">‹</Text>
        </Pressable>
        {!pagerOwnsHeading && (
          <Text style={styles.rangeHeading} accessibilityRole="header">
            {rangeLabel}
          </Text>
        )}
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.next')}
          onPress={() => stepMonths(1)}
          hitSlop={8}
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
          hitSlop={8}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.today')}</Text>
        </Pressable>
        {/* A real button that opens the picker on tap (no always-visible date
            chip duplicating the heading). */}
        <JumpToDateButton
          value={anchor}
          onSelect={(date) => {
            setError(null);
            setAnchor(localMidnight(date));
          }}
        />
        <CalendarActions navigation={navigation} anchorDay={anchor} />
      </View>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {/* Three-finger swipe (VoiceOver) / horizontal flick pages between the
          monthly agenda windows — the pager wraps the loading/empty states too,
          so paging out of an empty window works. Mirrors MonthScreen. */}
      <CalendarPager
        onPrev={() => stepMonths(-1)}
        onNext={() => stepMonths(1)}
        periodLabel={rangeLabel}
      >
      {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('views.loading')}>
          {t('views.loading')}
        </Text>
      ) : occurrences.length === 0 ? (
        <Text style={styles.muted}>{t('views.agenda.empty')}</Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          accessibilityLabel={t('views.agenda.eventList')}
          // flex:1 so the list fills the pager's fixed-size page and keeps its
          // own vertical scrolling (the CalendarDayList precedent).
          style={styles.scroll}
          contentContainerStyle={[styles.list, { paddingBottom: tabBarInset }]}
          keyboardShouldPersistTaps="handled"
        >
          {(() => {
            let prevKey: string | null = null;
            const rows: ReactNode[] = [];
            for (const occ of occurrences) {
              const key = localDateKey(occ.day);
              if (key !== prevKey) {
                prevKey = key;
                // Day-anchored create lives on the HEADER (SR custom action +
                // sighted long-press) — the old per-day "+ new event" footer
                // buttons were toolbar duplicates under every day (tester
                // feedback; same treatment as Week/Month).
                const headerActions: MenuAction[] =
                  firstWritableCalendarId != null
                    ? [{ name: 'newEvent', label: t('toolbar.newEvent') }]
                    : [];
                const dayKey = key;
                const headerTitle = fmtFullDate(occ.day);
                const runHeaderAction = (name: string) => {
                  if (name === 'newEvent') addEventOnDay(dayKey);
                };
                // Hosted in a Pressable (not a bare Text) — the device-proven
                // accessibilityActions pattern; role="header" keeps the
                // headings rotor. Mirrors CalendarDayList.
                rows.push(
                  <Pressable
                    key={`h-${key}`}
                    accessible
                    accessibilityRole="header"
                    accessibilityLabel={t('views.agenda.dayLabel', {
                      day: fmtFullDate(occ.day),
                      count: dayCounts.get(key) ?? 0,
                    })}
                    accessibilityActions={headerActions}
                    onAccessibilityAction={(e) => runHeaderAction(e.nativeEvent.actionName)}
                    onLongPress={
                      headerActions.length > 0
                        ? () =>
                            setMenu({
                              title: headerTitle,
                              actions: headerActions,
                              onAction: runHeaderAction,
                            })
                        : undefined
                    }
                  >
                    <Text style={styles.dayHeader} importantForAccessibility="no">
                      {fmtFullDate(occ.day)}
                    </Text>
                  </Pressable>,
                );
              }
              rows.push(renderRow(occ, key));
            }
            return rows;
          })()}
        </ScrollView>
      )}
      </CalendarPager>

      {/* The long-press action menu (one instance for the whole screen). */}
      <ActionsMenu
        visible={menu != null}
        title={menu?.title ?? ''}
        actions={menu?.actions ?? []}
        onAction={menu?.onAction ?? (() => undefined)}
        onClose={() => setMenu(null)}
      />
    </MagicTapView>
  );

  function renderRow(occ: DayOccurrence<CalendarEvent>, dayKey: string) {
    const ev = occ.ev;
    const rowKey = `${eventInstanceKey(ev)}@${dayKey}`;
    const hex = resolveEventColor(ev, calendarsById, labelsById).hex;
    // Sighted colour: tint the whole tile (replaces the colour dot) — matches
    // the CalendarDayList event rows; SR users get the label NAME in rowLabel.
    const tint =
      hex != null ? { backgroundColor: `${hex}2E`, borderColor: `${hex}66` } : null;
    // Cancelled event: dim the tile + strike the title (matches desktop); SR users
    // get ", abgesagt" in rowLabel.
    const cancelledTile = ev.cancelled ? styles.cancelledTile : null;
    const titleStyle = ev.cancelled
      ? [styles.eventTitle, styles.cancelledTitle]
      : styles.eventTitle;
    const badge = occ.span
      ? ` ${t('views.multiDayCompact', { day: occ.span.dayIndex, total: occ.span.totalDays })}`
      : '';
    if (readOnlyIds.has(ev.calendar_id)) {
      return (
        <View
          key={rowKey}
          accessible
          accessibilityRole="text"
          accessibilityLabel={rowLabel(ev, occ.day, occ.span)}
          style={[styles.row, tint, cancelledTile]}
        >
          <View style={styles.rowText}>
            <Text style={titleStyle} importantForAccessibility="no">
              {ev.title}
              {badge}
            </Text>
            <Text style={styles.eventTime} importantForAccessibility="no">
              {timeLabel(ev)}
            </Text>
          </View>
        </View>
      );
    }
    // ONE action list feeds the SR custom actions AND the sighted long-press
    // menu; the per-row delete button is gone (delete lives in the editor, the
    // menu and the SR action). Mirrors CalendarDayList.
    const actions: MenuAction[] = [
      { name: 'activate', label: t('mobile.editTaskLabel') },
      { name: 'moveCopy', label: t('mobile.moveCopy') },
      { name: 'delete', label: t('dialogs.event.delete'), destructive: true },
    ];
    const runAction = (name: string) => {
      if (name === 'delete') removeEvent(ev);
      else if (name === 'moveCopy') moveCopyEvent(ev);
      else editEvent(ev);
    };
    return (
      <View
        key={rowKey}
        accessible
        accessibilityRole="button"
        accessibilityLabel={rowLabel(ev, occ.day, occ.span)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityActions={actions}
        onAccessibilityAction={(e) => runAction(e.nativeEvent.actionName)}
        style={[styles.row, tint]}
      >
        <Pressable
          accessible={false}
          onPress={() => editEvent(ev)}
          onLongPress={() => setMenu({ title: ev.title, actions, onAction: runAction })}
          style={styles.rowText}
        >
          <Text style={titleStyle} importantForAccessibility="no">
            {ev.title}
            {badge}
          </Text>
          <Text style={styles.eventTime} importantForAccessibility="no">
            {timeLabel(ev)}
          </Text>
        </Pressable>
      </View>
    );
  }
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
      fontSize: 15,
      fontWeight: '700',
      color: c.textPrimary,
      textAlign: 'center',
    },
    // Hug the chevron instead of a fixed 44×44 box (which left a big empty
    // square around a narrow glyph, independent of font size). The 44pt tap
    // target is preserved by `hitSlop` on the Pressable.
    navButton: {
      paddingVertical: chrome(4),
      paddingHorizontal: chrome(12),
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      alignItems: 'center',
      justifyContent: 'center',
      backgroundColor: c.surfaceAlt,
    },
    navButtonText: { fontSize: 24, color: c.textPrimary, lineHeight: 26 },
    actionBar: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, padding: 12, alignItems: 'center' },
    ghostButton: {
      paddingVertical: chrome(6),
      paddingHorizontal: chrome(12),
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    scroll: { flex: 1 },
    list: { gap: chrome(8), padding: chrome(12) },
    dayHeader: {
      fontSize: 15,
      fontWeight: '700',
      color: c.textLabel,
      marginTop: 8,
      marginBottom: 2,
    },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: chrome(10),
      padding: chrome(12),
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowText: { flex: 1, gap: 2 },
    eventTitle: { fontSize: 18, fontWeight: '600', color: c.textPrimary },
    // Cancelled event: dim the tile + strike the title (matches desktop).
    cancelledTile: { opacity: 0.6 },
    cancelledTitle: { textDecorationLine: 'line-through' as const },
    eventTime: { fontSize: 14, color: c.textSecondary },
    pressed: { opacity: 0.7 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
  });
