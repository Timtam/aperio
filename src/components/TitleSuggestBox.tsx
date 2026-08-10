import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';

/**
 * The title field of the two editors, with what has been written before.
 *
 * Most appointments and most tasks are not new; they are the same thing again.
 * So typing offers the earlier ones by name, and accepting one fills the rest
 * of the editor from it — the length, the repetition, the description, the
 * reminders. What it never fills is the day: that is what makes this a new
 * entry, and it comes from wherever the user opened the editor.
 *
 * The ARIA is the one the AttendeePicker already uses, because a second
 * spelling of "combobox" in the same app is a second set of bugs: the input
 * carries `role="combobox"` with `aria-expanded` and `aria-activedescendant`,
 * the popup is a `listbox` of `option`s, Arrow keys move the highlight, Enter
 * accepts, Escape closes without accepting.
 *
 * Two rules the offers follow, both deliberate:
 *   - typing NEVER changes anything but the title. An offer is applied when it
 *     is accepted, not when it is highlighted, so arrowing through the list
 *     cannot quietly rewrite the form under someone reading it;
 *   - Escape closes the list and nothing else. It must not reach the dialog
 *     and discard the edit — that is the one keystroke a screen-reader user
 *     presses to get out of a popup.
 */
export interface TitleSuggestOption {
  /** Stable id of the earlier item. */
  id: string;
  /** Its title, as it was written. */
  title: string;
  /** A short line saying where it comes from — the calendar or the list. */
  hint?: string;
}

export interface TitleSuggestBoxProps {
  label: string;
  value: string;
  onChange: (value: string) => void;
  /** Offers for what is typed. Empty ⇒ a plain text field, no popup. */
  options: readonly TitleSuggestOption[];
  /** Fill the rest of the editor from this earlier item. */
  onAccept: (id: string) => void;
  inputRef?: React.RefObject<HTMLInputElement>;
  required?: boolean;
}

export function TitleSuggestBox({
  label,
  value,
  onChange,
  options,
  onAccept,
  inputRef,
  required,
}: TitleSuggestBoxProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const ownRef = useRef<HTMLInputElement>(null);
  const ref = inputRef ?? ownRef;
  const popupId = useId();
  const optionIdBase = useId();
  const [open, setOpen] = useState(false);
  const [highlighted, setHighlighted] = useState(-1);
  const showPopup = open && options.length > 0;
  const optionId = (i: number) => `${optionIdBase}-${i}`;

  // A fresh list is a fresh choice: an index left over from the previous one
  // would point at whatever now happens to sit there.
  useEffect(() => {
    setHighlighted(-1);
    if (options.length > 0) setOpen(true);
  }, [options]);

  // How many there are, once they arrive. A popup that opens silently is a
  // popup a screen-reader user never learns about — and the count is what
  // tells them whether it is worth arrowing into.
  const spoken = useRef(0);
  useEffect(() => {
    if (!showPopup || options.length === spoken.current) return;
    spoken.current = options.length;
    announce(t('suggestions.count', { count: options.length }));
  }, [showPopup, options.length, announce, t]);
  useEffect(() => {
    if (!showPopup) spoken.current = 0;
  }, [showPopup]);

  const accept = useCallback(
    (index: number) => {
      const option = options[index];
      if (!option) return;
      setOpen(false);
      setHighlighted(-1);
      onAccept(option.id);
      announce(t('suggestions.applied', { title: option.title }));
      ref.current?.focus();
    },
    [options, onAccept, announce, t, ref],
  );

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape') {
      if (!showPopup) return;
      // Stopped here: the dialog's own Escape closes the editor, and losing an
      // edit because a suggestion list was open would be the app punishing the
      // one keystroke that means "get me out of this popup".
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      setHighlighted(-1);
      return;
    }
    if (!showPopup) {
      if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && options.length > 0) {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHighlighted((i) => (i + 1) % options.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHighlighted((i) => (i <= 0 ? options.length - 1 : i - 1));
    } else if (e.key === 'Enter' && highlighted >= 0) {
      // Only with something highlighted: Enter on a typed title must still
      // submit the form, which is how everyone creates a one-off.
      e.preventDefault();
      accept(highlighted);
    }
  };

  return (
    <div className="form__field title-suggest">
      <label className="form__field title-suggest__label">
        <span className="form__label">{label}</span>
        <input
          ref={ref}
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          onBlur={() => setOpen(false)}
          required={required}
          autoComplete="off"
          role="combobox"
          aria-controls={popupId}
          aria-expanded={showPopup}
          aria-autocomplete="list"
          aria-activedescendant={
            showPopup && highlighted >= 0 ? optionId(highlighted) : undefined
          }
        />
      </label>
      {showPopup && (
        <ul
          id={popupId}
          role="listbox"
          className="title-suggest__popup"
          aria-label={t('suggestions.popupLabel')}
        >
          {options.map((option, i) => (
            <li
              key={option.id}
              id={optionId(i)}
              role="option"
              aria-selected={i === highlighted}
              // Mouse-DOWN, not click: the input's blur closes the popup, and
              // a click would land after it had already gone.
              onMouseDown={(e) => {
                e.preventDefault();
                accept(i);
              }}
              className={
                'title-suggest__option' +
                (i === highlighted ? ' title-suggest__option--focused' : '')
              }
            >
              <span className="title-suggest__option-title">{option.title}</span>
              {option.hint && (
                <span className="title-suggest__option-hint">{option.hint}</span>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
