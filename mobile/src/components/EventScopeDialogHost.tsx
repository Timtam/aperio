import { useEffect, useRef, useState } from 'react';
import { Platform, Pressable, StyleSheet, Text, View } from 'react-native';

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
  // Split "mounted" (config) from "shown" (visible) so the dismiss animation can
  // play and the chosen action runs only AFTER the modal is gone — see below.
  const [visible, setVisible] = useState(false);
  // Notify default = on (the common intent when cancelling a meeting you own).
  const [notify, setNotify] = useState(true);
  // The action to run once the dialog has fully dismissed. Deferring it is what
  // avoids an iOS "present while a presentation is in progress" crash: an option
  // often navigates (present another native modal) or goBack (dismiss the
  // editor), which iOS refuses while THIS modal is still dismissing. The old
  // Alert dismissed before firing its handler; we reproduce that ordering.
  const pending = useRef<(() => void) | null>(null);

  useEffect(
    () =>
      registerEventScopeDialog((cfg) => {
        pending.current = null;
        setNotify(true);
        setConfig(cfg);
        setVisible(true);
      }),
    [],
  );

  // Runs after the modal is gone: on iOS via the Modal's onDismiss; on Android
  // (no onDismiss, and no present-while-dismissing constraint) right after the
  // close is committed. Nulling `pending` first makes a double-fire a no-op.
  const finish = () => {
    const run = pending.current;
    pending.current = null;
    setConfig(null);
    run?.();
  };

  if (config == null) return null;
  const cfg = config;

  const dismiss = () => {
    setVisible(false);
    if (Platform.OS !== 'ios') setTimeout(finish, 0);
  };
  const cancel = () => {
    pending.current = null;
    dismiss();
  };
  const select = (opt: EventScopeOption) => {
    pending.current = () => opt.run(cfg.notify != null ? notify : false);
    dismiss();
  };

  return (
    <AppDialog
      visible={visible}
      title={config.title}
      message={config.message}
      cancelLabel={config.cancelLabel}
      onCancel={cancel}
      onDismiss={Platform.OS === 'ios' ? finish : undefined}
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
