import { useRef, useState } from 'react';
import { Platform, Pressable, StyleSheet, Text, View } from 'react-native';

import type { WeekdayQuestionText } from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

/** What the weekday question was answered with; `null` keeps the zone. */
export type WeekdayAnswer = 'rule' | 'now' | null;

/**
 * The weekday question on the phone (DESIGN-series-time-zone.md, "Die
 * Wochentag-Frage (15a)", in the wording and order of 138a): the title with
 * the first focus, the message, then „Zeitzone nicht ändern“ and the answers
 * that can be carried out, the rule's first. Keeping the zone is the dialog's
 * Cancel, so the back button and a tap outside keep it too.
 *
 * Mount it in the editor's own screen, never through a host at the app root:
 * a root modal does not appear over the editor sheet on iOS. The answer is
 * handed over once the dialog is gone (iOS `onDismiss`, Android the next
 * turn), so the caller can move the focus back to the zone button and speak
 * once, with nothing presenting in between.
 *
 * Unmeasured on a device (DESIGN "Ungeprüft"): VoiceOver and TalkBack reading
 * the title first, and the announcement after it closes.
 */
export function WeekdayQuestionDialog({
  question,
  onDone,
}: {
  question: WeekdayQuestionText;
  /** The answer once the dialog is gone. */
  onDone: (answer: WeekdayAnswer) => void;
}) {
  const styles = useThemedStyles(makeStyles);
  const [visible, setVisible] = useState(true);
  const pending = useRef<WeekdayAnswer>(null);
  const closing = useRef(false);
  const done = useRef(false);

  const finish = () => {
    if (done.current) return;
    done.current = true;
    onDone(pending.current);
  };
  // Once: a second tap while the dialog fades out changes nothing.
  const close = (answer: WeekdayAnswer) => {
    if (closing.current) return;
    closing.current = true;
    pending.current = answer;
    setVisible(false);
    if (Platform.OS !== 'ios') setTimeout(finish, 0);
  };

  return (
    <AppDialog
      visible={visible}
      title={question.title}
      message={question.message}
      cancelLabel={question.keep}
      cancelFirst
      onCancel={() => close(null)}
      onDismiss={Platform.OS === 'ios' ? finish : undefined}
    >
      <View style={styles.answers}>
        {question.answers.map((answer) => (
          <Pressable
            key={answer.answer}
            accessibilityRole="button"
            accessibilityLabel={answer.label}
            onPress={() => close(answer.answer)}
            style={({ pressed }) => [styles.answer, pressed && styles.pressed]}
          >
            <Text style={styles.answerText}>{answer.label}</Text>
          </Pressable>
        ))}
      </View>
    </AppDialog>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    answers: { gap: 10 },
    // Neutral: neither answer is dangerous, and neither is the default.
    answer: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    pressed: { opacity: 0.7 },
    answerText: { fontSize: 16, fontWeight: '600', color: c.textPrimary },
  });
