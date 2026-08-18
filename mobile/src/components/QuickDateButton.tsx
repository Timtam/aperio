import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { quickDates } from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

// The four dates people actually pick, one tap away. Twin of the desktop
// QuickDateButton — same shared `quickDates`, so both platforms offer the same
// days and compute them the same way.
//
// A fixed, tiny set with nothing to configure: four learnable buttons beat a
// list nobody would maintain, and the full date picker is right beside them.
// Each button names the DATE it will set as well as the offer, so nobody has
// to accept one to find out what it was.

export function QuickDateButton({
  onPick,
  weekStartsOn = 1,
}: {
  /** Receives a local `YYYY-MM-DD`. */
  onPick: (dayKey: string) => void;
  weekStartsOn?: number;
}) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [open, setOpen] = useState(false);

  // Computed on OPEN: a sheet left open across midnight would otherwise still
  // be offering yesterday's idea of "today".
  const offers = open ? quickDates(new Date(), weekStartsOn) : [];

  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.quickDate.open')}
        onPress={() => setOpen(true)}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>{t('dialogs.quickDate.open')}</Text>
      </Pressable>
      {open && (
        <AppDialog
          visible
          title={t('dialogs.quickDate.title')}
          cancelLabel={t('dialogs.cancel')}
          onCancel={() => setOpen(false)}
        >
          <View style={styles.choices}>
            {offers.map((offer) => {
              const label = t(`dialogs.quickDate.${offer.id}`, {
                date: new Date(`${offer.dayKey}T00:00:00`).toLocaleDateString(
                  i18n.language,
                  {
                    weekday: 'long',
                    year: 'numeric',
                    month: 'long',
                    day: 'numeric',
                  },
                ),
              });
              return (
                <Pressable
                  key={offer.id}
                  accessibilityRole="button"
                  accessibilityLabel={label}
                  onPress={() => {
                    onPick(offer.dayKey);
                    setOpen(false);
                  }}
                  style={({ pressed }) => [styles.choice, pressed && styles.pressed]}
                >
                  <Text style={styles.choiceText}>{label}</Text>
                </Pressable>
              );
            })}
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
  });
