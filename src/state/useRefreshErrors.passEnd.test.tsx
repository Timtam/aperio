import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { AccountRefreshErrors } from '../api/types';

// The desktop's one live region keeps only the last of two messages written in
// the same moment. After a manual refresh the closing sentence and the warning
// that follows it must therefore be ONE announcement: the caller says both,
// and the sidebar's announce-on-growth stays silent for that publish.

const announce = vi.fn();
let rows: AccountRefreshErrors[] = [];

vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
vi.mock('../intl/language', () => ({ applyStoredLanguage: () => Promise.resolve() }));
vi.mock('../api/client', () => ({ getRefreshErrors: () => Promise.resolve(rows) }));

import {
  notePassEndSpoken,
  resetAnnouncedAccountsForTest,
  useRefreshErrors,
} from './useRefreshErrors';

const failingAuth: AccountRefreshErrors = {
  account_id: 'acc-1',
  auth_suspected: true,
  no_access: false,
  cause: 'auth',
  rank: 1,
  errors: [],
};

describe('the warning after a manual refresh', () => {
  beforeEach(() => {
    resetAnnouncedAccountsForTest();
    announce.mockClear();
    rows = [failingAuth];
  });
  afterEach(() => {
    resetAnnouncedAccountsForTest();
  });

  it('is handed to the caller for its one sentence, not announced on its own', async () => {
    const { result } = renderHook(() => useRefreshErrors({ announceOnGrowth: true }));
    let cause: Awaited<ReturnType<typeof notePassEndSpoken>> = null;
    await act(async () => {
      // "Nothing could be updated" names nobody: the cause is still owed.
      cause = await notePassEndSpoken({
        failing: [
          { account_id: 'acc-1', name: 'Arbeit', cause: 'auth', rank: 1 },
        ],
        all_failed: true,
      });
    });
    expect(cause).toBe('auth');
    // The warning is visible at once, with the sentence.
    await waitFor(() => expect(result.current.errorsByAccount.has('acc-1')).toBe(true));
    expect(announce).not.toHaveBeenCalled();
  });

  it('owes nothing for an account the sentence named', async () => {
    renderHook(() => useRefreshErrors({ announceOnGrowth: true }));
    let cause: Awaited<ReturnType<typeof notePassEndSpoken>> = 'other';
    await act(async () => {
      cause = await notePassEndSpoken({
        failing: [{ account_id: 'acc-1', name: 'Arbeit', cause: 'auth', rank: 1 }],
        all_failed: false,
      });
    });
    expect(cause).toBeNull();
    expect(announce).not.toHaveBeenCalled();
  });
});
