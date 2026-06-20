import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { upcomingReminders, UpcomingReminder } from '../api/reminders';
import { useThemedStyles, type ThemeColors } from '../theme';

// Read-only overview of upcoming reminder triggers — the mobile twin of the
// desktop "Reminders overview" dialog. Lists the same triggers the on-device
// scheduler turns into OS notifications (one source of truth via the Host's
// host_core::reminders enumeration), so a screen-reader user can review what's
// coming up at a glance. Reachable from Settings.

/** The overview is a planning view of upcoming reminders — match the DESKTOP
 *  overview's 90-day forward horizon (src-tauri reminders OVERVIEW_FUTURE_DAYS),
 *  NOT the on-device scheduler's deliberately narrow 7-day window (that bound
 *  exists only to stay under iOS's ~64 pending-notification limit; see
 *  reminders/scheduler.ts). Kept at 90 days = the Host's EXTERNAL_FUTURE_DAYS so
 *  the external reminder scan still covers the whole window. */
const HORIZON_MINUTES = 90 * 24 * 60;

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function RemindersScreen() {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [reminders, setReminders] = useState<UpcomingReminder[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setReminders(await upcomingReminders(HORIZON_MINUTES));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, []);

  // Refresh on every focus so a just-created event/task's reminder shows up.
  useFocusEffect(
    useCallback(() => {
      void load();
    }, [load]),
  );

  const whenLabel = useCallback(
    (iso: string): string =>
      new Date(iso).toLocaleString(i18n.language, {
        weekday: 'long',
        month: 'long',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      }),
    [i18n.language],
  );

  const rowLabel = useCallback(
    (r: UpcomingReminder): string =>
      t(r.item_kind === 'event' ? 'dialogs.reminders.eventRow' : 'dialogs.reminders.taskRow', {
        when: whenLabel(r.trigger_at),
        title: r.title,
      }),
    [t, whenLabel],
  );

  return (
    <ScrollView style={styles.screen} contentContainerStyle={styles.content}>
      <Text
        style={styles.count}
        accessibilityRole="text"
        accessibilityLiveRegion="polite"
      >
        {t('dialogs.reminders.count', { count: reminders.length })}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('dialogs.reminders.loading')}>
          {t('dialogs.reminders.loading')}
        </Text>
      ) : reminders.length === 0 ? (
        <Text style={styles.muted}>{t('dialogs.reminders.empty')}</Text>
      ) : (
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.reminders.listLabel')}
          style={styles.list}
        >
          {reminders.map((r) => (
            <View
              key={`${r.item_id}:${r.trigger_at}`}
              accessible
              accessibilityRole="text"
              accessibilityLabel={rowLabel(r)}
              style={styles.row}
            >
              <Text style={styles.rowTitle}>{r.title}</Text>
              <Text style={styles.rowWhen}>
                {`${r.item_kind === 'event' ? t('dialogs.reminders.kindEvent') : t('dialogs.reminders.kindTask')} · ${whenLabel(r.trigger_at)}`}
              </Text>
              {r.body !== '' && r.body !== r.title && (
                <Text style={styles.rowBody}>{r.body}</Text>
              )}
            </View>
          ))}
        </View>
      )}
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 12 },
    count: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    list: { gap: 12 },
    row: {
      gap: 2,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowTitle: { fontSize: 18, fontWeight: '600', color: c.textPrimary },
    rowWhen: { fontSize: 14, color: c.textSecondary },
    rowBody: { fontSize: 14, color: c.textLabel },
    muted: { fontSize: 15, color: c.textSecondary },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
