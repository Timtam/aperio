import { useRef, type ReactNode } from 'react';
import {
  AccessibilityInfo,
  findNodeHandle,
  Modal,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';

// A reusable, screen-reader-trapping dialog — the app's popup primitive for
// destructive confirmations and secret-bearing decisions (e.g. the E2E
// "encrypted dataset found" join). Unlike the OS Alert it can host an input and
// a Confirm that stays disabled until the input is valid, and unlike an inline
// form section it TRAPS focus: a transparent <Modal> overlays a dimmed scrim,
// and the card carries `accessibilityViewIsModal` so VoiceOver can't reach the
// content behind it (and on Android the Modal owns its own window). Initial SR
// focus lands on the input when present (so the user starts typing), else the
// title. Tap-outside / hardware-back map to Cancel; both are blocked while busy.

export interface AppDialogInput {
  value: string;
  onChangeText: (v: string) => void;
  label: string;
  placeholder?: string;
  secureTextEntry?: boolean;
  autoCapitalize?: 'none' | 'sentences';
}

export function AppDialog({
  visible,
  title,
  message,
  confirmLabel,
  cancelLabel,
  onConfirm,
  onCancel,
  confirmDisabled = false,
  destructive = false,
  busy = false,
  input,
  children,
}: {
  visible: boolean;
  title: string;
  message?: string;
  confirmLabel: string;
  cancelLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
  /** Greys + disables Confirm until the caller's validity gate passes. */
  confirmDisabled?: boolean;
  /** Confirm uses danger styling (irreversible action). */
  destructive?: boolean;
  /** Disables every control + blocks dismissal while an action is in flight. */
  busy?: boolean;
  /** Optional secure/plain field; when present, initial SR focus lands here. */
  input?: AppDialogInput;
  /** Escape hatch for richer bodies (e.g. a radio choice). */
  children?: ReactNode;
}) {
  const styles = useThemedStyles(makeStyles);
  const titleRef = useRef<Text | null>(null);
  const inputRef = useRef<TextInput | null>(null);

  const focusInitial = () => {
    const node = input ? inputRef.current : titleRef.current;
    const tag = findNodeHandle(node);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  };

  const blocked = confirmDisabled || busy;

  return (
    <Modal
      transparent
      visible={visible}
      animationType="fade"
      onRequestClose={busy ? undefined : onCancel}
      onShow={focusInitial}
    >
      <View style={styles.root}>
        {/* Dimmed scrim behind the card; tap-outside cancels (sighted users). */}
        <Pressable
          accessible={false}
          importantForAccessibility="no-hide-descendants"
          style={[StyleSheet.absoluteFill, styles.backdrop]}
          onPress={busy ? undefined : onCancel}
        />
        <View accessibilityViewIsModal style={styles.card}>
          <Text ref={titleRef} style={styles.title} accessibilityRole="header">
            {title}
          </Text>
          {message != null && (
            <Text style={styles.message} accessibilityRole="text">
              {message}
            </Text>
          )}
          {children}
          {input != null && (
            <TextInput
              ref={inputRef}
              style={styles.input}
              value={input.value}
              onChangeText={input.onChangeText}
              placeholder={input.placeholder}
              accessibilityLabel={input.label}
              secureTextEntry={input.secureTextEntry}
              autoCapitalize={input.autoCapitalize ?? 'none'}
              autoCorrect={false}
              returnKeyType="done"
              editable={!busy}
              onSubmitEditing={() => {
                if (!blocked) onConfirm();
              }}
            />
          )}
          <View style={styles.buttons}>
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={cancelLabel}
              accessibilityState={{ disabled: busy }}
              disabled={busy}
              onPress={onCancel}
              style={({ pressed }) => [styles.ghost, pressed && styles.ghostPressed]}
            >
              <Text style={styles.ghostText}>{cancelLabel}</Text>
            </Pressable>
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={confirmLabel}
              accessibilityState={{ disabled: blocked, busy }}
              disabled={blocked}
              onPress={onConfirm}
              style={({ pressed }) => [
                destructive ? styles.danger : styles.primary,
                pressed && !blocked && styles.confirmPressed,
                blocked && styles.confirmDisabled,
              ]}
            >
              <Text style={destructive ? styles.dangerText : styles.primaryText}>
                {confirmLabel}
              </Text>
            </Pressable>
          </View>
        </View>
      </View>
    </Modal>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    root: { flex: 1, justifyContent: 'center', alignItems: 'center', padding: 24 },
    // The one justified literal: there is no dedicated scrim/overlay token.
    backdrop: { backgroundColor: 'rgba(0,0,0,0.5)' },
    card: {
      width: '100%',
      maxWidth: 480,
      gap: 14,
      padding: 20,
      borderRadius: 14,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    title: { fontSize: 19, fontWeight: '700', color: c.textPrimary },
    message: { fontSize: 15, color: c.textSecondary, lineHeight: 21 },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
    buttons: { flexDirection: 'row', justifyContent: 'flex-end', gap: 10, marginTop: 4 },
    ghost: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    ghostPressed: { backgroundColor: c.surfacePressed },
    ghostText: { fontSize: 16, fontWeight: '600', color: c.link },
    primary: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
    },
    confirmPressed: { backgroundColor: c.accentPressed },
    confirmDisabled: { backgroundColor: c.accentDisabled },
    primaryText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    danger: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.dangerBg,
      borderWidth: 1,
      borderColor: c.dangerBorder,
    },
    dangerText: { fontSize: 16, fontWeight: '700', color: c.danger },
  });
