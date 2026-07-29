import { useCallback, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { isCommandError, openExternalUrl } from '../api/client';
import { detectLinks } from '../util/links';

/**
 * Clickable link bar shown under a (plain-text, editable) description
 * field. Detects the URLs in the current text and offers each as a
 * real button that opens in the OS browser / mail client / dialler.
 *
 * The textarea above stays the editable source of truth — the links
 * here update live as the user types. Opening goes through the
 * `open_external_url` backend command, which re-validates the scheme
 * because descriptions can come from untrusted external invitations.
 *
 * Renders nothing when the text has no openable links, so callers can
 * drop it in unconditionally.
 *
 * ## One tab stop, however many links
 *
 * A `role="toolbar"` with a roving tabindex: Tab reaches the group once, arrow
 * keys move between the links inside it, Enter or Space opens one. Every link
 * used to be its own tab stop, which was tolerable when a description held one
 * — and stopped being tolerable the moment a meeting block put a join link, a
 * dial-in list and a global-numbers page in the same field, three stops between
 * the description and the next control.
 *
 * Toolbar rather than listbox, though the app uses listboxes for its
 * single-tab-stop pickers elsewhere: a listbox means SELECTION, and these are
 * ACTIONS. Each item stays a real button, which is what it does.
 *
 * ## What a link is called
 *
 * A real invitation names its links — `Join the meeting: https://…`. Where the
 * description says so, that label becomes the item's accessible name, and the
 * URL follows as detail. A ninety-character Webex link read out in full is a
 * miserable way to find out you are on the join link.
 */
export function DescriptionLinks({
  text,
}: {
  text: string | null | undefined;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const links = useMemo(() => detectLinks(text), [text]);
  // Which item Tab lands on. Kept as an index rather than a url so it survives
  // the user editing the description underneath — a stale url would leave the
  // group with no tabbable child at all, and then Tab would skip it entirely.
  const [active, setActive] = useState(0);
  const itemsRef = useRef<(HTMLButtonElement | null)[]>([]);

  const focusItem = useCallback(
    (index: number) => {
      setActive(index);
      itemsRef.current[index]?.focus();
    },
    [setActive],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
      const last = links.length - 1;
      let next: number | null = null;
      if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
        next = index === last ? 0 : index + 1;
      } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
        next = index === 0 ? last : index - 1;
      } else if (e.key === 'Home') {
        next = 0;
      } else if (e.key === 'End') {
        next = last;
      }
      if (next === null) return;
      e.preventDefault();
      focusItem(next);
    },
    [focusItem, links.length],
  );

  if (links.length === 0) {
    return null;
  }

  // The description can shrink under an index we are still holding.
  const tabbable = Math.min(active, links.length - 1);

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
      <span className="description-links__label" id="description-links-label">
        {t('descriptionLinks.label')}
      </span>
      <div
        className="description-links__list"
        role="toolbar"
        aria-orientation="horizontal"
        aria-labelledby="description-links-label"
      >
        {links.map((link, index) => (
          <button
            key={link.url}
            ref={(el) => {
              itemsRef.current[index] = el;
            }}
            type="button"
            className="description-links__item"
            tabIndex={index === tabbable ? 0 : -1}
            onKeyDown={(e) => onKeyDown(e, index)}
            onFocus={() => setActive(index)}
            onClick={() => void open(link.url)}
            aria-label={
              link.label
                ? t('descriptionLinks.openNamed', {
                    label: link.label,
                    url: link.url,
                  })
                : t('descriptionLinks.open', { url: link.url })
            }
            title={link.url}
          >
            <span className="description-links__icon" aria-hidden="true">
              🔗
            </span>
            <span className="description-links__url">
              {link.label ?? link.url}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}
