import { AccessibilityInfo } from 'react-native';

/**
 * A sentence that has to be heard whole, and the background cues that would
 * otherwise cut it off.
 *
 * An iOS announcement interrupts whatever VoiceOver is saying. The sentence
 * about the device calendars (after the start prompt, decision 166, the
 * account's "Allow access…", or a grant given in the settings) ends, on a
 * partial grant, with what holds the other store back — and the refresh cues
 * ("Externe Daten werden aktualisiert …", "… aktualisiert.") and the
 * refresh-errors warning arrive on their own clock, often in the middle of
 * it. While a sentence is held, those cues queue behind it instead.
 */
let held = 0;

/** Run `say` with other announcements queued behind it. */
export async function holdingSpeech<T>(say: () => Promise<T>): Promise<T> {
  held += 1;
  try {
    return await say();
  } finally {
    held -= 1;
  }
}

/** Announce a background cue: interrupting as always, but queued behind a
 *  held sentence. */
export function announceAround(message: string): void {
  if (held > 0) {
    AccessibilityInfo.announceForAccessibilityWithOptions(message, { queue: true });
  } else {
    AccessibilityInfo.announceForAccessibility(message);
  }
}
