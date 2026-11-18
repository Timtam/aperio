import { memo, useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  feedbackFailureField,
  feedbackOutcomeSentence,
  type FeedbackInfo,
  type FeedbackOutcome,
} from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import { feedbackCheck, feedbackInfo, feedbackLogs, sendFeedback } from '../api/client';
import { ReadOnlyText } from './ReadOnlyText';
import { useStepWriteWait } from './useStepWriteWait';

/** The logs field, rendered again only when its text changes: the text can
 *  be 30 MB, and every keystroke in the message renders the panel. */
const LogsField = memo(ReadOnlyText);

/** The logs as the form holds them: not asked for yet, on the way, read, or
 *  unreadable. */
type Logs =
  | { state: 'idle' }
  | { state: 'loading' }
  | { state: 'ready'; text: string }
  | { state: 'failed' };

/**
 * Settings → Feedback: a message, an optional reply address and, on request,
 * the logs, sent through `host_core::feedback`.
 *
 * Nothing leaves the device before Send, and what leaves is on screen: the
 * system line as a note, the logs as a field to read line by line. The text
 * that field shows is the text the draft carries, so what the user read is
 * what is sent. "Send logs" in the logs panel opens this tab with the box
 * ticked (`initialAttachLogs`).
 */
export function FeedbackPanel({ initialAttachLogs = false }: { initialAttachLogs?: boolean }) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const ids = useId();
  const emailHintId = `${ids}-email-hint`;
  const logsHintId = `${ids}-logs-hint`;

  const [info, setInfo] = useState<FeedbackInfo | null>(null);
  const [message, setMessage] = useState('');
  const [email, setEmail] = useState('');
  const [attachLogs, setAttachLogs] = useState(initialAttachLogs);
  const [logs, setLogs] = useState<Logs>({ state: 'idle' });
  const [busy, setBusy] = useState(false);
  // The guard against a second send: `busy` itself goes back before the
  // dialog closes, so the button focus returns to says "Send" again.
  const sending = useRef(false);
  const [error, setError] = useState<string | null>(null);
  // The thanks after a send, on screen as well as spoken: a sighted user
  // hears nothing, and the form clearing alone does not say it worked.
  const [notice, setNotice] = useState<string | null>(null);

  const messageRef = useRef<HTMLTextAreaElement>(null);
  const emailRef = useRef<HTMLInputElement>(null);

  // "Sending, please wait" while the send runs (Toni, 2026-11-18): the
  // button's own word changing is not said by a screen reader, and a send with
  // the logs can take up to a minute. The dialog takes focus, says itself, and
  // is gone with focus back on Send before the result is shown or said.
  // The logs go along only when they were read; a failed read sends the
  // message alone, and the dialog says so.
  const sentLogs = attachLogs && logs.state === 'ready' ? logs.text : null;
  const { waitDialog, wait } = useStepWriteWait({
    title: t('dialogs.settings.feedback.waitTitle'),
    body: t(
      sentLogs != null
        ? 'dialogs.settings.feedback.waitBodyLogs'
        : 'dialogs.settings.feedback.waitBody',
    ),
  });

  useEffect(() => {
    let cancelled = false;
    feedbackInfo()
      .then((next) => {
        if (!cancelled) setInfo(next);
      })
      .catch((err: unknown) => {
        // eslint-disable-next-line no-console
        console.warn('feedback_info failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Whether the panel is still there when a read comes back. A ref, not an
  // effect-scoped flag: the read is started by a click as well as on mount.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // The logs are read once, when the box is first ticked: the form then shows
  // and sends that text, rather than a newer one the user has not seen.
  const readLogs = useCallback(() => {
    setLogs({ state: 'loading' });
    feedbackLogs()
      .then((text) => {
        if (mounted.current) setLogs({ state: 'ready', text });
      })
      .catch((err: unknown) => {
        // eslint-disable-next-line no-console
        console.warn('feedback_logs failed', err);
        if (mounted.current) setLogs({ state: 'failed' });
      });
  }, []);

  // "Send logs" opens the panel with the box ticked.
  useEffect(() => {
    if (initialAttachLogs) readLogs();
  }, [initialAttachLogs, readLogs]);

  const onAttachLogs = (checked: boolean) => {
    setAttachLogs(checked);
    if (checked && logs.state === 'idle') readLogs();
  };

  const onSend = useCallback(async () => {
    if (sending.current) return;
    // Ticked but not yet read: sending now would send something unseen.
    if (attachLogs && logs.state === 'loading') return;
    setError(null);
    setNotice(null);
    sending.current = true;
    setBusy(true);
    const sentMessage = message;
    const draft = {
      message: sentMessage,
      email: email.trim() === '' ? null : email,
      logs: sentLogs,
    };
    // A failure after the panel is gone (the dialog closed, another tab
    // chosen) has no alert line to stand in; the announcer still says it.
    const fail = (sentence: string) => {
      if (mounted.current) setError(sentence);
      else announce(sentence);
    };
    try {
      // What the send would refuse at once (an empty message, an incomplete
      // address) is said without the dialog: a draft that never leaves must
      // not be announced as on its way. The check is the core's, without the
      // logs, which it does not read.
      const refused = await feedbackCheck({ ...draft, logs: null });
      const outcome: FeedbackOutcome =
        refused != null
          ? { outcome: 'failed', reason: refused }
          : await wait(true, async () => {
              try {
                return await sendFeedback(draft);
              } finally {
                // Back to "Send" before the dialog closes and focus returns
                // to the button: it must not say "Sending …" once it is done.
                sending.current = false;
                setBusy(false);
              }
            });
      const sentence = feedbackOutcomeSentence(outcome, t);
      if (outcome.outcome === 'sent') {
        // Only what was sent is cleared: text typed while it was on its way
        // stays in the field.
        setMessage((current) => (current === sentMessage ? '' : current));
        setAttachLogs(false);
        setLogs({ state: 'idle' });
        setNotice(sentence);
        announce(sentence);
        return;
      }
      fail(sentence);
      const field = feedbackFailureField(outcome);
      if (field === 'message') messageRef.current?.focus();
      if (field === 'email') emailRef.current?.focus();
    } catch (err) {
      // The command answers every ending as an outcome; a throw is the bridge
      // itself failing, which the service sentence covers well enough.
      // eslint-disable-next-line no-console
      console.warn('send_feedback failed', err);
      fail(t('dialogs.settings.feedback.failure.service'));
    } finally {
      sending.current = false;
      setBusy(false);
    }
  }, [attachLogs, logs.state, sentLogs, message, email, t, announce, wait]);

  // Nothing until the build's posture is known, so a keyless build never
  // shows a form for a moment and then takes it away.
  if (!info) return <div className="settings-panel feedback-panel" />;

  if (!info.available) {
    return (
      <div className="settings-panel feedback-panel">
        <FocusableNote className="form__hint">
          {t('dialogs.settings.feedback.unavailable')}
        </FocusableNote>
      </div>
    );
  }

  const logsText =
    logs.state === 'ready'
      ? logs.text || t('dialogs.settings.feedback.logsEmpty')
      : logs.state === 'failed'
        ? t('dialogs.settings.feedback.logsFailed')
        : t('dialogs.settings.feedback.logsLoading');
  const waitingForLogs = attachLogs && logs.state === 'loading';

  return (
    <div className="settings-panel feedback-panel">
      <FocusableNote className="form__hint">
        {t('dialogs.settings.feedback.hint')}
      </FocusableNote>

      {/* noValidate: the browser's own check of the email field would stop
          the submit with a bubble of its own, in its own words; the core
          checks the address and the form says why in ours. */}
      <form
        className="form"
        noValidate
        onSubmit={(e) => {
          e.preventDefault();
          void onSend();
        }}
      >
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.feedback.messageLabel')}
          </span>
          <textarea
            ref={messageRef}
            rows={8}
            value={message}
            onChange={(e) => setMessage(e.target.value)}
          />
        </label>

        {/* The hint stands outside the label: inside, it would be part of the
            field's name as well as its description, and NVDA would say it
            twice. */}
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.feedback.emailLabel')}
          </span>
          <input
            ref={emailRef}
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            autoComplete="email"
            aria-describedby={emailHintId}
          />
        </label>
        <p id={emailHintId} className="form__hint">
          {t('dialogs.settings.feedback.emailHint')}
        </p>

        <label className="general-panel__toggle">
          <input
            type="checkbox"
            checked={attachLogs}
            onChange={(e) => onAttachLogs(e.target.checked)}
            aria-describedby={logsHintId}
          />
          <span>{t('dialogs.settings.feedback.attachLogs')}</span>
        </label>
        <p id={logsHintId} className="form__hint general-panel__toggle-hint">
          {t('dialogs.settings.feedback.attachLogsHint')}
        </p>

        {attachLogs && (
          <LogsField
            label={t('dialogs.settings.feedback.logsLabel')}
            value={logsText}
          />
        )}

        <FocusableNote className="form__hint">
          {t('dialogs.settings.feedback.system', { system: info.system })}
        </FocusableNote>

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}
        {/* Spoken through the announcer; no live role, or it is said twice. */}
        {notice && <p className="form__hint">{notice}</p>}

        <div className="form__actions">
          <button
            type="submit"
            className="form__action form__action--primary"
            aria-disabled={busy || waitingForLogs || undefined}
          >
            {busy
              ? t('dialogs.settings.feedback.sending')
              : t('dialogs.settings.feedback.send')}
          </button>
        </div>
      </form>
      {waitDialog}
    </div>
  );
}
