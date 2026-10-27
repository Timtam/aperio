import { useId } from 'react';

/**
 * A multi-line value the user may read but not change — a locked invitation's
 * description (77a, I6), a read-only address book's notes —, as text to read.
 * A read-only textarea is no editable text to Chromium, so NVDA followed no
 * caret in it and said its first line only (Toni, 2026-10-26). A focusable
 * document is how a screen reader reads inside an application: NVDA switches
 * to browse mode on entering it and reads it line by line with the arrow
 * keys; Tab leaves it, back to focus mode. One block per line, so the value's
 * breaks and blank lines stay lines.
 */
export function ReadOnlyText({ label, value }: { label: string; value: string }) {
  const labelId = useId();
  return (
    <div className="form__field">
      <span className="form__label" id={labelId}>
        {label}
      </span>
      <div role="document" tabIndex={0} aria-labelledby={labelId} className="form__read-text">
        {value.split(/\r?\n/).map((line, index) => (
          <div key={index}>{line === '' ? <br /> : line}</div>
        ))}
      </div>
    </div>
  );
}
