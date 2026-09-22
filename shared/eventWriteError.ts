// Why a save, a delete or an answer did not happen, in the reader's language.
//
// A refusal crosses several layers — the adapter, the host's error type, a
// Tauri command or the phone's native module — and every one of them carries
// only text. So the adapter starts the message with a token from
// `cal_core::WriteRefusal`, and both surfaces look the sentence up here.
// Before this, a blind user heard "forbidden: reply-only-invitation: title"
// read out in English whatever language the app ran in.

import type { WriteRefusal } from './generated/WriteRefusal';

/** The sentence each refusal reads as. Typed by the generated union, so a new
 *  refusal in the core is a compile error here and not an English sentence in
 *  a German dialog. */
const REFUSAL_KEYS: Record<WriteRefusal, string> = {
  'reply-only-invitation': 'dialogs.event.writeError.replyOnly',
  'server-refused': 'dialogs.event.writeError.serverRefused',
  'identity-unknown': 'dialogs.event.writeError.identityUnknown',
  'occurrence-not-writable': 'dialogs.event.writeError.occurrenceNotWritable',
};

const TOKENS = Object.keys(REFUSAL_KEYS) as WriteRefusal[];

/** An error as the hosts hand it over: a code and a message. */
export interface CodedError {
  code: string;
  message: string;
}

/** The code and message a host error carries, or `null` for any other error. */
export function codedError(err: unknown): CodedError | null {
  if (typeof err !== 'object' || err === null) return null;
  const candidate = err as { code?: unknown; message?: unknown };
  return typeof candidate.code === 'string' && typeof candidate.message === 'string'
    ? { code: candidate.code, message: candidate.message }
    : null;
}

/** What Expo puts before the error a native module threw. */
const EXPO_CAUSE = '\u2192 Caused by: ';

/**
 * The text of any error, as a host, an `Error` or a bare value carries it.
 *
 * On the phone, Expo wraps whatever a native module threw in a sentence of its
 * own — "Call to function 'CalFfi.…' has been rejected." on Android, "Calling
 * the '…' function has failed" on iOS — followed by `→ Caused by: ` and the
 * module's own message. Only that last part is the host's: a refusal token
 * starts it, and read with the wrapper in front, no refusal was ever found on
 * the phone and a blind user heard Expo's English instead.
 */
export function errorMessageText(err: unknown): string {
  const known = codedError(err);
  const raw = known ? known.message : err instanceof Error ? err.message : String(err);
  const cause = raw.lastIndexOf(EXPO_CAUSE);
  return cause === -1 ? raw : raw.slice(cause + EXPO_CAUSE.length);
}

/**
 * The refusal a message names, whatever error carried it.
 *
 * The token is read from the start of the message and not from the code: a
 * refusal travels as whichever error its layer already used, and an unknown
 * identity, for one, is a network error (the server could not be asked).
 */
export function eventWriteRefusal(
  err: unknown,
): { refusal: WriteRefusal; key: string; detail: string } | null {
  const message = errorMessageText(err).trim();
  for (const refusal of TOKENS) {
    if (!message.startsWith(refusal)) continue;
    const rest = message.slice(refusal.length);
    if (rest === '') return { refusal, key: REFUSAL_KEYS[refusal], detail: '' };
    if (rest.startsWith(':')) {
      return { refusal, key: REFUSAL_KEYS[refusal], detail: rest.slice(1).trim() };
    }
  }
  return null;
}

type Translate = (key: string, values?: Record<string, unknown>) => string;

/**
 * What to tell the user about a failed event write.
 *
 * In order: a refusal token in the message; a plain "forbidden" from a
 * provider that gave no token; a conflict, which means the copy on the server
 * moved on; anything else keeps today's `code: message`, so nothing is ever
 * swallowed.
 */
export function eventWriteErrorMessage(err: unknown, t: Translate): string {
  const refusal = eventWriteRefusal(err);
  if (refusal) {
    return t(refusal.key, { detail: refusal.detail });
  }
  const known = codedError(err);
  if (known?.code === 'forbidden') {
    return t('dialogs.event.writeError.forbidden', { detail: errorMessageText(err) });
  }
  if (known?.code === 'conflict') {
    return t('dialogs.event.writeError.changedOnServer');
  }
  if (known) return `${known.code}: ${errorMessageText(err)}`;
  // A plain Error reads as "Error: …" when stringified; the prefix says
  // nothing to a reader.
  return errorMessageText(err).replace(/^Error:\s*/, '');
}
