import {
  codedError,
  errorMessageText,
  eventWriteErrorMessage,
  eventWriteFailureReason,
  eventWriteLeftover,
  eventWriteRefusal,
  exceptionsLossOf,
  isProviderBusy,
  saysBusy,
} from './eventWriteError';

/**
 * The message to put in front of a person when something failed.
 *
 * A rejected FFI bridge call arrives as an `Error`, whose message Expo starts
 * with an English sentence of its own; only what follows it is the host's. A
 * rejected Tauri command arrives as a plain `{code, message}` object, which
 * read "[object Object]" before; it reads `code: message`, as the desktop's
 * own copies did. A rejected promise can carry anything at all. All of them
 * have to end up as a string, because the string is what the screen reader
 * says — and saying nothing is the one outcome that must never happen.
 *
 * Four copies of this existed (three on mobile, one about to be written on the
 * desktop) before it was one.
 */
export function errorMessage(err: unknown): string {
  if (err instanceof Error) return errorMessageText(err);
  const known = codedError(err);
  if (known) return `${known.code}: ${errorMessageText(err)}`;
  return String(err);
}

type Translate = (key: string, values?: Record<string, unknown>) => string;

/**
 * The sentence for a READ that failed — a load, a probe, a search, a check of
 * availability. A busy provider and a device without access say so in words
 * that never claim a change was undone: a read changes nothing, and "Nothing
 * was changed" would only confuse (decision 273). Anything else reads as
 * {@link errorMessage}.
 */
export function readErrorMessage(err: unknown, t: Translate): string {
  const refusal = eventWriteRefusal(err);
  if (refusal?.refusal === 'provider-busy') return t('common.providerBusy');
  if (refusal?.refusal === 'access-not-granted') return t('common.accessNotGranted');
  return errorMessage(err);
}

/**
 * Why a read failed, as a clause for a frame of the caller's ("… could not be
 * loaded: {reason}"): the provider is busy, the device has no access, or what
 * the host said.
 */
export function readFailureReason(err: unknown, t: Translate): string {
  const refusal = eventWriteRefusal(err);
  if (refusal?.refusal === 'provider-busy') {
    return t('dialogs.event.writeError.reason.providerBusy');
  }
  if (refusal?.refusal === 'access-not-granted') {
    return t('dialogs.event.writeError.reason.accessNotGranted');
  }
  return errorMessage(err);
}

/**
 * A read failure said in the frame `key`, with its reason in the frame's
 * `slot`. A busy provider adds "Try again later." — never how long (decisions
 * 272, 278).
 */
export function readFailureSentence(
  key: string,
  err: unknown,
  t: Translate,
  slot = 'reason',
  values: Record<string, unknown> = {},
): string {
  const sentence = t(key, { ...values, [slot]: readFailureReason(err, t) }).trim();
  if (!isProviderBusy(err)) return sentence;
  const ended = /[.!?…]$/.test(sentence) ? sentence : `${sentence}.`;
  return `${ended} ${t('common.tryAgainLater')}`;
}

/**
 * {@link errorMessage} for a failed write: a refusal reads as its sentence, and
 * so does a leftover (`cal_core::WriteLeftover`: a new series that may stand,
 * decisions 203, 210).
 *
 * A refusal crosses the native module as a token at the start of the message
 * (`cal_core::WriteRefusal`), and only the event editor and deletes looked it
 * up; every other write said it raw, in English, with Expo's wrapper in
 * front — "Calling the 'updateTaskJson' function has failed → Caused by:
 * access-not-granted: reminders: Denied". Anything that is neither a refusal
 * nor a leftover comes out exactly as before.
 */
export function writeErrorMessage(err: unknown, t: Translate): string {
  // A save that would drop occurrences of a series says what and how many, as
  // the event editors do (decisions 243-253).
  if (exceptionsLossOf(err)) return eventWriteErrorMessage(err, t);
  // A new series that could be neither completed nor removed again, and one
  // that was removed again, say so as the editors do (decisions 203, 210).
  if (eventWriteLeftover(err)) return eventWriteErrorMessage(err, t);
  const refusal = eventWriteRefusal(err);
  if (refusal?.refusal === 'deletions-not-kept') return eventWriteErrorMessage(err, t);
  return refusal ? t(refusal.key, { detail: refusal.detail }) : errorMessage(err);
}

/**
 * The sentence for a write that failed, in a frame of the caller's ("That
 * could not be undone: {reason}"; decision 273). Where part of it landed, the
 * leftover's own sentence says what stands (277); anything else is its reason
 * in the frame, and a busy provider adds "Try again later." (272).
 */
export function writeFailureSentence(
  key: string,
  err: unknown,
  t: Translate,
  slot = 'reason',
  values: Record<string, unknown> = {},
): string {
  if (eventWriteLeftover(err)) return eventWriteErrorMessage(err, t);
  const sentence = t(key, { ...values, [slot]: eventWriteFailureReason(err, t) }).trim();
  return withLaterForBusy(sentence, err, t);
}

/**
 * `sentence` with "Try again later." after it where `err` says the provider
 * is busy ({@link saysBusy}; decisions 272, 278): a frame that embeds the
 * reason alone does not say it.
 */
export function withLaterForBusy(sentence: string, err: unknown, t: Translate): string {
  if (!saysBusy(err)) return sentence;
  const ended = /[.!?…]$/.test(sentence) ? sentence : `${sentence}.`;
  return `${ended} ${t('common.tryAgainLater')}`;
}

/**
 * The sentence for a meeting that could not be created, removed or taken over
 * (decision 273): {@link writeFailureSentence} in the meeting's frame. Where
 * part of it landed — the meeting exists, the event does not carry it — the
 * leftover's own sentence says what stands.
 */
export function meetingFailureSentence(err: unknown, t: Translate): string {
  return writeFailureSentence('conferencing.meetingFailed', err, t, 'message');
}
