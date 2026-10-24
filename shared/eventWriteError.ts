// Why a save, a delete or an answer did not happen, in the reader's language.
//
// A refusal crosses several layers — the adapter, the host's error type, a
// Tauri command or the phone's native module — and every one of them carries
// only text. So the adapter starts the message with a token from
// `cal_core::WriteRefusal`, and both surfaces look the sentence up here.
// Before this, a blind user heard "forbidden: reply-only-invitation: title"
// read out in English whatever language the app ran in.

import type { WriteLeftover } from './generated/WriteLeftover';
import type { WriteRefusal } from './generated/WriteRefusal';
import {
  exceptionsLossSentences,
  parseExceptionsLoss,
  type ExceptionsLoss,
} from './exceptionsLoss';

/** The sentence each refusal reads as. Typed by the generated union, so a new
 *  refusal in the core is a compile error here and not an English sentence in
 *  a German dialog. */
const REFUSAL_KEYS: Record<WriteRefusal, string> = {
  'reply-only-invitation': 'dialogs.event.writeError.replyOnly',
  'server-refused': 'dialogs.event.writeError.serverRefused',
  'identity-unknown': 'dialogs.event.writeError.identityUnknown',
  'occurrence-not-writable': 'dialogs.event.writeError.occurrenceNotWritable',
  'unsafe-to-write': 'dialogs.event.writeError.unsafeToWrite',
  'access-not-granted': 'dialogs.event.writeError.accessNotGranted',
  'day-zone-unreadable': 'dialogs.event.writeError.dayZoneUnreadable',
  'copy-unreadable': 'dialogs.event.writeError.copyUnreadable',
  'exceptions-would-be-lost': 'dialogs.event.writeError.exceptionsWouldBeLost',
  'deletions-not-kept': 'dialogs.event.writeError.deletionsNotKept',
  'provider-busy': 'dialogs.event.writeError.providerBusy',
};

const TOKENS = Object.keys(REFUSAL_KEYS) as WriteRefusal[];

/** The same refusals as a reason alone, for a sentence that goes on to say
 *  what DID change. */
const REFUSAL_REASON_KEYS: Record<WriteRefusal, string> = {
  'reply-only-invitation': 'dialogs.event.writeError.reason.replyOnly',
  'server-refused': 'dialogs.event.writeError.reason.serverRefused',
  'identity-unknown': 'dialogs.event.writeError.reason.identityUnknown',
  'occurrence-not-writable': 'dialogs.event.writeError.reason.occurrenceNotWritable',
  'unsafe-to-write': 'dialogs.event.writeError.reason.unsafeToWrite',
  'access-not-granted': 'dialogs.event.writeError.reason.accessNotGranted',
  'day-zone-unreadable': 'dialogs.event.writeError.reason.dayZoneUnreadable',
  'copy-unreadable': 'dialogs.event.writeError.reason.copyUnreadable',
  'exceptions-would-be-lost': 'dialogs.event.writeError.reason.exceptionsWouldBeLost',
  'deletions-not-kept': 'dialogs.event.writeError.reason.deletionsNotKept',
  'provider-busy': 'dialogs.event.writeError.reason.providerBusy',
};

/** The sentence each leftover reads as (`cal_core::WriteLeftover`): something
 *  was written and may still stand. Typed by the generated union, as above. */
const LEFTOVER_KEYS: Record<WriteLeftover, string> = {
  'created-may-stand': 'dialogs.event.writeError.createdMayStand',
  'partly-saved': 'dialogs.event.writeError.partlySaved',
};

/** The same leftovers as a reason alone. */
const LEFTOVER_REASON_KEYS: Record<WriteLeftover, string> = {
  'created-may-stand': 'dialogs.event.writeError.reason.createdMayStand',
  'partly-saved': 'dialogs.event.writeError.reason.partlySaved',
};

const LEFTOVER_TOKENS = Object.keys(LEFTOVER_KEYS) as WriteLeftover[];

/** The word a step-wise write's detail starts with when the invitees may have
 *  heard of it (`cal_core::step_write::INVITED`). */
const INVITED = 'invited';

/** The reason a step-wise write gives when its steps took too long
 *  (`cal_core::step_write::OUT_OF_TIME`). */
const OUT_OF_TIME = 'time';

/** The reason a step-wise write gives when the provider kept asking it to wait
 *  (`cal_core::step_write::PROVIDER_BUSY`, decision 276). */
const PROVIDER_BUSY = 'busy';

/**
 * The detail of a write that settles in steps (`cal_core::step_detail`): its
 * reason, a machine word for the log, and whether the invitees may have heard
 * of it — an invitation went out, so a take-back sent its cancellation too
 * (decisions 212, 264), or what may stand may stand in their calendars (228).
 */
export function stepDetail(detail: string): { invited: boolean; why: string } {
  const words = detail.trim().split(/\s+/).filter(Boolean);
  const invited = words[0] === INVITED;
  return { invited, why: (invited ? words.slice(1) : words).join(' ') };
}

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

/**
 * What a failed write may have left behind, whatever error carried it: a new
 * series that could neither be completed nor removed again (decisions 203,
 * 210). It always travels as a network error, never as a refusal — a caller
 * that read it as "nothing landed" would undo the rest of a write whose new
 * part stands.
 */
export function eventWriteLeftover(
  err: unknown,
): { leftover: WriteLeftover; key: string; detail: string; invited: boolean } | null {
  const message = errorMessageText(err).trim();
  for (const leftover of LEFTOVER_TOKENS) {
    if (!message.startsWith(leftover)) continue;
    const rest = message.slice(leftover.length);
    if (rest !== '' && !rest.startsWith(':')) continue;
    const detail = rest.slice(1).trim();
    return {
      leftover,
      key: LEFTOVER_KEYS[leftover],
      detail,
      // Only a series that may stand can stand in the invitees' calendars
      // (228); a save made in part names its step, never "invited".
      invited: leftover === 'created-may-stand' && stepDetail(detail).invited,
    };
  }
  return null;
}

/**
 * Whether the provider turned the request away because it is busy (decision
 * 270), whichever error carries the token: nothing was written, and trying
 * again later may work.
 */
export function isProviderBusy(err: unknown): boolean {
  return eventWriteRefusal(err)?.refusal === 'provider-busy';
}

/**
 * The loss a refused save asks about (decisions 243-253): what it would
 * rewrite and how many occurrences it would lose; `null` for any other error.
 */
export function exceptionsLossOf(err: unknown): ExceptionsLoss | null {
  const refusal = eventWriteRefusal(err);
  return refusal?.refusal === 'exceptions-would-be-lost'
    ? parseExceptionsLoss(refusal.detail)
    : null;
}

/**
 * The codes with which a host says a write was turned down as a whole: the
 * copy on the server moved on, the account may not, the write made no sense,
 * the event is gone, the sign-in failed, the provider cannot do it.
 */
const REFUSED_CODES = new Set([
  'conflict',
  'forbidden',
  'invalid_input',
  'not_found',
  'auth',
  'unsupported',
]);

/**
 * Whether a failed write certainly changed nothing at the provider.
 *
 * A refusal token or a refusing code says so. Anything else may have reached
 * the provider before its answer was lost — a network failure above all, but
 * also a protocol error on an answer that could not be read, or an error
 * without a code. Both hosts hand every error over with its code (the phone
 * through `hostCoded`, TODO B9), so one without is an error a surface threw
 * itself. Those are never taken as "nothing happened": a caller that undoes
 * on the strength of it would undo half of a write that went through.
 */
export function writeNeverLanded(err: unknown): boolean {
  // Something stands that could not be taken back, whatever code carried it.
  if (eventWriteLeftover(err)) return false;
  if (eventWriteRefusal(err)) return true;
  const code = codedError(err)?.code;
  return code != null && REFUSED_CODES.has(code);
}

/** How the core writes each kind of error it hands over under a code
 *  (`cal_core::Error`'s display): `afterLanded` keeps it behind the step, as
 *  the core does. */
const KIND_TEXT: Record<string, string> = {
  auth: 'authentication failed',
  forbidden: 'access denied',
  not_found: 'resource not found',
  conflict: 'conflict (ETag / precondition failed)',
  network: 'network error',
  protocol: 'protocol error',
  invalid_input: 'invalid input',
  unsupported: 'operation not supported by adapter',
  internal: 'internal error',
};

/**
 * What a failed request of a save becomes when an earlier request of the same
 * save had landed (decision 277): never an error a surface reads as "nothing
 * was changed". A busy provider leaves the change partly saved; a network
 * error that may have landed stays as it is; anything else — a refusal above
 * all — becomes a network error that keeps its text behind `step`, a word for
 * the log. The same rule as `cal_core::Error::after_landed`, for the saves a
 * surface sends as several calls (a move's subtasks after their parent, a
 * cascade); both read the `after_landed` rows of `writeOutcome.json`. An
 * error without a code (one a surface threw itself) keeps its text the same
 * way.
 */
export function afterLanded(err: unknown, step: string): CodedError {
  if (isProviderBusy(err)) return { code: 'network', message: `partly-saved: ${step}` };
  const code = codedError(err)?.code;
  const message = errorMessageText(err);
  if (code === 'network' && !writeNeverLanded(err)) return { code, message };
  const kind = code == null ? null : (KIND_TEXT[code] ?? code);
  return {
    code: 'network',
    message: kind == null ? `${step}: ${message}` : `${step}: ${kind}: ${message}`,
  };
}

/**
 * A write that follows one of the same save that landed: its failure reads as
 * {@link afterLanded} says, never as "nothing was changed".
 */
export async function writeAfterLanded<T>(step: string, write: () => Promise<T>): Promise<T> {
  try {
    return await write();
  } catch (err) {
    throw afterLanded(err, step);
  }
}

/**
 * The removal of an original after its copy landed — one occurrence moved to
 * another calendar or carved out of its series (decision 277). `true` when it
 * went; `false` when a busy provider kept the original: the copy stands, and
 * the caller says where the original stayed, as any move whose original
 * stayed does (`originalKeptSentence`) — doing it again would make a second
 * copy. Any other failure is a later step's, never "nothing was changed".
 */
export async function removeOriginalAfterCopy(
  step: string,
  remove: () => Promise<unknown>,
): Promise<boolean> {
  try {
    await remove();
    return true;
  } catch (err) {
    if (isProviderBusy(err)) return false;
    throw afterLanded(err, step);
  }
}

/**
 * What one action of several writes says when some of them failed
 * (decision 277): the first failure, read as {@link afterLanded} says where
 * another write landed or may have — one whose failure is not a certain
 * "never landed", as a lost answer — and as itself where every write
 * certainly changed nothing. The Rust walks keep an answer that may have
 * landed over a later busy one for the same reason.
 */
export function failureOfWrites(
  step: string,
  failures: readonly unknown[],
  landed: boolean,
): unknown {
  const [first] = failures;
  const mayStand = landed || failures.some((err) => !writeNeverLanded(err));
  return mayStand ? afterLanded(first, step) : first;
}

/**
 * The writes one action makes together (the day-start review's "all to
 * today"): every one is attempted, side by side or, with `oneAtATime`, one
 * after the other. What landed comes back, and the failure reads as
 * {@link failureOfWrites} says — never "nothing was changed" while another
 * write landed or may have. `null` when every write landed.
 */
export async function writeAll<T>(
  step: string,
  items: readonly T[],
  write: (item: T) => Promise<unknown>,
  { oneAtATime = false }: { oneAtATime?: boolean } = {},
): Promise<{ landed: T[]; failure: unknown }> {
  let results: PromiseSettledResult<unknown>[];
  if (oneAtATime) {
    results = [];
    for (const item of items) {
      const [result] = await Promise.allSettled([write(item)]);
      results.push(result);
    }
  } else {
    results = await Promise.allSettled(items.map((item) => write(item)));
  }
  const landed = items.filter((_, i) => results[i].status === 'fulfilled');
  const failures = results
    .filter((result): result is PromiseRejectedResult => result.status === 'rejected')
    .map((result) => result.reason as unknown);
  if (failures.length === 0) return { landed, failure: null };
  return { landed, failure: failureOfWrites(step, failures, landed.length > 0) };
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
  // A save that would drop occurrences of a series, on a path that does not
  // ask: what it would rewrite and lose, and that nothing happened.
  const loss = exceptionsLossOf(err);
  if (loss) {
    return t('dialogs.event.writeError.exceptionsWouldBeLostDetail', exceptionsLossSentences(loss, t));
  }
  const leftover = eventWriteLeftover(err);
  if (leftover) return leftoverSentence(leftover, t);
  const refusal = eventWriteRefusal(err);
  if (refusal) return refusalSentence(refusal, t);
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

/**
 * Why a write failed, as a clause for a sentence of the caller's: the reason
 * `eventWriteErrorMessage` gives, without its "Nothing was changed". A split
 * whose new series could not be taken back again did change something, and
 * the sentence that says so must not say the opposite in the middle.
 */
export function eventWriteFailureReason(err: unknown, t: Translate): string {
  const leftover = eventWriteLeftover(err);
  if (leftover) return t(LEFTOVER_REASON_KEYS[leftover.leftover]);
  const refusal = eventWriteRefusal(err);
  if (refusal) {
    // A series taken back because its steps took too long, or because the
    // provider was busy, says so (decisions 213, 276), as its sentence does.
    if (refusal.refusal === 'deletions-not-kept') {
      const { why } = stepDetail(refusal.detail);
      if (why === OUT_OF_TIME) return t('dialogs.event.writeError.reason.deletionsNotKeptTime');
      if (why === PROVIDER_BUSY) return t('dialogs.event.writeError.reason.deletionsNotKeptBusy');
    }
    return t(REFUSAL_REASON_KEYS[refusal.refusal], { detail: refusal.detail });
  }
  const known = codedError(err);
  if (known?.code === 'forbidden') {
    return t('dialogs.event.writeError.reason.forbidden', { detail: errorMessageText(err) });
  }
  if (known?.code === 'conflict') {
    return t('dialogs.event.writeError.reason.changedOnServer');
  }
  return eventWriteErrorMessage(err, t);
}

/**
 * A refusal's sentence. A new series taken back after its deleted occurrences
 * could not all be deleted again (decision 203) says why in words — a machine
 * reason never reaches the reader — and, where an invitation may have gone
 * out, that its cancellation went too (212).
 */
function refusalSentence(
  refusal: { refusal: WriteRefusal; key: string; detail: string },
  t: Translate,
): string {
  if (refusal.refusal !== 'deletions-not-kept') {
    return t(refusal.key, { detail: refusal.detail });
  }
  const { invited, why } = stepDetail(refusal.detail);
  const sentence =
    why === OUT_OF_TIME
      ? t('dialogs.event.writeError.deletionsNotKeptTime')
      : why === PROVIDER_BUSY
        ? t('dialogs.event.writeError.deletionsNotKeptBusy')
        : t(refusal.key);
  return invited ? `${sentence} ${t('dialogs.event.writeError.invitationsCancelledToo')}` : sentence;
}

/** A leftover's sentence, and where the invitees may have heard of the write,
 *  that they may hold an invitation (228). */
function leftoverSentence(
  leftover: { key: string; invited: boolean },
  t: Translate,
): string {
  const sentence = t(leftover.key);
  return leftover.invited
    ? `${sentence} ${t('dialogs.event.writeError.invitationsMayHaveGone')}`
    : sentence;
}
