import { eventWriteErrorMessage, eventWriteRefusal, exceptionsLossOf } from './eventWriteError';

/**
 * The message to put in front of a person when something failed.
 *
 * A rejected Tauri command or FFI bridge call arrives as an `Error`; a rejected
 * promise can carry anything at all. Both have to end up as a string, because
 * the string is what the screen reader says — and saying nothing is the one
 * outcome that must never happen.
 *
 * Four copies of this existed (three on mobile, one about to be written on the
 * desktop) before it was one.
 */
export function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

type Translate = (key: string, values?: Record<string, unknown>) => string;

/**
 * {@link errorMessage} for a failed write: a refusal reads as its sentence.
 *
 * A refusal crosses the native module as a token at the start of the message
 * (`cal_core::WriteRefusal`), and only the event editor and deletes looked it
 * up; every other write said it raw, in English, with Expo's wrapper in
 * front — "Calling the 'updateTaskJson' function has failed → Caused by:
 * access-not-granted: reminders: Denied". Anything that is not a refusal
 * comes out exactly as before.
 */
export function writeErrorMessage(err: unknown, t: Translate): string {
  // A save that would drop occurrences of a series says what and how many, as
  // the event editors do (decisions 243-253).
  if (exceptionsLossOf(err)) return eventWriteErrorMessage(err, t);
  const refusal = eventWriteRefusal(err);
  return refusal ? t(refusal.key, { detail: refusal.detail }) : errorMessage(err);
}
