import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { FlatList, StyleSheet, Text, View } from 'react-native';

import { feedbackLogLines, useHeldFeedbackLogs } from '../state/feedbackLogs';
import { useThemedStyles, type ThemeColors } from '../theme';

// The logs a feedback message would carry, to read before sending: the very
// text the form holds and sends. One line per row, so VoiceOver and TalkBack
// move through it line by line, and a list so two megabytes of text do not
// become one element nobody can navigate. It follows the form: after a
// relaunch it says the logs are loading until the form has read them.

export default function FeedbackLogsScreen() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const held = useHeldFeedbackLogs();
  const text = held.state === 'ready' ? held.text : null;
  const lines = useMemo(() => feedbackLogLines(text), [text]);

  if (lines.length === 0) {
    const note =
      held.state === 'loading'
        ? t('dialogs.settings.feedback.logsLoading')
        : held.state === 'failed'
          ? t('dialogs.settings.feedback.logsFailed')
          : held.state === 'ready'
            ? t('dialogs.settings.feedback.logsEmpty')
            : t('dialogs.settings.feedback.logsNotAttached');
    return (
      <View style={[styles.screen, styles.content]}>
        <Text style={styles.empty}>{note}</Text>
      </View>
    );
  }

  return (
    <FlatList
      style={styles.screen}
      contentContainerStyle={styles.content}
      data={lines}
      keyExtractor={(_, index) => String(index)}
      initialNumToRender={40}
      renderItem={({ item }) => (
        <Text style={styles.line} selectable>
          {item}
        </Text>
      )}
    />
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16 },
    empty: { fontSize: 16, color: c.textSecondary },
    line: {
      fontSize: 12,
      lineHeight: 18,
      color: c.textPrimary,
      fontFamily: 'monospace',
    },
  });
