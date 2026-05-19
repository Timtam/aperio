import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * Accessible color picker.
 *
 * Browsers' native `<input type="color">` is a black box for screen
 * readers: it announces nothing useful, intercepts no keys, and offers
 * no programmatic way for an assistive technology to choose a value.
 * Per WCAG 2.1.1 ("Keyboard"), every UI must be operable without a
 * mouse — including the color picker.
 *
 * This composite renders both:
 *
 *  - A hex text input (`#rrggbb`) as the *primary* control. It carries
 *    the accessible name, is in the tab order, and accepts any valid
 *    six-digit hex code. Invalid input is held locally until the user
 *    types something parseable.
 *  - The native swatch alongside, marked `aria-hidden` and out of the
 *    tab order. Sighted mouse users still get the OS picker; screen
 *    reader users never encounter it.
 *
 * The two inputs are kept in sync: a swatch pick updates the text and
 * commits the value; a text edit updates the swatch and commits on
 * change (only when the text is a valid hex code).
 */
export interface ColorComposerProps {
  value: string;
  onChange: (hex: string) => void;
  /** Accessible name for the text input. */
  label: string;
}

const HEX_RE = /^#[0-9a-fA-F]{6}$/;

export function ColorComposer({ value, onChange, label }: ColorComposerProps) {
  const { t } = useTranslation();
  const [text, setText] = useState(value);

  // Mirror an external value change into the text field. Important when
  // the parent rebuilds the form from a freshly fetched row, or when
  // another control (e.g. a preset palette later) changes the value.
  useEffect(() => {
    setText(value);
  }, [value]);

  const commit = (v: string) => {
    const normalized = normalize(v);
    setText(normalized);
    if (HEX_RE.test(normalized)) {
      onChange(normalized);
    }
  };

  const swatchValue = HEX_RE.test(text) ? text : '#000000';

  return (
    <div className="color-composer">
      <input
        type="text"
        className="color-composer__hex"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        aria-label={label}
        aria-describedby={
          HEX_RE.test(text) ? undefined : 'color-composer-help'
        }
        spellCheck={false}
        autoComplete="off"
        inputMode="text"
        placeholder="#000000"
      />
      <input
        type="color"
        className="color-composer__swatch"
        value={swatchValue}
        onChange={(e) => commit(e.target.value)}
        aria-hidden="true"
        tabIndex={-1}
      />
      {!HEX_RE.test(text) && (
        <span id="color-composer-help" className="sr-only">
          {t('common.colorComposer.invalidHex')}
        </span>
      )}
    </div>
  );
}

function normalize(input: string): string {
  let v = input.trim();
  if (v.length > 0 && !v.startsWith('#')) v = '#' + v;
  // Accept three-digit shorthand by expanding it.
  const short = /^#([0-9a-fA-F]{3})$/.exec(v);
  if (short) {
    const [r, g, b] = short[1];
    return ('#' + r + r + g + g + b + b).toLowerCase();
  }
  return v.toLowerCase();
}
