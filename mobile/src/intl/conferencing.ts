import type { TFunction } from 'i18next';
import { AccessibilityInfo, Linking } from 'react-native';

import {
  ALLOWED_LINK_SCHEMES,
  detectConference,
  schemeOf,
  type ConferenceLink,
} from '@aperio/shared';

/**
 * Opening a detected meeting, in one place.
 *
 * The scheme is re-validated here rather than trusted from the detector: a
 * description is external, attacker-influenceable text, and `Linking.openURL`
 * will happily hand a `javascript:` or an app-scheme URL to whatever claims it.
 * The detector filters too; this is the check that sits next to the actual
 * open, which is the one that has to hold.
 */
export async function openConference(
  conference: ConferenceLink,
  t: TFunction,
): Promise<void> {
  const scheme = schemeOf(conference.joinUrl);
  if (scheme == null || !ALLOWED_LINK_SCHEMES.has(scheme)) return;
  try {
    await Linking.openURL(conference.joinUrl);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    AccessibilityInfo.announceForAccessibility(
      t('conferencing.openFailed', { message }),
    );
  }
}

/**
 * The rotor / long-press action for joining, or `null` when the event carries
 * no meeting.
 *
 * Rows across the calendar and agenda screens build one action list that feeds
 * both the VoiceOver/TalkBack custom actions and the sighted long-press menu,
 * so a single helper keeps the entry identically worded and identically placed
 * everywhere. It leads the list: a meeting row is opened to join far more often
 * than to edit.
 */
export function joinAction(
  event: { location?: string | null; description?: string | null },
  t: TFunction,
): { action: { name: 'join'; label: string }; conference: ConferenceLink } | null {
  const conference = detectConference({
    location: event.location,
    description: event.description,
  });
  if (!conference) return null;
  return {
    conference,
    action: {
      name: 'join',
      label: t('conferencing.joinNamed', {
        provider: t(`conferencing.provider.${conference.provider}`),
      }),
    },
  };
}
