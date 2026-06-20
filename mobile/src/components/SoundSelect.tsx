import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';
import { useAudioPlayer } from 'expo-audio';

import type { SoundConfig } from '@aperio/shared';

import type { CustomSound } from '../api/sounds';
import { useCustomSounds } from '../state/useCustomSounds';
import { RadioGroup } from './RadioGroup';
import { useThemedStyles, type ThemeColors } from '../theme';

// Accessible notification-sound picker (DESIGN §14.4) — the mobile twin of the
// desktop SoundPicker. Offers System / Silent / (for a container) "Use default"
// = inherit, PLUS each imported custom sound as a pickable option, an
// "Import file …" action, and Preview / Remove for the selected custom sound.
// Custom playback in the actual OS notification is Android-only (a per-sound
// channel); iOS previews here but the notification falls back to the system
// sound (a build-time-bundle limitation). The caller (useSoundPref / per-item
// state) owns the pref load/save; this owns the sound library via
// useCustomSounds. RadioGroup keeps every option a screen-reader focus stop.

// OS notifications play at the system volume, so per-config volume is N/A on
// mobile; this keeps the wire SoundConfig well-formed (matches cal_core
// SoundConfig::default().volume) and round-trips a desktop-set volume untouched.
const DEFAULT_VOLUME = 80;

const CUSTOM_PREFIX = 'custom:';

/** The RadioGroup value for the current SoundConfig: 'inherit' (null + a
 *  container picker), 'system', 'silent', or `custom:<sha256>`. */
function toChoice(value: SoundConfig | null, allowInherit: boolean): string {
  if (value == null) return allowInherit ? 'inherit' : 'system';
  if (value.source.type === 'custom') return `${CUSTOM_PREFIX}${value.source.sha256}`;
  return value.source.type;
}

export function SoundSelect({
  label,
  value,
  allowInherit,
  onChange,
  disabled,
}: {
  label: string;
  value: SoundConfig | null;
  /** Container pickers offer "Use default" (inherit → clear the key); the global
   *  root does not (it IS the default, falling through to System). */
  allowInherit: boolean;
  onChange: (next: SoundConfig | null) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { sounds, importFromPicker, remove } = useCustomSounds();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const volume = value?.volume ?? DEFAULT_VOLUME;
  const choice = toChoice(value, allowInherit);
  const selectedSha = choice.startsWith(CUSTOM_PREFIX)
    ? choice.slice(CUSTOM_PREFIX.length)
    : null;
  const selectedSound = selectedSha
    ? sounds.find((s) => s.sha256 === selectedSha)
    : undefined;

  // One preview player tied to the selected custom sound's path (null when no
  // custom is selected). The hook reloads the source as the selection changes
  // and auto-releases on unmount.
  const player = useAudioPlayer(selectedSound?.path ?? null);

  const customLabel = (s: CustomSound): string =>
    t('reminders.sound.customItem', {
      ext: s.ext.toUpperCase(),
      short: s.sha256.slice(0, 8),
    });

  const options: { value: string; label: string }[] = [];
  if (allowInherit) options.push({ value: 'inherit', label: t('reminders.sound.inherit') });
  options.push({ value: 'system', label: t('reminders.sound.system') });
  options.push({ value: 'silent', label: t('reminders.sound.silent') });
  for (const s of sounds) {
    options.push({ value: `${CUSTOM_PREFIX}${s.sha256}`, label: customLabel(s) });
  }
  // A selected custom sound whose bytes aren't in the local library yet (synced
  // reference not fetched, or just deleted) still needs a radio option to show.
  if (selectedSha && selectedSound == null) {
    options.push({ value: choice, label: t('reminders.sound.custom') });
  }

  const handle = (next: string) => {
    if (next === 'inherit') onChange(null);
    else if (next === 'system') onChange({ source: { type: 'system' }, volume });
    else if (next === 'silent') onChange({ source: { type: 'silent' }, volume });
    else if (next.startsWith(CUSTOM_PREFIX)) {
      onChange({ source: { type: 'custom', sha256: next.slice(CUSTOM_PREFIX.length) }, volume });
    }
  };

  const doImport = useCallback(async () => {
    setError(null);
    setBusy(true);
    try {
      const imported = await importFromPicker();
      if (imported != null) {
        // Auto-select the freshly imported sound (a programmatic select isn't
        // announced by the RadioGroup, so announce it here).
        onChange({ source: { type: 'custom', sha256: imported.sha256 }, volume });
        AccessibilityInfo.announceForAccessibility(
          t('mobile.added', {
            title: t('reminders.sound.customItem', {
              ext: imported.ext.toUpperCase(),
              short: imported.sha256.slice(0, 8),
            }),
          }),
        );
      }
    } catch (e) {
      // Surface the importer's specific reason (unsupported format / too large)
      // when present — the same way the other mobile error surfaces do — falling
      // back to the generic message only when there's none.
      const msg = e instanceof Error && e.message ? e.message : t('reminders.sound.importError');
      setError(msg);
      AccessibilityInfo.announceForAccessibility(msg);
    } finally {
      setBusy(false);
    }
  }, [importFromPicker, onChange, t, volume]);

  const preview = () => {
    // Restart from the top so a re-tap always re-plays (seekTo is async; play
    // regardless if it rejects — e.g. a not-yet-loaded source).
    void player
      .seekTo(0)
      .then(() => player.play())
      .catch(() => player.play());
  };

  const removeSelected = useCallback(async () => {
    if (selectedSound == null) return;
    // Label inlined (not via the render-scoped customLabel helper) so the
    // callback's deps stay stable.
    const name = t('reminders.sound.customItem', {
      ext: selectedSound.ext.toUpperCase(),
      short: selectedSound.sha256.slice(0, 8),
    });
    setBusy(true);
    try {
      await remove(selectedSound.sha256);
      // The selection just disappeared → fall back to inherit (container) or
      // System (global root).
      onChange(allowInherit ? null : { source: { type: 'system' }, volume });
      AccessibilityInfo.announceForAccessibility(t('mobile.deleted', { title: name }));
    } catch {
      setError(t('reminders.sound.importError'));
    } finally {
      setBusy(false);
    }
  }, [allowInherit, onChange, remove, selectedSound, t, volume]);

  return (
    <View style={styles.field}>
      <RadioGroup<string>
        label={label}
        value={choice}
        options={options}
        onChange={handle}
        disabled={disabled || busy}
      />

      {selectedSound != null && (
        <View style={styles.actions}>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`${t('reminders.sound.test')}: ${customLabel(selectedSound)}`}
            disabled={disabled || busy}
            onPress={preview}
            style={({ pressed }) => [styles.actionButton, pressed && styles.pressed]}
          >
            <Text style={styles.actionText}>{t('reminders.sound.test')}</Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`${t('reminders.sound.remove')}: ${customLabel(selectedSound)}`}
            accessibilityState={{ disabled: disabled || busy }}
            disabled={disabled || busy}
            onPress={() => void removeSelected()}
            style={({ pressed }) => [styles.removeButton, pressed && styles.pressed]}
          >
            <Text style={styles.removeText}>{t('reminders.sound.remove')}</Text>
          </Pressable>
        </View>
      )}

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('reminders.sound.import')}
        accessibilityState={{ disabled: disabled || busy }}
        disabled={disabled || busy}
        onPress={() => void doImport()}
        style={({ pressed }) => [styles.importButton, pressed && styles.pressed]}
      >
        <Text style={styles.importText}>{t('reminders.sound.import')}</Text>
      </Pressable>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    field: { gap: 8 },
    actions: { flexDirection: 'row', gap: 10 },
    actionButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    actionText: { fontSize: 15, fontWeight: '600', color: c.accent },
    removeButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    removeText: { fontSize: 15, fontWeight: '600', color: c.danger },
    importButton: {
      alignSelf: 'flex-start',
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    importText: { fontSize: 15, fontWeight: '600', color: c.link },
    error: { fontSize: 14, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
  });
