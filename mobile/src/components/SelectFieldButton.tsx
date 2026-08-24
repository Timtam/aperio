import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { selectableRole } from '../a11y/roles';
import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';
import type { RadioOption } from './RadioGroup';

/**
 * Single-select as ONE focus stop: a button carrying "label: current value",
 * opening the option list in a focus-trapped dialog. The generalisation of
 * `ContactLabelButton`, whose header states the reason this exists — an inline
 * RadioGroup puts every option in front of the next field, so an editor with a
 * calendar picker, a section picker and a colour picker took dozens of swipes
 * to walk through. Collapsed, each picker costs exactly one stop, like the
 * desktop's native `<select>`.
 *
 * Deliberately prop-compatible with {@link RadioGroup} (label / value /
 * options / onChange / disabled / labelAsHeading), so converting a call site
 * is a component-name swap. Choosing an option applies immediately and closes
 * — the same commit model as a desktop `<select>`; Cancel (or tap-outside)
 * leaves the value untouched.
 */
export function SelectFieldButton<T extends string | number>({
  label,
  value,
  options,
  onChange,
  disabled,
  labelAsHeading,
  swatchFor,
}: {
  label: string;
  value: T;
  options: RadioOption<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  /** Expose the field label as an accessibility heading (section-level
   *  pickers); inline fields leave it off. */
  labelAsHeading?: boolean;
  /** Colour swatch per option (the colour-label picker): a hex renders a
   *  filled box, `null` an outlined "no colour" box; omit for no swatches. */
  swatchFor?: (value: T) => string | null;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [open, setOpen] = useState(false);

  const current = options.find((o) => o.value === value);
  const display = current?.label ?? '';
  const currentHex = swatchFor?.(value) ?? null;

  return (
    <View style={styles.field}>
      <Text
        style={styles.legend}
        accessibilityRole={labelAsHeading ? 'header' : undefined}
        importantForAccessibility={labelAsHeading ? 'yes' : 'no'}
      >
        {label}
      </Text>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${label}: ${display}`}
        accessibilityHint={t('mobile.selectFieldHint')}
        accessibilityState={{ disabled: !!disabled }}
        disabled={disabled}
        onPress={() => setOpen(true)}
        style={({ pressed }) => [
          styles.button,
          pressed && styles.pressed,
          disabled && styles.buttonDisabled,
        ]}
      >
        {swatchFor != null && (
          <View
            accessible={false}
            style={[
              styles.swatch,
              currentHex != null
                ? { backgroundColor: currentHex }
                : styles.swatchNone,
            ]}
          />
        )}
        <Text style={styles.buttonText}>{display}</Text>
      </Pressable>

      {open && (
        <AppDialog
          visible
          title={label}
          cancelLabel={t('mobile.cancel')}
          onCancel={() => setOpen(false)}
        >
          {options.map((opt) => {
            const selected = opt.value === value;
            const hex = swatchFor?.(opt.value) ?? null;
            return (
              <Pressable
                key={String(opt.value)}
                accessible
                accessibilityRole={selectableRole('radio')}
                accessibilityState={{ selected }}
                accessibilityLabel={opt.label}
                onPress={() => {
                  onChange(opt.value);
                  setOpen(false);
                }}
                style={({ pressed }) => [
                  styles.option,
                  selected && styles.optionSelected,
                  pressed && styles.optionPressed,
                ]}
              >
                <Text style={styles.marker} importantForAccessibility="no">
                  {selected ? '◉' : '○'}
                </Text>
                {swatchFor != null && (
                  <View
                    accessible={false}
                    style={[
                      styles.swatch,
                      hex != null
                        ? { backgroundColor: hex }
                        : styles.swatchNone,
                    ]}
                  />
                )}
                <Text style={styles.optionLabel} importantForAccessibility="no">
                  {opt.label}
                </Text>
              </Pressable>
            );
          })}
        </AppDialog>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    field: { gap: 6 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    // Same ghost-button chrome as DateTimeFieldButton / ContactLabelButton, so
    // a tappable field value reads consistently across the editors.
    button: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 10,
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    buttonDisabled: { opacity: 0.5 },
    pressed: { opacity: 0.7 },
    buttonText: { fontSize: 16, fontWeight: '600', color: c.link },
    option: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    optionSelected: { borderColor: c.accent, backgroundColor: c.surfaceSelected },
    optionPressed: { backgroundColor: c.surfacePressed },
    marker: { fontSize: 18, width: 22, textAlign: 'center', color: c.textPrimary },
    swatch: {
      width: 20,
      height: 20,
      borderRadius: 5,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    swatchNone: { backgroundColor: c.background, borderStyle: 'dashed' },
    optionLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
  });
