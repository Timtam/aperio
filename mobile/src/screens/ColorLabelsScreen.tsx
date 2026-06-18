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

// Manage the app-wide colour-label palette (§8). Colour labels are named hex
// colours any event / task / calendar / list can bind to; they live in local
// SQLite and sync across devices. Screen-reader-first: the colour is an
// accessible hex TEXT field (a visual swatch would be useless to a blind user —
// it's rendered only as decoration); each label is its own row with rename/
// recolour (inline) + delete; add/remove move SR focus via useListFocusManager.
// Ad-hoc one-off colours are hidden here (they're composed inline elsewhere).

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

      {/* Add a new named label */}
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
      <TextInput
        style={styles.input}
        value={newHex}
        onChangeText={setNewHex}
        accessibilityLabel={t('dialogs.colorLabels.fields.color')}
        placeholder="#RRGGBB"
        editable={!busy}
        autoCapitalize="none"
        autoCorrect={false}
      />
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

      {/* Existing labels */}
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
              <TextInput
                style={styles.input}
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
              <Text
                ref={focus.registerRow(index)}
                style={styles.labelName}
                accessibilityRole="text"
                accessibilityLabel={t('dialogs.colorLabels.optionLabel', {
                  name: label.name,
                  hex: label.hex,
                })}
              >
                {/* Decorative swatch (aria-hidden via the row's own label) + name. */}
                <Text style={[styles.swatch, { color: label.hex }]}>{'■ '}</Text>
                {label.name}
              </Text>
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={t('dialogs.colorLabels.renameLabel', {
                  name: label.name,
                })}
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
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={t('dialogs.colorLabels.deleteLabel', {
                  name: label.name,
                })}
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
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 16, gap: 12 },
  heading: { fontSize: 17, fontWeight: '700', color: '#2b3240', marginTop: 8 },
  label: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318' },
  muted: { fontSize: 15, color: '#5b6573' },
  field: { gap: 6 },
  input: {
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  addButton: {
    paddingVertical: 12,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  addButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  labelRow: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingVertical: 8 },
  labelName: { flex: 1, fontSize: 17, color: '#10131a' },
  swatch: { fontSize: 17 },
  rowButtons: { flexDirection: 'row', gap: 10 },
  smallButton: {
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  smallButtonText: { fontSize: 15, fontWeight: '600', color: '#1d4ed8' },
  pressed: { opacity: 0.7 },
});
