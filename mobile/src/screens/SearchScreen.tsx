import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from 'react';
import { useTranslation } from 'react-i18next';
import {
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { eventInstanceKey, statusI18nKey, type TaskStatus } from '@aperio/shared';

import { expandedA11y } from '../a11y/roles';
import { Calendar, listCalendars } from '../api/calendar';
import {
  search,
  EventTypeFilter,
  SearchFilters,
  SearchKind,
  SearchResults,
} from '../api/search';
import { CheckboxGroup } from '../components/CheckboxGroup';
import { RadioGroup } from '../components/RadioGroup';
import type { RootStackScreenProps } from '../navigation/types';
import { useTaskStore } from '../state/taskStoreContext';
import { useThemedStyles, type ThemeColors } from '../theme';

/** The selectable task statuses, in the desktop's order. */
const TASK_STATUSES: TaskStatus[] = ['open', 'in_progress', 'completed', 'cancelled'];

// Global local search — a faithful port of the desktop SearchDialog's core flow
// (events + tasks, FTS via the Host's search_json). Screen-reader-first: a
// search field, a polite live-region result-count, and a list grouped by kind
// (accessible headers) whose rows announce "Event/Task: title, secondary" and
// open the matching editor. Filters (kind/calendar/list/date/status) + the
// external snapshot-cache half are later increments (mirrors the desktop's own
// phasing). Both audiences: minimal title + secondary rows, like the desktop.

const DEBOUNCE_MS = 200;
const EMPTY: SearchResults = { events: [], tasks: [] };

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SearchScreen({ navigation }: RootStackScreenProps<'Search'>) {
  const { t, i18n } = useTranslation();
  const { taskLists } = useTaskStore();
  const styles = useThemedStyles(makeStyles);

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SearchResults>(EMPTY);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Filters (kind + date range — the simple, high-value subset; calendar/list
  // multi-selects are a later refinement). Hidden behind a toggle so the result
  // list stays prominent.
  const [showFilters, setShowFilters] = useState(false);
  const [kind, setKind] = useState<SearchKind>('both');
  const [since, setSince] = useState('');
  const [until, setUntil] = useState('');
  // Container + type/status narrowing (the backend treats empty sets / 'any' as
  // no restriction). Calendars + event-type apply to events; lists + statuses to
  // tasks — each shown only for the relevant `kind`.
  const [calendarIds, setCalendarIds] = useState<Set<string>>(new Set());
  const [listIds, setListIds] = useState<Set<string>>(new Set());
  const [eventType, setEventType] = useState<EventTypeFilter>('any');
  const [taskStatuses, setTaskStatuses] = useState<Set<string>>(new Set());

  // Toggle one id in a Set-valued filter (a fresh Set so the memo dep changes).
  const toggleIn = useCallback(
    (setter: Dispatch<SetStateAction<Set<string>>>) => (id: string) =>
      setter((prev) => {
        const next = new Set(prev);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
      }),
    [],
  );

  // Calendars (for the event row's calendar name); best-effort.
  useEffect(() => {
    listCalendars()
      .then(setCalendars)
      .catch(() => {});
  }, []);

  const calendarName = useCallback(
    (id: string) => calendars.find((c) => c.id === id)?.name ?? '—',
    [calendars],
  );
  const listName = useCallback(
    (id: string) => taskLists.find((l) => l.id === id)?.name ?? '—',
    [taskLists],
  );

  const formatDate = useMemo(() => {
    const dateOnly = new Intl.DateTimeFormat(i18n.language, {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    });
    const dateTime = new Intl.DateTimeFormat(i18n.language, {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
    return (iso: string, allDay: boolean) =>
      (allDay ? dateOnly : dateTime).format(new Date(iso));
  }, [i18n.language]);

  // The active filter set; identity stable so the search effect only re-runs
  // when a filter actually changes. The desktop sends the date range as
  // start-of-day / end-of-day UTC instants.
  const filters = useMemo<SearchFilters>(
    () => ({
      kind,
      calendar_ids: [...calendarIds],
      list_ids: [...listIds],
      since: since.trim() ? `${since.trim()}T00:00:00Z` : null,
      until: until.trim() ? `${until.trim()}T23:59:59Z` : null,
      event_type: eventType,
      task_statuses: [...taskStatuses],
    }),
    [kind, calendarIds, listIds, since, until, eventType, taskStatuses],
  );

  // Debounced search with a request-token stale-result guard (the latest
  // query+filters wins, so a slow earlier call can't overwrite newer results).
  const reqToken = useRef(0);
  useEffect(() => {
    const q = query.trim();
    const token = (reqToken.current += 1);
    if (q === '') {
      setResults(EMPTY);
      setLoading(false);
      setError(null);
      return;
    }
    setLoading(true);
    const handle = setTimeout(() => {
      void (async () => {
        try {
          const r = await search(q, filters);
          if (reqToken.current !== token) return;
          setResults(r);
          setError(null);
        } catch (err) {
          if (reqToken.current !== token) return;
          setError(errorMessage(err));
          setResults(EMPTY);
        } finally {
          if (reqToken.current === token) setLoading(false);
        }
      })();
    }, DEBOUNCE_MS);
    return () => clearTimeout(handle);
  }, [query, filters]);

  const total = results.events.length + results.tasks.length;
  const hasQuery = query.trim() !== '';

  // The polite live-region status line: searching / result count.
  const status = loading
    ? t('dialogs.search.searching')
    : t('dialogs.search.resultCount', { count: total });

  const eventSecondary = (calendarId: string, start: string, allDay: boolean) =>
    t('dialogs.search.eventSecondary', {
      date: formatDate(start, allDay),
      calendar: calendarName(calendarId),
    });

  return (
    <View style={styles.screen}>
      <TextInput
        style={styles.input}
        value={query}
        onChangeText={setQuery}
        placeholder={t('dialogs.search.placeholder')}
        accessibilityLabel={t('dialogs.search.field')}
        autoFocus
        autoCorrect={false}
        autoCapitalize="none"
        returnKeyType="search"
        clearButtonMode="while-editing"
      />

      <Pressable
        accessibilityRole="button"
        {...expandedA11y(showFilters, t(showFilters ? 'mobile.expandedState' : 'mobile.collapsedState'))}
        accessibilityLabel={t('dialogs.search.filtersTitle')}
        onPress={() => setShowFilters((v) => !v)}
        style={({ pressed }) => [styles.filtersToggle, pressed && styles.pressed]}
      >
        <Text style={styles.filtersToggleText} importantForAccessibility="no">
          {showFilters ? '▾' : '▸'} {t('dialogs.search.filtersTitle')}
        </Text>
      </Pressable>

      {showFilters && (
        <View style={styles.filters}>
          <RadioGroup<SearchKind>
            label={t('dialogs.search.kindLabel')}
            value={kind}
            options={[
              { value: 'both', label: t('dialogs.search.kind.both') },
              { value: 'events', label: t('dialogs.search.kind.events') },
              { value: 'tasks', label: t('dialogs.search.kind.tasks') },
            ]}
            onChange={setKind}
          />
          <View style={styles.rangeRow}>
            <View style={styles.rangeField}>
              <Text style={styles.rangeLabel}>{t('dialogs.search.sinceLabel')}</Text>
              <TextInput
                style={styles.rangeInput}
                value={since}
                onChangeText={setSince}
                placeholder="YYYY-MM-DD"
                accessibilityLabel={`${t('dialogs.search.rangeLabel')}, ${t('dialogs.search.sinceLabel')}`}
                autoCapitalize="none"
                autoCorrect={false}
              />
            </View>
            <View style={styles.rangeField}>
              <Text style={styles.rangeLabel}>{t('dialogs.search.untilLabel')}</Text>
              <TextInput
                style={styles.rangeInput}
                value={until}
                onChangeText={setUntil}
                placeholder="YYYY-MM-DD"
                accessibilityLabel={`${t('dialogs.search.rangeLabel')}, ${t('dialogs.search.untilLabel')}`}
                autoCapitalize="none"
                autoCorrect={false}
              />
            </View>
          </View>

          {/* Event-only filters (hidden when searching tasks only). */}
          {kind !== 'tasks' && calendars.length > 0 && (
            <CheckboxGroup
              label={t('dialogs.search.calendarsLabel')}
              hint={t('dialogs.search.containersHint')}
              options={calendars.map((c) => ({ value: c.id, label: c.name }))}
              selected={calendarIds}
              onToggle={toggleIn(setCalendarIds)}
            />
          )}
          {kind !== 'tasks' && (
            <RadioGroup<EventTypeFilter>
              label={t('dialogs.search.eventTypeLabel')}
              value={eventType}
              options={[
                { value: 'any', label: t('dialogs.search.eventType.any') },
                { value: 'single', label: t('dialogs.search.eventType.single') },
                { value: 'recurring', label: t('dialogs.search.eventType.recurring') },
                { value: 'all_day', label: t('dialogs.search.eventType.all_day') },
              ]}
              onChange={setEventType}
            />
          )}

          {/* Task-only filters (hidden when searching events only). */}
          {kind !== 'events' && taskLists.length > 0 && (
            <CheckboxGroup
              label={t('dialogs.search.listsLabel')}
              hint={t('dialogs.search.containersHint')}
              options={taskLists.map((l) => ({ value: l.id, label: l.name }))}
              selected={listIds}
              onToggle={toggleIn(setListIds)}
            />
          )}
          {kind !== 'events' && (
            <CheckboxGroup
              label={t('dialogs.search.taskStatusLabel')}
              options={TASK_STATUSES.map((s) => ({
                value: s,
                label: t(statusI18nKey(s)),
              }))}
              selected={taskStatuses}
              onToggle={toggleIn(setTaskStatuses)}
            />
          )}
        </View>
      )}

      {hasQuery && (
        <Text style={styles.status} accessibilityRole="text" accessibilityLiveRegion="polite">
          {status}
        </Text>
      )}
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {!hasQuery ? (
        <Text style={styles.muted} accessibilityRole="text">
          {t('dialogs.search.prompt')}
        </Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.search.results')}
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {!loading && total === 0 && (
            <Text style={styles.muted} accessibilityRole="text">
              {t('dialogs.search.noResults')}
            </Text>
          )}

          {results.events.length > 0 && (
            <Text style={styles.groupHeader} accessibilityRole="header">
              {t('dialogs.search.kindEvent')}
            </Text>
          )}
          {results.events.map((ev) => {
            const secondary = eventSecondary(ev.calendar_id, ev.start, ev.all_day);
            return (
              <Pressable
                key={`e-${eventInstanceKey(ev)}`}
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.search.eventAria', {
                  title: ev.title,
                  secondary,
                })}
                accessibilityHint={t('mobile.taskHint')}
                onPress={() =>
                  navigation.navigate('EventEditor', {
                    eventId: ev.id,
                    calendarId: ev.calendar_id,
                  })
                }
                style={({ pressed }) => [styles.row, pressed && styles.rowPressed]}
              >
                <Text style={styles.rowTitle} importantForAccessibility="no">
                  {ev.title}
                </Text>
                <Text style={styles.rowSecondary} importantForAccessibility="no">
                  {secondary}
                </Text>
              </Pressable>
            );
          })}

          {results.tasks.length > 0 && (
            <Text style={styles.groupHeader} accessibilityRole="header">
              {t('dialogs.search.kindTask')}
            </Text>
          )}
          {results.tasks.map((task) => {
            const secondary = t('dialogs.search.taskSecondary', {
              list: listName(task.list_id),
            });
            return (
              <Pressable
                key={`t-${task.id}`}
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.search.taskAria', {
                  title: task.title,
                  secondary,
                })}
                accessibilityHint={t('mobile.taskHint')}
                onPress={() =>
                  navigation.navigate('TaskEditor', {
                    taskId: task.id,
                    listId: task.list_id,
                  })
                }
                style={({ pressed }) => [styles.row, pressed && styles.rowPressed]}
              >
                <Text style={styles.rowTitle} importantForAccessibility="no">
                  {task.title}
                </Text>
                <Text style={styles.rowSecondary} importantForAccessibility="no">
                  {secondary}
                </Text>
              </Pressable>
            );
          })}
        </ScrollView>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    input: {
      margin: 16,
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    filtersToggle: { paddingHorizontal: 16, paddingBottom: 8 },
    filtersToggleText: { fontSize: 15, fontWeight: '600', color: c.accent },
    filters: { paddingHorizontal: 16, paddingBottom: 8, gap: 10 },
    rangeRow: { flexDirection: 'row', gap: 10 },
    rangeField: { flex: 1, gap: 4 },
    rangeLabel: { fontSize: 14, fontWeight: '600', color: c.textLabel },
    rangeInput: {
      fontSize: 16,
      color: c.textPrimary,
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    status: { fontSize: 14, color: c.textSecondary, paddingHorizontal: 16, paddingBottom: 4 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    list: { gap: 8, padding: 16 },
    groupHeader: { fontSize: 15, fontWeight: '700', color: c.textLabel, marginTop: 8 },
    row: {
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowPressed: { backgroundColor: c.surfacePressed },
    pressed: { opacity: 0.7 },
    rowTitle: { fontSize: 17, fontWeight: '600', color: c.textPrimary },
    rowSecondary: { fontSize: 14, color: c.textSecondary, marginTop: 2 },
  });
