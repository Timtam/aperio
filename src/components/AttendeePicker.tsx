import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { formatAttendee } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { searchContacts } from '../api/client';
import type { Contact } from '../api/types';

/**
 * Attendees picker for the EventDialog (DESIGN.md §10.4).
 *
 * Surface: chips for the current attendees plus a combobox input
 * that autocompletes against the user's contacts. Picks an
 * existing contact (Enter / click) or accepts a free-form
 * email-shaped string (Tab / Enter on an unmatched value) so
 * non-contact recipients still get on the list.
 *
 * ARIA shape (W3C ARIA APG combobox-with-listbox-popup, autoselect
 * variant):
 *  - The input has `role="combobox"`, `aria-autocomplete="list"`,
 *    `aria-controls` pointing at the popup, `aria-expanded`
 *    tracking whether suggestions are visible, and
 *    `aria-activedescendant` referencing the currently-highlighted
 *    option.
 *  - The popup is a `role="listbox"` with `role="option"` rows.
 *  - The current attendees render as a `role="list"` of
 *    `role="listitem"` chips, each with a delete button whose
 *    accessible name is "Max remove" (single-action confirm:
 *    backend has no concept of attendee soft-delete).
 *
 * Keyboard:
 *  - ArrowDown / ArrowUp move through the popup
 *  - Enter picks the highlighted suggestion (or commits the raw
 *    text when none is highlighted)
 *  - Escape closes the popup without committing
 *  - Backspace on an empty input removes the last chip
 *  - Tab closes the popup; if a suggestion is highlighted it's
 *    committed first (autoselect behaviour the APG calls out as
 *    the friendlier default for email pickers)
 */

const SEARCH_DEBOUNCE_MS = 180;
const MAX_SUGGESTIONS = 8;

export interface AttendeePickerProps {
  /** Current attendees — each entry is an opaque string that
   *  round-trips through the backend's `attendees` field. The
   *  picker uses `formatAttendee(Contact)` for picks and the raw
   *  string for free-form entries. */
  value: string[];
  onChange: (next: string[]) => void;
  /** Optional id used to label the combobox via `aria-labelledby`.
   *  EventDialog wraps the picker inside a labelled fieldset, so
   *  this is normally enough. */
  labelledBy?: string;
}

export function AttendeePicker({
  value,
  onChange,
  labelledBy,
}: AttendeePickerProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const id = useId();
  const inputRef = useRef<HTMLInputElement>(null);

  const [query, setQuery] = useState('');
  // Debounced query — what actually drives the search command.
  // 180ms is the sweet spot in our local tests: short enough that
  // the user doesn't perceive it as laggy, long enough that
  // typing "max@example.com" doesn't trigger 17 round-trips.
  const [debouncedQuery, setDebouncedQuery] = useState('');
  useEffect(() => {
    const handle = window.setTimeout(
      () => setDebouncedQuery(query),
      SEARCH_DEBOUNCE_MS,
    );
    return () => window.clearTimeout(handle);
  }, [query]);

  const [suggestions, setSuggestions] = useState<Contact[]>([]);
  const [highlightedIndex, setHighlightedIndex] = useState<number>(-1);
  const [open, setOpen] = useState(false);

  // Run the search whenever the debounced needle changes. Empty /
  // whitespace queries close the popup without a wire hit.
  useEffect(() => {
    let cancelled = false;
    const trimmed = debouncedQuery.trim();
    if (trimmed.length < 1) {
      setSuggestions([]);
      setOpen(false);
      setHighlightedIndex(-1);
      return;
    }
    searchContacts(trimmed)
      .then((rows) => {
        if (cancelled) return;
        // Filter out contacts that are already on the chip list —
        // the picker shouldn't suggest someone the user just added.
        const taken = new Set(value);
        const filtered = rows
          .filter((c) => !taken.has(formatAttendee(c)))
          .slice(0, MAX_SUGGESTIONS);
        setSuggestions(filtered);
        setOpen(filtered.length > 0);
        // Auto-highlight the first hit — APG autoselect pattern.
        // Saves a keypress for the common case ("type max →
        // Enter").
        setHighlightedIndex(filtered.length > 0 ? 0 : -1);
      })
      .catch((err) => {
        if (cancelled) return;
        // eslint-disable-next-line no-console
        console.warn('search_contacts failed', err);
        setSuggestions([]);
        setOpen(false);
      });
    return () => {
      cancelled = true;
    };
  }, [debouncedQuery, value]);

  const optionId = useCallback(
    (i: number) => `${id}-opt-${i}`,
    [id],
  );

  const addAttendee = useCallback(
    (entry: string) => {
      const trimmed = entry.trim();
      if (!trimmed) return;
      if (value.includes(trimmed)) {
        // Soft no-op + SR feedback — the user just confirmed an
        // already-listed attendee; saying so beats silently
        // swallowing the keystroke.
        announce(t('dialogs.event.attendees.alreadyOnList'));
        return;
      }
      onChange([...value, trimmed]);
      announce(t('dialogs.event.attendees.added', { name: trimmed }));
      setQuery('');
      setDebouncedQuery('');
      setSuggestions([]);
      setOpen(false);
      setHighlightedIndex(-1);
    },
    [value, onChange, announce, t],
  );

  const removeAt = useCallback(
    (index: number) => {
      const next = value.slice();
      const [removed] = next.splice(index, 1);
      onChange(next);
      if (removed) {
        announce(t('dialogs.event.attendees.removed', { name: removed }));
      }
      // Focus stays inside the picker — return to the input so the
      // user can keep adding without reaching for the mouse.
      queueMicrotask(() => inputRef.current?.focus());
    },
    [value, onChange, announce, t],
  );

  const commitCurrentInput = useCallback(() => {
    if (highlightedIndex >= 0 && suggestions[highlightedIndex]) {
      addAttendee(formatAttendee(suggestions[highlightedIndex]));
      return;
    }
    const raw = query.trim();
    if (!raw) return;
    addAttendee(raw);
  }, [highlightedIndex, suggestions, query, addAttendee]);

  const handleKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLInputElement>) => {
      switch (event.key) {
        case 'ArrowDown':
          if (suggestions.length === 0) return;
          event.preventDefault();
          setOpen(true);
          setHighlightedIndex((i) =>
            Math.min(i + 1, suggestions.length - 1),
          );
          return;
        case 'ArrowUp':
          if (suggestions.length === 0) return;
          event.preventDefault();
          setOpen(true);
          setHighlightedIndex((i) => Math.max(i - 1, 0));
          return;
        case 'Enter':
          // Prevent the surrounding form from submitting. The
          // picker is inside the EventDialog's form, and a stray
          // Enter on the input would save-and-close instead of
          // adding the attendee — which is almost certainly NOT
          // what the user means while typing in the picker.
          event.preventDefault();
          commitCurrentInput();
          return;
        case 'Escape':
          if (open) {
            // Eat the keystroke so the surrounding Modal doesn't
            // close on the same Escape.
            event.stopPropagation();
            setOpen(false);
            setHighlightedIndex(-1);
          }
          return;
        case 'Tab':
          // Autoselect-on-tab: commit the highlighted suggestion
          // before focus leaves, then close the popup. The user
          // expects "type-tab" to confirm; the APG autoselect
          // pattern recommends this for email-shaped pickers.
          if (highlightedIndex >= 0 && suggestions[highlightedIndex]) {
            addAttendee(formatAttendee(suggestions[highlightedIndex]));
          }
          setOpen(false);
          return;
        case 'Backspace':
          if (query.length === 0 && value.length > 0) {
            event.preventDefault();
            removeAt(value.length - 1);
          }
          return;
        default:
          return;
      }
    },
    [
      suggestions,
      open,
      highlightedIndex,
      query,
      value,
      addAttendee,
      removeAt,
      commitCurrentInput,
    ],
  );

  const popupId = `${id}-popup`;

  // Picker is a single, self-contained block — no portal, sits
  // where the dialog lays it out. The popup is absolutely
  // positioned below the input via CSS.
  const showPopup = open && suggestions.length > 0;

  // Stable input-aria-activedescendant: only set when the popup is
  // open AND a real option is highlighted, otherwise NVDA reads
  // the option name on every focus event even when the listbox
  // isn't visible.
  const ariaActiveDescendant = useMemo(() => {
    if (!showPopup || highlightedIndex < 0) return undefined;
    return optionId(highlightedIndex);
  }, [showPopup, highlightedIndex, optionId]);

  return (
    <div className="attendee-picker" aria-labelledby={labelledBy}>
      {value.length > 0 && (
        <ul
          role="list"
          className="attendee-picker__chips"
          aria-label={t('dialogs.event.attendees.chipsLabel')}
        >
          {value.map((entry, i) => (
            <li
              key={`${entry}-${i}`}
              role="listitem"
              className="attendee-picker__chip"
            >
              <span className="attendee-picker__chip-text">{entry}</span>
              <button
                type="button"
                className="attendee-picker__chip-remove"
                onClick={() => removeAt(i)}
                aria-label={t('dialogs.event.attendees.removeLabel', {
                  name: entry,
                })}
              >
                <span aria-hidden="true">×</span>
              </button>
            </li>
          ))}
        </ul>
      )}

      <div className="attendee-picker__combo">
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={handleKeyDown}
          onFocus={() => {
            if (suggestions.length > 0) setOpen(true);
          }}
          onBlur={() => {
            // Close the popup on blur, but don't commit — the user
            // may have clicked a chip's delete button or the
            // surrounding form. Click on an option fires before
            // blur, so the suggestion add path runs first.
            setOpen(false);
          }}
          placeholder={t('dialogs.event.attendees.placeholder')}
          role="combobox"
          aria-controls={popupId}
          aria-expanded={showPopup}
          aria-autocomplete="list"
          aria-activedescendant={ariaActiveDescendant}
          autoComplete="off"
          spellCheck={false}
          className="attendee-picker__input"
        />

        {showPopup && (
          <ul
            id={popupId}
            role="listbox"
            className="attendee-picker__popup"
            aria-label={t('dialogs.event.attendees.popupLabel')}
          >
            {suggestions.map((c, i) => {
              const focused = i === highlightedIndex;
              const email = c.emails[0];
              return (
                <li
                  key={c.id}
                  id={optionId(i)}
                  role="option"
                  aria-selected={focused}
                  // Mouse-down (not click) so the option commits
                  // BEFORE the input's blur handler fires and
                  // closes the popup. Otherwise clicking with the
                  // mouse never lands.
                  onMouseDown={(e) => {
                    e.preventDefault();
                    addAttendee(formatAttendee(c));
                  }}
                  className={
                    'attendee-picker__option' +
                    (focused ? ' attendee-picker__option--focused' : '')
                  }
                >
                  <span className="attendee-picker__option-name">
                    {c.display_name}
                  </span>
                  {email && (
                    <span className="attendee-picker__option-email">
                      {email}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
