import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Platform, Pressable, StyleSheet, Text } from 'react-native';
import { DateTimePicker } from '@expo/ui/community/datetime-picker';

import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

// "Jump to date" control — a real, obviously-tappable app button (a Pressable
// with accessibilityRole="button" + the same visible chrome as the other
// toolbar buttons) that opens a date picker ON DEMAND.
//
// It replaces an always-visible bare native compact DateTimePicker, which read
// as a plain unstyled date chip: no border/role/affordance, so sighted users
// couldn't tell it was interactive, AND it duplicated the date the screen's
// heading already shows. A button shows no date until tapped — fixing both.
//
// Presentation differs per platform because @expo/ui's DateTimePicker does:
//  - Android: `presentation="dialog"` pops the native date dialog ON MOUNT and
//    fires onValueChange (confirm) / onDismiss (cancel); we unmount in response.
//  - iOS: `presentation` is ignored (always inline), so we host the inline
//    graphical picker inside the app's AppDialog and apply the draft on confirm.
export function JumpToDateButton({
  value,
  onSelect,
}: {
  value: Date;
  onSelect: (date: Date) => void;
}) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [open, setOpen] = useState(false);
  // iOS draft: the inline picker mutates this; Confirm applies it, Cancel discards.
  const [draft, setDraft] = useState(value);

  const label = t('mobile.jumpToDateNative');

  const openPicker = () => {
    setDraft(value);
    setOpen(true);
  };

  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={label}
        onPress={openPicker}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>{label}</Text>
      </Pressable>

      {open &&
        (Platform.OS === 'android' ? (
          <DateTimePicker
            mode="date"
            display="default"
            presentation="dialog"
            value={value}
            positiveButton={{ label: t('mobile.jumpToDateAction') }}
            negativeButton={{ label: t('mobile.cancel') }}
            onValueChange={(_, date) => {
              onSelect(date);
              setOpen(false);
            }}
            onDismiss={() => setOpen(false)}
          />
        ) : (
          <AppDialog
            visible
            title={label}
            confirmLabel={t('mobile.jumpToDateAction')}
            cancelLabel={t('mobile.cancel')}
            onConfirm={() => {
              onSelect(draft);
              setOpen(false);
            }}
            onCancel={() => setOpen(false)}
          >
            <DateTimePicker
              mode="date"
              display="inline"
              value={draft}
              locale={i18n.language}
              onValueChange={(_, date) => setDraft(date)}
            />
          </AppDialog>
        ))}
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    // Matches the calendar toolbars' ghost buttons so it reads as one of them.
    button: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    pressed: { opacity: 0.7 },
    buttonText: { fontSize: 16, fontWeight: '600', color: c.link },
  });
