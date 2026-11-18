import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  feedbackOutcomeSentence,
  type FeedbackInfo,
} from '@aperio/shared';

import { feedbackInfo, feedbackLogs, sendFeedback } from '../api/feedback';
import { FormScrollView } from '../components/FormScrollView';
import { ReadOnlyField } from '../components/ReadOnlyField';
import { StepWriteWaitHost } from '../components/StepWriteWaitHost';
import { SwitchRow } from '../components/SwitchRow';
import type { RootStackScreenProps } from '../navigation/types';
import {
  feedbackLogLines,
  holdFeedbackLogs,
  type HeldFeedbackLogs,
} from '../state/feedbackLogs';
import { waitWithDialog } from '../state/stepWriteWait';
import { useThemedStyles, type ThemeColors } from '../theme';

// Feedback: a message, an optional reply address and, on request, the logs,
// sent through host_core::feedback. The phone twin of the desktop
// FeedbackPanel. Nothing leaves the phone before Send, and what leaves is on
// screen: the system line as a field, the logs on their own reading screen
// (FeedbackLogsScreen), which shows the very text the draft carries. "Send
// logs" on the Logs screen opens this one with the switch on.

/** The logs as the form holds them: not asked for yet, on the way, read, or
 *  unreadable. The reading screen sees the same shape. */
type Logs = HeldFeedbackLogs;

/**
 * The form, and beside it the "sending, please wait" dialog (Toni,
 * 2026-11-18): on iOS a dialog presents from the screen it is rendered in.
 * Beside the content, not in it, so it stays mounted whichever state the
 * form shows.
 */
export default function FeedbackScreen(props: RootStackScreenProps<'Feedback'>) {
  return (
    <>
      <FeedbackForm {...props} />
      <StepWriteWaitHost />
    </>
  );
}

function FeedbackForm({ navigation, route }: RootStackScreenProps<'Feedback'>) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const initialAttachLogs = route.params?.withLogs === true;

  const [info, setInfo] = useState<FeedbackInfo | null>(null);
  const [message, setMessage] = useState('');
  const [email, setEmail] = useState('');
  const [attachLogs, setAttachLogs] = useState(initialAttachLogs);
  const [logs, setLogs] = useState<Logs>({ state: 'idle' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    feedbackInfo()
      .then((next) => {
        if (mounted.current) setInfo(next);
      })
      .catch(() => {
        // The form stays empty; nothing to send through a bridge that fails.
      });
  }, []);

  // The reading screen shows what this form holds, and nothing once it is gone.
  useEffect(() => {
    holdFeedbackLogs(attachLogs ? logs : { state: 'idle' });
  }, [attachLogs, logs]);
  useEffect(() => () => holdFeedbackLogs({ state: 'idle' }), []);

  // Read once, when the switch is first turned on: the form then shows and
  // sends that text, rather than a newer one the user has not seen.
  const readLogs = useCallback(() => {
    setLogs({ state: 'loading' });
    feedbackLogs()
      .then((text) => {
        if (mounted.current) setLogs({ state: 'ready', text });
      })
      .catch(() => {
        if (mounted.current) setLogs({ state: 'failed' });
      });
  }, []);

  useEffect(() => {
    if (initialAttachLogs) readLogs();
  }, [initialAttachLogs, readLogs]);

  const onToggleLogs = useCallback(() => {
    const next = !attachLogs;
    setAttachLogs(next);
    if (next && logs.state === 'idle') readLogs();
  }, [attachLogs, logs.state, readLogs]);

  const waitingForLogs = attachLogs && logs.state === 'loading';

  const onSend = useCallback(async () => {
    if (busy || waitingForLogs) return;
    setError(null);
    setNotice(null);
    setBusy(true);
    const sentMessage = message;
    try {
      // The dialog takes the screen reader's focus and says itself: the
      // button's own word changing is not said, and a send with the logs can
      // take up to a minute. It is gone before the result is said.
      const outcome = await waitWithDialog(
        {
          title: t('dialogs.settings.feedback.waitTitle'),
          body: t(
            attachLogs
              ? 'dialogs.settings.feedback.waitBodyLogs'
              : 'dialogs.settings.feedback.waitBody',
          ),
        },
        () =>
          sendFeedback({
            message: sentMessage,
            email: email.trim() === '' ? null : email,
            logs: attachLogs && logs.state === 'ready' ? logs.text : null,
          }),
      );
      const sentence = feedbackOutcomeSentence(outcome, t);
      if (outcome.outcome === 'sent') {
        // Only what was sent is cleared: text typed while it was on its way
        // stays in the field.
        setMessage((current) => (current === sentMessage ? '' : current));
        setAttachLogs(false);
        setLogs({ state: 'idle' });
        setNotice(sentence);
      } else {
        setError(sentence);
      }
      AccessibilityInfo.announceForAccessibility(sentence);
    } catch {
      const sentence = t('dialogs.settings.feedback.failure.service');
      setError(sentence);
      AccessibilityInfo.announceForAccessibility(sentence);
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [busy, waitingForLogs, message, email, attachLogs, logs, t]);

  // Counted once per read, not per keystroke: the text can be 30 MB. Above
  // the early returns, like every hook.
  const lineCount = useMemo(
    () => (logs.state === 'ready' ? feedbackLogLines(logs.text).length : 0),
    [logs],
  );

  if (!info) return <View style={styles.screen} />;

  if (!info.available) {
    return (
      <View style={[styles.screen, styles.content]}>
        <Text style={styles.hint}>{t('dialogs.settings.feedback.unavailable')}</Text>
      </View>
    );
  }

  const sendDisabled = busy || waitingForLogs;

  return (
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
      <Text style={styles.hint}>{t('dialogs.settings.feedback.hint')}</Text>

      <View style={styles.field}>
        <Text style={styles.label} importantForAccessibility="no" accessibilityElementsHidden>
          {t('dialogs.settings.feedback.messageLabel')}
        </Text>
        <TextInput
          style={[styles.input, styles.multiline]}
          value={message}
          onChangeText={setMessage}
          accessibilityLabel={t('dialogs.settings.feedback.messageLabel')}
          multiline
          numberOfLines={6}
          textAlignVertical="top"
        />
      </View>

      <View style={styles.field}>
        <Text style={styles.label} importantForAccessibility="no" accessibilityElementsHidden>
          {t('dialogs.settings.feedback.emailLabel')}
        </Text>
        <TextInput
          style={styles.input}
          value={email}
          onChangeText={setEmail}
          accessibilityLabel={t('dialogs.settings.feedback.emailLabel')}
          keyboardType="email-address"
          autoCapitalize="none"
          autoCorrect={false}
          autoComplete="email"
          textContentType="emailAddress"
        />
        <Text style={styles.hint}>{t('dialogs.settings.feedback.emailHint')}</Text>
      </View>

      <SwitchRow
        label={t('dialogs.settings.feedback.attachLogs')}
        hint={t('dialogs.settings.feedback.attachLogsHint')}
        value={attachLogs}
        onToggle={onToggleLogs}
      />

      {attachLogs && logs.state === 'ready' && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.feedback.viewLogs', { count: lineCount })}
          onPress={() => navigation.navigate('FeedbackLogs')}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText} importantForAccessibility="no">
            {t('dialogs.settings.feedback.viewLogs', { count: lineCount })}
          </Text>
        </Pressable>
      )}
      {attachLogs && logs.state === 'loading' && (
        <Text style={styles.hint}>{t('dialogs.settings.feedback.logsLoading')}</Text>
      )}
      {attachLogs && logs.state === 'failed' && (
        <Text style={styles.hint}>{t('dialogs.settings.feedback.logsFailed')}</Text>
      )}

      <ReadOnlyField
        label={t('dialogs.settings.feedback.systemLabel')}
        value={info.system}
      />

      {error && <Text style={styles.error}>{error}</Text>}
      {notice && <Text style={styles.hint}>{notice}</Text>}

      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: sendDisabled }}
        accessibilityLabel={
          busy ? t('dialogs.settings.feedback.sending') : t('dialogs.settings.feedback.send')
        }
        disabled={sendDisabled}
        onPress={() => void onSend()}
        style={({ pressed }) => [
          styles.primaryButton,
          pressed && styles.pressed,
          sendDisabled && styles.disabled,
        ]}
      >
        <Text style={styles.primaryButtonText} importantForAccessibility="no">
          {busy ? t('dialogs.settings.feedback.sending') : t('dialogs.settings.feedback.send')}
        </Text>
      </Pressable>
    </FormScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 18 },
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 14, color: c.textSecondary, lineHeight: 20 },
    error: { fontSize: 15, color: c.danger, lineHeight: 21 },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    multiline: { minHeight: 140 },
    primaryButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
    disabled: { opacity: 0.5 },
  });
