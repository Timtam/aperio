import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { Signature } from '@aperio/shared';

import { useListFocusManager } from '../a11y/useListFocusManager';
import { FormScrollView } from '../components/FormScrollView';
import { refreshSignatures, saveSignatures, useSignatures } from '../state/useSignatures';
import { useThemedStyles, type ThemeColors } from '../theme';

// Signature blocks, managed on the phone. Twin of the desktop SignaturesPanel —
// same synced pref keys, same rules — and the same shape as the colour-label
// screen: the list FIRST, one screen-reader stop per signature (double-tap =
// edit; edit/delete ride the actions rotor), an inline editor for the one being
// edited, and the add form below. Every signature used to be a card of three
// stops (name, text, delete), so N signatures cost 3N swipes. Plain text only —
// see shared/signatures.ts for why an invitation cannot carry anything else.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** The first non-empty line of the text, trimmed to a spoken length. */
function summaryOf(body: string, empty: string): string {
  const firstLine = body
    .split('\n')
    .map((line) => line.trim())
    .find((line) => line.length > 0);
  if (!firstLine) return empty;
  const chars = Array.from(firstLine);
  return chars.length > 60 ? `${chars.slice(0, 60).join('')}…` : firstLine;
}

export default function SignaturesScreen() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { signatures, loading } = useSignatures();
  const [newName, setNewName] = useState('');
  const [newBody, setNewBody] = useState('');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');
  const [editBody, setEditBody] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (m: string) => AccessibilityInfo.announceForAccessibility(m),
    [],
  );

  const focus = useListFocusManager(signatures.length);
  const emptySummary = useMemo(() => t('dialogs.settings.signatures.summaryEmpty'), [t]);

  // The store hydrates once and re-reads on data reloads on its own; a screen
  // that comes back into view re-reads as well, in case a round landed while
  // it was on the stack but not focused.
  useFocusEffect(
    useCallback(() => {
      void refreshSignatures();
    }, []),
  );

  // Where the screen-reader cursor goes when the inline editor closes: back
  // to the row it came from. The editor replaces the row while open, so
  // Save / Cancel would otherwise unmount the control under the cursor and
  // strand it. Applied once the row has re-rendered (the effect keyed on
  // editingId runs after that commit).
  const returnRowIndex = useRef<number | null>(null);
  useEffect(() => {
    if (editingId != null) return;
    const index = returnRowIndex.current;
    if (index == null) return;
    returnRowIndex.current = null;
    focus.focusRow(Math.min(index, Math.max(0, signatures.length - 1)));
  }, [editingId, focus, signatures.length]);

  // The signature being edited was removed by a sync round: close the editor
  // and say why — a silently vanished editor reads as a crash.
  useEffect(() => {
    if (editingId == null || signatures.some((s) => s.id === editingId)) return;
    setEditingId(null);
    announce(t('dialogs.settings.signatures.editGone'));
  }, [announce, editingId, signatures, t]);

  const startEdit = useCallback((sig: Signature, index: number) => {
    returnRowIndex.current = index;
    setEditingId(sig.id);
    setEditName(sig.name);
    setEditBody(sig.body);
  }, []);

  const onAdd = useCallback(async () => {
    if (busy) return;
    const name = newName.trim();
    if (!name) {
      setError(t('dialogs.settings.signatures.nameRequired'));
      announce(t('dialogs.settings.signatures.nameRequired'));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      focus.onAdd();
      await saveSignatures([
        ...signatures,
        // A random id, not a counter: two devices adding one between sync
        // rounds must not mint the same one.
        { id: `sig-${Math.random().toString(36).slice(2, 10)}`, name, body: newBody },
      ]);
      setNewName('');
      setNewBody('');
      announce(t('dialogs.settings.signatures.added', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, busy, focus, newBody, newName, signatures, t]);

  const saveEdit = useCallback(async () => {
    if (busy || editingId == null) return;
    const name = editName.trim();
    if (!name) {
      setError(t('dialogs.settings.signatures.nameRequired'));
      announce(t('dialogs.settings.signatures.nameRequired'));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await saveSignatures(
        signatures.map((s) => (s.id === editingId ? { ...s, name, body: editBody } : s)),
      );
      setEditingId(null);
      announce(t('dialogs.settings.signatures.updated', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, busy, editBody, editName, editingId, signatures, t]);

  const onDelete = useCallback(
    async (sig: Signature, index: number) => {
      if (busy) return;
      setError(null);
      setBusy(true);
      try {
        focus.onRemove(index);
        await saveSignatures(signatures.filter((s) => s.id !== sig.id));
        // The calendars that pointed at it keep their binding: it simply
        // offers nothing now, and rewriting other calendars' settings to tidy
        // up would be a bigger action than the one asked for.
        announce(t('dialogs.settings.signatures.deleted', { name: sig.name }));
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    },
    [announce, busy, focus, signatures, t],
  );

  return (
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
      <Text style={styles.intro} accessibilityRole="text">
        {t('dialogs.settings.signatures.intro')}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {/* The list FIRST — it is what this screen is opened for; the add form
          is the rarer errand and reads below it. */}
      <Text style={styles.heading} accessibilityRole="header">
        {t('dialogs.settings.signatures.selectorLabel')}
      </Text>
      {loading ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.signatures.loading')}
        </Text>
      ) : signatures.length === 0 ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.signatures.empty')}
        </Text>
      ) : (
        signatures.map((sig, index) =>
          editingId === sig.id ? (
            <View key={sig.id} style={styles.card}>
              <Text style={styles.label} accessibilityRole="header">
                {t('dialogs.settings.signatures.editHeading', { name: sig.name })}
              </Text>
              <TextInput
                style={styles.input}
                value={editName}
                onChangeText={setEditName}
                accessibilityLabel={t('dialogs.settings.signatures.nameLabel')}
                editable={!busy}
                autoFocus
              />
              <TextInput
                style={[styles.input, styles.multiline]}
                value={editBody}
                onChangeText={setEditBody}
                accessibilityLabel={t('dialogs.settings.signatures.bodyLabel')}
                editable={!busy}
                multiline
                numberOfLines={4}
                textAlignVertical="top"
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
            <View key={sig.id} style={styles.row}>
              {/* ONE screen-reader stop per signature: the row is the element
                  (double-tap = edit; edit/delete ride the actions rotor). The
                  visible Edit / Delete buttons stay for sighted users and are
                  hidden from the screen reader — they duplicate the rotor verbs. */}
              <Pressable
                ref={focus.registerRow(index)}
                accessible
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.settings.signatures.optionLabel', {
                  name: sig.name,
                  summary: summaryOf(sig.body, emptySummary),
                })}
                accessibilityHint={t('dialogs.settings.signatures.rowHint')}
                accessibilityState={{ disabled: busy }}
                accessibilityActions={[
                  { name: 'edit', label: t('mobile.edit') },
                  { name: 'delete', label: t('mobile.delete') },
                ]}
                onAccessibilityAction={(e) => {
                  if (busy) return;
                  if (e.nativeEvent.actionName === 'delete') void onDelete(sig, index);
                  else startEdit(sig, index);
                }}
                // Guarded, not natively disabled: a disabled element under the
                // VoiceOver cursor strands the focus, and this row IS where the
                // cursor sits while a delete is in flight.
                onPress={() => {
                  if (!busy) startEdit(sig, index);
                }}
                style={styles.rowInfo}
              >
                <Text style={styles.rowName} importantForAccessibility="no">
                  {sig.name}
                </Text>
                <Text
                  style={styles.rowSummary}
                  importantForAccessibility="no"
                  numberOfLines={1}
                >
                  {summaryOf(sig.body, emptySummary)}
                </Text>
              </Pressable>
              <Pressable
                accessible={false}
                importantForAccessibility="no-hide-descendants"
                disabled={busy}
                onPress={() => startEdit(sig, index)}
                style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
              >
                <Text style={styles.smallButtonText}>{t('mobile.edit')}</Text>
              </Pressable>
              <Pressable
                accessible={false}
                importantForAccessibility="no-hide-descendants"
                disabled={busy}
                onPress={() => void onDelete(sig, index)}
                style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
              >
                <Text style={styles.removeButtonText}>{t('mobile.delete')}</Text>
              </Pressable>
            </View>
          ),
        )
      )}

      {/* Add a new signature — below the list, the rarer errand. */}
      <View style={styles.block}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.settings.signatures.addHeading')}
        </Text>
        <TextInput
          style={styles.input}
          value={newName}
          onChangeText={setNewName}
          accessibilityLabel={t('dialogs.settings.signatures.nameLabel')}
          placeholder={t('dialogs.settings.signatures.nameLabel')}
          editable={!busy}
        />
        <TextInput
          style={[styles.input, styles.multiline]}
          value={newBody}
          onChangeText={setNewBody}
          accessibilityLabel={t('dialogs.settings.signatures.bodyLabel')}
          placeholder={t('dialogs.settings.signatures.bodyLabel')}
          editable={!busy}
          multiline
          numberOfLines={4}
          textAlignVertical="top"
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.signatures.bodyHint')}
        </Text>
        <Pressable
          ref={focus.registerAdd}
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityLabel={t('dialogs.settings.signatures.add')}
          disabled={busy}
          onPress={() => void onAdd()}
          style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
        >
          <Text style={styles.addButtonText}>
            {t('dialogs.settings.signatures.add')}
          </Text>
        </Pressable>
      </View>
    </FormScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 12 },
    intro: { fontSize: 15, color: c.textSecondary },
    heading: { fontSize: 17, fontWeight: '700', color: c.textLabel, marginTop: 8 },
    hint: { fontSize: 13, color: c.textSecondary },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    block: { gap: 8 },
    card: {
      gap: 8,
      padding: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    row: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingVertical: 8 },
    rowInfo: { flex: 1, gap: 2 },
    rowName: { fontSize: 17, color: c.textPrimary },
    rowSummary: { fontSize: 14, color: c.textSecondary },
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
    multiline: { minHeight: 96 },
    pressed: { opacity: 0.7 },
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
    removeButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
    addButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    addButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
  });
