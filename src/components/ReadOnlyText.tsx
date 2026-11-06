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
  // Read by the listeners, which are set up once: a value that changes (a
  // sync refresh) is the one put back from the first frame it is shown.
  const shown = useRef(text);
  shown.current = text;

  useEffect(() => {
    const field = ref.current;
    if (!field) return;
    // The selection with its direction: one made upwards with Shift+Up must
    // grow upwards again after the text is put back.
    let kept: { start: number; end: number; direction: 'forward' | 'backward' | 'none' } = {
      start: 0,
      end: 0,
      direction: 'none',
    };
    const refuse = (e: Event) => {
      kept = {
        start: field.selectionStart,
        end: field.selectionEnd,
        direction: field.selectionDirection,
      };
      if (e.cancelable) e.preventDefault();
    };
    const putBack = () => {
      if (field.value === shown.current) return;
      field.value = shown.current;
      field.setSelectionRange(kept.start, kept.end, kept.direction);
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
  }, []);

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
        // The height follows the text (field-sizing in the style); three
        // rows where that is not supported.
        rows={3}
        spellCheck={false}
        autoComplete="off"
      />
    </div>
  );
}
