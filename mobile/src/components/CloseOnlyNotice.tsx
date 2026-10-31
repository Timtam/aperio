import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
} from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';
import { FormScrollView } from './FormScrollView';

/**
 * What a screen shows in place of its form once a write landed in part: the
 * sentence, focused, and Close. A new series that may stand (decisions 203,
 * 210), an item created without one of its follow-up steps (273), or a move
 * whose original stayed behind (277) — saving again would write twice what
 * already stands, so the form never comes back. Mirrors the desktop's
 * `CloseOnlyNotice`; the screen's header says Close too (`useCancelHeader`).
 */
export function CloseOnlyNotice({
  sentence,
  onClose,
}: {
  sentence: string;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const ref = useRef<Text | null>(null);
  useEffect(() => {
    // After the render that put it there, so the node exists.
    requestAnimationFrame(() => {
      const tag = findNodeHandle(ref.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    });
  }, [sentence]);
  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      <Text ref={ref} style={styles.sentence} accessibilityRole="text">
        {sentence}
      </Text>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.close')}
        onPress={onClose}
        style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
      >
        <Text style={styles.buttonText}>{t('dialogs.close')}</Text>
      </Pressable>
    </FormScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 20, gap: 18 },
    sentence: { fontSize: 15, fontWeight: '600', color: c.danger },
    button: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    buttonPressed: { backgroundColor: c.accentPressed },
    buttonText: { fontSize: 17, fontWeight: '700', color: c.textOnAccent },
  });
