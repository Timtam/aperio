import { afterEach, describe, expect, it, vi } from 'vitest';

import { describe as where, traceFocus } from './focusTrace';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = '';
});

describe('traceFocus', () => {
  it('writes each focus change with where it is, and stops after its time', () => {
    vi.useFakeTimers();
    const info = vi.spyOn(console, 'info').mockImplementation(() => {});
    document.body.innerHTML =
      '<div id="app-root" role="application"><div role="grid" tabindex="0" class="week-grid"></div></div>' +
      '<div role="dialog"><button id="save">Speichern</button></div>';
    traceFocus('import', 1000);
    (document.querySelector('[role="grid"]') as HTMLElement).focus();
    (document.getElementById('save') as HTMLElement).focus();
    const lines = info.mock.calls.map(([line]) => String(line));
    expect(lines[0]).toMatch(/^\[focus import \+\d+ms\] start: body hasFocus=/);
    expect(lines).toContainEqual(
      expect.stringMatching(/focusin: div\[role=grid\]\.week-grid \(in app-root\)/),
    );
    expect(lines).toContainEqual(
      expect.stringMatching(/focusin: button#save \(OUTSIDE app-root, in dialog\)/),
    );
    // The button's text never reaches the log.
    expect(lines.join('\n')).not.toContain('Speichern');

    // A focused element removed fires no focusout; the look catches the drop.
    document.getElementById('save')?.remove();
    vi.advanceTimersByTime(500);
    expect(info.mock.calls.at(-1)?.[0]).toMatch(/now: body/);

    vi.advanceTimersByTime(1000);
    expect(info.mock.calls.at(-1)?.[0]).toMatch(/end: body/);
    const after = info.mock.calls.length;
    (document.querySelector('[role="grid"]') as HTMLElement).focus();
    expect(info.mock.calls.length).toBe(after);
  });

  it('marks an element outside the document, or inert', () => {
    const lone = document.createElement('button');
    expect(where(lone)).toMatch(/DETACHED/);
    document.body.innerHTML = '<div inert><button id="b"></button></div>';
    expect(where(document.getElementById('b'))).toMatch(/INERT/);
    expect(where(null)).toBe('null');
  });
});
