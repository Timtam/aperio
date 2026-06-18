import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { Calendar, listCalendars } from '../api/calendar';
import { search, SearchResults } from '../api/search';
import type { RootStackScreenProps } from '../navigation/types';
import { useTaskStore } from '../state/taskStoreContext';

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

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SearchResults>(EMPTY);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
      month: 'short',
      day: 'numeric',
    });
    const dateTime = new Intl.DateTimeFormat(i18n.language, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
    return (iso: string, allDay: boolean) =>
      (allDay ? dateOnly : dateTime).format(new Date(iso));
  }, [i18n.language]);

  // Debounced search with a stale-result guard (the latest query wins).
  const lastQuery = useRef('');
  useEffect(() => {
    const q = query.trim();
    lastQuery.current = q;
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
          const r = await search(q);
          if (lastQuery.current !== q) return;
          setResults(r);
          setError(null);
        } catch (err) {
          if (lastQuery.current !== q) return;
          setError(errorMessage(err));
          setResults(EMPTY);
        } finally {
          if (lastQuery.current === q) setLoading(false);
        }
      })();
    }, DEBOUNCE_MS);
    return () => clearTimeout(handle);
  }, [query]);

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
                key={`e-${ev.id}`}
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

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  input: {
    margin: 16,
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  status: { fontSize: 14, color: '#5b6573', paddingHorizontal: 16, paddingBottom: 4 },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318', paddingHorizontal: 16 },
  muted: { fontSize: 15, color: '#5b6573', padding: 16 },
  list: { gap: 8, padding: 16 },
  groupHeader: { fontSize: 15, fontWeight: '700', color: '#2b3240', marginTop: 8 },
  row: {
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  rowPressed: { backgroundColor: '#e4ebf5' },
  rowTitle: { fontSize: 17, fontWeight: '600', color: '#10131a' },
  rowSecondary: { fontSize: 14, color: '#5b6573', marginTop: 2 },
});
