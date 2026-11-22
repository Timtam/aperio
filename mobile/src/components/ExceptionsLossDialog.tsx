import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Platform, Pressable, StyleSheet, Text } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

/**
 * The question before a save of a series that would make the provider drop
 * occurrences the user changed or deleted on their own (decisions 243-253),
 * with the shared wording (`exceptionsLossQuestion`): Cancel, and the
 * destructive „Trotzdem speichern“, laid out as the event scope dialog lays
 * its options out.
 *
 * Mounted in the editor's own screen (question 15 of stage 12): the root
 * `EventScopeDialogHost` it went through cannot present over the editor sheet
 * on iOS. The answer is handed over once the dialog is gone (iOS
 * `onDismiss`, Android the next turn), so the save it starts never presents
 * or navigates while this dialog is still dismissing.
 */
export function ExceptionsLossDialog({
  question,
  onDone,
}: {
  question: { title: string; message: string; confirm: string };
  /** Whether to save anyway, once the dialog is gone. */
  onDone: (saveAnyway: boolean) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [visible, setVisible] = useState(true);
  const pending = useRef(false);
  const closing = useRef(false);
  const done = useRef(false);

  const finish = () => {
    if (done.current) return;
    done.current = true;
    onDone(pending.current);
  };
  const close = (saveAnyway: boolean) => {
    if (closing.current) return;
    closing.current = true;
    pending.current = saveAnyway;
    setVisible(false);
    if (Platform.OS !== 'ios') setTimeout(finish, 0);
  };

  return (
    <AppDialog
      visible={visible}
      title={question.title}
      message={question.message}
      cancelLabel={t('dialogs.cancel')}
      onCancel={() => close(false)}
      onDismiss={Platform.OS === 'ios' ? finish : undefined}
    >
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={question.confirm}
        onPress={() => close(true)}
        style={({ pressed }) => [styles.danger, pressed && styles.pressed]}
      >
        <Text style={styles.dangerText}>{question.confirm}</Text>
      </Pressable>
    </AppDialog>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    // The event scope dialog's destructive option, as the question looked
    // there.
    danger: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    pressed: { opacity: 0.7 },
    dangerText: {
      fontSize: 16,
      fontWeight: '700',
      color: c.danger,
      textAlign: 'center',
    },
  });
