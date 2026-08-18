import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { useTimeStep } from '../state/useTimeStep';
import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

// The minutes of the current hour, on the grid — one tap instead of sixty.
//
// The desktop gets this for free: an `<input type="time">` takes a `step`. The
// native picker here cannot, because Expo's DateTimePicker documents
// `minuteInterval` as NOT SUPPORTED, so its wheel always moves one minute at a
// time. Reaching :30 from :00 is thirty swipes with VoiceOver.
//
// Rounding the user's choice behind their back would be the other way to make
// times land on the grid, and it is the wrong one: a time somebody deliberately
// set to 09:07 must stay 09:07. So this OFFERS instead — the hour stays
// whatever the wheel says, and the minutes become one press.

export function QuickTimeButton({
  value,
  onPick,
}: {
  /** Current 'HH:MM'. The hour is kept; only the minutes are offered. */
  value: string;
  onPick: (next: string) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const step = useTimeStep();
  const [open, setOpen] = useState(false);

  // A step of 1 means "no grid" — sixty offers would be worse than the wheel.
  if (step <= 1) return null;

  const [rawHour] = value.split(':');
  const hour = Number(rawHour);
  if (!Number.isFinite(hour) || hour < 0 || hour > 23) return null;

  const hh = String(hour).padStart(2, '0');
  const offers: string[] = [];
  for (let m = 0; m < 60; m += step) {
    offers.push(`${hh}:${String(m).padStart(2, '0')}`);
  }

  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.quickTime.open', { step })}
        onPress={() => setOpen(true)}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>{t('dialogs.quickTime.short')}</Text>
      </Pressable>
      {open && (
        <AppDialog
          visible
          title={t('dialogs.quickTime.title', { hour: hh })}
          cancelLabel={t('dialogs.cancel')}
          onCancel={() => setOpen(false)}
        >
          <View style={styles.choices}>
            {offers.map((time) => (
              <Pressable
                key={time}
                accessibilityRole="button"
                accessibilityLabel={time}
                onPress={() => {
                  onPick(time);
                  setOpen(false);
                }}
                style={({ pressed }) => [styles.choice, pressed && styles.pressed]}
              >
                <Text style={styles.choiceText}>{time}</Text>
              </Pressable>
            ))}
          </View>
        </AppDialog>
      )}
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    button: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    buttonText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
    choices: { gap: 6 },
    choice: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    choiceText: { fontSize: 17, color: c.textPrimary },
  });
