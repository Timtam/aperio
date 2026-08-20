import { useCallback, useState } from 'react';
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

import { FormScrollView } from '../components/FormScrollView';
import { saveSignatures, useSignatures } from '../state/useSignatures';
import { useThemedStyles, type ThemeColors } from '../theme';

// Signature blocks, managed on the phone. Twin of the desktop SignaturesPanel —
// same synced pref keys, same rules.
//
// Every signature is one card carrying its own fields, written on BLUR: no Save
// to hunt for and nothing lost by backing out, the same idiom the day-marker
// screen uses. Plain text only — see shared/signatures.ts for why an invitation
// cannot carry anything else.

export default function SignaturesScreen() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { signatures, loading } = useSignatures();
  const [newName, setNewName] = useState('');
  const [newBody, setNewBody] = useState('');
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (m: string) => AccessibilityInfo.announceForAccessibility(m),
    [],
  );

  const onAdd = useCallback(async () => {
    const name = newName.trim();
    if (!name) {
      setError(t('dialogs.settings.signatures.nameRequired'));
      announce(t('dialogs.settings.signatures.nameRequired'));
      return;
    }
    try {
      await saveSignatures([
        ...signatures,
        // A random id, not a counter: two devices adding one between sync
        // rounds must not mint the same one.
        { id: `sig-${Math.random().toString(36).slice(2, 10)}`, name, body: newBody },
      ]);
      setNewName('');
      setNewBody('');
      setError(null);
      announce(t('dialogs.settings.signatures.added', { name }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [newName, newBody, signatures, announce, t]);

  const onChange = useCallback(
    async (id: string, patch: Partial<Signature>) => {
      try {
        await saveSignatures(
          signatures.map((s) => (s.id === id ? { ...s, ...patch } : s)),
        );
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [signatures],
  );

  const onDelete = useCallback(
    async (sig: Signature) => {
      try {
        await saveSignatures(signatures.filter((s) => s.id !== sig.id));
        // The calendars that pointed at it keep their binding: it simply
        // offers nothing now, and rewriting other calendars' settings to tidy
        // up would be a bigger action than the one asked for.
        announce(t('dialogs.settings.signatures.deleted', { name: sig.name }));
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [signatures, announce, t],
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

      {loading ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.signatures.loading')}
        </Text>
      ) : signatures.length === 0 ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.signatures.empty')}
        </Text>
      ) : (
        signatures.map((sig) => (
          <SignatureCard
            key={sig.id}
            signature={sig}
            styles={styles}
            onCommit={onChange}
            onDelete={() => void onDelete(sig)}
          />
        ))
      )}

      <View style={styles.block}>
        <Text style={styles.label} accessibilityRole="header">
          {t('dialogs.settings.signatures.addHeading')}
        </Text>
        <TextInput
          style={styles.input}
          value={newName}
          onChangeText={setNewName}
          accessibilityLabel={t('dialogs.settings.signatures.nameLabel')}
          placeholder={t('dialogs.settings.signatures.nameLabel')}
        />
        <TextInput
          style={[styles.input, styles.multiline]}
          value={newBody}
          onChangeText={setNewBody}
          accessibilityLabel={t('dialogs.settings.signatures.bodyLabel')}
          placeholder={t('dialogs.settings.signatures.bodyLabel')}
          multiline
          numberOfLines={4}
          textAlignVertical="top"
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.signatures.bodyHint')}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.signatures.add')}
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

/** One signature: name and text, edited in place, written on blur. */
function SignatureCard({
  signature,
  styles,
  onCommit,
  onDelete,
}: {
  signature: Signature;
  styles: ReturnType<typeof makeStyles>;
  onCommit: (id: string, patch: Partial<Signature>) => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(signature.name);
  const [body, setBody] = useState(signature.body);

  return (
    <View style={styles.card}>
      <TextInput
        style={styles.input}
        value={name}
        onChangeText={setName}
        onBlur={() => {
          if (name.trim() && name !== signature.name) {
            onCommit(signature.id, { name: name.trim() });
          }
        }}
        accessibilityLabel={`${t('dialogs.settings.signatures.nameLabel')}, ${signature.name}`}
      />
      <TextInput
        style={[styles.input, styles.multiline]}
        value={body}
        onChangeText={setBody}
        onBlur={() => {
          if (body !== signature.body) onCommit(signature.id, { body });
        }}
        accessibilityLabel={`${t('dialogs.settings.signatures.bodyLabel')}, ${signature.name}`}
        multiline
        numberOfLines={4}
        textAlignVertical="top"
      />
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${t('dialogs.settings.signatures.delete')}, ${signature.name}`}
        onPress={onDelete}
        style={({ pressed }) => [styles.removeButton, pressed && styles.pressed]}
      >
        <Text style={styles.removeButtonText}>
          {t('dialogs.settings.signatures.delete')}
        </Text>
      </Pressable>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 16 },
    intro: { fontSize: 15, color: c.textSecondary },
    hint: { fontSize: 13, color: c.textSecondary },
    error: { fontSize: 15, color: c.danger },
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
    addButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    addButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    removeButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignSelf: 'flex-start',
    },
    removeButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
