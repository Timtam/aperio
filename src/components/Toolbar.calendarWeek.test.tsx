import { render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { ViewId, WeekStart } from '../state/viewMath';

/**
 * The toolbar's date names the calendar week of the day the views stand on
 * (TODO B15): "KW 45" for the eye, "Kalenderwoche 45" for the ear, which
 * would otherwise hear the letters spelled out. Switched off, the date
 * stands alone.
 */

const h = vi.hoisted(() => ({
  viewState: {
    view: 'day' as ViewId,
    setView: () => {},
    anchor: new Date(2026, 10, 1),
    jumpToToday: () => {},
    goPrev: () => {},
    goNext: () => {},
    weekStartsOn: 1 as WeekStart,
    weekNumbers: true,
  },
  dialogState: {
    openQuickAdd: () => {},
    openQuickAddTask: () => {},
    openSearch: () => {},
    openReminders: () => {},
  },
}));

vi.mock('../state/viewStateContext', () => ({ useViewState: () => h.viewState }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => h.dialogState }));
vi.mock('../state/taskCascadeContext', () => ({
  useTaskCascadeEnabled: () => ({ dayViewMode: 'grid', setDayViewMode: () => {} }),
}));
vi.mock('../state/useFileImport', () => ({ useFileImport: () => ({ importFromDisk: () => {} }) }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('./CacheRefreshIndicator', () => ({ CacheRefreshIndicator: () => null }));
vi.mock('./SyncStatusIndicator', () => ({ SyncStatusIndicator: () => null }));

afterEach(() => {
  document.body.innerHTML = '';
  h.viewState.weekNumbers = true;
  h.viewState.weekStartsOn = 1;
});

async function anchorText() {
  const { Toolbar } = await import('./Toolbar');
  const { container } = render(<Toolbar />);
  const anchor = container.querySelector('.toolbar__anchor');
  if (!anchor) throw new Error('the toolbar shows no date');
  const hidden = anchor.querySelector('[aria-hidden="true"]')?.textContent ?? '';
  const spoken = anchor.querySelector('.sr-only')?.textContent ?? '';
  return { all: anchor.textContent ?? '', hidden, spoken };
}

describe('the toolbar date', () => {
  it('names the calendar week, short for the eye and in words for the ear', async () => {
    const { all, hidden, spoken } = await anchorText();
    // Sunday, 1 November 2026, with the week starting on Monday: week 44.
    expect(all.startsWith('Sonntag, 1. November 2026')).toBe(true);
    expect(hidden).toBe(' · KW 44');
    expect(spoken).toBe(', Kalenderwoche 44');
  });

  it('follows the week start', async () => {
    h.viewState.weekStartsOn = 0;
    const { spoken } = await anchorText();
    expect(spoken).toBe(', Kalenderwoche 45');
  });

  it('stands alone with weeks switched off', async () => {
    h.viewState.weekNumbers = false;
    const { all } = await anchorText();
    expect(all).toBe('Sonntag, 1. November 2026');
  });
});
