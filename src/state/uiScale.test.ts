import { afterEach, describe, expect, it } from 'vitest';

import {
  applyUiScale,
  DEFAULT_UI_SCALE,
  readUiScale,
  writeUiScale,
} from './uiScale';

afterEach(() => {
  localStorage.clear();
  document.documentElement.style.fontSize = '';
});

describe('readUiScale', () => {
  it('returns the default when nothing is stored', () => {
    expect(readUiScale()).toBe(DEFAULT_UI_SCALE);
  });

  it('reads a stored preset', () => {
    localStorage.setItem('aperio.ui.fontScale', '1.25');
    expect(readUiScale()).toBe(1.25);
  });

  it('clamps an out-of-range stored value', () => {
    localStorage.setItem('aperio.ui.fontScale', '5');
    expect(readUiScale()).toBe(2); // MAX_SCALE
    localStorage.setItem('aperio.ui.fontScale', '0.1');
    expect(readUiScale()).toBe(0.7); // UI_MIN_SCALE
  });

  it('falls back to the default for a non-numeric value', () => {
    localStorage.setItem('aperio.ui.fontScale', 'not-a-number');
    expect(readUiScale()).toBe(DEFAULT_UI_SCALE);
  });
});

describe('applyUiScale', () => {
  it('sets the document root font-size from the 16px base', () => {
    applyUiScale(1);
    expect(document.documentElement.style.fontSize).toBe('16px');
    applyUiScale(1.5);
    expect(document.documentElement.style.fontSize).toBe('24px');
  });

  it('clamps before applying', () => {
    applyUiScale(99);
    expect(document.documentElement.style.fontSize).toBe('32px'); // 16 * MAX_SCALE(2)
  });
});

describe('writeUiScale', () => {
  it('persists and applies in one call', () => {
    writeUiScale(1.1);
    expect(localStorage.getItem('aperio.ui.fontScale')).toBe('1.1');
    expect(document.documentElement.style.fontSize).toBe(`${16 * 1.1}px`);
    expect(readUiScale()).toBe(1.1);
  });
});
