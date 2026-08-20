import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import { applySignature, signatureIn, type Signature } from '@aperio/shared';

import { signatureForCalendar, useSignatures } from '../state/useSignatures';
import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

// Put a signature at the end of a description. Twin of the desktop
// SignatureButton — same shared `applySignature`, so both platforms produce
// byte-identical blocks and neither can double one the other inserted.
//
// One tap when the calendar has a bound signature; the dialog is for the
// exceptions — a different one, or taking it back out.

export function SignatureButton({
  boundTo,
  description,
  onChange,
}: {
  /** The calendar whose bound signature is offered by default. Empty means
   *  "no binding": the button always asks, which is what the task editor does,
   *  since a task belongs to a LIST and lists carry no binding yet. */
  boundTo: string;
  description: string;
  onChange: (next: string) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { signatures } = useSignatures();
  const [bound, setBound] = useState<Signature | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void signatureForCalendar(boundTo, signatures).then((s) => {
      if (!cancelled) setBound(s);
    });
    return () => {
      cancelled = true;
    };
  }, [boundTo, signatures]);

  if (signatures.length === 0) return null;

  const present = signatureIn(description) !== null;

  const insert = (sig: Signature) => {
    onChange(applySignature(description, sig.body));
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.signature.inserted', { name: sig.name }),
    );
    setOpen(false);
  };

  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={
          bound
            ? t('dialogs.signature.insertNamed', { name: bound.name })
            : t('dialogs.signature.choose')
        }
        onPress={() => {
          if (bound && !present) insert(bound);
          else setOpen(true);
        }}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>{t('dialogs.signature.short')}</Text>
      </Pressable>
      {open && (
        <AppDialog
          visible
          title={t('dialogs.signature.title')}
          cancelLabel={t('dialogs.cancel')}
          onCancel={() => setOpen(false)}
        >
          <View style={styles.choices}>
            {signatures.map((sig) => (
              <Pressable
                key={sig.id}
                accessibilityRole="button"
                accessibilityLabel={sig.name}
                onPress={() => insert(sig)}
                style={({ pressed }) => [styles.choice, pressed && styles.pressed]}
              >
                <Text style={styles.choiceText}>{sig.name}</Text>
              </Pressable>
            ))}
            {present && (
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.signature.remove')}
                onPress={() => {
                  onChange(applySignature(description, ''));
                  AccessibilityInfo.announceForAccessibility(
                    t('dialogs.signature.removed'),
                  );
                  setOpen(false);
                }}
                style={({ pressed }) => [styles.remove, pressed && styles.pressed]}
              >
                <Text style={styles.removeText}>
                  {t('dialogs.signature.remove')}
                </Text>
              </Pressable>
            )}
          </View>
        </AppDialog>
      )}
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    button: {
      marginTop: 8,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignSelf: 'flex-start',
    },
    buttonText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
    choices: { gap: 8 },
    choice: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    choiceText: { fontSize: 16, color: c.textPrimary },
    remove: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    removeText: { fontSize: 16, fontWeight: '600', color: c.danger },
  });
