import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { detectConference } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { isCommandError, openExternalUrl } from '../api/client';

/**
 * The "join this meeting" affordance on an event.
 *
 * Works for any event with a conference, whoever created it — an Outlook
 * invitation, an eM Client one, a forwarded one from a company using something
 * else. Detection is shared with the mobile app and never reads prose labels,
 * so it does not care which language the invitation arrived in.
 *
 * Renders nothing when there is no meeting, so callers drop it in
 * unconditionally.
 *
 * ## Why the details are separate elements
 *
 * A meeting number and a password concatenated into one string are read out as
 * one run-on sentence, and a password is exactly the thing someone needs to
 * hear character by character. Each detail is therefore its own list item with
 * its own label, and the labels come from the invitation itself — Aperio does
 * not translate them, because it does not know what they mean. An invitation
 * that said "Besprechungs-ID" is read as "Besprechungs-ID".
 */
export function ConferenceSection({
  location,
  description,
}: {
  location?: string | null;
  description?: string | null;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const conference = useMemo(
    () => detectConference({ location, description }),
    [location, description],
  );

  if (!conference) {
    return null;
  }

  const providerName = t(`conferencing.provider.${conference.provider}`);
  // Details Aperio recovered itself get a translated label; the ones lifted out
  // of the invitation keep theirs verbatim.
  const derived: Array<{ label: string; value: string }> = [
    conference.meetingNumber && {
      label: t('conferencing.meetingNumber'),
      value: conference.meetingNumber,
    },
    conference.password && {
      label: t('conferencing.password'),
      value: conference.password,
    },
  ].filter((d): d is { label: string; value: string } => !!d);
  const details = derived.length > 0 ? derived : conference.labelledDetails;

  const open = async () => {
    try {
      await openExternalUrl(conference.joinUrl);
    } catch (err) {
      const message = isCommandError(err) ? err.message : String(err);
      announce(t('conferencing.openFailed', { message }));
    }
  };

  return (
    <section className="conference" aria-label={t('conferencing.label')}>
      <button
        type="button"
        className="conference__join form__action form__action--primary"
        onClick={open}
      >
        {t('conferencing.joinNamed', { provider: providerName })}
      </button>
      {details.length > 0 && (
        <dl className="conference__details">
          {details.map((detail) => (
            <div className="conference__detail" key={`${detail.label}:${detail.value}`}>
              <dt className="conference__detail-label">{detail.label}</dt>
              {/* tabIndex 0: the surrounding dialog body carries
                  role="application", where NVDA's focus-mode traversal skips
                  static text — a password nobody can reach is a password
                  nobody has. */}
              <dd
                className="conference__detail-value"
                tabIndex={0}
                aria-label={t('conferencing.detail', {
                  label: detail.label,
                  value: detail.value,
                })}
              >
                {detail.value}
              </dd>
            </div>
          ))}
        </dl>
      )}
    </section>
  );
}
