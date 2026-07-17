import { useEffect, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';
import {
  registerEventScopeDialog,
  type EventScopeDialogConfig,
  type EventScopeOption,
} from '../state/eventScopeDialog';
import { AppDialog } from './AppDialog';
import { RadioGroup } from './RadioGroup';

// Shared event-scope chooser — the accessible in-app replacement for a
// multi-button Alert. React Native's Android Alert silently keeps only the first
// three buttons (buttons.slice(0, 3)), so an organizer delete (notify/silent ×
// this-occurrence/this-and-following/whole-series) or even a plain 4-way scope
// prompt would drop options and make "whole series" unreachable. This renders
// every option as a real button inside the focus-trapping AppDialog, and folds
// the notify/silent choice into a radio group above the scope buttons (the same
// shape as the desktop DeleteEventScopeDialog) so the notify decision is made
// once and each scope button applies it. The imperative opener lives in
// ../state/eventScopeDialog (showEventScopeDialog); this is the render host.

export function EventScopeDialogHost() {
  const styles = useThemedStyles(makeStyles);
  const [config, setConfig] = useState<EventScopeDialogConfig | null>(null);
  // Notify default = on (the common intent when cancelling a meeting you own).
  const [notify, setNotify] = useState(true);

  useEffect(
    () =>
      registerEventScopeDialog((cfg) => {
        setNotify(true);
        setConfig(cfg);
      }),
    [],
  );

  if (config == null) return null;
  const close = () => setConfig(null);
  const select = (opt: EventScopeOption) => {
    close();
    opt.run(config.notify != null ? notify : false);
  };

  return (
    <AppDialog
      visible
      title={config.title}
      message={config.message}
      cancelLabel={config.cancelLabel}
      onCancel={close}
    >
      {config.notify != null && (
        <RadioGroup<'notify' | 'silent'>
          label={config.notify.legend}
          value={notify ? 'notify' : 'silent'}
          options={[
            { value: 'notify', label: config.notify.notifyLabel },
            { value: 'silent', label: config.notify.silentLabel },
          ]}
          onChange={(v) => setNotify(v === 'notify')}
        />
      )}
      <View style={styles.options}>
        {config.options.map((opt) => (
          <Pressable
            key={opt.key}
            accessibilityRole="button"
            accessibilityLabel={opt.label}
            onPress={() => select(opt)}
            style={({ pressed }) => [
              opt.destructive ? styles.danger : styles.neutral,
              pressed && styles.pressed,
            ]}
          >
            <Text
              style={opt.destructive ? styles.dangerText : styles.neutralText}
            >
              {opt.label}
            </Text>
          </Pressable>
        ))}
      </View>
    </AppDialog>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    options: { gap: 10 },
    neutral: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    danger: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    pressed: { opacity: 0.7 },
    neutralText: {
      fontSize: 16,
      fontWeight: '600',
      color: c.textPrimary,
      textAlign: 'center',
    },
    dangerText: {
      fontSize: 16,
      fontWeight: '700',
      color: c.danger,
      textAlign: 'center',
    },
  });
