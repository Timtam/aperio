import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { ColorLabel } from '@aperio/shared';

/**
 * Accessible colour-label picker: a radio group over "No colour" + every named
 * label. Serves both audiences — sighted users see a real colour SWATCH per
 * option, screen-reader users hear the label NAME (the swatch is decorative and
 * carries no a11y label; the option's accessibilityLabel is the name). `value`
 * is the bound `color_label` id, `''` = none; `onChange('')` clears it. Ad-hoc
 * one-off colours are excluded (they're not user-named).
 */
export function ColorLabelSelect({
  value,
  labels,
  onChange,
  disabled,
}: {
  value: string;
  labels: ColorLabel[];
  onChange: (id: string) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const options = useMemo(
    () => [
      { value: '', label: t('dialogs.task.section.noColor'), hex: null as string | null },
      ...labels
        .filter((l) => !l.ad_hoc)
        .map((l) => ({ value: l.id, label: l.name, hex: l.hex })),
    ],
    [labels, t],
  );

  return (
    <View
      accessibilityRole="radiogroup"
      accessibilityLabel={t('dialogs.colorLabels.fields.color')}
      style={styles.group}
    >
      <Text style={styles.legend}>{t('dialogs.colorLabels.fields.color')}</Text>
      {options.map((opt) => {
        const selected = opt.value === value;
        return (
          <Pressable
            key={opt.value || '__none__'}
            accessible
            accessibilityRole="radio"
            accessibilityState={{ selected, disabled: !!disabled }}
            accessibilityLabel={opt.label}
            disabled={disabled}
            onPress={() => onChange(opt.value)}
            style={({ pressed }) => [
              styles.option,
              selected && styles.optionSelected,
              pressed && styles.optionPressed,
              disabled && styles.optionDisabled,
            ]}
          >
            <Text style={styles.marker} importantForAccessibility="no">
              {selected ? '◉' : '○'}
            </Text>
            {/* Real colour swatch for sighted users (decorative for SR). "No
                colour" gets an outlined empty box rather than a filled one. */}
            <View
              accessible={false}
              style={[
                styles.swatch,
                opt.hex != null
                  ? { backgroundColor: opt.hex }
                  : styles.swatchNone,
              ]}
            />
            <Text style={styles.optionLabel} importantForAccessibility="no">
              {opt.label}
            </Text>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  group: { gap: 6 },
  legend: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  option: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  optionSelected: { borderColor: '#1d4ed8', backgroundColor: '#eaf0fd' },
  optionPressed: { backgroundColor: '#e4ebf5' },
  optionDisabled: { opacity: 0.5 },
  marker: { fontSize: 18, width: 22, textAlign: 'center', color: '#10131a' },
  swatch: {
    width: 20,
    height: 20,
    borderRadius: 5,
    borderWidth: 1,
    borderColor: 'rgba(0,0,0,0.18)',
  },
  swatchNone: { backgroundColor: '#ffffff', borderStyle: 'dashed' },
  optionLabel: { flex: 1, fontSize: 16, color: '#10131a' },
});
