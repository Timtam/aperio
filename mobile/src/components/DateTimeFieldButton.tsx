import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Platform, Pressable, StyleSheet, Text } from 'react-native';
import { DateTimePicker } from '@expo/ui/community/datetime-picker';

import {
  formatLocalDate,
  formatLocalTime,
  parseLocalDate,
  parseLocalTime,
} from '../intl/dateTimeField';
import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

// An accessible date/time FIELD for the editors: a real `role=button` element
// whose label carries the field name AND the current value, opening the native
// picker ON DEMAND. It replaces the always-inline compact @expo/ui
// DateTimePicker in the editor forms, which VoiceOver could not reach by
// swiping (the SwiftUI-hosted chip never joined the linear swipe order) — the
// same reason the calendar's jump-to-date became JumpToDateButton, whose
// picker-in-a-dialog flow Toni verified working with VoiceOver on-device.
//
// Presentation per platform (mirrors JumpToDateButton):
//  - Android: `presentation="dialog"` pops the native date/time dialog ON
//    MOUNT and fires onValueChange (confirm) / onDismiss (cancel); we unmount
//    in response.
//  - iOS: `presentation` is ignored (always inline), so the inline picker is
//    hosted inside the focus-trapping AppDialog — a graphical calendar for
//    dates, wheels for times — and the draft applies on Confirm only.
//
// The form state stays string-based ('YYYY-MM-DD' / 'HH:MM', strictly local —
// see intl/dateTimeField), so callers' save/validation paths are unchanged.
export function DateTimeFieldButton({
  label,
  mode,
  value,
  onChange,
  disabled = false,
}: {
  /** Full field label, e.g. "Scheduled – day". Folded into the button's
   *  accessibility label together with the current value. */
  label: string;
  mode: 'date' | 'time';
  /** 'YYYY-MM-DD' (date) or 'HH:MM' (time); empty falls back to today/now. */
  value: string;
  onChange: (next: string) => void;
  disabled?: boolean;
}) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [open, setOpen] = useState(false);
  // iOS draft: the inline picker mutates this; Confirm applies, Cancel discards.
  const [draft, setDraft] = useState<Date>(() =>
    mode === 'date' ? parseLocalDate(value) : parseLocalTime(value),
  );

  const parse = mode === 'date' ? parseLocalDate : parseLocalTime;
  const format = mode === 'date' ? formatLocalDate : formatLocalTime;
  const display =
    mode === 'date'
      ? parseLocalDate(value).toLocaleDateString(i18n.language, {
          day: 'numeric',
          month: 'long',
          year: 'numeric',
        })
      : parseLocalTime(value).toLocaleTimeString(i18n.language, {
          hour: '2-digit',
          minute: '2-digit',
        });

  const openPicker = () => {
    setDraft(parse(value));
    setOpen(true);
  };

  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${label}: ${display}`}
        accessibilityState={{ disabled }}
        disabled={disabled}
        onPress={openPicker}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>{display}</Text>
      </Pressable>

      {open &&
        (Platform.OS === 'android' ? (
          <DateTimePicker
            mode={mode}
            display="default"
            presentation="dialog"
            value={parse(value)}
            positiveButton={{ label: t('mobile.applyAction') }}
            negativeButton={{ label: t('mobile.cancel') }}
            onValueChange={(_, date) => {
              onChange(format(date));
              setOpen(false);
            }}
            onDismiss={() => setOpen(false)}
          />
        ) : (
          <AppDialog
            visible
            title={label}
            confirmLabel={t('mobile.applyAction')}
            cancelLabel={t('mobile.cancel')}
            onConfirm={() => {
              onChange(format(draft));
              setOpen(false);
            }}
            onCancel={() => setOpen(false)}
          >
            <DateTimePicker
              mode={mode}
              // Dates get the graphical calendar; times the classic wheels
              // (the only time UI iOS offers beyond the unreachable chip).
              display={mode === 'date' ? 'inline' : 'spinner'}
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
    // Field-value chrome: the same ghost-button look as JumpToDateButton so a
    // tappable date/time reads consistently across the app.
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
  });
