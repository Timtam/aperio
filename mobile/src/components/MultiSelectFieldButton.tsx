import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { selectableCheckState, selectableRole } from '../a11y/roles';
import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';
import type { RadioOption } from './RadioGroup';

/**
 * Multi-select as ONE focus stop — the checkbox twin of `SelectFieldButton`:
 * a button carrying "label: the selected values", opening the options as
 * checkboxes in a focus-trapped dialog. Toggles apply on Done; Cancel (or
 * tap-outside) leaves the selection untouched, so an accidental toggle can't
 * half-commit.
 */
export function MultiSelectFieldButton<T extends string | number>({
  label,
  values,
  options,
  onChange,
  disabled,
  emptyLabel,
}: {
  label: string;
  values: readonly T[];
  options: RadioOption<T>[];
  onChange: (values: T[]) => void;
  disabled?: boolean;
  /** Shown as the value when nothing is selected (e.g. "none"). */
  emptyLabel: string;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [open, setOpen] = useState(false);
  // Draft, applied on Done — multi-select has no single "the choice was made"
  // moment, so an explicit commit keeps backing out safe.
  const [draft, setDraft] = useState<T[]>([]);

  const display =
    options
      .filter((o) => values.includes(o.value))
      .map((o) => o.label)
      .join(', ') || emptyLabel;

  return (
    <View style={styles.field}>
      <Text style={styles.legend} importantForAccessibility="no">
        {label}
      </Text>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${label}: ${display}`}
        accessibilityHint={t('mobile.selectFieldHint')}
        accessibilityState={{ disabled: !!disabled }}
        disabled={disabled}
        onPress={() => {
          setDraft([...values]);
          setOpen(true);
        }}
        style={({ pressed }) => [
          styles.button,
          pressed && styles.pressed,
          disabled && styles.buttonDisabled,
        ]}
      >
        <Text style={styles.buttonText}>{display}</Text>
      </Pressable>

      {open && (
        <AppDialog
          visible
          title={label}
          confirmLabel={t('mobile.applyAction')}
          cancelLabel={t('mobile.cancel')}
          onConfirm={() => {
            onChange(draft);
            setOpen(false);
          }}
          onCancel={() => setOpen(false)}
        >
          {options.map((opt) => {
            const checked = draft.includes(opt.value);
            return (
              <Pressable
                key={String(opt.value)}
                accessible
                accessibilityRole={selectableRole('checkbox')}
                accessibilityState={selectableCheckState(checked)}
                accessibilityLabel={opt.label}
                onPress={() =>
                  setDraft((d) =>
                    checked ? d.filter((x) => x !== opt.value) : [...d, opt.value],
                  )
                }
                style={({ pressed }) => [
                  styles.option,
                  checked && styles.optionSelected,
                  pressed && styles.optionPressed,
                ]}
              >
                <Text style={styles.marker} importantForAccessibility="no">
                  {checked ? '☑' : '☐'}
                </Text>
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
    button: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
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
    optionLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
  });
