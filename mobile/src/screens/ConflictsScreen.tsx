import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import {
  listSyncConflicts,
  resolveSyncConflict,
  SyncConflict,
  SyncResolutionChoice,
} from '../api/sync';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// Sync-conflict resolution — a faithful RN port of the desktop SyncConflictsDialog
// (DESIGN §19.3). A field-level conflict (a field edited differently on two
// devices) is shown with both values; the user picks Keep-mine / Take-other /
// Save-both. Screen-reader-first: each card is NOT a single accessible node (so
// its three buttons stay separate), and every button's label carries the row
// context (kind + field) so a blind user knows which conflict they're resolving.
// No Tauri event bus on mobile → re-fetch after each resolve. Overrides are
// host-local; resolving take_remote logs a SyncEvent that pushes next round.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Conflict values are JSON-encoded scalars; decode for display, null → em-dash. */
function decodeForDisplay(raw: string | null): string {
  if (raw == null) return '—';
  try {
    const v = JSON.parse(raw);
    return v == null ? '—' : String(v);
  } catch {
    return raw;
  }
}

export default function ConflictsScreen({ navigation }: RootStackScreenProps<'Conflicts'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [conflicts, setConflicts] = useState<SyncConflict[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const load = useCallback(async () => {
    try {
      setConflicts(await listSyncConflicts());
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, []);

  // Load on mount + whenever the screen regains focus (a sync round may have
  // recorded more while we were away).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  const resolve = useCallback(
    async (conflict: SyncConflict, choice: SyncResolutionChoice) => {
      if (busy) return;
      setBusy(true);
      setError(null);
      try {
        await resolveSyncConflict(conflict.id, choice);
        announce(t('dialogs.syncConflicts.resolved'));
        await load();
      } catch (err) {
        // save_both is intentionally unsupported — surface it assertively.
        const message = errorMessage(err);
        setError(message);
        announce(
          choice === 'save_both'
            ? t('dialogs.syncConflicts.actionSaveBothUnsupported')
            : t('mobile.error', { message }),
        );
      } finally {
        setBusy(false);
      }
    },
    [announce, busy, load, t],
  );

  const fmtTime = useCallback(
    (iso: string) => new Date(iso).toLocaleString(i18n.language),
    [i18n.language],
  );

  return (
    <View style={styles.screen}>
      <Text
        style={styles.intro}
        accessibilityRole="text"
        accessibilityLiveRegion="polite"
      >
        {loading
          ? t('mobile.loading')
          : conflicts.length === 0
            ? t('dialogs.syncConflicts.empty')
            : t('dialogs.syncConflicts.intro', { count: conflicts.length })}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      <ScrollView
        accessibilityRole="list"
        accessibilityLabel={t('dialogs.syncConflicts.listLabel')}
        contentContainerStyle={styles.list}
        keyboardShouldPersistTaps="handled"
      >
        {conflicts.map((c) => {
          const kind = t(`dialogs.syncConflicts.rowKind.${c.row_kind}`);
          const ariaContext = t('dialogs.syncConflicts.actionAriaContext', {
            kind,
            field: c.field,
          });
          return (
            <View key={c.id} style={styles.card}>
              <Text style={styles.cardField} accessibilityRole="text">
                {kind} — {t('dialogs.syncConflicts.fieldLabel')}: {c.field}
              </Text>
              <Text style={styles.cardSource} accessibilityRole="text">
                {t('dialogs.syncConflicts.remoteSourceLabel', {
                  time: fmtTime(c.remote_timestamp),
                  device: c.remote_device_id,
                })}
              </Text>
              <Text style={styles.cardValue} accessibilityRole="text">
                {t('dialogs.syncConflicts.localValueLabel')}: {decodeForDisplay(c.local_value)}
              </Text>
              <Text style={styles.cardValue} accessibilityRole="text">
                {t('dialogs.syncConflicts.remoteValueLabel')}: {decodeForDisplay(c.remote_value)}
              </Text>
              <View style={styles.actions}>
                <Pressable
                  accessibilityRole="button"
                  accessibilityState={{ disabled: busy }}
                  accessibilityLabel={`${t('dialogs.syncConflicts.actionKeepLocal')} — ${ariaContext}`}
                  disabled={busy}
                  onPress={() => void resolve(c, 'keep_local')}
                  style={({ pressed }) => [styles.actionBtn, pressed && styles.pressed]}
                >
                  <Text style={styles.actionText}>
                    {t('dialogs.syncConflicts.actionKeepLocal')}
                  </Text>
                </Pressable>
                <Pressable
                  accessibilityRole="button"
                  accessibilityState={{ disabled: busy }}
                  accessibilityLabel={`${t('dialogs.syncConflicts.actionTakeRemote')} — ${ariaContext}`}
                  disabled={busy}
                  onPress={() => void resolve(c, 'take_remote')}
                  style={({ pressed }) => [styles.actionBtn, pressed && styles.pressed]}
                >
                  <Text style={styles.actionText}>
                    {t('dialogs.syncConflicts.actionTakeRemote')}
                  </Text>
                </Pressable>
                <Pressable
                  accessibilityRole="button"
                  accessibilityState={{ disabled: busy }}
                  accessibilityLabel={`${t('dialogs.syncConflicts.actionSaveBoth')} — ${ariaContext}`}
                  disabled={busy}
                  onPress={() => void resolve(c, 'save_both')}
                  style={({ pressed }) => [styles.actionBtn, pressed && styles.pressed]}
                >
                  <Text style={styles.actionText}>
                    {t('dialogs.syncConflicts.actionSaveBoth')}
                  </Text>
                </Pressable>
              </View>
            </View>
          );
        })}
      </ScrollView>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    intro: { fontSize: 15, color: c.textLabel, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
    list: { gap: 12, padding: 16 },
    card: {
      gap: 4,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    cardField: { fontSize: 16, fontWeight: '700', color: c.textPrimary },
    cardSource: { fontSize: 13, color: c.textSecondary },
    cardValue: { fontSize: 15, color: c.textPrimary },
    actions: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, marginTop: 8 },
    actionBtn: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
    actionText: { fontSize: 15, fontWeight: '600', color: c.accent },
    pressed: { opacity: 0.7 },
  });
