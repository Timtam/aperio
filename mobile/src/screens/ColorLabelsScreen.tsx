import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { ColorLabel } from '@aperio/shared';

import { useListFocusManager } from '../a11y/useListFocusManager';
import {
  createColorLabel,
  deleteColorLabel,
  listColorLabels,
  updateColorLabel,
} from '../api/colorLabels';
import { useThemedStyles, type ThemeColors } from '../theme';

// Manage the app-wide colour-label palette (§8). Colour labels are named hex
// colours any event / task / calendar / list can bind to; they live in local
// SQLite and sync across devices. Serves both audiences: sighted users see a
// real colour SWATCH (a coloured View) next to the name and a live preview of
// the entered hex; screen-reader users get the name + hex via the row's
// accessibilityLabel and edit the colour as a hex TEXT field. Each label is its
// own row with rename/recolour (inline) + delete; add/remove move SR focus via
// useListFocusManager. Ad-hoc one-off colours are hidden here (composed inline).

/** `#rrggbb` (case-insensitive). */
function normaliseHex(input: string): string | null {
  const raw = input.trim();
  const withHash = raw.startsWith('#') ? raw : `#${raw}`;
  return /^#[0-9a-fA-F]{6}$/.test(withHash) ? withHash.toLowerCase() : null;
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function ColorLabelsScreen() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [labels, setLabels] = useState<ColorLabel[]>([]);
  const [newName, setNewName] = useState('');
  const [newHex, setNewHex] = useState('');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');
  const [editHex, setEditHex] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const load = useCallback(async () => {
    try {
      // Hide ad-hoc one-off colours — they're composed inline, not managed here.
      setLabels((await listColorLabels()).filter((l) => !l.ad_hoc));
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const focus = useListFocusManager(labels.length);

  const addLabel = useCallback(async () => {
    if (busy) return;
    const name = newName.trim();
    if (name.length === 0) {
      setError(t('dialogs.colorLabels.nameRequired'));
      announce(t('dialogs.colorLabels.nameRequired'));
      return;
    }
    const hex = normaliseHex(newHex);
    if (hex == null) {
      setError(t('mobile.colorHexInvalid'));
      announce(t('mobile.colorHexInvalid'));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      focus.onAdd();
      await createColorLabel(name, hex);
      setNewName('');
      setNewHex('');
      await load();
      announce(t('dialogs.colorLabels.created', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, busy, focus, load, newHex, newName, t]);

  const saveEdit = useCallback(async () => {
    if (busy || editingId == null) return;
    const name = editName.trim();
    if (name.length === 0) {
      setError(t('dialogs.colorLabels.nameRequired'));
      announce(t('dialogs.colorLabels.nameRequired'));
      return;
    }
    const hex = normaliseHex(editHex);
    if (hex == null) {
      setError(t('mobile.colorHexInvalid'));
      announce(t('mobile.colorHexInvalid'));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await updateColorLabel({ id: editingId, name, hex, ad_hoc: false });
      setEditingId(null);
      await load();
      announce(t('dialogs.colorLabels.updated', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, busy, editHex, editName, editingId, load, t]);

  const removeLabel = useCallback(
    async (label: ColorLabel, index: number) => {
      if (busy) return;
      setError(null);
      setBusy(true);
      try {
        focus.onRemove(index);
        await deleteColorLabel(label.id);
        await load();
        announce(t('dialogs.colorLabels.deleted', { name: label.name }));
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    },
    [announce, busy, focus, load, t],
  );

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      {error != null && (
        <Text
          style={styles.error}
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
        >
          {error}
        </Text>
      )}

      {/* Existing labels FIRST — the list is what this screen is opened for;
          the create form is the rarer errand and reads below it. */}
      <Text style={styles.heading} accessibilityRole="header">
        {t('dialogs.colorLabels.existingHeading', { count: labels.length })}
      </Text>
      {labels.length === 0 ? (
        <Text style={styles.muted} accessibilityRole="text">
          {t('dialogs.colorLabels.emptyHint')}
        </Text>
      ) : (
        labels.map((label, index) =>
          editingId === label.id ? (
            <View key={label.id} style={styles.field}>
              <Text style={styles.label}>
                {t('dialogs.colorLabels.renameLabel', { name: label.name })}
              </Text>
              <TextInput
                style={styles.input}
                value={editName}
                onChangeText={setEditName}
                accessibilityLabel={t('dialogs.colorLabels.renameLabel', {
                  name: label.name,
                })}
                editable={!busy}
                autoFocus
              />
              <Text style={styles.label}>
                {t('dialogs.colorLabels.colorLabel', { name: label.name })}
              </Text>
              <View style={styles.hexRow}>
                <TextInput
                  style={[styles.input, styles.hexInput]}
                  value={editHex}
                  onChangeText={setEditHex}
                  accessibilityLabel={t('dialogs.colorLabels.colorLabel', {
                    name: label.name,
                  })}
                  placeholder="#RRGGBB"
                  editable={!busy}
                  autoCapitalize="none"
                  autoCorrect={false}
                />
                {normaliseHex(editHex) != null && (
                  <View
                    accessible={false}
                    style={[styles.swatch, { backgroundColor: normaliseHex(editHex) as string }]}
                  />
                )}
              </View>
              <View style={styles.rowButtons}>
                <Pressable
                  accessibilityRole="button"
                  accessibilityState={{ disabled: busy }}
                  accessibilityLabel={t('mobile.save')}
                  disabled={busy}
                  onPress={() => void saveEdit()}
                  style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                >
                  <Text style={styles.smallButtonText}>{t('mobile.save')}</Text>
                </Pressable>
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={t('mobile.cancel')}
                  onPress={() => setEditingId(null)}
                  style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                >
                  <Text style={styles.smallButtonText}>{t('mobile.cancel')}</Text>
                </Pressable>
              </View>
            </View>
          ) : (
            <View key={label.id} style={styles.labelRow}>
              {/* ONE screen-reader stop per label: the row itself is the
                  element (double-tap = edit; rename/delete ride the actions
                  rotor), so N labels cost N swipes, not 3N. The visible Edit /
                  Delete buttons stay for sighted users and are hidden from the
                  screen reader — they duplicate the rotor verbs exactly. */}
              <Pressable
                ref={focus.registerRow(index)}
                accessible
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.colorLabels.optionLabel', {
                  name: label.name,
                  hex: label.hex,
                })}
                accessibilityHint={t('dialogs.colorLabels.rowHint')}
                accessibilityState={{ disabled: busy }}
                accessibilityActions={[
                  { name: 'edit', label: t('mobile.edit') },
                  {
                    name: 'delete',
                    label: t('dialogs.colorLabels.deleteAction'),
                  },
                ]}
                onAccessibilityAction={(e) => {
                  if (busy) return;
                  if (e.nativeEvent.actionName === 'delete') {
                    void removeLabel(label, index);
                  } else {
                    setEditingId(label.id);
                    setEditName(label.name);
                    setEditHex(label.hex);
                  }
                }}
                // Guarded, not natively disabled: a disabled element under the
                // VoiceOver cursor strands the focus, and this row IS where the
                // cursor sits while a delete is in flight.
                onPress={() => {
                  if (busy) return;
                  setEditingId(label.id);
                  setEditName(label.name);
                  setEditHex(label.hex);
                }}
                style={styles.labelInfo}
              >
                {/* Real colour swatch + the exact hex (monospace) for sighted
                    users — the desktop palette manager shows the hex too, so two
                    similar colours are tellable apart without entering Edit. The
                    name + hex ride the row's accessibilityLabel for SR users, so
                    these stay importantForAccessibility="no". */}
                <View
                  style={[styles.swatch, { backgroundColor: label.hex }]}
                  accessible={false}
                />
                <Text style={styles.labelName} importantForAccessibility="no">
                  {label.name}
                </Text>
                <Text style={styles.hexValue} importantForAccessibility="no">
                  {label.hex}
                </Text>
              </Pressable>
              <Pressable
                accessible={false}
                importantForAccessibility="no-hide-descendants"
                disabled={busy}
                onPress={() => {
                  setEditingId(label.id);
                  setEditName(label.name);
                  setEditHex(label.hex);
                }}
                style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
              >
                <Text style={styles.smallButtonText}>{t('mobile.edit')}</Text>
              </Pressable>
              <Pressable
                accessible={false}
                importantForAccessibility="no-hide-descendants"
                disabled={busy}
                onPress={() => void removeLabel(label, index)}
                style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
              >
                <Text style={styles.smallButtonText}>
                  {t('dialogs.colorLabels.deleteAction')}
                </Text>
              </Pressable>
            </View>
          ),
        )
      )}

      {/* Add a new named label — below the list, the rarer errand. */}
      <Text style={styles.heading} accessibilityRole="header">
        {t('dialogs.colorLabels.newHeading')}
      </Text>
      <Text style={styles.label}>{t('dialogs.colorLabels.fields.name')}</Text>
      <TextInput
        style={styles.input}
        value={newName}
        onChangeText={setNewName}
        accessibilityLabel={t('dialogs.colorLabels.fields.name')}
        editable={!busy}
        autoCapitalize="sentences"
      />
      <Text style={styles.label}>{t('dialogs.colorLabels.fields.color')}</Text>
      <View style={styles.hexRow}>
        <TextInput
          style={[styles.input, styles.hexInput]}
          value={newHex}
          onChangeText={setNewHex}
          accessibilityLabel={t('dialogs.colorLabels.fields.color')}
          placeholder="#RRGGBB"
          editable={!busy}
          autoCapitalize="none"
          autoCorrect={false}
        />
        {/* Live preview of the entered colour (sighted users); the swatch is
            decorative so it carries no screen-reader label. */}
        {normaliseHex(newHex) != null && (
          <View
            accessible={false}
            style={[styles.swatch, { backgroundColor: normaliseHex(newHex) as string }]}
          />
        )}
      </View>
      <Pressable
        ref={focus.registerAdd}
        accessibilityRole="button"
        accessibilityState={{ disabled: busy }}
        accessibilityLabel={t('dialogs.colorLabels.create')}
        disabled={busy}
        onPress={() => void addLabel()}
        style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
      >
        <Text style={styles.addButtonText}>{t('dialogs.colorLabels.create')}</Text>
      </Pressable>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 12 },
    heading: { fontSize: 17, fontWeight: '700', color: c.textLabel, marginTop: 8 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
    muted: { fontSize: 15, color: c.textSecondary },
    field: { gap: 6 },
    hexRow: { flexDirection: 'row', alignItems: 'center', gap: 10 },
    hexInput: { flex: 1 },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    addButton: {
      paddingVertical: 12,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    addButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    labelRow: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingVertical: 8 },
    labelInfo: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 10 },
    labelName: { flex: 1, fontSize: 17, color: c.textPrimary },
    // The exact stored hex, monospace + muted (matches the SyncScreen fingerprint
    // convention) so a sighted user can read off / compare colours at a glance.
    hexValue: { fontSize: 14, color: c.textSecondary, fontFamily: 'monospace' },
    // A real colour swatch (a coloured box) for sighted users. The subtle border
    // keeps white / very light swatches visible on the white background.
    swatch: {
      width: 22,
      height: 22,
      borderRadius: 5,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    rowButtons: { flexDirection: 'row', gap: 10 },
    smallButton: {
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    smallButtonText: { fontSize: 15, fontWeight: '600', color: c.accent },
    pressed: { opacity: 0.7 },
  });
