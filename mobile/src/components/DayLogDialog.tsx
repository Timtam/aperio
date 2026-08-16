import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import {
  emptyDayLog,
  sortDayMarkers,
  toggleDayMarker,
  type DayLog,
  type DayMarker,
} from '@aperio/shared';

import { getDayLog, listDayMarkers, setDayLog } from '../api/dayMarkers';
import { selectableRole } from '../a11y/roles';
import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

// Tick a day with the user's own markers. Twin of the desktop DayLogDialog.
//
// One row per marker, each a checkbox that writes STRAIGHT THROUGH. Recording
// a day has to cost almost nothing, and a Save step at the end would be most
// of its cost — so the dialog's only button closes it, and unticking is the
// undo. Nothing to confirm means nothing to lose by backing out.

export function DayLogDialog({
  visible,
  onClose,
  day,
  dayLabel,
}: {
  visible: boolean;
  onClose: () => void;
  /** Local day key, `YYYY-MM-DD`. */
  day: string;
  /** The day as the user reads it, for the dialog title. */
  dayLabel: string;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [markers, setMarkers] = useState<DayMarker[]>([]);
  const [log, setLog] = useState<DayLog>(() => emptyDayLog(day));
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Load each time it opens, not once per mount: the same dialog serves
  // whichever day the user is standing on.
  useEffect(() => {
    if (!visible) return;
    let cancelled = false;
    setLoading(true);
    setLog(emptyDayLog(day));
    setError(null);
    void (async () => {
      try {
        const [vocabulary, loaded] = await Promise.all([
          listDayMarkers(),
          getDayLog(day),
        ]);
        if (cancelled) return;
        setMarkers(sortDayMarkers(vocabulary));
        setLog(loaded);
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [visible, day]);

  const onToggle = useCallback(
    async (marker: DayMarker) => {
      const next = toggleDayMarker(log, marker.id);
      // Optimistic: the checkbox answers the tap, not the disk. A failed
      // write puts the old state back and says which one did not land.
      setLog(next);
      try {
        setLog(await setDayLog(next));
      } catch (err) {
        setLog(log);
        setError(err instanceof Error ? err.message : String(err));
        AccessibilityInfo.announceForAccessibility(
          t('dialogs.dayLog.writeFailed', { name: marker.name }),
        );
      }
    },
    [log, t],
  );

  if (!visible) return null;

  return (
    <AppDialog
      visible
      title={t('dialogs.dayLog.title', { day: dayLabel })}
      // One way out, not two. Every tick already saved, so there is nothing
      // for a Cancel to undo — offering one would promise an undo that does
      // not exist.
      cancelLabel={t('dialogs.dayLog.close')}
      onCancel={onClose}
    >
      <Text style={styles.intro} accessibilityRole="text">
        {t('dialogs.dayLog.intro')}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.dayLog.loading')}
        </Text>
      ) : markers.length === 0 ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.dayLog.noMarkers')}
        </Text>
      ) : (
        <View style={styles.list}>
          {markers.map((m) => {
            const ticked = log.markers.includes(m.id);
            return (
              <Pressable
                key={m.id}
                accessible
                accessibilityRole={selectableRole('checkbox')}
                accessibilityState={{ checked: ticked }}
                // The NAME, never the symbol: a screen reader announces an
                // emoji by whatever name it has for it, which is not what the
                // user called this.
                accessibilityLabel={m.name}
                onPress={() => void onToggle(m)}
                style={({ pressed }) => [
                  styles.row,
                  ticked && styles.rowChecked,
                  pressed && styles.pressed,
                ]}
              >
                <Text style={styles.marker} importantForAccessibility="no">
                  {ticked ? '☑' : '☐'}
                </Text>
                <Text style={styles.rowLabel} importantForAccessibility="no">
                  {m.symbol ? `${m.symbol} ${m.name}` : m.name}
                </Text>
              </Pressable>
            );
          })}
        </View>
      )}
    </AppDialog>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    intro: { fontSize: 15, color: c.textSecondary, marginBottom: 12 },
    hint: { fontSize: 13, color: c.textSecondary },
    error: { fontSize: 15, color: c.danger, marginBottom: 8 },
    list: { gap: 6 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    rowChecked: { borderColor: c.accent, backgroundColor: c.surfaceSelected },
    pressed: { backgroundColor: c.surfacePressed },
    marker: { fontSize: 18, width: 22, textAlign: 'center', color: c.textPrimary },
    rowLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
  });
