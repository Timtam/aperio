// The feedback form (host_core::feedback). The shapes are generated from the
// Rust; this file holds what both forms say about an outcome, so the desktop
// panel and the phone screen cannot word one ending two ways.

import type { FeedbackFailure } from './generated/FeedbackFailure';
import type { FeedbackOutcome } from './generated/FeedbackOutcome';

export type { FeedbackDraft } from './generated/FeedbackDraft';
export type { FeedbackFailure } from './generated/FeedbackFailure';
export type { FeedbackInfo } from './generated/FeedbackInfo';
export type { FeedbackOutcome } from './generated/FeedbackOutcome';
export type { HostDetails } from './generated/HostDetails';

type Translate = (key: string) => string;

/** One sentence per reason the core can give. A `Record`, so a reason added
 *  in Rust is a compile error here until it has its sentence. */
const FAILURE_KEYS: Record<FeedbackFailure, string> = {
  unavailable: 'dialogs.settings.feedback.failure.unavailable',
  empty_message: 'dialogs.settings.feedback.failure.emptyMessage',
  invalid_email: 'dialogs.settings.feedback.failure.invalidEmail',
  too_large: 'dialogs.settings.feedback.failure.tooLarge',
  network: 'dialogs.settings.feedback.failure.network',
  busy: 'dialogs.settings.feedback.failure.busy',
  service: 'dialogs.settings.feedback.failure.service',
  rejected: 'dialogs.settings.feedback.failure.rejected',
};

/** What the form says after a send: thanks, or why not. */
export function feedbackOutcomeSentence(outcome: FeedbackOutcome, t: Translate): string {
  return outcome.outcome === 'sent'
    ? t('dialogs.settings.feedback.sent')
    : t(FAILURE_KEYS[outcome.reason]);
}

/** The field a failure is about, for the form to put focus back on, or
 *  `null` when it is about no field the user can change. */
export function feedbackFailureField(outcome: FeedbackOutcome): 'message' | 'email' | null {
  if (outcome.outcome === 'sent') return null;
  switch (outcome.reason) {
    case 'empty_message':
    case 'too_large':
      return 'message';
    case 'invalid_email':
      return 'email';
    default:
      return null;
  }
}
