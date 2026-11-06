import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';

import { ReadOnlyText } from './ReadOnlyText';

/**
 * A multi-line value to read, not change: an editable textarea marked
 * `aria-readonly`, so NVDA stays in focus mode and follows the caret line by
 * line, and the field turns every change back itself (Toni, 2026-11-06: a
 * `readonly` textarea read only its first line even with Chromium 157's
 * caret, a `role="document"` switched modes unreliably).
 */

function field(value = 'Erste Zeile\r\n\r\nDritte Zeile\nVierte') {
  render(<ReadOnlyText label="Notizen" value={value} />);
  return screen.getByRole('textbox', { name: 'Notizen' }) as HTMLTextAreaElement;
}

describe('ReadOnlyText', () => {
  it('is a text field named by its label, read-only for the screen reader, with its lines', () => {
    const text = field();
    expect(text.tagName).toBe('TEXTAREA');
    expect(text.getAttribute('aria-readonly')).toBe('true');
    // Not the readonly attribute: NVDA follows no caret in such a field.
    expect(text.readOnly).toBe(false);
    expect(text.disabled).toBe(false);
    expect(text.value).toBe('Erste Zeile\n\nDritte Zeile\nVierte');
    expect(screen.queryByRole('document')).toBeNull();
  });

  it('refuses a change before it happens', () => {
    const text = field();
    const typed = new InputEvent('beforeinput', { inputType: 'insertText', data: 'x', cancelable: true });
    text.dispatchEvent(typed);
    expect(typed.defaultPrevented).toBe(true);
  });

  it('puts the text and the caret back after a change it could not refuse', () => {
    const text = field();
    text.setSelectionRange(6, 6);
    // A composition, a dead key say: its beforeinput cannot be cancelled.
    text.dispatchEvent(new InputEvent('beforeinput', { inputType: 'insertCompositionText', cancelable: false }));
    text.value = 'Erste ^Zeile';
    text.dispatchEvent(new InputEvent('input', { inputType: 'insertCompositionText' }));
    expect(text.value).toBe('Erste Zeile\n\nDritte Zeile\nVierte');
    expect([text.selectionStart, text.selectionEnd]).toEqual([6, 6]);
  });

  it('takes nothing by paste, cut or drop', () => {
    const text = field();
    for (const name of ['paste', 'cut', 'drop']) {
      const event = new Event(name, { cancelable: true });
      text.dispatchEvent(event);
      expect(event.defaultPrevented, name).toBe(true);
    }
  });
});
