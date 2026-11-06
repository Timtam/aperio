import { useEffect, useId, useRef } from 'react';

/** React wants an onChange on a field with a value; this one changes nothing. */
function ignoreChange() {}

/**
 * A multi-line value the user may read but not change — a locked invitation's
 * description (77a, I6), a read-only address book's notes —, as a text field
 * to read line by line.
 *
 * Neither obvious way reads. A `<textarea readOnly>` gives NVDA no caret to
 * follow, so it said the first line only (Toni, 2026-10-26), and the caret
 * Chromium 157 gives read-only fields did not change that (2026-11-06). A
 * focusable `role="document"` made NVDA build a browse-mode buffer of its own
 * inside the dialog's application, and switch to it and back silently and
 * mostly not at all (2026-11-02).
 *
 * So it is an editable textarea marked `aria-readonly`: NVDA stays in focus
 * mode, as in every other field, says "schreibgeschützt", and follows the
 * caret by line, word and character; Shift+arrows select and Ctrl+C copies.
 * The field turns every change back itself. Most are refused before they
 * happen (`beforeinput`); a composition — a dead key, dictation, the emoji
 * panel — cannot be, so the text and the caret are put back after it. Native
 * listeners, because React's onBeforeInput is another event.
 */
export function ReadOnlyText({ label, value }: { label: string; value: string }) {
  const id = useId();
  const ref = useRef<HTMLTextAreaElement>(null);
  // A textarea holds its lines with "\n" alone; a value with "\r\n" would
  // never equal what it shows, and be put back on every render.
  const text = value.replace(/\r\n?/g, '\n');

  useEffect(() => {
    const field = ref.current;
    if (!field) return;
    let kept: [number, number] = [0, 0];
    const refuse = (e: Event) => {
      kept = [field.selectionStart, field.selectionEnd];
      if (e.cancelable) e.preventDefault();
    };
    const putBack = () => {
      if (field.value === text) return;
      field.value = text;
      field.setSelectionRange(kept[0], kept[1]);
    };
    const block = (e: Event) => e.preventDefault();
    field.addEventListener('beforeinput', refuse);
    field.addEventListener('input', putBack);
    field.addEventListener('compositionend', putBack);
    field.addEventListener('paste', block);
    field.addEventListener('cut', block);
    field.addEventListener('drop', block);
    return () => {
      field.removeEventListener('beforeinput', refuse);
      field.removeEventListener('input', putBack);
      field.removeEventListener('compositionend', putBack);
      field.removeEventListener('paste', block);
      field.removeEventListener('cut', block);
      field.removeEventListener('drop', block);
    };
  }, [text]);

  const lines = text.split('\n').length;
  return (
    <div className="form__field">
      <label className="form__label" htmlFor={id}>
        {label}
      </label>
      <textarea
        ref={ref}
        id={id}
        className="form__read-text"
        aria-readonly="true"
        value={text}
        onChange={ignoreChange}
        rows={Math.min(Math.max(lines, 3), 10)}
        spellCheck={false}
        autoComplete="off"
      />
    </div>
  );
}
