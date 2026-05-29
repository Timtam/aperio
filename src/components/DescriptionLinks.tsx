import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import { isCommandError, openExternalUrl } from '../api/client';
import { detectLinks } from '../util/links';

/**
 * Clickable link bar shown under a (plain-text, editable) description
 * field. Detects the URLs in the current text and offers each as a
 * real focusable button that opens in the OS browser / mail client.
 *
 * The textarea above stays the editable source of truth — the links
 * here update live as the user types. Opening goes through the
 * `open_external_url` backend command, which re-validates the scheme
 * (http/https/mailto only) because descriptions can come from
 * untrusted external invitations.
 *
 * Renders nothing when the text has no openable links, so callers can
 * drop it in unconditionally.
 */
export function DescriptionLinks({
  text,
}: {
  text: string | null | undefined;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const links = useMemo(() => detectLinks(text), [text]);

  if (links.length === 0) {
    return null;
  }

  const open = async (url: string) => {
    try {
      await openExternalUrl(url);
    } catch (err) {
      const message = isCommandError(err) ? err.message : String(err);
      announce(t('descriptionLinks.openFailed', { message }));
    }
  };

  return (
    <div className="description-links">
      <span className="description-links__label">
        {t('descriptionLinks.label')}
      </span>
      <ul className="description-links__list">
        {links.map((link) => (
          <li key={link.url}>
            <button
              type="button"
              className="description-links__item"
              onClick={() => open(link.url)}
              aria-label={t('descriptionLinks.open', { url: link.url })}
              title={link.url}
            >
              <span className="description-links__icon" aria-hidden="true">
                🔗
              </span>
              <span className="description-links__url">{link.url}</span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
