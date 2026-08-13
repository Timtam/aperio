import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, TextInput } from 'react-native';

import { CONTACT_LABELS, knownLabel, type KnownContactLabel } from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';
import { RadioGroup } from './RadioGroup';

// The label on one contact channel (an email address, a phone number, a
// website), as a button that opens the choice in a dialog — the same shape as
// DateTimeFieldButton.
//
// A RadioGroup inline would be the obvious thing, and it is what this dialog
// holds; but inline it would put SIX swipe stops in front of every value, and a
// contact with four numbers would take two dozen swipes to walk past. The
// button carries the current label in its own accessibility label, so the
// choice is audible without opening anything.
//
// "Custom" is not decoration: CardDAV and Google both store whatever word the
// user typed, so a number that arrives labelled "Ferienhaus" has to be editable
// without that word silently collapsing to "other" on save.

const CUSTOM = '__custom__';
const NONE = '__none__';

/** The word shown for a stored label — the translated one when Aperio knows
 *  it, the user's own otherwise, and a placeholder when there is none. */
function contactLabelText(
  label: string | null,
  t: (key: string) => string,
): string {
  const known = knownLabel(label);
  if (known) return t(`dialogs.contact.channelLabel.${known}`);
  const custom = label?.trim();
  return custom ? custom : t('dialogs.contact.channelLabelNone');
}

export function ContactLabelButton({
  label,
  fieldLabel,
  onChange,
  disabled = false,
}: {
  /** The stored label, or null for an unlabelled channel. */
  label: string | null;
  /** What this label belongs to, e.g. "Phone number 2" — folded into the
   *  button's accessibility label so the row identifies itself. */
  fieldLabel: string;
  onChange: (next: string | null) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [open, setOpen] = useState(false);
  const known = knownLabel(label);
  const isCustom = label !== null && known === null;
  // Draft state: the dialog applies on Confirm, so cancelling leaves the
  // channel exactly as it was.
  const [choice, setChoice] = useState<string>(NONE);
  const [customText, setCustomText] = useState('');

  const display = contactLabelText(label, t);

  const options = [
    { value: NONE, label: t('dialogs.contact.channelLabelNone') },
    ...CONTACT_LABELS.map((l: KnownContactLabel) => ({
      value: l as string,
      label: t(`dialogs.contact.channelLabel.${l}`),
    })),
    { value: CUSTOM, label: t('dialogs.contact.channelLabelCustom') },
  ];

  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${fieldLabel}, ${t('dialogs.contact.channelLabelField')}: ${display}`}
        accessibilityState={{ disabled }}
        disabled={disabled}
        onPress={() => {
          setChoice(isCustom ? CUSTOM : (known ?? NONE));
          setCustomText(isCustom ? (label ?? '') : '');
          setOpen(true);
        }}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>{display}</Text>
      </Pressable>

      {open && (
        <AppDialog
          visible
          title={t('dialogs.contact.channelLabelField')}
          confirmLabel={t('mobile.applyAction')}
          cancelLabel={t('mobile.cancel')}
          onConfirm={() => {
            if (choice === NONE) onChange(null);
            else if (choice === CUSTOM) onChange(customText.trim() || null);
            else onChange(choice);
            setOpen(false);
          }}
          onCancel={() => setOpen(false)}
        >
          <RadioGroup
            label={t('dialogs.contact.channelLabelField')}
            value={choice}
            options={options}
            onChange={setChoice}
          />
          {choice === CUSTOM && (
            <TextInput
              style={styles.input}
              value={customText}
              onChangeText={setCustomText}
              accessibilityLabel={t('dialogs.contact.channelLabelCustomValue')}
              autoCapitalize="none"
              autoCorrect={false}
            />
          )}
        </AppDialog>
      )}
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    // Same ghost-button chrome as DateTimeFieldButton, so a tappable field
    // value reads consistently across the editors.
    button: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    pressed: { opacity: 0.7 },
    buttonText: { fontSize: 16, fontWeight: '600', color: c.link },
    input: {
      marginTop: 12,
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
      color: c.textPrimary,
      fontSize: 16,
    },
  });
